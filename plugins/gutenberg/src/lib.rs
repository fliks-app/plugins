//! `fliks.gutenberg`: Project Gutenberg books through Gutendex, as a metadata provider and a source
//! a user adds a book from.

mod add;
mod api;
mod parse;
mod routes;

use direct_source::net::Live;
use direct_source::{Fail, host};
use fliks_plugin_sdk::bindings::fliks::plugin::types::Family;
use fliks_plugin_sdk::{ExternalId, HttpRequest, HttpResponse, Plugin};
use fliks_plugin_sdk_provider::{
    Lang, MetadataProvider, ProviderError, SearchQuery, export_provider,
};
use serde::Deserialize;
use std::time::Duration;

/// The prefix of this plugin's scheduled calls.
const SOURCE: &str = "add";
const USER_AGENT: &str = concat!(
    "Fliks-Gutenberg/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/fliks-app/plugins)"
);

/// The catalogue API: no pause of its own, the host paces a provider call.
fn api_client() -> Live {
    Live {
        user_agent: USER_AGENT,
        accept: "application/json",
        gap: Duration::ZERO,
        cap: 4 << 20,
    }
}

/// The book files: one request every two seconds, an EPUB is at most 64 MiB.
fn file_client() -> Live {
    Live {
        user_agent: USER_AGENT,
        accept: "application/epub+zip",
        gap: Duration::from_secs(2),
        cap: 64 << 20,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    id: String,
    media_id: u32,
    language: Option<String>,
}

/// Downloads the EPUB and hands it to the library, which removes it.
fn add_book(p: &Payload) -> Result<(), Fail> {
    let name = add::fetch(&file_client(), host::data_root(), &p.id)?;
    host::ingest(
        p.media_id,
        Family::Text,
        p.language.clone(),
        vec![host::guest_path(&name)],
        format!("gutenberg:{}", p.id),
        "Project Gutenberg",
    )
}

struct Gutenberg;

impl Plugin for Gutenberg {
    fn scheduled(key: String, payload: String, _attempt: u32) -> Result<(), String> {
        let Ok(p) = serde_json::from_str::<Payload>(&payload) else {
            fliks_plugin_sdk::warn!("scheduled call {key} dropped: unreadable payload");
            return Ok(());
        };
        match add_book(&p) {
            Ok(()) => Ok(()),
            Err(Fail::Retry(m)) => Err(m),
            Err(Fail::Drop(m)) => {
                fliks_plugin_sdk::error!("adding book {} failed for good: {m}", p.id);
                Ok(())
            }
        }
    }

    fn http(req: HttpRequest) -> Result<HttpResponse, String> {
        routes::ROUTER.handle(req)
    }
}

impl MetadataProvider for Gutenberg {
    fn search(q: SearchQuery, _lang: Lang) -> Result<String, ProviderError> {
        api::search(&api_client(), &q)
    }
    fn fetch_work(kind: String, id: ExternalId, _lang: Lang) -> Result<String, ProviderError> {
        api::work(&api_client(), &kind, &id)
    }
}

export_provider!(Gutenberg);
