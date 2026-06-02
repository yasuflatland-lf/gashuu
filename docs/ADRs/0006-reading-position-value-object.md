# ADR-0006: Model reading position as a core value object (ReadingProgress)

- Status: Accepted
- Decided: 2026-06-02
- Related: [ADR-0002](0002-layered-two-crate-architecture.md) (layered two-crate architecture / core↔UI boundary), [ADR-0005](0005-settings-persistence.md) (versioned JSON persistence)

## Context

The one durable fact the app records is how far the reader got in a book. Before #60 this fact was
three bare `usize`s scattered across layers — `Book.last_page` / `Book.page_count` (core),
`ViewerState.index` (UI), and a `last_page / total` progress derivation duplicated as the
`progress_fraction` free function in the UI crate (`library_model.rs`) — plus the open-time
resume/back-fill RULE living in the UI composition root (`main.rs` `open_and_present`). The
`total == 0` unknown-sentinel guard and the 1-based display offset were re-derived at each call
site; a domain rule (idempotent register + guarded page-count back-fill + resume lookup) sat in the
presentation layer.

## Decision

Name the fact, give it one home, and lift the open-time rule into the domain aggregate.

1. **Name the fact as a core value object.** `ReadingProgress { reached, total }` (immutable,
   `Copy`, headless core) owns the derivation in ONE place — `current()` (1-based, saturating),
   `fraction()` (`0.0..=1.0`, `total == 0 → 0.0`, overshoot clamps), and `is_unread()`. Both the
   carousel and the resume path consume it via `Book::progress()`.
2. **It is TRANSIENT — never serialized.** `library.json` keeps only the bare `last_page` +
   `page_count` fields (LIBRARY_VERSION stays 1), guarded by a serde-shape regression test.
3. **Move the open-time domain rule into the `Library` aggregate** as
   `register_opened(canonical, page_count) -> OpenRegistration { resume, count_changed }`, so
   `main.rs` no longer holds the idempotent-add / `page_count > 0` sentinel guard / resume lookup.

## Alternatives considered

- **(A) Keep the free-function derivation (`progress_fraction`) in the UI crate.** Rejected: it
  duplicates the guard, doesn't own `current`, lives at the wrong altitude (UI), and the resume path
  can't share it.
- **(B) Introduce a project-wide `PageIndex(usize)` newtype** across `spread.rs` / `cache.rs` /
  `ViewerState`. Rejected/deferred (tracked as a separate deferred issue): large blast radius for
  little proven bug-removal; any newtype use is confined to `ReadingProgress` internals for now.
- **(C) Redefine the persisted fact** from "leading page of the last-viewed spread" to "furthest
  page seen". Rejected for THIS decision: out of scope; #60 only NAMES the existing fact, it does not
  change its semantics (documented on the type).

## Consequences

### Positive
- Single source of truth for the `total == 0` guard / 1-based offset / overshoot clamp; the resume +
  carousel can't drift.
- The domain rule is unit-testable in headless core without the UI.
- The core↔UI boundary is tightened: no domain rule remains in the composition root.

### Costs / trade-offs accepted
- `register_opened` does two short linear scans over the small shelf (`set_page_count` + resume
  `find`).
- `ReadingProgress` permits `reached > total` (a stale position past a shrunken book) and tolerates
  it via the clamp rather than rejecting it at construction.
- Persisted semantics remain mode-dependent ("leading-of-last-spread") — a known, documented
  deferral.

## Implementation notes (as-built deltas)

- **No user-visible behavior change**: resume position + carousel progress are identical to before.
- `ReadingProgress` is re-exported from `gashuu-core`; `OpenRegistration` too.
- As shipped in #60, the same invariant was enforced as a headless `debug_assert!` in core and
  respected via a `page_count > 0` guard at the UI call site. This two-layer runtime enforcement was
  later SUPERSEDED by #65, which lifted the invariant into the type system (`NonZeroUsize`) — see the
  Update section below. (The `tracing::warn!` on the `open_file == None` branch is a separate
  condition and remains.)
- A serde-shape test (`reading_progress_is_not_persisted`) locks that the value object never leaks
  into `library.json`.

## Update (#65): unknown total lifted from a 0 sentinel into the type system

The `0 = unknown` page count, originally a bare `usize` sentinel with a runtime guard, was lifted
into the type system:

- `ReadingProgress.total` is now `Option<usize>` (`total() -> Option<usize>`); `fraction()` returns
  `0.0` for `None` and defensively for `Some(0)`.
- `Book::page_count_opt() -> Option<usize>` is the public accessor (the old `page_count() -> usize`
  was removed). The STORAGE is unchanged — `Book.page_count` is still a `usize` with `0` on disk for
  an unknown/old file, `LIBRARY_VERSION` still 1 — and the accessor maps stored `0 → None`.
- `set_page_count(_, NonZeroUsize)` + `register_opened(_, Option<NonZeroUsize>)` make `0`
  unrepresentable at the write boundary, which DISSOLVED both the core `debug_assert` and the UI
  `page_count > 0` guard recorded in the implementation note above. The UI converts at the boundary
  with `NonZeroUsize::new(page_count)` (a zero-page open → `None` → back-fill skipped).

This stayed WITHIN alternative (B)'s deferral: `Option`/`NonZeroUsize` are confined to
`ReadingProgress` and `Book`; there is still NO project-wide `PageIndex` newtype across
`spread.rs` / `cache.rs` / `ViewerState`. The `open_file == None` `tracing::warn!` remains.
