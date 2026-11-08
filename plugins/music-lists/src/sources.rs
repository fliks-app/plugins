//! What each list source reads: ListenBrainz, Last.fm, Spotify, Deezer and MusicBrainz collections.
use crate::model::{Entry, Rec, Settings, last_segment, plain_id, required, text, uuid};
use crate::net::Net;
use serde_json::Value;
use url::Url;

pub type R<T> = Result<T, String>;

const MAX_ENTRIES: usize = 2000;
const LISTENBRAINZ: &str = "https://api.listenbrainz.org";
const LASTFM: &str = "https://ws.audioscrobbler.com/2.0/";
const SPOTIFY: &str = "https://api.spotify.com/v1";
const DEEZER: &str = "https://api.deezer.com";
const MUSICBRAINZ: &str = "https://musicbrainz.org/ws/2";

fn arr<'a>(v: &'a Value, path: &str) -> &'a [Value] {
    v.pointer(path)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn s<'a>(v: &'a Value, path: &str) -> Option<&'a str> {
    v.pointer(path)?.as_str().filter(|s| !s.is_empty())
}

fn url(base: &str, path: &str, query: &[(&str, &str)]) -> R<String> {
    Url::parse_with_params(&format!("{base}{path}"), query)
        .map(Into::into)
        .map_err(|_| "not a valid URL".to_owned())
}

fn getj(net: &impl Net, u: &str, h: &[(&str, String)]) -> R<Value> {
    Ok(net.get(u, h)?.unwrap_or(Value::Null))
}

pub fn fetch(net: &impl Net, imp: &str, st: &Settings, token: Option<&str>) -> R<Vec<Entry>> {
    let mut out = match imp {
        "listenbrainz" => listenbrainz(net, st)?,
        "lastfm" => lastfm(net, st)?,
        "spotify" => spotify(net, st, token.ok_or("Spotify is not connected")?)?,
        "deezer" => deezer(net, st)?,
        "musicbrainz" => collection(net, st)?,
        _ => return Err(format!("unknown implementation {imp}")),
    };
    let mut seen = std::collections::HashSet::new();
    out.retain(|e| seen.insert(e.key.clone()));
    out.truncate(MAX_ENTRIES);
    Ok(out)
}

// --- ListenBrainz ---------------------------------------------------------------------------

/// The recording ids of a JSPF playlist.
pub fn jspf_recordings(playlist: &Value) -> Vec<Entry> {
    arr(playlist, "/playlist/track")
        .iter()
        .filter_map(|t| {
            let id = match t.get("identifier")? {
                Value::String(u) => u.clone(),
                Value::Array(a) => a.iter().find_map(|x| x.as_str().map(str::to_owned))?,
                _ => return None,
            };
            uuid(&last_segment(&id))
        })
        .map(|m| Entry::track(Rec::Mbid(m), None))
        .collect()
}

fn listenbrainz(net: &impl Net, st: &Settings) -> R<Vec<Entry>> {
    let user = required(st, "username")?;
    let user = url::form_urlencoded::byte_serialize(user.as_bytes()).collect::<String>();
    let token = text(st, "token");
    let h: Vec<(&str, String)> = Some(token)
        .filter(|t| !t.is_empty())
        .map(|t| ("authorization", format!("Token {t}")))
        .into_iter()
        .collect();
    let playlist = |id: &str| -> R<Vec<Entry>> {
        let id = uuid(&last_segment(id)).ok_or("not a playlist id")?;
        Ok(jspf_recordings(&getj(
            net,
            &url(LISTENBRAINZ, &format!("/1/playlist/{id}"), &[])?,
            &h,
        )?))
    };
    match text(st, "source").as_str() {
        "playlist" => playlist(&required(st, "playlistId")?),
        "created-for-you" => {
            let list = getj(
                net,
                &url(
                    LISTENBRAINZ,
                    &format!("/1/user/{user}/playlists/createdfor"),
                    &[("count", "10")],
                )?,
                &h,
            )?;
            let mut out = vec![];
            for p in arr(&list, "/playlists") {
                if let Some(id) = s(p, "/playlist/identifier") {
                    out.extend(playlist(id)?);
                }
            }
            Ok(out)
        }
        "top-albums" => {
            let v = getj(
                net,
                &url(
                    LISTENBRAINZ,
                    &format!("/1/stats/user/{user}/release-groups"),
                    &[("count", "100"), ("range", "all_time")],
                )?,
                &h,
            )?;
            Ok(arr(&v, "/payload/release_groups")
                .iter()
                .filter_map(|r| uuid(s(r, "/release_group_mbid")?))
                .map(|id| Entry::album("musicbrainz", &id))
                .collect())
        }
        "top-tracks" => {
            let v = getj(
                net,
                &url(
                    LISTENBRAINZ,
                    &format!("/1/stats/user/{user}/recordings"),
                    &[("count", "100"), ("range", "all_time")],
                )?,
                &h,
            )?;
            Ok(arr(&v, "/payload/recordings")
                .iter()
                .filter_map(|r| uuid(s(r, "/recording_mbid")?))
                .map(|m| Entry::track(Rec::Mbid(m), None))
                .collect())
        }
        "loved-tracks" => {
            let mut out = vec![];
            for page in 0..10 {
                let off = (page * 100).to_string();
                let v = getj(
                    net,
                    &url(
                        LISTENBRAINZ,
                        &format!("/1/feedback/user/{user}/get-feedback"),
                        &[("score", "1"), ("count", "100"), ("offset", &off)],
                    )?,
                    &h,
                )?;
                let rows = arr(&v, "/feedback");
                out.extend(
                    rows.iter()
                        .filter_map(|r| uuid(s(r, "/recording_mbid")?))
                        .map(|m| Entry::track(Rec::Mbid(m), None)),
                );
                if rows.len() < 100 {
                    break;
                }
            }
            Ok(out)
        }
        other => Err(format!("unknown ListenBrainz source {other:?}")),
    }
}

// --- Last.fm --------------------------------------------------------------------------------

/// Last.fm tracks that carry a recording id; the others cannot be told apart from a title alone.
pub fn lastfm_tracks(page: &Value, key: &str) -> Vec<Entry> {
    let tracks = match page.get(key).and_then(|k| k.get("track")) {
        Some(Value::Array(a)) => a.as_slice(),
        Some(t @ Value::Object(_)) => std::slice::from_ref(t),
        _ => &[],
    };
    tracks
        .iter()
        .filter_map(|t| uuid(s(t, "/mbid")?))
        .map(|m| Entry::track(Rec::Mbid(m), None))
        .collect()
}

fn lastfm(net: &impl Net, st: &Settings) -> R<Vec<Entry>> {
    let (user, key) = (required(st, "username")?, required(st, "apiKey")?);
    let (method, root) = match text(st, "source").as_str() {
        "loved-tracks" => ("user.getlovedtracks", "lovedtracks"),
        "top-tracks" => ("user.gettoptracks", "toptracks"),
        other => return Err(format!("unknown Last.fm source {other:?}")),
    };
    let mut out = vec![];
    for page in 1..=5 {
        let p = page.to_string();
        let u = url(
            LASTFM,
            "",
            &[
                ("method", method),
                ("user", &user),
                ("api_key", &key),
                ("format", "json"),
                ("limit", "200"),
                ("page", &p),
            ],
        )?;
        let v = getj(net, &u, &[])?;
        if let Some(e) = s(&v, "/message") {
            return Err(format!("Last.fm: {e}"));
        }
        out.extend(lastfm_tracks(&v, root));
        let pages: u32 = s(&v, &format!("/{root}/@attr/totalPages"))
            .and_then(|t| t.parse().ok())
            .unwrap_or(1);
        if page >= pages {
            break;
        }
    }
    Ok(out)
}

// --- Spotify --------------------------------------------------------------------------------

/// A Spotify track as an entry: its ISRC when it has one, its album always.
pub fn spotify_track(t: &Value) -> Option<Entry> {
    let album = plain_id(s(t, "/album/id")?);
    let isrc = s(t, "/external_ids/isrc").and_then(|i| plain_id(&i.to_ascii_uppercase()));
    match (isrc, album) {
        (Some(i), a) => Some(Entry::track(
            Rec::Isrc(i),
            a.as_deref().map(|a| ("spotify", a)),
        )),
        (None, Some(a)) => Some(Entry::album("spotify", &a)),
        _ => None,
    }
}

fn spotify(net: &impl Net, st: &Settings, token: &str) -> R<Vec<Entry>> {
    let h = [("authorization", format!("Bearer {token}"))];
    let (first, tracks) = match text(st, "source").as_str() {
        "playlist" => {
            let id =
                plain_id(&last_segment(&required(st, "playlistId")?)).ok_or("not a playlist id")?;
            (
                url(
                    SPOTIFY,
                    &format!("/playlists/{id}/tracks"),
                    &[("limit", "100")],
                )?,
                true,
            )
        }
        "saved-albums" => (url(SPOTIFY, "/me/albums", &[("limit", "50")])?, false),
        "saved-tracks" => (url(SPOTIFY, "/me/tracks", &[("limit", "50")])?, true),
        other => return Err(format!("unknown Spotify source {other:?}")),
    };
    let (mut out, mut next) = (vec![], Some(first));
    for _ in 0..40 {
        let Some(u) = next.take() else { break };
        let v = getj(net, &u, &h)?;
        for it in arr(&v, "/items") {
            let e = if tracks {
                it.get("track").and_then(spotify_track)
            } else {
                s(it, "/album/id")
                    .and_then(plain_id)
                    .map(|a| Entry::album("spotify", &a))
            };
            out.extend(e);
        }
        next = s(&v, "/next")
            .filter(|n| n.starts_with(SPOTIFY))
            .map(str::to_owned);
    }
    Ok(out)
}

// --- Deezer ---------------------------------------------------------------------------------

fn deezer(net: &impl Net, st: &Settings) -> R<Vec<Entry>> {
    let id = last_segment(&required(st, "id")?);
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err("not a Deezer id".into());
    }
    let (path, tracks) = match text(st, "source").as_str() {
        "playlist" => (format!("/playlist/{id}/tracks"), true),
        "user-albums" => (format!("/user/{id}/albums"), false),
        other => return Err(format!("unknown Deezer source {other:?}")),
    };
    let (mut out, mut next) = (vec![], Some(url(DEEZER, &path, &[("limit", "100")])?));
    for _ in 0..20 {
        let Some(u) = next.take() else { break };
        let v = getj(net, &u, &[])?;
        if let Some(e) = s(&v, "/error/message") {
            return Err(format!("Deezer: {e}"));
        }
        for it in arr(&v, "/data") {
            let (Some(i), album) = (
                it.get("id").and_then(Value::as_u64),
                it.pointer("/album/id").and_then(Value::as_u64),
            ) else {
                continue;
            };
            if tracks {
                let Some(a) = album else { continue };
                let mut e = Entry::album("deezer", &a.to_string());
                e.key = format!("deezer:track:{i}");
                e.deezer_track = Some(i.to_string());
                out.push(e);
            } else {
                out.push(Entry::album("deezer", &i.to_string()));
            }
        }
        next = s(&v, "/next")
            .filter(|n| n.starts_with(DEEZER))
            .map(str::to_owned);
    }
    Ok(out)
}

/// The ISRC of a Deezer track.
pub fn deezer_isrc(net: &impl Net, track: &str) -> Option<String> {
    let v = net.get(&format!("{DEEZER}/track/{track}"), &[]).ok()??;
    s(&v, "/isrc").and_then(|i| plain_id(&i.to_ascii_uppercase()))
}

// --- MusicBrainz collections ---------------------------------------------------------------

fn collection(net: &impl Net, st: &Settings) -> R<Vec<Entry>> {
    let id = uuid(&last_segment(&required(st, "collectionId")?)).ok_or("not a collection id")?;
    let groups = text(st, "entity") != "recording";
    let (path, key) = if groups {
        ("release-groups", "release-groups")
    } else {
        ("recordings", "recordings")
    };
    let mut out = vec![];
    for page in 0..20u32 {
        let off = (page * 100).to_string();
        let u = url(
            MUSICBRAINZ,
            &format!("/collection/{id}/{path}"),
            &[("fmt", "json"), ("limit", "100"), ("offset", &off)],
        )?;
        if page > 0 {
            net.pause(1100);
        }
        let v = getj(net, &u, &[])?;
        let rows = arr(&v, &format!("/{key}"));
        out.extend(rows.iter().filter_map(|r| uuid(s(r, "/id")?)).map(|i| {
            if groups {
                Entry::album("musicbrainz", &i)
            } else {
                Entry::track(Rec::Mbid(i), None)
            }
        }));
        if rows.len() < 100 {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const M1: &str = "11111111-1111-4111-8111-111111111111";
    const M2: &str = "22222222-2222-4222-8222-222222222222";

    #[test]
    fn a_jspf_playlist_yields_its_recordings() {
        let v = json!({"playlist": {"track": [
            {"identifier": [format!("https://musicbrainz.org/recording/{M1}")]},
            {"identifier": format!("https://musicbrainz.org/recording/{M2}")},
            {"identifier": ["https://example.org/nope"]},
        ]}});
        let keys: Vec<_> = jspf_recordings(&v).into_iter().map(|e| e.key).collect();
        assert_eq!(keys, [format!("recording:{M1}"), format!("recording:{M2}")]);
    }

    #[test]
    fn last_fm_keeps_the_tracks_with_a_recording_id() {
        let one = json!({"lovedtracks": {"track": {"mbid": M1}}});
        assert_eq!(lastfm_tracks(&one, "lovedtracks").len(), 1);
        let many = json!({"toptracks": {"track": [{"mbid": M1}, {"mbid": ""}, {"name": "x"}]}});
        assert_eq!(lastfm_tracks(&many, "toptracks").len(), 1);
    }

    #[test]
    fn a_spotify_track_prefers_its_isrc_and_keeps_its_album() {
        let t =
            json!({"id": "t", "external_ids": {"isrc": "usaaa2400001"}, "album": {"id": "alb1"}});
        let e = spotify_track(&t).unwrap();
        assert_eq!(e.rec, Some(Rec::Isrc("USAAA2400001".into())));
        assert_eq!(e.albums, [("spotify".to_owned(), "alb1".to_owned())]);
        let bare = json!({"album": {"id": "alb2"}});
        assert_eq!(spotify_track(&bare).unwrap().key, "spotify:alb2");
        assert!(spotify_track(&json!({"id": "local"})).is_none());
    }

    struct Fake(Vec<(&'static str, Value)>);

    impl Net for Fake {
        fn get(&self, u: &str, _: crate::net::Headers) -> R<Option<Value>> {
            self.0
                .iter()
                .find(|(p, _)| u.contains(p))
                .map(|(_, v)| Some(v.clone()))
                .ok_or_else(|| format!("unexpected {u}"))
        }
        fn post_form(&self, _: &str, _: crate::net::Headers, _: &[(&str, &str)]) -> R<Value> {
            Err("none".into())
        }
        fn pause(&self, _: u64) {}
    }

    fn st(v: Value) -> Settings {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn deezer_tracks_carry_their_album_and_are_deduplicated() {
        let net = Fake(vec![(
            "/playlist/9/tracks",
            json!({"data": [
            {"id": 1, "album": {"id": 10}}, {"id": 1, "album": {"id": 10}}, {"id": 2, "album": {"id": 10}}]}),
        )]);
        let e = fetch(
            &net,
            "deezer",
            &st(json!({"source": "playlist", "id": "https://www.deezer.com/fr/playlist/9"})),
            None,
        )
        .unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].albums, [("deezer".to_owned(), "10".to_owned())]);
        assert_eq!(e[1].deezer_track.as_deref(), Some("2"));
        assert!(
            fetch(
                &net,
                "deezer",
                &st(json!({"source": "playlist", "id": "../x"})),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn a_collection_is_read_as_albums_or_recordings() {
        let net = Fake(vec![
            ("/release-groups", json!({"release-groups": [{"id": M1}]})),
            ("/recordings", json!({"recordings": [{"id": M2}]})),
        ]);
        let s1 = st(json!({"collectionId": M1}));
        assert_eq!(
            fetch(&net, "musicbrainz", &s1, None).unwrap()[0].key,
            format!("musicbrainz:{M1}")
        );
        let s2 = st(json!({"collectionId": M1, "entity": "recording"}));
        assert_eq!(
            fetch(&net, "musicbrainz", &s2, None).unwrap()[0].key,
            format!("recording:{M2}")
        );
    }

    #[test]
    fn listenbrainz_stats_become_albums() {
        let net = Fake(vec![(
            "/stats/user/me/release-groups",
            json!({"payload": {"release_groups": [{"release_group_mbid": M1}]}}),
        )]);
        let e = fetch(
            &net,
            "listenbrainz",
            &st(json!({"username": "me", "source": "top-albums"})),
            None,
        )
        .unwrap();
        assert_eq!(e[0].albums, [("musicbrainz".to_owned(), M1.to_owned())]);
    }
}
