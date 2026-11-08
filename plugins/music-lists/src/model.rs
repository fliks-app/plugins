//! What a list yields and how a list is configured.
use serde_json::{Map, Value};

pub type Settings = Map<String, Value>;

/// A track named by its ISRC or its MusicBrainz recording id.
#[derive(Clone, Debug, PartialEq)]
pub enum Rec {
    Isrc(String),
    Mbid(String),
}

/// One album or track of a list. `albums` are the external ids the host resolves an album by;
/// `deezer_track` is a track whose ISRC is still to be read.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    pub albums: Vec<(String, String)>,
    pub rec: Option<Rec>,
    pub deezer_track: Option<String>,
}

impl Entry {
    pub fn album(provider: &str, id: &str) -> Entry {
        Entry {
            key: format!("{provider}:{id}"),
            albums: vec![(provider.into(), id.into())],
            rec: None,
            deezer_track: None,
        }
    }

    pub fn track(rec: Rec, album: Option<(&str, &str)>) -> Entry {
        let key = match &rec {
            Rec::Isrc(i) => format!("isrc:{i}"),
            Rec::Mbid(m) => format!("recording:{m}"),
        };
        Entry {
            key,
            albums: album
                .map(|(p, i)| (p.into(), i.into()))
                .into_iter()
                .collect(),
            rec: Some(rec),
            deezer_track: None,
        }
    }
}

pub fn text(s: &Settings, key: &str) -> String {
    s.get(key)
        .and_then(Value::as_str)
        .map(|v| v.trim().to_owned())
        .unwrap_or_default()
}

pub fn required(s: &Settings, key: &str) -> Result<String, String> {
    Some(text(s, key))
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{key} is missing"))
}

/// The actions a list runs: `albums`, `tracks`, `playlist`.
pub fn actions(s: &Settings) -> Vec<String> {
    let mut a: Vec<String> = s
        .get("actions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .filter(|v| ["albums", "tracks", "playlist"].contains(v))
        .map(str::to_owned)
        .collect();
    a.sort();
    a.dedup();
    a
}

pub fn library(s: &Settings) -> Option<u32> {
    s.get("libraryId")
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.trim().parse().ok()))
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v > 0)
}

/// A lower-case canonical UUID, or none.
pub fn uuid(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_lowercase();
    let ok = s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        });
    ok.then_some(s)
}

/// The id at the end of a URL or the bare id: the last non-empty path segment without its query.
pub fn last_segment(s: &str) -> String {
    let s = s.trim().split(['?', '#']).next().unwrap_or("");
    s.rsplit('/')
        .find(|p| !p.is_empty())
        .unwrap_or("")
        .to_owned()
}

/// Spotify ids and ISRCs are alphanumeric: anything else would reshape a URL.
pub fn plain_id(s: &str) -> Option<String> {
    Some(s.to_owned()).filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_taken_from_urls() {
        assert_eq!(
            last_segment("https://www.deezer.com/en/playlist/123?x=1"),
            "123"
        );
        assert_eq!(last_segment(" 456 "), "456");
        assert_eq!(
            uuid("https://musicbrainz.org/recording/AAAAAAAA-aaaa-4aaa-8aaa-aaaaaaaaaaaa").or_else(
                || uuid(&last_segment(
                    "https://musicbrainz.org/recording/AAAAAAAA-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
                ))
            ),
            Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into())
        );
        assert_eq!(uuid("nope"), None);
        assert_eq!(plain_id("37i9dQZF1DX"), Some("37i9dQZF1DX".into()));
        assert_eq!(plain_id("a/b"), None);
    }

    #[test]
    fn actions_keep_the_known_ones_sorted() {
        let s: Settings =
            serde_json::from_str(r#"{"actions":["tracks","x","albums","tracks"]}"#).unwrap();
        assert_eq!(actions(&s), ["albums", "tracks"]);
        let s: Settings = serde_json::from_str(r#"{"libraryId":"3"}"#).unwrap();
        assert_eq!(library(&s), Some(3));
    }
}
