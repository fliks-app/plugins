//! Golden vectors of the parser (`BLESS=1` rewrites `expected`).
use crate::parse::*;
use provider_common::book::region::Region;
use provider_common::book::to_record;
use provider_common::golden::{Args, body, record_body, replay, replay_where, res};
use serde_json::json;

const DIR: &str = "tests/fixtures";
const RECORD: &str = "../../crates/provider-common/fixtures/books-record";

fn region(a: &Args) -> Region {
    Region::from_code(a.s("region").unwrap()).unwrap()
}

#[test]
fn audible_reproduces_expected() {
    replay(DIR, 18, |i| {
        let a = Args(i);
        let b = || body(DIR, i);
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
fn books_record_reproduces_expected() {
    replay_where(
        RECORD,
        1,
        |i| i["provider"] == "audible",
        |i| match i["fn"].as_str().unwrap() {
            "parse_product" => res(parse_product(&record_body(DIR, i)).map(|r| to_record(&r))),
            f => panic!("unknown fn {f}"),
        },
    );
}
