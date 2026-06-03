# ADR-0007: Per-book view overrides with a global fallback

- Status: Accepted
- Decided: 2026-06-03
- Related: [ADR-0005](0005-settings-persistence.md) (versioned JSON persistence / `Settings`),
  [ADR-0006](0006-reading-position-value-object.md) (the per-book reading-position value object),
  [ADR-0002](0002-layered-two-crate-architecture.md) (core↔UI boundary)

## Context

The four view preferences — `reading_direction`, `spread_mode`, `cover_mode`, `fit_mode` — were
GLOBAL: a single value in `Settings` applied to every book. But these preferences are naturally
per-book (a manga reads RTL while a Western comic reads LTR; a wide art book wants double spreads
while a phone-format webtoon wants single). Readers expect a book to reopen in the layout they left
it in, without re-toggling. The change must (a) keep a sensible global DEFAULT for never-touched
books, (b) keep `library.json` byte-compatible with files written before the feature, and (c) not
let one book's preference silently overwrite the global default.

## Decision

Model the four preferences as a PER-BOOK override that falls back to the global `Settings`.

1. **Uniform 4-field override — no book-intrinsic vs. environment split.** All four modes are
   treated identically as overridable, even though one could argue `reading_direction` is
   book-INTRINSIC (a property of the work) while `spread_mode`/`fit_mode` are ENVIRONMENT (a
   property of the screen/window). That split is DELIBERATELY DEFERRED, not rejected: the
   per-field `Option<Enum>` shape preserves room to later make a subset inherit-only or
   environment-scoped without reshaping storage.
2. **A persisted PARTIAL + a transient TOTAL.** `ViewOverride` (persisted on `Book`) is partial —
   one `Option<Enum>` per field, `None` = inherit the global default (an ACTIVE choice, not
   "unknown"). `ResolvedView` (transient, never persisted) is total — every field concrete,
   produced ONLY by `ViewOverride::resolve(&Settings)`, the single definition of the per-field
   `unwrap_or(global)` merge. This makes "an unresolved view reaches the renderer" unrepresentable.
   See [patterns.md](../patterns.md), "Partial/total override pair".
3. **Persist on `Book`, backward-compatible.** `Book.overrides: ViewOverride` is
   `#[serde(default, skip_serializing_if = "ViewOverride::is_empty")]`, with per-field
   `skip_serializing_if = "Option::is_none"`, so an all-`None` override emits NO key and an old
   `library.json` round-trips byte-identically. `LIBRARY_VERSION` is unchanged (same mechanism as
   `Book::page_count` / `Settings`' forward-compat fields).
4. **Scope by screen.** ONE `SettingsDialog` edits different targets by `ui.get_screen()`: the
   Library screen edits the GLOBAL `Settings` defaults; the Viewer screen edits the current book's
   override. The Viewer dialog also exposes a "Reset to global" button
   (→ `ViewOverride::none()`).
5. **Two NAMED writes, one per scope.** `reconcile_settings` writes runtime → GLOBAL `Settings`
   (only via the Library settings dialog and the no-book-open exit path);
   `write_back_view_override` writes runtime → the open book's per-book override (at every leave
   point). The aggregate owns the mutation (`Library::set_overrides` / `overrides_for`); there is no
   `Book` setter.

## Alternatives considered

- **(A) Keep view modes global only.** Rejected: a reader with both manga and Western comics must
  re-toggle direction on every switch; nothing is remembered per book.
- **(B) Split book-intrinsic (direction) from environment (spread/fit) and scope each differently.**
  Deferred, not rejected — see Decision 1. Building it now would add UI/semantic complexity before
  there is evidence the split matters; the `Option<Enum>`-per-field shape keeps the door open.
- **(C) Persist only the fields the user explicitly CHANGED (change-tracking), so a book keeps
  inheriting global for untouched modes.** Rejected for the first cut: the leave-point write-back
  snapshots the whole runtime tuple, which pins all four fields to `Some` after the first leave. A
  precise diff against the resolved baseline at every toggle is more state and more failure modes;
  the "Reset to global" button covers the escape hatch. The partial shape still allows
  change-tracking later with no storage change. (Trade-off recorded in
  [patterns.md](../patterns.md), "write-back-at-leave-point".)
- **(D) Bump `LIBRARY_VERSION` + migrate for the new field.** Unnecessary: a defaulted, empty-skipped
  nested field is forward/backward-compatible (ADR-0005's `#[serde(default)]` mechanism).

## Consequences

### Positive

- A book reopens in the layout it was left in; never-touched books follow the global default.
- Old `library.json` files load unchanged; untouched books stay byte-identical on the next save (the
  empty override emits no key).
- The merge rule lives in exactly one place (`resolve`), unit-tested with per-field ISOLATION tests
  (each field resolves to its OWN global field) that guard a field-swap copy/paste bug.
- The core stays headless: `ViewOverride`/`ResolvedView` carry no `slint`/`tracing`; the UI consumes
  `ResolvedView` via `apply_resolved_view` (+ `ViewportState::set_fit`).

### Costs / trade-offs accepted

- After the first leave from a book, its override pins ALL FOUR fields to `Some` (full override) — it
  stops tracking later global changes for those modes until reset. The Viewer dialog's "Reset to
  global defaults" clears it back to all-`None`.
- The book-intrinsic vs. environment distinction is unmodeled (Decision 1 / Alternative B); a window
  resize on one machine writes back a `spread_mode` that travels with the book.

## Implementation notes (as-built deltas)

- **A shipped-then-caught clobber bug.** When a global-only setting becomes per-context, EVERY
  runtime→global write (`reconcile_settings`) becomes a potential clobber. The plan gated the EXIT
  reconcile on "no book open" but MISSED a second reconcile on the open path (inside
  `OpenBookUseCase::run`, behind the `track_recent_files` save gate): runtime modes are NOT reset on
  open (the new book's `ResolvedView` is applied later), so that save wrote the OUTGOING book's
  per-book modes into the global defaults. Fixed by NOT reconciling on the open-time save. Recorded
  as the headline harness in [patterns.md](../patterns.md), "Write-direction invariant audit".
- The invariant, stated explicitly: GLOBAL view modes are written ONLY by the Library settings dialog
  close and the no-book-open exit path; PER-BOOK overrides ONLY at leave points
  (`write_back_view_override`).
- No new dependencies; `LIBRARY_VERSION` unchanged.
