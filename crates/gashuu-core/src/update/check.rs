//! Throttle logic for the startup update check. `now` is supplied by the caller
//! (the UI reads the wall clock) so this stays pure and testable.

/// Minimum seconds between automatic checks (24 hours).
pub const CHECK_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// True iff an automatic check should run now: never checked before, or at least
/// `min_interval_secs` have elapsed since `last_check`. A clock that moved
/// backwards (now < last_check) does not check, to avoid a nag loop.
pub fn should_check(last_check: Option<i64>, now: i64, min_interval_secs: i64) -> bool {
    match last_check {
        None => true,
        Some(last) => now.saturating_sub(last) >= min_interval_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_checked_returns_true() {
        assert!(should_check(None, 1_000_000, CHECK_INTERVAL_SECS));
    }

    #[test]
    fn within_interval_returns_false() {
        let now = 1_000_000;
        assert!(!should_check(Some(now - 10), now, CHECK_INTERVAL_SECS));
    }

    #[test]
    fn past_interval_returns_true() {
        let now = 1_000_000;
        assert!(should_check(
            Some(now - CHECK_INTERVAL_SECS),
            now,
            CHECK_INTERVAL_SECS
        ));
    }

    #[test]
    fn clock_moved_back_does_not_check() {
        assert!(!should_check(
            Some(2_000_000),
            1_000_000,
            CHECK_INTERVAL_SECS
        ));
    }
}
