//! The HTTP status to `ProviderError` mapping every provider plugin shares.
use fliks_plugin_sdk_provider::{ProviderError, UpstreamError};
use serde_json::Value;

pub fn upstream(status: Option<u16>, message: impl Into<String>) -> ProviderError {
    ProviderError::Upstream(UpstreamError {
        status,
        message: message.into(),
    })
}

/// A non-2xx status as the provider error: 404 not found, 429 rate limited, else upstream with `message`.
pub fn status_error(
    status: u16,
    retry_after: Option<u32>,
    message: impl Into<String>,
) -> ProviderError {
    match status {
        404 => ProviderError::NotFound,
        429 => ProviderError::RateLimited(retry_after),
        _ => upstream(Some(status), message),
    }
}

/// An answer of a JSON API as the provider reports it; a message never carries the request URL.
pub fn json_reply(
    status: u16,
    retry_after: Option<&str>,
    body: &[u8],
) -> Result<Value, ProviderError> {
    let parsed = serde_json::from_slice::<Value>(body);
    if (200..300).contains(&status) {
        return parsed.map_err(|_| upstream(Some(status), "invalid JSON response"));
    }
    if status == 401 {
        return Err(ProviderError::Unauthorized);
    }
    let body = parsed.unwrap_or(Value::Null);
    let said = ["status_message", "message"]
        .iter()
        .find_map(|k| body.get(k).and_then(Value::as_str))
        .map_or_else(
            || format!("Request failed with status code {status}"),
            str::to_owned,
        );
    let retry = retry_after.and_then(|r| r.trim().parse().ok());
    Err(status_error(
        status,
        retry,
        format!("HTTP {status}: {said}"),
    ))
}
