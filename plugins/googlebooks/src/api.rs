//! The Google Books lookups over an injected transport, so recorded bodies can stand in for the network.
use book_providers::googlebooks::{self as gb, Search};
use book_providers::language::normalize_language_code;
use book_providers::{BookRecord, ParseError, to_record};
use fliks_plugin_sdk::ExternalId;
use fliks_plugin_sdk_provider::{ProviderError, SearchQuery, UpstreamError};
use serde_json::Value;

type R<T> = Result<T, ProviderError>;

const NS: &str = "googlebooks";
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

fn upstream(status: Option<u16>, message: impl Into<String>) -> ProviderError {
    ProviderError::Upstream(UpstreamError {
        status,
        message: message.into(),
    })
}

fn parse_err(e: ParseError) -> ProviderError {
    match e {
        ParseError::NotFound | ParseError::Moved(_) => ProviderError::NotFound,
        ParseError::RateLimited => ProviderError::RateLimited(None),
        ParseError::Malformed(m) | ParseError::Api(m) => upstream(None, m),
    }
}

/// Google also reports an exhausted quota as a 403 whose body names the reason.
fn get(h: &impl Http, url: &str) -> R<String> {
    let r = h.get(url).map_err(|m| upstream(None, m))?;
    match r.status {
        200..=299 => Ok(r.body),
        404 => Err(ProviderError::NotFound),
        s if s == 429 || gb::parse_volumes(&r.body) == Err(ParseError::RateLimited) => {
            Err(ProviderError::RateLimited(r.retry_after))
        }
        s => Err(upstream(Some(s), format!("Google Books answered {s}"))),
    }
}

fn filled(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

pub fn search(h: &impl Http, q: &SearchQuery) -> R<String> {
    if q.kind != KIND {
        return Err(ProviderError::Unsupported);
    }
    let isbn = filled(&q.isbn);
    let title = filled(&q.title);
    let lang = filled(&q.language)
        .map(|l| normalize_language_code(Some(l)))
        .filter(|l| l != "und");
    // An ISBN alone: the other terms would only narrow it.
    let by_isbn = isbn.is_some_and(|i| book_providers::isbn::parse(i).is_some());
    let page = q.page.max(1);
    let search = Search {
        isbn: isbn.filter(|_| by_isbn),
        title: title.filter(|_| !by_isbn),
        author: filled(&q.author).filter(|_| !by_isbn),
        text: Some(q.query.trim()).filter(|s| !s.is_empty() && title.is_none() && !by_isbn),
        lang: lang.as_deref(),
        max_results: Some(SEARCH_LIMIT),
        start_index: Some((page - 1) * SEARCH_LIMIT),
    };
    let Some(url) = gb::search_url(&search) else {
        return Ok("[]".into());
    };
    let page = gb::parse_volumes(&get(h, &url)?).map_err(parse_err)?;
    Ok(Value::Array(page.records.iter().map(to_record).collect()).to_string())
}

fn optional<T>(r: R<T>) -> R<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(ProviderError::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The volume, or the first hit of an ISBN query.
fn record(h: &impl Http, url: String, by_isbn: bool) -> R<Option<BookRecord>> {
    let body = get(h, &url);
    let Some(body) = optional(body)? else {
        return Ok(None);
    };
    let parsed = if by_isbn {
        gb::parse_volumes(&body)
            .and_then(|p| p.records.into_iter().next().ok_or(ParseError::NotFound))
    } else {
        gb::parse_volume(&body)
    };
    optional(parsed.map_err(parse_err))
}

pub fn fetch_edition(h: &impl Http, edition: &ExternalId) -> R<Option<String>> {
    let (url, by_isbn) = match edition.provider.as_str() {
        "isbn" => (
            gb::search_url(&Search {
                isbn: Some(&edition.value),
                max_results: Some(1),
                ..Search::default()
            }),
            true,
        ),
        NS => (gb::volume_url(&edition.value), false),
        _ => (None, false),
    };
    let Some(url) = url else { return Ok(None) };
    Ok(record(h, url, by_isbn)?.map(|r| to_record(&r).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    macro_rules! fixture {
        ($f:literal) => {
            include_str!(concat!(
                "../../../crates/book-providers/fixtures/googlebooks/",
                $f
            ))
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
    fn search_maps_volumes_to_records() {
        let h = Fake::new(reply(200, fixture!("volumes-search.json")));
        let mut q = query("ignored");
        q.title = Some("Placeholder Title".into());
        q.author = Some("Maëlis Quenotte".into());
        q.language = Some("fr-FR".into());
        q.page = 3;
        let v = json_of(search(&h, &q).unwrap());
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[0]["externalId"], "aB1cD2eF3gHI");
        let url = &h.asked()[0];
        assert!(url.contains("intitle%3A%22Placeholder+Title%22"));
        assert!(url.contains("langRestrict=fr&maxResults=20&startIndex=40"));
    }

    #[test]
    fn an_isbn_search_sends_the_isbn_alone() {
        let h = Fake::new(reply(200, fixture!("volumes-isbn.json")));
        let mut q = query("");
        q.isbn = Some("978-2-99900-001-8".into());
        q.title = Some("Placeholder Title".into());
        let v = json_of(search(&h, &q).unwrap());
        assert_eq!(v[0]["externalId"], "zX3kLmN0pQsC");
        let url = &h.asked()[0];
        assert!(url.contains("q=isbn%3A9782999000018&"));
        assert!(!url.contains("intitle"));
    }

    #[test]
    fn an_empty_search_asks_nothing() {
        let h = Fake::new(reply(200, ""));
        assert_eq!(search(&h, &query("  ")).unwrap(), "[]");
        assert!(h.asked().is_empty());
        let h = Fake::new(reply(200, fixture!("volumes-empty.json")));
        assert_eq!(search(&h, &query("nothing")).unwrap(), "[]");
    }

    #[test]
    fn a_volume_is_an_edition_record() {
        let h = Fake::new(reply(200, fixture!("volume.json")));
        let r = fetch_edition(&h, &id("googlebooks", "zX3kLmN0pQsC")).unwrap();
        let v = json_of(r.unwrap());
        assert_eq!(v["externalId"], "zX3kLmN0pQsC");
        assert_eq!(v["isbn13"], json!(["9781999000028"]));
        assert!(h.asked()[0].ends_with("/volumes/zX3kLmN0pQsC"));
    }

    #[test]
    fn an_isbn_reads_the_first_hit() {
        let h = Fake::new(reply(200, fixture!("volumes-isbn.json")));
        let r = fetch_edition(&h, &id("isbn", "9782999000018")).unwrap();
        assert_eq!(json_of(r.unwrap())["externalId"], "zX3kLmN0pQsC");
        assert!(h.asked()[0].contains("maxResults=1"));
        let h = Fake::new(reply(200, fixture!("volumes-empty.json")));
        assert!(
            fetch_edition(&h, &id("isbn", "9782999000018"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn an_unknown_edition_is_none_without_a_call_when_unaddressable() {
        let h = Fake::new(reply(404, fixture!("error-notfound.json")));
        assert!(
            fetch_edition(&h, &id("googlebooks", "zX3kLmN0pQsC"))
                .unwrap()
                .is_none()
        );
        let h = Fake::new(reply(200, ""));
        assert!(
            fetch_edition(&h, &id("asin", "B0QZ7LM4NV"))
                .unwrap()
                .is_none()
        );
        assert!(
            fetch_edition(&h, &id("googlebooks", "a/../b"))
                .unwrap()
                .is_none()
        );
        assert!(fetch_edition(&h, &id("isbn", "bad")).unwrap().is_none());
        assert!(h.asked().is_empty());
    }

    #[test]
    fn upstream_failures_map_to_the_provider_errors() {
        let get = |r: Reply| search(&Fake::new(r), &query("x")).unwrap_err();
        let mut limited = reply(429, fixture!("error-quota.json"));
        limited.retry_after = Some(7);
        assert!(matches!(get(limited), ProviderError::RateLimited(Some(7))));
        let quota = reply(403, fixture!("error-quota.json"));
        assert!(matches!(get(quota), ProviderError::RateLimited(None)));
        let e = get(reply(400, fixture!("error-badrequest.json")));
        assert!(matches!(e, ProviderError::Upstream(u) if u.status == Some(400)));
        let e = get(reply(503, "maintenance"));
        assert!(matches!(e, ProviderError::Upstream(u) if u.status == Some(503)));
        let e = get(reply(200, "not json"));
        assert!(matches!(e, ProviderError::Upstream(u) if u.status.is_none()));
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
