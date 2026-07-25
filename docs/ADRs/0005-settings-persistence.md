# ADR-0005: Persist settings as versioned JSON

- Status: Accepted
- Decided: 2026-05-31 (transcribed: 2026-06-01)
- Related: [ADR-0002](0002-layered-two-crate-architecture.md) (core layering)

## Context

User preferences (reading direction, spread mode, cache size, prefetch radius, recent files, …)
must persist across launches on all three OSes, and the schema must be able to evolve as features
land without breaking existing users' files. The store must also degrade gracefully on a corrupt or
hand-edited file (no startup crash).

## Decision

Persist settings as **JSON with an explicit `version` field**, in the OS-standard config location
(via the `directories` crate, e.g. `~/.config/<app>/settings.json`).

- Serialize with `serde` / `serde_json`. (This is the first use of `serde` in the core crate.)
- The schema carries `version`; on a schema change, bump it and convert older files in
  `Settings::migrate()`. (Amended: a stored version GREATER than the binary's is now a hard load
  error, not a silent pass-through — see the Amendment.)
- I/O takes explicit paths (`load_from` / `save_to`, tempfile-testable); `load` / `save` are thin
  OS-path wrappers.
- Corrupt-file recovery (warn + fall back to defaults) lives in the UI (`main.rs`) (now
  quarantine-then-fresh — see the Amendment); core only returns a typed `CoreError`.
- An `insta` snapshot of `Settings::default().to_json()` freezes the default schema; CI never
  updates snapshots, so an accidental schema change fails the build.

## Alternatives considered

- **TOML** — pleasant to hand-edit, but JSON is already the natural fit for `serde_json` round-trips
  and gives a simpler migration story for a machine-managed file.
- **SQLite** — overkill for a flat preferences blob; adds a native dependency and a migration engine
  for no benefit at this scale.

Chose JSON + a version field for simplicity, human readability, and easy migration.

## Consequences

### Positive
- Human-readable and trivially diffable; migration is a plain `serde_json::Value` transform.
- Read-path safety is enforced: non-object JSON roots are rejected *before* `migrate()` (which would
  otherwise panic indexing a non-map); the `version` is parsed with `u32::try_from` (not a
  truncating `as` cast); load-path normalization applies (`cache_size.max(1)`,
  `recent_files.truncate(MAX_RECENT_FILES)`), while `preload_pages` is deliberately not clamped
  (0 = "prefetch disabled" is valid).
- Privacy by default: `recent_files` is recorded only when `track_recent_files` is enabled (off by
  default).

### Costs / trade-offs accepted
- Adding a persisted variant can break *downgrade* compatibility (an older build may reject a new
  enum variant and fall back to defaults). This is accepted and handled by the existing
  `unwrap_or_else` + `tracing::warn!` recovery rather than by a version bump per field.

## Implementation notes (as-built deltas)

- The schema has grown well beyond the design doc's example while staying forward/backward-compatible
  via `#[serde(default)]`: it now includes `reading_direction`, `spread_mode` (incl. `Auto`),
  `cover_mode`, `fit_mode`, `cache_size`, `preload_pages`, `track_recent_files`, `recent_files`,
  `key_bindings`, and `seen_guide` (first-run guide flag).
- **`SETTINGS_VERSION` stays 1.** New fields are absorbed by `#[serde(default)]`, so no migration was
  needed; the frozen snapshot simply gained each new default. `Settings::migrate()` is the mechanism
  reserved for the first genuinely incompatible change.
- `key_bindings` is persisted but **inactive** (forward-compat only); user-remappable keys are
  deferred, and the settings dialog shows the bindings read-only.

## Amendment 2026-07-25: a future schema version is a hard load error; the version is stamped, never echoed

- **Dispatch.** `stored < current` → migrate; `stored == current` → pass through;
  `stored > current` → `CoreError::FutureSchema { what, found, supported }`. This holds for BOTH
  `settings.json` and `library.json`, via the shared
  `persist::parse_versioned_object(json, current, migrate)` guard both aggregates now run.
- **Why.** Such a file previously loaded UNMIGRATED: `serde_json::from_value` silently dropped every
  field this build does not know (every field is `#[serde(default)]`), and the survivors were
  re-saved. `settings.json` kept its FUTURE version label, which permanently inoculated it against
  every later migration step — each fires only when `stored < target` — so the loss was
  unrecoverable even by the build that would have known how to read it. `library.json` was
  force-stamped honestly by `LibraryDocument` and was therefore silently DOWNGRADED instead.
- **Recovery: quarantine-then-fresh.** The file is renamed aside as `<name>.corrupt-<now_secs>` via
  `gashuu_core::quarantine_file` (same directory, so the rename is atomic on one filesystem; it
  never reads or parses the file and takes the clock as a parameter), a notice is surfaced, and the
  app starts from defaults. The user keeps their bytes and a newer build can still read them.
  Before this change, a `settings.json` this build could not load was OVERWRITTEN with the defaults'
  canonical JSON by `repair_settings_file_if_needed`; that function is now inert after a quarantine
  (its `!path.exists()` early return).
- **`Settings::normalize` stamps `SETTINGS_VERSION` on save** — the `version` field is a schema
  label owned by the binary, not user data, and is never echoed back from disk. This restores the
  symmetry `Library::to_json` already had via `LibraryDocument`.
- **Unchanged.** `SETTINGS_VERSION` and `LIBRARY_VERSION` both stay `1`; no migration step is added;
  the non-object-root guard and the `u32::try_from` version resolution still run FIRST; and a
  malformed or `> u32::MAX` version is still "unknown" and still migrates from 0 — it must never be
  misrouted into the future-version arm. See [patterns.md](../patterns.md), "A schema `version` from
  the FUTURE is a hard load error", and [architecture.md](../architecture.md), "persist".
