# Conventions

This document captures key project conventions migrated from the CLAUDE.md "Conventions" section. These conventions guide implementation and quality standards across the gashuu codebase.

### Language

All comments and identifiers in **English**.

Rust string literals use the `\u{2014}` escape for an em-dash, never a literal `—` (e.g. status text like `"Already in library \u{2014} no new books added."`). Keeps source ASCII and consistent across the codebase.

### TDD and keeping the crate compiling

TDD: keep the crate compiling at every save (write test + implementation so each saved state builds — important when several changes land together or in parallel).

### PR size

Keep a PR ≤ ~1000 production LOC.

### Commit messages

Every commit subject MUST follow [Conventional Commits 1.0.0](https://www.conventionalcommits.org/ja/v1.0.0/) ([English version](https://www.conventionalcommits.org/en/v1.0.0/)): `type(scope): description` or `type: description`.

- Allowed types (closed set — extend this list via a docs PR, not ad hoc): `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `style`, `perf`, `ci`, `build`.
- Scope is optional; use the module or area touched (`i18n`, `ui`, `core`, `viewer_state`, `adr`, ...).
- **Exactly one line.** No body, no footer, no `Co-Authored-By` or other trailers.
- Aim for ≤72 characters, imperative mood, no trailing period, English.
- Breaking changes use the `!` marker (`feat(core)!: ...`) — the `BREAKING CHANGE:` footer form is unavailable because messages are single-line.
- GitHub-generated merge commits (the auto-generated `Merge ...` subject) are exempt.

### UI styling tokens

All visual tokens (colors, border radii, spacing, font sizes, component sizes, shadow colors) live in ONE `global Theme` at `crates/gashuu/ui/Theme.slint`, sourced from `/DESIGN.md`. UI components must reference `Theme.<token>` (e.g. `Theme.accent`, `Theme.radius-pill`, `Theme.shadow-popover`) and must **not** paste raw hex or length literals inline — `Theme.slint` is the only place those literals appear, so a restyle changes one file.

- When a new token is needed, extend `Theme` rather than hard-coding the value in the component.
- The whole UI now references `Theme.*`; there is **no** grandfathered inline hex. `scripts/check-tokens.sh` blocks any raw color hex (`#rgb`..`#rrggbbaa`) outside `Theme.slint` — treat a hit like a failing gate. The only raw values allowed in components are non-color icon glyph sizes (e.g. a `✕` `font-size`); the guard is hex-only and does not police those.
- **`check-tokens.sh` reads a `#` + 3–8 hex-ish chars as a color — including issue refs in COMMENTS.** A comment like `// (issue #102)` trips the guard because `#102` matches `#[0-9a-fA-F]{3,8}` (the digits are valid hex); 1–2-digit refs like `#88`/`#71` are too short to match and slip through. In `.slint` files write issue numbers WITHOUT the `#` (e.g. `issue 102`); the `#NN` form is only safe up to two digits. (Markdown docs are not scanned, so `#102` is fine in this file.) Precedent: `SettingsDialog.slint` "issue 103/104", `Strings.slint` "issue 113".
- Token types: colors `<color>`, radii/font-sizes `<length>`, motion `<duration>`, font weights `<int>`. Migrating a legacy file to `Theme.*` may deliberately CHANGE rendered values — re-basing to the dark palette is the goal, not a 1:1 hex-preserving swap.
- **Reuse colors before adding tokens.** A restyle should first reach for existing tokens (the scrubber HIG restyle reused `accent` for the fill, `text` for the white knob core, `accent-glow` for the halo — ZERO new color tokens); a new color token is a last resort, not the first move.
- **A length goes in `Theme` when it PAIRS with an existing size token; a one-off stays inline.** `check-tokens.sh` is hex-only and does NOT police lengths, so a single-use size (e.g. a `drop-shadow-blur: 4px`) lives inline per convention — but a value that forms a set with an existing token belongs in `Theme` for cohesion (`scrubber-knob-size-active: 20px` joined `Theme` because it pairs with `scrubber-knob-size: 16px`).
- **A restyle that changes visual SEMANTICS updates DESIGN.md prose AND tokens in the SAME change.** `/DESIGN.md` is the single source of visual truth; when meaning shifts (the HIG restyle moved the accent FROM the knob TO the fill and made the knob a white grabber), update both the `components.*` token blocks and the affected prose (the "single accent" paragraph + the Accent bullet) so the design language stays coherent — here "accent = where you are / progress" is preserved by the fill, and the "one glow = accent-glow" rule is unchanged.

Slint-specific: colors encode alpha as `#RRGGBBAA` (e.g. the `…40` byte is ~25% alpha), unlike CSS `rgba()`.

**Golden-ratio radius tokens**: `Theme.nav-search-radius` (search field corner radius) and `Theme.nav-pill-radius` (outer glass pill) are computed as `height / φ²` (≈ 0.382 × height). A radius below `height / 2` yields a rounded rectangle; `Theme.radius-pill` (`9999px`) yields a stadium/oval. Deriving radii from component height via φ keeps proportions harmonious without hard-coding lengths — a concrete example of the token-driven, no-inline-values rule. The settings panel (issue 103) extends the same φ discipline: `Theme.settings-radius` ALIASES `nav-pill-radius` (so the panel shares NavBar's glass corner language in one place), and the panel height is content-hug (header + body + footer, Marcotte-clamped) with φ relocated into component proportions — toggle track ratio, the 8/14/22 spacing ladder, and segment padding (spec 2026-06-04).

When REMOVING a token, sweep by name, literal value, AND concept phrase over `crates/`, `docs/`, and `DESIGN.md` together, and verify that every `{ns.key}` formula symbol in the DESIGN.md frontmatter is defined — a single name-only grep misses stragglers in docs. See [patterns.md](patterns.md) ("Removing a design token").

### Shared Slint components

Reusable atoms/molecules live one per file under `crates/gashuu/ui/components/` (e.g. `ProgressBar`, `PrimaryButton`, `ThumbnailCell`, `ViewerPill`); see [docs/architecture.md](architecture.md) for the current inventory. Conventions for a component there:

- **One `export` per file**, named after the file. A file-private helper sub-component (e.g. `NavBar`'s file-private `SearchField` or `ViewerPill`'s `PageJumpField`) stays un-`export`ed.
- **Reference `Theme.*` via `import { Theme } from "../Theme.slint";`** — no inline color hex (the recursive `scripts/check-tokens.sh` guard now covers `ui/components/` too).
- **Consumers import via `import { X } from "components/X.slint";`** (path relative to the importing file). No `build.rs` change is needed — `build.rs` compiles the single entry `ui/ViewerWindow.slint` and `import` statements cascade.
- **Keep the public API minimal:** in-props plus one callback where possible. Model mutually-exclusive states (focused / loaded / failed / highlighted) as explicit boolean in-props (Figma-variant parity), not derived in-Slint checks, so the call site stays declarative and the states can't drift.
- **Enforce domain-agnosticism of generic molecules with a grep acceptance criterion (issue 127):** A molecule intended to be reusable across domains (e.g. `ConfirmDialog`) must carry NO domain vocabulary anywhere in its file — including comments, which use the same terms a reviewer or future editor will search for. Add `grep -rn "<domain-term>" <component-file>` → 0 hits to the PR's acceptance criteria and run it before merge. For `ConfirmDialog` this is `grep -rn "delete\|Delete" crates/gashuu/ui/components/ConfirmDialog.slint`. When the component's own explanatory comments need to describe a destructive action, use a generic synonym ("destructive action", "affirmative choice") rather than the domain term — the grep check covers comments verbatim. All display strings must be caller-injected via `in property <string>`, never hardcoded.

### Splitting oversized modules — external test file via `#[path]`

When a module file grows too large, relocate its `#[cfg(test)] mod tests { … }` to a sibling file `<module>/tests.rs` and include it with:

```rust
#[cfg(test)]
#[path = "<module>/tests.rs"]
mod tests;
```

Key facts: `#[path]` resolves relative to the **parent file's directory** (`src/`), not the crate root; `use super::*;` in the moved file still resolves to the parent module; no `mod.rs` is needed. Mechanics: extract the brace-body, write the 3-line stub, then run `cargo fmt` — rustfmt de-indents the moved block automatically. This is the preferred file-shrinking technique for the ongoing refactor set. (`viewer_state` is the first module to use it; previously every module kept tests inline.)

### Extracting a collaborator-threading fn — field-alias verbatim move + `pub(crate)` bridge

Two rules validated by extracting the open-a-book use case into `app::OpenBookUseCase`; full rationale in [patterns.md](patterns.md) ("Use-case object").

1. **Field-alias verbatim move.** When extracting a fn that threads many shared collaborators into a `…UseCase::run` method, store the collaborators as fields and alias them to locals at the top of `run` (`let state = &self.state;` per field) so the moved body stays BYTE-IDENTICAL. `Deref` absorbs the `&Rc<T>` (field) vs `&T` (former parameter) difference for method calls, so no statement in the body changes — minimal diff, borrow-discipline comments preserved verbatim.

2. **`pub(crate)` bridge for module extraction under parallel writers.** Keep shared helpers (`refresh`/`persist_view_modes`/`write_back_position`) in `main.rs` but raise them to `pub(crate)` so the new module calls them via `crate::`. This lets a 2-file split (new module ∥ caller edit) be written by parallel no-cargo agents against an exact API contract, then verified once by the gates.

### Fluent catalog message IDs

The single Fluent catalog (`crates/gashuu/i18n/<lang>/gashuu.ftl`; ADR-0008) names every message
`<screen>-<element>[-<variant>]` in kebab-case. Established prefixes: `settings-`, `guide-`,
`carousel-`, `navbar-`, `shortcuts-`, `viewer-pill-`, `stepper-`, `viewer-`, `notice-`, `common-`.
Accessibility-only strings take an `-a11y` suffix (e.g. `navbar-open-a11y`). A string shared across
screens is ONE message under its PRIMARY owner's prefix — not duplicated per screen (the spread-mode
labels live once and are read by both the settings dialog and the viewer status line; pinned by
`ja_catalog_pins_spread_vocabulary`). This convention governs the i18n migration (#113-#115) and all
new strings. Prefer NAMED args (`{ $label }`) over positional placeholders — they are word-order-safe
for verb-final Japanese; see [patterns.md](patterns.md) ("Fluent catalog authoring gotchas").

### i18n load-failure policy: panic for the catalog we control

The Fluent `Localizer` (`new`/`switch`) `panic!`s on a load failure: assets are
compile-time-embedded and `langid_for` is exhaustive, so a failure is a programmer error, not a
runtime condition — fail LOUD. (The former gettext path used a never-fatal `tracing::warn` instead;
that asymmetry and its rationale — the repo's history of a silent gettext all-miss — are preserved in
[patterns.md](patterns.md) ("Fluent loader").)

### Test fixtures (no committed binaries)

Tests synthesize fixtures in memory (the `image` crate makes tiny PNGs — and tiny AVIFs via its bundled ravif encoder; keep AVIF fixtures a few pixels per side, the rav1e encode is slow in debug builds) plus `tempfile` for filesystem cases — **no committed binary fixtures.** Two exceptions, both committed TEXT not binaries: insta `.snap` files (see [docs/patterns.md](patterns.md)), and the base64-encoded RAR `.cbr` fixtures in `crates/gashuu-core/src/test_fixtures.rs` (RAR has no Rust encoder, so they cannot be synthesized in-memory like PNGs/ZIPs). Even cheaper for PAGE-COUNTING / empty-book tests: a ZERO-BYTE file named `*.png` counts as a page, because `FolderSource` lists by EXTENSION (case-insensitive, `max_depth(1)`) and does not read the bytes — so a temp dir of empty `*.png` files probes to an N-page book, an empty dir probes to `EmptyBook`, and no real image data is needed (used by the `probe_page_count` / `add_paths` tests).

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

### Add an optional nested config to a persisted aggregate without breaking old files

To add an optional nested struct (e.g. `ViewOverride` on `Book`) to an already-persisted aggregate
so that files written before the field existed round-trip byte-identically, use the same
`#[serde(default)]` forward-compat mechanism as a single defaulted field (see
[patterns.md](patterns.md), "Add a persisted core field with `#[serde(default)]`"), with one extra
layer for the all-empty case:

- **Per `Option` field:** `#[serde(default, skip_serializing_if = "Option::is_none")]` — a `None`
  field emits no key.
- **On the aggregate's field:** a struct-level `#[serde(default, skip_serializing_if = "X::is_empty")]`
  (e.g. `ViewOverride::is_empty` == all fields `None`) — so an all-`None` value emits NO `overrides`
  key at all, and an old `library.json` (no `overrides`) deserializes via `default` to the empty
  value. `LIBRARY_VERSION` is unchanged.

Lock it with a THREE-test trio (mirrors the `Book::page_count` schema-test pair, extended for the
all-empty case): (1) **old JSON loads as empty** — a `Book` JSON with no `overrides` key
deserializes to `ViewOverride::none()`; (2) **empty is omitted from JSON** — serializing an
all-`None` override emits no `overrides` key (so untouched books stay byte-identical on the next
save); (3) **non-empty round-trips** — a fully-set override serializes and deserializes back equal,
exercising ALL fields (a partial round-trip test can mask a single-field serde typo). Add these to
the value object's own per-field merge ISOLATION tests (see
[patterns.md](patterns.md), "Partial/total override pair"), which guard the resolve rule against a
field-swap copy/paste bug.
