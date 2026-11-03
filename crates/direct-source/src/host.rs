//! The host calls of an add: the book's media, the library it goes to, the ingest of its files.
use crate::Fail;
use fliks_plugin_sdk::bindings::fliks::plugin::types::{
    Family, HostError, IngestRequest, TransferMode,
};
use fliks_plugin_sdk::{ExternalId, host};
use std::path::Path;

/// The guest's view of the plugin data directory.
pub const DATA: &str = "/data";

pub fn data_root() -> &'static Path {
    Path::new(DATA)
}

/// A refusal of the request itself is final; an outage or a timeout is worth another try.
pub fn classify(e: HostError) -> Fail {
    let m = format!("{}: {}", e.code, e.message);
    match e.code.as_str() {
        "ERR_VALIDATION"
        | "ERR_DENIED"
        | "ERR_NOT_FOUND"
        | "ERR_CONFLICT"
        | "unsupported-kind"
        | "unsupported-provider"
        | "namespace-not-owned" => Fail::Drop(m),
        _ => Fail::Retry(m),
    }
}

/// The admin's `libraryId` setting; without it the host picks the sole library that accepts books.
pub fn library_id() -> Result<Option<u32>, Fail> {
    let keys = ["libraryId".to_owned()];
    let kv = host::config_get(Some(&keys)).map_err(classify)?;
    Ok(kv
        .into_iter()
        .find(|(k, _)| k == "libraryId")
        .and_then(|(_, v)| v.trim().parse().ok()))
}

/// The media of the book `id` of `namespace`, created when the library has none.
pub fn resolve(namespace: &str, id: &str) -> Result<u32, Fail> {
    let ids = [ExternalId {
        provider: namespace.to_owned(),
        value: id.to_owned(),
    }];
    host::media_resolve_or_create("book", &ids, library_id()?, None).map_err(classify)
}

/// Hands `paths` (guest paths under `/data`) to the library and lets the host remove them.
pub fn ingest(
    media: u32,
    family: Family,
    language: Option<String>,
    paths: Vec<String>,
    key: String,
    label: &str,
) -> Result<(), Fail> {
    host::library_ingest(&IngestRequest {
        idempotency_key: key,
        media_id: media,
        family,
        language,
        paths,
        transfer: TransferMode::Move,
        fallback_quality: None,
        source_label: label.to_owned(),
    })
    .map(|_| ())
    .map_err(classify)
}

/// Runs `payload` as the scheduled call `key` at once; a second call with the same key replaces it.
pub fn schedule(key: &str, payload: &serde_json::Value) -> Result<(), Fail> {
    host::schedule_call(key, 0, &payload.to_string()).map_err(classify)
}

/// The guest path of `name` under `/data`.
pub fn guest_path(name: &str) -> String {
    format!("{DATA}/{name}")
}
