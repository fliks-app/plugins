//! `GET /delivery-log` and `POST /test-delivery`.

use crate::env::Env;
use crate::queue;
use fliks_plugin_sdk::{HttpRequest, HttpResponse, json_response, time};
use serde_json::{Value, json};

pub fn handle(env: &impl Env, req: HttpRequest) -> Result<HttpResponse, String> {
    match (
        req.method.to_ascii_uppercase().as_str(),
        req.path.trim_end_matches('/'),
    ) {
        ("GET", "/delivery-log") => {
            let entries: Vec<Value> = env
                .entries()?
                .into_iter()
                .map(|e| {
                    json!({
                        "id": e.id,
                        "event": e.event,
                        "targetHost": e.target_host,
                        "attempt": e.attempt,
                        "lastStatus": e.last_status,
                        "createdAt": e.created_at,
                        "lastAttemptAt": e.last_attempt_at,
                    })
                })
                .collect();
            let s = env.stats()?;
            let body = json!({
                "entries": entries,
                "stats": {
                    "queued": s.queued,
                    "delivered": s.delivered,
                    "failedPermanently": s.failed,
                    "droppedForCapacity": s.dropped,
                },
            });
            Ok(json_response(200, body.to_string()))
        }
        ("POST", "/test-delivery") => {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let event = body["event"].as_str().unwrap_or("test.delivery");
            let payload = json!({ "triggeredBy": "admin-test", "at": time::now_rfc3339() });
            let id = queue::enqueue(env, event, payload)?;
            Ok(json_response(
                202,
                json!({ "queued": true, "id": id }).to_string(),
            ))
        }
        _ => Ok(json_response(404, r#"{"error":"not found"}"#)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Fake;
    use fliks_plugin_sdk::bindings::fliks::plugin::types::Principal;

    fn req(method: &str, path: &str, body: &str) -> HttpRequest {
        HttpRequest {
            method: method.into(),
            path: path.into(),
            query: vec![],
            body: body.as_bytes().to_vec(),
            principal: Principal::System,
        }
    }

    fn json_of(r: &HttpResponse) -> Value {
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k == "content-type" && v == "application/json")
        );
        serde_json::from_slice(&r.body).unwrap()
    }

    #[test]
    fn log_is_newest_first_capped_at_100_with_stats() {
        let f = Fake::with_target("https://hooks.example.com/");
        for i in 0..120 {
            queue::enqueue(&f, &format!("e{i}"), json!({})).unwrap();
        }
        let r = handle(&f, req("GET", "/delivery-log", "")).unwrap();
        assert_eq!(r.status, 200);
        let v = json_of(&r);
        let entries = v["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 100);
        assert_eq!(entries[0]["id"], 120);
        assert_eq!(entries[0]["event"], "e119");
        assert_eq!(entries[0]["targetHost"], "hooks.example.com");
        assert_eq!(entries[0]["lastStatus"], "pending");
        assert_eq!(entries[0]["lastAttemptAt"], Value::Null);
        assert_eq!(
            v["stats"],
            json!({"queued": 120, "delivered": 0, "failedPermanently": 0, "droppedForCapacity": 0})
        );
        let keys: Vec<&String> = entries[0].as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 7);
    }

    #[test]
    fn test_delivery_uses_the_event_name_when_a_string() {
        let f = Fake::with_target("https://hooks.example.com/");
        let r = handle(&f, req("POST", "/test-delivery", r#"{"event":"my.event"}"#)).unwrap();
        assert_eq!(
            (r.status, json_of(&r)),
            (202, json!({"queued": true, "id": 1}))
        );
        let r = handle(&f, req("POST", "/test-delivery", r#"{"event":5}"#)).unwrap();
        assert_eq!(json_of(&r), json!({"queued": true, "id": 2}));
        let r = handle(&f, req("POST", "/test-delivery/", "not json")).unwrap();
        assert_eq!(r.status, 202);
        let s = f.0.borrow();
        let events: Vec<&str> = s.rows.iter().map(|r| r.event.as_str()).collect();
        assert_eq!(events, ["my.event", "test.delivery", "test.delivery"]);
        let (_, call) = &s.calls["delivery:1"];
        assert_eq!(call["payload"]["triggeredBy"], "admin-test");
        assert!(call["payload"]["at"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn test_delivery_without_a_target_still_answers_202() {
        let f = Fake::default();
        let r = handle(&f, req("POST", "/test-delivery", "")).unwrap();
        assert_eq!(
            (r.status, json_of(&r)),
            (202, json!({"queued": true, "id": 1}))
        );
        assert_eq!(f.row(1).1, "refused:no target configured");
    }

    #[test]
    fn anything_else_is_404_json() {
        let f = Fake::default();
        for (m, p) in [
            ("GET", "/nope"),
            ("POST", "/delivery-log"),
            ("DELETE", "/test-delivery"),
        ] {
            let r = handle(&f, req(m, p, "")).unwrap();
            assert_eq!(
                (r.status, json_of(&r)),
                (404, json!({"error": "not found"}))
            );
        }
    }
}
