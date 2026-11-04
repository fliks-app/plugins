//! The lookups against recorded bodies through a fake transport (no network).
use crate::api::*;
use fliks_plugin_sdk_provider::ProviderError;
use serde_json::{Value, json};
use std::cell::RefCell;

fn fx(name: &str) -> Value {
    let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Answers by path (and page for `/issue/`); records the queries made.
struct Fake {
    detail: &'static str,
    seen: RefCell<Vec<String>>,
}

impl Http for Fake {
    fn get(&self, path: &str, query: &[(&str, String)]) -> R<Value> {
        let page = query
            .iter()
            .find(|(k, _)| *k == "page")
            .map(|(_, v)| v.as_str());
        self.seen.borrow_mut().push(format!("{path}{query:?}"));
        Ok(match (path, page) {
            ("/series/", _) => fx("series_list.json"),
            ("/issue/", Some("2")) => fx("issues_p2.json"),
            ("/issue/", _) => fx("issues_p1.json"),
            _ => fx(self.detail),
        })
    }
}

fn fake(detail: &'static str) -> Fake {
    Fake {
        detail,
        seen: RefCell::default(),
    }
}

#[test]
fn search_maps_series() {
    let out = search(&fake("series_detail.json"), "placeholder", Some(2016), None).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(
        out[0],
        json!({
            "ids": {"fliks.metron:metron": "1001"}, "externalId": "1001", "mediaType": "comic",
            "title": "Placeholder Series", "overview": null, "year": 2016, "posterUrl": null,
            "publisher": null, "volume": 2, "issueCount": 12, "sourceUrl": null,
        })
    );
}

#[test]
fn work_maps_volume_with_first_issue_cover() {
    let w = work(&fake("series_detail.json"), "1001").unwrap();
    assert_eq!(w["title"], "Placeholder Series");
    assert_eq!(w["publisher"], "Placeholder Press");
    assert_eq!(w["year"], 2016);
    assert_eq!(w["volume"], 2);
    assert_eq!(w["overview"], "A placeholder description.");
    assert_eq!(
        w["posterUrl"],
        "https://static.metron.cloud/media/issue/2016/01/01/placeholder-2.jpg"
    );
}

#[test]
fn work_refuses_a_non_numeric_id() {
    assert!(matches!(
        work(&fake("series_detail.json"), "../x"),
        Err(ProviderError::NotFound)
    ));
}

#[test]
fn children_follow_pages_sort_and_drop_variants() {
    let h = fake("series_detail.json");
    let out = children(&h, "1001").unwrap();
    let got: Vec<(&str, f64, &str)> = out
        .iter()
        .map(|v| {
            (
                v["number"].as_str().unwrap(),
                v["sortNumber"].as_f64().unwrap(),
                v["kind"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("1", 1.0, "issue"),
            ("1.5", 1.5, "issue"),
            ("2", 2.0, "issue")
        ]
    );
    assert_eq!(out[0]["releaseDate"], "2016-05-11");
    assert_eq!(out[0]["coverDate"], "2016-06-01");
    assert_eq!(out[1]["releaseDate"], "2016-06-01");
    assert_eq!(out[1]["storeDate"], Value::Null);
    assert_eq!(h.seen.borrow().len(), 3);
}

#[test]
fn an_annual_series_gives_annual_issues() {
    let out = children(&fake("series_detail_annual.json"), "1003").unwrap();
    assert!(out.iter().all(|v| v["kind"] == "annual"));
}

#[test]
fn throttle_waits_for_retry_after_then_reset() {
    let body = fx("error_throttled.json").to_string();
    let e = reply(429, Some("42"), None, 0, body.as_bytes()).unwrap_err();
    assert!(matches!(e, ProviderError::RateLimited(Some(42))));
    let e = reply(
        429,
        None,
        Some("1700000030"),
        1_700_000_000,
        body.as_bytes(),
    )
    .unwrap_err();
    assert!(matches!(e, ProviderError::RateLimited(Some(30))));
    let e = reply(429, None, None, 0, body.as_bytes()).unwrap_err();
    assert!(matches!(e, ProviderError::RateLimited(None)));
}

#[test]
fn status_mapping() {
    assert!(matches!(
        reply(401, None, None, 0, b"{}"),
        Err(ProviderError::Unauthorized)
    ));
    assert!(matches!(
        reply(404, None, None, 0, b"{}"),
        Err(ProviderError::NotFound)
    ));
    assert!(reply(200, None, None, 0, b"{}").is_ok());
}
