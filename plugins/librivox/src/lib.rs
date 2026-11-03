//! `fliks.librivox`: LibriVox recordings, as a metadata provider and a source a user adds an
//! audiobook from.

mod add;
mod api;
mod parse;
mod routes;

use add::Progress;
use direct_source::host::{self, Ingest};
use direct_source::net::Live;
use direct_source::{Fail, flow};
use fliks_plugin_sdk::bindings::fliks::plugin::types::Family;
use fliks_plugin_sdk::rt::Budget;
use fliks_plugin_sdk::{ExternalId, HttpRequest, HttpResponse, Plugin};
use fliks_plugin_sdk_provider::{
    Lang, MetadataProvider, ProviderError, SearchQuery, export_provider,
};
use serde::Deserialize;
use std::time::Duration;

/// The prefix of this plugin's scheduled calls.
const SOURCE: &str = "add";
const USER_AGENT: &str = concat!(
    "Fliks-LibriVox/",
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

/// The catalogue API for the search route, which the host does not pace: one request per second.
fn search_client() -> Live {
    Live {
        gap: Duration::from_secs(1),
        ..api_client()
    }
}

/// The audio files: one request per second, a section is at most 128 MiB.
fn file_client() -> Live {
    Live {
        user_agent: USER_AGENT,
        accept: "audio/mpeg",
        gap: Duration::from_secs(1),
        cap: 128 << 20,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    id: String,
    media_id: u32,
}

/// Downloads what is missing; once every section is there, hands them to the library as one audio
/// version, or schedules itself again when the call's time ran out first.
fn add_book(key: &str, p: &Payload) -> Result<(), Fail> {
    let book = api::book(&api_client(), &p.id).map_err(|e| match e {
        ProviderError::NotFound => Fail::Drop("the recording is gone".into()),
        _ => Fail::Retry("the LibriVox catalogue did not answer".into()),
    })?;
    let left = || Budget::host().remaining();
    match add::fetch(&file_client(), host::data_root(), &book, &left)? {
        Progress::More => host::schedule(
            key,
            &serde_json::json!({ "id": p.id, "mediaId": p.media_id }),
        ),
        Progress::Done(names) => {
            let req = Ingest {
                media: p.media_id,
                family: Family::Audio,
                language: book.language,
                key: format!("librivox:{}", p.id),
                label: "LibriVox",
            };
            flow::conclude(
                &flow::Host,
                host::data_root(),
                &add::dir(book.id),
                &req,
                &names,
            )
        }
    }
}

struct LibriVox;

impl Plugin for LibriVox {
    fn scheduled(key: String, payload: String, attempt: u32) -> Result<(), String> {
        let Ok(p) = serde_json::from_str::<Payload>(&payload) else {
            fliks_plugin_sdk::warn!("scheduled call {key} dropped: unreadable payload");
            return Ok(());
        };
        let dir = p.id.parse().map(add::dir).unwrap_or_default();
        let r = add_book(&key, &p);
        flow::settle(host::data_root(), &dir, attempt, r)
    }

    fn http(req: HttpRequest) -> Result<HttpResponse, String> {
        routes::ROUTER.handle(req)
    }
}

impl MetadataProvider for LibriVox {
    fn search(q: SearchQuery, _lang: Lang) -> Result<String, ProviderError> {
        api::search(&api_client(), &q)
    }
    fn fetch_work(kind: String, id: ExternalId, _lang: Lang) -> Result<String, ProviderError> {
        api::work(&api_client(), &kind, &id)
    }
}

export_provider!(LibriVox);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_route_is_paced() {
        assert!(!search_client().gap.is_zero());
    }
}
