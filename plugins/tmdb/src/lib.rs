//! `fliks.tmdb`: the TMDB metadata provider for movies and series.

mod map;
#[cfg(test)]
mod tests;
mod tmdb;

use fliks_plugin_sdk::bindings::wasi::http::types::Method;
use fliks_plugin_sdk::http::{Error as HttpError, Request};
use fliks_plugin_sdk::rt::{Budget, block_on};
use fliks_plugin_sdk::{ExternalId, Plugin, host, warn};
use fliks_plugin_sdk_provider::{
    Lang, MetadataProvider, ProviderError, SearchQuery, export_provider,
};
use serde_json::Value;
use std::time::Duration;
use tmdb::{Http, Query, Resolved, Tmdb, upstream};

type Res<T> = std::result::Result<T, ProviderError>;
use url::Url;

const API_URL: &str = "https://api.themoviedb.org/3";
const TIMEOUT: Duration = Duration::from_secs(10);

/// The wasm host's guarded HTTP client, with the API key from the plugin's config.
struct Live;

impl Live {
    fn api_key() -> Res<String> {
        let keys = ["apiKey".to_owned()];
        let key = host::config_get(Some(&keys))
            .map_err(|e| upstream(None, format!("{}: {}", e.code, e.message)))?
            .into_iter()
            .find(|(k, _)| k == "apiKey")
            .map(|(_, v)| v.trim().to_owned())
            .filter(|v| !v.is_empty());
        key.ok_or(ProviderError::Unauthorized)
    }
}

fn transport(e: HttpError) -> ProviderError {
    upstream(None, e.to_string())
}

impl Http for Live {
    async fn get(&self, path: &str, mut query: Query) -> Res<Value> {
        query.insert(0, ("api_key".into(), Self::api_key()?));
        let url = Url::parse_with_params(&format!("{API_URL}{path}"), query)
            .map_err(|_| upstream(None, "not a valid URL"))?;
        let resp = Request::new(Method::Get, url)
            .header("accept", "application/json")
            .timeout(TIMEOUT)
            .budget(&Budget::host())
            .send_async()
            .await
            .map_err(transport)?;
        tmdb::reply(
            resp.status,
            resp.header("retry-after").as_deref(),
            &resp.body,
        )
    }

    fn warn(&self, message: &str) {
        warn!("{message}");
    }
}

struct Provider;

impl Plugin for Provider {}

fn client() -> Tmdb<Live> {
    Tmdb { http: Live }
}

fn tv(kind: &str) -> bool {
    kind == "series"
}

/// A TMDB id is an integer; callers validate it first, so a failure here never reaches a response.
fn numeric(id: &ExternalId) -> Res<i64> {
    id.value
        .parse()
        .map_err(|_| upstream(None, "provider id is not a number"))
}

fn json(v: impl Into<Value>) -> Res<String> {
    Ok(v.into().to_string())
}

impl MetadataProvider for Provider {
    fn search(q: SearchQuery, lang: Lang) -> Res<String> {
        let found = block_on(client().search(tv(&q.kind), &q.query, q.year, &Resolved::of(&lang)))?;
        json(found)
    }

    fn fetch_work(kind: String, id: ExternalId, lang: Lang) -> Res<String> {
        let d = block_on(client().details(tv(&kind), numeric(&id)?, &Resolved::of(&lang)))?;
        json(d)
    }

    fn fetch_children(
        _kind: String,
        id: ExternalId,
        season: Option<u32>,
        lang: Lang,
    ) -> Res<String> {
        json(block_on(client().seasons(
            numeric(&id)?,
            season.map(i64::from),
            &Resolved::of(&lang),
        ))?)
    }

    fn child_stubs(_kind: String, id: ExternalId, lang: Lang) -> Res<String> {
        json(block_on(
            client().season_stubs(numeric(&id)?, &Resolved::of(&lang)),
        )?)
    }

    fn find_by(source: ExternalId, lang: Lang) -> Res<Option<(ExternalId, String)>> {
        let found =
            block_on(client().find_by(&source.provider, &source.value, &Resolved::of(&lang)))?;
        Ok(found.map(|(value, kind)| {
            let id = ExternalId {
                provider: "tmdb".into(),
                value,
            };
            (id, kind.into())
        }))
    }

    fn fetch_person(id: ExternalId, lang: Lang) -> Res<String> {
        json(block_on(
            client().person(numeric(&id)?, &Resolved::of(&lang)),
        )?)
    }

    fn person_credits(id: ExternalId, lang: Lang) -> Res<String> {
        json(block_on(
            client().credits(numeric(&id)?, &Resolved::of(&lang)),
        )?)
    }

    fn list(kind: String, name: String, params: Vec<(String, String)>, lang: Lang) -> Res<String> {
        json(block_on(client().list(
            tv(&kind),
            &name,
            &params,
            &Resolved::of(&lang),
        ))?)
    }
}

export_provider!(Provider);
