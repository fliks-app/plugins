//! What the plugin touches outside itself, as one trait so the delivery logic runs natively.

use crate::{delivery, log_db};
use fliks_plugin_sdk::bindings::wasi::http::types::Method;
use fliks_plugin_sdk::http::{Error as HttpError, Request};
use fliks_plugin_sdk::{host, scheduled};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, PartialEq)]
pub enum ScheduleError {
    /// The host refused the call (`ERR_VALIDATION`) and said why.
    Rejected(String),
    Other(String),
}

pub struct Entry {
    pub id: i64,
    pub event: String,
    pub target_host: String,
    pub attempt: i64,
    pub last_status: String,
    pub created_at: Option<String>,
    pub last_attempt_at: Option<String>,
}

pub struct Stats {
    pub queued: i64,
    pub delivered: i64,
    pub failed: i64,
    pub dropped: i64,
}

/// Terminal transitions that also move a counter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Counter {
    Delivered,
    FailedPermanently,
}

pub trait Env {
    fn target(&self) -> Result<Option<String>, String>;
    fn insert(&self, event: &str, host: &str, state: &str, status: &str) -> Result<i64, String>;
    fn state(&self, id: i64) -> Result<Option<String>, String>;
    /// Conditional on the row still being live; true when it changed.
    fn finish(
        &self,
        id: i64,
        state: &str,
        status: &str,
        attempt: u32,
        counter: Option<Counter>,
    ) -> Result<bool, String>;
    fn drop_over_capacity(&self) -> Result<Vec<i64>, String>;
    fn prune(&self) -> Result<(), String>;
    fn sweep_stuck(&self) -> Result<(), String>;
    fn schedule(&self, key: &str, delay_ms: u64, payload: &Value) -> Result<(), ScheduleError>;
    fn cancel(&self, key: &str);
    fn warn(&self, message: &str);
    fn post(&self, url: &str, body: String) -> Result<u16, HttpError>;
    fn entries(&self) -> Result<Vec<Entry>, String>;
    fn stats(&self) -> Result<Stats, String>;
}

/// The wasm host: Postgres, the scheduler, config and the guarded HTTP client.
pub struct Live;

fn db(e: fliks_plugin_sdk::db::DbError) -> String {
    format!("{}: {}", e.code, e.message)
}

impl Env for Live {
    fn target(&self) -> Result<Option<String>, String> {
        let kv = host::config_get(Some(&["target_url".to_owned()]))
            .map_err(|e| format!("{}: {}", e.code, e.message))?;
        let v = kv
            .into_iter()
            .find(|(k, _)| k == "target_url")
            .map(|(_, v)| v);
        Ok(v.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()))
    }
    fn insert(&self, event: &str, host: &str, state: &str, status: &str) -> Result<i64, String> {
        log_db::insert(event, host, state, status).map_err(db)
    }
    fn state(&self, id: i64) -> Result<Option<String>, String> {
        log_db::state(id).map_err(db)
    }
    fn finish(
        &self,
        id: i64,
        state: &str,
        status: &str,
        attempt: u32,
        counter: Option<Counter>,
    ) -> Result<bool, String> {
        log_db::finish(id, state, status, attempt, counter).map_err(db)
    }
    fn drop_over_capacity(&self) -> Result<Vec<i64>, String> {
        log_db::drop_over_capacity().map_err(db)
    }
    fn prune(&self) -> Result<(), String> {
        log_db::prune().map(drop).map_err(db)
    }
    fn sweep_stuck(&self) -> Result<(), String> {
        log_db::sweep_stuck().map(drop).map_err(db)
    }
    fn schedule(&self, key: &str, delay_ms: u64, payload: &Value) -> Result<(), ScheduleError> {
        scheduled::schedule(key, delay_ms, payload).map_err(|e| match e.code.as_str() {
            "ERR_VALIDATION" => ScheduleError::Rejected(e.message),
            _ => ScheduleError::Other(e.message),
        })
    }
    fn cancel(&self, key: &str) {
        let _ = scheduled::cancel(key);
    }
    fn warn(&self, message: &str) {
        fliks_plugin_sdk::warn!("{message}");
    }
    fn post(&self, url: &str, body: String) -> Result<u16, HttpError> {
        Request::new(Method::Post, url)
            .header("content-type", "application/json")
            .header("user-agent", delivery::USER_AGENT)
            .body(body)
            .timeout(Duration::from_secs(5))
            .body_cap(0)
            .send()
            .map(|r| r.status)
    }
    fn entries(&self) -> Result<Vec<Entry>, String> {
        log_db::entries().map_err(db)
    }
    fn stats(&self) -> Result<Stats, String> {
        log_db::stats().map_err(db)
    }
}
