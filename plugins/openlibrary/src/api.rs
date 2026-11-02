//! The OpenLibrary lookups over an injected transport, so recorded bodies can stand in for the network.
use crate::parse::{self as ol, Search};
use fliks_plugin_sdk::ExternalId;
use fliks_plugin_sdk_provider::{ProviderError, SearchQuery};
use provider_common::book::{BookRecord, ParseError, to_record};
use provider_common::http::{status_error, upstream};
use provider_common::lang::normalize_language_code;
use serde_json::Value;

type R<T> = Result<T, ProviderError>;

const NS: &str = "openlibrary";
const KIND: &str = "book";
const HOST: &str = "openlibrary.org";
const REDIRECTS: usize = 3;
const SEARCH_LIMIT: u32 = 20;
const EDITIONS_LIMIT: u32 = 100;
const AUTHOR_LOOKUPS: usize = 5;

#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub retry_after: Option<u32>,
    pub location: Option<String>,
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

/// Where a redirect leads, only within OpenLibrary.
fn redirect(from: &str, location: Option<&str>) -> R<String> {
    let to = location
        .and_then(|l| url::Url::parse(from).ok()?.join(l).ok())
        .filter(|u| u.scheme() == "https" && u.host_str() == Some(HOST));
    to.map(String::from)
        .ok_or_else(|| upstream(None, "redirect outside OpenLibrary"))
}

fn get(h: &impl Http, url: &str) -> R<String> {
    let mut url = url.to_owned();
    for _ in 0..=REDIRECTS {
        let r = h.get(&url).map_err(|m| upstream(None, m))?;
        match r.status {
            200..=299 => return Ok(r.body),
            301 | 302 | 303 | 307 | 308 => url = redirect(&url, r.location.as_deref())?,
            s => {
                return Err(status_error(
                    s,
                    r.retry_after,
                    format!("OpenLibrary answered {s}"),
                ));
            }
        }
    }
    Err(upstream(None, "too many redirects"))
}

/// Fetches and parses, following a merged record to its target once.
fn load<T>(
    h: &impl Http,
    url: String,
    target: fn(&str) -> Option<String>,
    parse: fn(&str) -> Result<T, ParseError>,
) -> R<T> {
    match parse(&get(h, &url)?) {
        Err(ParseError::Moved(Some(key))) => {
            let url = target(&key).ok_or(ProviderError::NotFound)?;
            parse(&get(h, &url)?).map_err(parse_err)
        }
        r => r.map_err(parse_err),
    }
}

fn optional<T>(r: R<T>) -> R<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(ProviderError::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Work and edition authors are bare references; names are best effort, the record stands without.
fn name_authors(h: &impl Http, r: &mut BookRecord) {
    let todo = r.authors.iter_mut().filter(|p| p.name.is_none());
    for p in todo.filter(|p| p.id.is_some()).take(AUTHOR_LOOKUPS) {
        let url = p.id.as_deref().and_then(ol::author_url);
        let Some(url) = url else { continue };
        match load(h, url, ol::author_url, ol::parse_author) {
            Ok(a) => p.name = Some(a.name),
            Err(_) => break,
        }
    }
}

fn records(list: &[BookRecord]) -> String {
    Value::Array(list.iter().map(to_record).collect()).to_string()
}

fn book(kind: &str) -> R<()> {
    (kind == KIND)
        .then_some(())
        .ok_or(ProviderError::Unsupported)
}

fn olid(id: &ExternalId) -> R<&str> {
    (id.provider == NS)
        .then_some(id.value.as_str())
        .ok_or(ProviderError::Unsupported)
}

fn filled(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

pub fn search(h: &impl Http, q: &SearchQuery) -> R<String> {
    book(&q.kind)?;
    let title = filled(&q.title);
    let free = Some(q.query.trim()).filter(|s| !s.is_empty() && title.is_none());
    let lang = filled(&q.language)
        .map(|l| normalize_language_code(Some(l)))
        .filter(|l| l != "und");
    let search = Search {
        title,
        author: filled(&q.author),
        isbn: filled(&q.isbn),
        q: free,
        lang: lang.as_deref(),
        limit: Some(SEARCH_LIMIT),
        page: Some(q.page.max(1)),
    };
    let Some(url) = ol::search_url(&search) else {
        return Ok("[]".into());
    };
    let page = ol::parse_search(&get(h, &url)?).map_err(parse_err)?;
    Ok(records(&page.records))
}

pub fn fetch_work(h: &impl Http, kind: &str, id: &ExternalId) -> R<String> {
    book(kind)?;
    let url = ol::work_url(olid(id)?).ok_or(ProviderError::NotFound)?;
    let mut r = load(h, url, ol::work_url, ol::parse_work)?;
    name_authors(h, &mut r);
    Ok(to_record(&r).to_string())
}

/// The first page of editions only.
pub fn fetch_children(h: &impl Http, kind: &str, id: &ExternalId) -> R<String> {
    book(kind)?;
    let url = ol::work_editions_url(olid(id)?, EDITIONS_LIMIT, 0).ok_or(ProviderError::NotFound)?;
    let page = ol::parse_editions(&get(h, &url)?).map_err(parse_err)?;
    Ok(records(&page.records))
}

pub fn find_by(h: &impl Http, source: &ExternalId) -> R<Option<(ExternalId, String)>> {
    let url = (source.provider == "isbn")
        .then(|| ol::isbn_url(&source.value))
        .flatten();
    let Some(url) = url else { return Ok(None) };
    let edition = optional(load(h, url, ol::edition_url, ol::parse_edition))?;
    Ok(edition.and_then(|e| e.ids.work).map(|work| {
        let id = ExternalId {
            provider: NS.into(),
            value: work,
        };
        (id, KIND.into())
    }))
}

pub fn fetch_edition(h: &impl Http, edition: &ExternalId) -> R<Option<String>> {
    let url = match edition.provider.as_str() {
        "isbn" => ol::isbn_url(&edition.value),
        NS => ol::edition_url(&edition.value),
        _ => None,
    };
    let Some(url) = url else { return Ok(None) };
    let Some(mut r) = optional(load(h, url, ol::edition_url, ol::parse_edition))? else {
        return Ok(None);
    };
    name_authors(h, &mut r);
    Ok(Some(to_record(&r).to_string()))
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

    const B: &str = "https://openlibrary.org";

    fn ok(body: &str) -> Reply {
        status(200, body)
    }

    fn status(status: u16, body: &str) -> Reply {
        Reply {
            status,
            retry_after: None,
            location: None,
            body: body.into(),
        }
    }

    /// Answers by exact URL and records what was asked.
    struct Fake {
        routes: Vec<(String, Reply)>,
        seen: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(routes: Vec<(&str, Reply)>) -> Fake {
            let routes = routes.into_iter().map(|(u, r)| (format!("{B}{u}"), r));
            Fake {
                routes: routes.collect(),
                seen: RefCell::default(),
            }
        }
    }

    impl Http for Fake {
        fn get(&self, url: &str) -> Result<Reply, String> {
            self.seen.borrow_mut().push(url.to_owned());
            let hit = self.routes.iter().find(|(u, _)| u == url);
            hit.map(|(_, r)| r.clone()).ok_or_else(|| "no route".into())
        }
    }

    fn id(provider: &str, value: &str) -> ExternalId {
        ExternalId {
            provider: provider.into(),
            value: value.into(),
        }
    }

    fn query(title: &str) -> SearchQuery {
        SearchQuery {
            kind: "book".into(),
            query: title.into(),
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
    fn search_maps_works_to_records() {
        let mut q = query("ignored");
        q.title = Some("Placeholder Title".into());
        q.language = Some("fr-FR".into());
        q.page = 2;
        let url = ol::search_url(&Search {
            title: Some("Placeholder Title"),
            lang: Some("fr"),
            limit: Some(SEARCH_LIMIT),
            page: Some(2),
            ..Search::default()
        })
        .unwrap();
        let h = Fake::new(vec![(&url[B.len()..], ok(fixture!("search-basic.json")))]);
        let v = json_of(search(&h, &q).unwrap());
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[0]["externalId"], "OL9200001W");
        assert_eq!(
            v[0]["authors"][0],
            json!({ "id": "OL9100001A", "name": "Maëlis Quenotte" })
        );
    }

    #[test]
    fn an_empty_search_asks_nothing() {
        let h = Fake::new(vec![]);
        assert_eq!(search(&h, &query("  ")).unwrap(), "[]");
        assert!(h.seen.borrow().is_empty());
    }

    #[test]
    fn a_work_gets_its_author_names() {
        let h = Fake::new(vec![
            ("/works/OL9200001W.json", ok(fixture!("work-full.json"))),
            ("/authors/OL9100001A.json", ok(fixture!("author-full.json"))),
        ]);
        let v = json_of(fetch_work(&h, "book", &id("openlibrary", "OL9200001W")).unwrap());
        assert_eq!(v["authors"][0]["name"], "Maëlis Quenotte");
        assert_eq!(
            v["overview"]
                .as_str()
                .map(|s| s.starts_with("Une cartographe")),
            Some(true)
        );
    }

    #[test]
    fn a_failed_author_lookup_keeps_the_work() {
        let h = Fake::new(vec![(
            "/works/OL9200001W.json",
            ok(fixture!("work-full.json")),
        )]);
        let v = json_of(fetch_work(&h, "book", &id("openlibrary", "OL9200001W")).unwrap());
        assert_eq!(v["authors"][0], json!({ "id": "OL9100001A", "name": null }));
    }

    #[test]
    fn a_merged_work_is_followed_once() {
        let h = Fake::new(vec![
            ("/works/OL9200009W.json", ok(fixture!("work-redirect.json"))),
            ("/works/OL9200001W.json", ok(fixture!("work-full.json"))),
        ]);
        let v = json_of(fetch_work(&h, "book", &id("openlibrary", "OL9200009W")).unwrap());
        assert_eq!(v["externalId"], "OL9200001W");
        assert!(h.seen.borrow()[1].ends_with("/works/OL9200001W.json"));
    }

    #[test]
    fn children_are_the_edition_records() {
        let url = ol::work_editions_url("OL9200001W", EDITIONS_LIMIT, 0).unwrap();
        let h = Fake::new(vec![(&url[B.len()..], ok(fixture!("editions-list.json")))]);
        let v = json_of(fetch_children(&h, "book", &id("openlibrary", "OL9200001W")).unwrap());
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[1]["externalId"], "OL9300011M");
    }

    #[test]
    fn an_isbn_redirect_leads_to_the_work() {
        let mut found = status(302, "");
        found.location = Some("/books/OL9300001M.json".into());
        let h = Fake::new(vec![
            ("/isbn/9782999000018.json", found),
            ("/books/OL9300001M.json", ok(fixture!("edition-full.json"))),
        ]);
        let (w, kind) = find_by(&h, &id("isbn", "978-2-99900-001-8"))
            .unwrap()
            .unwrap();
        assert_eq!(
            (w.provider.as_str(), w.value.as_str(), kind.as_str()),
            ("openlibrary", "OL9200001W", "book")
        );
    }

    #[test]
    fn a_redirect_off_site_is_refused() {
        let mut away = status(302, "");
        away.location = Some("https://example.org/x".into());
        let h = Fake::new(vec![("/isbn/9782999000018.json", away)]);
        let e = fetch_edition(&h, &id("isbn", "9782999000018")).unwrap_err();
        assert!(matches!(e, ProviderError::Upstream(u) if u.status.is_none()));
    }

    #[test]
    fn an_unknown_edition_is_none() {
        let h = Fake::new(vec![(
            "/books/OL9300099M.json",
            status(404, fixture!("notfound.json")),
        )]);
        assert!(
            fetch_edition(&h, &id("openlibrary", "OL9300099M"))
                .unwrap()
                .is_none()
        );
        assert!(
            fetch_edition(&h, &id("asin", "B0QZ7LM4NV"))
                .unwrap()
                .is_none()
        );
        assert!(find_by(&h, &id("asin", "B0QZ7LM4NV")).unwrap().is_none());
    }

    #[test]
    fn upstream_failures_map_to_the_provider_errors() {
        let mut limited = status(429, "");
        limited.retry_after = Some(7);
        let h = Fake::new(vec![
            ("/works/OL1W.json", limited),
            ("/works/OL2W.json", status(503, "maintenance")),
            ("/works/OL3W.json", status(404, fixture!("notfound.json"))),
            ("/works/OL4W.json", ok("not json")),
        ]);
        let work = |v: &str| fetch_work(&h, "book", &id("openlibrary", v)).unwrap_err();
        assert!(matches!(work("OL1W"), ProviderError::RateLimited(Some(7))));
        assert!(matches!(work("OL2W"), ProviderError::Upstream(u) if u.status == Some(503)));
        assert!(matches!(work("OL3W"), ProviderError::NotFound));
        assert!(matches!(work("OL4W"), ProviderError::Upstream(u) if u.status.is_none()));
        assert!(matches!(work("OL5W"), ProviderError::Upstream(u) if u.status.is_none()));
    }

    #[test]
    fn other_kinds_and_namespaces_are_unsupported() {
        let h = Fake::new(vec![]);
        let e = fetch_work(&h, "movie", &id("openlibrary", "OL1W")).unwrap_err();
        assert!(matches!(e, ProviderError::Unsupported));
        let e = fetch_work(&h, "book", &id("tmdb", "1")).unwrap_err();
        assert!(matches!(e, ProviderError::Unsupported));
    }
}
