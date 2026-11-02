//! The Audible catalog lookups over an injected transport, so recorded bodies can stand in for the network.
use crate::parse::{self as audible, Search};
use fliks_plugin_sdk::ExternalId;
use fliks_plugin_sdk_provider::{ProviderError, SearchQuery};
use provider_common::book::region::Region;
use provider_common::book::{ParseError, to_record};
use provider_common::http::{status_error, upstream};
use serde_json::Value;

type R<T> = Result<T, ProviderError>;

#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub retry_after: Option<u32>,
    pub body: String,
}

pub trait Http {
    /// A transport failure is its message, which must carry no URL.
    fn get(&self, url: &str) -> Result<Reply, String>;
}

fn parse_err(e: ParseError) -> ProviderError {
    match e {
        ParseError::NotFound | ParseError::Moved(_) => ProviderError::NotFound,
        ParseError::RateLimited => ProviderError::RateLimited(None),
        ParseError::Malformed(m) | ParseError::Api(m) => upstream(None, m),
    }
}

const NS: &str = "asin";
const KIND: &str = "book";
const SEARCH_LIMIT: u32 = 20;

fn get(h: &impl Http, url: &str) -> R<String> {
    let r = h.get(url).map_err(|m| upstream(None, m))?;
    match r.status {
        200..=299 => Ok(r.body),
        s => Err(status_error(
            s,
            r.retry_after,
            format!("Audible answered {s}"),
        )),
    }
}

fn filled(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

pub fn search(h: &impl Http, q: &SearchQuery, region: Region) -> R<String> {
    if q.kind != KIND {
        return Err(ProviderError::Unsupported);
    }
    let (title, author) = (filled(&q.title), filled(&q.author));
    let free = Some(q.query.trim()).filter(|s| !s.is_empty());
    let s = Search {
        title,
        author,
        keywords: free.filter(|_| title.is_none() && author.is_none()),
        num_results: Some(SEARCH_LIMIT),
        page: Some(q.page.max(1) - 1),
        ..Default::default()
    };
    let Some(url) = audible::search_url(region, &s) else {
        return Ok("[]".into());
    };
    let page = audible::parse_search(&get(h, &url)?).map_err(parse_err)?;
    Ok(Value::Array(page.records.iter().map(to_record).collect()).to_string())
}

pub fn fetch_edition(h: &impl Http, edition: &ExternalId, region: Region) -> R<Option<String>> {
    let url = (edition.provider == NS)
        .then(|| audible::product_url(region, &edition.value))
        .flatten();
    let Some(url) = url else { return Ok(None) };
    let found = get(h, &url).and_then(|b| audible::parse_product(&b).map_err(parse_err));
    match found {
        Ok(r) => Ok(Some(to_record(&r).to_string())),
        Err(ProviderError::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    macro_rules! fixture {
        ($f:literal) => {
            include_str!(concat!("../tests/fixtures/", $f))
        };
    }

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            status,
            retry_after: None,
            body: body.into(),
        }
    }

    /// Answers every request with one reply and records what was asked.
    struct Fake {
        reply: Reply,
        seen: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(reply: Reply) -> Fake {
            Fake {
                reply,
                seen: RefCell::default(),
            }
        }
        fn asked(&self) -> Vec<String> {
            self.seen.borrow().clone()
        }
    }

    impl Http for Fake {
        fn get(&self, url: &str) -> Result<Reply, String> {
            self.seen.borrow_mut().push(url.to_owned());
            Ok(self.reply.clone())
        }
    }

    fn id(provider: &str, value: &str) -> ExternalId {
        ExternalId {
            provider: provider.into(),
            value: value.into(),
        }
    }

    fn json_of(s: String) -> Value {
        serde_json::from_str(&s).unwrap()
    }

    fn query(text: &str) -> SearchQuery {
        SearchQuery {
            kind: "book".into(),
            query: text.into(),
            year: None,
            title: None,
            author: None,
            author_id: None,
            isbn: None,
            language: None,
            page: 1,
            extra: vec![],
        }
    }

    #[test]
    fn search_maps_products_to_records() {
        let h = Fake::new(reply(200, fixture!("search-fr.json")));
        let mut q = query("ignored");
        q.title = Some("La Cartographe des Brumes".into());
        q.author = Some("Maëlis Quenotte".into());
        q.page = 3;
        let v = json_of(search(&h, &q, Region::Fr).unwrap());
        assert_eq!(v[0]["externalId"], "B0QZ7LM4NV");
        assert_eq!(v[0]["asin"], "B0QZ7LM4NV");
        let url = &h.asked()[0];
        assert!(url.starts_with("https://api.audible.fr/1.0/catalog/products?"));
        assert!(url.contains("title=La+Cartographe+des+Brumes"));
        assert!(url.contains("num_results=20&page=2"));
        assert!(!url.contains("keywords"));
    }

    #[test]
    fn a_record_without_a_title_is_dropped_from_a_page() {
        let h = Fake::new(reply(200, fixture!("search-com.json")));
        let v = json_of(search(&h, &query("lantern"), Region::Us).unwrap());
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert!(h.asked()[0].starts_with("https://api.audible.com/1.0/"));
        assert!(h.asked()[0].contains("keywords=lantern"));
    }

    #[test]
    fn an_empty_search_asks_nothing() {
        let h = Fake::new(reply(200, ""));
        assert_eq!(search(&h, &query("  "), Region::Us).unwrap(), "[]");
        assert!(h.asked().is_empty());
        let h = Fake::new(reply(200, fixture!("search-empty.json")));
        assert_eq!(search(&h, &query("nothing"), Region::Us).unwrap(), "[]");
    }

    #[test]
    fn a_product_is_an_edition_record() {
        let h = Fake::new(reply(200, fixture!("product-fr.json")));
        let r = fetch_edition(&h, &id("asin", "B0QZ7LM4NV"), Region::Fr).unwrap();
        let v = json_of(r.unwrap());
        assert_eq!(v["externalId"], "B0QZ7LM4NV");
        assert_eq!(v["format"], "audio");
        assert_eq!(v["language"], "fr");
        assert!(
            h.asked()[0].starts_with("https://api.audible.fr/1.0/catalog/products/B0QZ7LM4NV?")
        );
    }

    #[test]
    fn an_unknown_edition_is_none() {
        let h = Fake::new(reply(200, fixture!("product-skeleton.json")));
        let skeleton = fetch_edition(&h, &id("asin", "B0ZZZZZZZZ"), Region::Us).unwrap();
        assert!(skeleton.is_none());
        let h = Fake::new(reply(404, ""));
        assert!(
            fetch_edition(&h, &id("asin", "B0QZ7LM4NV"), Region::Us)
                .unwrap()
                .is_none()
        );
        let h = Fake::new(reply(200, ""));
        assert!(
            fetch_edition(&h, &id("isbn", "9782999000018"), Region::Us)
                .unwrap()
                .is_none()
        );
        assert!(
            fetch_edition(&h, &id("asin", "b0qz"), Region::Us)
                .unwrap()
                .is_none()
        );
        assert!(h.asked().is_empty());
    }

    #[test]
    fn upstream_failures_map_to_the_provider_errors() {
        let get = |r: Reply| search(&Fake::new(r), &query("x"), Region::Us).unwrap_err();
        let mut limited = reply(429, "");
        limited.retry_after = Some(7);
        assert!(matches!(get(limited), ProviderError::RateLimited(Some(7))));
        assert!(matches!(
            get(reply(429, "")),
            ProviderError::RateLimited(None)
        ));
        let e = get(reply(503, "maintenance"));
        assert!(matches!(e, ProviderError::Upstream(u) if u.status == Some(503)));
        let e = get(reply(200, r#"{"error": {"code": "x"}}"#));
        assert!(matches!(e, ProviderError::Upstream(u) if u.status.is_none()));
        let e = get(reply(200, "<html>"));
        assert!(matches!(e, ProviderError::Upstream(u) if u.status.is_none()));
        assert!(matches!(get(reply(404, "")), ProviderError::NotFound));
    }

    #[test]
    fn a_transport_failure_is_an_upstream_error() {
        struct Down;
        impl Http for Down {
            fn get(&self, _: &str) -> Result<Reply, String> {
                Err("connection refused".into())
            }
        }
        let e = search(&Down, &query("x"), Region::Us).unwrap_err();
        assert!(matches!(e, ProviderError::Upstream(u) if u.status.is_none()));
    }

    #[test]
    fn other_kinds_are_unsupported() {
        let mut q = query("x");
        q.kind = "movie".into();
        let e = search(&Fake::new(reply(200, "")), &q, Region::Us).unwrap_err();
        assert!(matches!(e, ProviderError::Unsupported));
    }
}
