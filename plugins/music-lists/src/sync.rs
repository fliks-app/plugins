//! Runs one list: reads its entries, acts on the new ones through the host and mirrors the playlist.
//! The plugin never resolves a title itself: the host does (`media-resolve-or-create`, `music-want-recording`,
//! `music-find-recording`).
use crate::model::{Entry, Rec, Settings, actions, library};
use std::collections::HashMap;

pub struct HErr {
    pub code: String,
    pub message: String,
}

pub trait Host {
    fn album(&self, ids: &[(String, String)], library: Option<u32>) -> Result<u32, HErr>;
    fn want(&self, rec: &Rec, library: Option<u32>) -> Result<(u32, u32), HErr>;
    /// The track the library already owns, without creating or monitoring anything.
    fn find(&self, rec: &Rec) -> Result<Option<(u32, u32)>, HErr>;
    fn playlist(&self, user: u32, name: &str, items: &[u32]) -> Result<u32, HErr>;
    fn isrc_of(&self, deezer_track: &str) -> Option<String>;
    fn out_of_time(&self) -> bool;
}

pub struct Known {
    pub item: Option<u32>,
}

pub trait Store {
    /// The entries already acted on with these actions; one that resolved to nothing is retried after a week.
    fn known(&self, list: i64, actions: &str) -> Result<HashMap<String, Known>, String>;
    fn save(
        &self,
        list: i64,
        key: &str,
        actions: &str,
        media: Option<u32>,
        item: Option<u32>,
    ) -> Result<(), String>;
}

pub struct Ctx<'a> {
    pub id: i64,
    pub name: &'a str,
    pub owner: Option<u32>,
    pub settings: &'a Settings,
}

#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    pub total: usize,
    pub done: usize,
    pub unresolved: usize,
    pub errors: usize,
    pub playlist: Option<u32>,
    pub complete: bool,
    pub last_error: Option<String>,
}

/// An answer that will not change on a retry soon: the title is unknown or the request is refused.
fn permanent(code: &str) -> bool {
    matches!(
        code,
        "ERR_NOT_FOUND"
            | "ERR_VALIDATION"
            | "ERR_CONFLICT"
            | "unsupported-kind"
            | "unsupported-provider"
            | "unsupported-family"
    )
}

pub fn run(
    c: &Ctx,
    entries: &[Entry],
    host: &impl Host,
    store: &impl Store,
) -> Result<Summary, String> {
    let acts = actions(c.settings);
    if acts.is_empty() {
        return Err("no action is selected".into());
    }
    let has = |a: &str| acts.iter().any(|x| x == a);
    let (albums, tracks, playlist) = (has("albums"), has("tracks"), has("playlist"));
    if playlist && c.owner.is_none() {
        return Err("the playlist needs a user: save the list from the settings page".into());
    }
    let (sig, lib) = (acts.join(","), library(c.settings));
    let known = store.known(c.id, &sig)?;
    let mut sum = Summary {
        total: entries.len(),
        complete: true,
        ..Summary::default()
    };
    let mut items = vec![];
    let mut streak = 0;
    for e in entries {
        if let Some(k) = known.get(&e.key) {
            items.extend(k.item);
            sum.done += 1;
            continue;
        }
        if streak >= 5 || host.out_of_time() {
            sum.complete = false;
            continue;
        }
        let (mut media, mut item, mut err) = (None, None, None);
        if albums && !e.albums.is_empty() {
            match host.album(&e.albums, lib) {
                Ok(m) => media = Some(m),
                Err(x) => err = Some(x),
            }
        }
        if err.as_ref().is_none_or(|x| permanent(&x.code)) && (tracks || playlist) {
            let rec = e.rec.clone().or_else(|| {
                e.deezer_track
                    .as_deref()
                    .and_then(|t| host.isrc_of(t))
                    .map(Rec::Isrc)
            });
            if let Some(r) = rec {
                let found = if tracks {
                    host.want(&r, lib).map(Some)
                } else {
                    host.find(&r)
                };
                match found {
                    Ok(Some((m, i))) => (media, item, err) = (Some(m), Some(i), None),
                    Ok(None) => err = None,
                    Err(x) => err = Some(x),
                }
            }
        }
        match err {
            Some(x) if !permanent(&x.code) => {
                streak += 1;
                sum.errors += 1;
                sum.last_error = Some(x.message);
                continue;
            }
            _ => streak = 0,
        }
        store.save(c.id, &e.key, &sig, media, item)?;
        sum.done += 1;
        sum.unresolved += usize::from(media.is_none());
        items.extend(item);
    }
    if playlist && !items.is_empty() {
        let o = c.owner.unwrap_or(0);
        match host.playlist(o, c.name, &items) {
            Ok(id) => sum.playlist = Some(id),
            Err(x) => sum.last_error = Some(x.message),
        }
    }
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct Fake {
        albums: RefCell<Vec<String>>,
        wants: RefCell<Vec<Rec>>,
        owned: Vec<Rec>,
        playlists: RefCell<Vec<Vec<u32>>>,
        fail_not_found: bool,
        fail_down: bool,
        calls: Cell<u32>,
    }

    fn e(code: &str) -> HErr {
        HErr {
            code: code.into(),
            message: code.into(),
        }
    }

    impl Host for Fake {
        fn album(&self, ids: &[(String, String)], _: Option<u32>) -> Result<u32, HErr> {
            self.calls.set(self.calls.get() + 1);
            if self.fail_down {
                return Err(e("unavailable"));
            }
            self.albums.borrow_mut().push(ids[0].1.clone());
            Ok(10)
        }
        fn want(&self, r: &Rec, _: Option<u32>) -> Result<(u32, u32), HErr> {
            if self.fail_down {
                return Err(e("unavailable"));
            }
            if self.fail_not_found {
                return Err(e("ERR_NOT_FOUND"));
            }
            self.wants.borrow_mut().push(r.clone());
            Ok((10, 100 + self.wants.borrow().len() as u32))
        }
        fn find(&self, r: &Rec) -> Result<Option<(u32, u32)>, HErr> {
            Ok(self
                .owned
                .iter()
                .position(|o| o == r)
                .map(|p| (10, 200 + p as u32)))
        }
        fn playlist(&self, _: u32, _: &str, items: &[u32]) -> Result<u32, HErr> {
            self.playlists.borrow_mut().push(items.to_vec());
            Ok(7)
        }
        fn isrc_of(&self, t: &str) -> Option<String> {
            Some(format!("ISRC{t}"))
        }
        fn out_of_time(&self) -> bool {
            false
        }
    }

    #[derive(Default)]
    struct Mem(RefCell<HashMap<String, Option<u32>>>);

    impl Store for Mem {
        fn known(&self, _: i64, _: &str) -> Result<HashMap<String, Known>, String> {
            Ok(self
                .0
                .borrow()
                .iter()
                .map(|(k, i)| (k.clone(), Known { item: *i }))
                .collect())
        }
        fn save(
            &self,
            _: i64,
            k: &str,
            _: &str,
            _: Option<u32>,
            i: Option<u32>,
        ) -> Result<(), String> {
            self.0.borrow_mut().insert(k.into(), i);
            Ok(())
        }
    }

    fn ctx(s: &Settings) -> Ctx<'_> {
        Ctx {
            id: 1,
            name: "L",
            owner: Some(2),
            settings: s,
        }
    }

    fn settings(a: &[&str]) -> Settings {
        json!({ "actions": a }).as_object().unwrap().clone()
    }

    #[test]
    fn new_entries_are_acted_on_once() {
        let (host, store, s) = (
            Fake::default(),
            Mem::default(),
            settings(&["albums", "tracks", "playlist"]),
        );
        let mut t = Entry::track(Rec::Isrc("AA".into()), Some(("spotify", "a1")));
        t.deezer_track = None;
        let list = [t, Entry::album("musicbrainz", "g1")];
        let sum = run(&ctx(&s), &list, &host, &store).unwrap();
        assert_eq!((sum.done, sum.unresolved, sum.playlist), (2, 0, Some(7)));
        assert_eq!(host.albums.borrow().as_slice(), ["a1", "g1"]);
        assert_eq!(host.playlists.borrow()[0], [101]);
        let again = run(&ctx(&s), &list, &host, &store).unwrap();
        assert_eq!(again.done, 2);
        assert_eq!(
            host.albums.borrow().len(),
            2,
            "known entries are not sent again"
        );
        assert_eq!(
            host.playlists.borrow()[1],
            [101],
            "the playlist is rebuilt from what is known"
        );
    }

    #[test]
    fn a_playlist_keeps_only_owned_tracks_and_wants_nothing() {
        let (host, store, s) = (
            Fake {
                owned: vec![Rec::Mbid("b".into())],
                ..Fake::default()
            },
            Mem::default(),
            settings(&["playlist"]),
        );
        let list = [
            Entry::track(Rec::Mbid("a".into()), None),
            Entry::track(Rec::Mbid("b".into()), None),
        ];
        let sum = run(&ctx(&s), &list, &host, &store).unwrap();
        assert_eq!((sum.done, sum.unresolved, sum.errors), (2, 1, 0));
        assert!(host.wants.borrow().is_empty());
        assert_eq!(host.playlists.borrow()[0], [200]);
    }

    #[test]
    fn an_unavailable_want_is_retried_not_saved() {
        let s = settings(&["tracks"]);
        let store = Mem::default();
        let host = Fake {
            fail_down: true,
            ..Fake::default()
        };
        let list = [Entry::track(Rec::Mbid("m".into()), None)];
        let sum = run(&ctx(&s), &list, &host, &store).unwrap();
        assert_eq!((sum.errors, store.0.borrow().len()), (1, 0));
    }

    #[test]
    fn a_deezer_track_is_wanted_by_its_isrc() {
        let (host, store, s) = (Fake::default(), Mem::default(), settings(&["tracks"]));
        let mut d = Entry::album("deezer", "5");
        d.deezer_track = Some("9".into());
        run(&ctx(&s), &[d], &host, &store).unwrap();
        assert_eq!(host.wants.borrow().as_slice(), [Rec::Isrc("ISRC9".into())]);
    }

    #[test]
    fn an_unknown_track_is_remembered_and_an_outage_is_not() {
        let s = settings(&["tracks"]);
        let (store, list) = (Mem::default(), [Entry::track(Rec::Mbid("m".into()), None)]);
        let sum = run(
            &ctx(&s),
            &list,
            &Fake {
                fail_not_found: true,
                ..Fake::default()
            },
            &store,
        )
        .unwrap();
        assert_eq!(
            (sum.unresolved, sum.errors, store.0.borrow().len()),
            (1, 0, 1)
        );
        let s = settings(&["albums"]);
        let (store, list) = (Mem::default(), [Entry::album("deezer", "1")]);
        let sum = run(
            &ctx(&s),
            &list,
            &Fake {
                fail_down: true,
                ..Fake::default()
            },
            &store,
        )
        .unwrap();
        assert_eq!(
            (
                sum.errors,
                sum.last_error.as_deref(),
                store.0.borrow().len()
            ),
            (1, Some("unavailable"), 0)
        );
    }

    #[test]
    fn a_list_needs_an_action_and_a_playlist_a_user() {
        let (host, store) = (Fake::default(), Mem::default());
        assert!(run(&ctx(&settings(&[])), &[], &host, &store).is_err());
        let s = settings(&["playlist"]);
        let c = Ctx {
            owner: None,
            ..ctx(&s)
        };
        assert!(run(&c, &[], &host, &store).is_err());
    }

    #[test]
    fn five_outages_in_a_row_stop_the_run() {
        let (host, store, s) = (
            Fake {
                fail_down: true,
                ..Fake::default()
            },
            Mem::default(),
            settings(&["albums"]),
        );
        let list: Vec<Entry> = (0..9)
            .map(|i| Entry::album("deezer", &i.to_string()))
            .collect();
        let sum = run(&ctx(&s), &list, &host, &store).unwrap();
        assert_eq!((host.calls.get(), sum.complete), (5, false));
    }
}
