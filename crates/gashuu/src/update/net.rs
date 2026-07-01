//! Blocking HTTP for the update checker. Always call from a background thread
//! (`rayon::spawn`), never the UI thread.

use super::{user_agent, UpdateError, RELEASES_LATEST_API};

/// Fetch the `releases/latest` JSON payload. Blocking.
pub fn fetch_latest_release_json() -> Result<String, UpdateError> {
    let resp = ureq::get(RELEASES_LATEST_API)
        .set("User-Agent", &user_agent())
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| UpdateError::Http(e.to_string()))?;
    resp.into_string()
        .map_err(|e| UpdateError::Io(e.to_string()))
}
