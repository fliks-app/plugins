//! Files under the plugin's data directory (`/data` for the guest).
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static PARTS: AtomicU64 = AtomicU64::new(0);

/// Writes `root/name` through a `.part` file of its own, so a file that exists is a complete one
/// and two calls never share a partial file.
pub fn save(root: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let path = root.join(name);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let part = path.with_extension(format!(
        "part-{stamp}-{}",
        PARTS.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&part, bytes)?;
    fs::rename(&part, &path).inspect_err(|_| {
        let _ = fs::remove_file(&part);
    })?;
    Ok(path)
}

/// Removes `root/dir` with what is left in it; how many files it still held.
pub fn remove_dir(root: &Path, dir: &str) -> usize {
    let path = root.join(dir);
    let left = fs::read_dir(&path).map_or(0, Iterator::count);
    let _ = fs::remove_dir_all(&path);
    left
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_through_a_part_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = save(dir.path(), "b/001.mp3", b"abc").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"abc");
        assert_eq!(fs::read_dir(dir.path().join("b")).unwrap().count(), 1);
    }

    #[test]
    fn remove_dir_counts_what_was_left() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "b/001.mp3", b"abc").unwrap();
        assert_eq!(remove_dir(dir.path(), "b"), 1);
        assert!(!dir.path().join("b").exists());
        assert_eq!(remove_dir(dir.path(), "b"), 0);
    }
}
