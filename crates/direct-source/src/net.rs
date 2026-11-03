//! A GET that follows redirects itself (the host never does) and refuses a truncated body.
use crate::Fail;
use fliks_plugin_sdk::bindings::wasi::http::types::Method;
use fliks_plugin_sdk::http::Request;
use fliks_plugin_sdk::rt::{self, Budget};
use std::time::Duration;
use url::Url;

const MAX_REDIRECTS: usize = 5;
/// Left of the host's deadline when a request gives up, so it fails before the host kills the call.
const SLACK: Duration = Duration::from_secs(10);
/// One download may not run longer than this, so an endless body is cut.
const REQUEST_LIMIT: Duration = Duration::from_secs(240);

pub const TOO_LARGE: &str = "the body is larger than the download cap";
const DENIED: &str = "target address is not allowed";

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

/// A transport failure as an add failure: a refused address is final, anything else is retried.
pub fn fail(message: String) -> Fail {
    if message == DENIED {
        Fail::Drop(message)
    } else {
        Fail::Retry(message)
    }
}

/// A body is whole when it is under the cap and as long as the server announced.
fn verify(len: usize, cap: usize, announced: Option<usize>) -> Result<(), &'static str> {
    if len >= cap {
        return Err(TOO_LARGE);
    }
    if announced.is_some_and(|n| n != len) {
        return Err("the body is shorter than announced");
    }
    Ok(())
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
            .deadline(REQUEST_LIMIT)
            .budget(&Budget::new(
                Budget::host().remaining().saturating_sub(SLACK),
            ))
            .send()
            .map_err(|e| e.to_string())?;
        let announced = r
            .header("content-length")
            .and_then(|v| v.trim().parse::<usize>().ok());
        verify(r.body.len(), self.cap, announced)?;
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
    fn a_body_must_be_whole_and_under_the_cap() {
        assert_eq!(verify(10, 100, Some(10)), Ok(()));
        assert_eq!(verify(10, 100, None), Ok(()));
        assert_eq!(verify(100, 100, None), Err(TOO_LARGE));
        assert!(verify(5, 100, Some(10)).is_err());
    }

    #[test]
    fn a_refused_address_is_final() {
        assert!(matches!(
            fail("target address is not allowed".into()),
            Fail::Drop(_)
        ));
        assert!(matches!(fail("timeout".into()), Fail::Retry(_)));
    }

    #[test]
    fn statuses_split_into_drop_and_retry() {
        assert!(body(reply(200, None)).is_ok());
        assert!(matches!(body(reply(404, None)), Err(Fail::Drop(_))));
        assert!(matches!(body(reply(429, None)), Err(Fail::Retry(_))));
        assert!(matches!(body(reply(503, None)), Err(Fail::Retry(_))));
    }
}
