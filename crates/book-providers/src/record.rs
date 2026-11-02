//! A parsed book as the record map the core reads (`book-metadata.md` rules 4 and 11).
use super::{BookRecord, Level, Person};
use serde_json::{Value, json};

fn people(list: &[Person]) -> Vec<Value> {
    list.iter()
        .map(|p| json!({ "name": p.name, "id": p.id }))
        .collect()
}

/// A field the provider lacks is `null` or `[]`; `externalId` is the record's own OLID or id.
pub fn to_record(r: &BookRecord) -> Value {
    let year = r
        .published
        .as_deref()
        .and_then(|p| p.get(..4)?.parse::<u32>().ok());
    let external_id = match r.level {
        Level::Work => &r.ids.work,
        Level::Edition => &r.ids.edition,
    };
    json!({
        "title": r.title,
        "subtitle": r.subtitle,
        "overview": r.description,
        "year": year,
        "releaseDate": r.published,
        "posterUrl": r.covers.first(),
        "rating": r.rating.as_ref().map(|g| g.average),
        "authors": people(&r.authors),
        "narrators": people(&r.narrators),
        "translators": people(&r.translators),
        "series": r.series.iter()
            .map(|s| json!({ "name": s.name, "position": s.position, "id": s.id }))
            .collect::<Vec<_>>(),
        "language": r.language,
        "publisher": r.publisher,
        "pageCount": r.page_count,
        "durationSeconds": r.duration_secs,
        "format": r.format,
        "subjects": r.subjects,
        "externalId": external_id,
        "isbn13": r.ids.isbn13,
        "editionIsbn13": r.ids.edition_isbn13,
        "asin": r.ids.asin,
    })
}
