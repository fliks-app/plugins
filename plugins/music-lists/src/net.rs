//! The HTTP transport the sources read through, so recorded bodies can stand in for the network.
use fliks_plugin_sdk::bindings::wasi::http::types::Method;
use fliks_plugin_sdk::http::Request;
use fliks_plugin_sdk::rt::{self, Budget};
use serde_json::Value;
use std::time::Duration;

pub const USER_AGENT: &str = concat!(
    "Fliks/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/fliks-app/fliks)"
);

pub type Headers<'a> = &'a [(&'a str, String)];

pub trait Net {
    /// A GET answered as JSON; `None` for an empty answer (204 or no body).
    fn get(&self, url: &str, headers: Headers) -> Result<Option<Value>, String>;
    fn post_form(
        &self,
        url: &str,
        headers: Headers,
        form: &[(&str, &str)],
    ) -> Result<Value, String>;
    fn pause(&self, ms: u64);
}

pub struct Live;

fn send(mut req: Request, headers: Headers) -> Result<Option<Value>, String> {
    req = req
        .header("user-agent", USER_AGENT)
        .header("accept", "application/json");
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let resp = req
        .timeout(Duration::from_secs(20))
        .budget(&Budget::host())
        .send()
        .map_err(|e| e.to_string())?;
    if !(200..300).contains(&resp.status) {
        return Err(format!("http {}", resp.status));
    }
    if resp.status == 204 || resp.body.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(&resp.body)
        .map(Some)
        .map_err(|_| "not JSON".to_owned())
}

impl Net for Live {
    fn get(&self, url: &str, headers: Headers) -> Result<Option<Value>, String> {
        send(Request::new(Method::Get, url), headers)
    }

    fn post_form(
        &self,
        url: &str,
        headers: Headers,
        form: &[(&str, &str)],
    ) -> Result<Value, String> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let req = Request::new(Method::Post, url)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body);
        send(req, headers)?.ok_or_else(|| "empty answer".to_owned())
    }

    fn pause(&self, ms: u64) {
        rt::block_on(rt::sleep(Duration::from_millis(ms)));
    }
}
