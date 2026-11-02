//! `fliks.audnexus`: audiobook metadata and chapters from Audnexus, by ASIN.

mod api;
#[cfg(test)]
mod golden;
mod parse;

use api::{Http, Reply};
use fliks_plugin_sdk::bindings::wasi::http::types::Method;
use fliks_plugin_sdk::http::Request;
use fliks_plugin_sdk::rt::Budget;
use fliks_plugin_sdk::{ExternalId, Plugin};
use fliks_plugin_sdk_provider::{Lang, MetadataProvider, ProviderError, export_provider};
use provider_common::book::region::Region;
use std::time::Duration;

const USER_AGENT: &str = concat!("Fliks-Audnexus/", env!("CARGO_PKG_VERSION"));

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

fn region(lang: &Lang) -> Region {
    Region::for_lang(&lang.language, lang.region.as_deref())
}

struct Audnexus;

impl Plugin for Audnexus {}

impl MetadataProvider for Audnexus {
    fn fetch_edition(edition: ExternalId, lang: Lang) -> Result<Option<String>, ProviderError> {
        api::fetch_edition(&Live, &edition, region(&lang))
    }
    fn fetch_chapters(edition: ExternalId, lang: Lang) -> Result<Option<String>, ProviderError> {
        api::fetch_chapters(&Live, &edition, region(&lang))
    }
}

export_provider!(Audnexus);
