//! The TheXEM lookups over an injected transport, so recorded bodies can stand in for the network.
use fliks_plugin_sdk_provider::{ProviderError, ScenePair};
use provider_common::http::{json_reply, upstream};
use serde_json::Value;

pub type R<T> = Result<T, ProviderError>;

/// The only origin TheXEM numbering is mapped from here.
pub const ORIGIN: &str = "tvdb";

pub trait Http {
    /// The `data` of a success answer; `None` for a `failure` one.
    fn get(&self, path: &str, query: &[(&str, String)]) -> R<Option<Value>>;
}

/// A TheXEM answer: `data` of a `result: success` body, `None` of a `result: failure` one (an
/// unmapped id answers 200 with `failure` and an empty `data`).
pub fn reply(status: u16, body: &[u8]) -> R<Option<Value>> {
    let v = json_reply(status, None, body)?;
    match v.get("result").and_then(Value::as_str) {
        Some("success") => Ok(Some(v.get("data").cloned().unwrap_or(Value::Null))),
        Some("failure") => Ok(None),
        _ => Err(upstream(Some(status), "unexpected TheXEM answer")),
    }
}

/// `/map/havemap?origin=tvdb`: the ids TheXEM maps, as strings.
pub fn mapped_ids(h: &impl Http, origin: &str) -> R<Vec<String>> {
    if origin != ORIGIN {
        return Err(ProviderError::Unsupported);
    }
    let v = h.get("/map/havemap", &[("origin", origin.to_owned())])?;
    let Some(Value::Array(ids)) = v else {
        return Err(upstream(None, "havemap data is not a list"));
    };
    Ok(ids
        .iter()
        .filter_map(|i| match i {
            Value::String(s) => Some(s.trim().to_owned()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .collect())
}

fn side(v: &Value) -> Option<(u32, u32)> {
    let n = |k: &str| u32::try_from(v.get(k)?.as_i64()?).ok();
    Some((n("season")?, n("episode")?))
}

/// `/map/all?origin=tvdb&id=<id>`: one pair per episode; an entry missing `scene` or `tvdb` is
/// skipped, and a mapping left with no pair is none.
pub fn mapping(h: &impl Http, origin: &str, id: &str) -> R<Option<Vec<ScenePair>>> {
    if origin != ORIGIN {
        return Err(ProviderError::Unsupported);
    }
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ProviderError::NotFound);
    }
    let q = [("origin", origin.to_owned()), ("id", id.to_owned())];
    let Some(data) = h.get("/map/all", &q)? else {
        return Ok(None);
    };
    let Value::Array(rows) = data else {
        return Err(upstream(None, "mapping data is not a list"));
    };
    let pairs: Vec<ScenePair> = rows
        .iter()
        .filter_map(|r| {
            let (scene_season, scene_episode) = side(r.get("scene")?)?;
            let (season, episode) = side(r.get("tvdb")?)?;
            Some(ScenePair {
                scene_season,
                scene_episode,
                season,
                episode,
            })
        })
        .collect();
    Ok((!pairs.is_empty()).then_some(pairs))
}
