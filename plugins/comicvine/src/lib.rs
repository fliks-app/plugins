//! `fliks.comicvine`: the ComicVine comic metadata provider (volumes and their issues).
//! ComicVine data is for non-commercial use and must link back to ComicVine.

mod api;
#[cfg(test)]
mod tests;

use api::{Http, R, reply};
use fliks_plugin_sdk::bindings::wasi::http::types::Method;
use fliks_plugin_sdk::http::Request;
use fliks_plugin_sdk::rt::Budget;
use fliks_plugin_sdk::{ExternalId, Plugin, host};
use fliks_plugin_sdk_provider::{
    Lang, MetadataProvider, ProviderError, SearchQuery, export_provider,
};
use provider_common::http::upstream;
use serde_json::Value;
use std::time::Duration;
use url::Url;

const API_URL: &str = "https://comicvine.gamespot.com/api";
const USER_AGENT: &str = concat!("Fliks-ComicVine/", env!("CARGO_PKG_VERSION"));

struct Live;

/// The admin's own key; none leaves the provider unauthorized.
fn api_key() -> R<String> {
    let keys = ["apiKey".to_owned()];
    host::config_get(Some(&keys))
        .map_err(|e| upstream(None, format!("{}: {}", e.code, e.message)))?
        .into_iter()
        .find(|(k, _)| k == "apiKey")
        .map(|(_, v)| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .ok_or(ProviderError::Unauthorized)
}

impl Http for Live {
    fn get(&self, path: &str, query: &[(&str, String)]) -> R<Value> {
        let key = api_key()?;
        let params = query.iter().map(|(k, v)| (*k, v.as_str()));
        let url = Url::parse_with_params(
            &format!("{API_URL}{path}"),
            params.chain([("format", "json"), ("api_key", key.as_str())]),
        )
        .map_err(|_| upstream(None, "not a valid URL"))?;
        let resp = Request::new(Method::Get, url)
            .header("user-agent", USER_AGENT)
            .header("accept", "application/json")
            .timeout(Duration::from_secs(15))
            .budget(&Budget::host())
            .send()
            .map_err(|e| upstream(None, e.to_string()))?;
        reply(resp.status, &resp.body)
    }
}

struct Provider;

impl Plugin for Provider {}

fn json_of(v: impl Into<Value>) -> R<String> {
    Ok(v.into().to_string())
}

impl MetadataProvider for Provider {
    fn search(q: SearchQuery, _lang: Lang) -> R<String> {
        json_of(api::search(&Live, &q.query, q.year, Some(q.page))?)
    }

    fn fetch_work(_kind: String, id: ExternalId, _lang: Lang) -> R<String> {
        json_of(api::work(&Live, &id.value)?)
    }

    fn fetch_children(_kind: String, id: ExternalId, _lang: Lang) -> R<String> {
        json_of(api::children(&Live, &id.value)?)
    }
}

export_provider!(Provider);
