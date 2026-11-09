//! `fliks.download` as a `fliks:plugin@3.0.0` component: indexer search, download clients and
//! the acquisition grab pipeline.

#[macro_use]
extern crate fliks_plugin_sdk;

pub mod client;
pub mod config;
pub mod db;
pub mod error;
pub mod grab;
pub mod indexers;
pub mod net;
pub mod poller;
pub mod qbit;
pub mod routes;
pub mod sources;
pub mod throttle;
pub mod torznab;
pub mod transmission;
pub mod util;

use fliks_plugin_sdk::rt::{self, Budget, budgets};
use fliks_plugin_sdk::{HealthReport, HttpRequest, HttpResponse, Plugin, host};
use serde_json::{Value, json};
use std::time::Duration;

/// The guest budget of the export just entered, never past the host's own deadline.
fn enter(budget: Duration) {
    net::set_budget(Budget::new(Budget::host().cap(budget)));
}

type Job = fn(Option<&str>) -> error::R<()>;

/// The handler of a manifest job.
pub fn job_handler(name: &str) -> Option<Job> {
    Some(match name {
        "RssSync" => |_| grab::rss_sync(),
        "ImportCompleted" => |_| poller::poll(),
        "CleanStalled" => |_| poller::clean_stalled(),
        "CleanSeeded" => |_| poller::clean_seeded(),
        _ => return None,
    })
}

struct Download;

fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl Plugin for Download {
    fn health() -> HealthReport {
        match fliks_plugin_sdk::db::query("SELECT 1 AS one", &[]) {
            Ok(_) => HealthReport {
                ok: true,
                detail: None,
            },
            Err(e) => HealthReport {
                ok: false,
                detail: Some(e.message),
            },
        }
    }

    fn start() -> Result<(), String> {
        let applied = fliks_plugin_sdk::db::migrate(&db::MIGRATIONS)
            .map_err(|e| format!("{}: {}", e.code, e.message))?;
        if !applied.is_empty() {
            info!("migrations applied: {}", applied.join(", "));
        }
        let swept = db::history::sweep_import_leases().map_err(text)?;
        if swept > 0 {
            warn!("Import: {swept} importing entries outlived their lease — re-armed as grabbed");
        }
        indexers::publish_sources();
        // Viewers may hold a snapshot from before the restart: the next tick states every set.
        db::state::set("progressResync", &json!(true)).map_err(text)
    }

    fn job(name: String, _job_id: String, args: Option<String>) -> Result<(), String> {
        enter(budgets::JOB);
        let h = job_handler(&name)
            .ok_or_else(|| format!("no handler registered for job \"{name}\""))?;
        h(args.as_deref()).map_err(text)
    }

    fn scheduled(key: String, payload: String, _attempt: u32) -> Result<(), String> {
        enter(budgets::SCHEDULED);
        let payload: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
        let (prefix, rest) = key.split_once(':').unwrap_or((&key, ""));
        let id: Option<u32> = rest.parse().ok();
        match (prefix, id) {
            ("caps", Some(id)) => {
                let Some(mut ix) = db::indexers::find(id.into()).map_err(text)? else {
                    return Ok(());
                };
                let round = torznab::Round {
                    budget: net::budget(),
                    fetch_timeout: Duration::from_secs(10),
                };
                rt::block_on(torznab::refresh_caps(&mut ix, round))
                    .map_err(|_| format!("[{}] caps probe: the slot stayed busy", ix.name))?;
                indexers::publish_sources();
                Ok(())
            }
            ("grab-started", Some(_)) => {
                if let Err(e) = host::notifications_dispatch_grab_started(&payload.to_string()) {
                    warn!("AutoGrab: notifications.dispatch failed: {}", e.message);
                }
                Ok(())
            }
            _ => {
                warn!("scheduled call \"{key}\" has no handler, dropped");
                Ok(())
            }
        }
    }

    fn http(req: HttpRequest) -> Result<HttpResponse, String> {
        enter(budgets::HTTP);
        routes::ROUTER.handle(req)
    }

    fn config_changed(keys: Vec<String>) {
        info!("config changed: {} (read on next use)", keys.join(", "));
    }
}

fliks_plugin_sdk::export!(Download);
