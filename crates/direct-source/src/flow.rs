//! The end of an add: the ingest, then what the data directory keeps.
use crate::host::{self, Ingest};
use crate::{Fail, store};
use std::path::Path;

/// The attempt the host drops a scheduled call after is the 20th.
const LAST_ATTEMPT: u32 = 19;

pub trait Library {
    fn ingest(&self, req: &Ingest, paths: Vec<String>) -> Result<(), Fail>;
}

pub struct Host;

impl Library for Host {
    fn ingest(&self, req: &Ingest, paths: Vec<String>) -> Result<(), Fail> {
        host::ingest(req, paths)
    }
}

/// Ingests `names` (under `dir` of the data directory), then removes `dir`, whose files the library
/// should have taken; a file it left is logged.
pub fn conclude(
    lib: &impl Library,
    root: &Path,
    dir: &str,
    req: &Ingest,
    names: &[String],
) -> Result<(), Fail> {
    lib.ingest(req, names.iter().map(|n| host::guest_path(n)).collect())?;
    let left = store::remove_dir(root, dir);
    if left > 0 {
        fliks_plugin_sdk::warn!("{left} file(s) of {dir} were not taken by the library");
    }
    Ok(())
}

/// The outcome of a scheduled call: a final failure, or the last retry, removes `dir`.
pub fn settle(root: &Path, dir: &str, attempt: u32, r: Result<(), Fail>) -> Result<(), String> {
    match r {
        Ok(()) => Ok(()),
        Err(Fail::Retry(m)) if attempt < LAST_ATTEMPT => Err(m),
        Err(Fail::Retry(m)) => {
            store::remove_dir(root, dir);
            Err(m)
        }
        Err(Fail::Drop(m)) => {
            store::remove_dir(root, dir);
            fliks_plugin_sdk::error!("{dir}: given up: {m}");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fliks_plugin_sdk::bindings::fliks::plugin::types::Family;
    use std::cell::RefCell;

    struct Fake(Result<(), Fail>, RefCell<Vec<String>>);

    impl Library for Fake {
        fn ingest(&self, _: &Ingest, paths: Vec<String>) -> Result<(), Fail> {
            self.1.borrow_mut().extend(paths);
            self.0.clone()
        }
    }

    fn req() -> Ingest {
        Ingest {
            media: 1,
            family: Family::Audio,
            language: None,
            key: "k".into(),
            label: "L",
        }
    }

    fn seeded() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        store::save(d.path(), "b/001.mp3", b"a").unwrap();
        d
    }

    #[test]
    fn a_taken_ingest_removes_the_directory() {
        let d = seeded();
        let lib = Fake(Ok(()), RefCell::default());
        conclude(&lib, d.path(), "b", &req(), &["b/001.mp3".into()]).unwrap();
        assert_eq!(lib.1.borrow()[0], "/data/b/001.mp3");
        assert!(!d.path().join("b").exists());
    }

    #[test]
    fn a_failed_ingest_keeps_the_files_for_the_retry() {
        let d = seeded();
        let lib = Fake(Err(Fail::Retry("busy".into())), RefCell::default());
        assert!(conclude(&lib, d.path(), "b", &req(), &["b/001.mp3".into()]).is_err());
        assert!(d.path().join("b/001.mp3").exists());
    }

    #[test]
    fn a_refused_ingest_is_given_up_and_cleaned() {
        let d = seeded();
        let lib = Fake(Err(Fail::Drop("ERR_VALIDATION".into())), RefCell::default());
        let r = conclude(&lib, d.path(), "b", &req(), &["b/001.mp3".into()]);
        assert!(matches!(r, Err(Fail::Drop(_))));
        assert_eq!(settle(d.path(), "b", 1, r), Ok(()));
        assert!(!d.path().join("b").exists());
    }

    #[test]
    fn settle_cleans_on_a_final_failure_only() {
        let d = seeded();
        assert!(settle(d.path(), "b", 1, Err(Fail::Retry("x".into()))).is_err());
        assert!(d.path().join("b").exists());
        assert!(settle(d.path(), "b", 19, Err(Fail::Retry("x".into()))).is_err());
        assert!(!d.path().join("b").exists());
        let d = seeded();
        assert_eq!(
            settle(d.path(), "b", 1, Err(Fail::Drop("x".into()))),
            Ok(())
        );
        assert!(!d.path().join("b").exists());
    }
}
