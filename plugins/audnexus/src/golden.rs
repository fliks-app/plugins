//! Golden vectors of the parser (`BLESS=1` rewrites `expected`).
use crate::parse::*;
use provider_common::book::region::Region;
use provider_common::book::to_record;
use provider_common::golden::{Args, body, record_body, replay, replay_where, res};
use serde_json::json;

const DIR: &str = "tests/fixtures";
const RECORD: &str = "../../crates/provider-common/fixtures/books-record";

#[test]
fn audnexus_reproduces_expected() {
    replay(DIR, 14, |i| {
        let a = Args(i);
        let b = || body(DIR, i);
        let region = || a.s("region").map(|c| Region::from_code(c).unwrap());
        match i["fn"].as_str().unwrap() {
            "parse_book" => res(parse_book(&b())),
            "parse_chapters" => res(parse_chapters(&b())),
            "book_url" => json!(book_url(a.s("asin").unwrap(), region())),
            "chapters_url" => json!(chapters_url(a.s("asin").unwrap(), region())),
            f => panic!("unknown fn {f}"),
        }
    });
}

#[test]
fn books_record_reproduces_expected() {
    replay_where(
        RECORD,
        1,
        |i| i["provider"] == "audnexus",
        |i| match i["fn"].as_str().unwrap() {
            "parse_book" => res(parse_book(&record_body(DIR, i)).map(|r| to_record(&r))),
            f => panic!("unknown fn {f}"),
        },
    );
}
