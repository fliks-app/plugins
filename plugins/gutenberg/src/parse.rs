//! Gutendex records and the record map the host reads for a book.
use provider_common::lang::normalize_language_code;
use serde_json::{Value, json};

pub const NS: &str = "fliks.gutenberg:id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Book {
    pub id: u32,
    pub title: String,
    pub authors: Vec<String>,
    /// ISO 639-1 where one exists, in the order Gutendex lists them.
    pub languages: Vec<String>,
    pub subjects: Vec<String>,
    pub has_epub: bool,
}

/// `Surname, Given` as `Given Surname`; a name with no comma (`Various`) stays as it is.
pub fn display_name(name: &str) -> String {
    match name.split_once(',') {
        Some((family, given)) if !given.trim().is_empty() => {
            format!("{} {}", given.trim(), family.trim())
        }
        _ => name.trim().to_owned(),
    }
}

fn strings(v: &Value) -> impl Iterator<Item = &str> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str)
}

pub fn parse_book(v: &Value) -> Option<Book> {
    let title = v["title"].as_str()?.trim();
    Some(Book {
        id: u32::try_from(v["id"].as_u64()?).ok()?,
        title: title.to_owned(),
        authors: v["authors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["name"].as_str())
            .map(display_name)
            .collect(),
        languages: strings(&v["languages"])
            .map(|l| normalize_language_code(Some(l)))
            .filter(|l| l != "und")
            .collect(),
        subjects: strings(&v["subjects"]).map(str::to_owned).collect(),
        has_epub: v["formats"]["application/epub+zip"].is_string(),
    })
}

/// The books of a search page that have an EPUB; `count` is Gutendex's total of the whole search.
pub fn parse_search(body: &[u8]) -> Result<(u64, Vec<Book>), String> {
    let v: Value = serde_json::from_slice(body).map_err(|_| "invalid JSON response")?;
    let results = v["results"].as_array().ok_or("no results list")?;
    let books = results
        .iter()
        .filter_map(parse_book)
        .filter(|b| b.has_epub)
        .collect();
    Ok((v["count"].as_u64().unwrap_or(0), books))
}

impl Book {
    pub fn record(&self) -> Value {
        json!({
            "title": self.title,
            "authors": self.authors.iter().map(|n| json!({ "name": n })).collect::<Vec<_>>(),
            "language": self.languages.first(),
            "subjects": self.subjects,
            "format": "ebook",
            "externalId": self.id.to_string(),
            "ids": { NS: self.id.to_string() },
        })
    }

    /// A row of the search table, whose `id` is what the add action posts.
    pub fn row(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "title": self.title,
            "authors": self.authors.join(", "),
            "language": self.languages.join(", "),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = include_str!("../tests/fixtures/search.json");
    const BOOK: &str = include_str!("../tests/fixtures/book.json");

    #[test]
    fn names_read_given_name_first() {
        assert_eq!(display_name("Doe, Jane"), "Jane Doe");
        assert_eq!(display_name("Various"), "Various");
        assert_eq!(display_name("Doe,"), "Doe,");
    }

    #[test]
    fn a_search_keeps_the_books_with_an_epub() {
        let (count, books) = parse_search(SEARCH.as_bytes()).unwrap();
        assert_eq!(count, 3);
        let ids: Vec<u32> = books.iter().map(|b| b.id).collect();
        assert_eq!(ids, [1001, 1002]);
        assert_eq!(books[1].authors, ["Richard Q. Roe", "Various"]);
        assert_eq!(books[1].languages, ["fr", "en"]);
    }

    #[test]
    fn a_record_carries_the_namespace_id() {
        let v: Value = serde_json::from_str(BOOK).unwrap();
        let r = parse_book(&v).unwrap().record();
        assert_eq!(r["externalId"], "1001");
        assert_eq!(r["ids"][NS], "1001");
        assert_eq!(r["authors"][0]["name"], "Jane Doe");
        assert_eq!(r["language"], "en");
    }

    #[test]
    fn malformed_bodies_are_refused() {
        assert!(parse_search(b"nope").is_err());
        assert!(parse_search(br#"{"detail":"x"}"#).is_err());
        assert!(parse_book(&json!({ "id": 1 })).is_none());
    }
}
