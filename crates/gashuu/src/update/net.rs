//! Blocking HTTP for the update checker. Always call from a background thread
//! (`rayon::spawn`), never the UI thread.
//! Every call is bounded by agent timeouts; ureq v3 would otherwise wait forever.

use super::{user_agent, UpdateError, RELEASES_LATEST_API};
use std::io::Read;
use std::sync::LazyLock;
use std::time::Duration;
use ureq::Agent;

// ureq v3 ships NO default timeouts (every `Timeouts` field defaults to `None`),
// so an unconfigured call waits forever on a half-open socket. Both calls here run
// on a rayon worker shared with thumbnail/cover work, and a wedged call also leaves
// the update dialog stuck in its in-progress state — so every phase is bounded.
//
// NOTE: ureq 3.3.0 has no per-read/idle/stall timeout. The closest is `recv_body`,
// which bounds the TOTAL body transfer rather than the gap between reads, so the
// download agent bounds each phase tightly and puts a deliberately generous hard
// ceiling on the body instead. The goal is "always fails in bounded time", not a
// tight transfer SLA.

/// DNS lookup budget. Not covered by `connect`, so it is set explicitly.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);
/// TCP + TLS handshake budget.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// End-to-end budget for the `releases/latest` JSON (a few KB).
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// Response-header budget for an asset download (body excluded).
const DOWNLOAD_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
/// Hard ceiling on a whole asset body transfer (~100 MB on a slow link).
const DOWNLOAD_BODY_TIMEOUT: Duration = Duration::from_secs(600);

/// Agent for the release-metadata check: small payload, so an end-to-end budget.
fn check_agent() -> Agent {
    Agent::new_with_config(
        Agent::config_builder()
            .timeout_resolve(Some(RESOLVE_TIMEOUT))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(CHECK_TIMEOUT))
            .build(),
    )
}

/// Agent for asset downloads: per-phase budgets, no global one — a large asset on
/// a slow link is legitimate, a silent phase is not.
fn download_agent() -> Agent {
    Agent::new_with_config(
        Agent::config_builder()
            .timeout_resolve(Some(RESOLVE_TIMEOUT))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_recv_response(Some(DOWNLOAD_RESPONSE_TIMEOUT))
            .timeout_recv_body(Some(DOWNLOAD_BODY_TIMEOUT))
            .build(),
    )
}

static CHECK_AGENT: LazyLock<Agent> = LazyLock::new(check_agent);
static DOWNLOAD_AGENT: LazyLock<Agent> = LazyLock::new(download_agent);

/// Fetch the `releases/latest` JSON payload. Blocking.
pub fn fetch_latest_release_json() -> Result<String, UpdateError> {
    let mut resp = CHECK_AGENT
        .get(RELEASES_LATEST_API)
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
    let resp = DOWNLOAD_AGENT
        .get(url)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_agent_bounds_resolve_connect_and_end_to_end() {
        let t = check_agent().config().timeouts();
        assert_eq!(t.resolve, Some(RESOLVE_TIMEOUT));
        assert_eq!(t.connect, Some(CONNECT_TIMEOUT));
        assert_eq!(t.global, Some(CHECK_TIMEOUT));
    }

    #[test]
    fn download_agent_bounds_each_phase_but_not_the_whole_call() {
        let t = download_agent().config().timeouts();
        assert_eq!(t.resolve, Some(RESOLVE_TIMEOUT));
        assert_eq!(t.connect, Some(CONNECT_TIMEOUT));
        assert_eq!(t.recv_response, Some(DOWNLOAD_RESPONSE_TIMEOUT));
        assert_eq!(t.recv_body, Some(DOWNLOAD_BODY_TIMEOUT));
        assert_eq!(t.global, None);
    }

    #[test]
    fn no_response_path_is_left_unbounded() {
        for t in [
            check_agent().config().timeouts(),
            download_agent().config().timeouts(),
        ] {
            assert!(t.resolve.is_some(), "DNS lookup is unbounded");
            assert!(t.connect.is_some(), "connect is unbounded");
            assert!(
                t.global.is_some() || (t.recv_response.is_some() && t.recv_body.is_some()),
                "response path is unbounded"
            );
        }
    }

    #[test]
    fn download_body_ceiling_is_generous_enough_for_a_real_asset() {
        assert!(DOWNLOAD_BODY_TIMEOUT >= Duration::from_secs(300));
        assert!(CONNECT_TIMEOUT <= Duration::from_secs(30));
    }
}
