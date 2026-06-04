# ADR-0008: Adopt Fluent (.ftl) as the single i18n catalog via i18n-embed

- Status: Proposed
- Decided: 2026-06-04
- Related: [ADR-0001](0001-gui-framework-slint.md) (Slint's translation infrastructure is the key
  constraint), [ADR-0002](0002-layered-two-crate-architecture.md) (core↔UI boundary governs where
  Fluent machinery may live)

## Context

Today the app maintains TWO catalogs that must stay in vocabulary lockstep:

1. **Slint `@tr()` + gettext** — `crates/gashuu/translations/ja/LC_MESSAGES/gashuu.po` holds 56
   msgids for strings declared inside `.slint` files. Slint's bundled-translation mechanism is
   gettext-only by design; there is no upstream support for alternative catalog formats
   (https://slint.dev/blog/translation-infrastructure, slint-ui/slint#33).
2. **`src/messages.rs`** — 17 `pub(crate) fn msg_*` functions, each an exhaustive `match` on
   `Language`, translate the strings composed in Rust (status line, open/save notices, decode
   errors). Every function must mirror the vocabulary of the `.po` file (e.g. the spread-mode
   labels must agree); a code comment enforces this by instruction, not by tooling.

Every vocabulary addition must be made twice, and cross-catalog consistency is checked only by
hand. There is no `xgettext`-style extraction from Rust source so the Rust catalog cannot be
verified mechanically against the `.po` file. Fluent (https://projectfluent.org) would unify both
catalogs into one format with named arguments for word-order safety, but there is no documented
prior art for Fluent + Slint.

## Decision

Migrate to ONE Fluent `.ftl` catalog per locale (`en`, `ja`) under
`crates/gashuu/i18n/`, loaded with `i18n-embed` (`FluentLanguageLoader`, assets embedded via
`rust-embed`) and consumed via `i18n-embed-fl`'s `fl!()` macro.

1. **Single catalog.** All translatable strings — both the former gettext msgids and the former
   `msg_*` messages — move into the `.ftl` files. The `.po` file and `messages.rs` are removed in
   the final PR of the migration.

2. **Slint bypass via a `global Strings` push.** Slint's `@tr()` cannot consume Fluent. The
   sanctioned workaround (from PR-2 onward): a Slint `global Strings { … }` of string properties
   is populated from Rust. Rust resolves every string through `fl!()` and sets `Strings.*` on the
   component handle; `.slint` bindings read `Strings.*` instead of `@tr()`. This keeps the `.slint`
   markup declarative while the Fluent resolution stays entirely in Rust.

3. **Two-tier compile-time + runtime safety net.** The exhaustive-match guarantee of `messages.rs`
   is replaced by:
   - `fl!()` validates all referenced message IDs against the fallback (`en`) catalog at compile
     time — a missing or misspelled ID is a build error.
   - `langid_for(Language)` is an exhaustive `match` on the `Language` enum so a new variant fails
     compilation until a catalog is wired.
   - A completeness integration test parses both `.ftl` files with `fluent-syntax` and asserts
     identical message-ID sets — covering the gap `fl!()` cannot see (missing translations in
     non-fallback locales).

4. **Bidi isolation disabled.** `FluentLanguageLoader::set_use_isolating(false)` — the app is not
   bidi and legacy strings are pinned byte-identical by tests.

5. **Staged 4-PR migration; both systems coexist until the last.** No single PR is a big-bang
   cutover:
   - **PR-1 (#112)** — foundation: `i18n-embed`/`rust-embed` deps, `en` + `ja` `.ftl` catalogs
     (full vocabulary), loader init, completeness test. Zero behavior change; gettext and
     `messages.rs` still active.
   - **PR-2 (#113)** — wire the `Strings` global; push Fluent strings into Slint; remove `@tr()`
     call-sites one section at a time.
   - **PR-3 (#114)** — cut `messages.rs` call-sites over to `fl!()`; dynamic strings served by
     Fluent.
   - **PR-4 (#115)** — delete gettext scaffolding (`.po`, `.slint` `@tr()` residue,
     `messages.rs`), finalize.

6. **Crate boundary preserved.** The `Language` enum stays in headless `gashuu-core`. ALL Fluent
   machinery (`i18n-embed`, `rust-embed`, `fl!()` call-sites, the `Strings` push logic) lives
   exclusively in the `gashuu` presentation crate, consistent with ADR-0002.

## Consequences

### Positive

- One catalog format and one vocabulary source of truth; vocabulary drift between the two old
  catalogs becomes impossible.
- Named arguments are word-order-safe: Japanese verb-final ordering (e.g. `{ $label }を減らす`)
  and English prefix ordering are expressed naturally in the same message pattern without ad-hoc
  string concatenation.
- Fluent's native multiline value syntax and attribute support improve translator ergonomics.
- `fl!()` compile-time ID validation + the completeness test give stronger mechanical guarantees
  than the old by-convention lockstep comment.

### Costs / trade-offs accepted

- No `xgettext`-style automatic extraction from source: message IDs are hand-maintained.
  Mitigated by the completeness test catching ID-set drift between locales; `fl!()`'s compile-time
  check catches misspelled or removed IDs in Rust code.
- Pioneering path — no known Fluent + Slint prior art. Slint behaviors (e.g. the `global Strings`
  push pattern) were verified empirically rather than by reference to existing integrations.
- `fluent-rs` is pre-1.0; crate versions are pinned and APIs may drift across minor releases.
- Fluent trims leading whitespace from message patterns. Values that must begin with whitespace
  (e.g. ` • item`) require a string-literal placeable (`{" "} • item`).

## Implementation notes (as-built deltas — PR-1, #112)

PR-1 landed the foundation only (deps, `en`/`ja` catalogs with full vocabulary, the `i18n/`
localizer module, and the completeness/parity tests); gettext and `messages.rs` are still live, so
this ADR stays `Proposed` until the PR-4 (#115) cutover. Deltas found while building the loader,
with the full harness in [docs/patterns.md](../patterns.md) ("Fluent loader", "Fluent catalog
authoring gotchas", "i18n test harness"):

- **No `load_fallback_language` preload.** `i18n-embed` 0.16's `load_languages` already auto-appends
  the `en` fallback and atomically REPLACES all loader state, so a preceding fallback load is
  redundant (its effect is discarded). The fallback is structurally guaranteed but not reported by
  `current_languages()` and not behaviorally observable while catalogs are in ID lockstep.
- **`set_use_isolating(false)` is re-applied after EVERY `load_languages`** (decision point 4): the
  swap rebuilds bundles with the default (isolating on), so a single boot-time call is insufficient.
- **Two-tier safety net beyond the design (decision point 3):** alongside `fl!()` and the ID-set
  completeness test, PR-1 added a cross-locale `$arg`-set parity test (catches a per-locale arg-name
  typo `fl!()` can't see) and a duplicate-message-ID guard (the parser does not error on duplicates).
- **Load-failure policy:** `Localizer::new`/`switch` `panic!` on a load error (compile-time-embedded
  assets ⇒ programmer error), deliberately asymmetric to the gettext path's never-fatal warn.
- The message-ID naming convention adopted here is recorded in
  [docs/conventions.md](../conventions.md) ("Fluent catalog message IDs").
