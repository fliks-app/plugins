//! The lookups against recorded bodies through a fake transport (no network).
use crate::api::*;
use fliks_plugin_sdk_provider::ProviderError;
use serde_json::{Value, json};

const ARTIST: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const GROUP: &str = "11111111-1111-4111-8111-111111111111";

fn fx(name: &str) -> Value {
    let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

struct Fake(&'static str, &'static str);

impl Http for Fake {
    fn get(&self, path: &str) -> R<Value> {
        match path {
            p if p == format!("/music/{ARTIST}") => Ok(fx(self.0)),
            p if p == format!("/music/albums/{GROUP}") => Ok(fx(self.1)),
            _ => Err(ProviderError::NotFound),
        }
    }
}

#[test]
fn the_person_is_the_most_liked_thumb_over_https() {
    let p = person(&Fake("artist.json", ""), ARTIST).unwrap();
    assert_eq!(
        p,
        json!({"name": "Example Artist", "avatarUrl": "https://assets.fanart.tv/fanart/music/thumb-b.jpg"})
    );
}

#[test]
fn an_artist_without_a_thumb_or_an_unknown_one_is_not_found() {
    let h = Fake("artist_no_thumb.json", "");
    assert!(matches!(person(&h, ARTIST), Err(ProviderError::NotFound)));
    assert!(matches!(
        person(&h, "5b11f4ce-a62d-471e-81fc-a69a8278c7da"),
        Err(ProviderError::NotFound)
    ));
    assert!(matches!(person(&h, "nope"), Err(ProviderError::NotFound)));
}

#[test]
fn the_work_is_the_most_liked_album_cover() {
    let w = work(&Fake("", "album.json"), &GROUP.to_uppercase()).unwrap();
    assert_eq!(w["externalId"], GROUP);
    assert_eq!(w["title"], "Example Artist");
    assert_eq!(
        w["posterUrl"],
        "https://assets.fanart.tv/fanart/music/cover-b.jpg"
    );
    assert_eq!(w["mediaType"], "album");
}

#[test]
fn a_group_with_only_cd_art_has_no_cover() {
    assert!(matches!(
        work(&Fake("", "album_cd_only.json"), GROUP),
        Err(ProviderError::NotFound)
    ));
}

#[test]
fn replies_map_to_provider_errors() {
    assert!(matches!(
        reply(404, None, b"{}"),
        Err(ProviderError::NotFound)
    ));
    assert!(matches!(
        reply(401, None, b"{}"),
        Err(ProviderError::Unauthorized)
    ));
    assert!(matches!(
        reply(429, Some("3"), b"{}"),
        Err(ProviderError::RateLimited(Some(3)))
    ));
}

#[test]
fn records_pass_the_host_checks() {
    use fliks_domain::music::provider as host;
    let h = Fake("artist.json", "album.json");
    host::person(&person(&h, ARTIST).unwrap()).unwrap();
    host::album(&work(&h, GROUP).unwrap()).unwrap();
}
