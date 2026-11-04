//! The comic records both comic providers return: a volume and its numbered issues.
use regex::Regex;
use serde_json::{Value, json};
use std::cmp::Ordering;
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

/// The label as printed, the decimal sort key and the kind; no number leaves the sort `None`.
fn number(raw: &str, annual: bool) -> (String, Option<f64>, &'static str) {
    static N: OnceLock<Regex> = OnceLock::new();
    let n = N.get_or_init(|| Regex::new(r"\d+(?:\.\d+)?").unwrap());
    let label = raw.trim().to_owned();
    let sort = if label == "½" {
        Some(0.5)
    } else {
        n.find(&label).and_then(|m| m.as_str().parse::<f64>().ok())
    };
    let kind = match sort {
        _ if annual || label.to_lowercase().contains("annual") => "annual",
        Some(_) => "issue",
        None => "special",
    };
    (label, sort, kind)
}

/// Issues ordered as the host lists them (issues, then annuals and specials); a repeated
/// (kind, sort) is a variant of one issue and is dropped.
pub fn issues(raw: Vec<RawIssue>) -> Vec<Value> {
    let mut specials = 0.0;
    let mut out: Vec<(&'static str, f64, Value)> = vec![];
    for r in raw {
        let (mut label, sort, kind) = number(&r.number, r.annual);
        let sort = sort.unwrap_or_else(|| {
            specials += 1.0;
            specials
        });
        if label.is_empty() {
            label = sort.to_string();
        }
        if out.iter().any(|(k, s, _)| *k == kind && *s == sort) {
            continue;
        }
        let release = r.store_date.clone().or_else(|| r.cover_date.clone());
        let v = json!({
            "externalId": r.id,
            "number": label,
            "sortNumber": sort,
            "kind": kind,
            "title": r.title,
            "releaseDate": release,
            "coverDate": r.cover_date,
            "storeDate": r.store_date,
            "posterUrl": r.cover,
        });
        out.push((kind, sort, v));
    }
    let rank = |k: &str| u8::from(k != "issue");
    out.sort_by(|a, b| {
        rank(a.0)
            .cmp(&rank(b.0))
            .then(a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
            .then(a.0.cmp(b.0))
    });
    out.into_iter().map(|(_, _, v)| v).collect()
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

    fn raw(n: &str, annual: bool) -> RawIssue {
        RawIssue {
            id: None,
            number: n.into(),
            annual,
            title: None,
            cover_date: Some("2016-06-01".into()),
            store_date: None,
            cover: None,
        }
    }

    #[test]
    fn numbers_decimal_annual_special_and_variants() {
        let out = issues(vec![
            raw("Annual 1", false),
            raw("2", false),
            raw("1.5", false),
            raw("Special", false),
            raw("2", false),
            raw("1", true),
            raw("½", false),
        ]);
        let got: Vec<(String, f64, String)> = out
            .iter()
            .map(|v| {
                (
                    v["number"].as_str().unwrap().into(),
                    v["sortNumber"].as_f64().unwrap(),
                    v["kind"].as_str().unwrap().into(),
                )
            })
            .collect();
        let want = [
            ("½", 0.5, "issue"),
            ("1.5", 1.5, "issue"),
            ("2", 2.0, "issue"),
            ("Annual 1", 1.0, "annual"),
            ("Special", 1.0, "special"),
        ];
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(want) {
            assert_eq!((g.0.as_str(), g.1, g.2.as_str()), w);
        }
    }

    #[test]
    fn dates_must_be_real() {
        assert_eq!(date(Some("2016-06-01")).as_deref(), Some("2016-06-01"));
        assert_eq!(date(Some("2016-00-00")), None);
    }
}
