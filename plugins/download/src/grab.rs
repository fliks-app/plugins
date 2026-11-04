//! Search, scoring, grabs and the automatic sweeps (`SearchMissing`, `RssSync`).

use crate::config;
use crate::db::{Coverage, DownloadClientRow, IndexerRow, blocklist, clients, history, indexers};
use crate::error::{Error, R};
use crate::torznab::{self, Ids, Kind, Release, Round};
use crate::util::{
    decode_html_entities, decode_uri_component, js_number, now_iso, num, pad2, today,
};
use crate::{client, net, throttle};
use fliks_plugin_sdk::bindings::fliks::plugin::types::{
    AcquisitionEvent, Audience, CandidateRelease, CandidatesQuery, Family, Grabbed, ParamValue,
    ReleaseCoverage, ReleaseTitle, ScoredRelease, SkipReason, Target, UnitRef, WantDecision,
};
use fliks_plugin_sdk::host;
use fliks_plugin_sdk::rt::{self, Budget, FuturesUnordered, StreamExt};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// What a target is acquired as: the download client's category and the log tag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Media {
    Movie,
    Series,
    Ebook,
    Audiobook,
    Comic,
}

impl Media {
    /// `None` for `album`, any kind added later and a book or comic of another family.
    pub fn of(kind: &str, family: Family) -> Option<Self> {
        match (kind, family) {
            ("movie", _) => Some(Self::Movie),
            ("series", _) => Some(Self::Series),
            ("book", Family::Text) => Some(Self::Ebook),
            ("book", Family::Audio) => Some(Self::Audiobook),
            ("comic", Family::Image) => Some(Self::Comic),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Series => "series",
            Self::Ebook => "ebook",
            Self::Audiobook => "audiobook",
            Self::Comic => "comic",
        }
    }
}

/// The family as stored on a history row.
pub fn family_str(f: Family) -> &'static str {
    match f {
        Family::Video => "video",
        Family::Audio => "audio",
        Family::Text => "text",
        Family::Image => "image",
    }
}

pub fn family_of(s: &str) -> Option<Family> {
    [Family::Video, Family::Audio, Family::Text, Family::Image]
        .into_iter()
        .find(|f| family_str(*f) == s)
}

/// Book formats a release title can name, with the family each belongs to and whether the host's
/// ingest accepts files of it (`.wav`, `.wma`, `.azw` and `.djvu` it parses but refuses).
const BOOK_FORMATS: [(&str, Family, bool); 19] = [
    ("m4b", Family::Audio, true),
    ("m4a", Family::Audio, true),
    ("alac", Family::Audio, true),
    ("mp3", Family::Audio, true),
    ("flac", Family::Audio, true),
    ("ogg", Family::Audio, true),
    ("opus", Family::Audio, true),
    ("aac", Family::Audio, true),
    ("wav", Family::Audio, false),
    ("wma", Family::Audio, false),
    ("epub", Family::Text, true),
    ("pdf", Family::Text, true),
    ("azw3", Family::Text, true),
    ("kf8", Family::Text, true),
    ("mobi", Family::Text, true),
    ("fb2", Family::Text, true),
    ("cbz", Family::Text, true),
    ("azw", Family::Text, false),
    ("djvu", Family::Text, false),
];

/// The formats a title names, as (family, accepted).
pub(crate) fn title_formats(title: &str) -> Vec<(Family, bool)> {
    static WORD: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"[a-z0-9]+").unwrap());
    let lower = title.to_lowercase();
    WORD.find_iter(&lower)
        .filter_map(|w| BOOK_FORMATS.iter().find(|(f, ..)| *f == w.as_str()))
        .map(|(_, family, ok)| (*family, *ok))
        .collect()
}

/// The one family a release title names by its format; none when it names both or neither.
pub fn title_family(title: &str) -> Option<Family> {
    let mut found = title_formats(title).into_iter().map(|(f, _)| f);
    let first = found.next()?;
    found.all(|f| f == first).then_some(first)
}

/// False when the title's only formats for `family` are ones the ingest would refuse.
fn ingestable(title: &str, family: Family) -> bool {
    let named: Vec<bool> = title_formats(title)
        .into_iter()
        .filter(|(f, _)| *f == family)
        .map(|(_, ok)| ok)
        .collect();
    named.is_empty() || named.contains(&true)
}

/// What a video target asks for: the whole work, a season or one episode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    Work,
    Season(u32),
    Episode(u32, u32),
}

/// The target's single unit: none is the whole work.
fn unit_of(t: &Target) -> Option<&fliks_plugin_sdk::bindings::fliks::plugin::types::Unit> {
    t.scope.first()
}

/// Video numbers are whole, so the host's `f64` is exact.
pub fn numbers(t: &Target) -> (Option<u32>, Option<u32>) {
    let r = unit_of(t).map(|u| &u.ref_);
    (
        r.and_then(|r| r.group),
        r.and_then(|r| r.number).map(|n| n as u32),
    )
}

/// One indexer query: the numbers release names use, from `search-units`; none is the whole work.
fn asks(t: &Target) -> Vec<Piece> {
    let asks: Vec<Piece> = t
        .search_units
        .iter()
        .filter_map(|u| match (u.group, u.number) {
            (Some(s), Some(e)) => Some(Piece::Episode(s, e as u32)),
            (Some(s), None) => Some(Piece::Season(s)),
            _ => None,
        })
        .collect();
    if asks.is_empty() {
        vec![Piece::Work]
    } else {
        asks
    }
}

/// Every query of a target on one indexer, the results merged without repeats.
async fn search_asks(
    ix: &mut IndexerRow,
    kind: Kind,
    query: &str,
    asks: &[Piece],
    ids: &Ids<'_>,
    round: Round,
) -> Result<Vec<Release>, throttle::Lapsed> {
    let mut out: Vec<Release> = vec![];
    for ask in asks {
        let found = match *ask {
            Piece::Episode(s, e) => torznab::search_series(ix, kind, query, s, e, ids, round).await,
            Piece::Season(s) => torznab::search_season_pack(ix, kind, query, s, ids, round).await,
            Piece::Work => torznab::search_movie(ix, kind, query, ids, round).await,
        };
        // A later query that runs out of budget keeps what the earlier ones found.
        let found = match found {
            Err(lapsed) if out.is_empty() => return Err(lapsed),
            Err(_) => break,
            Ok(found) => found,
        };
        for r in found {
            if !out.iter().any(|o| o.download_url == r.download_url) {
                out.push(r);
            }
        }
    }
    Ok(out)
}

pub fn piece(t: &Target) -> Piece {
    match numbers(t) {
        (Some(s), Some(e)) => Piece::Episode(s, e),
        (Some(s), None) => Piece::Season(s),
        (None, _) => Piece::Work,
    }
}

fn unit_ref_of(t: &Target) -> Option<UnitRef> {
    unit_of(t).map(|u| u.ref_.clone())
}

fn group_id(t: &Target) -> Option<u32> {
    unit_of(t).and_then(|u| u.group_id)
}

fn item_id(t: &Target) -> Option<u32> {
    unit_of(t).and_then(|u| u.item_id)
}

pub struct Ranked {
    pub raw: Release,
    pub scored: ScoredRelease,
}

pub fn coverage_json(c: &ReleaseCoverage) -> Value {
    let span = |s: &Option<fliks_plugin_sdk::bindings::fliks::plugin::types::Span>| {
        s.as_ref().map_or(
            Value::Null,
            |s| json!({ "from": num(s.first), "to": num(s.last) }),
        )
    };
    json!({ "seasons": span(&c.groups), "episodes": span(&c.units), "completeSeries": c.complete })
}

/// Unparseable coverage (no season, not a complete series) is stored as unknown.
pub fn coverage_columns(c: Option<&ReleaseCoverage>) -> Coverage {
    match c {
        Some(c) if c.complete || c.groups.is_some() => Coverage {
            cover_season_from: c.groups.as_ref().map(|s| s.first as i64),
            cover_season_to: c.groups.as_ref().map(|s| s.last as i64),
            cover_episode_from: c.units.as_ref().map(|s| s.first as i64),
            cover_episode_to: c.units.as_ref().map(|s| s.last as i64),
            cover_complete_series: Some(c.complete),
        },
        _ => Coverage::default(),
    }
}

/// The indexer fields, then core's verdict; the source is `sourceId`/`sourceName` on the wire.
pub fn to_wire(r: &Ranked) -> Value {
    let (raw, s) = (&r.raw, &r.scored);
    let mut o = Map::new();
    o.insert("title".into(), json!(raw.title));
    o.insert("downloadUrl".into(), json!(raw.download_url));
    if let Some(i) = &raw.info_url {
        o.insert("infoUrl".into(), json!(i));
    }
    o.insert("size".into(), json!(raw.size));
    o.insert("seeders".into(), json!(raw.seeders));
    o.insert("leechers".into(), json!(raw.leechers));
    o.insert("publishDate".into(), json!(raw.publish_date));
    o.insert("freeleech".into(), json!(raw.freeleech));
    o.insert(
        "downloadVolumeFactor".into(),
        num(raw.download_volume_factor),
    );
    o.insert("id".into(), json!(s.id));
    o.insert("qualityId".into(), json!(s.quality_id));
    o.insert("qualityName".into(), json!(s.quality_name));
    o.insert("rank".into(), json!(s.rank));
    o.insert("allowed".into(), json!(s.allowed));
    o.insert("customFormatScore".into(), json!(s.custom_format_score));
    o.insert("blocklisted".into(), json!(s.blocklisted));
    let languages: Vec<Value> = s
        .languages
        .iter()
        .map(|l| json!({"id": l.id, "name": l.name}))
        .collect();
    o.insert("languages".into(), json!(languages));
    o.insert("languageAllowed".into(), json!(s.language_allowed));
    o.insert("isFullSeason".into(), json!(s.whole_group));
    if let Some(c) = &s.coverage {
        o.insert("coverage".into(), coverage_json(c));
    }
    o.insert(
        "sizeDeviation".into(),
        s.size_deviation.map_or(Value::Null, num),
    );
    o.insert("videoCodec".into(), json!(s.video_codec));
    let rejections: Vec<Value> = s
        .rejections
        .iter()
        .map(|r| {
            let mut v = json!({ "code": r.code });
            if !r.params.is_empty() {
                let params: Map<String, Value> = r
                    .params
                    .iter()
                    .map(|(k, p)| {
                        let v = match p {
                            ParamValue::Number(n) => num(*n),
                            ParamValue::Text(t) => json!(t),
                        };
                        (k.clone(), v)
                    })
                    .collect();
                v["params"] = Value::Object(params);
            }
            v
        })
        .collect();
    o.insert("rejections".into(), Value::Array(rejections));
    o.insert("sourceId".into(), json!(raw.indexer_id));
    o.insert("sourceName".into(), json!(raw.indexer_name));
    Value::Object(o)
}

/// One `releases.score` request per release; `blocked` holds the titles on this plugin's blocklist.
pub fn score_request(
    releases: &[Release],
    indexers: &[IndexerRow],
    blocked: &HashSet<String>,
) -> Vec<CandidateRelease> {
    releases
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let settings = indexers
                .iter()
                .find(|ix| ix.id == r.indexer_id)
                .map(|ix| &ix.settings);
            let min = settings.map_or(f64::NAN, |s| js_number(s.get("minSeeders")));
            CandidateRelease {
                id: i.to_string(),
                title: r.title.clone(),
                size: r.size.max(0) as u64,
                seeders: r.seeders.clamp(0, u32::MAX as i64) as u32,
                leechers: r.leechers.clamp(0, u32::MAX as i64) as u32,
                publish_date: r
                    .publish_date
                    .clone()
                    .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".into()),
                flags: None,
                freeleech: Some(r.freeleech),
                download_volume_factor: Some(r.download_volume_factor),
                source_ref: r.download_url.clone(),
                min_seeders: (min.is_finite() && min > 0.0).then_some(min.ceil() as u32),
                unknown_language_iso_code: settings
                    .and_then(|s| s.get("unknownLanguageIsoCode"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                blocked: blocked.contains(&r.title),
            }
        })
        .collect()
}

fn rank(target: &Target, indexers: &[IndexerRow], releases: &[Release]) -> R<Vec<Ranked>> {
    if releases.is_empty() {
        return Ok(vec![]);
    }
    let blocked =
        blocklist::blocked(&releases.iter().map(|r| r.title.clone()).collect::<Vec<_>>())?;
    let scored = host::releases_score(
        target.media_id,
        Some(target.family),
        book_language(target),
        unit_ref_of(target).as_ref(),
        &score_request(releases, indexers, &blocked),
    )?;
    Ok(scored
        .into_iter()
        .filter_map(|s| {
            let raw = releases.get(s.id.parse::<usize>().ok()?)?.clone();
            Some(Ranked { raw, scored: s })
        })
        .collect())
}

/// Every release without a rejection, best first, capped: a dead link costs a fetch each.
/// A season target takes packs only; its loose episodes are grabbed as episode targets.
pub fn pick(sorted: &[Ranked], target: &Target) -> Vec<usize> {
    let season_only = is_season(target);
    let book = target.kind == "book";
    let comic = target.kind == "comic";
    match &target.want {
        Some(w) if w.decision != WantDecision::Skip => sorted
            .iter()
            .enumerate()
            .filter(|(_, r)| r.scored.rejections.is_empty())
            .filter(|(_, r)| !comic || r.scored.allowed)
            .filter(|(_, r)| {
                !book
                    || (r.scored.allowed
                        && r.scored.language_allowed
                        && title_family(&r.raw.title).is_none_or(|f| f == target.family)
                        && ingestable(&r.raw.title, target.family))
            })
            .filter(|(_, r)| !season_only || r.scored.whole_group)
            .map(|(i, _)| i)
            .take(3)
            .collect(),
        _ => vec![],
    }
}

fn is_season(t: &Target) -> bool {
    matches!(piece(t), Piece::Season(_))
}

fn is_special(t: &Target) -> bool {
    numbers(t).0 == Some(0)
}

/// The episode title of a special, the only thing to search it by.
fn special_title(t: &Target) -> Option<&str> {
    unit_of(t)?
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Movies get their year; a special is searched by its own title.
pub fn search_query(t: &Target, custom: Option<&str>) -> String {
    if let Some(c) = custom.map(str::trim).filter(|c| !c.is_empty()) {
        return c.to_owned();
    }
    if piece(t) == Piece::Work
        && t.kind == "movie"
        && let Some(y) = t.year.filter(|y| *y != 0)
    {
        return format!("{} {y}", t.search_title);
    }
    if is_special(t)
        && let Some(title) = special_title(t)
    {
        return format!("{} {title}", t.search_title);
    }
    t.search_title.clone()
}

/// The queries that find one comic issue: the series and the number as printed in release names
/// (`003`), then bare (`3`); an annual or a special keeps its word. A custom query stands alone.
pub fn issue_queries(t: &Target, custom: Option<&str>) -> Vec<String> {
    if let Some(c) = custom.map(str::trim).filter(|c| !c.is_empty()) {
        return vec![c.to_owned()];
    }
    let Some(r) = unit_of(t).map(|u| &u.ref_) else {
        return vec![t.search_title.clone()];
    };
    let label = r.label.clone().unwrap_or_else(|| {
        r.number
            .map_or_else(String::new, |n| crate::util::num(n).to_string())
    });
    let (word, number) = match label.split_once(' ') {
        Some((w, n)) => (format!("{w} "), n.to_owned()),
        None => (String::new(), label),
    };
    let padded = match number.split_once('.') {
        Some((i, f)) if i.len() < 3 && !i.starts_with('-') => format!("{i:0>3}.{f}"),
        None if number.len() < 3 && !number.starts_with('-') => format!("{number:0>3}"),
        _ => number.clone(),
    };
    let mut out = vec![];
    for n in [padded, number] {
        let q = format!("{} {word}{n}", t.search_title);
        if !out.contains(&q) {
            out.push(q);
        }
    }
    out
}

/// Who a streamed search reports to.
pub struct Stream {
    pub user_id: u32,
    pub search_id: String,
}

struct Streamer<'a> {
    to: &'a Stream,
    entries: Vec<(i64, String, &'static str)>,
}

impl Streamer<'_> {
    fn emit(&self, kind: &str, extra: Option<Vec<Value>>) {
        let indexers: Vec<Value> = self
            .entries
            .iter()
            .map(|(id, name, state)| json!({ "id": id, "name": name, "state": state }))
            .collect();
        let mut payload = json!({ "searchId": self.to.search_id, "indexers": indexers });
        if let Some(r) = extra {
            payload["releases"] = Value::Array(r);
        }
        if let Err(e) =
            host::events_emit_own(kind, &payload.to_string(), Audience::User(self.to.user_id))
        {
            warn!("search stream {kind} dropped: {}", e.message);
        }
    }

    fn set(&mut self, ix: &IndexerRow, state: &'static str) {
        match self.entries.iter_mut().find(|e| e.0 == ix.id) {
            Some(e) => e.2 = state,
            None => self.entries.push((ix.id, ix.name.clone(), state)),
        }
    }
}

/// The search budget as the admin set it, clamped to 5..120 s, within the export's; the fetch
/// gets 5/6 of it.
pub fn round() -> Round {
    let budget = config::search_budget_ms();
    let fetch = Duration::from_millis(((budget as f64) * 5.0 / 6.0).round() as u64);
    Round {
        budget: Budget::new(net::budget().cap(Duration::from_millis(budget))),
        fetch_timeout: fetch,
    }
}

/// Fans out to every ready indexer at once, streams each answer, then scores the union in one
/// `releases.score` call.
pub fn search_scored(
    target: &Target,
    kind: Kind,
    custom: Option<&str>,
    stream: Option<&Stream>,
) -> R<Vec<Ranked>> {
    let all = indexers::list_enabled()?;
    if all.is_empty() {
        return Ok(vec![]);
    }
    let round = round();
    let ids = Ids::of(&target.external_ids);
    let query = search_query(target, custom);
    let book = target.kind == "book";
    let comic = target.kind == "comic";
    let issues = &issue_queries(target, custom);
    let author = author_of(target).filter(|_| custom.is_none_or(|c| c.trim().is_empty()));

    if is_special(target) && special_title(target).is_none() {
        let what = match piece(target) {
            Piece::Episode(_, e) => format!("S00E{}", pad2(e)),
            _ => "S00 pack".to_owned(),
        };
        info!(
            "Search skipped for \"{}\" {what}: no episode title to search a special by",
            target.title
        );
        return Ok(vec![]);
    }

    let mut ready = throttle::filter_ready(&all)?;
    if ready.is_empty() {
        warn!(
            "{}: every indexer is in cooldown — skipping this run",
            target.title
        );
    }
    let mut streamer = stream.map(|to| Streamer {
        to,
        entries: vec![],
    });
    if let Some(s) = &mut streamer {
        for ix in &ready {
            s.set(ix, "pending");
        }
        for ix in all.iter().filter(|ix| !ready.iter().any(|r| r.id == ix.id)) {
            s.set(ix, "skipped");
        }
        s.emit("search.state", None);
    }
    if ready.is_empty() {
        return Ok(vec![]);
    }

    let mut raw: Vec<Release> = vec![];
    let (query, ids) = (&query, &ids);
    let asks = asks(target);
    let asks = &asks;
    rt::block_on(async {
        let mut pending: FuturesUnordered<_> = ready
            .iter_mut()
            .map(|ix| async move {
                let out = if book {
                    torznab::search_book(ix, kind, target.family, author, query, round).await
                } else if comic {
                    torznab::search_comic(ix, kind, issues, round).await
                } else {
                    search_asks(ix, kind, query, asks, ids, round).await
                };
                (ix, out)
            })
            .collect();
        while let Some((ix, out)) = pending.next().await {
            match out {
                Ok(found) => {
                    let added = !found.is_empty();
                    raw.extend(found);
                    if let Some(s) = &mut streamer {
                        s.set(ix, "done");
                        if added {
                            match rank(target, &all, &raw) {
                                Ok(r) => {
                                    s.emit("search.partial", Some(r.iter().map(to_wire).collect()))
                                }
                                Err(e) => warn!("search stream re-rank failed: {e}"),
                            }
                        } else {
                            s.emit("search.state", None);
                        }
                    }
                }
                Err(throttle::Lapsed) => {
                    warn!(
                        "[{}] still searching after {}ms — dropped from this round",
                        ix.name,
                        config::search_budget_ms()
                    );
                    if let Some(s) = &mut streamer {
                        s.set(ix, "failed");
                        s.emit("search.state", None);
                    }
                }
            }
        }
    });
    rank(target, &all, &raw)
}

/// The first credited author of a book.
fn author_of(t: &Target) -> Option<&str> {
    t.creators
        .iter()
        .find(|c| c.role == "author")
        .map(|c| c.name.trim())
        .filter(|n| !n.is_empty())
}

/// The language a host call carries: a book's want language, none for video (the host refuses one).
pub fn book_language(t: &Target) -> Option<&str> {
    matches!(t.family, Family::Text | Family::Audio)
        .then_some(t.language.as_deref())
        .flatten()
}

/// No family: the host answers the media's first want, whose family the target carries.
fn fetch_target(
    media_id: u32,
    family: Option<Family>,
    language: Option<&str>,
    group_id: Option<u32>,
    item_id: Option<u32>,
) -> R<Target> {
    host::media_acquisition_context(media_id, family, language, group_id, item_id)?.ok_or_else(
        || Error::grab_with("download.grab.errors.media_not_found", media_id.to_string()),
    )
}

/// A target of a kind this plugin does not acquire is refused, not searched. `language` picks a
/// book's want; none takes the first unsatisfied one.
fn load_target(
    media_id: u32,
    group_id: Option<u32>,
    item_id: Option<u32>,
    language: Option<&str>,
) -> R<(Target, Media)> {
    let mut target = fetch_target(media_id, None, language, group_id, item_id)?;
    if target.kind == "book"
        && language.is_none()
        && target
            .want
            .as_ref()
            .is_some_and(|w| w.decision == WantDecision::Skip)
    {
        let page = host::acquisition_candidates(&CandidatesQuery {
            kind: Some("book".into()),
            family: None,
            media_ids: Some(vec![media_id]),
            available_on: today(),
            limit: 50,
            cursor: None,
        })?;
        if let Some(next) = page.items.into_iter().find(|t| {
            t.want
                .as_ref()
                .is_some_and(|w| w.decision != WantDecision::Skip)
        }) {
            target = next;
        }
    }
    let Some(media) = Media::of(&target.kind, target.family) else {
        info!(
            "Media #{media_id} \"{}\" is a {}: this plugin acquires movies, series, books and comics only",
            target.title, target.kind
        );
        return Err(Error::grab("download.grab.errors.unsupported_kind"));
    };
    Ok((target, media))
}

pub fn search_releases(
    media_id: u32,
    group_id: Option<u32>,
    item_id: Option<u32>,
    language: Option<&str>,
    custom: Option<&str>,
    stream: Option<&Stream>,
) -> R<Vec<Ranked>> {
    let (target, _) = load_target(media_id, group_id, item_id, language)?;
    let Some(want) = &target.want else {
        return Err(Error::grab("download.grab.errors.unprofiled"));
    };
    let scored = search_scored(&target, Kind::Manual, custom, stream)?;
    let satisfied = if want.decision == WantDecision::Skip {
        " (profile already satisfied)"
    } else {
        ""
    };
    info!(
        "Search #{media_id} \"{}\"{satisfied} q=\"{}\" → {} result(s)",
        target.title,
        search_query(&target, custom),
        scored.len()
    );
    Ok(scored)
}

pub struct Manual {
    pub download_url: String,
    pub source_title: Option<String>,
    pub indexer_id: Option<i64>,
    pub info_url: Option<String>,
    pub force: bool,
}

fn title_from_url(url: &str) -> String {
    if url.starts_with("magnet:") {
        static DN: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)[?&]dn=([^&]+)").unwrap());
        if let Some(c) = DN.captures(url) {
            return decode_uri_component(&c[1].replace('+', " "))
                .unwrap_or_else(|| c[1].to_owned());
        }
    }
    url.chars().take(240).collect()
}

struct GrabArgs<'a> {
    media_id: u32,
    client: &'a DownloadClientRow,
    media: Media,
    family: Family,
    language: Option<String>,
    label: &'a str,
    source_title: String,
    download_url: String,
    quality: String,
    size: i64,
    info_url: Option<String>,
    indexer_id: Option<i64>,
    grab_source: &'static str,
    unit: Option<UnitRef>,
    group_id: Option<u32>,
    item_id: Option<u32>,
    coverage: Option<ReleaseCoverage>,
    reject_if_present: bool,
}

/// Adds the torrent, records the row, publishes the grab and hands the notification to the
/// task lane (its senders must not hold the user's click).
fn grab_and_record(a: GrabArgs) -> R<String> {
    info!(
        "AutoGrab[{}]: sending \"{}\" to the download client — {}",
        a.media.as_str(),
        a.source_title,
        fliks_plugin_sdk::log::redact_url(&a.download_url)
    );
    let hash = client::add(
        a.client,
        &a.download_url,
        a.media.as_str(),
        a.reject_if_present,
    )?;
    let cov = coverage_columns(a.coverage.as_ref());
    if let Some(existing) = history::latest_by_hash(&hash)?
        && existing.media_id == Some(a.media_id.into())
        && existing.is_live()
    {
        info!(
            "AutoGrab[{}]: \"{}\" is already in flight as history #{}",
            a.media.as_str(),
            a.source_title,
            existing.id
        );
        history::fill_coverage(existing.id, &cov)?;
        return Ok(hash);
    }
    let row = history::insert_grab(&history::Grab {
        source_title: decode_html_entities(&a.source_title),
        quality: a.quality.clone(),
        family: Some(family_str(a.family)),
        language: a.language.clone(),
        torrent_hash: Some(hash.clone()).filter(|h| !h.is_empty()),
        size: Some(a.size).filter(|s| *s != 0),
        info_url: a.info_url.filter(|u| !u.is_empty()),
        grab_source: a.grab_source,
        media_id: a.media_id.into(),
        item_id: a.item_id.map(Into::into),
        season_id: a.group_id.map(Into::into),
        indexer_id: a.indexer_id,
        download_client_id: Some(a.client.id),
        coverage: cov,
    })?;
    let ev = AcquisitionEvent::Grabbed(Grabbed {
        media_id: a.media_id,
        family: a.family,
        unit: a.unit,
    });
    if let Err(e) = host::events_publish(&[ev]) {
        warn!("AutoGrab: events.publish failed: {}", e.message);
    }
    let payload = json!({ "title": a.label, "quality": a.quality, "sourceTitle": a.source_title });
    if let Err(e) =
        fliks_plugin_sdk::scheduled::schedule(&format!("grab-started:{}", row.id), 0, &payload)
    {
        warn!("AutoGrab: notifications.dispatch failed: {}", e.message);
    }
    info!(
        "AutoGrab[{}]: grabbed \"{}\" for \"{}\"",
        a.media.as_str(),
        a.source_title,
        a.label
    );
    Ok(hash)
}

fn pick_client() -> R<Option<DownloadClientRow>> {
    Ok(clients::list_enabled()?.into_iter().find(client::supports))
}

fn args_for<'a>(
    t: &'a Target,
    media: Media,
    client: &'a DownloadClientRow,
    r: &Ranked,
    grab_source: &'static str,
) -> GrabArgs<'a> {
    GrabArgs {
        media_id: t.media_id,
        client,
        media,
        family: t.family,
        language: t.language.clone(),
        label: &t.title,
        source_title: r.raw.title.clone(),
        download_url: r.raw.download_url.clone(),
        quality: r.scored.quality_name.clone(),
        size: r.raw.size,
        info_url: r.raw.info_url.clone(),
        indexer_id: Some(r.raw.indexer_id).filter(|id| *id != 0),
        grab_source,
        unit: unit_ref_of(t),
        group_id: group_id(t),
        item_id: item_id(t),
        coverage: r.scored.coverage,
        reject_if_present: false,
    }
}

/// A manual URL is scored alone; a missing one auto-picks from a fresh search.
pub fn grab_release(
    media_id: u32,
    group_id: Option<u32>,
    item_id: Option<u32>,
    language: Option<&str>,
    manual: Option<Manual>,
) -> R<Value> {
    let (target, media) = load_target(media_id, group_id, item_id, language)?;
    let client =
        pick_client()?.ok_or_else(|| Error::grab("download.grab.errors.no_download_client"))?;
    if target.want.is_none() {
        return Err(Error::grab("download.grab.errors.unprofiled"));
    }

    if let Some(m) = manual {
        let source_title = m
            .source_title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map_or_else(|| title_from_url(&m.download_url), str::to_owned);
        // The route names no family: a book release is grabbed for the family its format names.
        let (target, media) = title_family(&source_title)
            .filter(|f| target.kind == "book" && *f != target.family)
            .and_then(|f| {
                let ctx =
                    |l| host::media_acquisition_context(media_id, Some(f), l, group_id, item_id);
                ctx(book_language(&target))
                    .ok()?
                    .or_else(|| ctx(None).ok()?)
            })
            .and_then(|t| Some((Media::of(&t.kind, t.family)?, t)))
            .map_or((target, media), |(media, t)| (t, media));
        let fabricated = Release {
            title: source_title.clone(),
            download_url: m.download_url.clone(),
            info_url: None,
            indexer_id: 0,
            indexer_name: String::new(),
            size: 0,
            seeders: 0,
            leechers: 0,
            publish_date: Some(now_iso()),
            freeleech: false,
            download_volume_factor: 1.0,
        };
        let scored = rank(&target, &[], std::slice::from_ref(&fabricated))?
            .into_iter()
            .next()
            .ok_or_else(|| Error::grab("download.grab.errors.no_eligible_release"))?;
        if scored.scored.blocklisted {
            return Err(Error::grab_with(
                "download.grab.errors.blocklisted",
                source_title,
            ));
        }
        if !scored.scored.allowed && !m.force {
            return Err(Error::grab_with(
                "download.grab.errors.quality_not_allowed",
                scored.scored.quality_name,
            ));
        }
        info!(
            "Grab #{media_id} \"{}\" — manual URL{}",
            target.title,
            if scored.scored.allowed {
                ""
            } else {
                " (forced)"
            }
        );
        let mut a = args_for(&target, media, &client, &scored, "manual");
        a.info_url = m.info_url;
        a.indexer_id = m.indexer_id;
        a.source_title = source_title;
        let hash = grab_and_record(a)?;
        return Ok(json!({ "torrentHash": hash }));
    }

    info!("Grab #{media_id} \"{}\" — auto-pick", target.title);
    let scored = search_scored(&target, Kind::Auto, None, None)?;
    let picks = pick(&scored, &target);
    if picks.is_empty() && !is_season(&target) {
        return Err(Error::grab("download.grab.errors.no_eligible_release"));
    }
    let mut last = None;
    for i in picks {
        let c = &scored[i];
        match grab_and_record(args_for(&target, media, &client, c, "manual")) {
            Ok(hash) => return Ok(json!({ "torrentHash": hash })),
            Err(Error::Unobtainable(m)) => {
                warn!(
                    "Grab #{media_id}: \"{}\" is unobtainable ({m}) — trying the next release",
                    c.raw.title
                );
                last = Some(m);
            }
            Err(e) => return Err(e),
        }
    }
    if is_season(&target) {
        let why = if last.is_some() {
            "every season pack was unobtainable"
        } else {
            "no eligible season pack"
        };
        return grab_season_episodes(&target, why);
    }
    Err(Error::Grab {
        key: "download.grab.errors.releases_unobtainable",
        detail: last,
    })
}

/// The still-wanted episodes of the target's season, in airing order.
fn season_episodes(target: &Target) -> R<Vec<Target>> {
    let mut out = vec![];
    let mut cursor = None;
    loop {
        let page = host::acquisition_candidates(&CandidatesQuery {
            kind: None,
            family: Some(Family::Video),
            media_ids: Some(vec![target.media_id]),
            available_on: today(),
            limit: 200,
            cursor: cursor.take(),
        })?;
        for it in page.items {
            if matches!(piece(&it), Piece::Episode(..))
                && group_id(&it) == group_id(target)
                && it
                    .want
                    .as_ref()
                    .is_some_and(|w| w.decision != WantDecision::Skip)
            {
                out.push(it);
            }
        }
        match page.cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    out.sort_by_key(|t| numbers(t).1);
    Ok(out)
}

fn grab_season_episodes(target: &Target, why: &str) -> R<Value> {
    let season = format!("S{}", pad2(numbers(target).0.unwrap_or(0)));
    let episodes = season_episodes(target)?;
    info!(
        "Grab #{} \"{}\" {season} — {why}, grabbing {} episode(s) individually",
        target.media_id,
        target.title,
        episodes.len()
    );
    let mut hashes = vec![];
    for ep in &episodes {
        let (Some(g), Some(i), Some(e)) = (group_id(ep), item_id(ep), numbers(ep).1) else {
            continue;
        };
        match grab_release(target.media_id, Some(g), Some(i), None, None) {
            Ok(v) => hashes.push(v["torrentHash"].clone()),
            Err(err) => warn!(
                "Grab #{} \"{}\" {season}E{} skipped — {err}",
                target.media_id,
                target.title,
                pad2(e)
            ),
        }
    }
    if hashes.is_empty() {
        return Err(Error::grab("download.grab.errors.no_eligible_release"));
    }
    Ok(json!({ "torrentHash": hashes[0], "torrentHashes": hashes }))
}

/// Season and episode targets are judged on what pending grabs cover; anything else on any
/// pending grab of the media.
fn pending(target: &Target) -> R<Option<String>> {
    let (season, episode) = numbers(target);
    match (season, group_id(target)) {
        (Some(s), Some(season_id)) => history::find_pending_covering(&history::Covering {
            media_id: target.media_id.into(),
            season_id: season_id.into(),
            season_number: s.into(),
            episode_number: episode.map(Into::into),
            episode_tag_pattern: episode.map(|e| format!("%S{}E{}%", pad2(s), pad2(e))),
        }),
        _ if target.kind == "comic" && item_id(target).is_some() => history::find_pending_for_item(
            target.media_id.into(),
            item_id(target).unwrap_or(0).into(),
        )
        .map(|h| h.map(|h| h.source_title)),
        _ => Ok(history::find_pending_for_media(
            target.media_id.into(),
            family_str(target.family),
            book_language(target),
        )?
        .map(|h| h.source_title)),
    }
}

/// Search, pick and grab one automatic target; false when nothing was grabbed.
fn try_auto_grab(target: &Target, client: &DownloadClientRow) -> R<bool> {
    let Some(media) = Media::of(&target.kind, target.family) else {
        info!(
            "AutoGrab[{}]: \"{}\" skipped — this plugin acquires movies, series, books and comics only",
            target.kind, target.title
        );
        return Ok(false);
    };
    let kind = media.as_str();
    let skip = |reason: String| {
        info!("AutoGrab[{kind}]: \"{}\" skipped — {reason}", target.title);
        Ok(false)
    };
    match &target.want {
        None => return skip("no quality/language profile on media".into()),
        Some(w) if w.decision == WantDecision::Skip => {
            return skip("media already satisfies its profile".into());
        }
        _ => {}
    }
    if let Some(blocking) = pending(target)? {
        return skip(format!("a grab is already pending (\"{blocking}\")"));
    }
    let scored = search_scored(target, Kind::Auto, None, None)?;
    if scored.is_empty() {
        return skip("no releases returned by indexers".into());
    }
    let picks = pick(&scored, target);
    if picks.is_empty() {
        let sample: Vec<String> = scored
            .iter()
            .take(3)
            .map(|r| {
                let codes: Vec<&str> = r
                    .scored
                    .rejections
                    .iter()
                    .map(|x| x.code.as_str())
                    .collect();
                let tail = if codes.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", codes.join(", "))
                };
                format!("\"{}\" → rank {}{tail}", r.raw.title, r.scored.rank)
            })
            .collect();
        let top = if sample.is_empty() {
            String::new()
        } else {
            format!(" — top: {}", sample.join(" | "))
        };
        let what = if is_season(target) {
            "season pack"
        } else {
            "release"
        };
        return skip(format!(
            "no eligible {what} ({} checked){top}",
            scored.len()
        ));
    }
    for i in picks {
        let mut a = args_for(target, media, client, &scored[i], "auto");
        a.reject_if_present = true;
        match grab_and_record(a) {
            Ok(_) => return Ok(true),
            Err(e) => warn!(
                "AutoGrab[{kind}]: grab failed for \"{}\": {e}",
                target.title
            ),
        }
    }
    skip("every eligible release was unobtainable".into())
}

/// One target's failure is logged and skipped: it must not end the sweep of the rest.
fn auto_grab_or_skip(target: &Target, client: &DownloadClientRow) -> bool {
    try_auto_grab(target, client).unwrap_or_else(|e| {
        warn!(
            "AutoGrab[{}]: \"{}\" failed, the sweep goes on — {e}",
            target.kind, target.title
        );
        false
    })
}

/// Candidates arrive sorted by this tuple; a season pack sorts before its episodes. The last
/// field is a book want's position among those of its media, the host's order of (family, language).
pub type Key = (u32, Option<u32>, Option<u32>, u8);

pub fn key_of(t: &Target, ordinal: u8) -> Key {
    (t.media_id, group_id(t), item_id(t), ordinal)
}

/// Whether a target still has to run in a sweep resumed after `resume`.
pub fn after(key: Key, resume: Option<Key>) -> bool {
    resume.is_none_or(|r| key > r)
}

/// `Ok(Some(key))`: the time budget ran out after `key`; the caller resumes from there.
pub fn search_missing(media_ids: Option<Vec<u32>>, resume: Option<Key>) -> R<Option<Key>> {
    if config::auto_acquisition_paused("SearchMissing").map_err(Error::Other)? {
        return Ok(None);
    }
    let Some(client) = pick_client()? else {
        warn!("SearchMissing: no enabled download client configured");
        return Ok(None);
    };
    let (mut cursor, mut count, mut done, mut current) = (None, 0, resume, None);
    let mut prev: Option<(u32, Option<u32>, Option<u32>)> = None;
    let mut ordinal = 0u8;
    let mut packs: Vec<u32> = vec![];
    loop {
        let page = host::acquisition_candidates(&CandidatesQuery {
            kind: None,
            family: None,
            media_ids: media_ids.clone(),
            available_on: today(),
            limit: 200,
            cursor: cursor.take(),
        })?;
        for target in &page.items {
            count += 1;
            let id = (target.media_id, group_id(target), item_id(target));
            ordinal = if prev == Some(id) {
                ordinal.saturating_add(1)
            } else {
                0
            };
            prev = Some(id);
            let key = key_of(target, ordinal);
            if !after(key, resume) {
                continue;
            }
            if current != Some(key) {
                done = current.or(done);
                current = Some(key);
            }
            if net::budget().remaining() < Duration::from_millis(config::search_budget_ms()) {
                info!("SearchMissing: out of time after {count} candidate(s), resuming later");
                return Ok(done);
            }
            if target
                .want
                .as_ref()
                .is_none_or(|w| w.decision == WantDecision::Skip)
            {
                continue;
            }
            if matches!(piece(target), Piece::Episode(..))
                && let Some(g) = group_id(target)
                && packs.contains(&g)
            {
                continue;
            }
            let grabbed = auto_grab_or_skip(target, &client);
            if grabbed
                && is_season(target)
                && let Some(g) = group_id(target)
            {
                packs.push(g);
            }
        }
        match page.cursor {
            Some(c)
                if !config::auto_acquisition_paused("SearchMissing").map_err(Error::Other)? =>
            {
                cursor = Some(c)
            }
            _ => break,
        }
    }
    info!("SearchMissing: {count} candidate(s) checked");
    Ok(None)
}

pub struct Match {
    pub skip: Option<SkipReason>,
    pub media_id: u32,
    pub season_number: Option<u32>,
    pub episode_number: Option<u32>,
    /// The unit as the host sent it: a comic issue's number is decimal and its label says its kind.
    pub unit: Option<UnitRef>,
    pub coverage: Option<ReleaseCoverage>,
}

/// One `releases.match` call; `None` for a title core found no media for.
pub fn identify(titles: &[String]) -> R<HashMap<String, Option<Match>>> {
    let mut out = HashMap::new();
    if titles.is_empty() {
        return Ok(out);
    }
    let now = now_iso();
    let req: Vec<ReleaseTitle> = titles
        .iter()
        .enumerate()
        .map(|(i, t)| ReleaseTitle {
            id: i.to_string(),
            title: t.clone(),
            publish_date: now.clone(),
        })
        .collect();
    for r in host::releases_match(&req, None)? {
        let Some(title) = r.id.parse::<usize>().ok().and_then(|i| titles.get(i)) else {
            continue;
        };
        let m = r.media_id.map(|media_id| Match {
            skip: r.skip_reason,
            media_id,
            season_number: r.unit.as_ref().and_then(|u| u.group),
            episode_number: r.unit.as_ref().and_then(|u| u.number).map(|n| n as u32),
            unit: r.unit.clone(),
            coverage: r.coverage,
        });
        out.insert(title.clone(), m);
    }
    Ok(out)
}

/// Whether a comic target is the issue a match names: the same decimal number and printed label.
pub fn same_issue(t: &Target, u: &UnitRef) -> bool {
    let Some(r) = unit_of(t).map(|u| &u.ref_) else {
        return false;
    };
    let label = |l: &Option<String>| l.as_deref().map(|l| l.trim().to_lowercase());
    r.number == u.number && label(&r.label) == label(&u.label)
}

/// The item of the wanted comic issue a match names, for a torrent the plugin did not grab itself.
pub fn comic_item(media_id: u32, unit: &UnitRef) -> R<Option<i64>> {
    let page = host::acquisition_candidates(&CandidatesQuery {
        kind: Some("comic".into()),
        family: Some(Family::Image),
        media_ids: Some(vec![media_id]),
        available_on: today(),
        limit: 500,
        cursor: None,
    })?;
    Ok(page
        .items
        .iter()
        .find(|it| same_issue(it, unit))
        .and_then(item_id)
        .map(i64::from))
}

/// Group and item ids for a match carrying only numbers, when core still lists them as open.
pub fn group_item_ids(
    media_id: u32,
    season: Option<u32>,
    episode: Option<u32>,
) -> R<(Option<i64>, Option<i64>)> {
    let Some(sn) = season else {
        return Ok((None, None));
    };
    let page = host::acquisition_candidates(&CandidatesQuery {
        kind: None,
        family: Some(Family::Video),
        media_ids: Some(vec![media_id]),
        available_on: today(),
        limit: 500,
        cursor: None,
    })?;
    let in_season: Vec<&Target> = page
        .items
        .iter()
        .filter(|it| numbers(it).0 == Some(sn))
        .collect();
    if let Some(en) = episode
        && let Some(it) = in_season.iter().find(|it| numbers(it).1 == Some(en))
        && let (Some(g), Some(i)) = (group_id(it), item_id(it))
    {
        return Ok((Some(g.into()), Some(i.into())));
    }
    Ok((
        in_season
            .first()
            .and_then(|it| group_id(it))
            .map(Into::into),
        None,
    ))
}

/// Each ready indexer's feed, identified by `releases.match`, grabbed through the auto path.
pub fn rss_sync() -> R<()> {
    if config::auto_acquisition_paused("RssSync").map_err(Error::Other)? {
        return Ok(());
    }
    let all = indexers::list_enabled()?;
    let Some(client) = pick_client()? else {
        warn!("RssSync: no enabled download client configured");
        return Ok(());
    };
    let ready = throttle::filter_ready(&all)?;
    if ready.is_empty() {
        if !all.is_empty() {
            warn!("RssSync: every indexer is in cooldown — skipping this run");
        }
        return Ok(());
    }
    let out_of_time = || net::budget().expired();
    for ix in &ready {
        if out_of_time() {
            info!("RssSync: out of time, the next tick re-reads the feeds");
            return Ok(());
        }
        let releases = match rt::block_on(torznab::rss(ix, net::budget())) {
            Ok(r) => r,
            Err(throttle::Lapsed) => {
                warn!(
                    "RssSync: indexer \"{}\" failed: its slot stayed busy",
                    ix.name
                );
                continue;
            }
        };
        if releases.is_empty() {
            continue;
        }
        if config::auto_acquisition_paused("RssSync").map_err(Error::Other)? {
            return Ok(());
        }
        let matched = identify(&releases.iter().map(|r| r.title.clone()).collect::<Vec<_>>())?;
        for release in &releases {
            let Some(Some(m)) = matched.get(&release.title) else {
                continue;
            };
            if out_of_time() {
                info!("RssSync: out of time, the next tick re-reads the feeds");
                return Ok(());
            }
            let page = host::acquisition_candidates(&CandidatesQuery {
                kind: None,
                family: None,
                media_ids: Some(vec![m.media_id]),
                available_on: today(),
                limit: 500,
                cursor: None,
            })?;
            // `releases.match` names no family: a book is tried on each target of the family the
            // title names, else on each target it has.
            let named = title_family(&release.title);
            let targets: Vec<&Target> = page
                .items
                .iter()
                .filter(|it| match (it.kind == "comic", &m.unit) {
                    (true, Some(u)) => same_issue(it, u),
                    (true, None) => false,
                    (false, _) => numbers(it) == (m.season_number, m.episode_number),
                })
                .filter(|it| named.is_none_or(|f| it.kind != "book" || it.family == f))
                .collect();
            if targets.is_empty() && page.cursor.is_some() {
                warn!(
                    "RssSync: \"{}\" matched #{} but its candidate was past the first page — skipped",
                    release.title, m.media_id
                );
            }
            for t in targets {
                if t.want
                    .as_ref()
                    .is_some_and(|w| w.decision != WantDecision::Skip)
                {
                    auto_grab_or_skip(t, &client);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_names_at_most_one_family() {
        assert_eq!(title_family("Dune [M4B] 64kbps"), Some(Family::Audio));
        assert_eq!(title_family("Dune (EPUB)"), Some(Family::Text));
        assert_eq!(title_family("Dune epub + mp3"), None);
        assert_eq!(title_family("Dune"), None);
    }

    #[test]
    fn a_format_the_ingest_refuses_is_not_grabbed() {
        assert!(!ingestable("Dune WAV", Family::Audio));
        assert!(!ingestable("Dune azw", Family::Text));
        assert!(!ingestable("Dune djvu", Family::Text));
        assert!(ingestable("Dune azw3", Family::Text));
        assert!(ingestable("Dune flac wav", Family::Audio));
        assert!(ingestable("Dune", Family::Audio));
        assert!(ingestable("Dune wav", Family::Text));
    }

    #[test]
    fn the_format_table_follows_the_hosts_ingest_lists() {
        use crate::poller::{AUDIO_EXTS, TEXT_EXTS};
        // `alac` rides `.m4a` and `kf8` is `.azw3`: names, not extensions.
        let alias = ["alac", "kf8"];
        for (name, family, ok) in BOOK_FORMATS {
            let exts: &[&str] = if family == Family::Audio {
                &AUDIO_EXTS
            } else {
                &TEXT_EXTS
            };
            let listed = exts.contains(&format!(".{name}").as_str());
            assert_eq!(listed || alias.contains(&name), ok, "{name}");
        }
        for e in AUDIO_EXTS.iter().chain(&TEXT_EXTS) {
            assert!(
                BOOK_FORMATS.iter().any(|(n, ..)| format!(".{n}") == **e),
                "{e}"
            );
        }
    }
}
