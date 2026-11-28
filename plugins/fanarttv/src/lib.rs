//! `fliks.fanarttv`: artist images and album covers from Fanart.tv, keyed by MusicBrainz ids
//! (`docs/spec/music.md` rules 25 and 26).

mod api;
mod images;
#[cfg(test)]
mod tests;

use api::{Http, R, reply};
use fliks_plugin_sdk::bindings::wasi::http::types::Method;
use fliks_plugin_sdk::http::Request;
use fliks_plugin_sdk::rt::Budget;
use fliks_plugin_sdk::{ExternalId, Plugin, host};
use fliks_plugin_sdk_provider::{
    ImageScope, Lang, MetadataProvider, ProviderError, export_provider, images_json,
};
use provider_common::http::upstream;
use serde_json::Value;
use std::time::Duration;
use url::Url;

const API_URL: &str = "https://webservice.fanart.tv/v3";
const USER_AGENT: &str = concat!(
    "Fliks/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/fliks-app/fliks)"
);

struct Live;

/// The project key (the host's default, or the admin's own) and the optional personal key.
fn keys() -> R<(String, Option<String>)> {
    let want = ["apiKey".to_owned(), "clientKey".to_owned()];
    let all = host::config_get(Some(&want))
        .map_err(|e| upstream(None, format!("{}: {}", e.code, e.message)))?;
    let get = |k: &str| {
        all.iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    Ok((
        get("apiKey").ok_or(ProviderError::Unauthorized)?,
        get("clientKey"),
    ))
}

impl Http for Live {
    fn get(&self, path: &str) -> R<Value> {
        let (key, client) = keys()?;
        let url = Url::parse_with_params(&format!("{API_URL}{path}"), [("api_key", key.as_str())])
            .map_err(|_| upstream(None, "not a valid URL"))?;
        let mut req = Request::new(Method::Get, url)
            .header("user-agent", USER_AGENT)
            .header("accept", "application/json")
            .timeout(Duration::from_secs(15))
            .budget(&Budget::host());
        if let Some(c) = client {
            req = req.header("client-key", &c);
        }
        let resp = req.send().map_err(|e| upstream(None, e.to_string()))?;
        reply(
            resp.status,
            resp.header("retry-after").as_deref(),
            &resp.body,
        )
    }
}

struct Provider;

impl Plugin for Provider {}

impl MetadataProvider for Provider {
    fn fetch_work(_kind: String, id: ExternalId, _lang: Lang) -> R<String> {
        Ok(api::work(&Live, &id.value)?.to_string())
    }

    fn fetch_person(id: ExternalId, _lang: Lang) -> R<String> {
        Ok(api::person(&Live, &id.value)?.to_string())
    }

    fn fetch_images(kind: String, id: ExternalId, scope: ImageScope, _lang: Lang) -> R<String> {
        Ok(images_json(&images::images(
            &Live, &kind, &id.value, &scope,
        )?))
    }

    /// Fanart.tv is keyed by MusicBrainz ids for music, TMDB ids for movies and TVDB ids for shows.
    fn find_by(
        source: ExternalId,
        _lang: Lang,
    ) -> Result<Option<(ExternalId, String)>, ProviderError> {
        let (value, kind) = match source.provider.as_str() {
            "musicbrainz" => (api::uuid(&source.value), "album"),
            "tmdb" => (api::number(&source.value), "movie"),
            "tvdb" => (api::number(&source.value), "series"),
            _ => return Ok(None),
        };
        Ok(value.map(|value| {
            let id = ExternalId {
                provider: "fanarttv".into(),
                value,
            };
            (id, kind.into())
        }))
    }
}

export_provider!(Provider);
