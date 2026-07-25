# Quality gates

Reference doc migrated from the CLAUDE.md "Quality gates" section.
A change is not done until ALL gates are green.

### The three cargo gates (run before calling any change done)

```bash
mise exec -- cargo fmt --check
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo nextest run --workspace --profile ci
```

This is the canonical form and stays canonical: CI runs the gates as separate steps across separate jobs on purpose, so each one reports its own red/green.

### Fast local path: `mise run gates` (all five blocking gates in parallel)

The three cargo gates and the two bash harnesses (`check-tokens`, `check-docs`) are mutually independent, so `mise run gates` fans all five out concurrently via mise's `depends`. Each gate is also a task on its own (`check-fmt`, `check-clippy`, `check-tests`, `check-tokens`, `check-docs`) when you only want one:

```bash
mise run gates         # all five, concurrently
mise run check-clippy  # just one
```

Wall clock, measured in a worktree with warm target directories after `touch crates/gashuu-core/src/lib.rs` (18 cores, median of 6 reps, machine sharing CPU with two other compile jobs):

| mode | wall |
| --- | --- |
| serial: the five commands one after another | 9.8 s |
| all five concurrent, clippy sharing the default target dir | 8.2 s |
| `mise run gates` — clippy on its own `--target-dir target/clippy` | 5.5 s |

The original measurement on an idle machine recorded 12 s / 10 s / 7 s for the same three modes. Both runs agree on the point: naive concurrency is a marginal win, and giving clippy its own target directory is what turns it into a ~1.8x one. Individual warm-incremental costs here: `fmt --check` 0.2 s, clippy 3.6-4.0 s, `nextest run` 6.0 s (915 tests themselves 1.2 s), `check-tokens` 0.2 s, `check-docs` 0.3 s.

**Why clippy needs its own `--target-dir`.** Cargo takes an exclusive build lock on a target directory, so clippy and nextest sharing the default one serialize on that lock and most of the parallelism evaporates. The separate directory is about the lock only — clippy and test artifacts do **not** invalidate each other's fingerprints (`nextest run --no-run` straight after clippy, and clippy straight after a test build, are both near-instant in a shared directory).

**Disk cost, not a leak.** `target/clippy` is a second check-only artifact tree: measured 1.5 GB here, inside a 5.1 GB `target/`. It is covered by the existing `/target/` line in `.gitignore` (`git check-ignore -v target/clippy` confirms). Seeing it appear is expected — do not go hunting for a stale-artifact bug.

**Fan-out width.** mise bounds concurrent dependencies by its `jobs` setting, which is 8 by default (`mise run --help` still advertises 4; `mise settings get jobs` is authoritative). Anything >= 5 gives full fan-out; a lower value only costs wall time, never correctness, which is why the repo does not pin `jobs` — that same knob also caps tool-install concurrency.

**Usable as a pre-commit gate.** A single failing dependency fails `mise run gates` with a non-zero exit code and the aggregate's own body never runs (verified by rigging `check-docs` to fail via its `CHECK_DOCS_ROOT` fixture seam: aggregate exit 1).

**No `mise exec --` prefix inside a task body.** mise runs task bodies in the activated tool environment — it exports `RUSTUP_TOOLCHAIN=1.97.1` and prepends the `cargo-nextest` install directory to `PATH` — so the bare `cargo` in these tasks *is* the pinned toolchain. Commands you type by hand are a different case and still need `mise exec --` (see CLAUDE.md).

### Token-drift guard (blocking)

`scripts/check-tokens.sh` fails on any raw color hex (`#rgb`..`#rrggbbaa`) found in `crates/gashuu/ui/` scanned RECURSIVELY (every `*.slint`, including `ui/components/` and any future subdirectory) except `Theme.slint`. It is color-hex only — length/size literals are NOT policed. It runs in CI (the `docs` job) and via `mise run check-tokens`, and is unconditionally blocking for the whole UI (no allowlist). On a hit, move the value into `Theme.slint` and reference the token.

### Always run cargo fmt after editing

**Always run `cargo fmt` after editing — not just the tests.** Code that compiles and passes `nextest` can still fail the CI `fmt --check` gate (e.g. compact struct/expr literals exceeding rustfmt's default width). Skipping fmt is the easiest way to land a red CI here.

### Clippy runs with -D warnings

Clippy runs with `-D warnings`; a warning is a build failure.

### Coverage (gashuu-core only)

Coverage is `gashuu-core` only (the UI needs a display server): `MISE_ENV=coverage mise exec -- cargo llvm-cov nextest -p gashuu-core --profile ci --summary-only`. `cargo-llvm-cov` lives in `mise.coverage.toml` and is only active under `MISE_ENV=coverage` (so the per-OS CI `app` jobs stay lean and don't install it; the `core` CI job sets this env and adds `llvm-tools-preview` via `rustup`). Forget the env and you get `error: no such command: llvm-cov`. Core sits ~96.5% line coverage.

### UI interaction behavior (coverage-exempt)

UI interaction and timing/positioning behavior — auto-hide chrome fade timing, scrubber popover positioning, live drag-preview — is coverage-exempt and verified by manual observation (same policy as dialogs and the thumbnail strip). Only the headless logic behind such UI (e.g. `scrub_fraction_to_page`, `preview_is_double`) is unit-tested; pure mapping/decision functions are extracted specifically so they can be tested without a display server.

A function returning `ModelRc<T>` is ALSO headlessly testable, not "untestable UI": use `slint::Image::default()` for `image` fields (constructs with no backend) and assert via the `slint::Model` trait — `row_count()` and `row_data(i)`. So model-mapping logic (e.g. `build_carousel_model`'s 0-based `last_page` → 1-based `current` conversion) gets unit tests, not a coverage exemption. Precedent: `crates/gashuu/src/thumbnail_strip.rs`.

### Exercise a real successful `open_path` in UI tests without an archive fixture

`ArchiveLoader::open` succeeds on an EMPTY on-disk directory (it becomes a valid `FolderSource`), so
a UI-crate test can drive the `open_path` Ok-path — and the invariants that need it, e.g.
`open_file()` becoming `Some(canonical)` — with just `std::env::temp_dir()` +
`std::fs::create_dir_all`, no zip/image dev-fixture. This complements the existing UI-crate
error-path/default-state strategy (the `gashuu` crate deliberately has no `tempfile`/`zip`/`rar`
dev-dep — see [docs/patterns.md](patterns.md)); archive correctness still lives in core's tests.

### Accepted uncovered lines (cache.rs, settings.rs)

`cache.rs` is ~95% because the rayon background-thread paths cannot be exercised deterministically — specifically `spawn_prefetch` (fire-and-forget), the dropped-prefetch-error path, and the `InFlightGuard` poisoned-lock recovery branch. `settings.rs` is ~95% because the `config_path()` `NoConfigDir` branch cannot be triggered on a normal OS with a config dir. Both sets of uncovered lines receive the same accepted treatment: do not chase them with `sleep`-based or environment-manipulation tests; they will make CI flaky.
