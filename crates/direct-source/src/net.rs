//! A GET that follows redirects itself (the host never does) and refuses a truncated body.
use crate::Fail;
use fliks_plugin_sdk::bindings::wasi::http::types::Method;
use fliks_plugin_sdk::http::Request;
use fliks_plugin_sdk::rt::{self, Budget};
use std::time::Duration;
use url::Url;

const MAX_REDIRECTS: usize = 5;

pub struct Reply {
    pub status: u16,
    pub location: Option<String>,
    pub retry_after: Option<u32>,
    pub body: Vec<u8>,
}

pub trait Transport {
    /// A transport failure is its message, which must carry no URL.
    fn get(&self, url: &str) -> Result<Reply, String>;
}

/// The reply of `url` after at most five redirects; a redirect target is resolved against its source.
pub fn follow(t: &impl Transport, url: &str) -> Result<Reply, String> {
    let mut url = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        let r = t.get(&url)?;
        if !matches!(r.status, 301 | 302 | 303 | 307 | 308) {
            return Ok(r);
        }
        let to = r.location.ok_or("redirect without a location")?;
        url = Url::parse(&url)
            .and_then(|base| base.join(&to))
            .map_err(|_| "redirect to an invalid URL")?
            .into();
    }
    Err("too many redirects".into())
}

/// The body of a 2xx reply; 404 and other 4xx (429 aside) end the add, anything else is retried.
pub fn body(r: Reply) -> Result<Vec<u8>, Fail> {
    match r.status {
        200..=299 => Ok(r.body),
        429 => Err(Fail::Retry("rate limited".into())),
        400..=499 => Err(Fail::Drop(format!("the source answered {}", r.status))),
        s => Err(Fail::Retry(format!("the source answered {s}"))),
    }
}

/// The wasm host's guarded client, paced by `gap` before every request.
pub struct Live {
    pub user_agent: &'static str,
    pub accept: &'static str,
    pub gap: Duration,
    pub cap: usize,
}

impl Transport for Live {
    fn get(&self, url: &str) -> Result<Reply, String> {
        if !self.gap.is_zero() {
            rt::block_on(rt::sleep(self.gap));
        }
        let r = Request::new(Method::Get, url)
            .header("user-agent", self.user_agent)
            .header("accept", self.accept)
            .timeout(Duration::from_secs(60))
            .body_cap(self.cap)
            .budget(&Budget::host())
            .send()
            .map_err(|e| e.to_string())?;
        if r.body.len() >= self.cap {
            return Err("the body is larger than the download cap".into());
        }
        let announced = r
            .header("content-length")
            .and_then(|v| v.trim().parse::<usize>().ok());
        if announced.is_some_and(|n| n != r.body.len()) {
            return Err("the body is shorter than announced".into());
        }
        Ok(Reply {
            status: r.status,
            location: r.header("location"),
            retry_after: r.header("retry-after").and_then(|v| v.trim().parse().ok()),
            body: r.body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake(RefCell<Vec<(String, Reply)>>);

    impl Transport for Fake {
        fn get(&self, url: &str) -> Result<Reply, String> {
            let mut hops = self.0.borrow_mut();
            let (want, reply) = hops.remove(0);
            assert_eq!(url, want);
            Ok(reply)
        }
    }

    fn reply(status: u16, location: Option<&str>) -> Reply {
        Reply {
            status,
            location: location.map(Into::into),
            retry_after: None,
            body: b"ok".to_vec(),
        }
    }

    #[test]
    fn follows_absolute_and_relative_redirects() {
        let t = Fake(RefCell::new(vec![
            ("https://a.test/x".into(), reply(302, Some("/y"))),
            (
                "https://a.test/y".into(),
                reply(301, Some("https://b.test/z")),
            ),
            ("https://b.test/z".into(), reply(200, None)),
        ]));
        assert_eq!(follow(&t, "https://a.test/x").unwrap().body, b"ok");
    }

    #[test]
    fn gives_up_after_five_redirects() {
        let hops = (0..6)
            .map(|_| ("https://a.test/".into(), reply(302, Some("/"))))
            .collect();
        assert!(follow(&Fake(RefCell::new(hops)), "https://a.test/").is_err());
    }

    #[test]
    fn statuses_split_into_drop_and_retry() {
        assert!(body(reply(200, None)).is_ok());
        assert!(matches!(body(reply(404, None)), Err(Fail::Drop(_))));
        assert!(matches!(body(reply(429, None)), Err(Fail::Retry(_))));
        assert!(matches!(body(reply(503, None)), Err(Fail::Retry(_))));
    }
}
