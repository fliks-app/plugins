//! The lookups against recorded TheXEM bodies through a fake transport (no network).
use crate::api::*;
use fliks_plugin_sdk_provider::ProviderError;
use serde_json::Value;

fn fx(name: &str) -> Vec<u8> {
    let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(p).unwrap()
}

/// Answers each path with a recorded body.
struct Fake {
    havemap: &'static str,
    all: &'static str,
}

impl Http for Fake {
    fn get(&self, path: &str, _: &[(&str, String)]) -> R<Option<Value>> {
        let body = fx(if path == "/map/havemap" {
            self.havemap
        } else {
            self.all
        });
        reply(200, &body)
    }
}

const FAKE: Fake = Fake {
    havemap: "havemap.json",
    all: "map-all.json",
};

#[test]
fn the_havemap_lists_its_ids_as_strings() {
    let ids = mapped_ids(&FAKE, "tvdb").unwrap();
    assert_eq!(ids, ["70668", "70900", "71146", "71313", "375921"]);
}

#[test]
fn a_recorded_mapping_gives_one_pair_per_episode() {
    let pairs = mapping(&FAKE, "tvdb", "375921").unwrap().unwrap();
    assert_eq!(pairs.len(), 25);
    let at = |ss, se| {
        pairs
            .iter()
            .find(|p| p.scene_season == ss && p.scene_episode == se)
            .map(|p| (p.season, p.episode))
    };
    assert_eq!(at(2, 1), Some((1, 6)));
    assert_eq!(at(4, 1), Some((3, 1)));
}

#[test]
fn an_unmapped_id_is_none_not_an_error() {
    let h = Fake {
        havemap: "havemap.json",
        all: "map-unknown.json",
    };
    assert!(mapping(&h, "tvdb", "1").unwrap().is_none());
}

#[test]
fn an_entry_missing_scene_or_tvdb_is_skipped() {
    let h = Fake {
        havemap: "havemap.json",
        all: "map-ambiguous.json",
    };
    let pairs = mapping(&h, "tvdb", "1").unwrap().unwrap();
    assert_eq!(pairs.len(), 8, "8 of the 10 recorded entries are whole");
}

#[test]
fn an_empty_mapping_is_none() {
    let body = br#"{"result":"success","data":[]}"#;
    struct One(&'static [u8]);
    impl Http for One {
        fn get(&self, _: &str, _: &[(&str, String)]) -> R<Option<Value>> {
            reply(200, self.0)
        }
    }
    assert!(mapping(&One(body), "tvdb", "1").unwrap().is_none());
}

#[test]
fn a_bad_request_is_refused_before_any_call() {
    assert!(matches!(
        mapping(&FAKE, "anidb", "1"),
        Err(ProviderError::Unsupported)
    ));
    assert!(matches!(
        mapping(&FAKE, "tvdb", "12a"),
        Err(ProviderError::NotFound)
    ));
    assert!(matches!(
        mapped_ids(&FAKE, "anidb"),
        Err(ProviderError::Unsupported)
    ));
}

#[test]
fn a_failed_status_or_body_is_an_upstream_error() {
    assert!(matches!(
        reply(503, b"{}"),
        Err(ProviderError::Upstream(u)) if u.status == Some(503)
    ));
    assert!(reply(200, b"<html>blocked</html>").is_err());
    assert!(reply(200, br#"{"foo":1}"#).is_err());
}
