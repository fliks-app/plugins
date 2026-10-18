//! `fliks.notify`: forwards every domain event to the admin's HTTPS webhook, with retry.

mod delivery;
mod env;
mod log_db;
mod queue;
mod routes;
#[cfg(test)]
mod testing;

use env::{Env, Live};
use fliks_plugin_sdk::{HealthReport, HttpRequest, HttpResponse, Plugin, db, info, warn};

const MIGRATIONS: [(&str, &str); 1] = [(
    "0001_deliveries",
    include_str!("../migrations/0001_deliveries.sql"),
)];

struct Notify;

impl Plugin for Notify {
    fn start() -> Result<(), String> {
        db::migrate(&MIGRATIONS).map_err(|e| format!("{}: {}", e.code, e.message))?;
        Live.sweep_stuck()
    }

    fn health() -> HealthReport {
        match Live.stats() {
            Ok(s) => HealthReport {
                ok: true,
                detail: Some(format!(
                    "queued={} delivered={} failed={}",
                    s.queued, s.delivered, s.failed
                )),
            },
            Err(e) => HealthReport {
                ok: false,
                detail: Some(e),
            },
        }
    }

    fn event(name: String, payload: String) {
        let payload = serde_json::from_str(&payload).unwrap_or(serde_json::Value::String(payload));
        if let Err(e) = queue::enqueue(&Live, &name, payload) {
            warn!("event {name} not queued: {e}");
        }
    }

    fn scheduled(key: String, payload: String, _attempt: u32) -> Result<(), String> {
        if key.starts_with(queue::PREFIX) {
            return queue::attempt(&Live, &key, &payload);
        }
        warn!("scheduled key {key} dropped: unknown prefix");
        Ok(())
    }

    fn http(req: HttpRequest) -> Result<HttpResponse, String> {
        routes::handle(&Live, req)
    }

    fn config_changed(keys: Vec<String>) {
        info!("config changed: {}", keys.join(", "));
    }
}

fliks_plugin_sdk::export!(Notify);

#[cfg(test)]
mod manifest_tests {
    use serde_json::Value;

    fn template() -> Value {
        serde_json::from_str(include_str!("../plugin.template.json")).unwrap()
    }

    #[test]
    fn template_matches_what_the_crate_ships() {
        let t = template();
        assert_eq!(t["id"], "fliks.notify");
        assert_eq!(t["pluginApi"], 2);
        assert_eq!(t["runtime"], "wasm");
        assert_eq!(t["fliks"], ">=5.0.0 <6.0.0");
        assert_eq!(env!("CARGO_PKG_VERSION"), "1.0.0");
        let m = fliks_plugin_sdk::manifest_check(include_str!("../plugin.template.json"), "1.0.0")
            .unwrap();
        assert_eq!((m.id.as_str(), m.plugin_api), ("fliks.notify", 2));
    }

    #[test]
    fn every_route_policy_names_a_declared_permission() {
        let t = template();
        let perms: Vec<&str> = t["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect();
        for r in t["routes"].as_array().unwrap() {
            let policy = r["policy"].as_str().unwrap();
            assert!(policy.contains(":plugin:fliks.notify:"), "{policy}");
            assert!(
                perms.contains(&policy.rsplit(':').next().unwrap()),
                "{policy}"
            );
        }
    }

    #[test]
    fn every_ui_key_is_translated_in_every_locale() {
        let t = template();
        let ui = t["ui"].to_string();
        let used: Vec<&str> = ui.split('"').filter(|s| s.starts_with("notify.")).collect();
        assert_eq!(used.len(), 5);
        for (locale, dict) in t["i18n"].as_object().unwrap() {
            for k in &used {
                assert!(dict.get(k).is_some(), "{locale} lacks {k}");
            }
        }
    }

    #[test]
    fn target_url_is_the_one_secret_form_field() {
        let t = template();
        let fields = t["ui"]["configPages"][0]["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0]["key"], "target_url");
        assert_eq!(fields[0]["secret"], true);
    }
}
