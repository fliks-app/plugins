//! Fanart.tv images as candidates: every artwork kind of an artist or a release group, likes as the score.
use crate::api::{Http, R, number, uuid};
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
const MOVIE: [(&str, &str); 4] = [
    ("movieposter", "poster"),
    ("moviebackground", "fanart"),
    ("hdmovielogo", "logo"),
    ("movielogo", "logo"),
];
const SHOW: [(&str, &str); 4] = [
    ("tvposter", "poster"),
    ("showbackground", "fanart"),
    ("hdtvlogo", "logo"),
    ("clearlogo", "logo"),
];
const SEASON: [(&str, &str); 1] = [("seasonposter", "poster")];

/// The candidates of the artwork lists `kinds` names; with `season`, only the entries of that season (or of all).
pub fn candidates(v: &Value, kinds: &[(&str, &str)], season: Option<u32>) -> Vec<ImageCandidate> {
    let mut out = Vec::new();
    for (key, slot) in kinds {
        for i in v.get(key).and_then(Value::as_array).into_iter().flatten() {
            if let Some(n) = season
                && !matches!(i["season"].as_str(), Some("all") | None)
                && i["season"].as_str().and_then(|s| s.parse().ok()) != Some(n)
            {
                continue;
            }
            let Some(url) = i["url"].as_str().and_then(https) else {
                continue;
            };
            out.push(ImageCandidate {
                url,
                slot: (*slot).into(),
                language: i["lang"]
                    .as_str()
                    .filter(|l| l.len() == 2 && l.bytes().all(|b| b.is_ascii_lowercase()))
                    .map(str::to_owned),
                score: i["likes"].as_str().and_then(|l| l.parse().ok()),
                ..Default::default()
            });
        }
    }
    out
}

/// Music is keyed by MusicBrainz ids, movies by TMDB ids and shows by TVDB ids.
pub fn images(h: &impl Http, kind: &str, id: &str, scope: &ImageScope) -> R<Vec<ImageCandidate>> {
    match (kind, scope) {
        ("album", ImageScope::Person) => {
            let id = uuid(id).ok_or(ProviderError::NotFound)?;
            Ok(candidates(&h.get(&format!("/music/{id}"))?, &ARTIST, None))
        }
        ("album", ImageScope::Work) => {
            let id = uuid(id).ok_or(ProviderError::NotFound)?;
            let a = h.get(&format!("/music/albums/{id}"))?;
            Ok(a.pointer(&format!("/albums/{id}"))
                .map_or_else(Vec::new, |g| candidates(g, &ALBUM, None)))
        }
        ("movie", ImageScope::Work) => {
            let id = number(id).ok_or(ProviderError::NotFound)?;
            Ok(candidates(&h.get(&format!("/movies/{id}"))?, &MOVIE, None))
        }
        ("series", ImageScope::Work | ImageScope::Season(_)) => {
            let id = number(id).ok_or(ProviderError::NotFound)?;
            let v = h.get(&format!("/tv/{id}"))?;
            Ok(match scope {
                ImageScope::Season(n) => candidates(&v, &SEASON, Some(*n)),
                _ => candidates(&v, &SHOW, None),
            })
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
        let c = candidates(&v, &ARTIST, None);
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
        assert_eq!(candidates(&v, &ALBUM, None).len(), 2);
    }

    #[test]
    fn images_of_a_show_keep_the_language_and_the_season() {
        let v = json!({
            "tvposter": [{"url": "https://assets.fanart.tv/p.jpg", "lang": "fr", "likes": "3"}],
            "seasonposter": [
                {"url": "https://assets.fanart.tv/s1.jpg", "lang": "00", "season": "1"},
                {"url": "https://assets.fanart.tv/s2.jpg", "season": "2"},
                {"url": "https://assets.fanart.tv/sa.jpg", "season": "all"}
            ]
        });
        let show = candidates(&v, &SHOW, None);
        assert_eq!(
            (show[0].language.as_deref(), show[0].slot.as_str()),
            (Some("fr"), "poster")
        );
        let season = candidates(&v, &SEASON, Some(1));
        let urls: Vec<_> = season.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://assets.fanart.tv/s1.jpg",
                "https://assets.fanart.tv/sa.jpg"
            ]
        );
        assert_eq!(season[0].language, None);
    }

    #[test]
    fn images_of_a_movie_use_every_artwork_list() {
        let v = json!({"movieposter": [{"url": "https://assets.fanart.tv/a.jpg"}],
            "moviebackground": [{"url": "https://assets.fanart.tv/b.jpg"}],
            "hdmovielogo": [{"url": "https://assets.fanart.tv/c.png"}]});
        let slots: Vec<_> = candidates(&v, &MOVIE, None)
            .into_iter()
            .map(|c| c.slot)
            .collect();
        assert_eq!(slots, ["poster", "fanart", "logo"]);
    }
}
