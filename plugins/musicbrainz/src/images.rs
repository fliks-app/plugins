//! The Cover Art Archive front covers of a release group or a release as image candidates.
use crate::api::{Http, R};
use crate::map::uuid;
use fliks_plugin_sdk_provider::{ImageCandidate, ImageScope, ProviderError};
use provider_common::comic::https;
use serde_json::Value;

pub fn candidates(v: &Value) -> Vec<ImageCandidate> {
    v.get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|i| i["front"].as_bool().unwrap_or(false))
        .filter_map(|i| {
            Some(ImageCandidate {
                url: https(i["image"].as_str()?)?,
                slot: "cover".into(),
                ..Default::default()
            })
        })
        .collect()
}

pub fn images(h: &impl Http, id: &str, scope: &ImageScope) -> R<Vec<ImageCandidate>> {
    let kind = match scope {
        ImageScope::Work => "release-group",
        ImageScope::Edition => "release",
        _ => return Err(ProviderError::Unsupported),
    };
    let id = uuid(id).ok_or(ProviderError::NotFound)?;
    match h.caa(&format!("/{kind}/{id}")) {
        Ok(v) => Ok(candidates(&v)),
        Err(ProviderError::NotFound) => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn images_keep_the_front_covers_over_https() {
        let v = json!({"images": [
            {"front": true, "back": false, "image": "http://coverartarchive.org/release/r/1.jpg"},
            {"front": false, "back": true, "image": "http://coverartarchive.org/release/r/2.jpg"}
        ]});
        let c = candidates(&v);
        assert_eq!(c.len(), 1);
        assert_eq!(
            (c[0].url.as_str(), c[0].slot.as_str()),
            ("https://coverartarchive.org/release/r/1.jpg", "cover")
        );
    }

    #[test]
    fn images_of_a_person_are_unsupported() {
        struct No;
        impl Http for No {
            fn mb(&self, _: &str, _: &[(&str, String)]) -> R<Value> {
                unreachable!()
            }
            fn caa(&self, _: &str) -> R<Value> {
                unreachable!()
            }
            fn lb(&self, _: &str, _: &Value) -> Option<Value> {
                None
            }
            fn lb_get(&self, _: &str) -> Option<Value> {
                None
            }
        }
        assert!(matches!(
            images(&No, "x", &ImageScope::Person),
            Err(ProviderError::Unsupported)
        ));
    }
}
