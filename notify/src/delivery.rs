//! One delivery attempt: the payloads, the outcome classification and the retry ladder.

use fliks_plugin_sdk::http::Error as HttpError;
use serde_json::{Value, json};

pub const MAX_ATTEMPTS: u32 = 6;
pub const BACKOFF_MS: [u64; 5] = [1000, 5000, 15000, 30000, 60000];
pub const USER_AGENT: &str = concat!("fliks.notify/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Delivered(u16),
    /// Final without retry: guard denial, DNS failure, bad URL, no target.
    Refused(String),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Delivered { status: String },
    Refused { status: String },
    Retry { status: String, delay_ms: u64 },
    Failed { status: String },
}

/// `n` counts completed attempts, this one included.
pub fn next(n: u32, outcome: Outcome) -> Action {
    match outcome {
        Outcome::Delivered(code) => Action::Delivered {
            status: format!("delivered:{code}"),
        },
        Outcome::Refused(r) => Action::Refused {
            status: format!("refused:{r}"),
        },
        Outcome::Failed(r) if n < MAX_ATTEMPTS => Action::Retry {
            status: format!("retrying:{r}"),
            delay_ms: BACKOFF_MS[(n - 1) as usize],
        },
        Outcome::Failed(r) => Action::Failed {
            status: format!("failed:{r}"),
        },
    }
}

/// The target as an `https` URL, or the reason it is refused.
pub fn parse_target(target: &str) -> Result<url::Url, Outcome> {
    let url = url::Url::parse(target).map_err(|_| Outcome::Refused("not a valid URL".into()))?;
    if url.scheme() != "https" {
        return Err(Outcome::Refused("target must be https:".into()));
    }
    Ok(url)
}

/// The JSON a target receives.
pub fn body(id: i64, event: &str, payload: &Value, attempt: u32) -> String {
    json!({ "id": id, "event": event, "payload": payload, "attempt": attempt }).to_string()
}

pub fn classify(sent: Result<u16, HttpError>) -> Outcome {
    match sent {
        Ok(s) if (200..300).contains(&s) => Outcome::Delivered(s),
        Ok(s) => Outcome::Failed(format!("target replied {s}")),
        Err(e @ (HttpError::Denied | HttpError::Dns(_))) => Outcome::Refused(e.to_string()),
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// `targetHost` as the log shows it.
pub fn host_of(target: Option<&str>) -> String {
    match target {
        None => "unconfigured".into(),
        Some(t) => url::Url::parse(t)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_else(|| "invalid".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed() -> Outcome {
        Outcome::Failed("target replied 500".into())
    }

    #[test]
    fn six_attempts_with_five_delays() {
        let delays: Vec<u64> = (1..MAX_ATTEMPTS)
            .map(|n| match next(n, failed()) {
                Action::Retry { status, delay_ms } => {
                    assert_eq!(status, "retrying:target replied 500");
                    delay_ms
                }
                a => panic!("{a:?}"),
            })
            .collect();
        assert_eq!(delays, [1000, 5000, 15000, 30000, 60000]);
        assert_eq!(
            next(6, failed()),
            Action::Failed {
                status: "failed:target replied 500".into()
            }
        );
    }

    #[test]
    fn success_and_refusals_are_final_at_any_attempt() {
        for n in 1..=MAX_ATTEMPTS {
            assert_eq!(
                next(n, Outcome::Delivered(204)),
                Action::Delivered {
                    status: "delivered:204".into()
                }
            );
            assert_eq!(
                next(n, Outcome::Refused("x".into())),
                Action::Refused {
                    status: "refused:x".into()
                }
            );
        }
    }

    #[test]
    fn classifies_http_results() {
        assert_eq!(classify(Ok(200)), Outcome::Delivered(200));
        assert_eq!(classify(Ok(299)), Outcome::Delivered(299));
        assert_eq!(
            classify(Ok(301)),
            Outcome::Failed("target replied 301".into())
        );
        assert_eq!(
            classify(Ok(503)),
            Outcome::Failed("target replied 503".into())
        );
        assert_eq!(
            classify(Err(HttpError::Denied)),
            Outcome::Refused("target address is not allowed".into())
        );
        assert_eq!(
            classify(Err(HttpError::Dns("nxdomain".into()))),
            Outcome::Refused("DNS lookup failed: nxdomain".into())
        );
        for e in [HttpError::Timeout, HttpError::Refused, HttpError::Tls] {
            assert!(matches!(classify(Err(e)), Outcome::Failed(_)));
        }
    }

    #[test]
    fn target_must_be_a_https_url() {
        assert_eq!(
            parse_target("::"),
            Err(Outcome::Refused("not a valid URL".into()))
        );
        assert_eq!(
            parse_target("http://hooks.example.com/x"),
            Err(Outcome::Refused("target must be https:".into()))
        );
        assert!(parse_target("https://hooks.example.com/x?t=1").is_ok());
    }

    #[test]
    fn delivery_body_shape() {
        let v: Value =
            serde_json::from_str(&body(7, "media.imported", &json!({"a": 1}), 3)).unwrap();
        assert_eq!(
            v,
            json!({"id": 7, "event": "media.imported", "payload": {"a": 1}, "attempt": 3})
        );
    }

    #[test]
    fn user_agent_carries_the_crate_version() {
        assert_eq!(USER_AGENT, "fliks.notify/1.0.0");
    }

    #[test]
    fn hosts() {
        assert_eq!(host_of(None), "unconfigured");
        assert_eq!(host_of(Some("::")), "invalid");
        assert_eq!(
            host_of(Some("https://hooks.example.com/x?t=1")),
            "hooks.example.com"
        );
    }
}
