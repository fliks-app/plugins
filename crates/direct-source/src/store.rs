//! Files under the plugin's data directory (`/data` for the guest).
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Writes `root/name` through a `.part` file, so a file that exists is a complete one.
pub fn save(root: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let path = root.join(name);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let part = path.with_extension("part");
    fs::write(&part, bytes)?;
    fs::rename(&part, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_through_a_part_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = save(dir.path(), "b/001.mp3", b"abc").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"abc");
        assert!(!dir.path().join("b/001.part").exists());
    }
}
