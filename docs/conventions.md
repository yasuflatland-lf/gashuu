# Conventions

This document captures key project conventions migrated from the CLAUDE.md "Conventions" section. These conventions guide implementation and quality standards across the gashuu codebase.

### Language

All comments and identifiers in **English**.

Rust string literals use the `\u{2014}` escape for an em-dash, never a literal `—` (e.g. status text like `"Already in library \u{2014} no new books added."`). Keeps source ASCII and consistent across the codebase.

### TDD and keeping the crate compiling

TDD: keep the crate compiling at every save (write test + implementation so each saved state builds — important when several changes land together or in parallel).

### PR size

Keep a PR ≤ ~1000 production LOC.

### UI styling tokens

All visual tokens (colors, border radii, spacing, font sizes, component sizes, shadow colors) live in ONE `global Theme` at `crates/gashuu/ui/Theme.slint`, sourced from `/DESIGN.md`. UI components must reference `Theme.<token>` (e.g. `Theme.accent`, `Theme.radius-pill`, `Theme.shadow-popover`) and must **not** paste raw hex or length literals inline — `Theme.slint` is the only place those literals appear, so a restyle changes one file.

- When a new token is needed, extend `Theme` rather than hard-coding the value in the component.
- Pre-existing inline hex in older dialogs and `ThumbnailStrip.slint` is grandfathered (out of scope to migrate), but it is **not** a licence to add new inline hex.

Slint-specific: colors encode alpha as `#RRGGBBAA` (e.g. the `…40` byte is ~25% alpha), unlike CSS `rgba()`.

### Test fixtures (no committed binaries)

Tests synthesize fixtures in memory (the `image` crate makes tiny PNGs) plus `tempfile` for filesystem cases — **no committed binary fixtures.** Two exceptions, both committed TEXT not binaries: insta `.snap` files (see [docs/patterns.md](patterns.md)), and (PR7) the base64-encoded RAR `.cbr` fixtures in `crates/gashuu-core/src/test_fixtures.rs` (RAR has no Rust encoder, so they cannot be synthesized in-memory like PNGs/ZIPs).
