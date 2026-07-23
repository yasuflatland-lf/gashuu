//! Version comparison for the update checker.

use semver::Version;

/// Parse a release tag or a bare version string as semver, tolerating a leading
/// `v`/`V` and surrounding whitespace. Unparseable input yields `None`. The
/// parameter is `input` (not `tag`) because callers pass either form.
fn parse(input: &str) -> Option<Version> {
    let trimmed = input.trim().trim_start_matches(['v', 'V']);
    Version::parse(trimmed).ok()
}

/// True iff `latest` is a strictly newer semver than `current`. Any unparseable
/// input returns `false` so a malformed tag never nags the user.
pub fn is_update_available(current: &str, latest: &str) -> bool {
    match (parse(current), parse(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// True iff an update should be surfaced to the user: newer than `current` AND
/// not the version the user chose to skip.
pub fn should_notify(current: &str, latest_version: &str, skipped: Option<&str>) -> bool {
    if !is_update_available(current, latest_version) {
        return false;
    }
    let Some(skipped) = skipped else {
        return true;
    };
    match (parse(skipped), parse(latest_version)) {
        (Some(s), Some(l)) => s != l,
        // Unparseable input: fall back to the raw comparison.
        _ => skipped != latest_version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_patch_is_available() {
        assert!(is_update_available("0.10.0", "0.10.1"));
    }

    #[test]
    fn equal_is_not_available() {
        assert!(!is_update_available("0.11.0", "0.11.0"));
    }

    #[test]
    fn older_is_not_available() {
        assert!(!is_update_available("0.11.0", "0.10.0"));
    }

    #[test]
    fn leading_v_is_tolerated_on_both_sides() {
        assert!(is_update_available("v0.10.0", "v0.11.0"));
        assert!(!is_update_available("v0.11.0", "0.11.0"));
    }

    #[test]
    fn malformed_input_is_never_available() {
        assert!(!is_update_available("not-a-version", "1.0.0"));
        assert!(!is_update_available("1.0.0", ""));
    }

    #[test]
    fn should_notify_respects_skipped_version() {
        assert!(should_notify("0.10.0", "0.11.0", None));
        assert!(should_notify("0.10.0", "0.11.0", Some("0.10.5")));
        assert!(!should_notify("0.10.0", "0.11.0", Some("0.11.0")));
    }

    #[test]
    fn should_notify_compares_skipped_version_using_semver() {
        assert!(!should_notify("0.10.0", "v0.11.0", Some("0.11.0")));
    }

    #[test]
    fn should_notify_tolerates_uppercase_v_on_skipped_version() {
        assert!(!should_notify("0.10.0", "0.11.0", Some("V0.11.0")));
    }

    #[test]
    fn should_notify_falls_back_to_raw_comparison_for_unparseable_skip() {
        assert!(should_notify("0.10.0", "0.11.0", Some("not-a-version")));
    }

    #[test]
    fn should_notify_false_when_not_newer_even_if_not_skipped() {
        assert!(!should_notify("0.11.0", "0.11.0", None));
    }
}
