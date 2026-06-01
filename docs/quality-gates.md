# Quality gates

Reference doc migrated from the CLAUDE.md "Quality gates" section.
A change is not done until ALL gates are green.

### The three gates (run before calling any change done)

```bash
mise exec -- cargo fmt --check
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo nextest run --workspace --profile ci
```

### Always run cargo fmt after editing

**Always run `cargo fmt` after editing — not just the tests.** Code that compiles and passes `nextest` can still fail the CI `fmt --check` gate (e.g. compact struct/expr literals exceeding rustfmt's default width). Skipping fmt is the easiest way to land a red CI here.

### Clippy runs with -D warnings

Clippy runs with `-D warnings`; a warning is a build failure.

### Coverage (gashuu-core only)

Coverage is `gashuu-core` only (the UI needs a display server): `MISE_ENV=coverage mise exec -- cargo llvm-cov nextest -p gashuu-core --profile ci --summary-only`. `cargo-llvm-cov` lives in `mise.coverage.toml` and is only active under `MISE_ENV=coverage` (so the per-OS CI `app` jobs stay lean and don't install it; the `core` CI job sets this env and adds `llvm-tools-preview` via `rustup`). Forget the env and you get `error: no such command: llvm-cov`. Core sits ~96.5% line coverage.

### Accepted uncovered lines (cache.rs, settings.rs)

`cache.rs` is ~95% because the rayon background-thread paths cannot be exercised deterministically — specifically `spawn_prefetch` (fire-and-forget), the dropped-prefetch-error path, and the `InFlightGuard` poisoned-lock recovery branch. `settings.rs` is ~95% because the `config_path()` `NoConfigDir` branch cannot be triggered on a normal OS with a config dir. Both sets of uncovered lines receive the same accepted treatment: do not chase them with `sleep`-based or environment-manipulation tests; they will make CI flaky.
