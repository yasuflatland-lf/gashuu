# ADR-0008: Adopt Fluent (.ftl) as the single i18n catalog via i18n-embed

- Status: Accepted
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
   sanctioned workaround (introduced in stage 2): a Slint `global Strings { … }` of string properties
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

5. **Staged migration; both systems coexist until the last step.** No single step is a big-bang
   cutover. The work is split into four stages:
   - **Stage 1 (#112)** — foundation: `i18n-embed`/`rust-embed` deps, `en` + `ja` `.ftl` catalogs
     (full vocabulary), loader init, completeness test. Zero behavior change; gettext and
     `messages.rs` still active.
   - **Stage 2 (#113)** — wire the `Strings` global; push Fluent strings into Slint; remove `@tr()`
     call-sites one section at a time.
   - **Stage 3 (#114)** — cut `messages.rs` call-sites over to `fl!()`; dynamic strings served by
     Fluent.
   - **Stage 4 (#115)** — delete gettext scaffolding (`.po`, `.slint` `@tr()` residue,
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

## Implementation notes (as-built deltas — stage 1, #112)

Stage 1 landed the foundation only (deps, `en`/`ja` catalogs with full vocabulary, the `i18n/`
localizer module, and the completeness/parity tests); gettext and `messages.rs` are still live, so
this ADR stayed `Proposed` until the stage-4 (#115) cutover. Deltas found while building the loader,
with the full harness in [docs/patterns.md](../patterns.md) ("Fluent loader", "Fluent catalog
authoring gotchas", "i18n test harness"):

- **No `load_fallback_language` preload.** `i18n-embed` 0.16's `load_languages` already auto-appends
  the `en` fallback and atomically REPLACES all loader state, so a preceding fallback load is
  redundant (its effect is discarded). The fallback is structurally guaranteed but not reported by
  `current_languages()` and not behaviorally observable while catalogs are in ID lockstep.
- **`set_use_isolating(false)` is re-applied after EVERY `load_languages`** (decision point 4): the
  swap rebuilds bundles with the default (isolating on), so a single boot-time call is insufficient.
- **Two-tier safety net beyond the design (decision point 3):** alongside `fl!()` and the ID-set
  completeness test, stage 1 added a cross-locale `$arg`-set parity test (catches a per-locale arg-name
  typo `fl!()` can't see) and a duplicate-message-ID guard (the parser does not error on duplicates).
- **Load-failure policy:** `Localizer::new`/`switch` `panic!` on a load error (compile-time-embedded
  assets ⇒ programmer error), deliberately asymmetric to the gettext path's never-fatal warn.
- The message-ID naming convention adopted here is recorded in
  [docs/conventions.md](../conventions.md) ("Fluent catalog message IDs").

## Implementation notes (as-built deltas — stage 2, #113)

Stage 2 landed decision point 2 (the `global Strings` push) and removed every `@tr()` call-site; the
gettext machinery is left INERT (not deleted) for rollback, so this ADR stayed `Proposed` until the
stage-4 (#115) cutover. Full harness in [docs/patterns.md](../patterns.md) ("The `Strings`-global
push", "Word-order-safe composed a11y labels", "The gettext bundler is keyed by live `@tr()`").

- **`Strings` global + `apply()` chokepoint.** `ui/Strings.slint` declares 67 string properties with
  English-literal defaults; `Localizer::apply(&ui)` resolves them all via `fl!()` and pushes them at
  boot and after each `switch()`. The global is re-exported from `ViewerWindow.slint` (the build
  entry) or Slint generates no `ui.global::<Strings>()` accessor.
- **`in`-property verdict (open question resolved):** plain `in property <string>` (NOT `in-out`)
  suffices for Rust-side `set_*` on Slint 1.16.1 — verified empirically.
- **Canary half-retired (decision point 3 interaction).** Removing all `@tr()` leaves the gettext
  bundler with nothing to compile, so the OUT_DIR canary can no longer assert Japanese *text* in
  generated code; it now asserts only that the `"ja"` locale slot is registered (a loud guard if the
  build flags are ripped out early), and the text guarantee moved to the loader test
  `ja_catalog_pins_spread_vocabulary`.
- **Zero `@tr()` remain** in `.slint`, yet the `.po` + `build.rs` `with_bundled_translations` +
  `select_ui_language` stay in place as an inert rollback surface until #115.

## Implementation notes (as-built deltas — stage 3, #114)

Stage 3 landed decision point 1's Rust half (decision point 3 stays in force): every `messages.rs`
call-site moved to `fl!()`, so dynamic strings are now served by Fluent. gettext is still inert (the
`.po` + build flags survive for one more stage), so this ADR stayed `Proposed` until the stage-4 (#115)
cutover. Full harness in [docs/patterns.md](../patterns.md) ("Neutral content structs", "`OpenOutcome`
+ `finalize_open`", "`fl!()` numeric arg type", "Word-order-safe composed a11y labels").

- **`messages.rs` deleted, replaced by `src/i18n/dynamic.rs`.** The exhaustive-match Rust catalog
  (17 `msg_*` functions, each a `match` on `Language`) is gone; `dynamic.rs` exposes one typed
  `pub(crate) fn` per former `msg_*`, each taking `&FluentLanguageLoader` and NO `Language` param —
  the loader carries the active locale, so the per-function enum dispatch disappears (enum dispatch
  survives only where the *argument* is an enum, e.g. `spread_label(SpreadMode)`).
- **Neutral content structs decouple domain state from formatting — structurally, not by convention.**
  `StatusContent`/`StatusKind` (in `viewer_state.rs`) and `NoticesContent`/`OpenOutcome`/`SkippedDetail`
  (in `app.rs`) carry plain domain facts; the `fl!()` formatting lives only in `dynamic.rs`'s
  aggregators (`format_status`, `format_notices`). Consequently `viewer_state.rs` and `app.rs` now have
  ZERO `crate::i18n` imports — a single grep verifies the boundary, so the decoupling is enforced by
  the import graph rather than by discipline.
- **`refresh()` gained a `loader` parameter; `finalize_open()` deduplicates the three open-flow
  call-sites.** Both helpers live in `main.rs` — the layer that owns BOTH the active locale (`Localizer`)
  and the UI handle — so threading the loader there avoids a dependency cycle with `app.rs` (which stays
  i18n-free per the delta above). `finalize_open` is the single place the `OpenOutcome` match + the
  notice-append loop appear.
- **`Localizer::loader()` getter added now — the defer-getters-until-consumed rule (stage 1) discharged.**
  Stage 1 deliberately did not expose the loader; stage 3 is its first real cross-file consumer (`dynamic.rs`),
  so the getter landed exactly when a caller existed, not speculatively.
- **`OpenOutcome::Error(String)` pre-captures `format!("{e}")` at the `CoreError` site.** The error is
  flattened to a `String` inside `app.rs` (where `CoreError` is in scope) so no `CoreError` ever escapes
  the module as a type. Production formats the captured string via `open_error_str(&str)`; the
  `open_error(&dyn Display)` flavor is reachable only from tests (`#[cfg(test)]` — the function does
  not exist in non-test builds), making the production-vs-test split compiler-enforced rather than
  documented.
- **`fl!()` numeric args require explicit `usize as i64` casts.** Fluent does not coerce `usize`; every
  numeric call-site (`page`, `n`, `skipped`) casts to `i64` first — consistent across `dynamic.rs`.
- **Test-guarantee migration used a named-successor audit.** Byte-exact content pins, the
  cross-locale-differ assertions, and the args-embedded test families were each carried into the `i18n`
  test module under named successors (no guarantee silently dropped). 515 tests passing at stage-3 HEAD.

## Implementation notes (as-built deltas — stage 4, #115)

Stage 4 is the one-way door: it deletes the gettext scaffolding the prior stages kept inert, so decision
point 1 is complete and this ADR flips to `Accepted`. Full harness in
[docs/patterns.md](../patterns.md).

- **The excision as executed.** `build.rs` lost `.with_bundled_translations("translations")` and
  `.with_default_translation_context(DefaultTranslationContext::None)`; the
  `crates/gashuu/translations/ja/LC_MESSAGES/gashuu.po` tree was deleted; `select_ui_language` and both
  of its call-sites were removed. The `with_style("fluent-dark")` build flag STAYS — it is the Slint
  widget visual style (the cross-platform Fluent Design widget set) — an unrelated naming collision
  with Mozilla Fluent, not part of the i18n machinery.
  Removing `select_ui_language` also retires the old "must run after the first component is created"
  ordering constraint: `Localizer` has no such requirement.
- **OUT_DIR canary retired.** The `bundled_translations_compiled_into_generated_code` test (which
  inspected the gettext bundler's generated code) was deleted; its guarantee is now carried by
  `i18n::tests::ja_catalog_pins_spread_vocabulary` plus `fl!()`'s compile-time ID validation.
- **Extra scope beyond the issue: `ftl_static_channel_covers_every_po_msgid` also deleted.** It
  `include_str!`-read the `.po` file, so it could not survive the tree deletion. Its completeness
  guarantee was already double-covered by `all_ftl_ids_present_in_every_locale` + `fl!()`'s compile
  checks; the `.po` byte-oracle tests were transitional bridges by design.
- **One-way door now closed.** There is no `@tr()`/`.po` surface left to revert onto; regressions in the
  Fluent path are fixed forward, not rolled back.
