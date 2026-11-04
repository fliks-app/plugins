//! The comic records both comic providers return: a volume and its numbered issues.
use regex::Regex;
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::OnceLock;

/// One issue as a provider describes it, before numbering is resolved.
pub struct RawIssue {
    pub id: Option<String>,
    pub number: String,
    /// The issue belongs to an annual series.
    pub annual: bool,
    pub title: Option<String>,
    pub cover_date: Option<String>,
    pub store_date: Option<String>,
    pub cover: Option<String>,
}

/// `YYYY-MM-DD` when it is a real date.
pub fn date(raw: Option<&str>) -> Option<String> {
    let d = chrono::NaiveDate::parse_from_str(raw?.trim(), "%Y-%m-%d").ok()?;
    Some(d.to_string())
}

pub fn https(url: &str) -> Option<String> {
    url.strip_prefix("http://")
        .map(|r| format!("https://{r}"))
        .or_else(|| url.starts_with("https://").then(|| url.to_owned()))
}

/// The numbering of one issue: the label as printed, the decimal sort key (`None` for a special) and the kind.
fn number(raw: &str, annual: bool) -> (String, Option<f64>, &'static str) {
    static N: OnceLock<Regex> = OnceLock::new();
    static A: OnceLock<Regex> = OnceLock::new();
    let full = N.get_or_init(|| Regex::new(r"^-?\d+(\.\d+)?$").unwrap());
    let word = A.get_or_init(|| Regex::new(r"(?i)\bannual\b").unwrap());
    let raw = raw.trim();
    let annual = annual || word.is_match(raw);
    let rest = word.replace_all(raw, "");
    let rest = rest.trim();
    let sort = match rest {
        "½" | "1/2" => Some(0.5),
        r if full.is_match(r) => r.parse::<f64>().ok(),
        _ => None,
    };
    match sort {
        Some(n) => (
            rest.to_owned(),
            Some(n),
            if annual { "annual" } else { "issue" },
        ),
        None if raw.is_empty() => ("Special".to_owned(), None, "special"),
        None => (raw.to_owned(), None, "special"),
    }
}

/// Issues ordered as the host lists them (issues, then annuals, then specials); an exact repeat of
/// a kind and printed number is dropped. A special's sort key is its rank by upstream id, so it
/// does not move when the list order does.
pub fn issues(raw: Vec<RawIssue>) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut rows: Vec<(&'static str, Option<f64>, String, RawIssue)> = vec![];
    for r in raw {
        let (label, sort, kind) = number(&r.number, r.annual);
        if seen.insert((kind, label.clone())) {
            rows.push((kind, sort, label, r));
        }
    }
    let uid = |r: &RawIssue| {
        r.id.as_deref()
            .and_then(|i| i.parse::<u64>().ok())
            .unwrap_or(u64::MAX)
    };
    let mut specials: Vec<usize> = (0..rows.len()).filter(|i| rows[*i].1.is_none()).collect();
    specials.sort_by_key(|i| uid(&rows[*i].3));
    for (rank, i) in specials.into_iter().enumerate() {
        rows[i].1 = Some((rank + 1) as f64);
    }
    let rank = |k: &str| match k {
        "issue" => 0,
        "annual" => 1,
        _ => 2,
    };
    rows.sort_by(|a, b| {
        rank(a.0)
            .cmp(&rank(b.0))
            .then(a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
    });
    rows.into_iter()
        .map(|(kind, sort, label, r)| {
            json!({
                "externalId": r.id,
                "number": label,
                "sortNumber": sort,
                "kind": kind,
                "title": r.title,
                "releaseDate": r.store_date,
                "coverDate": r.cover_date,
                "storeDate": r.store_date,
                "posterUrl": r.cover,
            })
        })
        .collect()
}

/// A volume (search result or work): `ns` is the plugin's id namespace.
pub struct Volume {
    pub ns: &'static str,
    pub id: String,
    pub title: String,
    pub overview: Option<String>,
    pub year: Option<u32>,
    pub cover: Option<String>,
    pub publisher: Option<String>,
    pub volume: Option<u32>,
    pub issue_count: Option<u32>,
    pub url: Option<String>,
}

pub fn volume(v: &Volume) -> Value {
    json!({
        "ids": { v.ns: v.id },
        "externalId": v.id,
        "mediaType": "comic",
        "title": v.title,
        "overview": v.overview,
        "year": v.year,
        "posterUrl": v.cover,
        "publisher": v.publisher,
        "volume": v.volume,
        "issueCount": v.issue_count,
        "sourceUrl": v.url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(id: u32, n: &str, annual: bool) -> RawIssue {
        RawIssue {
            id: Some(id.to_string()),
            number: n.into(),
            annual,
            title: None,
            cover_date: Some("2016-06-01".into()),
            store_date: None,
            cover: None,
        }
    }

    fn rows(list: Vec<RawIssue>) -> Vec<(String, f64, String)> {
        issues(list)
            .iter()
            .map(|v| {
                (
                    v["number"].as_str().unwrap().into(),
                    v["sortNumber"].as_f64().unwrap(),
                    v["kind"].as_str().unwrap().into(),
                )
            })
            .collect()
    }

    fn row(n: &str, s: f64, k: &str) -> (String, f64, String) {
        (n.into(), s, k.into())
    }

    #[test]
    fn numbers_keep_every_real_issue() {
        let got = rows(vec![
            raw(1, "-1", false),
            raw(2, "1/2", false),
            raw(3, "1.10", false),
            raw(4, "1.1", false),
            raw(5, "Annual 1", false),
            raw(6, "2", true),
            raw(7, "1.MU", false),
            raw(8, "", false),
            raw(9, "Annual", false),
            raw(10, "½", false),
            raw(11, "2", true),
        ]);
        let want = vec![
            row("-1", -1.0, "issue"),
            row("1/2", 0.5, "issue"),
            row("½", 0.5, "issue"),
            row("1.10", 1.1, "issue"),
            row("1.1", 1.1, "issue"),
            row("1", 1.0, "annual"),
            row("2", 2.0, "annual"),
            row("1.MU", 1.0, "special"),
            row("Special", 2.0, "special"),
            row("Annual", 3.0, "special"),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn a_special_sorts_by_upstream_id_not_list_order() {
        let a = rows(vec![raw(9, "1AU", false), raw(3, "1.MU", false)]);
        let b = rows(vec![raw(3, "1.MU", false), raw(9, "1AU", false)]);
        assert_eq!(a, b);
        assert_eq!(a[0], row("1.MU", 1.0, "special"));
    }

    #[test]
    fn dates_must_be_real() {
        assert_eq!(date(Some("2016-06-01")).as_deref(), Some("2016-06-01"));
        assert_eq!(date(Some("2016-00-00")), None);
    }

    #[test]
    fn release_date_is_the_store_date_only() {
        let mut r = raw(1, "1", false);
        assert_eq!(issues(vec![r])[0]["releaseDate"], Value::Null);
        r = raw(1, "1", false);
        r.store_date = Some("2016-05-11".into());
        assert_eq!(issues(vec![r])[0]["releaseDate"], "2016-05-11");
    }
}
