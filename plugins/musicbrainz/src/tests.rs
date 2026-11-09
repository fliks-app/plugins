//! The lookups against recorded bodies through a fake transport (no network).
use crate::api::*;
use crate::map::{self, Target};
use fliks_plugin_sdk_provider::ProviderError;
use serde_json::{Value, json};
use std::cell::RefCell;

const GROUP: &str = "11111111-1111-4111-8111-111111111111";
const RELEASE: &str = "44444444-4444-4444-8444-444444444444";
const ARTIST: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

fn fx(name: &str) -> Value {
    let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[derive(Default)]
struct Fake {
    /// Every `query` text asked of MusicBrainz.
    queries: RefCell<Vec<String>>,
    popularity: Option<Value>,
}

impl Fake {
    fn queries(&self) -> Vec<String> {
        self.queries.borrow().clone()
    }
}

impl Http for Fake {
    fn mb(&self, path: &str, query: &[(&str, String)]) -> R<Value> {
        self.queries.borrow_mut().extend(
            query
                .iter()
                .filter(|(k, _)| *k == "query")
                .map(|(_, v)| v.clone()),
        );
        let has = |k: &str| query.iter().any(|(n, _)| *n == k);
        Ok(match path {
            "/artist" => fx("search_artist.json"),
            "/recording" => fx("search_recording.json"),
            "/release-group" if has("query") => fx("search.json"),
            "/release-group" => fx("rg_browse.json"),
            "/release" => fx("releases.json"),
            "/url" => {
                let r = query.iter().find(|(k, _)| *k == "resource").unwrap();
                if r.1.contains("0000") {
                    fx("url.json")
                } else {
                    return Err(ProviderError::NotFound);
                }
            }
            "/isrc/USAAA2400001" => fx("isrc.json"),
            p if p == format!("/release-group/{GROUP}") => fx("group.json"),
            p if p == format!("/release/{RELEASE}") && has("inc") => {
                if query.iter().any(|(_, v)| v.contains("recordings")) {
                    fx("release.json")
                } else {
                    fx("release_group.json")
                }
            }
            p if p == format!("/artist/{ARTIST}") => fx("artist.json"),
            _ => return Err(ProviderError::NotFound),
        })
    }

    fn lb(&self, _path: &str, _body: &Value) -> Option<Value> {
        self.popularity.clone()
    }
}

fn ids(v: &[Value]) -> Vec<&str> {
    v.iter()
        .map(|a| a["externalId"].as_str().unwrap())
        .collect()
}

#[test]
fn search_reranks_by_exact_title_type_and_releases() {
    let h = Fake::default();
    let hits = search(
        &h,
        "Placeholder Album",
        None,
        Some("Example Artist"),
        None,
        Some(2004),
        1,
    )
    .unwrap();
    // The exact title as an album comes before the live variant the search ranked first.
    assert_eq!(
        ids(&hits),
        [
            GROUP,
            "33333333-3333-4333-8333-333333333333",
            "22222222-2222-4222-8222-222222222222"
        ]
    );
    let q = h.queries().remove(0);
    assert!(q.contains("releasegroup:(Placeholder Album)"));
    assert!(q.contains("artist:\"Example Artist\""));
    assert!(q.contains("firstreleasedate:2004"));
    assert!(!q.contains("firstreleasedate:2004*"));
}

#[test]
fn a_hit_is_an_album_record() {
    let h = Fake::default();
    let a = &search(&h, "Placeholder Album", None, None, None, None, 1).unwrap()[0];
    assert_eq!(a["externalId"], GROUP);
    assert_eq!(a["primaryType"], "album");
    assert_eq!(a["secondaryTypes"], json!([]));
    assert_eq!(a["firstReleaseDate"], "2004-05-01");
    assert_eq!(a["releaseCount"], 12);
    assert_eq!(a["mediaType"], "album");
    assert_eq!(
        a["posterUrl"],
        format!("https://coverartarchive.org/release-group/{GROUP}/front-500")
    );
    assert_eq!(
        a["sourceUrl"],
        format!("https://musicbrainz.org/release-group/{GROUP}")
    );
    assert_eq!(a["artists"][0]["joinPhrase"], " feat. ");
    assert_eq!(a["artists"][1]["name"], "Guest Person");
    assert_eq!(a["artists"][1]["creditedName"], "Guest");
    assert_eq!(a["artists"][1]["sortName"], "Person, Guest");
    assert!(a["artists"][0].get("creditedName").is_none());
}

#[test]
fn author_id_wins_over_the_author_name() {
    let h = Fake::default();
    search(&h, "x", None, Some("Name"), Some(ARTIST), None, 1).unwrap();
    let q = h.queries().remove(0);
    assert!(q.contains(&format!("arid:{ARTIST}")) && !q.contains("artist:"));
}

#[test]
fn listenbrainz_popularity_lifts_a_hit_and_is_reported() {
    let h = Fake {
        popularity: Some(json!([
            {"release_group_mbid": "33333333-3333-4333-8333-333333333333", "total_listen_count": 9_000_000.0},
        ])),
        ..Fake::default()
    };
    let hits = search(&h, "Placeholder Album", None, None, None, None, 1).unwrap();
    assert_eq!(
        hits[0]["externalId"],
        "33333333-3333-4333-8333-333333333333"
    );
    assert_eq!(hits[0]["popularity"], 9_000_000.0);
    assert!(hits[1].get("popularity").is_none());
}

#[test]
fn search_text_is_escaped() {
    assert_eq!(map::escape(r#"a+b "c" (d)/e"#), r#"a\+b \"c\" \(d\)\/e"#);
}

#[test]
fn targets_are_told_from_text() {
    assert_eq!(map::target(GROUP), Target::Id(GROUP.into()));
    assert_eq!(
        map::target(&format!(
            "https://musicbrainz.org/release-group/{}?x=1",
            GROUP.to_uppercase()
        )),
        Target::Group(GROUP.into())
    );
    assert_eq!(
        map::target(&format!("https://beta.musicbrainz.org/release/{RELEASE}")),
        Target::Release(RELEASE.into())
    );
    assert_eq!(
        map::target(&format!("https://musicbrainz.org/artist/{ARTIST}")),
        Target::Artist(ARTIST.into())
    );
    assert_eq!(map::target("https://example.org/artist/x"), Target::Text);
    assert_eq!(map::target("https://musicbrainz.org/label/x"), Target::Text);
    assert_eq!(map::target("placeholder"), Target::Text);
}

#[test]
fn a_group_url_a_release_url_and_a_bare_id_resolve() {
    let h = Fake::default();
    let url = format!("https://musicbrainz.org/release-group/{GROUP}");
    assert_eq!(
        ids(&search(&h, &url, None, None, None, None, 1).unwrap()),
        [GROUP]
    );
    let rel = format!("https://musicbrainz.org/release/{RELEASE}");
    assert_eq!(
        ids(&search(&h, &rel, None, None, None, None, 1).unwrap()),
        [GROUP]
    );
    // A bare release id is no group, then a release.
    assert_eq!(
        ids(&search(&h, RELEASE, None, None, None, None, 1).unwrap()),
        [GROUP]
    );
    // A bare artist id is neither: its release groups.
    let a = search(&h, ARTIST, None, None, None, None, 1).unwrap();
    assert_eq!(ids(&a), [GROUP]);
}

#[test]
fn work_maps_the_group() {
    let a = work(&Fake::default(), GROUP).unwrap();
    assert_eq!(a["secondaryTypes"], json!(["compilation"]));
    assert!(
        a.get("overview").is_none(),
        "the annotation is editor notes"
    );
    assert_eq!(a["genres"], json!(["electronic", "ambient", "idm"]));
    assert_eq!(a["releaseCount"], 2);
    assert!(matches!(
        work(&Fake::default(), "nope"),
        Err(ProviderError::NotFound)
    ));
}

#[test]
fn track_popularity_comes_from_one_recording_batch() {
    let h = Fake {
        popularity: Some(json!([
            {"recording_mbid": "77777777-7777-4777-8777-777777777777", "total_listen_count": 1234.0},
        ])),
        ..Fake::default()
    };
    let t = edition(&h, RELEASE).unwrap()["tracks"].clone();
    assert_eq!(t[0]["popularity"], 1234.0);
    assert!(t[1].get("popularity").is_none());
    let none = edition(&Fake::default(), RELEASE).unwrap();
    assert!(none["tracks"][0].get("popularity").is_none());
}

#[test]
fn children_are_the_releases() {
    let c = children(&Fake::default(), GROUP).unwrap();
    assert_eq!(c.len(), 3);
    assert_eq!(c[0]["country"], "XW");
    assert_eq!(c[0]["status"], "official");
    assert_eq!(c[0]["media"], "cd");
    assert_eq!(c[0]["barcode"], "012345678905");
    assert_eq!(c[0]["catalogNumber"], "CAT-1");
    assert_eq!(c[0]["label"], "Example Label");
    assert_eq!(c[0]["trackCount"], 3);
    assert_eq!(c[0]["discCount"], 2);
    assert_eq!(c[1]["country"], "FR");
    assert_eq!(c[1]["status"], "pseudo-release");
    assert_eq!(c[1]["media"], "vinyl");
    assert!(c[1].get("barcode").is_none());
}

#[test]
fn the_edition_carries_the_tracklist_with_credits_and_works() {
    let e = edition(&Fake::default(), RELEASE).unwrap();
    assert_eq!(e["externalId"], RELEASE);
    assert_eq!(e["language"], "eng");
    assert_eq!(
        e["coverUrl"],
        format!("https://coverartarchive.org/release/{RELEASE}/front-500")
    );
    let t = e["tracks"].as_array().unwrap();
    assert_eq!(t.len(), 3);
    let first = &t[0];
    assert_eq!(first["discNumber"], 1);
    assert_eq!(first["discTitle"], "First disc");
    assert_eq!(first["position"], 1);
    assert_eq!(first["number"], 1);
    assert_eq!(first["durationMs"], 181_000);
    assert_eq!(first["isrc"], "USAAA2400001");
    assert_eq!(first["recordingId"], "77777777-7777-4777-8777-777777777777");
    assert_eq!(
        first["releaseTrackId"],
        "66666666-6666-4666-8666-666666666666"
    );
    assert_eq!(first["artists"][0]["name"], "Example Artist");
    let credits: Vec<(&str, &str, Option<&str>)> = first["credits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap(),
                c["role"].as_str().unwrap(),
                c["instrument"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        credits,
        [
            ("Guest Person", "performer", Some("guitar, bass guitar")),
            ("Example Artist", "performer", Some("vocals")),
            ("Placeholder Conductor", "conductor", None),
            ("Placeholder Composer", "composer", None),
            ("Placeholder Lyricist", "lyricist", None),
        ]
    );
    let w = &first["works"][0];
    assert_eq!(w["title"], "Placeholder Movement");
    assert_eq!(w["parentTitle"], "Placeholder Suite");
    assert_eq!(w["parentId"], "99999999-9999-4999-8999-999999999999");
    assert_eq!(w["composer"]["name"], "Placeholder Composer");
    // The printed number `A2` is a label; a zero length is no duration; the disc restarts the numbers, not the position.
    assert_eq!(t[1]["label"], "A2");
    assert!(t[1].get("number").is_none() && t[1].get("durationMs").is_none());
    assert_eq!(
        (t[2]["discNumber"].as_u64(), t[2]["position"].as_u64()),
        (Some(2), Some(3))
    );
}

#[test]
fn the_person_is_the_artist() {
    let p = person(&Fake::default(), ARTIST).unwrap();
    assert_eq!(p["id"], ARTIST);
    assert_eq!(p["type"], "group");
    assert_eq!(p["birthday"], "1999-04");
    assert_eq!(p["deathday"], "2010");
    assert_eq!(p["placeOfBirth"], "Example City");
    assert_eq!(p["biography"], "A placeholder biography.");
    assert_eq!(p["disambiguation"], "placeholder");
}

#[test]
fn credits_are_the_discography_without_broken_entries() {
    let c = credits(&Fake::default(), ARTIST).unwrap();
    assert_eq!(ids(c["albums"].as_array().unwrap()), [GROUP]);
}

#[test]
fn find_by_resolves_urls_and_isrc() {
    let h = Fake::default();
    // A Spotify URL attached to a release resolves through the release's group.
    assert_eq!(
        find_by(&h, "spotify", "0000000000000000000000")
            .unwrap()
            .as_deref(),
        Some(GROUP)
    );
    assert_eq!(find_by(&h, "deezer", "42").unwrap(), None);
    // The official plain album wins over a single, a compilation and a bootleg album.
    assert_eq!(
        find_by(&h, "isrc", "US-AAA-24-00001").unwrap().as_deref(),
        Some(GROUP)
    );
    assert_eq!(find_by(&h, "isrc", "USZZZ2400001").unwrap(), None);
    assert_eq!(find_by(&h, "imdb", "tt1").unwrap(), None);
}

#[test]
fn replies_map_to_provider_errors() {
    assert!(matches!(
        reply(503, Some("7"), b"{}"),
        Err(ProviderError::RateLimited(Some(7)))
    ));
    assert!(matches!(
        reply(404, None, b"{}"),
        Err(ProviderError::NotFound)
    ));
    assert!(matches!(
        reply(500, None, b"{\"error\":\"x\"}"),
        Err(ProviderError::Upstream(_))
    ));
    assert!(reply(200, None, b"{\"a\":1}").is_ok());
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn every_record_passes_the_host_checks() {
    use fliks_domain::music::provider as host;
    let h = Fake::default();
    for a in search(&h, "Placeholder Album", None, None, None, None, 1).unwrap() {
        host::album(&a).unwrap();
    }
    host::album(&work(&h, GROUP).unwrap()).unwrap();
    let kids = children(&h, GROUP).unwrap();
    assert_eq!(host::releases(&kids).kept.len(), 2);
    let e = host::edition(&edition(&h, RELEASE).unwrap()).unwrap();
    assert_eq!((e.tracks.len(), e.dropped), (3, 0));
    assert_eq!(e.tracks[0].credits.len(), 6);
    assert_eq!(
        e.tracks[0].works[0].composer.as_ref().unwrap().role,
        "composer"
    );
    host::person(&person(&h, ARTIST).unwrap()).unwrap();
    let c = credits(&h, ARTIST).unwrap();
    assert_eq!(host::albums(c["albums"].as_array().unwrap()).kept.len(), 1);
}

#[test]
fn an_artist_search_keeps_the_records_with_an_id_and_a_name() {
    let h = Fake::default();
    let hits = search_artists(&h, "Example (Artist)", 2).unwrap();
    assert_eq!(ids(&hits), [ARTIST, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"]);
    assert_eq!(
        (
            hits[0]["artistType"].clone(),
            hits[0]["country"].clone(),
            hits[0]["sortName"].clone()
        ),
        (json!("group"), json!("GB"), json!("Artist, Example"))
    );
    assert_eq!(hits[0]["disambiguation"], "placeholder band");
    assert!(hits[1].get("country").is_none());
    assert_eq!(h.queries(), [r"Example \(Artist\)"]);
}

#[test]
fn a_recording_is_listed_under_its_best_release_group_and_once_per_title() {
    let h = Fake::default();
    let hits = search_recordings(&h, "placeholder song", 1).unwrap();
    // The orphan has no release group, and the second record repeats the title of the first.
    assert_eq!(hits.len(), 1);
    let r = &hits[0];
    assert_eq!(r["externalId"], "dddddddd-dddd-4ddd-8ddd-dddddddddddd");
    assert_eq!(
        (r["title"].clone(), r["durationMs"].clone()),
        (json!("Placeholder Song"), json!(183000))
    );
    assert_eq!(r["artists"][0]["name"], "Example Artist");
    let a = &r["album"];
    assert_eq!(
        a["externalId"], "22222222-aaaa-4aaa-8aaa-000000000003",
        "official plain album first"
    );
    assert_eq!(
        (a["title"].clone(), a["mediaType"].clone()),
        (json!("Placeholder Album"), json!("album"))
    );
    assert_eq!(a["firstReleaseDate"], "2004-05-01");
    assert_eq!(a["artists"][0]["name"], "Example Artist");
    assert!(
        a["posterUrl"]
            .as_str()
            .unwrap()
            .contains("coverartarchive.org/release-group/")
    );
}
