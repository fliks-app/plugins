//! Enqueue and the scheduled delivery handler, over [`Env`].

use crate::delivery::{self, Action, Outcome};
use crate::env::{Counter, Env, ScheduleError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PREFIX: &str = "delivery:";

#[derive(Serialize, Deserialize)]
struct Call {
    event: String,
    #[serde(default)]
    payload: Value,
    /// Completed attempts; the host's own `attempt` also counts re-fires after a trap.
    #[serde(default)]
    n: u32,
}

pub(crate) fn key(id: i64) -> String {
    format!("{PREFIX}{id}")
}

pub fn enqueue(env: &impl Env, event: &str, payload: Value) -> Result<i64, String> {
    let _ = env.sweep_stuck();
    let target = env.target()?;
    let host = delivery::host_of(target.as_deref());
    if target.is_none() {
        let id = env.insert(event, &host, "refused", "refused:no target configured")?;
        env.warn(&format!("delivery {id} refused: no target configured"));
        return Ok(id);
    }
    let id = env.insert(event, &host, "pending", "pending")?;
    let dropped = env.drop_over_capacity()?;
    for d in &dropped {
        env.cancel(&key(*d));
    }
    if !dropped.is_empty() {
        env.warn(&format!(
            "queue full: dropped {} oldest deliveries",
            dropped.len()
        ));
    }
    let call = Call {
        event: event.into(),
        payload,
        n: 0,
    };
    match env.schedule(
        &key(id),
        0,
        &serde_json::to_value(&call).map_err(|e| e.to_string())?,
    ) {
        Ok(()) => {}
        Err(ScheduleError::Rejected(why)) => {
            env.finish(id, "dropped", &format!("dropped:{why}"), 0, None)?;
        }
        Err(ScheduleError::Other(e)) => return Err(e),
    }
    let _ = env.prune();
    Ok(id)
}

pub fn attempt(env: &impl Env, key: &str, payload: &str) -> Result<(), String> {
    let Some(id) = key.strip_prefix(PREFIX).and_then(|i| i.parse::<i64>().ok()) else {
        env.warn(&format!("scheduled key {key} dropped: bad id"));
        return Ok(());
    };
    // A missing or finished row means the call is stale (capacity drop, re-fire after the outcome).
    if !matches!(
        env.state(id)?.as_deref(),
        Some("pending" | "retrying" | "lost")
    ) {
        return Ok(());
    }
    let Ok(call) = serde_json::from_str::<Call>(payload) else {
        env.warn(&format!("scheduled key {key} dropped: unreadable payload"));
        return Ok(());
    };
    let n = call.n + 1;
    let outcome = match env.target() {
        Err(e) => Outcome::Failed(format!("config unavailable: {e}")),
        Ok(None) => Outcome::Refused("no target configured".into()),
        Ok(Some(t)) => match delivery::parse_target(&t) {
            Err(refused) => refused,
            Ok(url) => delivery::classify(env.post(
                url.as_str(),
                delivery::body(id, &call.event, &call.payload, n),
            )),
        },
    };
    let (state, status, counter, retry) = match delivery::next(n, outcome) {
        Action::Delivered { status } => ("delivered", status, Some(Counter::Delivered), None),
        Action::Refused { status } => ("refused", status, None, None),
        Action::Retry { status, delay_ms } => ("retrying", status, None, Some(delay_ms)),
        Action::Failed { status } => ("failed", status, Some(Counter::FailedPermanently), None),
    };
    // Conditional: a row dropped meanwhile is neither overwritten nor re-scheduled.
    let live = env.finish(id, state, &status, n, counter)?;
    if let (true, Some(delay)) = (live, retry) {
        let next = serde_json::to_value(Call { n, ..call }).map_err(|e| e.to_string())?;
        env.schedule(key, delay, &next)
            .map_err(|e| format!("{e:?}"))?;
    }
    if live && state == "failed" {
        env.warn(&format!("delivery {id} failed after {n} attempts"));
    }
    if live && state == "refused" {
        env.warn(&format!("delivery {id} {status}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Fake;
    use fliks_plugin_sdk::http::Error as HttpError;
    use serde_json::json;

    const URL: &str = "https://hooks.example.com/x?t=secret";

    #[test]
    fn no_target_is_logged_and_never_queued() {
        let f = Fake::default();
        let id = enqueue(&f, "e", json!({})).unwrap();
        assert_eq!(
            f.row(id),
            ("refused".into(), "refused:no target configured".into(), 0)
        );
        assert!(f.0.borrow().calls.is_empty());
        assert_eq!(f.0.borrow().rows[0].host, "unconfigured");
    }

    #[test]
    fn enqueue_schedules_one_call_per_delivery_without_the_url() {
        let f = Fake::with_target(URL);
        let a = enqueue(&f, "media.imported", json!({"k": 1})).unwrap();
        let b = enqueue(&f, "e2", json!(null)).unwrap();
        let s = f.0.borrow();
        assert_eq!(s.calls.len(), 2);
        let (delay, payload) = &s.calls[&format!("delivery:{a}")];
        assert_eq!(*delay, 0);
        assert_eq!(
            payload,
            &json!({"event": "media.imported", "payload": {"k": 1}, "n": 0})
        );
        assert!(s.calls.contains_key(&format!("delivery:{b}")));
        assert!(!payload.to_string().contains("secret"));
        assert_eq!(s.rows[0].host, "hooks.example.com");
        assert_eq!(s.rows[0].event, "media.imported");
    }

    #[test]
    fn fails_five_times_then_delivers_on_the_sixth_attempt() {
        let f = Fake::with_target(URL);
        let id = enqueue(&f, "e", json!({"a": 1})).unwrap();
        for _ in 0..5 {
            f.reply(Ok(500));
        }
        f.reply(Ok(204));
        let delays: Vec<u64> = std::iter::from_fn(|| f.fire(id)).collect();
        assert_eq!(delays, [0, 1000, 5000, 15000, 30000, 60000]);
        assert_eq!(f.row(id), ("delivered".into(), "delivered:204".into(), 6));
        assert_eq!(f.0.borrow().stats, [1, 0, 0]);
        let s = f.0.borrow();
        let attempts: Vec<i64> = s
            .posts
            .iter()
            .map(|(_, b)| {
                serde_json::from_str::<Value>(b).unwrap()["attempt"]
                    .as_i64()
                    .unwrap()
            })
            .collect();
        assert_eq!(attempts, [1, 2, 3, 4, 5, 6]);
        assert!(s.posts.iter().all(|(u, _)| u == URL));
    }

    #[test]
    fn six_failures_end_failed_and_count_once() {
        let f = Fake::with_target(URL);
        let id = enqueue(&f, "e", json!({})).unwrap();
        for _ in 0..6 {
            f.reply(Err(HttpError::Timeout));
        }
        let delays: Vec<u64> = std::iter::from_fn(|| f.fire(id)).collect();
        assert_eq!(delays.len(), 6);
        assert_eq!(f.row(id), ("failed".into(), "failed:timeout".into(), 6));
        assert_eq!(f.0.borrow().stats, [0, 1, 0]);
        assert!(f.0.borrow().calls.is_empty());
        assert!(
            f.0.borrow()
                .warns
                .iter()
                .any(|w| w.contains("failed after 6"))
        );
    }

    #[test]
    fn refusals_are_final_without_retry() {
        for (target, reply, want) in [
            (
                "http://hooks.example.com/x",
                None,
                "refused:target must be https:",
            ),
            ("::", None, "refused:not a valid URL"),
            (
                URL,
                Some(HttpError::Denied),
                "refused:target address is not allowed",
            ),
            (
                URL,
                Some(HttpError::Dns("nx".into())),
                "refused:DNS lookup failed: nx",
            ),
        ] {
            let f = Fake::with_target(target);
            let id = enqueue(&f, "e", json!({})).unwrap();
            let posted = reply.is_some();
            if let Some(e) = reply {
                f.reply(Err(e));
            }
            f.fire(id);
            assert_eq!(f.row(id), ("refused".into(), want.into(), 1));
            assert_eq!(f.0.borrow().warns, [format!("delivery {id} {want}")]);
            assert!(f.0.borrow().calls.is_empty(), "{want}");
            assert_eq!(f.posts(), usize::from(posted), "{want}");
        }
    }

    #[test]
    fn config_is_read_at_each_attempt() {
        let f = Fake::with_target("https://one.example.com/");
        let id = enqueue(&f, "e", json!({})).unwrap();
        f.reply(Ok(500));
        f.reply(Ok(200));
        f.fire(id);
        f.0.borrow_mut().target = Some("https://two.example.com/".into());
        f.fire(id);
        let s = f.0.borrow();
        assert_eq!(s.posts[0].0, "https://one.example.com/");
        assert_eq!(s.posts[1].0, "https://two.example.com/");
    }

    #[test]
    fn target_removed_between_attempts_is_refused() {
        let f = Fake::with_target(URL);
        let id = enqueue(&f, "e", json!({})).unwrap();
        f.reply(Ok(500));
        f.fire(id);
        f.0.borrow_mut().target = None;
        f.fire(id);
        assert_eq!(f.row(id).1, "refused:no target configured");
    }

    #[test]
    fn a_config_error_at_attempt_time_retries_without_leaking_the_url() {
        let f = Fake::with_target(URL);
        let id = enqueue(&f, "e", json!({})).unwrap();
        f.0.borrow_mut().target_error = Some("ERR_INTERNAL: db down".into());
        f.fire(id);
        assert_eq!(f.posts(), 0);
        assert_eq!(
            f.row(id).1,
            "retrying:config unavailable: ERR_INTERNAL: db down"
        );
        assert!(f.0.borrow().calls.contains_key(&key(id)));
        f.0.borrow_mut().target_error = None;
        f.reply(Ok(200));
        f.fire(id);
        assert_eq!(f.row(id).0, "delivered");
    }

    #[test]
    fn a_config_error_at_enqueue_is_an_error_not_a_refusal() {
        let f = Fake::default();
        f.0.borrow_mut().target_error = Some("boom".into());
        assert!(enqueue(&f, "e", json!({})).is_err());
        assert!(f.0.borrow().rows.is_empty());
    }

    #[test]
    fn the_201st_live_delivery_drops_the_oldest_and_cancels_its_call() {
        let f = Fake::with_target(URL);
        for _ in 0..200 {
            enqueue(&f, "e", json!({})).unwrap();
        }
        assert_eq!(f.live_count(), 200);
        assert!(f.0.borrow().cancelled.is_empty());
        enqueue(&f, "e", json!({})).unwrap();
        assert_eq!(f.live_count(), 200);
        assert_eq!(f.row(1), ("dropped".into(), "dropped:queue full".into(), 0));
        assert_eq!(f.0.borrow().cancelled, ["delivery:1"]);
        assert!(!f.0.borrow().calls.contains_key("delivery:1"));
        assert_eq!(f.0.borrow().stats, [0, 0, 1]);
    }

    #[test]
    fn a_payload_over_64_kib_is_dropped() {
        let f = Fake::with_target(URL);
        let id = enqueue(&f, "e", json!("x".repeat(70_000))).unwrap();
        assert_eq!(
            f.row(id),
            ("dropped".into(), "dropped:payload exceeds 64 KiB".into(), 0)
        );
        assert!(f.0.borrow().calls.is_empty());
    }

    #[test]
    fn a_stale_call_does_no_work() {
        let f = Fake::with_target(URL);
        let id = enqueue(&f, "e", json!({})).unwrap();
        f.reply(Ok(200));
        let payload = f.0.borrow().calls[&key(id)].1.to_string();
        f.fire(id);
        // Re-fired after the outcome was written, and for a row that never existed.
        attempt(&f, &key(id), &payload).unwrap();
        attempt(&f, "delivery:99", &payload).unwrap();
        assert_eq!(f.posts(), 1);
        assert_eq!(f.0.borrow().stats, [1, 0, 0]);
    }

    #[test]
    fn a_row_dropped_mid_attempt_is_neither_overwritten_nor_rescheduled() {
        let f = Fake::with_target(URL);
        let id = enqueue(&f, "e", json!({})).unwrap();
        f.reply(Ok(500));
        f.0.borrow_mut().drop_on_post = Some(id);
        f.fire(id);
        assert_eq!(
            f.row(id),
            ("dropped".into(), "dropped:queue full".into(), 0)
        );
        assert!(f.0.borrow().calls.is_empty());
        assert_eq!(f.0.borrow().stats, [0, 0, 0]);

        // Same for a success: the counter moves only with the row.
        let id = enqueue(&f, "e", json!({})).unwrap();
        f.reply(Ok(200));
        f.0.borrow_mut().drop_on_post = Some(id);
        f.fire(id);
        assert_eq!(f.row(id).0, "dropped");
        assert_eq!(f.0.borrow().stats, [0, 0, 0]);
    }

    #[test]
    fn stuck_rows_become_lost_and_a_late_call_still_delivers_them() {
        let f = Fake::with_target(URL);
        let old = enqueue(&f, "e", json!({})).unwrap();
        f.0.borrow_mut().rows[0].age_hours = 4;
        let fresh = enqueue(&f, "e", json!({})).unwrap();
        assert_eq!(f.row(old), ("lost".into(), "failed:lost".into(), 0));
        assert_eq!(f.row(fresh).0, "pending");
        f.reply(Ok(200));
        f.fire(old);
        assert_eq!(f.row(old), ("delivered".into(), "delivered:200".into(), 1));
        assert_eq!(f.0.borrow().stats, [1, 0, 0]);
    }

    #[test]
    fn unreadable_calls_are_dropped_with_a_warning() {
        let f = Fake::with_target(URL);
        let id = enqueue(&f, "e", json!({})).unwrap();
        attempt(&f, &key(id), "not json").unwrap();
        attempt(&f, "delivery:abc", "{}").unwrap();
        assert_eq!(f.posts(), 0);
        assert_eq!(f.0.borrow().warns.len(), 2);
    }
}
