//! Getting the EPUB of a book into the plugin data directory.
use direct_source::Fail;
use direct_source::net::{self, Transport};
use direct_source::store;
use std::path::Path;

/// EPUB 3 with images first, then the plain EPUB with and without images.
fn candidates(id: &str) -> [String; 3] {
    let base = format!("https://www.gutenberg.org/ebooks/{id}");
    [
        format!("{base}.epub3.images"),
        format!("{base}.epub.images"),
        format!("{base}.epub.noimages"),
    ]
}

/// Writes the first EPUB the site serves as `gutenberg-<id>.epub` under `root`; its name.
pub fn fetch(t: &impl Transport, root: &Path, id: &str) -> Result<String, Fail> {
    for url in candidates(id) {
        let r = net::follow(t, &url).map_err(Fail::Retry)?;
        if r.status == 404 {
            continue;
        }
        let bytes = net::body(r)?;
        if bytes.starts_with(b"PK") {
            let name = format!("gutenberg-{id}.epub");
            store::save(root, &name, &bytes).map_err(|e| Fail::Retry(format!("write: {e}")))?;
            return Ok(name);
        }
    }
    Err(Fail::Drop("no EPUB is served for this book".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests::Replay;

    #[test]
    fn falls_back_to_the_next_format() {
        let dir = tempfile::tempdir().unwrap();
        let t = Replay::new(&[(404, ""), (200, "PKzip")]);
        assert_eq!(fetch(&t, dir.path(), "7").unwrap(), "gutenberg-7.epub");
        assert_eq!(
            std::fs::read(dir.path().join("gutenberg-7.epub")).unwrap(),
            b"PKzip"
        );
        let urls = t.1.borrow();
        assert!(urls[0].ends_with("/7.epub3.images") && urls[1].ends_with("/7.epub.images"));
    }

    #[test]
    fn a_page_that_is_not_an_epub_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let t = Replay::new(&[(200, "<html>"), (200, "<html>"), (200, "<html>")]);
        assert!(matches!(fetch(&t, dir.path(), "7"), Err(Fail::Drop(_))));
    }

    #[test]
    fn an_outage_is_retried() {
        let dir = tempfile::tempdir().unwrap();
        let t = Replay::new(&[(503, "")]);
        assert!(matches!(fetch(&t, dir.path(), "7"), Err(Fail::Retry(_))));
    }
}
