//! `fliks.tvdb`: the TheTVDB metadata provider for movies and series.

mod map;
#[cfg(test)]
mod tests;
mod tvdb;

use fliks_plugin_sdk::bindings::wasi::http::types::Method;
use fliks_plugin_sdk::http::{Error as HttpError, Request};
use fliks_plugin_sdk::rt::{Budget, block_on};
use fliks_plugin_sdk::{ExternalId, Plugin, host};
use fliks_plugin_sdk_provider::{
    Lang, MetadataProvider, ProviderError, SearchQuery, export_provider,
};
use serde_json::{Value, json};
use std::rc::Rc;
use std::time::Duration;
use tvdb::{Auth, Http, Query, Tvdb, code, upstream};
use url::Url;

type Res<T> = std::result::Result<T, ProviderError>;

const API_URL: &str = "https://api4.thetvdb.com/v4";
const TIMEOUT: Duration = Duration::from_secs(10);

thread_local! {
    static AUTH: Rc<Auth> = Rc::default();
}

/// The wasm host's guarded HTTP client, with the API key from the plugin's config.
struct Live;

impl Live {
    async fn send(request: Request) -> Res<Value> {
        let resp = request
            .header("accept", "application/json")
            .timeout(TIMEOUT)
            .budget(&Budget::host())
            .send_async()
            .await
            .map_err(transport)?;
        tvdb::reply(
            resp.status,
            resp.header("retry-after").as_deref(),
            &resp.body,
        )
    }
}

fn transport(e: HttpError) -> ProviderError {
    upstream(None, e.to_string())
}

impl Http for Live {
    fn api_key(&self) -> Res<String> {
        let keys = ["apiKey".to_owned()];
        let key = host::config_get(Some(&keys))
            .map_err(|e| upstream(None, format!("{}: {}", e.code, e.message)))?
            .into_iter()
            .find(|(k, _)| k == "apiKey")
            .map(|(_, v)| v.trim().to_owned())
            .filter(|v| !v.is_empty());
        key.ok_or(ProviderError::Unauthorized)
    }

    async fn login(&self, key: &str) -> Res<Value> {
        let body = json!({"apikey": key}).to_string();
        let request = Request::new(Method::Post, format!("{API_URL}/login"))
            .header("content-type", "application/json")
            .body(body);
        Self::send(request).await
    }

    async fn get(&self, token: &str, path: &str, query: Query) -> Res<Value> {
        let url = Url::parse_with_params(&format!("{API_URL}{path}"), query)
            .map_err(|_| upstream(None, "not a valid URL"))?;
        Self::send(
            Request::new(Method::Get, url).header("authorization", &format!("Bearer {token}")),
        )
        .await
    }
}

struct Provider;

impl Plugin for Provider {}

fn client() -> Tvdb<Live> {
    Tvdb {
        http: Live,
        auth: AUTH.with(Rc::clone),
    }
}

fn tv(kind: &str) -> bool {
    kind == "series"
}

/// A TVDB id is an integer; callers validate it first, so a failure here never reaches a response.
fn numeric(id: &ExternalId) -> Res<i64> {
    id.value
        .parse()
        .map_err(|_| upstream(None, "provider id is not a number"))
}

fn json_of(v: impl Into<Value>) -> Res<String> {
    Ok(v.into().to_string())
}

impl MetadataProvider for Provider {
    fn search(q: SearchQuery, lang: Lang) -> Res<String> {
        let found =
            block_on(client().search_titles(tv(&q.kind), &q.query, q.year, code(&lang.language)))?;
        json_of(found)
    }

    fn fetch_work(kind: String, id: ExternalId, lang: Lang) -> Res<String> {
        let d = block_on(client().details(tv(&kind), numeric(&id)?, code(&lang.language)))?;
        json_of(d)
    }

    fn fetch_children(_kind: String, id: ExternalId, lang: Lang) -> Res<String> {
        json_of(block_on(
            client().seasons(numeric(&id)?, code(&lang.language)),
        )?)
    }

    fn child_stubs(_kind: String, id: ExternalId, _lang: Lang) -> Res<String> {
        json_of(block_on(client().season_stubs(numeric(&id)?))?)
    }

    fn find_by(source: ExternalId, _lang: Lang) -> Res<Option<(ExternalId, String)>> {
        if source.provider != "imdb" {
            return Ok(None);
        }
        let found = block_on(client().find_by_imdb(&source.value))?;
        Ok(found.map(|(value, kind)| {
            let id = ExternalId {
                provider: "tvdb".into(),
                value,
            };
            (id, kind.into())
        }))
    }

    fn fetch_person(id: ExternalId, lang: Lang) -> Res<String> {
        json_of(block_on(
            client().person(numeric(&id)?, code(&lang.language)),
        )?)
    }

    fn person_credits(id: ExternalId, _lang: Lang) -> Res<String> {
        json_of(block_on(client().credits(numeric(&id)?))?)
    }
}

export_provider!(Provider);
