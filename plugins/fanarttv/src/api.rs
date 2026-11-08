//! The Fanart.tv lookups over an injected transport, so recorded bodies can stand in for the network.
use fliks_plugin_sdk_provider::ProviderError;
use provider_common::comic::https;
use provider_common::http::json_reply;
use serde_json::{Value, json};

pub type R<T> = Result<T, ProviderError>;

pub trait Http {
    fn get(&self, path: &str) -> R<Value>;
}

/// An id Fanart.tv is keyed by: a MusicBrainz UUID, in lower case.
pub fn uuid(id: &str) -> Option<String> {
    let id = id.trim().to_lowercase();
    let ok = id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        });
    ok.then_some(id)
}

pub fn reply(status: u16, retry_after: Option<&str>, body: &[u8]) -> R<Value> {
    json_reply(status, retry_after, body)
}

/// The most liked image of a list, as an https URL.
fn best(v: &Value, key: &str) -> Option<String> {
    let likes = |i: &Value| {
        i["likes"]
            .as_str()
            .and_then(|l| l.parse::<u64>().ok())
            .unwrap_or(0)
    };
    v.get(key)?
        .as_array()?
        .iter()
        .filter(|i| i["url"].as_str().is_some_and(|u| https(u).is_some()))
        .max_by_key(|i| likes(i))
        .and_then(|i| https(i["url"].as_str()?))
}

fn name(v: &Value) -> Option<&str> {
    v.get("name")?
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())
}

/// The image part of an artist: its thumb as `avatarUrl` (rule 22).
pub fn person(h: &impl Http, id: &str) -> R<Value> {
    let id = uuid(id).ok_or(ProviderError::NotFound)?;
    let a = h.get(&format!("/music/{id}"))?;
    let avatar = best(&a, "artistthumb").ok_or(ProviderError::NotFound)?;
    Ok(json!({ "name": name(&a), "avatarUrl": avatar }))
}

/// The cover of a release group as an album record (rule 18). The answer names the artist only, and the
/// record needs a title: it is the artist's name, which a provider ranked before this one overrides.
pub fn work(h: &impl Http, id: &str) -> R<Value> {
    let id = uuid(id).ok_or(ProviderError::NotFound)?;
    let a = h.get(&format!("/music/albums/{id}"))?;
    let cover = a
        .pointer(&format!("/albums/{id}"))
        .and_then(|g| best(g, "albumcover"))
        .ok_or(ProviderError::NotFound)?;
    let title = name(&a).ok_or(ProviderError::NotFound)?;
    Ok(json!({ "externalId": id, "title": title, "posterUrl": cover, "mediaType": "album" }))
}
