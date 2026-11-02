//! The Audnexus lookups over an injected transport, so recorded bodies can stand in for the network.
use crate::parse::{self as audnexus};
use fliks_plugin_sdk::ExternalId;
use fliks_plugin_sdk_provider::ProviderError;
use provider_common::book::region::Region;
use provider_common::book::{ParseError, to_record};
use provider_common::http::{status_error, upstream};
use serde_json::Value;

type R<T> = Result<T, ProviderError>;

#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub retry_after: Option<u32>,
    pub body: String,
}

pub trait Http {
    /// A transport failure is its message, which must carry no URL.
    fn get(&self, url: &str) -> Result<Reply, String>;
}

fn parse_err(e: ParseError) -> ProviderError {
    match e {
        ParseError::NotFound | ParseError::Moved(_) => ProviderError::NotFound,
        ParseError::RateLimited => ProviderError::RateLimited(None),
        ParseError::Malformed(m) | ParseError::Api(m) => upstream(None, m),
    }
}

const NS: &str = "asin";

/// `Retry-After`, else the `retryAfterSeconds` Audnexus puts in its 429 body.
fn retry_after(r: &Reply) -> Option<u32> {
    r.retry_after.or_else(|| {
        let v: Value = serde_json::from_str(&r.body).ok()?;
        let secs = v.pointer("/error/details/retryAfterSeconds")?.as_u64()?;
        u32::try_from(secs).ok()
    })
}

fn get(h: &impl Http, url: &str) -> R<String> {
    let r = h.get(url).map_err(|m| upstream(None, m))?;
    match r.status {
        200..=299 => Ok(r.body),
        s => Err(status_error(
            s,
            retry_after(&r),
            format!("Audnexus answered {s}"),
        )),
    }
}

/// `None` for an id outside the `asin` namespace or a malformed ASIN, with no request made.
fn lookup<T>(
    h: &impl Http,
    edition: &ExternalId,
    region: Region,
    url: fn(&str, Option<Region>) -> Option<String>,
    parse: fn(&str) -> Result<T, ParseError>,
) -> R<Option<T>> {
    let url = (edition.provider == NS)
        .then(|| url(&edition.value, Some(region)))
        .flatten();
    let Some(url) = url else { return Ok(None) };
    match get(h, &url).and_then(|b| parse(&b).map_err(parse_err)) {
        Ok(r) => Ok(Some(r)),
        Err(ProviderError::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn fetch_edition(h: &impl Http, edition: &ExternalId, region: Region) -> R<Option<String>> {
    let r = lookup(h, edition, region, audnexus::book_url, audnexus::parse_book)?;
    Ok(r.map(|r| to_record(&r).to_string()))
}

pub fn fetch_chapters(h: &impl Http, edition: &ExternalId, region: Region) -> R<Option<String>> {
    let r = lookup(
        h,
        edition,
        region,
        audnexus::chapters_url,
        audnexus::parse_chapters,
    )?;
    Ok(r.map(|c| serde_json::json!(c).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    macro_rules! fixture {
        ($f:literal) => {
            include_str!(concat!("../tests/fixtures/", $f))
        };
    }

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            status,
            retry_after: None,
            body: body.into(),
        }
    }

    /// Answers every request with one reply and records what was asked.
    struct Fake {
        reply: Reply,
        seen: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(reply: Reply) -> Fake {
            Fake {
                reply,
                seen: RefCell::default(),
            }
        }
        fn asked(&self) -> Vec<String> {
            self.seen.borrow().clone()
        }
    }

    impl Http for Fake {
        fn get(&self, url: &str) -> Result<Reply, String> {
            self.seen.borrow_mut().push(url.to_owned());
            Ok(self.reply.clone())
        }
    }

    fn id(provider: &str, value: &str) -> ExternalId {
        ExternalId {
            provider: provider.into(),
            value: value.into(),
        }
    }

    fn json_of(s: String) -> Value {
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn a_book_is_an_edition_record() {
        let h = Fake::new(reply(200, fixture!("book-fr.json")));
        let r = fetch_edition(&h, &id("asin", "B0QZ7LM4NV"), Region::Fr).unwrap();
        let v = json_of(r.unwrap());
        assert_eq!(v["externalId"], "B0QZ7LM4NV");
        assert_eq!(v["format"], "audio");
        assert_eq!(
            h.asked(),
            ["https://api.audnex.us/books/B0QZ7LM4NV?region=fr"]
        );
    }

    #[test]
    fn chapters_keep_the_parsed_shape() {
        let h = Fake::new(reply(200, fixture!("chapters.json")));
        let r = fetch_chapters(&h, &id("asin", "B0QZ7LM4NV"), Region::Uk).unwrap();
        let v = json_of(r.unwrap());
        assert_eq!(v["asin"], "B0QZ7LM4NV");
        assert_eq!(v["accurate"], true);
        assert_eq!(v["brandIntroMs"], 2000);
        assert_eq!(
            v["chapters"][1],
            json!({"title": "La rue des Lampistes", "startMs": 60000, "lengthMs": 1800500})
        );
        assert_eq!(v["chapters"].as_array().unwrap().len(), 3);
        assert_eq!(
            h.asked(),
            ["https://api.audnex.us/books/B0QZ7LM4NV/chapters?region=uk"]
        );
    }

    #[test]
    fn a_region_miss_is_none() {
        let h = Fake::new(reply(404, fixture!("error-region.json")));
        let asin = id("asin", "B0QZ7LM4NV");
        assert!(fetch_edition(&h, &asin, Region::Us).unwrap().is_none());
        assert!(fetch_chapters(&h, &asin, Region::Us).unwrap().is_none());
        let h = Fake::new(reply(200, fixture!("error-region.json")));
        assert!(fetch_edition(&h, &asin, Region::Us).unwrap().is_none());
    }

    #[test]
    fn a_foreign_or_malformed_id_asks_nothing() {
        let h = Fake::new(reply(200, ""));
        assert!(
            fetch_edition(&h, &id("isbn", "9782999000018"), Region::Us)
                .unwrap()
                .is_none()
        );
        assert!(
            fetch_chapters(&h, &id("asin", "../x"), Region::Us)
                .unwrap()
                .is_none()
        );
        assert!(h.asked().is_empty());
    }

    #[test]
    fn rate_limits_carry_their_retry_after() {
        let asin = id("asin", "B0QZ7LM4NV");
        let get = |r: Reply| fetch_chapters(&Fake::new(r), &asin, Region::Us).unwrap_err();
        let mut header = reply(429, "");
        header.retry_after = Some(7);
        assert!(matches!(get(header), ProviderError::RateLimited(Some(7))));
        let body = reply(429, fixture!("error-ratelimit.json"));
        assert!(matches!(get(body), ProviderError::RateLimited(Some(60))));
        assert!(matches!(
            get(reply(429, "")),
            ProviderError::RateLimited(None)
        ));
        let in_body = reply(200, fixture!("error-ratelimit.json"));
        assert!(matches!(get(in_body), ProviderError::RateLimited(None)));
    }

    #[test]
    fn upstream_failures_map_to_the_provider_errors() {
        let asin = id("asin", "B0QZ7LM4NV");
        let get = |r: Reply| fetch_edition(&Fake::new(r), &asin, Region::Us).unwrap_err();
        let e = get(reply(503, "maintenance"));
        assert!(matches!(e, ProviderError::Upstream(u) if u.status == Some(503)));
        let e = get(reply(200, fixture!("error-other.json")));
        assert!(matches!(e, ProviderError::Upstream(u) if u.status.is_none()));
        let e = get(reply(200, "<html>"));
        assert!(matches!(e, ProviderError::Upstream(u) if u.status.is_none()));
    }

    #[test]
    fn a_transport_failure_is_an_upstream_error() {
        struct Down;
        impl Http for Down {
            fn get(&self, _: &str) -> Result<Reply, String> {
                Err("connection refused".into())
            }
        }
        let e = fetch_edition(&Down, &id("asin", "B0QZ7LM4NV"), Region::Us).unwrap_err();
        assert!(matches!(e, ProviderError::Upstream(u) if u.status.is_none()));
    }
}
