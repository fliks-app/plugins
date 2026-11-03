//! The search table and its add action.
use crate::api;
use crate::parse::NS;
use crate::{SOURCE, api_client};
use direct_source::{Fail, host};
use fliks_plugin_sdk::router::{Request, Response, Router};
use fliks_plugin_sdk_provider::ProviderError;
use serde_json::{Value, json};
use std::sync::LazyLock;

pub static ROUTER: LazyLock<Router> = LazyLock::new(|| {
    Router::new()
        .route("GET", "/search", search)
        .route("POST", "/add/:id", add)
});

fn fail(e: ProviderError) -> Response {
    let (status, message) = match e {
        ProviderError::NotFound => (404, "not found".to_owned()),
        ProviderError::RateLimited(_) => (429, "the source is rate limiting".to_owned()),
        ProviderError::Upstream(u) => (502, u.message),
        ProviderError::Unauthorized | ProviderError::Unsupported => (502, "unavailable".to_owned()),
    };
    Response::json(status, &json!({ "error": message }))
}

fn search(r: &Request) -> Result<Response, String> {
    let page = r.query("page").and_then(|p| p.parse().ok()).unwrap_or(1);
    match api::search_books(
        &api_client(),
        r.query("q").unwrap_or(""),
        r.query("language"),
        page,
    ) {
        Ok(books) => Ok(Response::json(
            200,
            &books.iter().map(|b| b.row()).collect::<Vec<Value>>(),
        )),
        Err(e) => Ok(fail(e)),
    }
}

fn add(r: &Request) -> Result<Response, String> {
    let id = r.param("id").unwrap_or_default();
    let book = match api::book(&api_client(), &id) {
        Ok(b) => b,
        Err(e) => return Ok(fail(e)),
    };
    if book.sections.is_empty() {
        return Ok(Response::json(
            409,
            &json!({ "error": "the recording has no section" }),
        ));
    }
    let media = match host::resolve(NS, &id) {
        Ok(m) => m,
        Err(Fail::Drop(m)) => return Ok(Response::json(409, &json!({ "error": m }))),
        Err(Fail::Retry(m)) => return Ok(Response::json(503, &json!({ "error": m }))),
    };
    let payload = json!({ "id": id, "mediaId": media });
    if let Err(e) = host::schedule(&format!("{SOURCE}:{id}"), &payload) {
        return Ok(Response::json(503, &json!({ "error": e.to_string() })));
    }
    Ok(Response::json(202, &json!({ "mediaId": media })))
}
