//! `fliks.thexem`: the scene to aired numbering of a series, from TheXEM. It serves no work, only the
//! two scene calls the core reads to translate release and file names (`scene-numbering.md`).

mod api;
#[cfg(test)]
mod tests;

use api::{Http, R, reply};
use fliks_plugin_sdk::bindings::wasi::http::types::Method;
use fliks_plugin_sdk::http::Request;
use fliks_plugin_sdk::rt::Budget;
use fliks_plugin_sdk::{ExternalId, Plugin, host};
use fliks_plugin_sdk_provider::{MetadataProvider, ProviderError, ScenePair, export_provider};
use provider_common::http::upstream;
use serde_json::Value;
use std::time::Duration;
use url::Url;

const DEFAULT_URL: &str = "https://thexem.info";
const USER_AGENT: &str = concat!("Fliks-TheXEM/", env!("CARGO_PKG_VERSION"));

struct Live;

/// The plugin's own `baseUrl` setting, else TheXEM.
fn base_url() -> R<String> {
    let keys = ["baseUrl".to_owned()];
    let set = host::config_get(Some(&keys))
        .map_err(|e| upstream(None, format!("{}: {}", e.code, e.message)))?
        .into_iter()
        .find(|(k, _)| k == "baseUrl")
        .map(|(_, v)| v.trim().trim_end_matches('/').to_owned())
        .filter(|v| !v.is_empty());
    Ok(set.unwrap_or_else(|| DEFAULT_URL.to_owned()))
}

impl Http for Live {
    fn get(&self, path: &str, query: &[(&str, String)]) -> R<Option<Value>> {
        let params = query.iter().map(|(k, v)| (*k, v.as_str()));
        let url = Url::parse_with_params(&format!("{}{path}", base_url()?), params)
            .map_err(|_| upstream(None, "not a valid URL"))?;
        let resp = Request::new(Method::Get, url)
            .header("user-agent", USER_AGENT)
            .header("accept", "application/json")
            .timeout(Duration::from_secs(20))
            .budget(&Budget::host())
            .send()
            .map_err(|e| upstream(None, e.to_string()))?;
        reply(resp.status, &resp.body)
    }
}

struct Provider;

impl Plugin for Provider {}

impl MetadataProvider for Provider {
    fn scene_mapped_ids(origin: String) -> R<Vec<String>> {
        api::mapped_ids(&Live, &origin)
    }

    fn scene_mapping(id: ExternalId) -> Result<Option<Vec<ScenePair>>, ProviderError> {
        api::mapping(&Live, &id.provider, &id.value)
    }
}

export_provider!(Provider);
