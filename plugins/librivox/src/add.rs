//! Getting the sections of a recording into the plugin data directory.
use crate::parse::Book;
use direct_source::Fail;
use direct_source::net::{self, Transport};
use direct_source::store;
use std::path::Path;
use std::time::Duration;

/// A section is not started with less than this left of the call's deadline.
pub const MARGIN: Duration = Duration::from_secs(90);

#[derive(Debug, PartialEq, Eq)]
pub enum Progress {
    /// Every section is on disk: their names under the data directory, in listening order.
    Done(Vec<String>),
    /// The call ran out of time with sections left; a later call carries on.
    More,
}

/// The directory of a recording under the data directory.
pub fn dir(book: u32) -> String {
    format!("librivox-{book}")
}

fn name(book: u32, number: u32) -> String {
    format!("{}/{number:03}.mp3", dir(book))
}

/// Downloads the sections not on disk yet while `left` allows, one file each.
pub fn fetch(
    t: &impl Transport,
    root: &Path,
    book: &Book,
    left: &dyn Fn() -> Duration,
) -> Result<Progress, Fail> {
    if book.sections.is_empty() {
        return Err(Fail::Drop("the recording has no section".into()));
    }
    if book.gaps > 0 {
        return Err(Fail::Drop(format!("{} section(s) have no file", book.gaps)));
    }
    let mut saved = 0;
    let mut names = vec![];
    for s in &book.sections {
        let n = name(book.id, s.number);
        names.push(n.clone());
        if root.join(&n).exists() {
            continue;
        }
        if left() < MARGIN {
            continue;
        }
        let r = net::follow(t, &s.url).map_err(net::fail)?;
        let bytes = net::body(r)?;
        if bytes.is_empty() {
            return Err(Fail::Retry("an empty section".into()));
        }
        store::save(root, &n, &bytes).map_err(|e| Fail::Retry(format!("write: {e}")))?;
        saved += 1;
    }
    if names.iter().all(|n| root.join(n).exists()) {
        // The ingest gets a call of its own when this one has spent its time downloading.
        if saved > 0 && left() < MARGIN {
            return Ok(Progress::More);
        }
        return Ok(Progress::Done(names));
    }
    if saved == 0 {
        return Err(Fail::Retry("no time left to download a section".into()));
    }
    Ok(Progress::More)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests::Replay;
    use crate::parse::parse_books;

    const BOOK: &str = include_str!("../tests/fixtures/book.json");

    fn book() -> Book {
        parse_books(BOOK.as_bytes()).unwrap().remove(0)
    }

    const PLENTY: fn() -> Duration = || Duration::from_secs(3600);

    #[test]
    fn downloads_every_section_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let t = Replay::new(&[(200, "a"), (200, "b"), (200, "c")]);
        let Progress::Done(names) = fetch(&t, dir.path(), &book(), &PLENTY).unwrap() else {
            panic!("not done");
        };
        assert_eq!(
            names,
            [
                "librivox-2001/001.mp3",
                "librivox-2001/002.mp3",
                "librivox-2001/003.mp3"
            ]
        );
        assert!(t.1.borrow()[0].ends_with("part1.mp3"));
        assert_eq!(std::fs::read(dir.path().join(&names[2])).unwrap(), b"c");
    }

    #[test]
    fn a_later_call_skips_the_sections_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        store::save(dir.path(), "librivox-2001/001.mp3", b"a").unwrap();
        store::save(dir.path(), "librivox-2001/002.mp3", b"b").unwrap();
        let t = Replay::new(&[(200, "c")]);
        assert!(matches!(
            fetch(&t, dir.path(), &book(), &PLENTY),
            Ok(Progress::Done(_))
        ));
        assert_eq!(t.1.borrow().len(), 1);
    }

    #[test]
    fn running_out_of_time_leaves_the_rest_for_a_later_call() {
        let dir = tempfile::tempdir().unwrap();
        store::save(dir.path(), "librivox-2001/001.mp3", b"a").unwrap();
        let t = Replay::new(&[]);
        let none = || Duration::from_secs(10);
        assert!(matches!(
            fetch(&t, dir.path(), &book(), &none),
            Err(Fail::Retry(_))
        ));
        let calls = std::cell::Cell::new(0);
        let once = || {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Duration::from_secs(3600)
            } else {
                Duration::from_secs(10)
            }
        };
        let t = Replay::new(&[(200, "b")]);
        assert_eq!(fetch(&t, dir.path(), &book(), &once), Ok(Progress::More));
    }

    #[test]
    fn the_ingest_waits_for_a_fresh_call_after_late_downloads() {
        let dir = tempfile::tempdir().unwrap();
        store::save(dir.path(), "librivox-2001/001.mp3", b"a").unwrap();
        store::save(dir.path(), "librivox-2001/002.mp3", b"b").unwrap();
        let calls = std::cell::Cell::new(0);
        let late = || {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Duration::from_secs(3600)
            } else {
                Duration::from_secs(10)
            }
        };
        let t = Replay::new(&[(200, "c")]);
        assert_eq!(fetch(&t, dir.path(), &book(), &late), Ok(Progress::More));
        let t = Replay::new(&[]);
        assert!(matches!(
            fetch(&t, dir.path(), &book(), &|| Duration::from_secs(10)),
            Ok(Progress::Done(_))
        ));
    }

    #[test]
    fn a_recording_with_a_gap_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = book();
        b.gaps = 1;
        let t = Replay::new(&[]);
        assert!(matches!(
            fetch(&t, dir.path(), &b, &PLENTY),
            Err(Fail::Drop(_))
        ));
    }

    #[test]
    fn a_missing_section_ends_the_add_and_an_outage_retries_it() {
        let dir = tempfile::tempdir().unwrap();
        let t = Replay::new(&[(404, "")]);
        assert!(matches!(
            fetch(&t, dir.path(), &book(), &PLENTY),
            Err(Fail::Drop(_))
        ));
        let t = Replay::new(&[(503, "")]);
        assert!(matches!(
            fetch(&t, dir.path(), &book(), &PLENTY),
            Err(Fail::Retry(_))
        ));
    }
}
