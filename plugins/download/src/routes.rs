//! The proxied routes. Bodies and statuses are the TypeScript plugin's, frozen by its tests.

use crate::db::{HistoryRow, blocklist, clients, history, indexer_stats};
use crate::error::{Error, R};
use crate::grab::{self, Manual, Stream, to_wire};
use crate::util::{Settings, js_number};
use crate::{client, indexers, poller, sources};
use fliks_plugin_sdk::bindings::fliks::plugin::types::{AcquisitionEvent, Family, Principal};
use fliks_plugin_sdk::host;
use fliks_plugin_sdk::router::{Handler, Request, Response, Router};
use serde_json::{Map, Value, json};
use std::sync::LazyLock;

fn ok(v: impl serde::Serialize) -> R<Response> {
    Ok(Response::json(200, &v))
}

fn err_body(status: u16, key: &str, detail: Option<&str>) -> Response {
    let mut e = json!({ "key": key });
    if let Some(d) = detail {
        e["detail"] = json!(d);
    }
    Response::json(status, &json!({ "error": e }))
}

fn bad_param(p: &str) -> R<Response> {
    Ok(err_body(400, "download.http.errors.bad_param", Some(p)))
}

fn bad_body(field: &str) -> R<Response> {
    Ok(err_body(400, "download.http.errors.bad_body", Some(field)))
}

fn not_found(detail: &str) -> R<Response> {
    Ok(err_body(
        404,
        "download.http.errors.not_found",
        Some(detail),
    ))
}

/// A domain error as its structured, i18n-keyed answer.
pub fn respond(r: R<Response>) -> Response {
    match r {
        Ok(r) => r,
        Err(Error::Grab { key, detail }) => {
            let status = if key == "download.grab.errors.media_not_found" {
                404
            } else {
                409
            };
            err_body(status, key, detail.as_deref())
        }
        Err(Error::NotFound(m)) => err_body(404, "download.http.errors.not_found", Some(&m)),
        Err(Error::UnknownImplementation(_)) => {
            err_body(400, "download.http.errors.bad_body", Some("implementation"))
        }
        Err(Error::Source {
            key,
            detail,
            message,
        }) => {
            error!("indexer source import refused: {message}");
            err_body(400, key, detail.as_deref())
        }
        Err(Error::Client(m)) => {
            error!("download client refused the request: {m}");
            err_body(502, "download.http.errors.download_client", Some(&m))
        }
        Err(e) => {
            error!("http handler failed: {e}");
            err_body(500, "download.http.errors.internal", Some(&e.to_string()))
        }
    }
}

fn body(r: &Request) -> Map<String, Value> {
    match serde_json::from_slice(&r.body) {
        Ok(Value::Object(o)) => o,
        _ => Map::new(),
    }
}

/// A canonical positive integer, as core's object guard requires of `:id`.
fn int_param(r: &Request, name: &str) -> Option<Option<u32>> {
    let v = r.param(name)?;
    let valid = !v.is_empty() && !v.starts_with('0') && v.bytes().all(|b| b.is_ascii_digit());
    Some(if valid { v.parse().ok() } else { None })
}

fn id(r: &Request) -> Result<u32, R<Response>> {
    int_param(r, "id").flatten().ok_or_else(|| bad_param("id"))
}

macro_rules! need_id {
    ($r:expr) => {
        match id($r) {
            Ok(i) => i64::from(i),
            Err(resp) => return resp,
        }
    };
}

/// `Math.max(1, Math.trunc(Number(raw)) || fallback)`.
fn page_number(raw: Option<&str>, fallback: i64) -> i64 {
    let n = js_number(raw.map(|s| Value::String(s.into())).as_ref()).trunc();
    let n = if n.is_nan() || n == 0.0 {
        fallback as f64
    } else {
        n
    };
    n.max(1.0) as i64
}

fn page_size(raw: Option<&str>) -> i64 {
    page_number(raw, 25).min(100)
}

fn str_field(b: &Map<String, Value>, k: &str) -> Option<String> {
    b.get(k).and_then(Value::as_str).map(str::to_owned)
}
fn num_field(b: &Map<String, Value>, k: &str) -> Option<f64> {
    b.get(k).and_then(Value::as_f64)
}
fn bool_field(b: &Map<String, Value>, k: &str) -> Option<bool> {
    b.get(k).and_then(Value::as_bool)
}
fn settings_field(b: &Map<String, Value>) -> Option<Settings> {
    b.get("settings").and_then(Value::as_object).cloned()
}

// --- releases ------------------------------------------------------------------------------

fn scope(r: &Request) -> Result<(u32, Option<u32>, Option<u32>), R<Response>> {
    let media = id(r)?;
    let group = match int_param(r, "groupId") {
        Some(None) => return Err(bad_param("groupId")),
        g => g.flatten(),
    };
    let item = match int_param(r, "itemId") {
        Some(None) => return Err(bad_param("itemId")),
        i => i.flatten(),
    };
    Ok((media, group, item))
}

/// An absent family keeps the media's only want; an unknown one is refused.
fn family_arg(raw: Option<&str>) -> Result<Option<Family>, R<Response>> {
    match raw.filter(|f| !f.is_empty()) {
        None => Ok(None),
        Some(f) => grab::family_of(f)
            .map(Some)
            .ok_or_else(|| bad_param("family")),
    }
}

fn search_releases(r: &Request) -> R<Response> {
    let (media, group, item) = match scope(r) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let family = match family_arg(r.query("family")) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    // The core's sweep (no user): what this provider would grab, from the auto-search indexers.
    let releases = if matches!(r.principal, Principal::System) {
        grab::search_auto(media, group, item, family, r.query("language"))?
    } else {
        grab::search_releases(
            media,
            group,
            item,
            family,
            r.query("language"),
            r.query("q"),
            stream_of(r).as_ref(),
        )?
    };
    ok(releases.iter().map(to_wire).collect::<Vec<_>>())
}

/// Only a delegated viewer with a modal open (`sid`) gets the streamed partial results.
fn stream_of(r: &Request) -> Option<Stream> {
    match (&r.principal, r.query("sid")) {
        (Principal::Delegated(user), Some(sid)) if !sid.is_empty() => Some(Stream {
            user_id: *user,
            search_id: sid.to_owned(),
        }),
        _ => None,
    }
}

/// Only an absolute http(s) URL is kept: it reaches a browser as an anchor.
fn info_url(raw: Option<&Value>) -> Option<String> {
    let s = raw?.as_str().filter(|s| !s.is_empty())?;
    let u = url::Url::parse(s).ok()?;
    matches!(u.scheme(), "http" | "https").then(|| s.to_owned())
}

fn grab_release(r: &Request) -> R<Response> {
    let (media, group, item) = match scope(r) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let who = match r.principal {
        Principal::Delegated(u) => format!("user #{u}"),
        Principal::System => "system".into(),
    };
    info!("grab http request for media #{media} by {who}");
    let b = body(r);
    let family = match family_arg(str_field(&b, "family").as_deref()) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    let auto = match str_field(&b, "method").as_deref() {
        None | Some("manual") => false,
        Some("auto") => true,
        Some(_) => return bad_param("method"),
    };
    let Some(manual) = manual_of(&b, auto) else {
        return bad_param("downloadUrl");
    };
    ok(grab::grab_release(
        media,
        group,
        item,
        family,
        str_field(&b, "language").as_deref(),
        manual,
    )?)
}

/// A grab names its release and its method (`auto` from the core's sweep, else `manual`).
fn manual_of(b: &Map<String, Value>, auto: bool) -> Option<Manual> {
    str_field(b, "downloadUrl")
        .filter(|u| !u.is_empty())
        .map(|download_url| Manual {
            auto,
            download_url,
            source_title: str_field(b, "sourceTitle"),
            indexer_id: num_field(b, "sourceId").map(|f| f as i64),
            info_url: info_url(b.get("infoUrl")),
            force: b.get("force") == Some(&Value::Bool(true)),
        })
}

// --- indexers ------------------------------------------------------------------------------

enum Gates {
    Ok(Option<bool>, Option<bool>, Option<bool>),
    Empty,
    Invalid,
}

/// `useFor` wins over the three booleans: a save body spreads the row the client last read.
fn gates(b: &Map<String, Value>) -> Gates {
    match b.get("useFor") {
        Some(Value::Array(a)) if a.is_empty() => Gates::Empty,
        Some(Value::Array(a))
            if a.iter()
                .all(|v| v.as_str().is_some_and(|s| indexers::USE_FOR.contains(&s))) =>
        {
            let has = |k: &str| Some(a.iter().any(|v| v == k));
            Gates::Ok(has("rss"), has("auto"), has("manual"))
        }
        Some(_) => Gates::Invalid,
        None => Gates::Ok(
            bool_field(b, "enableRss"),
            bool_field(b, "enableSearch"),
            bool_field(b, "enableInteractiveSearch"),
        ),
    }
}

fn indexer_input(b: &Map<String, Value>) -> Result<indexers::Input, R<Response>> {
    let (rss, search, interactive) = match gates(b) {
        Gates::Ok(a, b, c) => (a, b, c),
        Gates::Empty => {
            return Err(Ok(err_body(
                400,
                "download.config.indexers.errors.no_usage",
                None,
            )));
        }
        Gates::Invalid => return Err(bad_body("useFor")),
    };
    Ok(indexers::Input {
        name: str_field(b, "name"),
        implementation: str_field(b, "implementation"),
        settings: settings_field(b),
        enable_rss: rss,
        enable_search: search,
        enable_interactive_search: interactive,
        priority: num_field(b, "priority"),
        request_delay: num_field(b, "requestDelay"),
        enabled: bool_field(b, "enabled"),
    })
}

/// `'name'`/`'implementation'` when either is missing on a create.
fn required(b: &Map<String, Value>) -> Option<&'static str> {
    if str_field(b, "name").is_none_or(|n| n.trim().is_empty()) {
        return Some("name");
    }
    if str_field(b, "implementation").is_none_or(|i| i.is_empty()) {
        return Some("implementation");
    }
    None
}

fn list_indexers(_: &Request) -> R<Response> {
    ok(indexers::list()?)
}

fn create_indexer(r: &Request) -> R<Response> {
    let b = body(r);
    if let Some(f) = required(&b) {
        return bad_body(f);
    }
    let input = match indexer_input(&b) {
        Ok(i) => i,
        Err(resp) => return resp,
    };
    Ok(Response::json(201, &indexers::create(input)?))
}

fn update_indexer(r: &Request) -> R<Response> {
    let id = need_id!(r);
    let b = body(r);
    if b.get("name").is_some() && str_field(&b, "name").is_none_or(|n| n.trim().is_empty()) {
        return bad_body("name");
    }
    let input = match indexer_input(&b) {
        Ok(i) => i,
        Err(resp) => return resp,
    };
    ok(indexers::update(id, input)?)
}

fn delete_indexer(r: &Request) -> R<Response> {
    let id = need_id!(r);
    indexers::remove(id)?;
    ok(json!({}))
}

/// `(implementation, settings, id)` of a test-connection body.
fn test_input(r: &Request) -> Option<(String, Settings, Option<i64>)> {
    let b = body(r);
    let implementation = str_field(&b, "implementation")?;
    let id = b.get("id").and_then(Value::as_i64);
    Some((implementation, settings_field(&b).unwrap_or_default(), id))
}

fn test_indexer(r: &Request) -> R<Response> {
    let Some((i, s, id)) = test_input(r) else {
        return bad_body("implementation");
    };
    ok(indexers::test_connection(&i, &s, id))
}

fn indexer_stats(r: &Request) -> R<Response> {
    let id = need_id!(r);
    ok(indexer_stats::daily(id)?)
}

fn clear_cooldown(r: &Request) -> R<Response> {
    let id = need_id!(r);
    ok(indexers::clear_cooldown(id)?)
}

fn clear_cooldowns(_: &Request) -> R<Response> {
    ok(indexers::clear_all_cooldowns()?)
}

// --- indexer sources -----------------------------------------------------------------------

fn source_input(b: &Map<String, Value>) -> sources::Input {
    sources::Input {
        name: str_field(b, "name"),
        implementation: str_field(b, "implementation"),
        settings: settings_field(b),
        priority: num_field(b, "priority"),
        enabled: bool_field(b, "enabled"),
    }
}

fn list_sources(_: &Request) -> R<Response> {
    ok(crate::db::indexer_sources::list_all()?
        .iter()
        .map(sources::redact)
        .collect::<Vec<_>>())
}

fn create_source(r: &Request) -> R<Response> {
    let b = body(r);
    if let Some(f) = required(&b) {
        return bad_body(f);
    }
    Ok(Response::json(201, &sources::create(source_input(&b))?))
}

fn update_source(r: &Request) -> R<Response> {
    let id = need_id!(r);
    ok(sources::update(id, source_input(&body(r)))?)
}

fn delete_source(r: &Request) -> R<Response> {
    let id = need_id!(r);
    sources::remove(id)?;
    ok(json!({}))
}

fn test_source(r: &Request) -> R<Response> {
    let Some((i, s, id)) = test_input(r) else {
        return bad_body("implementation");
    };
    ok(sources::test_connection(&i, &s, id))
}

fn import_source(r: &Request) -> R<Response> {
    let id = need_id!(r);
    ok(sources::import(id)?)
}

// --- download clients ----------------------------------------------------------------------

fn client_input(b: &Map<String, Value>) -> client::Input {
    client::Input {
        name: str_field(b, "name"),
        implementation: str_field(b, "implementation"),
        settings: settings_field(b),
        enabled: bool_field(b, "enabled"),
        priority: num_field(b, "priority"),
    }
}

fn list_clients(_: &Request) -> R<Response> {
    ok(clients::list_all()?
        .iter()
        .map(client::redact)
        .collect::<Vec<_>>())
}

fn create_client(r: &Request) -> R<Response> {
    let b = body(r);
    if let Some(f) = required(&b) {
        return bad_body(f);
    }
    Ok(Response::json(201, &client::create(client_input(&b))?))
}

fn update_client(r: &Request) -> R<Response> {
    let id = need_id!(r);
    ok(client::update(id, client_input(&body(r)))?)
}

fn delete_client(r: &Request) -> R<Response> {
    let id = need_id!(r);
    client::remove(id)?;
    ok(json!({}))
}

fn test_client(r: &Request) -> R<Response> {
    let Some((i, s, id)) = test_input(r) else {
        return bad_body("implementation");
    };
    ok(client::test(&i, &s, id))
}

// --- blocklist -----------------------------------------------------------------------------

fn list_blocklist(r: &Request) -> R<Response> {
    let page = page_number(r.query("page"), 1);
    let size = page_size(r.query("pageSize"));
    let (items, total) = blocklist::list(size, (page - 1) * size)?;
    ok(json!({ "data": items, "total": total, "page": page, "pageSize": size }))
}

fn clear_blocklist(_: &Request) -> R<Response> {
    blocklist::clear()?;
    ok(json!({}))
}

fn remove_blocklist(r: &Request) -> R<Response> {
    let id = need_id!(r);
    if blocklist::find(id)?.is_none() {
        return not_found(&id.to_string());
    }
    blocklist::remove(id)?;
    ok(json!({}))
}

// --- queue ---------------------------------------------------------------------

/// Whether the client can be asked: enabled, of a known kind and drivable.
fn client_usable(id: i64) -> R<bool> {
    Ok(clients::list_all()?.iter().any(|c| {
        c.id == id
            && c.enabled
            && client::IMPLEMENTATIONS.contains(&c.implementation.as_str())
            && client::supports(c)
    }))
}

// --- queue actions -------------------------------------------------------------------------

/// The history row a queue action names: `{ref}` is what the snapshot reported (`poller::queue_ref`).
fn target(r: &Request) -> Result<HistoryRow, R<Response>> {
    let b = body(r);
    let Some(reference) = b.get("ref").and_then(Value::as_str) else {
        return Err(bad_param("ref"));
    };
    let Some(id) = reference
        .split('-')
        .next()
        .and_then(|s| s.parse::<i64>().ok())
    else {
        return Err(bad_param("ref"));
    };
    match history::find(id) {
        Ok(Some(row)) if poller::queue_ref(&row) == reference => Ok(row),
        Ok(_) => Err(not_found(reference)),
        Err(e) => Err(Err(e)),
    }
}

/// Only a row in one of `states` can be acted on: an importing one's files are being moved.
fn controllable(r: &Request, states: &[&str]) -> Result<HistoryRow, R<Response>> {
    let row = target(r)?;
    if !states.contains(&row.status.as_str()) {
        return Err(Ok(err_body(
            409,
            "download.queue.errors.not_controllable",
            Some(&row.status),
        )));
    }
    Ok(row)
}

fn settle(row: &HistoryRow, expect: poller::Outcome) {
    if let Err(e) = poller::settle_and_publish(row, expect) {
        warn!("queue publish failed: {e}");
    }
}

fn control(r: &Request, pause: bool) -> R<Response> {
    let row = match controllable(r, &["grabbed"]) {
        Ok(row) => row,
        Err(resp) => return resp,
    };
    // A row the client has not matched yet is matched now, so a queued transfer can be held.
    let row = if row.torrent_hash.as_deref().is_none_or(str::is_empty) {
        poller::match_now(row)?
    } else {
        row
    };
    let (Some(client), Some(hash)) = (
        row.download_client_id,
        row.torrent_hash.clone().filter(|h| !h.is_empty()),
    ) else {
        return Ok(err_body(
            409,
            "download.queue.errors.no_torrent",
            Some(&row.id.to_string()),
        ));
    };
    let c = client::drivable(client)?;
    if pause {
        client::pause(&c, &hash)?
    } else {
        client::resume(&c, &hash)?
    }
    settle(&row, if pause { "paused" } else { "running" });
    ok(json!({}))
}

fn pause(r: &Request) -> R<Response> {
    control(r, true)
}

fn resume(r: &Request) -> R<Response> {
    control(r, false)
}

/// Removes the torrent and its files when its client can be asked, retires the row either way, and
/// blocklists the release when the core says so. An importing row is only retired: its files are in use.
fn remove_queued(r: &Request) -> R<Response> {
    let row = match controllable(r, &["grabbed", "importing", "import_failed"]) {
        Ok(row) => row,
        Err(resp) => return resp,
    };
    if row.status == "importing" {
        history::mark_failed(row.id, "download.queue.removed_by_user")?;
        if body(r).get("blocklist") == Some(&json!(true)) {
            poller::auto_blocklist(&row, "Blocklisted when removed from the queue");
        }
        settle(&row, "absent");
        return ok(json!({}));
    }
    let consulted = match row.download_client_id {
        Some(c) => client_usable(c)?,
        None => false,
    };
    match (
        consulted,
        row.download_client_id,
        row.torrent_hash.as_deref().filter(|h| !h.is_empty()),
    ) {
        (true, Some(client), Some(hash)) => {
            client::delete(&client::drivable(client)?, hash, true)?;
            history::mark_failed(row.id, "download.queue.removed_by_user")?;
        }
        _ => history::mark_failed(row.id, "download.queue.retired_unverifiable")?,
    }
    if body(r).get("blocklist") == Some(&json!(true)) {
        poller::auto_blocklist(&row, "Blocklisted when removed from the queue");
    }
    settle(&row, "absent");
    ok(json!({}))
}

/// Hands a refused import back to the next `ImportCompleted` tick.
fn retry_import(r: &Request) -> R<Response> {
    let id = match target(r) {
        Ok(row) => row.id,
        Err(resp) => return resp,
    };
    if !history::retry_import(id)? {
        return match history::find(id)? {
            None => not_found(&id.to_string()),
            Some(row) => Ok(err_body(
                409,
                "download.queue.errors.not_retryable",
                Some(&row.status),
            )),
        };
    }
    if let Err(e) = host::events_publish(&[AcquisitionEvent::QueueChanged]) {
        warn!("queue-changed publish failed: {}", e.message);
    }
    ok(json!({}))
}

// --- implementations -----------------------------------------------------------------------

static INDEXER_IMPLEMENTATIONS: LazyLock<Value> = LazyLock::new(|| {
    let f = "download.config.indexers.fields";
    json!([{
        "implementation": "torznab",
        "labelKey": "download.config.indexers.implementations.torznab",
        "fields": [
            { "key": "baseUrl", "type": "url", "labelKey": format!("{f}.base_url"), "required": true },
            { "key": "apiKey", "type": "password", "labelKey": format!("{f}.api_key"), "secret": true },
            { "key": "requestDelay", "type": "number", "labelKey": format!("{f}.request_delay"), "hint": format!("{f}.request_delay_hint"), "default": 2, "topLevel": true },
            {
                "key": "useFor", "type": "multiselect", "labelKey": format!("{f}.use_for"), "hint": format!("{f}.use_for_hint"),
                "required": true, "topLevel": true, "default": ["rss", "auto", "manual"],
                "options": [
                    { "value": "rss", "labelKey": format!("{f}.use_for_rss") },
                    { "value": "auto", "labelKey": format!("{f}.use_for_auto") },
                    { "value": "manual", "labelKey": format!("{f}.use_for_manual") },
                ],
            },
            { "key": "minSeeders", "type": "number", "labelKey": format!("{f}.min_seeders"), "default": 0 },
            { "key": "seedRatio", "type": "number", "labelKey": format!("{f}.seed_ratio"), "hint": format!("{f}.seed_ratio_hint"), "default": 1 },
            { "key": "maxRetentionDays", "type": "number", "labelKey": format!("{f}.max_retention_days"), "hint": format!("{f}.max_retention_days_hint") },
            { "key": "unknownLanguageIsoCode", "type": "text", "labelKey": format!("{f}.unknown_language"), "hint": format!("{f}.unknown_language_hint") },
        ],
    }])
});

static SOURCE_IMPLEMENTATIONS: LazyLock<Value> = LazyLock::new(|| {
    let def = |imp: &str| {
        json!({
            "implementation": imp,
            "labelKey": format!("download.config.indexer_sources.implementations.{imp}"),
            "fields": [
                { "key": "baseUrl", "type": "url", "labelKey": "download.config.indexer_sources.fields.base_url", "required": true },
                { "key": "apiKey", "type": "password", "labelKey": "download.config.indexer_sources.fields.api_key", "secret": true, "required": true },
            ],
        })
    };
    json!([def("prowlarr"), def("jackett")])
});

static CLIENT_IMPLEMENTATIONS: LazyLock<Value> = LazyLock::new(|| {
    let f = "download.config.download_clients.fields";
    let def = |imp: &str, port: u16, url_base: Option<&str>| {
        let mut fields = vec![
            json!({ "key": "host", "type": "text", "labelKey": format!("{f}.host"), "required": true, "default": "localhost" }),
            json!({ "key": "port", "type": "number", "labelKey": format!("{f}.port"), "default": port }),
            json!({ "key": "useSsl", "type": "toggle", "labelKey": format!("{f}.use_ssl"), "default": false }),
        ];
        if let Some(u) = url_base {
            fields.push(json!({ "key": "urlBase", "type": "text", "labelKey": format!("{f}.url_base"), "default": u }));
        }
        fields.extend([
            json!({ "key": "username", "type": "text", "labelKey": format!("{f}.username") }),
            json!({ "key": "password", "type": "password", "labelKey": format!("{f}.password"), "secret": true }),
            json!({ "key": "category", "type": "text", "labelKey": format!("{f}.category"), "default": "fliks" }),
            json!({ "key": "movieCategory", "type": "text", "labelKey": format!("{f}.movie_category") }),
            json!({ "key": "seriesCategory", "type": "text", "labelKey": format!("{f}.series_category") }),
            json!({ "key": "ebookCategory", "type": "text", "labelKey": format!("{f}.ebook_category") }),
            json!({ "key": "audiobookCategory", "type": "text", "labelKey": format!("{f}.audiobook_category") }),
            json!({ "key": "comicCategory", "type": "text", "labelKey": format!("{f}.comic_category") }),
            json!({ "key": "albumCategory", "type": "text", "labelKey": format!("{f}.album_category") }),
        ]);
        json!({
            "implementation": imp,
            "labelKey": format!("download.config.download_clients.implementations.{imp}"),
            "fields": fields,
        })
    };
    json!([
        def("qbittorrent", 8080, None),
        def("transmission", 9091, Some("/transmission/")),
    ])
});

fn indexer_implementations(_: &Request) -> R<Response> {
    ok(&*INDEXER_IMPLEMENTATIONS)
}
fn source_implementations(_: &Request) -> R<Response> {
    ok(&*SOURCE_IMPLEMENTATIONS)
}
fn client_implementations(_: &Request) -> R<Response> {
    ok(&*CLIENT_IMPLEMENTATIONS)
}

// --- table ---------------------------------------------------------------------------------

macro_rules! h {
    ($f:path) => {{
        fn wrapped(r: &Request) -> Result<Response, String> {
            Ok(respond($f(r)))
        }
        wrapped as Handler
    }};
}

/// Literal segments come before their `:id` siblings: the first match wins.
pub fn table() -> Vec<(&'static str, &'static str, Handler)> {
    vec![
        ("GET", "/:id/releases", h!(search_releases)),
        ("POST", "/:id/grab", h!(grab_release)),
        ("GET", "/:id/groups/:groupId/releases", h!(search_releases)),
        ("POST", "/:id/groups/:groupId/grab", h!(grab_release)),
        ("GET", "/:id/items/:itemId/releases", h!(search_releases)),
        ("POST", "/:id/items/:itemId/grab", h!(grab_release)),
        ("GET", "/indexers", h!(list_indexers)),
        ("POST", "/indexers", h!(create_indexer)),
        ("POST", "/indexers/test-connection", h!(test_indexer)),
        ("DELETE", "/indexers/cooldowns", h!(clear_cooldowns)),
        (
            "GET",
            "/indexers/implementations",
            h!(indexer_implementations),
        ),
        ("PUT", "/indexers/:id", h!(update_indexer)),
        ("DELETE", "/indexers/:id", h!(delete_indexer)),
        ("DELETE", "/indexers/:id/cooldown", h!(clear_cooldown)),
        ("GET", "/indexers/:id/stats", h!(indexer_stats)),
        ("GET", "/indexer-sources", h!(list_sources)),
        ("POST", "/indexer-sources", h!(create_source)),
        ("POST", "/indexer-sources/test-connection", h!(test_source)),
        (
            "GET",
            "/indexer-sources/implementations",
            h!(source_implementations),
        ),
        ("PUT", "/indexer-sources/:id", h!(update_source)),
        ("DELETE", "/indexer-sources/:id", h!(delete_source)),
        ("POST", "/indexer-sources/:id/import", h!(import_source)),
        ("GET", "/download-clients", h!(list_clients)),
        ("POST", "/download-clients", h!(create_client)),
        ("POST", "/download-clients/test-connection", h!(test_client)),
        (
            "GET",
            "/download-clients/implementations",
            h!(client_implementations),
        ),
        ("PUT", "/download-clients/:id", h!(update_client)),
        ("DELETE", "/download-clients/:id", h!(delete_client)),
        ("GET", "/blocklist", h!(list_blocklist)),
        ("DELETE", "/blocklist/all", h!(clear_blocklist)),
        ("DELETE", "/blocklist/:id", h!(remove_blocklist)),
        ("POST", "/queue/pause", h!(pause)),
        ("POST", "/queue/resume", h!(resume)),
        ("POST", "/queue/remove", h!(remove_queued)),
        ("POST", "/queue/retry-import", h!(retry_import)),
    ]
}

pub static ROUTER: LazyLock<Router> = LazyLock::new(|| {
    table()
        .into_iter()
        .fold(Router::new(), |r, (m, p, h)| r.route(m, p, h))
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_parsing() {
        assert_eq!(page_number(None, 1), 1);
        assert_eq!(page_number(Some("2.7"), 1), 2);
        assert_eq!(page_number(Some("-3"), 1), 1);
        assert_eq!(page_number(Some(""), 1), 1);
        assert_eq!(page_size(Some("500")), 100);
        assert_eq!(page_size(Some("0")), 25);
        assert_eq!(page_size(None), 25);
    }

    #[test]
    fn info_urls() {
        assert_eq!(
            info_url(Some(&json!("https://t/x"))),
            Some("https://t/x".into())
        );
        assert_eq!(info_url(Some(&json!("javascript:alert(1)"))), None);
        assert_eq!(info_url(Some(&json!("/rel"))), None);
    }

    #[test]
    fn use_for_gates() {
        let b = |v: Value| v.as_object().unwrap().clone();
        assert!(matches!(gates(&b(json!({ "useFor": [] }))), Gates::Empty));
        assert!(matches!(
            gates(&b(json!({ "useFor": ["bogus"] }))),
            Gates::Invalid
        ));
        assert!(matches!(
            gates(&b(json!({ "useFor": null }))),
            Gates::Invalid
        ));
        assert!(matches!(
            gates(&b(json!({ "useFor": ["auto"], "enableRss": true }))),
            Gates::Ok(Some(false), Some(true), Some(false))
        ));
        assert!(matches!(
            gates(&b(json!({ "enableRss": false }))),
            Gates::Ok(Some(false), None, None)
        ));
    }
}
