//! Version comparison for the update checker.

use semver::Version;
use std::cmp::Ordering;

/// Trim surrounding whitespace, then remove leading `v` or `V` characters.
pub(crate) fn strip_tag_prefix(input: &str) -> &str {
    input.trim().trim_start_matches(['v', 'V'])
}

/// A release version compared by SemVer precedence, ignoring build metadata.
#[derive(Debug, Clone)]
pub(crate) struct ReleaseVersion(Version);

impl ReleaseVersion {
    pub(crate) fn parse(input: &str) -> Option<Self> {
        Version::parse(strip_tag_prefix(input)).ok().map(Self)
    }

    pub(crate) fn is_newer_than(&self, other: &Self) -> bool {
        self.0.cmp_precedence(&other.0) == Ordering::Greater
    }
}

impl PartialEq for ReleaseVersion {
    fn eq(&self, other: &Self) -> bool {
        self.0.cmp_precedence(&other.0) == Ordering::Equal
    }
}

impl Eq for ReleaseVersion {}

/// True iff `latest` is a strictly newer semver than `current`. Any unparseable
/// input returns `false` so a malformed tag never nags the user.
pub fn is_update_available(current: &str, latest: &str) -> bool {
    match (
        ReleaseVersion::parse(current),
        ReleaseVersion::parse(latest),
    ) {
        (Some(current), Some(latest)) => latest.is_newer_than(&current),
        _ => false,
    }
}

/// True iff an update should be surfaced to the user: newer than `current` AND
/// not the version the user chose to skip.
pub fn should_notify(current: &str, latest_version: &str, skipped: Option<&str>) -> bool {
    let (Some(current), Some(latest)) = (
        ReleaseVersion::parse(current),
        ReleaseVersion::parse(latest_version),
    ) else {
        return false;
    };
    if !latest.is_newer_than(&current) {
        return false;
    }
    let Some(skipped) = skipped else {
        return true;
    };
    match ReleaseVersion::parse(skipped) {
        Some(skipped) => skipped != latest,
        // Unparseable input: fall back to the raw comparison.
        None => skipped != latest_version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_tag_prefix_trims_whitespace_then_v() {
        assert_eq!(strip_tag_prefix(" v1.2.3 "), "1.2.3");
        assert_eq!(strip_tag_prefix("V2.0.0"), "2.0.0");
    }

    #[test]
    fn newer_patch_is_available() {
        assert!(is_update_available("0.10.0", "0.10.1"));
    }

    #[test]
    fn equal_is_not_available() {
        assert!(!is_update_available("0.11.0", "0.11.0"));
    }

    #[test]
    fn build_metadata_is_not_an_update() {
        assert!(!is_update_available("0.0.0+0", "0.0.0+a"));
        assert!(!is_update_available("1.2.3", "1.2.3+build"));
    }

    #[test]
    fn skipped_version_ignores_build_metadata() {
        assert!(!should_notify("1.0.0", "1.1.0+b", Some("1.1.0+a")));
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
