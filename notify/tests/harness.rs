#![cfg(not(target_arch = "wasm32"))]
//! The built component under the real host and a scratch Postgres. Needs the release component
//! (`cargo build --release --target wasm32-wasip2`) and `FLIKS_HARNESS_PG_URL`.

use fliks_plugin_sdk::harness::{Harness, Setup, request, until};
use serde_json::{Value, json};
use std::net::TcpListener;
use std::time::Duration;

const MAX: Duration = Duration::from_secs(20);

fn setup(target: Option<&str>) -> Setup {
    let mut s = Setup::from_crate(env!("CARGO_MANIFEST_DIR"));
    s.config = target
        .map(|t| vec![("target_url".to_owned(), t.to_owned())])
        .unwrap_or_default();
    s
}

async fn log(h: &Harness) -> Value {
    let r = h.proxy("GET", "/delivery-log", None).await;
    assert_eq!(r.status, 200);
    serde_json::from_slice(&r.body).unwrap()
}

async fn status_of(h: &Harness, id: i64) -> String {
    let l = log(h).await;
    let e = l["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id);
    e.map_or_else(String::new, |e| {
        e["lastStatus"].as_str().unwrap().to_owned()
    })
}

async fn test_delivery(h: &Harness, event: &str) -> i64 {
    let r = h
        .proxy("POST", "/test-delivery", Some(&json!({ "event": event })))
        .await;
    assert_eq!(r.status, 202);
    serde_json::from_slice::<Value>(&r.body).unwrap()["id"]
        .as_i64()
        .unwrap()
}

/// A local port nothing listens on, so a routed connection is refused.
fn closed_port() -> std::net::SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_target_is_refused_and_logged() {
    let Some(h) = Harness::start(setup(None)).await else {
        return;
    };
    h.event("ping", &json!({ "n": 1 }));
    until("the event is logged", MAX, async || {
        log(&h).await["entries"].as_array().unwrap().len() == 1
    })
    .await;
    let id = test_delivery(&h, "custom.event").await;
    assert_eq!(status_of(&h, id).await, "refused:no target configured");
    let l = log(&h).await;
    assert_eq!(l["entries"][0]["id"], id, "newest first");
    assert_eq!(l["entries"][0]["targetHost"], "unconfigured");
    assert_eq!(l["stats"]["queued"], 0);
    assert!(h.scheduled_calls().await.is_empty());
    // The host only routes declared paths, so the plugin's own 404 is reached directly.
    assert_eq!(
        h.http(request("GET", "/nope", &[], b""))
            .await
            .unwrap()
            .status,
        404
    );
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guard_and_dns_refusals_are_final() {
    let s = setup(Some("https://nowhere.test/hook"));
    s.targets.answer("priv.test", &["10.0.0.7"]);
    let Some(h) = Harness::start(s).await else {
        return;
    };
    let dns = test_delivery(&h, "a").await;
    until("the DNS refusal", MAX, async || {
        status_of(&h, dns).await.starts_with("refused:")
    })
    .await;
    h.set_config("target_url", Some("https://priv.test/hook"))
        .await;
    let denied = test_delivery(&h, "b").await;
    until("the guard denial", MAX, async || {
        status_of(&h, denied).await.starts_with("refused:")
    })
    .await;
    h.set_config("target_url", Some("http://pub.test/hook"))
        .await;
    let plain = test_delivery(&h, "c").await;
    until("the http refusal", MAX, async || {
        status_of(&h, plain).await == "refused:target must be https:"
    })
    .await;
    assert!(
        h.scheduled_calls().await.is_empty(),
        "no retry after a refusal"
    );
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_attempts_follow_the_ladder_then_fail() {
    let s = setup(Some("https://pub.test/hook?t=s3cret"));
    s.targets.answer("pub.test", &["93.184.216.34"]);
    s.targets.route("93.184.216.34:443", closed_port());
    let Some(h) = Harness::start(s).await else {
        return;
    };
    let id = test_delivery(&h, "a").await;
    for (n, delay_ms) in [1000, 5000, 15000, 30000, 60000].into_iter().enumerate() {
        // The claimed call keeps its lease until the plugin replaces it with the retry.
        until("the attempt and its retry", MAX, async || {
            let l = log(&h).await;
            let calls = h.scheduled_calls().await;
            l["entries"][0]["attempt"] == n as i64 + 1
                && calls.len() == 1
                && calls[0].due_in_ms <= delay_ms
        })
        .await;
        assert!(status_of(&h, id).await.starts_with("retrying:"));
        let calls = h.scheduled_calls().await;
        assert_eq!(calls.len(), 1);
        assert!(!calls[0].payload.contains("s3cret"));
        assert!(
            (delay_ms - 2000..=delay_ms).contains(&calls[0].due_in_ms),
            "attempt {} retries in {delay_ms} ms, not {}",
            n + 1,
            calls[0].due_in_ms
        );
        h.advance(Duration::from_millis(delay_ms as u64)).await;
    }
    until("the final failure", MAX, async || {
        status_of(&h, id).await.starts_with("failed:")
    })
    .await;
    let l = log(&h).await;
    assert_eq!(l["entries"][0]["attempt"], 6);
    assert_eq!(l["stats"]["failedPermanently"], 1);
    assert_eq!(l["stats"]["queued"], 0);
    assert!(h.scheduled_calls().await.is_empty());
    let logs = h.logs();
    assert!(logs.iter().any(|l| l.contains("failed after 6 attempts")));
    assert!(
        logs.iter().all(|l| !l.contains("s3cret")),
        "the URL is never logged"
    );
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retries_follow_the_current_target_and_survive_a_restart() {
    let s = setup(Some("https://pub.test/hook"));
    s.targets.answer("pub.test", &["93.184.216.34"]);
    s.targets.route("93.184.216.34:443", closed_port());
    let Some(h) = Harness::start(s).await else {
        return;
    };
    let id = test_delivery(&h, "a").await;
    until("the first attempt", MAX, async || {
        status_of(&h, id).await.starts_with("retrying:")
    })
    .await;
    h.restart().await;
    assert_eq!(h.scheduled_calls().await.len(), 1, "the retry survives");
    h.set_config("target_url", Some("https://nowhere.test/hook"))
        .await;
    h.advance(Duration::from_secs(2)).await;
    until("the retargeted attempt", MAX, async || {
        status_of(&h, id).await.starts_with("refused:")
    })
    .await;
    h.finish().await;
}
