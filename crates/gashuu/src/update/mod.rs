//! Presentation-side update machinery: HTTP fetch/download, and (in PR2)
//! self-replacement. Decision logic lives in `gashuu_core::update`.

pub mod net;

/// The running app's version (compared against the latest release).
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// GitHub `releases/latest` API endpoint (excludes prereleases and drafts).
pub const RELEASES_LATEST_API: &str =
    "https://api.github.com/repos/yasuflatland-lf/gashuu/releases/latest";

/// User-Agent required by the GitHub API. Includes the app version.
pub fn user_agent() -> String {
    format!("gashuu/{CURRENT_VERSION}")
}

/// Errors from the network/update glue.
#[derive(Debug)]
pub enum UpdateError {
    Http(String),
    Io(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Http(m) => write!(f, "network error: {m}"),
            UpdateError::Io(m) => write!(f, "I/O error: {m}"),
        }
    }
}

impl std::error::Error for UpdateError {}
