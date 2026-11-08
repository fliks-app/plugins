//! The LRCLIB answer as a lyrics hit, over bytes so recorded bodies can stand in for the network.
use fliks_plugin_sdk_provider::{LyricsHit, LyricsQuery, ProviderError};
use provider_common::http::{json_reply, status_error};
use serde_json::Value;

pub type R<T> = Result<T, ProviderError>;

/// `/api/get` takes the duration in whole seconds, 1 to 3600, and matches within 2 s.
pub fn params(q: &LyricsQuery) -> Vec<(&'static str, String)> {
    let mut p = vec![
        ("track_name", q.title.clone()),
        ("artist_name", q.artist.clone()),
    ];
    if let Some(a) = q.album.as_ref().filter(|a| !a.trim().is_empty()) {
        p.push(("album_name", a.clone()));
    }
    if let Some(s) = q
        .duration_ms
        .map(|ms| ms.div_ceil(500) / 2)
        .filter(|s| (1..=3600).contains(s))
    {
        p.push(("duration", s.to_string()));
    }
    p
}

/// A 404 is a miss; 429 and 503 carry the seconds to wait.
pub fn reply(status: u16, retry_after: Option<&str>, body: &[u8]) -> R<Option<LyricsHit>> {
    match status {
        404 => Ok(None),
        503 => Err(status_error(
            429,
            retry_after.and_then(|r| r.trim().parse().ok()),
            "",
        )),
        _ => json_reply(status, retry_after, body).map(|v| hit(&v)),
    }
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

/// Synced LRC when present, else plain; an instrumental record is an empty document.
fn hit(v: &Value) -> Option<LyricsHit> {
    let (format, document, instrumental) = if v["instrumental"].as_bool() == Some(true) {
        ("plain", "", true)
    } else if let Some(s) = text(v, "syncedLyrics") {
        ("lrc", s, false)
    } else {
        ("plain", text(v, "plainLyrics")?, false)
    };
    Some(LyricsHit {
        format: format.into(),
        document: document.into(),
        instrumental,
        lang: None,
        source_ref: v["id"].as_u64().map(|i| i.to_string()),
    })
}
