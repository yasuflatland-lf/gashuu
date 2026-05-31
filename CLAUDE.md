# gashuu — Project Guide for Claude

gashuu is a cross-platform manga viewer in Rust + Slint. This file captures the conventions and hard-won gotchas an agent needs to work effectively here. User instructions always override it.

## Architecture

Two-crate Cargo workspace; keep the layer boundary strict:

- `crates/gashuu-core` — Slint-independent domain + I/O. Owns the `PageSource` trait, `FolderSource` (top-level directory walk with natural filename ordering), and `image_ops::decode` (returns raw RGBA8 + dimensions). **Never import `slint` here.** Errors are typed with `thiserror` (`CoreError`); this crate does no logging.
- `crates/gashuu` — Slint presentation layer. `ViewerState` (navigation + decode-on-demand), `keymap` (key token → `NavAction`), the Slint UI, and the `rfd` folder picker. Converts core RGBA to `slint::Image::from_rgba8`. Logs via `tracing`; user-facing errors are formatted with `color-eyre` and shown in the status bar.

**Why the split:** core stays headless and unit-testable (no display server); the UI is the only place that touches Slint, pixel buffers, and logging.

## Toolchain & build

- Rust is pinned to **1.96.0** via `mise.toml`. Run every cargo command through the pin: `mise exec -- cargo <...>`.
- **A fresh `mise install` fails with "Config files are not trusted."** Run `mise trust` once, then `mise install`. CI's `jdx/mise-action` handles trust automatically.
- Slint links system libraries on **Linux** only: `libfontconfig1-dev libfreetype6-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev`. macOS/Windows need nothing extra.
- `cargo run` opens a GUI window — never launch the app from a non-interactive/headless session (it hangs). Verify with build + clippy + tests instead.

## Quality gates (run before calling any change "done")

TDD with `cargo-nextest`. A change is not done until ALL of these are green:

```bash
mise exec -- cargo fmt --check
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo nextest run --workspace --profile ci
```

- **Always run `cargo fmt` after editing — not just the tests.** Code that compiles and passes `nextest` can still fail the CI `fmt --check` gate (e.g. compact struct/expr literals exceeding rustfmt's default width). Skipping fmt is the easiest way to land a red CI here.
- Clippy runs with `-D warnings`; a warning is a build failure.
- Coverage is `gashuu-core` only (the UI needs a display server): `mise exec -- cargo llvm-cov nextest -p gashuu-core --profile ci --summary-only`. Core sits ~99%.

## Conventions

- All comments and identifiers in **English**.
- TDD: keep the crate compiling at every save (write test + implementation so each saved state builds — important when several changes land together or in parallel).
- Keep a PR ≤ ~1000 production LOC.
- Tests synthesize fixtures in memory (the `image` crate makes tiny PNGs) plus `tempfile` for filesystem cases — **no committed binary fixtures.**

## Patterns & gotchas (learned the hard way)

- **Cross-crate mocking via a `testing` feature.** `gashuu-core` gates `mockall::automock` on `PageSource` behind `[features] testing = ["dep:mockall"]`; `gashuu`'s dev-dependency enables it, so `ViewerState` tests use `MockPageSource` without pulling `mockall` into release builds.
- **`#[allow(dead_code)]` on test-only accessors.** `ViewerState::page_count()`/`index()` are used only by `#[cfg(test)]` code. In a *binary* crate `pub` is not a public API surface, so `-D warnings` flags them as dead code; the `#[allow(dead_code)]` is intentional and documented in place.
- **Enforce load-bearing invariants in the type, not in prose.** `DecodedImage` keeps `rgba`/`width`/`height` private with a checked `new() -> Result<_, CoreError>` (validates `rgba.len() == width*height*4`, else `CoreError::MalformedImage`); public fields would let a caller build a value that panics `copy_from_slice` in `to_slint_image`. Construct via `new`; read via `width()/height()/rgba()`.
- **Decode with limits.** `image_ops::decode` uses `image::ImageReader` + `image::Limits` (16384×16384, 512 MiB alloc cap) to reject decompression bombs before allocating. `image::Limits` is `#[non_exhaustive]`, so build it with `Limits::default()` + field assignment (hence the local `#[allow(clippy::field_reassign_with_default)]`).
- **Don't swallow `WalkDir` errors.** `FolderSource::open` counts unreadable entries into `skipped_count()` rather than `.filter_map(Result::ok)`; the UI (`ViewerState::open_folder`) logs them via `tracing::warn!`. Core stays logging-free while the failure still surfaces.
- **Slint focus after a Button click.** Clicking a `Button` moves focus to it; the page `FocusScope` must call `fs.focus()` after the action (and on `init`) or keyboard navigation silently stops working.
- **Clear the displayed page on error.** `refresh` clears `current-page` to `slint::Image::default()` on an empty folder and on a decode error, so the view never shows a stale page that contradicts the status text.
- **`CoreError` is `#[non_exhaustive]`** so later PRs can add variants without breaking matches.

## Scope markers (what is intentionally deferred)

PR1 (current MVP): top-level folder walk only (`max_depth(1)`, no recursion), LTR navigation (→/Space = next, ←/Backspace = prev), decode-on-demand (no caching), PNG/JPG/JPEG.
Deferred: LRU page cache → PR2; archive `PageSource`s → later; RTL reading → PR4.
