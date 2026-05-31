# gashuu — Project Guide for Claude

gashuu is a cross-platform manga viewer in Rust + Slint. This file captures the conventions and hard-won gotchas an agent needs to work effectively here. User instructions always override it.

## Architecture

Two-crate Cargo workspace; keep the layer boundary strict:

- `crates/gashuu-core` — Slint-independent domain + I/O. Owns the `PageSource` trait (requires `Send + Sync` so `Arc<dyn PageSource>` can be shared with rayon worker threads during prefetch), `FolderSource` (top-level directory walk with natural filename ordering), `image_ops::decode` (returns raw RGBA8 + dimensions), and `cache::ImageCache` (LRU of `Arc<DecodedImage>` up to `DEFAULT_CAPACITY`=50 + background ±`DEFAULT_PREFETCH_RADIUS`=3 prefetch in front of any `PageSource`). **Never import `slint` OR `tracing` here** — the hard import boundary keeps core headless; prefetch errors are surfaced silently (counted, not logged). Errors are typed with `thiserror` (`CoreError`).
- `crates/gashuu` — Slint presentation layer. `ViewerState` (navigation backed by `ImageCache`; `current_image()` returns `Arc<DecodedImage>`), `keymap` (key token → `NavAction`), the Slint UI, and the `rfd` folder picker. Converts core RGBA to `slint::Image::from_rgba8`. Logs via `tracing`; user-facing errors are formatted with `color-eyre` and shown in the status bar.

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
- Coverage is `gashuu-core` only (the UI needs a display server): `mise exec -- cargo llvm-cov nextest -p gashuu-core --profile ci --summary-only`. Core sits ~97–98% line coverage; `cache.rs` is ~95% because the uncovered lines are the rayon background-thread paths that deterministic tests cannot exercise without flakiness — specifically `spawn_prefetch` (fire-and-forget), the dropped-prefetch-error path, and the `InFlightGuard` poisoned-lock recovery branch. Do not try to cover these with `sleep`-based timing assertions; they will make CI flaky.

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
- **Background prefetch is fire-and-forget on rayon over `Arc<Mutex<LruCache>>`.** Cache hits must stay instant (clone an `Arc`, never block on prefetch). Locks are released before the parallel decode section, so mutexes cannot be poisoned in practice — `lock().unwrap()` calls are intentional fail-fast, documented at the `Inner` struct.
- **Lock order is `cache` → `in_flight`** whenever both are held; `get` only ever takes `cache`. Violating this order risks deadlock — never reverse it in new code.
- **Clean up reserved shared state with an RAII guard; `Drop` must never `.unwrap()` a lock.** Use `unwrap_or_else(|e| e.into_inner())` to recover a poisoned lock, or a panic during unwind becomes a double-panic abort. `InFlightGuard` exists so a panic in the decode section cannot permanently leak in-flight markers (which would silently disable prefetch for those pages).
- **`get`/`current_image` return `Arc<DecodedImage>`** so cache hits never copy the multi-MB RGBA buffer; the UI's `to_slint_image(&DecodedImage)` is unchanged thanks to deref coercion (`&Arc<DecodedImage>` → `&DecodedImage`).
- **Verify trait thread-safety at compile time.** A `#[cfg(all(test, feature="testing"))]` test asserting `fn assert_send_sync<T: Send + Sync>()` over `FolderSource` and `MockPageSource` locks in the `Send + Sync` supertrait — if a future `PageSource` impl breaks it, the crate won't compile.
- **Test async caches deterministically by exercising the synchronous core.** Cache-semantics tests use `radius = 0` so rayon tasks are inert; `prefetch_indices` (pure) and `Inner::prefetch_blocking` (sync) are tested directly; the in-flight skip branch is tested by pre-seeding `in_flight`. Never assert on wall-clock timing — the `<50 ms` page-turn target is observed via `RUST_LOG=debug` `tracing::debug!(elapsed_ms=…)` in the UI, not asserted.
- **An LRU eviction test must distinguish LRU from FIFO.** A plain sequential `get(0), get(1), get(2)` eviction test passes under FIFO too; add a hit-promotion case (re-hit an old key, then verify a later miss evicts the *other* key) to actually pin LRU recency semantics.
- **Use `saturating_add`/`saturating_sub` for page-index arithmetic** (e.g. `center.saturating_add(radius)`) so debug builds don't panic on overflow.
- **`rayon` is already transitive via `image`** — adding it as a direct dependency pulls in no new third-party code; it just lets core `use rayon` directly.

## Scope markers (what is intentionally deferred)

Baseline (PR1 + PR2): top-level folder walk only (`max_depth(1)`, no recursion), LTR navigation (→/Space = next, ←/Backspace = prev), PNG/JPG/JPEG. LRU page cache (up to `DEFAULT_CAPACITY`=50 decoded images) + background ±`DEFAULT_PREFETCH_RADIUS`=3 prefetch replaces PR1's decode-on-demand.
Deferred: archive `PageSource`s → later; RTL reading → PR4.
