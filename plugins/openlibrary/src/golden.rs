//! Golden vectors of the parser (`BLESS=1` rewrites `expected`).
use crate::parse::*;
use provider_common::book::{BookRecord, ParseError, SearchPage, to_record};
use provider_common::golden::{Args, body, record_body, replay, replay_where, res};
use serde_json::{Value, json};

const DIR: &str = "tests/fixtures";
const RECORD: &str = "../../crates/provider-common/fixtures/books-record";

#[test]
fn openlibrary_reproduces_expected() {
    replay(DIR, 34, |i| {
        let a = Args(i);
        let b = || body(DIR, i);
        match i["fn"].as_str().unwrap() {
            "parse_search" => res(parse_search(&b())),
            "parse_work" => res(parse_work(&b())),
            "parse_edition" => res(parse_edition(&b())),
            "parse_editions" => res(parse_editions(&b())),
            "parse_isbn_batch" => res(parse_isbn_batch(&b())),
            "parse_author" => res(parse_author(&b())),
            "search_url" => json!(search_url(&Search {
                title: a.s("title"),
                author: a.s("author"),
                isbn: a.s("isbn"),
                q: a.s("q"),
                author_key: a.s("author_key"),
                lang: a.s("lang"),
                limit: a.n("limit"),
                page: a.n("page"),
            })),
            "isbn_url" => json!(isbn_url(a.s("isbn").unwrap())),
            "isbn_batch_url" => {
                let v = i["args"]["isbns"].as_array().unwrap();
                let v: Vec<&str> = v.iter().filter_map(Value::as_str).collect();
                json!(isbn_batch_url(&v))
            }
            "work_url" => json!(work_url(a.s("id").unwrap())),
            "work_editions_url" => json!(work_editions_url(
                a.s("id").unwrap(),
                a.n("limit").unwrap(),
                a.n("offset").unwrap()
            )),
            "edition_url" => json!(edition_url(a.s("id").unwrap())),
            "author_url" => json!(author_url(a.s("id").unwrap())),
            f => panic!("unknown fn {f}"),
        }
    });
}

#[test]
fn books_record_reproduces_expected() {
    let one = |r: Result<BookRecord, ParseError>| res(r.map(|r| to_record(&r)));
    let page = |r: Result<SearchPage, ParseError>| res(r.map(|p| many(&p.records)));
    let list = |r: Result<Vec<BookRecord>, ParseError>| res(r.map(|v| many(&v)));
    replay_where(
        RECORD,
        7,
        |i| i["provider"] == "openlibrary",
        |i| {
            let b = record_body(DIR, i);
            match i["fn"].as_str().unwrap() {
                "parse_search" => page(parse_search(&b)),
                "parse_editions" => page(parse_editions(&b)),
                "parse_work" => one(parse_work(&b)),
                "parse_edition" => one(parse_edition(&b)),
                "parse_isbn_batch" => list(parse_isbn_batch(&b)),
                f => panic!("unknown fn {f}"),
            }
        },
    );
}

fn many(records: &[BookRecord]) -> Value {
    records.iter().map(to_record).collect()
}
