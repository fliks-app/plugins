//! The lookup against recorded bodies (no network).
use crate::api::*;
use fliks_plugin_sdk_provider::{LyricsQuery, ProviderError};

fn fx(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn query(album: Option<&str>, duration_ms: Option<u32>) -> LyricsQuery {
    LyricsQuery {
        title: "Song".into(),
        artist: "Band".into(),
        album: album.map(Into::into),
        duration_ms,
        recording_id: None,
        isrc: None,
        language: None,
    }
}

#[test]
fn synced_lyrics_win_over_plain() {
    let h = reply(200, None, &fx("synced.json")).unwrap().unwrap();
    assert_eq!((h.format.as_str(), h.instrumental), ("lrc", false));
    assert!(h.document.starts_with("[00:17.12]"));
    assert_eq!(h.source_ref.as_deref(), Some("3396226"));
}

#[test]
fn plain_only_and_empty_records() {
    let h = reply(200, None, &fx("plain.json")).unwrap().unwrap();
    assert_eq!(
        (h.format.as_str(), h.document.as_str()),
        ("plain", "la la\n")
    );
    assert!(reply(200, None, &fx("empty.json")).unwrap().is_none());
}

#[test]
fn instrumental_is_an_empty_document() {
    let h = reply(200, None, &fx("instrumental.json")).unwrap().unwrap();
    assert!(h.instrumental && h.document.is_empty());
}

#[test]
fn statuses_map_to_the_provider_contract() {
    assert!(reply(404, None, b"{}").unwrap().is_none());
    assert!(matches!(
        reply(429, Some("7"), b"{}"),
        Err(ProviderError::RateLimited(Some(7)))
    ));
    assert!(matches!(
        reply(503, Some("1"), b"overloaded"),
        Err(ProviderError::RateLimited(Some(1)))
    ));
    assert!(matches!(
        reply(401, None, b"{}"),
        Err(ProviderError::Unauthorized)
    ));
    assert!(matches!(
        reply(500, None, b"{}"),
        Err(ProviderError::Upstream(_))
    ));
}

#[test]
fn duration_is_rounded_to_seconds_and_bounded() {
    let get =
        |p: &[(&str, String)], k: &str| p.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
    assert_eq!(
        get(&params(&query(Some("A"), Some(233_600))), "duration").as_deref(),
        Some("234")
    );
    assert_eq!(
        get(&params(&query(Some("A"), Some(233_400))), "duration").as_deref(),
        Some("233")
    );
    assert_eq!(get(&params(&query(None, Some(0))), "duration"), None);
    assert_eq!(
        get(&params(&query(None, Some(4_000_000))), "duration"),
        None
    );
    assert_eq!(get(&params(&query(Some(" "), None)), "album_name"), None);
    assert_eq!(
        get(&params(&query(Some("A"), None)), "album_name").as_deref(),
        Some("A")
    );
}
