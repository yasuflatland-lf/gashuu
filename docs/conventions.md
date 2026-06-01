# Conventions

This document captures key project conventions migrated from the CLAUDE.md "Conventions" section. These conventions guide implementation and quality standards across the gashuu codebase.

### Language

All comments and identifiers in **English**.

### TDD and keeping the crate compiling

TDD: keep the crate compiling at every save (write test + implementation so each saved state builds — important when several changes land together or in parallel).

### PR size

Keep a PR ≤ ~1000 production LOC.

### Test fixtures (no committed binaries)

Tests synthesize fixtures in memory (the `image` crate makes tiny PNGs) plus `tempfile` for filesystem cases — **no committed binary fixtures.** Two exceptions, both committed TEXT not binaries: insta `.snap` files (see Patterns & gotchas), and (PR7) the base64-encoded RAR `.cbr` fixtures in `crates/gashuu-core/src/test_fixtures.rs` (RAR has no Rust encoder, so they cannot be synthesized in-memory like PNGs/ZIPs).
