//! The artwork of a TheTVDB record as image candidates.
use crate::map::{get, text};
use crate::tvdb::{Http, Result, Tvdb, aired_seasons, missing};
use fliks_plugin_sdk_provider::{ImageCandidate, ImageScope, ProviderError};
use provider_common::lang::normalize_language_code;
use serde_json::Value;

/// The slot of an artwork type: series, movie and season posters, backgrounds and clear logos.
fn slot(kind: i64) -> Option<&'static str> {
    match kind {
        2 | 7 | 14 => Some("poster"),
        3 | 8 | 15 => Some("fanart"),
        23 | 25 => Some("logo"),
        _ => None,
    }
}

pub fn candidates(artworks: &Value) -> Vec<ImageCandidate> {
    let dim = |a: &Value, k: &str| get(a, k).as_u64().map(|n| n as u32);
    artworks
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|a| {
            let url = text(a, "image")
                .filter(|u| u.starts_with("https://") && !u.contains("/images/missing/"))?;
            Some(ImageCandidate {
                url: url.to_owned(),
                slot: slot(get(a, "type").as_i64()?)?.into(),
                language: text(a, "language").map(|l| normalize_language_code(Some(l))),
                width: dim(a, "width"),
                height: dim(a, "height"),
                score: get(a, "score").as_f64(),
            })
        })
        .collect()
}

/// A person's single portrait.
pub fn portrait(person: &Value) -> Vec<ImageCandidate> {
    text(person, "image")
        .filter(|u| u.starts_with("https://") && !u.contains("/images/missing/"))
        .map(|u| ImageCandidate {
            url: u.to_owned(),
            slot: "photo".into(),
            ..Default::default()
        })
        .into_iter()
        .collect()
}

impl<H: Http> Tvdb<H> {
    pub async fn images(
        &self,
        tv: bool,
        id: i64,
        scope: &ImageScope,
    ) -> Result<Vec<ImageCandidate>> {
        match scope {
            ImageScope::Work => {
                let kind = if tv { "series" } else { "movies" };
                let m = self.data(&format!("/{kind}/{id}/extended"), &[]).await?;
                Ok(candidates(get(&m, "artworks")))
            }
            ImageScope::Season(n) if tv => {
                let series = self
                    .data(
                        &format!("/series/{id}/extended"),
                        &[("short", "true".into())],
                    )
                    .await?;
                let sid = aired_seasons(&series)
                    .into_iter()
                    .find(|s| get(s, "number").as_i64() == Some(i64::from(*n)))
                    .and_then(|s| get(s, "id").as_i64())
                    .ok_or_else(|| missing("season"))?;
                let s = self.data(&format!("/seasons/{sid}/extended"), &[]).await?;
                Ok(candidates(get(&s, "artwork")))
            }
            ImageScope::Person => Ok(portrait(
                &self.data(&format!("/people/{id}/extended"), &[]).await?,
            )),
            _ => Err(ProviderError::Unsupported),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn images_map_types_to_slots_and_normalise_languages() {
        let v = json!([
            {"image": "https://artworks.thetvdb.com/p.jpg", "type": 2, "language": "fra", "width": 680, "height": 1000, "score": 100},
            {"image": "https://artworks.thetvdb.com/b.jpg", "type": 15, "language": null},
            {"image": "https://artworks.thetvdb.com/l.png", "type": 23},
            {"image": "https://artworks.thetvdb.com/i.png", "type": 5},
            {"image": "https://artworks.thetvdb.com/banners/images/missing/x.jpg", "type": 2}
        ]);
        let c = candidates(&v);
        let slots: Vec<_> = c.iter().map(|i| i.slot.as_str()).collect();
        assert_eq!(slots, ["poster", "fanart", "logo"]);
        assert_eq!(
            (c[0].language.as_deref(), c[0].width, c[0].score),
            (Some("fr"), Some(680), Some(100.0))
        );
        assert_eq!(c[1].language, None);
    }

    #[test]
    fn images_portrait_is_one_photo() {
        let c = portrait(&json!({"image": "https://artworks.thetvdb.com/h.jpg"}));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].slot, "photo");
        assert!(portrait(&json!({})).is_empty());
    }
}
