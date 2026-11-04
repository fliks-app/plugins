//! `fliks.metron`: the Metron comic metadata provider (series and their issues).

mod api;
#[cfg(test)]
mod tests;

use api::{Http, R, reply};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use fliks_plugin_sdk::bindings::wasi::http::types::Method;
use fliks_plugin_sdk::http::Request;
use fliks_plugin_sdk::rt::Budget;
use fliks_plugin_sdk::{ExternalId, Plugin, host};
use fliks_plugin_sdk_provider::{
    Lang, MetadataProvider, ProviderError, SearchQuery, export_provider,
};
use provider_common::http::upstream;
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

const API_URL: &str = "https://metron.cloud/api";
const USER_AGENT: &str = concat!("Fliks-Metron/", env!("CARGO_PKG_VERSION"));

struct Live;

/// The admin's own account; no account leaves the provider unauthorized.
fn credentials() -> R<String> {
    let keys = ["username".to_owned(), "password".to_owned()];
    let all = host::config_get(Some(&keys))
        .map_err(|e| upstream(None, format!("{}: {}", e.code, e.message)))?;
    let get = |k: &str| {
        all.iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let (user, pass) = get("username")
        .zip(get("password"))
        .ok_or(ProviderError::Unauthorized)?;
    Ok(STANDARD.encode(format!("{user}:{pass}")))
}

impl Http for Live {
    fn get(&self, path: &str, query: &[(&str, String)]) -> R<Value> {
        let url = Url::parse_with_params(&format!("{API_URL}{path}"), query)
            .map_err(|_| upstream(None, "not a valid URL"))?;
        let resp = Request::new(Method::Get, url)
            .header("authorization", &format!("Basic {}", credentials()?))
            .header("user-agent", USER_AGENT)
            .header("accept", "application/json")
            .timeout(Duration::from_secs(15))
            .budget(&Budget::host())
            .send()
            .map_err(|e| upstream(None, e.to_string()))?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let out = reply(
            resp.status,
            resp.header("retry-after").as_deref(),
            resp.header("x-ratelimit-reset").as_deref(),
            now,
            &resp.body,
        );
        // A wait of an hour or more is the day's quota, not the minute's.
        if let Err(ProviderError::RateLimited(Some(s))) = &out
            && *s >= 3600
        {
            let _ = host::provider_quota_spent();
        }
        out
    }
}

struct Provider;

impl Plugin for Provider {}

fn json_of(v: impl Into<Value>) -> R<String> {
    Ok(v.into().to_string())
}

impl MetadataProvider for Provider {
    fn search(q: SearchQuery, _lang: Lang) -> R<String> {
        let page = Some(q.page);
        json_of(api::search(&Live, &q.query, q.year, page)?)
    }

    fn fetch_work(_kind: String, id: ExternalId, _lang: Lang) -> R<String> {
        json_of(api::work(&Live, &id.value)?)
    }

    fn fetch_children(_kind: String, id: ExternalId, _lang: Lang) -> R<String> {
        json_of(api::children(&Live, &id.value)?)
    }
}

export_provider!(Provider);
