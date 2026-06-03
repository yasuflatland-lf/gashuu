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
- The whole UI now references `Theme.*`; there is **no** grandfathered inline hex. `scripts/check-tokens.sh` blocks any raw color hex (`#rgb`..`#rrggbbaa`) outside `Theme.slint` — treat a hit like a failing gate. The only raw values allowed in components are non-color icon glyph sizes (e.g. a `✕` `font-size`); the guard is hex-only and does not police those.
- Token types: colors `<color>`, radii/font-sizes `<length>`, motion `<duration>`, font weights `<int>`. Migrating a legacy file to `Theme.*` may deliberately CHANGE rendered values — re-basing to the dark palette is the goal, not a 1:1 hex-preserving swap.

Slint-specific: colors encode alpha as `#RRGGBBAA` (e.g. the `…40` byte is ~25% alpha), unlike CSS `rgba()`.

**Golden-ratio radius tokens** (introduced/refined in PR#83 and PR#88): `Theme.nav-search-radius` (search field corner radius) and `Theme.nav-pill-radius` (outer glass pill) are computed as `height / φ²` (≈ 0.382 × height). A radius below `height / 2` yields a rounded rectangle; `Theme.radius-pill` (`9999px`) yields a stadium/oval. Deriving radii from component height via φ keeps proportions harmonious without hard-coding lengths — a concrete example of the token-driven, no-inline-values rule.

### Shared Slint components

Reusable atoms/molecules live one per file under `crates/gashuu/ui/components/` (e.g. `ProgressBar`, `Chip`, `PrimaryButton`, `ThumbnailCell`, `ViewerPill`); see [docs/architecture.md](architecture.md) for the current inventory. Conventions for a component there:

- **One `export` per file**, named after the file. A file-private helper sub-component (e.g. `NavBar`'s file-private `SearchField` or `ViewerPill`'s `PageJumpField`) stays un-`export`ed.
- **Reference `Theme.*` via `import { Theme } from "../Theme.slint";`** — no inline color hex (the recursive `scripts/check-tokens.sh` guard now covers `ui/components/` too).
- **Consumers import via `import { X } from "components/X.slint";`** (path relative to the importing file). No `build.rs` change is needed — `build.rs` compiles the single entry `ui/ViewerWindow.slint` and `import` statements cascade.
- **Keep the public API minimal:** in-props plus one callback where possible. Model mutually-exclusive states (focused / loaded / failed / highlighted) as explicit boolean in-props (Figma-variant parity), not derived in-Slint checks, so the call site stays declarative and the states can't drift.

### Splitting oversized modules — external test file via `#[path]`

When a module file grows too large, relocate its `#[cfg(test)] mod tests { … }` to a sibling file `<module>/tests.rs` and include it with:

```rust
#[cfg(test)]
#[path = "<module>/tests.rs"]
mod tests;
```

Key facts: `#[path]` resolves relative to the **parent file's directory** (`src/`), not the crate root; `use super::*;` in the moved file still resolves to the parent module; no `mod.rs` is needed. Mechanics: extract the brace-body, write the 3-line stub, then run `cargo fmt` — rustfmt de-indents the moved block automatically. This is the preferred file-shrinking technique for the ongoing refactor set. (`viewer_state` is the first module to use it; previously every module kept tests inline.)

### Extracting a collaborator-threading fn — field-alias verbatim move + `pub(crate)` bridge

Two rules validated by PR67 (extracting the open-a-book use case into `app::OpenBookUseCase`); full rationale in [patterns.md](patterns.md) ("Use-case object").

1. **Field-alias verbatim move.** When extracting a fn that threads many shared collaborators into a `…UseCase::run` method, store the collaborators as fields and alias them to locals at the top of `run` (`let state = &self.state;` per field) so the moved body stays BYTE-IDENTICAL. `Deref` absorbs the `&Rc<T>` (field) vs `&T` (former parameter) difference for method calls, so no statement in the body changes — minimal diff, borrow-discipline comments preserved verbatim.

2. **`pub(crate)` bridge for module extraction under parallel writers.** Keep shared helpers (`refresh`/`reconcile_settings`/`write_back_position`) in `main.rs` but raise them to `pub(crate)` so the new module calls them via `crate::`. This lets a 2-file split (new module ∥ caller edit) be written by parallel no-cargo agents against an exact API contract, then verified once by the gates.

### Test fixtures (no committed binaries)

Tests synthesize fixtures in memory (the `image` crate makes tiny PNGs) plus `tempfile` for filesystem cases — **no committed binary fixtures.** Two exceptions, both committed TEXT not binaries: insta `.snap` files (see [docs/patterns.md](patterns.md)), and (PR7) the base64-encoded RAR `.cbr` fixtures in `crates/gashuu-core/src/test_fixtures.rs` (RAR has no Rust encoder, so they cannot be synthesized in-memory like PNGs/ZIPs).

### Validated value objects must not derive `Deserialize`

A type that enforces an invariant in its constructor (e.g. `CacheConfig::new` clamps
`capacity >= 1`) must NOT `#[derive(Deserialize)]`: serde would populate its private fields
directly and bypass the constructor, allowing invalid states from a corrupt or hand-edited file.
Persist the raw primitives on a plain struct (`Settings`) and build the validated object on read
via a getter (`Settings::cache_config()`). Full pattern: see
[patterns.md](patterns.md) ("Value objects own their invariants"). For the second value-object
flavor — a cohesion wrapper that bundles always-co-travelling args and delegates to existing free
fns (no invariant enforced, no Deserialize concern) — see
[patterns.md](patterns.md) ("Cohesion-wrapper value object").
