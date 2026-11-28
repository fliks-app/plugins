//! The image candidates of a TMDB `/images` answer, one per file, every language.
use crate::map::{arr, text};
use crate::tmdb::IMAGE_URL;
use fliks_plugin_sdk_provider::{ImageCandidate, ImageScope};
use serde_json::Value;

/// The `/images` path of a scope; none when TMDB has no image list for it.
pub fn path(tv: bool, id: i64, scope: &ImageScope) -> Option<String> {
    match (tv, scope) {
        (false, ImageScope::Work) => Some(format!("/movie/{id}/images")),
        (true, ImageScope::Work) => Some(format!("/tv/{id}/images")),
        (true, ImageScope::Season(s)) => Some(format!("/tv/{id}/season/{s}/images")),
        (true, ImageScope::Episode((s, e))) => {
            Some(format!("/tv/{id}/season/{s}/episode/{e}/images"))
        }
        (_, ImageScope::Person) => Some(format!("/person/{id}/images")),
        _ => None,
    }
}

const SLOTS: [(&str, &str); 5] = [
    ("posters", "poster"),
    ("backdrops", "fanart"),
    ("logos", "logo"),
    ("stills", "still"),
    ("profiles", "photo"),
];

/// An SVG is asked as the PNG rendition TMDB serves for it.
fn original(path: &str) -> String {
    let path = match path.len().checked_sub(4) {
        Some(i) if path.is_char_boundary(i) && path[i..].eq_ignore_ascii_case(".svg") => {
            format!("{}.png", &path[..i])
        }
        _ => path.to_owned(),
    };
    format!("{IMAGE_URL}/original{path}")
}

pub fn candidates(images: &Value) -> Vec<ImageCandidate> {
    let dim = |i: &Value, k: &str| i.get(k).and_then(Value::as_u64).map(|n| n as u32);
    let mut out = Vec::new();
    for (key, slot) in SLOTS {
        for i in arr(images, key) {
            let Some(p) = text(i, "file_path") else {
                continue;
            };
            out.push(ImageCandidate {
                url: original(p),
                slot: slot.into(),
                language: text(i, "iso_639_1").map(str::to_owned),
                width: dim(i, "width"),
                height: dim(i, "height"),
                score: i.get("vote_average").and_then(Value::as_f64),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn images_cover_every_slot_and_language() {
        let v = json!({
            "posters": [{"file_path": "/p.jpg", "iso_639_1": "fr", "width": 1000, "height": 1500, "vote_average": 5.5}],
            "backdrops": [{"file_path": "/b.jpg", "iso_639_1": null, "width": 1920, "height": 1080}],
            "logos": [{"file_path": "/l.svg", "iso_639_1": "en"}],
            "stills": [{"file_path": "/s.jpg"}],
            "profiles": [{"file_path": "/h.jpg"}, {"file_path": ""}]
        });
        let c = candidates(&v);
        let slots: Vec<_> = c.iter().map(|i| i.slot.as_str()).collect();
        assert_eq!(slots, ["poster", "fanart", "logo", "still", "photo"]);
        assert_eq!(c[0].url, "https://image.tmdb.org/t/p/original/p.jpg");
        assert_eq!(
            (c[0].language.as_deref(), c[0].width, c[0].score),
            (Some("fr"), Some(1000), Some(5.5))
        );
        assert_eq!(c[1].language, None);
        assert!(c[2].url.ends_with("/l.png"));
    }

    #[test]
    fn images_paths_follow_the_scope() {
        assert_eq!(
            path(false, 3, &ImageScope::Work).unwrap(),
            "/movie/3/images"
        );
        assert_eq!(
            path(true, 3, &ImageScope::Episode((2, 4))).unwrap(),
            "/tv/3/season/2/episode/4/images"
        );
        assert!(path(false, 3, &ImageScope::Season(1)).is_none());
        assert!(path(true, 3, &ImageScope::Edition).is_none());
    }
}
