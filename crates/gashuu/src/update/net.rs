//! Blocking HTTP for the update checker. Always call from a background thread
//! (`rayon::spawn`), never the UI thread.

use super::{user_agent, UpdateError, RELEASES_LATEST_API};
use std::io::Read;

/// Fetch the `releases/latest` JSON payload. Blocking.
pub fn fetch_latest_release_json() -> Result<String, UpdateError> {
    let mut resp = ureq::get(RELEASES_LATEST_API)
        .header("User-Agent", &user_agent())
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| UpdateError::Http(e.to_string()))?;
    resp.body_mut()
        .read_to_string()
        .map_err(|e| UpdateError::Io(e.to_string()))
}

/// Download `url` into memory. Blocking. Follows redirects (ureq default), so a
/// `browser_download_url` that 302s to codeload/S3 works.
pub fn download_bytes(url: &str) -> Result<Vec<u8>, UpdateError> {
    let resp = ureq::get(url)
        .header("User-Agent", &user_agent())
        .call()
        .map_err(|e| UpdateError::Http(e.to_string()))?;
    let mut buf = Vec::new();
    resp.into_body()
        .into_reader()
        .read_to_end(&mut buf)
        .map_err(|e| UpdateError::Io(e.to_string()))?;
    Ok(buf)
}
