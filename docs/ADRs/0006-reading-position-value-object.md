# ADR-0006: Model reading position as a core value object (ReadingProgress)

- Status: Accepted
- Decided: 2026-06-02
- Related: [ADR-0002](0002-layered-two-crate-architecture.md) (layered two-crate architecture / core↔UI boundary), [ADR-0005](0005-settings-persistence.md) (versioned JSON persistence)

## Context

The one durable fact the app records is how far the reader got in a book. Before #60 this fact was
three bare `usize`s scattered across layers — `Book.last_page` / `Book.page_count` (core),
`ViewerState.index` (UI), and a `last_page / total` progress derivation duplicated as the
`progress_fraction` free function in the UI crate (`library_model.rs`) — plus the open-time
resume/back-fill RULE living in the UI composition root (`main.rs` `open_and_present`, now `app::OpenBookUseCase::run`). The
`total == 0` unknown-sentinel guard and the 1-based display offset were re-derived at each call
site; a domain rule (idempotent register + guarded page-count back-fill + resume lookup) sat in the
presentation layer.

## Decision

> Note: the Decision 1 derivation formulas were amended in 2026-07 (Alternative (C) adopted) —
> see the Amendment section at the end of this record.

Name the fact, give it one home, and lift the open-time rule into the domain aggregate.

1. **Name the fact as a core value object.** `ReadingProgress { reached, total }` (now
   `{ last_viewed, total }`) (immutable, `Copy`, headless core) owns the derivation in ONE place —
   `current()` (1-based, saturating), `fraction()` (`0.0..=1.0`, `total == 0 → 0.0` (now
   `total <= 1 → 0.0`; see Amendment), overshoot clamps), and `is_unread()` (never shipped; now
   `is_at_start()`). Both the carousel and the resume path consume it via `Book::progress()`.
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
  change its semantics (documented on the type) (now ADOPTED by issue #454; see Amendment).

## Consequences

### Positive
- Single source of truth for the `total == 0` guard (now `total <= 1`) / 1-based offset / overshoot
  clamp; the resume + carousel can't drift.
- The domain rule is unit-testable in headless core without the UI.
- The core↔UI boundary is tightened: no domain rule remains in the composition root.

### Costs / trade-offs accepted
- `register_opened` does two short linear scans over the small shelf (`set_page_count` + resume
  `find`).
- `ReadingProgress` permits `reached > total` (now `last_viewed > total - 1`) (a stale position past
  a shrunken book) and tolerates it via the clamp rather than rejecting it at construction.
- Persisted semantics remain mode-dependent ("leading-of-last-spread") — a known, documented
  deferral (now RESOLVED by the final-page exception; see Amendment).

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

- `ReadingProgress.total` is now `Option<usize>` (now `Option<NonZeroUsize>`)
  (`total() -> Option<usize>`); `fraction()` returns `0.0` for `None` and defensively for `Some(0)`
  (`Some(0)` is now unrepresentable; guard is `t <= 1`).
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

## Amendment 2026-07-25: position-normalized fraction — Alternative (C) ADOPTED

Alternative (C) above ("redefine the persisted fact so completion is representable") was recorded
as REJECTED for #60. It was subsequently ADOPTED (issue #454, 2026-07). The Decision 1 derivation
formulas stated above are therefore SUPERSEDED by the ones below; Decisions 2 and 3, and the
whole Update (#65) type-system lift, are unaffected.

### Shipped formulas (authoritative; verbatim from `crates/gashuu-core/src/reading_progress.rs`)

- Fields: `ReadingProgress { last_viewed: usize, total: Option<NonZeroUsize> }` — immutable,
  `Copy`, headless.
- `current() = last_viewed.saturating_add(1)` (1-based display page, always >= 1). Unchanged.
- `fraction()` — POSITION-NORMALIZED, spanning the last page INDEX so the final page reads
  exactly 1.0:

  ```rust
  match self.total {
      Some(t) if t.get() > 1 => (self.last_viewed as f32 / (t.get() - 1) as f32).clamp(0.0, 1.0),
      _ => 0.0,
  }
  ```

- `is_finished() = total.is_some_and(|t| t.get() > 1 && last_viewed >= t.get() - 1)`.
- `is_at_start() = (last_viewed == 0)` — a START-POSITION predicate, not "never read": a book left
  on its first page also satisfies it. It REPLACES the `is_unread()` named in Decision 1, which
  never shipped.
- Write-back rule (`crates/gashuu/src/viewer_state.rs`, `resume_index_to_persist()`): persist
  `total - 1` when the CURRENTLY DISPLAYED spread contains the final page index
  (`spread.leading == total-1 || spread.trailing == Some(total-1)`), otherwise the spread leading
  (`index()` when there is no source). So `last_viewed` stores the leading page of the last-viewed
  spread EXCEPT on the final-page spread, which stores the final page index. Both values normalize
  (`jump_to` -> `SpreadContext::normalize`) onto the SAME spread on reopen.

### Accepted side effects

- A one-page book and an unknown total both yield `fraction() == 0.0` and
  `is_finished() == false`: a one-page book has no meaningful progress bar and is never "finished".
- A stale resume past the final index (`last_viewed > total - 1`, e.g. after the archive shrank)
  still clamps `fraction()` to 1.0 and reports finished; it is tolerated at read time, never
  rejected at construction (unchanged from the original Costs bullet).
- The persisted resume stays a BARE page index and `LIBRARY_VERSION` stays 1 — no migration.
  Resumes saved before the finished-aware write-back therefore reach 100% only after the reader
  revisits the final spread.
- `fraction() == 1 <=> is_finished()` holds for `total > 1` in exact arithmetic; in `f32` it can
  break beyond 2^24 pages (latent, unreachable for real books).
- The original Costs bullet "Persisted semantics remain mode-dependent ('leading-of-last-spread')
  — a known, documented deferral" is RESOLVED by the final-page exception above.

### Superseded statements in this record

| Section (original text kept intact) | Superseded claim | Current truth |
| --- | --- | --- |
| Decision 1 | `ReadingProgress { reached, total }` | fields are `{ last_viewed, total }` |
| Decision 1 | `fraction()` … `total == 0 -> 0.0` | `total` is `None` or `Some(t) with t <= 1` -> `0.0` |
| Decision 1 | `is_unread()` | never shipped; `is_at_start()` is the shipped predicate |
| Alternatives (C) | "Rejected for THIS decision" | ADOPTED (issue #454); see this Amendment |
| Positive, bullet 1 | "the `total == 0` guard" | the `total <= 1` guard |
| Costs, bullet 2 | "permits `reached > total`" | permits `last_viewed > total - 1` |
| Costs, bullet 3 | "Persisted semantics remain mode-dependent … deferral" | resolved (final-page exception) |
| Update (#65), bullet 1 | "`total` is now `Option<usize>`" | the FIELD is `Option<NonZeroUsize>`; the `total()` ACCESSOR still returns `Option<usize>` |
| Update (#65), bullet 1 | "`fraction()` returns `0.0` … defensively for `Some(0)`" | `Some(0)` is unrepresentable (`NonZeroUsize`); the real guard is `t <= 1` |

`Book::page_count_opt() -> Option<usize>` (Update #65, bullet 2) is UNCHANGED and still correct
(`crates/gashuu-core/src/library.rs:120`).

See [architecture.md](../architecture.md), "reading_progress", for the as-built module entry
(already consistent with the formulas above).
