//! Golden vectors of the parser (`BLESS=1` rewrites `expected`).
use crate::parse::*;
use provider_common::book::to_record;
use provider_common::golden::{Args, body, record_body, replay, replay_where, res};
use serde_json::json;

const DIR: &str = "tests/fixtures";
const RECORD: &str = "../../crates/provider-common/fixtures/books-record";

#[test]
fn googlebooks_reproduces_expected() {
    replay(DIR, 18, |i| {
        let a = Args(i);
        let b = || body(DIR, i);
        match i["fn"].as_str().unwrap() {
            "parse_volumes" => res(parse_volumes(&b())),
            "parse_volume" => res(parse_volume(&b())),
            "search_url" => json!(search_url(&Search {
                title: a.s("title"),
                author: a.s("author"),
                isbn: a.s("isbn"),
                text: a.s("text"),
                lang: a.s("lang"),
                max_results: a.n("max_results"),
                start_index: a.n("start_index"),
            })),
            "volume_url" => json!(volume_url(a.s("id").unwrap())),
            f => panic!("unknown fn {f}"),
        }
    });
}

#[test]
fn books_record_reproduces_expected() {
    replay_where(
        RECORD,
        1,
        |i| i["provider"] == "googlebooks",
        |i| match i["fn"].as_str().unwrap() {
            "parse_volume" => res(parse_volume(&record_body(DIR, i)).map(|r| to_record(&r))),
            f => panic!("unknown fn {f}"),
        },
    );
}
