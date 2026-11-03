//! LibriVox records and the record map the host reads for a book.
use provider_common::lang::normalize_language_code;
use serde_json::{Value, json};

pub const NS: &str = "fliks.librivox:id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub number: u32,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Book {
    pub id: u32,
    pub title: String,
    pub authors: Vec<String>,
    pub narrators: Vec<String>,
    /// ISO 639-1 where one exists.
    pub language: Option<String>,
    pub year: Option<u32>,
    pub overview: Option<String>,
    pub duration_secs: Option<u32>,
    /// Ordered by number; only the sections that have a file.
    pub sections: Vec<Section>,
    /// Sections the API lists without a number or a file.
    pub gaps: usize,
}

/// LibriVox sends most numbers as strings.
fn num(v: &Value) -> Option<u32> {
    v.as_u64()
        .or_else(|| v.as_str()?.trim().parse().ok())
        .and_then(|n| u32::try_from(n).ok())
}

fn text(v: &Value) -> Option<String> {
    Some(v.as_str()?.trim().to_owned()).filter(|s| !s.is_empty())
}

/// The API names a language in English; a code is read as it is.
pub fn language_code(name: &str) -> Option<String> {
    const NAMES: [(&str, &str); 24] = [
        ("english", "en"),
        ("french", "fr"),
        ("français", "fr"),
        ("german", "de"),
        ("deutsch", "de"),
        ("spanish", "es"),
        ("español", "es"),
        ("italian", "it"),
        ("italiano", "it"),
        ("dutch", "nl"),
        ("nederlands", "nl"),
        ("portuguese", "pt"),
        ("português", "pt"),
        ("russian", "ru"),
        ("polish", "pl"),
        ("latin", "la"),
        ("chinese", "zh"),
        ("japanese", "ja"),
        ("swedish", "sv"),
        ("danish", "da"),
        ("finnish", "fi"),
        ("hebrew", "he"),
        ("greek", "el"),
        ("esperanto", "eo"),
    ];
    let lower = name.trim().to_lowercase();
    let code = NAMES.iter().find(|(n, _)| *n == lower).map_or_else(
        || normalize_language_code(Some(&lower)),
        |(_, c)| (*c).to_owned(),
    );
    (code != "und" && code.len() <= 3).then_some(code)
}

fn person(a: &Value) -> Option<String> {
    let name = [&a["first_name"], &a["last_name"]]
        .into_iter()
        .filter_map(text)
        .collect::<Vec<_>>()
        .join(" ");
    Some(name).filter(|n| !n.is_empty())
}

pub fn parse_book(v: &Value) -> Option<Book> {
    let mut narrators: Vec<String> = vec![];
    let mut sections: Vec<Section> = vec![];
    let mut gaps = 0;
    for s in v["sections"].as_array().into_iter().flatten() {
        for r in s["readers"].as_array().into_iter().flatten() {
            if let Some(n) = text(&r["display_name"]).filter(|n| !narrators.contains(n)) {
                narrators.push(n);
            }
        }
        let (Some(number), Some(url)) = (num(&s["section_number"]), text(&s["listen_url"])) else {
            gaps += 1;
            continue;
        };
        if !sections.iter().any(|x| x.number == number) {
            sections.push(Section { number, url });
        }
    }
    sections.sort_by_key(|s| s.number);
    Some(Book {
        id: num(&v["id"])?,
        title: text(&v["title"])?,
        authors: v["authors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(person)
            .collect(),
        narrators,
        language: text(&v["language"]).and_then(|l| language_code(&l)),
        year: num(&v["copyright_year"]).filter(|y| *y > 0),
        overview: text(&v["description"]),
        duration_secs: num(&v["totaltimesecs"]).filter(|s| *s > 0),
        sections,
        gaps,
    })
}

/// The books of a search answer, `{"books": [...]}`.
pub fn parse_books(body: &[u8]) -> Result<Vec<Book>, String> {
    let v: Value = serde_json::from_slice(body).map_err(|_| "invalid JSON response")?;
    match v["books"].as_array() {
        Some(list) => Ok(list.iter().filter_map(parse_book).collect()),
        None if v["error"].is_string() => Ok(vec![]),
        None => Err("no books list".into()),
    }
}

impl Book {
    pub fn record(&self) -> Value {
        let people = |l: &[String]| l.iter().map(|n| json!({ "name": n })).collect::<Vec<_>>();
        json!({
            "title": self.title,
            "overview": self.overview,
            "year": self.year,
            "authors": people(&self.authors),
            "narrators": people(&self.narrators),
            "language": self.language,
            "durationSeconds": self.duration_secs,
            "format": "audio",
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
            "language": self.language,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = include_str!("../tests/fixtures/search.json");
    const BOOK: &str = include_str!("../tests/fixtures/book.json");
    const NOT_FOUND: &str = include_str!("../tests/fixtures/not-found.json");

    #[test]
    fn a_search_reads_strings_and_numbers() {
        let books = parse_books(SEARCH.as_bytes()).unwrap();
        assert_eq!(books.len(), 2);
        let (a, b) = (&books[0], &books[1]);
        assert_eq!(
            (a.id, a.year, a.duration_secs),
            (2001, Some(1850), Some(5405))
        );
        assert_eq!(a.language.as_deref(), Some("en"));
        assert_eq!(a.authors, ["Jane Doe"]);
        assert_eq!(
            (b.year, b.duration_secs, b.overview.as_deref()),
            (None, Some(600), None)
        );
        assert_eq!(b.language.as_deref(), Some("fr"));
        assert_eq!(b.authors, ["Anonymous", "Richard Roe"]);
    }

    #[test]
    fn a_book_has_ordered_sections_and_its_readers() {
        let books = parse_books(BOOK.as_bytes()).unwrap();
        let b = &books[0];
        let numbers: Vec<u32> = b.sections.iter().map(|s| s.number).collect();
        assert_eq!(numbers, [1, 2, 3]);
        assert!(b.sections[0].url.ends_with("part1.mp3"));
        assert_eq!(b.narrators, ["Reader Two", "Reader One"]);
        let r = b.record();
        assert_eq!(r["ids"][NS], "2001");
        assert_eq!(r["narrators"][1]["name"], "Reader One");
    }

    #[test]
    fn a_section_without_a_file_is_a_gap() {
        let v = json!({ "id": "1", "title": "T", "sections": [
            { "section_number": "1", "listen_url": "https://a.test/1.mp3" },
            { "section_number": "2", "listen_url": "" },
            { "listen_url": "https://a.test/3.mp3" }
        ]});
        let b = parse_book(&v).unwrap();
        assert_eq!((b.sections.len(), b.gaps), (1, 2));
    }

    #[test]
    fn a_search_with_no_hit_is_empty() {
        assert!(parse_books(NOT_FOUND.as_bytes()).unwrap().is_empty());
        assert!(parse_books(b"nope").is_err());
        assert!(parse_books(b"{}").is_err());
    }

    #[test]
    fn languages_are_read_by_name_or_code() {
        assert_eq!(language_code("English").as_deref(), Some("en"));
        assert_eq!(language_code("Español").as_deref(), Some("es"));
        assert_eq!(language_code("fra").as_deref(), Some("fr"));
        assert_eq!(language_code("Klingon"), None);
        assert_eq!(language_code(""), None);
    }
}
