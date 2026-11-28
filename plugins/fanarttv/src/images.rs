//! Fanart.tv images as candidates: every artwork kind of an artist or a release group, likes as the score.
use crate::api::{Http, R, uuid};
use fliks_plugin_sdk_provider::{ImageCandidate, ImageScope, ProviderError};
use provider_common::comic::https;
use serde_json::Value;

const ARTIST: [(&str, &str); 5] = [
    ("artistthumb", "photo"),
    ("artistbackground", "fanart"),
    ("hdmusiclogo", "logo"),
    ("musiclogo", "logo"),
    ("musicbanner", "fanart"),
];
const ALBUM: [(&str, &str); 2] = [("albumcover", "cover"), ("cdart", "cover")];

pub fn candidates(v: &Value, kinds: &[(&str, &str)]) -> Vec<ImageCandidate> {
    let mut out = Vec::new();
    for (key, slot) in kinds {
        for i in v.get(key).and_then(Value::as_array).into_iter().flatten() {
            let Some(url) = i["url"].as_str().and_then(https) else {
                continue;
            };
            out.push(ImageCandidate {
                url,
                slot: (*slot).into(),
                score: i["likes"].as_str().and_then(|l| l.parse().ok()),
                ..Default::default()
            });
        }
    }
    out
}

pub fn images(h: &impl Http, id: &str, scope: &ImageScope) -> R<Vec<ImageCandidate>> {
    let id = uuid(id).ok_or(ProviderError::NotFound)?;
    match scope {
        ImageScope::Person => Ok(candidates(&h.get(&format!("/music/{id}"))?, &ARTIST)),
        ImageScope::Work => {
            let a = h.get(&format!("/music/albums/{id}"))?;
            Ok(a.pointer(&format!("/albums/{id}"))
                .map_or_else(Vec::new, |g| candidates(g, &ALBUM)))
        }
        _ => Err(ProviderError::Unsupported),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn images_use_every_kind_over_https_with_likes_as_score() {
        let v = json!({
            "artistthumb": [{"url": "http://assets.fanart.tv/t.jpg", "likes": "4"}],
            "hdmusiclogo": [{"url": "https://assets.fanart.tv/l.png", "likes": "x"}],
            "artistbackground": [{"url": "ftp://x/b.jpg"}]
        });
        let c = candidates(&v, &ARTIST);
        let got: Vec<_> = c
            .iter()
            .map(|i| (i.slot.as_str(), i.url.as_str(), i.score))
            .collect();
        assert_eq!(
            got,
            [
                ("photo", "https://assets.fanart.tv/t.jpg", Some(4.0)),
                ("logo", "https://assets.fanart.tv/l.png", None)
            ]
        );
    }

    #[test]
    fn images_album_covers_include_the_disc_art() {
        let v = json!({"albumcover": [{"url": "https://assets.fanart.tv/c.jpg"}], "cdart": [{"url": "https://assets.fanart.tv/d.jpg"}]});
        assert_eq!(candidates(&v, &ALBUM).len(), 2);
    }
}
