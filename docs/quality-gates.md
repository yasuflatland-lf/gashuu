# Quality gates

Reference doc migrated from the CLAUDE.md "Quality gates" section.
A change is not done until ALL gates are green.

### The three gates (run before calling any change done)

```bash
mise exec -- cargo fmt --check
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo nextest run --workspace --profile ci
```

### Token-drift guard (blocking)

`scripts/check-tokens.sh` fails on any raw color hex (`#rgb`..`#rrggbbaa`) in `crates/gashuu/ui/*.slint` except `Theme.slint`. It runs in CI (the `docs` job) and via `mise run check-tokens`, and is unconditionally blocking for the whole UI (no allowlist). On a hit, move the value into `Theme.slint` and reference the token.

### Always run cargo fmt after editing

**Always run `cargo fmt` after editing — not just the tests.** Code that compiles and passes `nextest` can still fail the CI `fmt --check` gate (e.g. compact struct/expr literals exceeding rustfmt's default width). Skipping fmt is the easiest way to land a red CI here.

### Clippy runs with -D warnings

Clippy runs with `-D warnings`; a warning is a build failure.

### Coverage (gashuu-core only)

Coverage is `gashuu-core` only (the UI needs a display server): `MISE_ENV=coverage mise exec -- cargo llvm-cov nextest -p gashuu-core --profile ci --summary-only`. `cargo-llvm-cov` lives in `mise.coverage.toml` and is only active under `MISE_ENV=coverage` (so the per-OS CI `app` jobs stay lean and don't install it; the `core` CI job sets this env and adds `llvm-tools-preview` via `rustup`). Forget the env and you get `error: no such command: llvm-cov`. Core sits ~96.5% line coverage.

### UI interaction behavior (coverage-exempt)

UI interaction and timing/positioning behavior — auto-hide chrome fade timing, scrubber popover positioning, live drag-preview — is coverage-exempt and verified by manual observation (same policy as dialogs and the thumbnail strip). Only the headless logic behind such UI (e.g. `scrub_fraction_to_page`, `preview_is_double`) is unit-tested; pure mapping/decision functions are extracted specifically so they can be tested without a display server.

A function returning `ModelRc<T>` is ALSO headlessly testable, not "untestable UI": use `slint::Image::default()` for `image` fields (constructs with no backend) and assert via the `slint::Model` trait — `row_count()` and `row_data(i)`. So model-mapping logic (e.g. `build_carousel_model`'s 0-based `last_page` → 1-based `current` conversion) gets unit tests, not a coverage exemption. Precedent: `crates/gashuu/src/thumbnail_strip.rs`.

### Exercise a real successful `open_path` in UI tests without an archive fixture (PR-R)

`ArchiveLoader::open` succeeds on an EMPTY on-disk directory (it becomes a valid `FolderSource`), so
a UI-crate test can drive the `open_path` Ok-path — and the invariants that need it, e.g.
`open_file()` becoming `Some(canonical)` — with just `std::env::temp_dir()` +
`std::fs::create_dir_all`, no zip/image dev-fixture. This complements the existing UI-crate
error-path/default-state strategy (the `gashuu` crate deliberately has no `tempfile`/`zip`/`rar`
dev-dep — see [docs/patterns.md](patterns.md)); archive correctness still lives in core's tests.

### Accepted uncovered lines (cache.rs, settings.rs)

`cache.rs` is ~95% because the rayon background-thread paths cannot be exercised deterministically — specifically `spawn_prefetch` (fire-and-forget), the dropped-prefetch-error path, and the `InFlightGuard` poisoned-lock recovery branch. `settings.rs` is ~95% because the `config_path()` `NoConfigDir` branch cannot be triggered on a normal OS with a config dir. Both sets of uncovered lines receive the same accepted treatment: do not chase them with `sleep`-based or environment-manipulation tests; they will make CI flaky.
