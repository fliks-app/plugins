//! In-memory [`Env`] for the native tests.

use crate::env::{Counter, Entry, Env, ScheduleError, Stats};
use crate::queue::{self, key};
use fliks_plugin_sdk::http::Error as HttpError;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

pub(crate) struct Row {
    pub(crate) id: i64,
    pub(crate) event: String,
    pub(crate) host: String,
    pub(crate) state: String,
    pub(crate) status: String,
    pub(crate) attempt: u32,
    pub(crate) age_hours: u32,
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) target: Option<String>,
    pub(crate) target_error: Option<String>,
    pub(crate) rows: Vec<Row>,
    pub(crate) stats: [i64; 3], // delivered, failedPermanently, droppedForCapacity
    pub(crate) calls: BTreeMap<String, (u64, Value)>,
    pub(crate) cancelled: Vec<String>,
    pub(crate) posts: Vec<(String, String)>,
    pub(crate) replies: VecDeque<Result<u16, HttpError>>,
    pub(crate) warns: Vec<String>,
    pub(crate) drop_on_post: Option<i64>,
}

/// In-memory stand-in for the host that follows the same conditional-write rules as the SQL.
#[derive(Default)]
pub(crate) struct Fake(pub(crate) RefCell<State>);

pub(crate) const LIVE: [&str; 3] = ["pending", "retrying", "lost"];

impl Env for Fake {
    fn target(&self) -> Result<Option<String>, String> {
        let s = self.0.borrow();
        s.target_error.clone().map_or(Ok(s.target.clone()), Err)
    }
    fn insert(&self, event: &str, host: &str, state: &str, status: &str) -> Result<i64, String> {
        let mut s = self.0.borrow_mut();
        let id = s.rows.len() as i64 + 1;
        s.rows.push(Row {
            id,
            event: event.into(),
            host: host.into(),
            state: state.into(),
            status: status.into(),
            attempt: 0,
            age_hours: 0,
        });
        Ok(id)
    }
    fn state(&self, id: i64) -> Result<Option<String>, String> {
        let s = self.0.borrow();
        Ok(s.rows.iter().find(|r| r.id == id).map(|r| r.state.clone()))
    }
    fn finish(
        &self,
        id: i64,
        state: &str,
        status: &str,
        attempt: u32,
        counter: Option<Counter>,
    ) -> Result<bool, String> {
        let mut s = self.0.borrow_mut();
        let Some(r) = s
            .rows
            .iter_mut()
            .find(|r| r.id == id && LIVE.contains(&r.state.as_str()))
        else {
            return Ok(false);
        };
        (r.state, r.status, r.attempt) = (state.into(), status.into(), attempt);
        match counter {
            Some(Counter::Delivered) => s.stats[0] += 1,
            Some(Counter::FailedPermanently) => s.stats[1] += 1,
            None => {}
        }
        Ok(true)
    }
    fn drop_over_capacity(&self) -> Result<Vec<i64>, String> {
        let mut s = self.0.borrow_mut();
        let mut ids: Vec<i64> = s
            .rows
            .iter()
            .filter(|r| r.state == "pending" || r.state == "retrying")
            .map(|r| r.id)
            .collect();
        ids.sort_by(|a, b| b.cmp(a));
        let over: Vec<i64> = ids.into_iter().skip(200).collect();
        for r in s.rows.iter_mut().filter(|r| over.contains(&r.id)) {
            (r.state, r.status) = ("dropped".into(), "dropped:queue full".into());
        }
        s.stats[2] += over.len() as i64;
        Ok(over)
    }
    fn prune(&self) -> Result<(), String> {
        Ok(())
    }
    fn sweep_stuck(&self) -> Result<(), String> {
        let mut s = self.0.borrow_mut();
        for r in s
            .rows
            .iter_mut()
            .filter(|r| (r.state == "pending" || r.state == "retrying") && r.age_hours >= 3)
        {
            (r.state, r.status) = ("lost".into(), "failed:lost".into());
        }
        Ok(())
    }
    fn schedule(&self, key: &str, delay_ms: u64, payload: &Value) -> Result<(), ScheduleError> {
        if payload.to_string().len() > 64 * 1024 {
            return Err(ScheduleError::Rejected("payload exceeds 64 KiB".into()));
        }
        self.0
            .borrow_mut()
            .calls
            .insert(key.into(), (delay_ms, payload.clone()));
        Ok(())
    }
    fn cancel(&self, key: &str) {
        let mut s = self.0.borrow_mut();
        s.calls.remove(key);
        s.cancelled.push(key.into());
    }
    fn warn(&self, message: &str) {
        self.0.borrow_mut().warns.push(message.into());
    }
    fn post(&self, url: &str, body: String) -> Result<u16, HttpError> {
        let mut s = self.0.borrow_mut();
        s.posts.push((url.into(), body));
        if let Some(id) = s.drop_on_post.take() {
            let r = s.rows.iter_mut().find(|r| r.id == id).unwrap();
            (r.state, r.status) = ("dropped".into(), "dropped:queue full".into());
        }
        s.replies.pop_front().expect("unexpected POST")
    }
    fn entries(&self) -> Result<Vec<Entry>, String> {
        let s = self.0.borrow();
        Ok(s.rows
            .iter()
            .rev()
            .take(100)
            .map(|r| Entry {
                id: r.id,
                event: r.event.clone(),
                target_host: r.host.clone(),
                attempt: i64::from(r.attempt),
                last_status: r.status.clone(),
                created_at: Some("2026-01-01T00:00:00.000Z".into()),
                last_attempt_at: (r.attempt > 0).then(|| "2026-01-01T00:00:01.000Z".into()),
            })
            .collect())
    }
    fn stats(&self) -> Result<Stats, String> {
        let s = self.0.borrow();
        let queued = s
            .rows
            .iter()
            .filter(|r| r.state == "pending" || r.state == "retrying")
            .count();
        Ok(Stats {
            queued: queued as i64,
            delivered: s.stats[0],
            failed: s.stats[1],
            dropped: s.stats[2],
        })
    }
}

impl Fake {
    pub(crate) fn with_target(url: &str) -> Fake {
        let f = Fake::default();
        f.0.borrow_mut().target = Some(url.into());
        f
    }
    pub(crate) fn reply(&self, r: Result<u16, HttpError>) {
        self.0.borrow_mut().replies.push_back(r);
    }
    pub(crate) fn row(&self, id: i64) -> (String, String, u32) {
        let s = self.0.borrow();
        let r = s.rows.iter().find(|r| r.id == id).unwrap();
        (r.state.clone(), r.status.clone(), r.attempt)
    }
    /// Runs the pending call of `id` the way the host would, returning its delay.
    pub(crate) fn fire(&self, id: i64) -> Option<u64> {
        let k = key(id);
        let (delay, payload) = self.0.borrow_mut().calls.remove(&k)?;
        queue::attempt(self, &k, &payload.to_string()).unwrap();
        Some(delay)
    }
    pub(crate) fn posts(&self) -> usize {
        self.0.borrow().posts.len()
    }
    pub(crate) fn live_count(&self) -> usize {
        let s = self.0.borrow();
        s.rows
            .iter()
            .filter(|r| r.state == "pending" || r.state == "retrying")
            .count()
    }
}
