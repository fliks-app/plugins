//! The MusicBrainz lookups over an injected transport, so recorded bodies can stand in for the network.
use crate::map::{self, Target};
use fliks_plugin_sdk_provider::ProviderError;
use provider_common::http::json_reply;
use serde_json::{Value, json};
use std::collections::HashMap;

pub type R<T> = Result<T, ProviderError>;

const PAGE: u32 = 100;
/// 1000 releases of a group and 2000 release groups of an artist: a longer list is cut.
const MAX_RELEASE_PAGES: u32 = 10;
const MAX_GROUP_PAGES: u32 = 20;
const HITS: u32 = 25;
/// ListenBrainz answers at most 100 recordings a call.
const RECORDINGS_PER_CALL: usize = 100;
const EDITION_INC: &str = "recordings+artist-credits+media+labels+isrcs+recording-level-rels+work-rels+work-level-rels+artist-rels";

pub trait Http {
    /// A GET of the web service (`/ws/2` is the transport's), the answer as JSON.
    fn mb(&self, path: &str, query: &[(&str, String)]) -> R<Value>;
    /// A POST of ListenBrainz; none when it refuses, is unreachable or answers nothing usable.
    fn lb(&self, path: &str, body: &Value) -> Option<Value>;
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
        query += &format!(" AND firstreleasedate:{y}*");
    }
    let body = h.mb(
        "/release-group",
        &[
            ("query", query),
            ("limit", HITS.to_string()),
            ("offset", ((page.max(1) - 1) * HITS).to_string()),
        ],
    )?;
    Ok(hits(&array(&body, "release-groups"), name, h))
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
    Ok(json!({ "albums": albums }))
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
