//! Indexer CRUD, the `useFor` gates and the cooldown view.

use crate::db::{IndexerRow, indexers};
use crate::error::{Error, R};
use crate::throttle;
use crate::torznab::{album_categories, pick_category};
use crate::util::{Settings, js_number, merge_secrets, redact_secrets, setting};
use fliks_plugin_sdk::bindings::fliks::plugin::types::{Family, Source};
use fliks_plugin_sdk::host;
use serde_json::{Map, Value, json};

const SECRET_KEYS: [&str; 1] = ["apiKey"];
pub const USE_FOR: [&str; 3] = ["rss", "auto", "manual"];

pub fn redact(ix: &IndexerRow) -> IndexerRow {
    IndexerRow {
        settings: redact_secrets(Some(&ix.settings), &SECRET_KEYS),
        ..ix.clone()
    }
}

pub fn use_for(ix: &IndexerRow) -> Vec<&'static str> {
    let mut v = vec![];
    if ix.enable_rss {
        v.push("rss");
    }
    if ix.enable_search {
        v.push("auto");
    }
    if ix.enable_interactive_search {
        v.push("manual");
    }
    v
}

#[derive(Default)]
pub struct Input {
    pub name: Option<String>,
    pub implementation: Option<String>,
    pub settings: Option<Settings>,
    pub enable_rss: Option<bool>,
    pub enable_search: Option<bool>,
    pub enable_interactive_search: Option<bool>,
    pub priority: Option<f64>,
    pub request_delay: Option<f64>,
    pub enabled: Option<bool>,
}

fn check_implementation(implementation: &str) -> R<()> {
    if implementation == "torznab" {
        Ok(())
    } else {
        Err(Error::UnknownImplementation(format!(
            "Unknown indexer implementation \"{implementation}\""
        )))
    }
}

fn sanitize(settings: Option<&Settings>) -> Settings {
    let mut out = settings.cloned().unwrap_or_default();
    if out.contains_key("minSeeders") {
        let n = js_number(out.get("minSeeders"));
        let n = if n.is_nan() { 0.0 } else { n.floor().max(0.0) };
        out.insert("minSeeders".into(), crate::util::num(n));
    }
    out
}

/// The caps probe runs on the task lane: the response must not wait for the tracker.
fn schedule_caps(id: i64) {
    if let Err(e) = fliks_plugin_sdk::scheduled::schedule(&format!("caps:{id}"), 0, &json!({})) {
        warn!("caps refresh not scheduled: {}", e.message);
    }
}

pub fn create(i: Input) -> R<IndexerRow> {
    let implementation = i.implementation.unwrap_or_default();
    check_implementation(&implementation)?;
    let saved = indexers::insert(&indexers::Fields {
        name: i.name.unwrap_or_default(),
        implementation,
        settings: merge_secrets(None, &sanitize(i.settings.as_ref()), &SECRET_KEYS),
        enable_rss: i.enable_rss.unwrap_or(true),
        enable_search: i.enable_search.unwrap_or(true),
        enable_interactive_search: i.enable_interactive_search.unwrap_or(true),
        priority: i.priority.unwrap_or(25.0),
        request_delay: i.request_delay.unwrap_or(2.0),
        enabled: i.enabled.unwrap_or(true),
    })?;
    schedule_caps(saved.id);
    publish_sources();
    Ok(redact(&saved))
}

pub fn find(id: i64) -> R<IndexerRow> {
    indexers::find(id)?.ok_or_else(|| Error::NotFound(format!("Indexer #{id} not found")))
}

pub fn update(id: i64, i: Input) -> R<IndexerRow> {
    let e = find(id)?;
    if let Some(imp) = &i.implementation {
        check_implementation(imp)?;
    }
    let settings = match &i.settings {
        Some(s) => merge_secrets(Some(&e.settings), &sanitize(Some(s)), &SECRET_KEYS),
        None => e.settings.clone(),
    };
    let saved = indexers::update(
        id,
        &indexers::Fields {
            name: i.name.unwrap_or(e.name),
            implementation: i.implementation.unwrap_or(e.implementation),
            settings,
            enable_rss: i.enable_rss.unwrap_or(e.enable_rss),
            enable_search: i.enable_search.unwrap_or(e.enable_search),
            enable_interactive_search: i
                .enable_interactive_search
                .unwrap_or(e.enable_interactive_search),
            priority: i.priority.unwrap_or(e.priority as f64),
            request_delay: i.request_delay.unwrap_or(e.request_delay as f64),
            enabled: i.enabled.unwrap_or(e.enabled),
        },
    )?;
    schedule_caps(saved.id);
    publish_sources();
    Ok(redact(&saved))
}

pub fn remove(id: i64) -> R<()> {
    find(id)?;
    indexers::remove(id)?;
    publish_sources();
    Ok(())
}

/// The media kinds an indexer's category tree serves; an unprobed tree (none) serves them all.
pub fn kinds(tree: Option<&str>) -> Vec<&'static str> {
    let has = |lo: u32, hi: u32| {
        tree.is_none_or(|t| {
            t.split(',')
                .filter_map(|id| id.parse::<u32>().ok())
                .any(|id| (lo..hi).contains(&id))
        })
    };
    let book =
        pick_category(tree, Family::Text).is_some() || pick_category(tree, Family::Audio).is_some();
    [
        ("movie", has(2000, 3000)),
        ("series", has(5000, 6000)),
        ("book", book),
        ("comic", pick_category(tree, Family::Image).is_some()),
        ("album", album_categories(tree).is_some()),
    ]
    .into_iter()
    .filter_map(|(k, on)| on.then_some(k))
    .collect()
}

/// The host refuses a list over this size, or a name outside 1-200 characters, as a whole.
const MAX_SOURCES: usize = 200;

/// One source per enabled, searchable indexer that serves at least one kind: the first 200, names clamped.
pub fn sources_of(rows: &[IndexerRow]) -> Vec<Source> {
    rows.iter()
        .filter(|ix| ix.enabled && (ix.enable_search || ix.enable_interactive_search))
        .filter_map(|ix| {
            let kinds = kinds(ix.caps_categories.as_deref());
            let name: String = ix.name.trim().chars().take(200).collect();
            (!kinds.is_empty()).then(|| Source {
                id: ix.id.to_string(),
                name: if name.is_empty() {
                    format!("Indexer {}", ix.id)
                } else {
                    name
                },
                kinds: kinds.into_iter().map(String::from).collect(),
            })
        })
        .take(MAX_SOURCES)
        .collect()
}

/// Tells the host which indexers this provider offers; a failure is logged, the next change retries.
pub fn publish_sources() {
    match indexers::list_all() {
        Ok(rows) => {
            if let Err(e) = host::acquisition_sources_set(&sources_of(&rows)) {
                warn!("acquisition sources not published: {}", e.message);
            }
        }
        Err(e) => warn!("acquisition sources not published: {e}"),
    }
}

/// Redacted rows with `useFor` and the open cooldown, in fan-out order.
pub fn list() -> R<Vec<Value>> {
    let rows = indexers::list_all()?;
    let cooldowns = throttle::cooldowns()?;
    Ok(rows
        .iter()
        .map(|ix| {
            let mut v = serde_json::to_value(redact(ix)).unwrap_or_default();
            v["useFor"] = json!(use_for(ix));
            v["cooldown"] =
                cooldowns
                    .iter()
                    .find(|c| c.indexer_id == ix.id)
                    .map_or(Value::Null, |c| {
                        let mut o = Map::new();
                        o.insert("reason".into(), json!(c.reason));
                        o.insert("remainingMs".into(), json!(c.remaining_ms));
                        o.insert("until".into(), json!(c.until));
                        if let Some(n) = c.failure_count {
                            o.insert("failureCount".into(), json!(n));
                        }
                        if let Some(d) = &c.detail {
                            o.insert("detail".into(), json!(d));
                        }
                        Value::Object(o)
                    });
            v
        })
        .collect())
}

pub fn clear_cooldown(id: i64) -> R<Value> {
    find(id)?;
    Ok(json!({ "cleared": throttle::clear(id)? }))
}

pub fn clear_all_cooldowns() -> R<Value> {
    Ok(json!({ "cleared": throttle::clear_all()? }))
}

/// A blank key on a saved row tests the stored one; a `null` tests without a key.
pub fn test_connection(implementation: &str, settings: &Settings, id: Option<i64>) -> Value {
    if implementation != "torznab" {
        return json!({ "ok": false, "messageKey": "download.indexers.test.unknown_implementation", "detail": implementation });
    }
    let base = setting(settings, "baseUrl").trim().to_owned();
    let key = if settings.get("apiKey") == Some(&Value::Null) {
        String::new()
    } else {
        let submitted = setting(settings, "apiKey").trim().to_owned();
        match id.filter(|_| submitted.is_empty()) {
            Some(id) => find(id).map_or(submitted, |ix| {
                setting(&ix.settings, "apiKey").trim().to_owned()
            }),
            None => submitted,
        }
    };
    crate::torznab::test_connection(&base, &key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_follow_the_category_tree() {
        assert_eq!(kinds(None), ["movie", "series", "book", "comic", "album"]);
        assert_eq!(kinds(Some("2000,2040,5000")), ["movie", "series"]);
        assert_eq!(kinds(Some("3010")), ["album"]);
        assert_eq!(kinds(Some("7020,7030")), ["book", "comic"]);
        assert!(kinds(Some("1000,4000")).is_empty());
    }
}
