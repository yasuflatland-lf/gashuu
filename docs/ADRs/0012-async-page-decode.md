# ADR-0012: Decode cache-miss pages off the UI thread, carrying the result back

- Status: Accepted
- Decided: 2026-06-10
- Related: [ADR-0002](0002-layered-two-crate-architecture.md) (the core↔UI boundary
  governs why the decode handle is `Send`-and-headless), [ADR-0003](0003-image-loading-and-caching.md)
  (the ±N prefetch LRU this seam shares), [ADR-0010](0010-avif-decode-via-dav1d.md)
  (the AVIF full-decode-in-ctor cost this work moves off-thread rather than reduces)
- Spec / brainstorm: `.claude/plans/async_page_decode.md`

## Context

A cache-MISS page decode ran SYNCHRONOUSLY on the UI thread. `refresh` (`main.rs`)
→ `ViewerState::current_spread` → `ImageCache::get` decoded the page (archive extract
+ JPEG/PNG/AVIF decode + RGBA convert) inline, and a double spread decoded its two
pages SERIALLY. The first spread on open is always a MISS, and a jump beyond the
prefetch radius, a fast turn, a scrub-commit, or a thumbnail click can outrun
`spawn_prefetch` (already async on rayon) and fall back to the synchronous MISS path.
Heavy files — a large AVIF (full decode in the decoder ctor, ADR-0010) or a large
JPEG/PNG — made this a visible freeze of the event loop.

Covers and thumbnails (`cover_loader.rs`) and bulk-add probing were already fully
off-thread; the page view was the last synchronous-decode holdout. The constraint
is the layered architecture (ADR-0002): `gashuu-core` is headless, so the off-thread
decode handle must be Slint-free and `Send`. The hard asymmetry vs. covers is that
the viewport (`Rc<RefCell<ViewportState>>`) and the i18n loader
(`Rc<FluentLanguageLoader>`) are `!Send`, so a worker cannot apply the geometry/status
itself.

## Decision

Carry the decoded result back as explicit data. The rayon worker decodes the page,
caches it in the shared core LRU, and marshals the `Arc<DecodedImage>` back to the UI
thread; the UI side builds the `slint::Image` and applies it.

1. **Carry-result, not re-refresh.** The worker flows the value explicitly rather than
   merely warming the cache and signalling "ready" for the handler to re-run `refresh`.
2. **A `Send`, headless decode handle carved off the inner `Arc`.** `Inner` is already
   `Arc<Inner>` and `Send + Sync` (prefetch hands it to rayon), so `CacheDispatch`
   (`#[derive(Clone)]`, `inner: Arc<Inner>` + `radius`) shares the live LRU and source.
   `ImageCache::get_cached(index) -> Option<Arc<DecodedImage>>` is a pure HIT probe that
   NEVER reads or decodes on a miss (its test asserts the read counter does not increase);
   `CacheDispatch::decode_and_cache(index)` reads + decodes on the calling rayon worker,
   inserts into the shared LRU, and warms neighbours. `decode_and_cache` double-checks
   `inner.cached(index)` after the read so a racing prefetch that already filled the page
   returns an immediate HIT (no re-decode). `ImageCache::get` is refactored to delegate
   (`get_cached` HIT, else `dispatch_handle().decode_and_cache`) and stays `pub` as the
   sync entry for the existing cache tests.
3. **The `!Send` boundary bridges via scalar-only Slint callbacks.** The marshal closure
   is `Send` (`Weak<ViewerWindow>`, `Arc<AtomicUsize>` epoch, `my_epoch`,
   `Arc<DecodedImage>`): inside `slint::invoke_from_event_loop` it builds the
   `slint::Image`, sets the image properties on `ui` via `apply_spread_images`, then
   invokes `spread-anchored(float, float, bool, int, int, int)` or `page-decode-error(int)`.
   The handlers (`on_spread_anchored` / `on_page_decode_error` in `handlers/viewer.rs`),
   holding the `!Send` `Rc` state, clear the dispatch reservations and run
   `apply_spread_geometry` (viewport reanchor + status). `slint::Image`, `Rc`, and
   `VecModel` are never moved across threads. This is the established
   `add-progress`/`add-finalize` idiom from the bulk-add work.
4. **Atomic spread apply; parallel decode for a double-MISS.** `PageController::dispatch_spread`
   spawns one rayon job; when both leading and trailing slots miss, that job uses
   `rayon::join` to decode them in parallel (the direct fix for the serial-decode freeze).
   The result is applied atomically when all needed slots are ready — there is NO
   progressive per-slot display.
5. **Fast-turn staleness handled by an epoch.** `PageController` owns an
   `Arc<AtomicUsize>` epoch; `set_target(leading_idx, trailing_idx, single)` advances it
   ONLY on a real `SpreadTarget` change (an unchanged target returns the current epoch),
   and `set_source` advances it on opening a different book. The marshal closure drops
   its result when `is_current(&epoch, my_epoch)` is false, so superseded generations are
   discarded. A UI-thread-only `dispatched: RefCell<HashSet<usize>>` dedups in-flight
   dispatches per page (`reserve_dispatch` / `clear_dispatched`).

## Alternatives considered

- **(A) Carry-result (chosen).** The worker decodes, caches in the shared LRU, and
  marshals the `Arc<DecodedImage>` back as explicit data. No implicit invariant; failure
  is marshalled once; the only dedup state is a clearly-named `dispatched` set under an
  epoch.
- **(B) Re-refresh.** The worker only warms the cache and signals "ready"; the handler
  re-runs `refresh`, which then HITs. Rejected under the single-maintainer / DDD lens for
  three reasons: it carries an IMPLICIT TEMPORAL INVARIANT ("after the ready signal the
  page is in the cache") true only by capacity/radius coincidence, not by construction;
  it needs a separate `failed` set with reset RESPONSIBILITY to avoid a re-dispatch loop
  on a permanent decode failure; and it introduces a second `in_flight` notion that
  COLLIDES BY NAME with the core's prefetch `in_flight`.

## Consequences

### Positive

- Opening a heavy book and cache-miss page turns (fast turns, far jumps, scrub-commit,
  thumbnail jumps) no longer freeze the event loop; a per-slot placeholder shows, then
  the page appears. HIT page turns stay immediate (no spinner, applied synchronously).
- A double-MISS spread decodes in parallel via `rayon::join` instead of serially.
- The core stays headless: `CacheDispatch` is Slint-free, `get_cached` never decodes,
  and the decode implementation lives in one place (`decode_and_cache`), shared by the
  sync `get` and the async worker.

### Costs / trade-offs accepted

- A MISS shows a per-slot loading placeholder; a single spread has one slot, a double
  spread has two independent slots each with its own placeholder.
- Progressive per-slot double-spread display is out of scope: per-slot geometry would
  fracture the unified zoom/pan content rectangle. The spread is applied atomically.
- The ADR-0010 AVIF full-decode-in-ctor cost is accepted as-is; this work makes that
  cost off-thread, not cheaper.
- Sub-frame HIT→MISS→HIT flicker (a spinner flashing for one frame) is a known
  gate-invisible risk; flicker gating via a one-frame-delay `Timer` is deferred to the
  manual visual check.

## Implementation notes (as-built deltas)

- **`spawn_prefetch` is unchanged.** Only the MISS render path became async; the ±N
  prefetch behaviour (ADR-0003) is untouched, and `get_cached` still spawns neighbour
  prefetch on a HIT exactly as `get` did.
- **`refresh` classifies HIT / MISS / empty only.** A decode FAILURE is never a `refresh`
  outcome (it cannot occur on the synchronous HIT path); it arrives asynchronously via
  the `page-decode-error` callback, which owns the decode-error status + clear. The
  blocking `Some(Err)` arm left `current_spread`. `refresh` gained `&PageController` and
  a `ui_weak` parameter, threaded mechanically through its call sites (the
  `CoverController` threading precedent).
- **`SpreadTarget` / `set_target` and `DispatchStatus` / `reserve_missing_slots` are
  extras beyond the bare spec**, justified by the fast-turn epoch semantics (advance the
  epoch only on a genuine target change) and partner-slot reservation (reserve a newly-
  missing partner even when the other slot is already in flight).
- **The dispatch path is gated `#[cfg(not(test))]`.** `dispatch_spread`, `spawn_decode_spread`,
  `marshal_spread`, and the slot/request types are unreachable under test (the marshal
  chain needs a Slint event loop); the pure `is_current` predicate and the controller's
  bookkeeping (`reserve_dispatch`, `set_target`, `set_source`) are unit-tested directly.
- **No new dependencies; no public-surface change in core beyond `get_cached`,
  `dispatch_handle`, and `CacheDispatch` / `decode_and_cache`.** `CacheConfig`, the LRU,
  and the `in_flight` prefetch reservation are reused unchanged.
