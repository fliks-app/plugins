//! `fliks.musicbrainz`: albums, releases and artists from MusicBrainz, covers from the Cover Art
//! Archive and popularity from ListenBrainz (`docs/spec/music.md` rules 18 to 24).

mod api;
mod map;
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

const DEFAULT_URL: &str = "https://musicbrainz.org";
const LISTENBRAINZ: &str = "https://api.listenbrainz.org";
const USER_AGENT: &str = concat!(
    "Fliks/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/fliks-app/fliks)"
);

struct Live;

/// The plugin's own settings: `baseUrl` (empty means MusicBrainz) and the optional `listenbrainzToken`.
fn setting(key: &str) -> R<Option<String>> {
    let keys = [key.to_owned()];
    Ok(host::config_get(Some(&keys))
        .map_err(|e| upstream(None, format!("{}: {}", e.code, e.message)))?
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.trim().to_owned())
        .filter(|v| !v.is_empty()))
}

impl Http for Live {
    fn mb(&self, path: &str, query: &[(&str, String)]) -> R<Value> {
        let base = setting("baseUrl")?.map_or_else(
            || DEFAULT_URL.to_owned(),
            |b| b.trim_end_matches('/').to_owned(),
        );
        let params = query.iter().map(|(k, v)| (*k, v.as_str()));
        let url = Url::parse_with_params(
            &format!("{base}/ws/2{path}"),
            params.chain([("fmt", "json")]),
        )
        .map_err(|_| upstream(None, "not a valid URL"))?;
        let resp = Request::new(Method::Get, url)
            .header("user-agent", USER_AGENT)
            .header("accept", "application/json")
            .timeout(Duration::from_secs(20))
            .budget(&Budget::host())
            .send()
            .map_err(|e| upstream(None, e.to_string()))?;
        reply(
            resp.status,
            resp.header("retry-after").as_deref(),
            &resp.body,
        )
    }

    fn lb(&self, path: &str, body: &Value) -> Option<Value> {
        let mut req = Request::new(Method::Post, format!("{LISTENBRAINZ}{path}"))
            .header("user-agent", USER_AGENT)
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .body(body.to_string())
            .timeout(Duration::from_secs(10))
            .budget(&Budget::host());
        if let Ok(Some(token)) = setting("listenbrainzToken") {
            req = req.header("authorization", &format!("Token {token}"));
        }
        let resp = req.send().ok()?;
        (200..300)
            .contains(&resp.status)
            .then(|| serde_json::from_slice(&resp.body).ok())?
    }
}

struct Provider;

impl Plugin for Provider {}

fn json_of(v: impl Into<Value>) -> R<String> {
    Ok(v.into().to_string())
}

impl MetadataProvider for Provider {
    fn search(q: SearchQuery, _lang: Lang) -> R<String> {
        let target = q
            .extra
            .iter()
            .find(|(k, _)| k == "target")
            .map(|(_, v)| v.as_str());
        match target {
            Some("artist") => return json_of(api::search_artists(&Live, &q.query, q.page)?),
            Some("recording") => return json_of(api::search_recordings(&Live, &q.query, q.page)?),
            _ => {}
        }
        json_of(api::search(
            &Live,
            &q.query,
            q.title.as_deref(),
            q.author.as_deref(),
            q.author_id.as_deref(),
            q.year,
            q.page,
        )?)
    }

    fn fetch_work(_kind: String, id: ExternalId, _lang: Lang) -> R<String> {
        json_of(api::work(&Live, &id.value)?)
    }

    fn fetch_children(
        _kind: String,
        id: ExternalId,
        _season: Option<u32>,
        _lang: Lang,
    ) -> R<String> {
        json_of(api::children(&Live, &id.value)?)
    }

    fn fetch_edition(edition: ExternalId, _lang: Lang) -> R<Option<String>> {
        Ok(Some(json_of(api::edition(&Live, &edition.value)?)?))
    }

    fn fetch_person(id: ExternalId, _lang: Lang) -> R<String> {
        json_of(api::person(&Live, &id.value)?)
    }

    fn person_credits(id: ExternalId, _lang: Lang) -> R<String> {
        json_of(api::credits(&Live, &id.value)?)
    }

    fn find_by(
        source: ExternalId,
        _lang: Lang,
    ) -> Result<Option<(ExternalId, String)>, ProviderError> {
        Ok(
            api::find_by(&Live, &source.provider, &source.value)?.map(|id| {
                (
                    ExternalId {
                        provider: "musicbrainz".into(),
                        value: id,
                    },
                    "album".into(),
                )
            }),
        )
    }
}

export_provider!(Provider);
