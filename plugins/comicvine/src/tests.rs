//! The lookups against recorded bodies through a fake transport (no network).
use crate::api::*;
use fliks_plugin_sdk_provider::ProviderError;
use serde_json::{Value, json};
use std::cell::RefCell;

fn fx(name: &str) -> Value {
    let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

struct Fake {
    volume: &'static str,
    /// The `offset` of every `/issues/` call.
    offsets: RefCell<Vec<String>>,
}

impl Http for Fake {
    fn get(&self, path: &str, query: &[(&str, String)]) -> R<Value> {
        Ok(match path {
            "/search/" => fx("search.json"),
            "/issues/" => {
                let off = query
                    .iter()
                    .find(|(k, _)| *k == "offset")
                    .unwrap()
                    .1
                    .clone();
                self.offsets.borrow_mut().push(off);
                fx("issues_p1.json")
            }
            _ => fx(self.volume),
        })
    }
}

fn fake(volume: &'static str) -> Fake {
    Fake {
        volume,
        offsets: RefCell::default(),
    }
}

#[test]
fn search_maps_volumes_and_filters_the_year() {
    let h = fake("volume.json");
    let all = search(&h, "placeholder", None, None).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(
        all[0],
        json!({
            "ids": {"fliks.comicvine:comicvine": "4001"}, "externalId": "4001", "mediaType": "comic",
            "title": "Placeholder Series", "overview": "A deck.",
            "year": 2016,
            "posterUrl": "https://comicvine.gamespot.com/a/uploads/scale_large/0/1/placeholder.jpg",
            "publisher": "Placeholder Press", "volume": null, "issueCount": 12,
            "sourceUrl": "https://comicvine.gamespot.com/placeholder-series/4050-4001/",
        })
    );
    let y = search(&h, "placeholder", Some(1998), None).unwrap();
    assert_eq!(y.len(), 1);
    assert_eq!(y[0]["externalId"], "4002");
}

#[test]
fn work_maps_the_volume() {
    let w = work(&fake("volume.json"), "4001").unwrap();
    assert_eq!(w["overview"], "A deck.");
    assert_eq!(w["year"], 2016);
    assert_eq!(
        w["posterUrl"],
        "https://comicvine.gamespot.com/a/uploads/scale_medium/0/1/placeholder.jpg"
    );
    assert!(matches!(
        work(&fake("volume.json"), "4001/../x"),
        Err(ProviderError::NotFound)
    ));
}

#[test]
fn children_sort_and_read_dates() {
    let h = fake("volume.json");
    let out = children(&h, "4001").unwrap();
    let nums: Vec<&str> = out.iter().map(|v| v["number"].as_str().unwrap()).collect();
    assert_eq!(nums, ["1", "1.5", "2", "1"]);
    assert_eq!(*h.offsets.borrow(), ["0"]);
    assert_eq!(out[0]["coverDate"], Value::Null);
    assert_eq!(out[1]["releaseDate"], "2016-05-25");
    assert_eq!(out[2]["releaseDate"], "2016-06-08");
    assert_eq!(out[2]["title"], "Second");
}

#[test]
fn an_annual_volume_gives_annual_issues() {
    let out = children(&fake("volume.json"), "4003").unwrap();
    assert!(out.iter().any(|v| v["kind"] == "annual"));
    assert!(out.iter().any(|v| v["kind"] == "issue"));
}

struct Endless;

impl Http for Endless {
    fn get(&self, _p: &str, query: &[(&str, String)]) -> R<Value> {
        let off = &query.iter().find(|(k, _)| *k == "offset").unwrap().1;
        let mut page = fx("issues_p1.json");
        page["number_of_total_results"] = json!(1_000_000);
        page["results"][0]["id"] = json!(off.parse::<u64>().unwrap() + 100_000);
        Ok(page)
    }
}

#[test]
fn a_volume_past_the_page_cap_is_an_error() {
    assert!(children(&Endless, "4001").is_err());
}

#[test]
fn replies_map_to_provider_errors() {
    let body = |n: &str| fx(n).to_string();
    assert!(matches!(
        reply(401, body("error_key.json").as_bytes()),
        Err(ProviderError::Unauthorized)
    ));
    assert!(matches!(
        reply(200, body("error_key.json").as_bytes()),
        Err(ProviderError::Unauthorized)
    ));
    assert!(matches!(
        reply(200, body("error_missing.json").as_bytes()),
        Err(ProviderError::NotFound)
    ));
    assert!(matches!(
        reply(420, b"{}"),
        Err(ProviderError::RateLimited(None))
    ));
    assert!(reply(200, body("volume.json").as_bytes()).is_ok());
}
