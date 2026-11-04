//! The Metron lookups over an injected transport, so recorded bodies can stand in for the network.
use fliks_plugin_sdk_provider::ProviderError;
use provider_common::comic::{RawIssue, Volume, date, https, issues, volume};
use provider_common::http::{json_reply, upstream};
use serde_json::Value;

pub type R<T> = Result<T, ProviderError>;

pub const NS: &str = "fliks.metron:metron";
/// 30 issues a page: a long run is cut here instead of spending the day's quota on one series.
const MAX_PAGES: u32 = 40;

pub trait Http {
    fn get(&self, path: &str, query: &[(&str, String)]) -> R<Value>;
}

/// Seconds to wait: `Retry-After`, else `X-RateLimit-Reset` as seconds or as a Unix time.
pub fn retry_secs(retry_after: Option<&str>, reset: Option<&str>, now: u64) -> Option<u32> {
    let secs = |v: &str| v.trim().parse::<u64>().ok();
    let wait = retry_after.and_then(secs).or_else(|| {
        let r = secs(reset?)?;
        Some(if r > 1_000_000_000 {
            r.saturating_sub(now)
        } else {
            r
        })
    })?;
    u32::try_from(wait).ok()
}

/// A Metron answer as the provider reports it.
pub fn reply(
    status: u16,
    retry_after: Option<&str>,
    reset: Option<&str>,
    now: u64,
    body: &[u8],
) -> R<Value> {
    let retry = retry_secs(retry_after, reset, now).map(|s| s.to_string());
    json_reply(status, retry.as_deref(), body)
}

fn s(v: &Value, k: &str) -> Option<String> {
    let t = v.get(k)?.as_str()?.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

fn n(v: &Value, k: &str) -> Option<u32> {
    u32::try_from(v.get(k)?.as_u64()?).ok()
}

fn id_of(v: &Value) -> Option<String> {
    Some(v.get("id")?.as_u64()?.to_string())
}

fn series_id(id: &str) -> R<&str> {
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(id)
    } else {
        Err(ProviderError::NotFound)
    }
}

fn list(v: &Value) -> &[Value] {
    v.get("results")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn base(v: &Value, id: String) -> Volume {
    Volume {
        ns: NS,
        id,
        title: s(v, "name").or_else(|| s(v, "series")).unwrap_or_default(),
        overview: s(v, "desc"),
        year: n(v, "year_began"),
        cover: None,
        publisher: v
            .pointer("/publisher/name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        volume: n(v, "volume"),
        issue_count: n(v, "issue_count"),
        url: None,
    }
}

pub fn search(h: &impl Http, query: &str, year: Option<i32>, page: Option<u32>) -> R<Vec<Value>> {
    let mut q = vec![("name", query.to_owned())];
    if let Some(y) = year {
        q.push(("year_began", y.to_string()));
    }
    if let Some(p) = page.filter(|p| *p > 1) {
        q.push(("page", p.to_string()));
    }
    let body = h.get("/series/", &q)?;
    Ok(list(&body)
        .iter()
        .filter_map(|r| Some(volume(&base(r, id_of(r)?))))
        .collect())
}

fn cover_of(issue: &Value) -> Option<String> {
    https(issue.get("image")?.as_str()?)
}

pub fn work(h: &impl Http, id: &str) -> R<Value> {
    let id = series_id(id)?;
    let d = h.get(&format!("/series/{id}/"), &[])?;
    let first = h.get("/issue/", &[("series_id", id.to_owned())])?;
    let mut v = base(&d, id_of(&d).unwrap_or_else(|| id.to_owned()));
    v.cover = list(&first).iter().find_map(cover_of);
    v.url = Some(format!("https://metron.cloud/series/{id}/"));
    Ok(volume(&v))
}

fn is_annual(series: &Value) -> bool {
    series
        .pointer("/series_type/name")
        .and_then(Value::as_str)
        .is_some_and(|t| t.to_lowercase().contains("annual"))
}

pub fn children(h: &impl Http, id: &str) -> R<Vec<Value>> {
    let id = series_id(id)?;
    let annual = is_annual(&h.get(&format!("/series/{id}/"), &[])?);
    let mut raw = vec![];
    for page in 1..=MAX_PAGES {
        let body = h.get(
            "/issue/",
            &[("series_id", id.to_owned()), ("page", page.to_string())],
        )?;
        raw.extend(list(&body).iter().filter_map(|i| {
            Some(RawIssue {
                id: id_of(i),
                number: s(i, "number")?,
                annual,
                title: None,
                cover_date: date(s(i, "cover_date").as_deref()),
                store_date: date(s(i, "store_date").as_deref()),
                cover: cover_of(i),
            })
        }));
        if body.get("next").is_none_or(Value::is_null) {
            return Ok(issues(raw));
        }
    }
    Err(upstream(None, "series has more issues than the page cap"))
}
