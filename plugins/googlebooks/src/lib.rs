//! `fliks.googlebooks`: book metadata from Google Books (search, editions).

mod api;
#[cfg(test)]
mod golden;
mod parse;

use api::{Http, Reply};
use fliks_plugin_sdk::bindings::wasi::http::types::Method;
use fliks_plugin_sdk::http::Request;
use fliks_plugin_sdk::rt::Budget;
use fliks_plugin_sdk::{ExternalId, Plugin};
use fliks_plugin_sdk_provider::{
    Lang, MetadataProvider, ProviderError, SearchQuery, export_provider,
};
use std::time::Duration;

const USER_AGENT: &str = concat!("Fliks-GoogleBooks/", env!("CARGO_PKG_VERSION"));

struct Live;

impl Http for Live {
    fn get(&self, url: &str) -> Result<Reply, String> {
        let r = Request::new(Method::Get, url)
            .header("user-agent", USER_AGENT)
            .header("accept", "application/json")
            .timeout(Duration::from_secs(10))
            .body_cap(4 << 20)
            .budget(&Budget::host())
            .send()
            .map_err(|e| e.to_string())?;
        Ok(Reply {
            status: r.status,
            retry_after: r.header("retry-after").and_then(|v| v.trim().parse().ok()),
            body: String::from_utf8_lossy(&r.body).into_owned(),
        })
    }
}

struct GoogleBooks;

impl Plugin for GoogleBooks {}

impl MetadataProvider for GoogleBooks {
    fn search(q: SearchQuery, _lang: Lang) -> Result<String, ProviderError> {
        api::search(&Live, &q)
    }
    fn fetch_edition(edition: ExternalId, _lang: Lang) -> Result<Option<String>, ProviderError> {
        api::fetch_edition(&Live, &edition)
    }
}

export_provider!(GoogleBooks);
