//! `fliks.lrclib`: synced and plain lyrics from LRCLIB.

mod api;
#[cfg(test)]
mod tests;

use api::{R, params, reply};
use fliks_plugin_sdk::Plugin;
use fliks_plugin_sdk::bindings::wasi::http::types::Method;
use fliks_plugin_sdk::http::Request;
use fliks_plugin_sdk::rt::Budget;
use fliks_plugin_sdk_provider::{LyricsHit, LyricsQuery, MetadataProvider, export_provider};
use provider_common::http::upstream;
use std::time::Duration;
use url::Url;

const GET_URL: &str = "https://lrclib.net/api/get";
// Registered client-side application id: public by design, it only raises the rate limits.
const APP_ID: &str = "VSjRGNC7rL5vn87f2rXLW98uy1xR0OJmimFX-RWXVXk";
const USER_AGENT: &str = concat!(
    "Fliks/",
    env!("CARGO_PKG_VERSION"),
    " (https://fliks.media)"
);

struct Provider;

impl Plugin for Provider {}

impl MetadataProvider for Provider {
    fn fetch_lyrics(q: LyricsQuery) -> R<Option<LyricsHit>> {
        let url = Url::parse_with_params(GET_URL, params(&q))
            .map_err(|_| upstream(None, "not a valid URL"))?;
        let resp = Request::new(Method::Get, url)
            .header("user-agent", USER_AGENT)
            .header("lrclib-app-id", APP_ID)
            .header("accept", "application/json")
            .timeout(Duration::from_secs(15))
            .budget(&Budget::host())
            .send()
            .map_err(|e| upstream(None, e.to_string()))?;
        reply(
            resp.status,
            resp.header("retry-after").as_deref(),
            &resp.body,
        )
    }
}

export_provider!(Provider);
