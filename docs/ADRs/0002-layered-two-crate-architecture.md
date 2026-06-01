# ADR-0002: Use Rust with a two-crate workspace, layered architecture

- Status: Accepted
- Decided: 2026-05-31 (transcribed: 2026-06-01)
- Related: [ADR-0001](0001-gui-framework-slint.md) (GUI framework), [ADR-0004](0004-archive-abstraction-and-extraction.md) (`PageSource`)

## Context

Having chosen Slint for the UI ([ADR-0001](0001-gui-framework-slint.md)), we need a project
structure that:

- keeps the domain and I/O logic **unit-testable without a display server** (a Slint window cannot
  be created in headless CI);
- gives clear, reviewable PR boundaries as features land;
- prevents UI concerns (Slint types, pixel buffers, logging) from leaking into the core.

The reference repository `simple-archiver` follows a DDD layered / hexagonal layout. We borrow its
Rust-side conventions (workspace split, testing, CI) — but **not** its UI stack, which is Tauri +
React.

## Decision

Structure the project as a **Cargo workspace split into two crates** with a single, one-way
dependency (`gashuu` → `gashuu-core`):

- `crates/gashuu-core` — Slint-independent domain + I/O. Owns the `PageSource` trait, archive
  sources, `image_ops` decode, `ImageCache`, spread/viewport geometry, and `Settings`.
- `crates/gashuu` — the Slint presentation layer. The only place that touches Slint, pixel
  buffers, and logging.

The **core↔UI boundary is RGBA bytes + dimensions**: core returns decoded `DecodedImage`
(raw RGBA8 + width/height), and the UI converts via `slint::Image::from_rgba8()`. Core never
returns a Slint type.

Within each crate, keep a three-layer separation: presentation (`.slint`) / application logic
(`ViewerState`, `ImageCache`, `Settings`) / I/O & decode (`PageSource` impls, `image_ops`).

## Alternatives considered

- **Single crate** — simpler, but the domain code would compile against Slint and could not be
  tested headless; PR boundaries would blur. Rejected.
- **Pass Slint types into core** (e.g. build `slint::Image` in core) — would couple the domain to
  the renderer and break mockability. Rejected in favor of the RGBA-bytes boundary.

## Consequences

### Positive
- The core is mockable: `PageSource` exposes `mockall::automock` behind a `testing` feature, so
  `gashuu` tests use `MockPageSource` without pulling `mockall` into release builds.
- CI can run a Slint-free `core` job (build + test + coverage) with no display server; the per-OS
  `app` jobs build the UI. Coverage is measured on `gashuu-core` only.
- The "view must match status" and pure-geometry-in-core patterns (spread.rs, viewport.rs) fall
  out naturally from the boundary.

### Costs / trade-offs accepted
- Crossing the boundary costs one RGBA copy per displayed page (`slint::Image::from_rgba8`), paid
  intentionally to keep the core renderer-agnostic.
- Some test-only accessors in the binary crate need `#[allow(dead_code)]` because `pub` is not a
  public API surface in a binary (documented in place).

## Implementation notes (as-built deltas)

- **Toolchain pin**: the design doc said "Rust 1.75+"; the repository pins Rust to **1.96.0** via
  `mise.toml` (run every cargo command through `mise exec --`). The pin is stricter than the
  design's floor; no decision reversed.
- The realized boundary type is `Arc<dyn PageSource>` rather than `Box<dyn PageSource>` — see
  [ADR-0004](0004-archive-abstraction-and-extraction.md) for why (`Send + Sync` sharing with rayon
  prefetch threads).
