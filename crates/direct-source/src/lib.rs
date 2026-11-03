//! What the direct-source plugins share: a redirect-following GET, files under the plugin data
//! directory, and the host calls that turn a downloaded file into a library book.
pub mod host;
pub mod net;
pub mod store;

/// How an add failed: `Retry` leaves the scheduled call to fire again, `Drop` ends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    Retry(String),
    Drop(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Self::Retry(m) | Self::Drop(m)) = self;
        f.write_str(m)
    }
}
