//! MusicBrainz JSON to the album records of `docs/spec/music.md` (rules 18 to 23), and the search helpers.
use serde_json::{Map, Value, json};
use std::collections::HashSet;

const WEB: &str = "https://musicbrainz.org";
const CAA: &str = "https://coverartarchive.org";
const MAX_TRACKS: usize = 2000;

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k)?.as_str().map(str::trim).filter(|t| !t.is_empty())
}

fn lower(v: &Value, k: &str) -> Option<String> {
    s(v, k).map(str::to_lowercase)
}

fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// An object without its null entries: the host reads an absent key and a null one alike, a record stays small.
pub fn compact(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(m.into_iter().filter(|(_, v)| !v.is_null()).collect()),
        v => v,
    }
}

/// A MusicBrainz id as the core stores it: a UUID in lower case.
pub fn uuid(id: &str) -> Option<String> {
    let id = id.trim().to_lowercase();
    let ok = id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        });
    ok.then_some(id)
}

/// What a search text names: an entity of musicbrainz.org (a URL, or a bare id of any kind), or free text.
#[derive(Debug, PartialEq)]
pub enum Target {
    Group(String),
    Release(String),
    Artist(String),
    /// A bare id: the kind is not known.
    Id(String),
    Text,
}

pub fn target(text: &str) -> Target {
    let t = text.trim();
    if let Some(id) = uuid(t) {
        return Target::Id(id);
    }
    let Some(rest) = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
    else {
        return Target::Text;
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if !(host == "musicbrainz.org" || host.ends_with(".musicbrainz.org")) {
        return Target::Text;
    }
    let mut seg = path.split(['/', '?', '#']);
    match (seg.next(), seg.next().and_then(uuid)) {
        (Some("release-group"), Some(id)) => Target::Group(id),
        (Some("release"), Some(id)) => Target::Release(id),
        (Some("artist"), Some(id)) => Target::Artist(id),
        _ => Target::Text,
    }
}

/// Lucene special characters escaped, so a title never changes the query's meaning.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "+-&|!(){}[]^\"~*?:\\/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The credits of an `artist-credit` list, in order.
pub fn credits(ac: &Value) -> Vec<Value> {
    ac.as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|c| {
            let a = c.get("artist").unwrap_or(&Value::Null);
            let printed = s(c, "name");
            let name = s(a, "name").or(printed)?;
            let join = s_raw(c, "joinphrase").filter(|j| j.chars().count() <= 64);
            Some(compact(json!({
                "name": name,
                "id": s(a, "id").and_then(uuid),
                "sortName": s(a, "sort-name"),
                "creditedName": printed.filter(|p| *p != name),
                "joinPhrase": join,
            })))
        })
        .collect()
}

/// A string that may hold meaningful spaces (a join phrase), non-empty.
fn s_raw<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k)?.as_str().filter(|t| !t.is_empty())
}

pub fn cover_of_group(id: &str) -> String {
    format!("{CAA}/release-group/{id}/front-500")
}

pub fn url_of(kind: &str, id: &str) -> String {
    format!("{WEB}/{kind}/{id}")
}

/// The group's genres, most voted first; a hit or a discography entry carries none.
fn genres(g: &Value) -> Vec<String> {
    let mut all: Vec<(u64, String)> = list(g, "genres")
        .iter()
        .filter_map(|x| {
            Some((
                x.get("count")?.as_u64().unwrap_or(0),
                s(x, "name")?.to_owned(),
            ))
        })
        .collect();
    all.sort_by_key(|a| std::cmp::Reverse(a.0));
    all.into_iter().map(|(_, n)| n).take(20).collect()
}

/// A release group: a search hit, a work record or a discography entry.
pub fn album(g: &Value, popularity: Option<f64>) -> Option<Value> {
    let id = s(g, "id").and_then(uuid)?;
    let secondary: Vec<String> = list(g, "secondary-types")
        .iter()
        .filter_map(|t| Some(t.as_str()?.trim().to_lowercase()))
        .collect();
    let releases = g
        .get("count")
        .and_then(Value::as_u64)
        .or_else(|| g.get("releases")?.as_array().map(|r| r.len() as u64));
    Some(compact(json!({
        "externalId": id,
        "title": s(g, "title")?,
        "genres": genres(g),
        "primaryType": lower(g, "primary-type"),
        "secondaryTypes": secondary,
        "firstReleaseDate": s(g, "first-release-date"),
        "posterUrl": cover_of_group(&id),
        "sourceUrl": url_of("release-group", &id),
        "artists": credits(g.get("artist-credit").unwrap_or(&Value::Null)),
        "releaseCount": releases,
        "popularity": popularity,
        "mediaType": "album",
    })))
}

/// An artist search hit.
pub fn artist_hit(a: &Value) -> Option<Value> {
    Some(compact(json!({
        "externalId": s(a, "id").and_then(uuid)?,
        "name": s(a, "name")?,
        "sortName": s(a, "sort-name"),
        "disambiguation": s(a, "disambiguation"),
        "artistType": lower(a, "type"),
        "country": s(a, "country"),
    })))
}

/// The release a recording is shown under: official first, a plain album before the rest, then the earliest.
fn best_release(r: &Value) -> Option<&Value> {
    let rank = |x: &&Value| {
        let g = x.get("release-group").unwrap_or(&Value::Null);
        let plain = lower(g, "primary-type").as_deref() == Some("album")
            && list(g, "secondary-types").is_empty();
        (
            !s(x, "status").is_some_and(|t| t.eq_ignore_ascii_case("official")),
            !plain,
            s(x, "date").is_none(),
            s(x, "date").unwrap_or_default().to_owned(),
        )
    };
    list(r, "releases")
        .iter()
        .filter(|x| x.get("release-group").and_then(|g| s(g, "id")).is_some())
        .min_by_key(rank)
}

/// A recording search hit with the release group it is listed under as an album record.
pub fn recording_hit(r: &Value) -> Option<Value> {
    let rel = best_release(r)?;
    let mut g = rel["release-group"].clone();
    g["artist-credit"] = r["artist-credit"].clone();
    if g.get("first-release-date").is_none_or(Value::is_null)
        && let Some(d) = s(rel, "date")
    {
        g["first-release-date"] = d.into();
    }
    Some(compact(json!({
        "externalId": s(r, "id").and_then(uuid)?,
        "title": s(r, "title")?,
        "durationMs": r.get("length").and_then(Value::as_u64).filter(|l| *l > 0),
        "artists": credits(r.get("artist-credit").unwrap_or(&Value::Null)),
        "album": album(&g, None)?,
    })))
}

/// Higher is better: the search score first, then what the owner of a library wants first.
pub fn score(g: &Value, text: &str, popularity: Option<f64>) -> f64 {
    let exact = s(g, "title").is_some_and(|t| t.to_lowercase() == text.trim().to_lowercase());
    let primary = match lower(g, "primary-type").as_deref() {
        Some("album") => 30.0,
        Some("ep") => 15.0,
        Some("single") => 5.0,
        _ => 0.0,
    };
    let releases = g
        .get("count")
        .and_then(Value::as_f64)
        .or_else(|| Some(list(g, "releases").len() as f64))
        .unwrap_or(0.0)
        .min(30.0);
    let base = g.get("score").and_then(Value::as_f64).unwrap_or(0.0);
    let secondary = if list(g, "secondary-types").is_empty() {
        0.0
    } else {
        -15.0
    };
    base + if exact { 100.0 } else { 0.0 }
        + primary
        + releases
        + secondary
        + popularity.map_or(0.0, |p| (1.0 + p).ln() * 5.0)
}

/// The medium kind of a MusicBrainz format name.
fn medium(format: &str) -> &'static str {
    let f = format.to_lowercase();
    if f.contains("vinyl") {
        "vinyl"
    } else if f.contains("digital") {
        "digital"
    } else if f.contains("cassette") {
        "cassette"
    } else if f.contains("cd") {
        "cd"
    } else {
        "other"
    }
}

/// The release head (rule 20): `rel` is a release as `/release` browses or looks it up.
pub fn release(r: &Value) -> Option<Map<String, Value>> {
    let id = s(r, "id").and_then(uuid)?;
    let media = list(r, "media");
    let tracks: u64 = media
        .iter()
        .filter_map(|m| m.get("track-count")?.as_u64())
        .sum();
    let label = list(r, "label-info").first();
    let country = s(r, "country").map(str::to_owned).or_else(|| {
        let e = list(r, "release-events").first()?;
        let code = e.pointer("/area/iso-3166-1-codes/0")?.as_str()?;
        Some(code.to_owned())
    });
    let Value::Object(m) = compact(json!({
        "externalId": id,
        "title": s(r, "title")?,
        "country": country,
        "status": lower(r, "status"),
        "media": media.first().and_then(|m| s(m, "format")).map(medium),
        "releaseDate": s(r, "date"),
        "barcode": s(r, "barcode").filter(|b| b.bytes().all(|c| c.is_ascii_digit())),
        "catalogNumber": label.and_then(|l| s(l, "catalog-number")),
        "label": label.and_then(|l| l.get("label")).and_then(|l| s(l, "name")),
        "trackCount": (!media.is_empty()).then_some(tracks),
        "discCount": (!media.is_empty()).then_some(media.len()),
    })) else {
        return None;
    };
    Some(m)
}

fn credit_of(artist: &Value, role: &str, instrument: Option<String>) -> Option<Value> {
    Some(compact(json!({
        "name": s(artist, "name")?,
        "id": s(artist, "id").and_then(uuid),
        "sortName": s(artist, "sort-name"),
        "role": role,
        "instrument": instrument,
    })))
}

/// Performers and conductors of a recording, from its artist relations.
fn performers(rec: &Value) -> Vec<Value> {
    list(rec, "relations")
        .iter()
        .filter_map(|r| {
            let a = r.get("artist")?;
            let attrs: Vec<&str> = list(r, "attributes")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let instrument = (!attrs.is_empty()).then(|| attrs.join(", "));
            match s(r, "type")? {
                "instrument" | "performer" => credit_of(a, "performer", instrument),
                "vocal" => credit_of(a, "performer", instrument.or(Some("vocals".into()))),
                "conductor" => credit_of(a, "conductor", None),
                "performing orchestra" => credit_of(a, "orchestra", None),
                _ => None,
            }
        })
        .collect()
}

/// The works a recording performs, and the composers and lyricists they name.
fn works(rec: &Value) -> (Vec<Value>, Vec<Value>) {
    let (mut refs, mut people) = (vec![], vec![]);
    for r in list(rec, "relations") {
        let Some(w) = r
            .get("work")
            .filter(|_| s(r, "type") == Some("performance"))
        else {
            continue;
        };
        let by = |role: &str| {
            list(w, "relations")
                .iter()
                .filter(|x| s(x, "type") == Some(role))
                .filter_map(|x| credit_of(x.get("artist")?, role, None))
                .collect::<Vec<_>>()
        };
        let (composers, lyricists) = (by("composer"), by("lyricist"));
        let parent = list(w, "relations")
            .iter()
            .find(|x| s(x, "type") == Some("parts") && s(x, "direction") == Some("backward"))
            .and_then(|x| x.get("work"));
        refs.push(compact(json!({
            "externalId": s(w, "id").and_then(uuid),
            "title": s(w, "title"),
            "parentId": parent.and_then(|p| s(p, "id")).and_then(uuid),
            "parentTitle": parent.and_then(|p| s(p, "title")),
            "composer": composers.first(),
        })));
        people.extend(composers);
        people.extend(lyricists);
    }
    (refs, people)
}

fn track(t: &Value, disc: u64, disc_title: Option<&str>, position: usize) -> Option<Value> {
    let rec = t.get("recording").unwrap_or(&Value::Null);
    let printed = s(t, "number");
    let (refs, writers) = works(rec);
    let mut credits_: Vec<Value> = performers(rec).into_iter().chain(writers).collect();
    let mut seen = HashSet::new();
    credits_.retain(|c| seen.insert(c.to_string()));
    let length = |v: &Value| v.get("length").and_then(Value::as_u64).filter(|n| *n > 0);
    Some(compact(json!({
        "title": s(t, "title").or_else(|| s(rec, "title"))?,
        "discNumber": disc,
        "discTitle": disc_title,
        "position": position,
        "number": printed.and_then(|n| n.parse::<u16>().ok()),
        "label": printed.filter(|n| n.chars().count() <= 32),
        "recordingId": s(rec, "id").and_then(uuid),
        "releaseTrackId": s(t, "id").and_then(uuid),
        "isrc": list(rec, "isrcs").first().and_then(Value::as_str),
        "durationMs": length(t).or_else(|| length(rec)),
        "artists": credits(t.get("artist-credit").unwrap_or(&Value::Null)),
        "credits": credits_,
        "works": refs,
    })))
}

/// A release with its tracklist (rule 21).
pub fn edition(r: &Value) -> Option<Value> {
    let mut head = release(r)?;
    let id = head["externalId"].as_str()?.to_owned();
    let mut tracks = vec![];
    for (i, m) in list(r, "media").iter().enumerate() {
        let disc = m
            .get("position")
            .and_then(Value::as_u64)
            .unwrap_or(i as u64 + 1);
        for t in list(m, "tracks") {
            if let Some(t) = track(t, disc.max(1), s(m, "title"), tracks.len() + 1) {
                tracks.push(t);
            }
        }
    }
    tracks.truncate(MAX_TRACKS);
    let front = r.pointer("/cover-art-archive/front") == Some(&Value::Bool(true));
    head.insert("tracks".into(), Value::Array(tracks));
    for (k, v) in [
        (
            "language",
            r.pointer("/text-representation/language").cloned(),
        ),
        (
            "coverUrl",
            front.then(|| json!(format!("{CAA}/release/{id}/front-500"))),
        ),
        (
            "artists",
            Some(json!(credits(
                r.get("artist-credit").unwrap_or(&Value::Null)
            ))),
        ),
    ] {
        if let Some(v) = v.filter(|v| !v.is_null()) {
            head.insert(k.into(), v);
        }
    }
    Some(Value::Object(head))
}

/// An artist (rule 22); `id` is the canonical id, which differs from the one asked after a merge.
pub fn person(a: &Value) -> Value {
    let span = a.get("life-span").unwrap_or(&Value::Null);
    let ended = span.get("ended") == Some(&Value::Bool(true));
    let area = a.get("begin-area").or_else(|| a.get("area"));
    compact(json!({
        "id": s(a, "id").and_then(uuid),
        "name": s(a, "name"),
        "sortName": s(a, "sort-name"),
        "disambiguation": s(a, "disambiguation"),
        "type": lower(a, "type"),
        "birthday": s(span, "begin"),
        "deathday": s(span, "end").filter(|_| ended),
        "placeOfBirth": area.and_then(|a| s(a, "name")),
        "biography": s(a, "annotation"),
    }))
}
