//! The Gutendex lookups over an injected transport, so recorded bodies can stand in for the network.
use crate::parse::{self, Book};
use direct_source::net::{Reply, Transport};
use fliks_plugin_sdk::ExternalId;
use fliks_plugin_sdk_provider::{ProviderError, SearchQuery};
use provider_common::http::{status_error, upstream};
use provider_common::lang::normalize_language_code;
use serde_json::Value;
use url::Url;

type R<T> = Result<T, ProviderError>;

const BASE: &str = "https://gutendex.com/books";

fn get(t: &impl Transport, url: &Url) -> R<Reply> {
    let r = t.get(url.as_str()).map_err(|m| upstream(None, m))?;
    if (200..300).contains(&r.status) {
        return Ok(r);
    }
    Err(status_error(
        r.status,
        r.retry_after,
        format!("Gutendex answered {}", r.status),
    ))
}

/// A search text and an optional language; a page is 32 books, 1-based.
pub fn search_url(text: &str, language: Option<&str>, page: u32) -> Url {
    let mut u = Url::parse(BASE).expect("constant URL");
    {
        let mut q = u.query_pairs_mut();
        q.append_pair("search", text);
        q.append_pair("mime_type", "application/epub");
        if let Some(l) = language {
            q.append_pair("languages", l);
        }
        if page > 1 {
            q.append_pair("page", &page.to_string());
        }
    }
    u
}

fn query_text(q: &SearchQuery) -> String {
    let filled = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    match (filled(&q.title), filled(&q.author)) {
        (Some(t), Some(a)) => format!("{t} {a}"),
        (Some(t), None) => t,
        (None, Some(a)) => a,
        (None, None) => q.query.trim().to_owned(),
    }
}

/// The books of a search; Gutendex answers a page past the end with a 404, read as no more books.
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
    let lang = language
        .map(|l| normalize_language_code(Some(l)))
        .filter(|l| l != "und");
    match get(t, &search_url(text, lang.as_deref(), page.max(1))) {
        Ok(r) => Ok(parse::parse_search(&r.body)
            .map_err(|m| upstream(None, m))?
            .1),
        Err(ProviderError::NotFound) => Ok(vec![]),
        Err(e) => Err(e),
    }
}

pub fn search(t: &impl Transport, q: &SearchQuery) -> R<String> {
    if q.kind != "book" {
        return Err(ProviderError::Unsupported);
    }
    let books = search_books(t, &query_text(q), q.language.as_deref(), q.page)?;
    Ok(Value::Array(books.iter().map(Book::record).collect()).to_string())
}

fn book_url(id: &str) -> Option<Url> {
    id.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| Url::parse(&format!("{BASE}/{id}")).ok())
        .flatten()
}

/// One book; an id Gutendex does not know, or one that has no EPUB, is not found.
pub fn book(t: &impl Transport, id: &str) -> R<Book> {
    let url = book_url(id).ok_or(ProviderError::NotFound)?;
    let r = get(t, &url)?;
    let v: Value = serde_json::from_slice(&r.body)
        .map_err(|_| upstream(Some(r.status), "invalid JSON response"))?;
    parse::parse_book(&v)
        .filter(|b| b.has_epub)
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

    fn query(title: Option<&str>, author: Option<&str>, text: &str) -> SearchQuery {
        SearchQuery {
            kind: "book".into(),
            query: text.into(),
            year: None,
            title: title.map(Into::into),
            author: author.map(Into::into),
            author_id: None,
            isbn: None,
            language: Some("fra".into()),
            page: 2,
            extra: vec![],
        }
    }

    #[test]
    fn search_asks_for_epubs_in_the_language_and_page() {
        let t = Replay::new(&[(200, SEARCH)]);
        let out: Value =
            serde_json::from_str(&search(&t, &query(Some("sample"), Some("doe"), "")).unwrap())
                .unwrap();
        assert_eq!(out.as_array().unwrap().len(), 2);
        assert_eq!(
            t.1.borrow()[0],
            "https://gutendex.com/books?search=sample+doe&mime_type=application%2Fepub&languages=fr&page=2"
        );
    }

    #[test]
    fn an_empty_query_makes_no_request() {
        let t = Replay::new(&[]);
        assert_eq!(search(&t, &query(None, None, "  ")).unwrap(), "[]");
        assert!(t.1.borrow().is_empty());
    }

    #[test]
    fn a_page_past_the_end_is_empty() {
        let t = Replay::new(&[(404, NOT_FOUND)]);
        assert_eq!(search(&t, &query(None, None, "x")).unwrap(), "[]");
    }

    #[test]
    fn work_reads_one_book() {
        let t = Replay::new(&[(200, BOOK)]);
        let id = ExternalId {
            provider: parse::NS.into(),
            value: "1001".into(),
        };
        let w: Value = serde_json::from_str(&work(&t, "book", &id).unwrap()).unwrap();
        assert_eq!(w["title"], "Sample Title One");
        assert_eq!(t.1.borrow()[0], "https://gutendex.com/books/1001");
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
        let t = Replay::new(&[(429, "")]);
        assert!(matches!(
            work(&t, "book", &id("5")),
            Err(ProviderError::RateLimited(Some(7)))
        ));
        let t = Replay::new(&[]);
        assert!(matches!(
            work(&t, "book", &id("../x")),
            Err(ProviderError::NotFound)
        ));
        assert!(matches!(
            work(&t, "movie", &id("5")),
            Err(ProviderError::Unsupported)
        ));
    }
}
