//! The UI crate's only clock. The standard wall and monotonic clocks panic on
//! wasm32-unknown-unknown. `web_time` uses the standard clocks on native targets
//! and the browser's `Date.now()` and `performance.now()` on the web.
//! Workspace Clippy rules disallow direct clock reads elsewhere.

/// A monotonic instant for elapsed-time logging.
#[allow(clippy::disallowed_methods)]
pub(crate) fn now() -> web_time::Instant {
    web_time::Instant::now()
}

/// Whole seconds since the Unix epoch, or zero for a time before the epoch.
#[allow(clippy::disallowed_methods)]
pub(crate) fn unix_now_secs() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{now, unix_now_secs};

    #[test]
    fn unix_now_secs_is_after_2020() {
        assert!(unix_now_secs() > 1_577_836_800);
    }

    #[test]
    fn now_is_monotonic() {
        let a = now();
        let b = now();
        assert!(b >= a);
    }
}
