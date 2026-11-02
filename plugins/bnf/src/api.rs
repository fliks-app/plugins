//! The BnF SRU lookups over an injected transport, so recorded bodies can stand in for the network.
use crate::parse as bnf;
use fliks_plugin_sdk::ExternalId;
use fliks_plugin_sdk_provider::{ProviderError, SearchQuery};
use provider_common::book::{ParseError, to_record};
use provider_common::http::{status_error, upstream};
use serde_json::Value;

type R<T> = Result<T, ProviderError>;

const NS: &str = "bnf";
const KIND: &str = "book";
const SEARCH_LIMIT: u32 = 20;

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

fn get(h: &impl Http, url: &str) -> R<String> {
    let r = h.get(url).map_err(|m| upstream(None, m))?;
    match r.status {
        200..=299 => Ok(r.body),
        s => Err(status_error(s, r.retry_after, format!("BnF answered {s}"))),
    }
}

fn filled(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

pub fn search(h: &impl Http, q: &SearchQuery) -> R<String> {
    if q.kind != KIND {
        return Err(ProviderError::Unsupported);
    }
    let free = Some(q.query.trim()).filter(|s| !s.is_empty());
    let by_isbn = filled(&q.isbn).and_then(bnf::isbn_url);
    let start = (q.page.max(1) - 1) * SEARCH_LIMIT + 1;
    let by_title = filled(&q.title)
        .or(free)
        .and_then(|t| bnf::title_author_url(t, filled(&q.author), start, SEARCH_LIMIT));
    let Some(url) = by_isbn.or(by_title) else {
        return Ok("[]".into());
    };
    let page = bnf::parse_search(&get(h, &url)?).map_err(parse_err)?;
    Ok(Value::Array(page.records.iter().map(to_record).collect()).to_string())
}

pub fn fetch_edition(h: &impl Http, edition: &ExternalId) -> R<Option<String>> {
    let url = match edition.provider.as_str() {
        "isbn" => bnf::isbn_url(&edition.value),
        NS => bnf::record_url(&edition.value),
        _ => None,
    };
    let Some(url) = url else { return Ok(None) };
    let found = get(h, &url).and_then(|b| bnf::parse_record(&b).map_err(parse_err));
    match found {
        Ok(r) => Ok(Some(to_record(&r).to_string())),
        Err(ProviderError::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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

    fn json_of(s: String) -> Value {
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn search_maps_notices_to_records() {
        let h = Fake::new(reply(200, fixture!("unimarc-search.xml")));
        let mut q = query("ignored");
        q.title = Some("Placeholder Title".into());
        q.author = Some("Maëlis Quenotte".into());
        q.page = 3;
        let v = json_of(search(&h, &q).unwrap());
        assert_eq!(v[0]["externalId"], "cb99900022m");
        assert_eq!(v[0]["pageCount"], 410);
        let url = &h.asked()[0];
        assert!(url.contains("bib.title+all+%22Placeholder+Title%22"));
        assert!(url.contains("startRecord=41&maximumRecords=20"));
    }

    #[test]
    fn a_free_query_searches_titles() {
        let h = Fake::new(reply(200, fixture!("unimarc-search.xml")));
        search(&h, &query("clockmaker")).unwrap();
        assert!(h.asked()[0].contains("bib.title+all+%22clockmaker%22"));
    }

    #[test]
    fn an_isbn_search_ignores_the_title() {
        let h = Fake::new(reply(200, fixture!("unimarc-isbn.xml")));
        let mut q = query("");
        q.isbn = Some("978-2-99900-001-8".into());
        q.title = Some("Placeholder Title".into());
        let v = json_of(search(&h, &q).unwrap());
        assert_eq!(v[0]["externalId"], "cb99900011d");
        assert!(h.asked()[0].contains("bib.isbn+adj+%229782999000018%22"));
    }

    #[test]
    fn an_empty_search_asks_nothing() {
        let h = Fake::new(reply(200, ""));
        assert_eq!(search(&h, &query("  ")).unwrap(), "[]");
        assert!(h.asked().is_empty());
        let h = Fake::new(reply(200, fixture!("unimarc-empty.xml")));
        assert_eq!(search(&h, &query("nothing")).unwrap(), "[]");
    }

    #[test]
    fn a_notice_is_an_edition_record() {
        let h = Fake::new(reply(200, fixture!("unimarc-isbn.xml")));
        let r = fetch_edition(&h, &id("bnf", "cb99900011d")).unwrap();
        let v = json_of(r.unwrap());
        assert_eq!(v["externalId"], "cb99900011d");
        assert_eq!(v["isbn13"], json!(["9782999000018"]));
        assert!(h.asked()[0].contains("bib.persistentid"));
        let r = fetch_edition(&h, &id("isbn", "2999000014")).unwrap();
        assert!(r.is_some());
    }

    #[test]
    fn an_unknown_edition_is_none() {
        let h = Fake::new(reply(200, fixture!("unimarc-empty.xml")));
        assert!(
            fetch_edition(&h, &id("bnf", "cb99900099x"))
                .unwrap()
                .is_none()
        );
        let h = Fake::new(reply(200, ""));
        assert!(
            fetch_edition(&h, &id("asin", "B0QZ7LM4NV"))
                .unwrap()
                .is_none()
        );
        assert!(fetch_edition(&h, &id("bnf", "xx1")).unwrap().is_none());
        assert!(fetch_edition(&h, &id("isbn", "nope")).unwrap().is_none());
        assert!(h.asked().is_empty());
    }

    #[test]
    fn upstream_failures_map_to_the_provider_errors() {
        let get = |r: Reply| search(&Fake::new(r), &query("x")).unwrap_err();
        let mut limited = reply(429, "");
        limited.retry_after = Some(7);
        assert!(matches!(get(limited), ProviderError::RateLimited(Some(7))));
        let e = get(reply(503, "maintenance"));
        assert!(matches!(e, ProviderError::Upstream(u) if u.status == Some(503)));
        let e = get(reply(200, fixture!("diagnostic.xml")));
        assert!(matches!(e, ProviderError::Upstream(u) if u.status.is_none()));
        let e = get(reply(200, "<a><b></a>"));
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
        let e = search(&Down, &query("x")).unwrap_err();
        assert!(matches!(e, ProviderError::Upstream(u) if u.status.is_none()));
    }

    #[test]
    fn other_kinds_are_unsupported() {
        let mut q = query("x");
        q.kind = "movie".into();
        let e = search(&Fake::new(reply(200, "")), &q).unwrap_err();
        assert!(matches!(e, ProviderError::Unsupported));
    }
}
