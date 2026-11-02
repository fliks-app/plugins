//! Golden vectors of the pending book parsers (`BLESS=1` rewrites `expected`).
use provider_common::book::pending::audible::{self, Region};
use provider_common::book::pending::audnexus;
use provider_common::book::to_record;
use provider_common::golden::{Args, body, record_body, replay, replay_where, res};
use serde_json::json;

const RECORD: &str = "fixtures/books-record";

fn region(a: &Args) -> Region {
    Region::from_code(a.s("region").unwrap()).unwrap()
}

#[test]
fn audible_reproduces_expected() {
    use audible::*;
    let dir = "fixtures/pending/audible";
    replay(dir, 18, |i| {
        let a = Args(i);
        let b = || body(dir, i);
        match i["fn"].as_str().unwrap() {
            "parse_search" => res(parse_search(&b())),
            "parse_product" => res(parse_product(&b())),
            "search_url" => json!(search_url(
                region(&a),
                &Search {
                    title: a.s("title"),
                    author: a.s("author"),
                    narrator: a.s("narrator"),
                    keywords: a.s("keywords"),
                    num_results: a.n("num_results"),
                    page: a.n("page"),
                }
            )),
            "product_url" => json!(product_url(region(&a), a.s("asin").unwrap())),
            "region_codes" => json!(Region::ALL.map(|r| json!([r.code(), r.tld()]))),
            f => panic!("unknown fn {f}"),
        }
    });
}

#[test]
fn audnexus_reproduces_expected() {
    use audnexus::*;
    let dir = "fixtures/pending/audnexus";
    replay(dir, 17, |i| {
        let a = Args(i);
        let b = || body(dir, i);
        let region = || a.s("region").map(|_| region(&a));
        match i["fn"].as_str().unwrap() {
            "parse_book" => res(parse_book(&b())),
            "parse_chapters" => res(parse_chapters(&b())),
            "parse_author" => res(parse_author(&b())),
            "book_url" => json!(book_url(a.s("asin").unwrap(), region())),
            "chapters_url" => json!(chapters_url(a.s("asin").unwrap(), region())),
            "author_url" => json!(author_url(a.s("asin").unwrap(), region())),
            f => panic!("unknown fn {f}"),
        }
    });
}

#[test]
fn books_record_reproduces_expected_for_the_pending_providers() {
    replay_where(
        RECORD,
        2,
        |i| matches!(i["provider"].as_str(), Some("audible" | "audnexus")),
        |i| {
            let provider = i["provider"].as_str().unwrap();
            let b = record_body(&format!("fixtures/pending/{provider}"), i);
            let r = match (i["provider"].as_str().unwrap(), i["fn"].as_str().unwrap()) {
                ("audible", "parse_product") => audible::parse_product(&b),
                ("audnexus", "parse_book") => audnexus::parse_book(&b),
                (p, f) => panic!("unknown fn {p}/{f}"),
            };
            res(r.map(|r| to_record(&r)))
        },
    );
}
