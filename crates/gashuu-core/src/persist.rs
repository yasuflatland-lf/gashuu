//! Shared parse/migrate guard for versioned-object JSON documents.
//!
//! `Settings::from_json` and `Library::from_json` both load a `{ "version": N, … }`
//! document: reject a non-object root, resolve the schema `version`, and run a
//! module-specific `migrate` only when the stored version is older than the
//! current one. That guard is correctness-critical — a missed non-object check
//! panics in `migrate`, and a truncating `as u32` cast would silently re-migrate
//! a crafted huge version — so it lives here once rather than in two aggregates
//! that can drift. Each caller keeps its own `from_value` mapping, error variant,
//! and post-deserialize step; only the shared prefix is single-homed.
//!
//! Headless: this module uses only `serde_json` (no `slint`, no `tracing`).

/// Parse a versioned-object JSON document: reject a non-object root, resolve the
/// schema `version` (truncating-cast-safe), and run `migrate(value, from)` iff the
/// stored version is older than `current`. Returns the post-migrate value; the
/// caller deserializes it and maps the error to its own `CoreError` variant.
pub(crate) fn parse_versioned_object(
    json: &str,
    current: u32,
    migrate: impl Fn(serde_json::Value, u32) -> serde_json::Value,
) -> Result<serde_json::Value, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    if !value.is_object() {
        // Reject non-object roots (e.g. `5`, `[]`, `"x"`, `true`, `null`): `migrate`
        // indexes the value as a map and would otherwise panic. Surface a typed serde
        // error instead. We deserialize into a Map (not `from_value::<T>`) because the
        // caller aggregates carry `#[serde(default)]` on every field, so serde would
        // happily turn a non-object into an all-defaults value — defeating the guard.
        // A non-object → Map deserialize is guaranteed to error, hence `unwrap_err`.
        let err = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(value)
            .unwrap_err();
        return Err(err);
    }
    // Checked conversion, not a truncating `as u32`: a crafted future-version value
    // (> u32::MAX) is treated as unknown (0) rather than silently wrapping into a
    // small number that would trigger an unexpected migration.
    let from = value
        .get("version")
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0);
    Ok(if from < current {
        migrate(value, from)
    } else {
        value
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURRENT: u32 = 2;

    /// A migrate stamp that records it ran by writing `migrated_from` into the doc
    /// and bumping `version` to CURRENT. Lets a test assert whether migration fired.
    fn stamp_migrate(mut value: serde_json::Value, from: u32) -> serde_json::Value {
        value["migrated_from"] = serde_json::json!(from);
        value["version"] = serde_json::json!(CURRENT);
        value
    }

    #[test]
    fn non_object_root_errors_without_panicking() {
        for src in ["5", "[]", "\"x\"", "true", "null"] {
            assert!(
                parse_versioned_object(src, CURRENT, stamp_migrate).is_err(),
                "expected Err for non-object root {src:?}"
            );
        }
    }

    #[test]
    fn invalid_json_propagates_the_parse_error() {
        assert!(parse_versioned_object("{ not json", CURRENT, stamp_migrate).is_err());
    }

    #[test]
    fn older_version_is_migrated() {
        let value = parse_versioned_object(r#"{"version":0}"#, CURRENT, stamp_migrate).unwrap();
        assert_eq!(value["migrated_from"], serde_json::json!(0));
        assert_eq!(value["version"], serde_json::json!(CURRENT));
    }

    #[test]
    fn missing_version_is_treated_as_zero_and_migrated() {
        let value = parse_versioned_object(r#"{"books":[]}"#, CURRENT, stamp_migrate).unwrap();
        assert_eq!(value["migrated_from"], serde_json::json!(0));
    }

    #[test]
    fn huge_version_is_treated_as_unknown_and_migrated() {
        // > u32::MAX: a truncating cast would wrap to a small number and skip/mis-run
        // migration; the checked conversion treats it as 0 and migrates from there.
        let huge = u64::from(u32::MAX) + 1;
        let value =
            parse_versioned_object(&format!(r#"{{"version":{huge}}}"#), CURRENT, stamp_migrate)
                .unwrap();
        assert_eq!(value["migrated_from"], serde_json::json!(0));
    }

    #[test]
    fn current_version_is_returned_untouched() {
        let src = format!(r#"{{"version":{CURRENT},"keep":"me"}}"#);
        let value = parse_versioned_object(&src, CURRENT, stamp_migrate).unwrap();
        // migrate must NOT have run: no stamp, original fields intact.
        assert!(value.get("migrated_from").is_none());
        assert_eq!(value["keep"], serde_json::json!("me"));
        assert_eq!(value["version"], serde_json::json!(CURRENT));
    }
}
