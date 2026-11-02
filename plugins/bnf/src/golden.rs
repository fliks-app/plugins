//! Golden vectors of the parser (`BLESS=1` rewrites `expected`).
use crate::parse::*;
use provider_common::book::to_record;
use provider_common::golden::{Args, body, record_body, replay, replay_where, res};
use serde_json::json;

const DIR: &str = "tests/fixtures";
const RECORD: &str = "../../crates/provider-common/fixtures/books-record";

#[test]
fn bnf_reproduces_expected() {
    replay(DIR, 18, |i| {
        let a = Args(i);
        let b = || body(DIR, i);
        match i["fn"].as_str().unwrap() {
            "parse_search" => res(parse_search(&b())),
            "parse_record" => res(parse_record(&b())),
            "isbn_url" => json!(isbn_url(a.s("isbn").unwrap())),
            "title_author_url" => json!(title_author_url(
                a.s("title").unwrap(),
                a.s("author"),
                a.n("start").unwrap(),
                a.n("max").unwrap()
            )),
            "record_url" => json!(record_url(a.s("ark").unwrap())),
            f => panic!("unknown fn {f}"),
        }
    });
}

#[test]
fn books_record_reproduces_expected() {
    replay_where(
        RECORD,
        1,
        |i| i["provider"] == "bnf",
        |i| match i["fn"].as_str().unwrap() {
            "parse_record" => res(parse_record(&record_body(DIR, i)).map(|r| to_record(&r))),
            f => panic!("unknown fn {f}"),
        },
    );
}
