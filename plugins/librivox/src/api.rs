//! The LibriVox lookups over an injected transport, so recorded bodies can stand in for the network.
use crate::parse::{self, Book};
use direct_source::net::{Reply, Transport};
use fliks_plugin_sdk::ExternalId;
use fliks_plugin_sdk_provider::{ProviderError, SearchQuery};
use provider_common::http::{status_error, upstream};
use provider_common::lang::normalize_language_code;
use serde_json::Value;
use url::Url;

type R<T> = Result<T, ProviderError>;

const BASE: &str = "https://librivox.org/api/feed/audiobooks/";
const PAGE: u32 = 20;

fn get(t: &impl Transport, url: &Url) -> R<Reply> {
    let r = t.get(url.as_str()).map_err(|m| upstream(None, m))?;
    if (200..300).contains(&r.status) {
        return Ok(r);
    }
    Err(status_error(
        r.status,
        r.retry_after,
        format!("LibriVox answered {}", r.status),
    ))
}

/// One request of the feed; the API answers 404 when nothing matches, read as no books.
fn books(t: &impl Transport, params: &[(&str, &str)]) -> R<Vec<Book>> {
    let mut url = Url::parse(BASE).expect("constant URL");
    url.query_pairs_mut()
        .append_pair("format", "json")
        .extend_pairs(params);
    match get(t, &url) {
        Ok(r) => parse::parse_books(&r.body).map_err(|m| upstream(None, m)),
        Err(ProviderError::NotFound) => Ok(vec![]),
        Err(e) => Err(e),
    }
}

/// A title search, or on the first page with no hit the last word of the text as an author name.
pub fn search_books(
    t: &impl Transport,
    text: &str,
    language: Option<&str>,
    page: u32,
) -> R<Vec<Book>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(vec![]);
    }
    let page = page.max(1);
    let offset = ((page - 1) * PAGE).to_string();
    let limit = PAGE.to_string();
    let mut found = books(
        t,
        &[("title", text), ("limit", &limit), ("offset", &offset)],
    )?;
    if found.is_empty() && page == 1 {
        if let Some(last) = text.split_whitespace().last() {
            found = books(t, &[("author", last), ("limit", &limit)])?;
        }
    }
    let want = language
        .map(|l| normalize_language_code(Some(l)))
        .filter(|l| l != "und");
    if let Some(want) = want {
        found.retain(|b| b.language.as_deref().is_none_or(|l| l == want));
    }
    Ok(found)
}

pub fn search(t: &impl Transport, q: &SearchQuery) -> R<String> {
    if q.kind != "book" {
        return Err(ProviderError::Unsupported);
    }
    let filled = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let text = filled(&q.title)
        .or_else(|| filled(&q.author))
        .unwrap_or_else(|| q.query.trim().to_owned());
    let found = search_books(t, &text, q.language.as_deref(), q.page)?;
    Ok(Value::Array(found.iter().map(Book::record).collect()).to_string())
}

/// One book with its sections and readers; an unknown id is not found.
pub fn book(t: &impl Transport, id: &str) -> R<Book> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ProviderError::NotFound);
    }
    books(t, &[("id", id), ("extended", "1")])?
        .into_iter()
        .find(|b| b.id.to_string() == id)
        .ok_or(ProviderError::NotFound)
}

pub fn work(t: &impl Transport, kind: &str, id: &ExternalId) -> R<String> {
    if kind != "book" || id.provider != parse::NS {
        return Err(ProviderError::Unsupported);
    }
    Ok(book(t, &id.value)?.record().to_string())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Answers each request with the next recorded reply and keeps the URLs it was asked for.
    pub struct Replay(pub RefCell<Vec<(u16, String)>>, pub RefCell<Vec<String>>);

    impl Replay {
        pub fn new(replies: &[(u16, &str)]) -> Self {
            Replay(
                RefCell::new(
                    replies
                        .iter()
                        .rev()
                        .map(|(s, b)| (*s, (*b).to_owned()))
                        .collect(),
                ),
                RefCell::default(),
            )
        }
    }

    impl Transport for Replay {
        fn get(&self, url: &str) -> Result<Reply, String> {
            self.1.borrow_mut().push(url.to_owned());
            let (status, body) = self.0.borrow_mut().pop().ok_or("unexpected request")?;
            Ok(Reply {
                status,
                location: None,
                retry_after: Some(7),
                body: body.into_bytes(),
            })
        }
    }

    const SEARCH: &str = include_str!("../tests/fixtures/search.json");
    const BOOK: &str = include_str!("../tests/fixtures/book.json");
    const NOT_FOUND: &str = include_str!("../tests/fixtures/not-found.json");

    #[test]
    fn a_search_is_a_paged_title_query() {
        let t = Replay::new(&[(200, SEARCH)]);
        let found = search_books(&t, "sample", None, 2).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(
            t.1.borrow()[0],
            "https://librivox.org/api/feed/audiobooks/?format=json&title=sample&limit=20&offset=20"
        );
    }

    #[test]
    fn no_title_hit_falls_back_to_the_author_on_the_first_page() {
        let t = Replay::new(&[(404, NOT_FOUND), (200, SEARCH)]);
        assert_eq!(search_books(&t, "jane doe", None, 1).unwrap().len(), 2);
        assert!(t.1.borrow()[1].contains("author=doe"));
        let t = Replay::new(&[(404, NOT_FOUND)]);
        assert!(search_books(&t, "jane doe", None, 2).unwrap().is_empty());
    }

    #[test]
    fn the_language_filters_the_hits() {
        let t = Replay::new(&[(200, SEARCH)]);
        let found = search_books(&t, "sample", Some("fra"), 1).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, 2002);
    }

    #[test]
    fn an_empty_query_makes_no_request() {
        let t = Replay::new(&[]);
        assert!(search_books(&t, " ", None, 1).unwrap().is_empty());
        assert!(t.1.borrow().is_empty());
    }

    #[test]
    fn work_asks_for_the_extended_record() {
        let t = Replay::new(&[(200, BOOK)]);
        let id = ExternalId {
            provider: parse::NS.into(),
            value: "2001".into(),
        };
        let w: Value = serde_json::from_str(&work(&t, "book", &id).unwrap()).unwrap();
        assert_eq!(w["title"], "Sample Recording One");
        assert!(t.1.borrow()[0].ends_with("format=json&id=2001&extended=1"));
    }

    #[test]
    fn errors_map_to_provider_errors() {
        let id = |v: &str| ExternalId {
            provider: parse::NS.into(),
            value: v.into(),
        };
        let t = Replay::new(&[(404, NOT_FOUND)]);
        assert!(matches!(
            work(&t, "book", &id("5")),
            Err(ProviderError::NotFound)
        ));
        let t = Replay::new(&[(200, SEARCH)]);
        assert!(matches!(
            work(&t, "book", &id("5")),
            Err(ProviderError::NotFound)
        ));
        let t = Replay::new(&[(429, "")]);
        assert!(matches!(
            work(&t, "book", &id("5")),
            Err(ProviderError::RateLimited(Some(7)))
        ));
        let t = Replay::new(&[]);
        assert!(matches!(
            work(&t, "book", &id("x")),
            Err(ProviderError::NotFound)
        ));
        assert!(matches!(
            work(&t, "movie", &id("5")),
            Err(ProviderError::Unsupported)
        ));
    }
}
