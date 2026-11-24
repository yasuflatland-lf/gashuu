use super::*;
use crate::window_geometry::{WindowGeometry, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH};
use crate::{MAX_CACHE_CAPACITY, MAX_PREFETCH_RADIUS};
use proptest::prelude::*;

fn settings() -> impl Strategy<Value = Settings> {
    let view = (
        prop::sample::select(vec![ReadingDirection::Ltr, ReadingDirection::Rtl]),
        prop::sample::select(vec![
            SpreadMode::Auto,
            SpreadMode::Single,
            SpreadMode::Double,
        ]),
        prop::sample::select(vec![CoverMode::Standalone, CoverMode::Paired]),
        prop::sample::select(vec![FitMode::Whole, FitMode::Width, FitMode::Actual]),
    );
    let window = prop::option::of(
        (0u32..40_000, 0u32..40_000, any::<i32>(), any::<i32>()).prop_map(
            |(width, height, x, y)| WindowGeometry {
                width,
                height,
                x,
                y,
            },
        ),
    );
    let key_bindings = (
        prop::collection::vec("[a-z]{1,8}", 0..4),
        prop::collection::vec("[a-z]{1,8}", 0..4),
    )
        .prop_map(|(next, prev)| KeyBindings { next, prev });
    let reading = (
        key_bindings,
        any::<bool>(),
        // Unique recents keep the generated value at the normalization fixpoint.
        prop::collection::vec("[a-z]{1,8}", 0..=MAX_RECENT_SOURCES + 2),
        prop::sample::select(vec![Language::En, Language::Ja]),
        any::<bool>(),
    );
    let updates = (
        any::<bool>(),
        prop::option::of("v[0-9]\\.[0-9]\\.[0-9]"),
        prop::option::of(any::<i64>()),
    );
    (
        any::<u32>(),
        view,
        (
            0usize..MAX_CACHE_CAPACITY * 2,
            0usize..MAX_PREFETCH_RADIUS * 2,
        ),
        reading,
        window,
        updates,
        prop::collection::btree_set("[a-z]{1,8}", 0..4),
    )
        .prop_map(
            |(
                version,
                (reading_direction, spread_mode, cover_mode, fit_mode),
                (cache_capacity, prefetch_radius),
                (key_bindings, track_recent_sources, recent_sources, language, allow_rar_archives),
                window,
                (auto_update_check, skipped_version, last_update_check),
                web_guidance_dismissed,
            )| Settings {
                version,
                reading_direction,
                spread_mode,
                cover_mode,
                fit_mode,
                cache_capacity,
                prefetch_radius,
                key_bindings,
                track_recent_sources,
                recent_sources: recent_sources
                    .into_iter()
                    .enumerate()
                    .map(|(index, name)| PathBuf::from(format!("/{name}/{index}")))
                    .collect(),
                language,
                allow_rar_archives,
                window,
                auto_update_check,
                skipped_version,
                last_update_check,
                web_guidance_dismissed,
            },
        )
}

proptest! {
    #[test]
    fn normalized_settings_round_trip_through_json(mut settings in settings()) {
        settings.normalize();
        let json = settings.to_json().unwrap();

        prop_assert_eq!(Settings::from_json(&json).unwrap(), settings);
    }
}

#[test]
fn view_defaults_copies_the_four_global_modes() {
    let settings = Settings {
        reading_direction: ReadingDirection::Ltr,
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Paired,
        fit_mode: FitMode::Actual,
        ..Settings::default()
    };
    assert_eq!(
        settings.view_defaults(),
        ResolvedView {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
        }
    );
}

#[test]
fn set_view_defaults_is_the_inverse_of_view_defaults() {
    let mut settings = Settings::default();
    let view = ResolvedView {
        reading_direction: ReadingDirection::Ltr,
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Paired,
        fit_mode: FitMode::Actual,
    };

    settings.set_view_defaults(view);

    let actual = settings.view_defaults();
    assert_eq!(actual.reading_direction, view.reading_direction);
    assert_eq!(actual.spread_mode, view.spread_mode);
    assert_eq!(actual.cover_mode, view.cover_mode);
    assert_eq!(actual.fit_mode, view.fit_mode);
}

#[test]
fn default_settings_have_expected_values() {
    let s = Settings::default();
    assert_eq!(s.version, 1);
    assert_eq!(s.reading_direction, ReadingDirection::Rtl);
    assert_eq!(s.spread_mode, SpreadMode::Auto);
    assert_eq!(s.cover_mode, CoverMode::Standalone);
    assert_eq!(s.fit_mode, FitMode::Whole);
    assert_eq!(s.cache_capacity, 50);
    assert_eq!(s.prefetch_radius, 3);
    assert_eq!(s.key_bindings.next, vec!["right", "space"]);
    assert_eq!(s.key_bindings.prev, vec!["left", "backspace"]);
    assert!(!s.track_recent_sources);
    assert!(s.recent_sources.is_empty());
    assert!(
        s.allow_rar_archives,
        "allow_rar_archives must default to true"
    );
    assert_eq!(s.language, Language::En);
    assert!(
        s.auto_update_check,
        "auto_update_check must default to true"
    );
    assert_eq!(s.skipped_version, None);
    assert_eq!(s.last_update_check, None);
}

fn non_default_settings() -> Settings {
    Settings {
        version: SETTINGS_VERSION,
        reading_direction: ReadingDirection::Ltr,
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Paired,
        fit_mode: FitMode::Width,
        cache_capacity: 99,
        prefetch_radius: 4,
        key_bindings: KeyBindings {
            next: vec!["down".into()],
            prev: vec!["up".into()],
        },
        track_recent_sources: true,
        recent_sources: vec![PathBuf::from("/a"), PathBuf::from("/b")],
        language: Language::Ja,
        allow_rar_archives: false,
        window: Some(WindowGeometry {
            width: 1024,
            height: 768,
            x: 120,
            y: -40,
        }),
        auto_update_check: false,
        skipped_version: Some("v9.9.9".to_string()),
        last_update_check: Some(1_700_000_000),
        web_guidance_dismissed: ["evicted".to_owned()].into_iter().collect(),
    }
}

#[test]
fn non_default_settings_differs_from_default_in_every_field() {
    let fixture = serde_json::to_value(non_default_settings()).unwrap();
    let defaults = serde_json::to_value(Settings::default()).unwrap();

    for (key, value) in fixture.as_object().unwrap() {
        if key != "version" {
            assert_ne!(
                defaults.get(key),
                Some(value),
                "field {key} matches its default"
            );
        }
    }
}

#[test]
fn from_json_future_version_errors() {
    let result = Settings::from_json(r#"{"version":99}"#);

    assert!(matches!(
        result,
        Err(CoreError::FutureSchema {
            what: "settings",
            found: 99,
            supported: 1,
        })
    ));
}

#[test]
fn to_json_always_stamps_the_current_schema_version() {
    let mut settings = non_default_settings();
    settings.version = 99;

    settings.normalize();

    assert_eq!(settings.version, SETTINGS_VERSION);
    let value: serde_json::Value = serde_json::from_str(&settings.to_json().unwrap()).unwrap();
    assert_eq!(value["version"].as_u64(), Some(u64::from(SETTINGS_VERSION)));
}

#[test]
fn normalize_is_idempotent_for_the_version_field() {
    let mut settings = non_default_settings();
    settings.version = 99;

    settings.normalize();
    let once = settings.clone();
    settings.normalize();

    assert_eq!(settings, once);
}

#[test]
fn migrate_v0_fills_preload_and_bumps_version() {
    let value = serde_json::json!({"version": 0, "cache_size": 7});
    let migrated = migrate(value, 0);
    assert_eq!(
        migrated["preload_pages"].as_u64().unwrap() as usize,
        DEFAULT_PREFETCH_RADIUS
    );
    assert_eq!(migrated["version"].as_u64().unwrap() as u32, 1);
}

#[test]
fn migrate_noop_for_current_version() {
    let value = serde_json::to_value(non_default_settings()).unwrap();
    assert_eq!(migrate(value.clone(), SETTINGS_VERSION), value);
}

#[test]
fn from_json_defaults_missing_fields() {
    let s = Settings::from_json("{\"version\":1}").unwrap();
    assert_eq!(s, Settings::default());
}

#[test]
fn from_json_empty_document_uses_serde_defaults() {
    // A `{}` document exercises the `default_*` serde helpers (version absent
    // triggers migration to v1, while cache_capacity falls back to its default).
    let s = Settings::from_json("{}").unwrap();
    assert_eq!(s, Settings::default());
}

// ── fit_mode tests ──

#[test]
fn fit_mode_defaults_to_whole() {
    assert_eq!(FitMode::default(), FitMode::Whole);
    assert_eq!(Settings::default().fit_mode, FitMode::Whole);
}

#[test]
fn from_json_missing_fit_mode_defaults_to_whole() {
    // JSON without fit_mode must produce FitMode::Whole via #[serde(default)].
    let json = serde_json::json!({
        "version": SETTINGS_VERSION,
        "reading_direction": "ltr",
        "spread_mode": "single",
        "cover_mode": "standalone",
    })
    .to_string();
    let s = Settings::from_json(&json).unwrap();
    assert_eq!(s.fit_mode, FitMode::Whole);
    // The explicit keys must parse as written — "ltr"/"single" are NON-default
    // variants, so these also prove explicit values win over the defaults.
    assert_eq!(s.reading_direction, ReadingDirection::Ltr);
    assert_eq!(s.spread_mode, SpreadMode::Single);
    assert_eq!(s.cover_mode, CoverMode::Standalone);
}

#[test]
fn from_json_unknown_fit_mode_value_errors() {
    // An unknown fit_mode variant isn't covered by #[serde(default)] (which only fills an
    // ABSENT key); serde rejects it, so from_json returns Err(CoreError::Settings(_)).
    let json = serde_json::json!({
        "version": SETTINGS_VERSION,
        "fit_mode": "auto",
    })
    .to_string();
    let result = Settings::from_json(&json);
    assert!(
        matches!(result, Err(CoreError::Settings(_))),
        "expected Err(CoreError::Settings(_)) for unknown fit_mode value, got {:?}",
        result
    );
}

#[test]
fn push_recent_disabled_is_noop() {
    let mut s = Settings::default();
    assert!(!s.track_recent_sources);
    s.push_recent(PathBuf::from("/some/path"));
    assert!(s.recent_sources.is_empty());
}

#[test]
fn push_recent_dedups_moves_to_front_and_caps() {
    let mut s = Settings {
        track_recent_sources: true,
        ..Default::default()
    };

    // Pushing the same path twice dedups to a single entry.
    s.push_recent(PathBuf::from("/dup"));
    s.push_recent(PathBuf::from("/dup"));
    assert_eq!(s.recent_sources.len(), 1);

    // Push more than MAX_RECENT_SOURCES distinct paths; cap is enforced and
    // the most-recent push is at the front.
    s.recent_sources.clear();
    for i in 0..(MAX_RECENT_SOURCES + 5) {
        s.push_recent(PathBuf::from(format!("/path/{i}")));
    }
    assert_eq!(s.recent_sources.len(), MAX_RECENT_SOURCES);
    let last = MAX_RECENT_SOURCES + 5 - 1;
    assert_eq!(s.recent_sources[0], PathBuf::from(format!("/path/{last}")));
}

#[test]
fn default_settings_json_snapshot() {
    insta::assert_snapshot!(Settings::default().to_json().unwrap());
}

// ── FIX 1 regression: non-object JSON roots must return Err, never panic ──

#[test]
fn from_json_non_object_root_errors() {
    for input in &["5", "[]", "\"x\"", "true", "null"] {
        let result = Settings::from_json(input);
        assert!(
            matches!(result, Err(CoreError::Settings(_))),
            "expected Err(CoreError::Settings(_)) for input {:?}, got {:?}",
            input,
            result
        );
    }
}

// ── FIX 3 regression: recent sources capped on load ──

#[test]
fn from_json_caps_recent_sources_on_load() {
    let entries: Vec<String> = (0..(MAX_RECENT_SOURCES + 5))
        .map(|i| format!("/path/{i}"))
        .collect();
    // The on-disk key stays `recent_files` (serde rename); the in-memory field
    // it deserializes into is `recent_sources`.
    let json = serde_json::json!({
        "version": SETTINGS_VERSION,
        "recent_files": entries,
    })
    .to_string();
    let s = Settings::from_json(&json).unwrap();
    assert_eq!(s.recent_sources.len(), MAX_RECENT_SOURCES);
}

// ── FIX 3 regression: cache_size=0 normalized to 1; preload_pages=0 kept ──

// ── push_recent ordering: promote an existing middle entry ──

#[test]
fn push_recent_promotes_existing_middle_entry() {
    let mut s = Settings {
        track_recent_sources: true,
        recent_sources: vec![
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/c"),
        ],
        ..Default::default()
    };
    s.push_recent(PathBuf::from("/b"));
    assert_eq!(
        s.recent_sources,
        vec![
            PathBuf::from("/b"),
            PathBuf::from("/a"),
            PathBuf::from("/c"),
        ],
        "existing middle entry must be moved to front, others shifted back"
    );
}

// ── migrate guard: present preload_pages must not be overwritten ──

#[test]
fn migrate_preserves_existing_preload_pages() {
    let value = serde_json::json!({"version": 0, "preload_pages": 99});
    let migrated = migrate(value, 0);
    assert_eq!(
        migrated["preload_pages"].as_u64().unwrap(),
        99,
        "migrate must not overwrite a present preload_pages value"
    );
    assert_eq!(
        migrated["version"].as_u64().unwrap() as u32,
        1,
        "migrate must stamp version=1 after the v0→v1 step"
    );
}

// ── language tests ──

#[test]
fn language_defaults_to_en() {
    assert_eq!(Language::default(), Language::En);
    assert_eq!(Settings::default().language, Language::En);
}

#[test]
fn language_serializes_as_ietf_tag() {
    // The snake_case serde tags must stay "en"/"ja" — the presentation layer's
    // Fluent localizer (`langid_for`) consumes them as locale identifiers.
    let json = Settings {
        language: Language::Ja,
        ..Default::default()
    }
    .to_json()
    .unwrap();
    assert!(json.contains("\"language\": \"ja\""), "json was {json}");
}

#[test]
fn from_json_missing_language_defaults_to_en() {
    // A JSON object that omits `language` must produce `En` via
    // `#[serde(default)]`, identical to how `cover_mode`/`fit_mode` were added.
    let s = Settings::from_json(r#"{"version":1}"#).unwrap();
    assert_eq!(s.language, Language::En);
}

#[test]
fn from_json_unknown_language_value_errors() {
    // An unrecognised variant is rejected by serde (mirrors the fit_mode
    // unknown-value contract); #[serde(default)] only covers an ABSENT key.
    let json = serde_json::json!({
        "version": SETTINGS_VERSION,
        "language": "fr",
    })
    .to_string();
    let result = Settings::from_json(&json);
    assert!(
        matches!(result, Err(CoreError::Settings(_))),
        "expected Err(CoreError::Settings(_)) for unknown language value, got {:?}",
        result
    );
}

#[test]
fn cache_config_reflects_fields() {
    let s = Settings {
        cache_capacity: 30,
        prefetch_radius: 5,
        ..Default::default()
    };
    let cfg = s.cache_config();
    assert_eq!(cfg.capacity(), 30);
    assert_eq!(cfg.radius(), 5);
}

#[test]
fn cache_config_capacity_is_at_least_the_window_floor() {
    // The value object clamps capacity even if the raw field somehow held 0.
    let s = Settings {
        cache_capacity: 0,
        ..Default::default()
    };
    assert_eq!(s.cache_config().capacity(), 8);
    assert_eq!(s.cache_config().radius(), s.prefetch_radius);
}

#[test]
fn set_prefetch_radius_raises_capacity_to_the_window_floor() {
    let mut settings = Settings {
        cache_capacity: 2,
        prefetch_radius: 0,
        ..Settings::default()
    };

    let cfg = settings.set_prefetch_radius(3);

    assert_eq!(cfg.capacity(), 8);
    assert_eq!(cfg.radius(), 3);
    assert_eq!(settings.cache_capacity, 8);
    assert_eq!(settings.prefetch_radius, 3);
    assert_eq!(settings.cache_config(), cfg);
}

#[test]
fn set_cache_capacity_clamps_and_persists() {
    use crate::cache_config::{MAX_CACHE_CAPACITY, MAX_PREFETCH_RADIUS};

    let mut settings = Settings {
        prefetch_radius: MAX_PREFETCH_RADIUS + 1,
        ..Settings::default()
    };

    for (requested, expected) in [(0, 12), (MAX_CACHE_CAPACITY + 1, MAX_CACHE_CAPACITY)] {
        let cfg = settings.set_cache_capacity(requested);

        assert_eq!(cfg.capacity(), expected);
        assert_eq!(cfg.radius(), MAX_PREFETCH_RADIUS);
        assert_eq!(settings.cache_capacity, cfg.capacity());
        assert_eq!(settings.prefetch_radius, cfg.radius());
    }
}

#[test]
fn from_json_old_flat_keys_load_into_cache_config() {
    // Back-compat: an existing settings.json carrying the flat cache_size /
    // preload_pages keys still loads and yields a matching CacheConfig.
    let json = serde_json::json!({
        "version": SETTINGS_VERSION,
        "cache_size": 12,
        "preload_pages": 4,
    })
    .to_string();
    let cfg = Settings::from_json(&json).unwrap().cache_config();
    assert_eq!(cfg.capacity(), 12);
    assert_eq!(cfg.radius(), 4);
}

#[test]
fn from_json_clamps_cache_size_above_max() {
    use crate::cache_config::MAX_CACHE_CAPACITY;
    let json = serde_json::json!({
        "version": SETTINGS_VERSION,
        "cache_size": MAX_CACHE_CAPACITY + 50,
    })
    .to_string();
    let s = Settings::from_json(&json).unwrap();
    assert_eq!(s.cache_capacity, MAX_CACHE_CAPACITY);
}

#[test]
fn from_json_clamps_preload_pages_above_max() {
    use crate::cache_config::MAX_PREFETCH_RADIUS;
    let json = serde_json::json!({
        "version": SETTINGS_VERSION,
        "preload_pages": MAX_PREFETCH_RADIUS + 10,
    })
    .to_string();
    let s = Settings::from_json(&json).unwrap();
    assert_eq!(s.prefetch_radius, MAX_PREFETCH_RADIUS);
}

// ── window geometry tests ──

#[test]
fn window_defaults_to_none() {
    assert_eq!(Settings::default().window, None);
}

#[test]
fn window_omitted_from_default_json() {
    // skip_serializing_if keeps a None window out of the serialized document.
    let json = Settings::default().to_json().unwrap();
    assert!(
        !json.contains("window"),
        "a None window must not appear in settings.json, got {json}"
    );
}

#[test]
fn from_json_missing_window_defaults_to_none() {
    let s = Settings::from_json(r#"{"version":1}"#).unwrap();
    assert_eq!(s.window, None);
}

#[test]
fn normalize_floors_window_size() {
    // A hand-edited / undersized stored geometry is floored to the minimum,
    // mirroring how cache_capacity is normalized. Position is untouched.
    let mut s = Settings {
        window: Some(WindowGeometry {
            width: 100,
            height: 50,
            x: 7,
            y: 9,
        }),
        ..Default::default()
    };
    s.normalize();
    assert_eq!(
        s.window,
        Some(WindowGeometry {
            width: MIN_WINDOW_WIDTH,
            height: MIN_WINDOW_HEIGHT,
            x: 7,
            y: 9,
        })
    );
}

#[test]
fn normalize_discards_inflated_window_geometry() {
    // The corruption that blanked the window: a scale-factor round-trip inflated the
    // stored size off-screen. Such geometry is discarded so the app boots at default.
    let mut s = Settings {
        window: Some(WindowGeometry {
            width: 110592,
            height: 1982,
            x: 0,
            y: 66,
        }),
        ..Default::default()
    };
    s.normalize();
    assert_eq!(s.window, None);
}

#[test]
fn normalize_keeps_a_sane_window_geometry() {
    let geom = WindowGeometry {
        width: 1400,
        height: 900,
        x: 100,
        y: 100,
    };
    let mut s = Settings {
        window: Some(geom),
        ..Default::default()
    };
    s.normalize();
    assert_eq!(s.window, Some(geom));
}

#[test]
fn from_json_preserves_preload_pages_zero() {
    let json = serde_json::json!({
        "version": SETTINGS_VERSION,
        "preload_pages": 0,
    })
    .to_string();
    let s = Settings::from_json(&json).unwrap();
    assert_eq!(s.prefetch_radius, 0, "preload_pages=0 must remain valid");
}

#[test]
fn normalize_clamps_out_of_range_fields() {
    use crate::cache_config::{MAX_CACHE_CAPACITY, MAX_PREFETCH_RADIUS};
    // A `Settings` built any way other than `from_json` (e.g. hand-built in memory)
    // must still be brought inside its domain invariants by `normalize`.
    let mut s = Settings {
        cache_capacity: MAX_CACHE_CAPACITY + 50,
        prefetch_radius: MAX_PREFETCH_RADIUS + 10,
        recent_sources: (0..MAX_RECENT_SOURCES + 5)
            .map(|i| PathBuf::from(format!("/p{i}")))
            .collect(),
        ..Settings::default()
    };
    s.normalize();
    assert_eq!(s.cache_capacity, MAX_CACHE_CAPACITY);
    assert_eq!(s.prefetch_radius, MAX_PREFETCH_RADIUS);
    assert_eq!(s.recent_sources.len(), MAX_RECENT_SOURCES);
}

#[test]
fn allow_rar_archives_missing_field_loads_as_true() {
    // A settings file written before this field existed must adopt the new
    // default (`true`) via `default_allow_rar`, not serde's bare `bool::default()`.
    let json = serde_json::json!({"version": SETTINGS_VERSION}).to_string();
    let s = Settings::from_json(&json).unwrap();
    assert!(
        s.allow_rar_archives,
        "missing allow_rar_archives must load as true (new default)"
    );
}

#[test]
fn missing_auto_update_check_loads_as_true() {
    let s = Settings::from_json(r#"{"version":1}"#).unwrap();
    assert!(
        s.auto_update_check,
        "absent auto_update_check must load as true"
    );
}

#[test]
fn none_skipped_and_last_check_are_omitted_from_json() {
    let json = Settings::default().to_json().unwrap();
    assert!(!json.contains("skipped_version"));
    assert!(!json.contains("last_update_check"));
}
