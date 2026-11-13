//! The MusicBrainz lookups over an injected transport, so recorded bodies can stand in for the network.
use crate::map::{self, Target};
use fliks_plugin_sdk_provider::ProviderError;
use provider_common::http::{json_reply, upstream};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub type R<T> = Result<T, ProviderError>;

const PAGE: u32 = 100;
/// 1000 releases of a group and 2000 release groups of an artist: a longer list is cut.
const MAX_RELEASE_PAGES: u32 = 10;
const MAX_GROUP_PAGES: u32 = 20;
const HITS: u32 = 25;
/// The candidates a text search ranks: Lucene's own order buries exact titles under repeated words.
const POOL: u32 = 100;
/// ListenBrainz answers at most 100 recordings a call.
const RECORDINGS_PER_CALL: usize = 100;
const TOP_RECORDINGS: usize = 10;
const EDITION_INC: &str = "recordings+artist-credits+media+labels+isrcs+recording-level-rels+work-rels+work-level-rels+artist-rels";

pub trait Http {
    /// A GET of the web service (`/ws/2` is the transport's), the answer as JSON.
    fn mb(&self, path: &str, query: &[(&str, String)]) -> R<Value>;
    /// A POST of ListenBrainz; none when it refuses, is unreachable or answers nothing usable.
    fn lb(&self, path: &str, body: &Value) -> Option<Value>;
    /// A GET of ListenBrainz, under the same rules as `lb`.
    fn lb_get(&self, path: &str) -> Option<Value>;
}

/// A MusicBrainz answer as the provider reports it; 503 is its rate limit.
pub fn reply(status: u16, retry_after: Option<&str>, body: &[u8]) -> R<Value> {
    if status == 503 {
        return Err(ProviderError::RateLimited(
            retry_after.and_then(|r| r.trim().parse().ok()),
        ));
    }
    json_reply(status, retry_after, body)
}

fn id_of(id: &str) -> R<String> {
    map::uuid(id).ok_or(ProviderError::NotFound)
}

fn array(v: &Value, k: &str) -> Vec<Value> {
    v.get(k)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn q(pairs: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    pairs.iter().map(|(k, v)| (*k, (*v).to_owned())).collect()
}

/// ListenBrainz listen counts of release groups, by id; empty when it gives none.
fn popularity(h: &impl Http, ids: &[String]) -> HashMap<String, f64> {
    if ids.is_empty() {
        return HashMap::new();
    }
    let body = json!({ "release_group_mbids": ids });
    let Some(Value::Array(rows)) = h.lb("/1/popularity/release-group", &body) else {
        return HashMap::new();
    };
    rows.iter()
        .filter_map(|r| {
            let id = r["release_group_mbid"].as_str()?;
            Some((id.to_lowercase(), r["total_listen_count"].as_f64()?))
        })
        .collect()
}

fn group(h: &impl Http, id: &str) -> R<Value> {
    h.mb(
        &format!("/release-group/{}", id_of(id)?),
        &q(&[("inc", "artist-credits+releases+genres")]),
    )
}

/// The MusicBrainz offset of the pool holding `page`, and where the page starts in it.
pub(crate) fn pool_of(page: u32) -> (u32, usize) {
    let i = (page.max(1) - 1) * HITS;
    (i / POOL * POOL, (i % POOL) as usize)
}

fn page_of(ranked: Vec<Value>, skip: usize) -> Vec<Value> {
    ranked.into_iter().skip(skip).take(HITS as usize).collect()
}

fn hits(groups: &[Value], text: &str, h: &impl Http) -> Vec<Value> {
    let ids: Vec<String> = groups
        .iter()
        .filter_map(|g| map::uuid(g["id"].as_str()?))
        .collect();
    let pop = popularity(h, &ids);
    let mut ranked: Vec<(f64, Value)> = groups
        .iter()
        .filter_map(|g| {
            let p = pop.get(g["id"].as_str()?).copied();
            Some((map::score(g, text, p), map::album(g, p)?))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked.into_iter().map(|(_, a)| a).collect()
}

fn artist_groups(h: &impl Http, id: &str, page: u32) -> R<Vec<Value>> {
    let body = h.mb(
        "/release-group",
        &[
            ("artist", id_of(id)?),
            ("inc", "artist-credits".to_owned()),
            ("limit", HITS.to_string()),
            ("offset", ((page.max(1) - 1) * HITS).to_string()),
        ],
    )?;
    Ok(hits(&array(&body, "release-groups"), "", h))
}

/// The group a release belongs to, as an album.
fn release_album(h: &impl Http, id: &str) -> R<Value> {
    let r = h.mb(
        &format!("/release/{}", id_of(id)?),
        &q(&[("inc", "release-groups+artist-credits")]),
    )?;
    let mut g = r["release-group"].clone();
    if g["artist-credit"].is_null() {
        g["artist-credit"] = r["artist-credit"].clone();
    }
    map::album(&g, None).ok_or(ProviderError::NotFound)
}

pub fn search(
    h: &impl Http,
    text: &str,
    title: Option<&str>,
    artist: Option<&str>,
    artist_id: Option<&str>,
    year: Option<i32>,
    page: u32,
) -> R<Vec<Value>> {
    let album_of_group =
        |id: &str| -> R<Vec<Value>> { Ok(map::album(&group(h, id)?, None).into_iter().collect()) };
    match map::target(text) {
        Target::Group(id) => return album_of_group(&id),
        Target::Release(id) => return release_album(h, &id).map(|a| vec![a]),
        Target::Artist(id) => return artist_groups(h, &id, page),
        Target::Id(id) => {
            return match album_of_group(&id) {
                Err(ProviderError::NotFound) => match release_album(h, &id) {
                    Err(ProviderError::NotFound) => artist_groups(h, &id, page),
                    r => r.map(|a| vec![a]),
                },
                r => r,
            };
        }
        Target::Text => {}
    }
    let name = title.unwrap_or(text).trim();
    let mut query = format!("releasegroup:({})", map::escape(name));
    if let Some(id) = artist_id.and_then(map::uuid) {
        query += &format!(" AND arid:{id}");
    } else if let Some(a) = artist.map(str::trim).filter(|a| !a.is_empty()) {
        query += &format!(" AND artist:\"{}\"", map::escape(a));
    }
    if let Some(y) = year.filter(|y| (1..=9999).contains(y)) {
        query += &format!(" AND firstreleasedate:{y}");
    }
    query += " AND (primarytype:album^5 OR primarytype:ep OR primarytype:single OR primarytype:broadcast OR primarytype:other)";
    let (offset, skip) = pool_of(page);
    let body = h.mb(
        "/release-group",
        &[
            ("query", query),
            ("limit", POOL.to_string()),
            ("offset", offset.to_string()),
        ],
    )?;
    Ok(page_of(
        hits(&array(&body, "release-groups"), name, h),
        skip,
    ))
}

/// The artists matching `text`, in the search's own order.
pub fn search_artists(h: &impl Http, text: &str, page: u32) -> R<Vec<Value>> {
    let body = h.mb(
        "/artist",
        &[
            ("query", map::escape(text.trim())),
            ("limit", HITS.to_string()),
            ("offset", ((page.max(1) - 1) * HITS).to_string()),
        ],
    )?;
    Ok(array(&body, "artists")
        .iter()
        .filter_map(map::artist_hit)
        .collect())
}

/// The recordings matching `text` with the album each is listed under; one entry per title and artist.
pub fn search_recordings(h: &impl Http, text: &str, page: u32) -> R<Vec<Value>> {
    let text = text.trim();
    let (offset, skip) = pool_of(page);
    let body = h.mb(
        "/recording",
        &[
            (
                "query",
                format!(
                    "recording:({}) AND (tag:[a TO z]^10 OR primarytype:album^3 OR status:official)",
                    map::escape(text)
                ),
            ),
            ("limit", POOL.to_string()),
            ("offset", offset.to_string()),
        ],
    )?;
    let recs = array(&body, "recordings");
    let key = |r: &Value| {
        let by = r["artists"][0]["name"].as_str().unwrap_or_default();
        format!(
            "{}\u{1f}{by}",
            r["title"].as_str().unwrap_or_default().to_lowercase()
        )
    };
    let hits: Vec<(f64, Value)> = recs
        .iter()
        .filter_map(|r| Some((map::recording_score(r, text), map::recording_hit(r)?)))
        .collect();
    // A known song is many recordings of one title and artist: their releases and tags add up.
    let mut weight: HashMap<String, f64> = HashMap::new();
    for (r, (_, hit)) in recs
        .iter()
        .filter(|r| map::recording_hit(r).is_some())
        .zip(&hits)
    {
        *weight.entry(key(hit)).or_default() += map::recording_weight(r);
    }
    let mut ranked: Vec<(f64, Value)> = hits
        .into_iter()
        .map(|(s, r)| (s + (1.0 + weight[&key(&r)]).ln() * 20.0, r))
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut seen = std::collections::HashSet::new();
    let unique = ranked
        .into_iter()
        .map(|(_, r)| r)
        .filter(|r| seen.insert(key(r)));
    Ok(page_of(unique.collect(), skip))
}

pub fn work(h: &impl Http, id: &str) -> R<Value> {
    let g = group(h, id)?;
    let gid = g["id"].as_str().unwrap_or_default().to_lowercase();
    let pop = popularity(h, std::slice::from_ref(&gid));
    map::album(&g, pop.get(&gid).copied()).ok_or(ProviderError::NotFound)
}

pub fn children(h: &impl Http, id: &str) -> R<Vec<Value>> {
    let id = id_of(id)?;
    let mut out = vec![];
    for p in 0..MAX_RELEASE_PAGES {
        let body = h.mb(
            "/release",
            &[
                ("release-group", id.clone()),
                ("inc", "labels+media".to_owned()),
                ("limit", PAGE.to_string()),
                ("offset", (p * PAGE).to_string()),
            ],
        )?;
        let page = array(&body, "releases");
        out.extend(page.iter().filter_map(map::release).map(Value::Object));
        let total = body["release-count"].as_u64().unwrap_or(0);
        if page.is_empty() || u64::from((p + 1) * PAGE) >= total {
            break;
        }
    }
    Ok(out)
}

/// ListenBrainz listen counts of recordings, by id; empty when it gives none.
fn recording_popularity(h: &impl Http, ids: &[String]) -> HashMap<String, f64> {
    ids.chunks(RECORDINGS_PER_CALL)
        .filter_map(|chunk| {
            h.lb(
                "/1/popularity/recording",
                &json!({ "recording_mbids": chunk }),
            )
        })
        .flat_map(|rows| rows.as_array().cloned().unwrap_or_default())
        .filter_map(|r| {
            let id = r["recording_mbid"].as_str()?;
            Some((id.to_lowercase(), r["total_listen_count"].as_f64()?))
        })
        .collect()
}

pub fn edition(h: &impl Http, id: &str) -> R<Value> {
    let r = h.mb(
        &format!("/release/{}", id_of(id)?),
        &q(&[("inc", EDITION_INC)]),
    )?;
    let mut e = map::edition(&r).ok_or(ProviderError::NotFound)?;
    let tracks = e["tracks"].as_array_mut().into_iter().flatten();
    let tracks: Vec<&mut Value> = tracks.collect();
    let mut ids: Vec<String> = tracks
        .iter()
        .filter_map(|t| t["recordingId"].as_str().map(str::to_owned))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let pop = recording_popularity(h, &ids);
    for t in tracks {
        let p = t["recordingId"].as_str().and_then(|i| pop.get(i)).copied();
        if let Some(p) = p {
            t["popularity"] = json!(p);
        }
    }
    Ok(e)
}

pub fn person(h: &impl Http, id: &str) -> R<Value> {
    let a = h.mb(
        &format!("/artist/{}", id_of(id)?),
        &q(&[("inc", "annotation")]),
    )?;
    Ok(map::person(&a))
}

/// The discography of an artist: `{albums}` (rule 23).
pub fn credits(h: &impl Http, id: &str) -> R<Value> {
    let id = id_of(id)?;
    let mut albums = vec![];
    for p in 0..MAX_GROUP_PAGES {
        let body = h.mb(
            "/release-group",
            &[
                ("artist", id.clone()),
                ("inc", "artist-credits".to_owned()),
                ("limit", PAGE.to_string()),
                ("offset", (p * PAGE).to_string()),
            ],
        )?;
        let page = array(&body, "release-groups");
        albums.extend(page.iter().filter_map(|g| map::album(g, None)));
        let total = body["release-group-count"].as_u64().unwrap_or(0);
        if page.is_empty() || u64::from((p + 1) * PAGE) >= total {
            break;
        }
    }
    let mut out = json!({ "albums": albums });
    if let Some(top) = top_recordings(h, &id, &albums) {
        out["topRecordings"] = json!(top);
    }
    Ok(out)
}

/// The artist's most listened recordings, none when ListenBrainz gives nothing; one title once.
fn top_recordings(h: &impl Http, artist: &str, albums: &[Value]) -> Option<Vec<Value>> {
    let Value::Array(rows) =
        h.lb_get(&format!("/1/popularity/top-recordings-for-artist/{artist}"))?
    else {
        return None;
    };
    let mut seen = HashSet::new();
    let mut top: Vec<(String, &Value)> = rows
        .iter()
        .filter_map(|r| Some((map::uuid(r["recording_mbid"].as_str()?)?, r)))
        .filter(|(_, r)| {
            r["recording_name"]
                .as_str()
                .is_some_and(|t| seen.insert(t.trim().to_lowercase()))
        })
        .take(TOP_RECORDINGS)
        .collect();
    let groups = groups_by_title(albums);
    let release_name = |r: &Value| r["release_name"].as_str().map(|t| t.trim().to_lowercase());
    let misses: Vec<String> = top
        .iter()
        .filter(|(_, r)| release_name(r).is_some_and(|n| !groups.contains_key(&n)))
        .filter_map(|(_, r)| map::uuid(r["release_mbid"].as_str()?))
        .collect();
    let by_release = groups_of_releases(h, &misses);
    top.drain(..)
        .map(|(id, r)| {
            let album = release_name(r).and_then(|n| groups.get(&n).map(|g| (*g).to_owned()));
            let album = album.or_else(|| {
                by_release
                    .get(&map::uuid(r["release_mbid"].as_str()?)?)
                    .cloned()
            });
            let cover = r["caa_release_mbid"]
                .as_str()
                .and_then(map::uuid)
                .map(|c| format!("https://coverartarchive.org/release/{c}/front-250"));
            Some(map::compact(json!({
                "recordingId": id,
                "title": r["recording_name"],
                "durationMs": r["length"],
                "listenCount": r["total_listen_count"],
                "albumExternalId": album,
                "albumTitle": r["release_name"],
                "coverUrl": cover,
            })))
        })
        .collect()
}

/// The release group of each lowercased title; when several share it (an album and its single), the studio album.
pub(crate) fn groups_by_title(albums: &[Value]) -> HashMap<String, &str> {
    let mut best: HashMap<String, (u8, &str)> = HashMap::new();
    for a in albums {
        let (Some(title), Some(id)) = (a["title"].as_str(), a["externalId"].as_str()) else {
            continue;
        };
        let plain = a["secondaryTypes"].as_array().is_none_or(|s| s.is_empty());
        let rank = match a["primaryType"].as_str() {
            Some("album") if plain => 0,
            Some("album") => 1,
            Some("ep") => 2,
            Some("single") => 3,
            _ => 4,
        };
        let slot = best
            .entry(title.trim().to_lowercase())
            .or_insert((rank, id));
        if rank < slot.0 {
            *slot = (rank, id);
        }
    }
    best.into_iter().map(|(t, (_, id))| (t, id)).collect()
}

/// The release group of each release id, from one search; empty when it fails.
fn groups_of_releases(h: &impl Http, ids: &[String]) -> HashMap<String, String> {
    if ids.is_empty() {
        return HashMap::new();
    }
    let query = format!("reid:({})", ids.join(" OR "));
    let body = h.mb("/release", &[("query", query), ("limit", PAGE.to_string())]);
    array(&body.unwrap_or_default(), "releases")
        .iter()
        .filter_map(|r| {
            let rid = map::uuid(r["id"].as_str()?)?;
            Some((rid, map::uuid(r["release-group"]["id"].as_str()?)?))
        })
        .collect()
}

/// The release group of a release id.
fn group_of_release(h: &impl Http, id: &str) -> R<Option<String>> {
    let r = h.mb(
        &format!("/release/{}", id_of(id)?),
        &q(&[("inc", "release-groups")]),
    )?;
    Ok(r["release-group"]["id"].as_str().and_then(map::uuid))
}

/// The album a Spotify or Deezer album URL is attached to, through the release group or its release.
fn by_url(h: &impl Http, resource: &str) -> R<Option<String>> {
    let body = match h.mb(
        "/url",
        &[
            ("resource", resource.to_owned()),
            ("inc", "release-group-rels+release-rels".to_owned()),
        ],
    ) {
        Err(ProviderError::NotFound) => return Ok(None),
        r => r?,
    };
    let rels = array(&body, "relations");
    let target = |kind: &str| {
        rels.iter()
            .find_map(|r| map::uuid(r.get(kind)?.get("id")?.as_str()?))
    };
    match (target("release_group"), target("release")) {
        (Some(g), _) => Ok(Some(g)),
        (None, Some(r)) => group_of_release(h, &r),
        _ => Ok(None),
    }
}

/// The album of a recording code: the group of its earliest official plain album, else of an official
/// release; a group in no release is read from the first official release.
fn by_isrc(h: &impl Http, isrc: &str) -> R<Option<String>> {
    let body = match h.mb(
        &format!("/isrc/{isrc}"),
        &q(&[("inc", "releases+release-groups")]),
    ) {
        Err(ProviderError::NotFound) => return Ok(None),
        r => r?,
    };
    let releases: Vec<Value> = array(&body, "recordings")
        .iter()
        .flat_map(|r| array(r, "releases"))
        .collect();
    let official = |r: &Value| {
        r["status"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("official"))
    };
    let plain_album = |g: &Value| {
        g["primary-type"].as_str() == Some("Album")
            && g["secondary-types"].as_array().is_none_or(Vec::is_empty)
    };
    let best = releases
        .iter()
        .filter_map(|r| {
            let g = r.get("release-group")?;
            let id = map::uuid(g["id"].as_str()?)?;
            let date = g["first-release-date"].as_str().filter(|d| !d.is_empty());
            let rank = (
                !(official(r) && plain_album(g)),
                !official(r),
                date.is_none(),
                date.unwrap_or_default().to_owned(),
            );
            Some((rank, id))
        })
        .min_by(|a, b| a.0.cmp(&b.0));
    if let Some((_, id)) = best {
        return Ok(Some(id));
    }
    let first = releases.iter().find(|r| official(r)).or(releases.first());
    match first.and_then(|r| r["id"].as_str()) {
        Some(id) => group_of_release(h, id),
        None => Ok(None),
    }
}

pub fn find_by(h: &impl Http, provider: &str, value: &str) -> R<Option<String>> {
    match provider {
        "spotify" => by_url(h, &format!("https://open.spotify.com/album/{value}")),
        "deezer" => by_url(h, &format!("https://www.deezer.com/album/{value}")),
        "isrc" => by_isrc(h, &value.replace('-', "").to_uppercase()),
        _ => Ok(None),
    }
}

const TRENDING: usize = 30;

/// The week's most listened albums, artists or recordings on ListenBrainz, as search hits.
/// `target` is `artist`, `recording` or anything else for albums.
pub fn trending(h: &impl Http, target: &str) -> R<Vec<Value>> {
    let (path, key) = match target {
        "artist" => ("artists", "artists"),
        "recording" => ("recordings", "recordings"),
        _ => ("release-groups", "release_groups"),
    };
    let body = h
        .lb_get(&format!(
            "/1/stats/sitewide/{path}?count={TRENDING}&range=week"
        ))
        .ok_or_else(|| upstream(None, "ListenBrainz gave no statistics"))?;
    let rows = array(&body["payload"], key);
    Ok(match target {
        "artist" => rows.iter().filter_map(map::trending_artist).collect(),
        "recording" => {
            let ids: Vec<&str> = rows
                .iter()
                .filter_map(|r| r["recording_mbid"].as_str())
                .collect();
            let meta = h
                .lb_get(&format!(
                    "/1/metadata/recording/?recording_mbids={}&inc=release+artist",
                    ids.join(",")
                ))
                .unwrap_or(Value::Null);
            rows.iter()
                .filter_map(|r| {
                    map::trending_recording(r, meta.get(r["recording_mbid"].as_str()?)?)
                })
                .collect()
        }
        _ => rows.iter().filter_map(map::trending_album).collect(),
    })
}
