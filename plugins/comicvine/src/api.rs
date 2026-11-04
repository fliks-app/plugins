//! The ComicVine lookups over an injected transport, so recorded bodies can stand in for the network.
use fliks_plugin_sdk_provider::ProviderError;
use provider_common::comic::{RawIssue, Volume, date, https, issues, volume};
use provider_common::http::{json_reply, upstream};
use serde_json::Value;

pub type R<T> = Result<T, ProviderError>;

pub const NS: &str = "fliks.comicvine:comicvine";
const PAGE: u32 = 100;
/// 3,000 issues: a longer run is cut instead of spending the hour's 200 requests on one volume.
const MAX_PAGES: u32 = 30;
const VOLUME_FIELDS: &str =
    "id,name,start_year,publisher,count_of_issues,description,deck,image,site_detail_url";

pub trait Http {
    fn get(&self, path: &str, query: &[(&str, String)]) -> R<Value>;
}

/// A ComicVine answer as the provider reports it; 420 is its "slow down" and `status_code` 100 a bad key.
pub fn reply(status: u16, body: &[u8]) -> R<Value> {
    if status == 420 {
        return Err(ProviderError::RateLimited(None));
    }
    let v = json_reply(status, None, body)?;
    match v.get("status_code").and_then(Value::as_u64) {
        Some(1) | None => Ok(v),
        Some(100) => Err(ProviderError::Unauthorized),
        Some(101) => Err(ProviderError::NotFound),
        Some(c) => Err(upstream(None, format!("ComicVine status {c}"))),
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    let t = v.get(k)?.as_str()?.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

fn id_of(v: &Value) -> Option<String> {
    Some(v.get("id")?.as_u64()?.to_string())
}

fn volume_id(id: &str) -> R<&str> {
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(id)
    } else {
        Err(ProviderError::NotFound)
    }
}

fn results(v: &Value) -> &[Value] {
    v.get("results")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The largest cover that is not the original scan.
fn cover(v: &Value) -> Option<String> {
    let img = v.get("image")?;
    ["super_url", "medium_url", "small_url", "original_url"]
        .iter()
        .find_map(|k| https(img.get(*k)?.as_str()?))
}

fn year(v: &Value) -> Option<u32> {
    s(v, "start_year")?.parse().ok()
}

fn to_volume(v: &Value, id: String) -> Volume {
    Volume {
        ns: NS,
        id,
        title: s(v, "name").unwrap_or_default(),
        overview: s(v, "description").or_else(|| s(v, "deck")),
        year: year(v),
        cover: cover(v),
        publisher: v
            .pointer("/publisher/name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        volume: None,
        issue_count: v
            .get("count_of_issues")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
        url: s(v, "site_detail_url").and_then(|u| https(&u)),
    }
}

pub fn search(h: &impl Http, query: &str, year_: Option<i32>, page: Option<u32>) -> R<Vec<Value>> {
    let q = [
        ("query", query.to_owned()),
        ("resources", "volume".to_owned()),
        ("field_list", VOLUME_FIELDS.to_owned()),
        ("limit", "20".to_owned()),
        ("page", page.unwrap_or(1).max(1).to_string()),
    ];
    let body = h.get("/search/", &q)?;
    Ok(results(&body)
        .iter()
        .filter(|r| year_.is_none_or(|y| year(r) == u32::try_from(y).ok()))
        .filter_map(|r| Some(volume(&to_volume(r, id_of(r)?))))
        .collect())
}

pub fn work(h: &impl Http, id: &str) -> R<Value> {
    let id = volume_id(id)?;
    let body = h.get(
        &format!("/volume/4050-{id}/"),
        &[("field_list", VOLUME_FIELDS.to_owned())],
    )?;
    let r = body
        .get("results")
        .filter(|r| r.is_object())
        .ok_or(ProviderError::NotFound)?;
    Ok(volume(&to_volume(r, id.to_owned())))
}

pub fn children(h: &impl Http, id: &str) -> R<Vec<Value>> {
    let id = volume_id(id)?;
    let named = h.get(
        &format!("/volume/4050-{id}/"),
        &[("field_list", "name".to_owned())],
    )?;
    let annual = named
        .pointer("/results/name")
        .and_then(Value::as_str)
        .is_some_and(|n| n.to_lowercase().contains("annual"));
    let mut raw = vec![];
    for p in 0..MAX_PAGES {
        let body = h.get(
            "/issues/",
            &[
                ("filter", format!("volume:{id}")),
                (
                    "field_list",
                    "id,issue_number,name,cover_date,store_date,image".to_owned(),
                ),
                ("limit", PAGE.to_string()),
                ("offset", (p * PAGE).to_string()),
            ],
        )?;
        let page = results(&body);
        raw.extend(page.iter().filter_map(|i| {
            Some(RawIssue {
                id: id_of(i),
                number: s(i, "issue_number")?,
                annual,
                title: s(i, "name"),
                cover_date: date(s(i, "cover_date").as_deref()),
                store_date: date(s(i, "store_date").as_deref()),
                cover: cover(i),
            })
        }));
        let total = body["number_of_total_results"].as_u64().unwrap_or(0);
        if u64::from((p + 1) * PAGE) >= total || page.is_empty() {
            return Ok(issues(raw));
        }
    }
    Err(upstream(None, "volume has more issues than the page cap"))
}
