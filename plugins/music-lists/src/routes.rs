//! The list settings page (`providers` kind), the manual sync and Spotify's callback.
use crate::model::{Settings, text};
use crate::net::Live as Web;
use crate::{db, run, spotify};
use fliks_plugin_sdk::router::{Request, Response, Router};
use serde_json::{Map, Value, json};

type H = Result<Response, String>;

const SECRET_KEYS: [&str; 3] = ["token", "apiKey", "clientSecret"];
const SECRETS_SET: &str = "secretsSet";

fn truthy(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
}

/// The credentials leave the read response; `secretsSet` names the ones that hold a value.
pub fn redact(settings: &Settings) -> Settings {
    let mut out = settings.clone();
    let mut set = vec![];
    for k in SECRET_KEYS {
        if truthy(out.get(k)) {
            set.push(Value::from(k));
        }
        out.shift_remove(k);
    }
    out.insert(SECRETS_SET.into(), Value::Array(set));
    out
}

/// A blank credential keeps the stored one; an explicit `null` erases it.
pub fn merge(existing: Option<&Settings>, incoming: &Settings) -> Settings {
    let mut out = incoming.clone();
    out.shift_remove(SECRETS_SET);
    for k in SECRET_KEYS {
        if out.get(k) == Some(&Value::Null) {
            out.shift_remove(k);
        } else if !truthy(out.get(k)) {
            match existing.and_then(|e| e.get(k)).filter(|v| truthy(Some(v))) {
                Some(v) => {
                    out.insert(k.into(), v.clone());
                }
                None => {
                    out.shift_remove(k);
                }
            }
        }
    }
    out
}

fn wire(r: &db::Row) -> Value {
    let connected = r.implementation == "spotify"
        && db::secrets(r.id).is_ok_and(|s| truthy(s.get("refreshToken")));
    json!({
        "id": r.id, "name": r.name, "implementation": r.implementation,
        "settings": redact(&r.settings), "enabled": r.enabled,
        "lastSyncAt": r.last_sync_at, "lastError": r.last_error, "lastSummary": r.last_summary,
        "connected": connected,
    })
}

fn bad(field: &str) -> H {
    Ok(Response::json(
        400,
        &json!({ "error": format!("invalid {field}") }),
    ))
}

fn id_of(r: &Request) -> Option<i64> {
    r.param("id")?.parse().ok().filter(|i| *i > 0)
}

fn body(r: &Request) -> Map<String, Value> {
    r.json().unwrap_or_default()
}

fn list_all(_: &Request) -> H {
    let rows = db::list()?;
    Ok(Response::json(
        200,
        &rows.iter().map(wire).collect::<Vec<_>>(),
    ))
}

fn settings_of(b: &Map<String, Value>) -> Settings {
    b.get("settings")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn create(r: &Request) -> H {
    let b = body(r);
    let (name, imp) = (text(&b, "name"), text(&b, "implementation"));
    if name.is_empty() {
        return bad("name");
    }
    if !IMPLEMENTATIONS.contains(&imp.as_str()) {
        return bad("implementation");
    }
    let enabled = b.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let id = db::insert(
        &name,
        &imp,
        &merge(None, &settings_of(&b)),
        enabled,
        r.user(),
    )?;
    let row = db::find(id)?.ok_or("lost")?;
    Ok(Response::json(201, &wire(&row)))
}

fn update(r: &Request) -> H {
    let Some(id) = id_of(r) else { return bad("id") };
    let Some(old) = db::find(id)? else {
        return Ok(Response::json(404, &json!({ "error": "not found" })));
    };
    let b = body(r);
    let name = Some(text(&b, "name"))
        .filter(|n| !n.is_empty())
        .unwrap_or(old.name.clone());
    let enabled = b
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(old.enabled);
    let settings = merge(Some(&old.settings), &settings_of(&b));
    db::update(id, &name, &settings, enabled, r.user())?;
    let row = db::find(id)?.ok_or("lost")?;
    Ok(Response::json(200, &wire(&row)))
}

fn remove(r: &Request) -> H {
    let Some(id) = id_of(r) else { return bad("id") };
    db::remove(id)?;
    Ok(Response::json(200, &json!({})))
}

fn sync_now(r: &Request) -> H {
    let Some(id) = id_of(r) else { return bad("id") };
    let Some(row) = db::find(id)? else {
        return Ok(Response::json(404, &json!({ "error": "not found" })));
    };
    match run::sync_list(&row) {
        Ok(s) => Ok(Response::json(200, &run::summary_json(&s))),
        Err(e) => Ok(Response::json(502, &json!({ "error": e }))),
    }
}

fn test_connection(r: &Request) -> H {
    let b = body(r);
    let imp = text(&b, "implementation");
    let existing = b
        .get("id")
        .and_then(Value::as_i64)
        .and_then(|i| db::find(i).ok().flatten());
    let settings = merge(existing.as_ref().map(|e| &e.settings), &settings_of(&b));
    let checked = if imp == "spotify" {
        existing
            .as_ref()
            .ok_or_else(|| "save the list, then connect Spotify".to_owned())
            .and_then(|e| run::spotify_access(e.id, &settings).map(|_| 0))
    } else {
        crate::sources::fetch(&Web, &imp, &settings, None).map(|e| e.len())
    };
    Ok(Response::json(
        200,
        &match checked {
            Ok(n) => {
                json!({ "ok": true, "messageKey": "musiclists.test.ok", "detail": n.to_string() })
            }
            Err(e) => json!({ "ok": false, "messageKey": "musiclists.test.failed", "detail": e }),
        },
    ))
}

fn state_token() -> String {
    use fliks_plugin_sdk::bindings::wasi::random::random::get_random_bytes;
    get_random_bytes(16)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn authorize(r: &Request) -> H {
    let Some(id) = id_of(r) else { return bad("id") };
    let Some(row) = db::find(id)?.filter(|r| r.implementation == "spotify") else {
        return Ok(Response::json(404, &json!({ "error": "not found" })));
    };
    let state = state_token();
    let url = match spotify::authorize_url(&row.settings, &state) {
        Ok(u) => u,
        Err(e) => return Ok(Response::json(400, &json!({ "error": e }))),
    };
    db::new_state(id, &state)?;
    Ok(Response::empty(302).with_header("location", &url))
}

fn page(status: u16, message: &str) -> Response {
    let html = format!("<!doctype html><meta charset=utf-8><title>Spotify</title><p>{message}</p>");
    Response::empty(status)
        .with_header("content-type", "text/html; charset=utf-8")
        .with_body(html)
}

fn callback(r: &Request) -> H {
    let (Some(code), Some(state)) = (r.query("code"), r.query("state")) else {
        return Ok(page(400, "Spotify refused the connection."));
    };
    let Some(id) = db::take_state(state)? else {
        return Ok(page(
            400,
            "This connection link has expired. Start again from the settings.",
        ));
    };
    let Some(row) = db::find(id)? else {
        return Ok(page(404, "The list no longer exists."));
    };
    match spotify::exchange(&Web, &row.settings, code) {
        Ok(token) => {
            let mut data = db::secrets(id)?;
            data.insert("refreshToken".into(), Value::String(token));
            db::set_secrets(id, &data)?;
            Ok(page(200, "Spotify is connected. You can close this tab."))
        }
        Err(e) => Ok(page(502, &format!("Spotify answered an error: {e}"))),
    }
}

pub const IMPLEMENTATIONS: [&str; 5] =
    ["listenbrainz", "lastfm", "spotify", "deezer", "musicbrainz"];

fn field(key: &str, ty: &str, required: bool, secret: bool) -> Value {
    let mut f = json!({ "key": key, "type": ty, "labelKey": format!("musiclists.fields.{key}") });
    if required {
        f["required"] = true.into();
    }
    if secret {
        f["secret"] = true.into();
    }
    if matches!(
        key,
        "redirectUri" | "playlistId" | "collectionId" | "token" | "id" | "libraryId"
    ) {
        f["hint"] = format!("musiclists.fields.{key}_hint").into();
    }
    f
}

fn select(key: &str, options: &[&str]) -> Value {
    let opts: Vec<Value> = options
        .iter()
        .map(|o| json!({ "value": o, "labelKey": format!("musiclists.sources.{o}") }))
        .collect();
    let mut f = field(key, "select", true, false);
    f["options"] = opts.into();
    f["default"] = options[0].into();
    f
}

fn common() -> Vec<Value> {
    let acts = ["albums", "tracks", "playlist"]
        .map(|a| json!({ "value": a, "labelKey": format!("musiclists.actions.{a}") }));
    let mut a = field("actions", "multiselect", true, false);
    a["options"] = acts.to_vec().into();
    a["default"] = json!(["albums"]);
    a["hint"] = "musiclists.fields.actions_hint".into();
    vec![a, field("libraryId", "number", false, false)]
}

pub fn implementations() -> Value {
    let def = |imp: &str, mut fields: Vec<Value>| {
        fields.extend(common());
        json!({ "implementation": imp, "labelKey": format!("musiclists.implementations.{imp}"), "fields": fields })
    };
    json!([
        def(
            "listenbrainz",
            vec![
                field("username", "text", true, false),
                field("token", "password", false, true),
                select(
                    "source",
                    &[
                        "playlist",
                        "created-for-you",
                        "top-albums",
                        "top-tracks",
                        "loved-tracks"
                    ]
                ),
                field("playlistId", "text", false, false),
            ]
        ),
        def(
            "lastfm",
            vec![
                field("username", "text", true, false),
                field("apiKey", "password", true, true),
                select("source", &["loved-tracks", "top-tracks"]),
            ]
        ),
        def(
            "spotify",
            vec![
                field("clientId", "text", true, false),
                field("clientSecret", "password", true, true),
                field("redirectUri", "url", true, false),
                select("source", &["playlist", "saved-albums", "saved-tracks"]),
                field("playlistId", "text", false, false),
            ]
        ),
        def(
            "deezer",
            vec![
                select("source", &["playlist", "user-albums"]),
                field("id", "text", true, false),
            ]
        ),
        def(
            "musicbrainz",
            vec![
                field("collectionId", "text", true, false),
                select("entity", &["release-group", "recording"]),
            ]
        ),
    ])
}

fn impls(_: &Request) -> H {
    Ok(Response::json(200, &implementations()))
}

pub fn router() -> Router {
    Router::new()
        .route("GET", "/lists", list_all)
        .route("POST", "/lists", create)
        .route("GET", "/lists/implementations", impls)
        .route("POST", "/lists/test-connection", test_connection)
        .route("PUT", "/lists/:id", update)
        .route("DELETE", "/lists/:id", remove)
        .route("POST", "/lists/:id/sync", sync_now)
        .route("GET", "/spotify/callback", callback)
        .route("GET", "/spotify/:id/authorize", authorize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: Value) -> Settings {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn credentials_are_redacted_and_kept_on_a_blank_save() {
        let stored = s(json!({"username": "u", "token": "t", "apiKey": ""}));
        let r = redact(&stored);
        assert!(r.get("token").is_none());
        assert_eq!(r[SECRETS_SET], json!(["token"]));
        let blank = s(json!({"username": "v", "token": "", SECRETS_SET: ["token"]}));
        let merged = merge(Some(&stored), &blank);
        assert_eq!(merged["token"], "t");
        assert!(merged.get(SECRETS_SET).is_none());
        let erased = merge(Some(&stored), &s(json!({"token": null})));
        assert!(erased.get("token").is_none());
        assert_eq!(
            merge(Some(&stored), &s(json!({"token": "n"})))["token"],
            "n"
        );
    }

    #[test]
    fn every_implementation_is_declared_with_its_fields() {
        let v = implementations();
        let names: Vec<_> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["implementation"].as_str().unwrap())
            .collect();
        assert_eq!(names, IMPLEMENTATIONS);
        for i in v.as_array().unwrap() {
            let keys: Vec<_> = i["fields"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f["key"].as_str().unwrap())
                .collect();
            assert!(keys.contains(&"actions") && keys.contains(&"libraryId"));
        }
    }

    #[test]
    fn every_key_the_forms_use_is_translated() {
        let t: Value = serde_json::from_str(include_str!("../plugin.template.json")).unwrap();
        let mut used = vec![];
        let mut walk = vec![implementations(), t["ui"].clone()];
        while let Some(v) = walk.pop() {
            match v {
                Value::Object(o) => {
                    o.into_iter()
                        .for_each(|(k, v)| match (&v, k.ends_with("Key") || k == "hint") {
                            (Value::String(s), true) => used.push(s.clone()),
                            _ => walk.push(v),
                        })
                }
                Value::Array(a) => walk.extend(a),
                _ => {}
            }
        }
        assert!(used.len() > 40);
        for k in used {
            for l in ["en", "fr"] {
                assert!(t["i18n"][l][&k].is_string(), "{l} lacks {k}");
            }
        }
    }
}
