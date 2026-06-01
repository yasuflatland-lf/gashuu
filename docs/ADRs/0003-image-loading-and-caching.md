# ADR-0003: Load images lazily with ±3 prefetch and an LRU cache

- Status: Accepted
- Decided: 2026-05-31 (transcribed: 2026-06-01)
- Related: [ADR-0001](0001-gui-framework-slint.md) (native memory control), [ADR-0004](0004-archive-abstraction-and-extraction.md) (`PageSource`)

## Context

The viewer must render 4K-class pages while meeting hard performance and memory targets:

| Metric | Target |
| --- | --- |
| First display of a 4K page | < 200 ms |
| Page turn (cache hit) | < 50 ms |
| Zoom / pan | sustained 60fps |
| Memory after browsing 50 pages | < 500 MB |

A naive "decode every page up front" or "hold every decoded page forever" approach would exhaust
memory on a large archive (a 1 GB CBZ / 500 MB CBR is in scope). The native memory control enabled
by Slint ([ADR-0001](0001-gui-framework-slint.md)) is what makes a bounded strategy feasible.

## Decision

Use **lazy decode-on-demand plus background prefetch, fronted by a bounded LRU cache**:

- `ImageCache` holds decoded images in an LRU keyed by page index, capacity
  `DEFAULT_CAPACITY = 50` (via the `lru` crate). This caps resident decoded memory regardless of
  archive size.
- On each navigation, prefetch the current page ± `DEFAULT_PREFETCH_RADIUS = 3` in the background on
  the existing `rayon` pool (fire-and-forget). Cache hits stay instant — they clone an
  `Arc<DecodedImage>` and never block on prefetch.
- Decode is bomb-guarded (defense in depth): an early `check_pixel_limit` (`MAX_PIXELS`, rejects via
  `CoreError::ImageTooLarge` with no allocation) ahead of an `image::Limits`-bounded full decode.

## Alternatives considered

- **Decode all pages up front** — simplest navigation, but unbounded memory on large archives.
  Rejected.
- **Hold all decoded pages (no eviction)** — fast re-visits, but the same unbounded-memory problem.
  Rejected in favor of an LRU ceiling.

## Consequences

### Positive
- Resident decoded memory has a hard ceiling (≈ 50 pages + a few in-flight prefetch decodes),
  meeting the < 500 MB target independent of archive size.
- Cache hits return `Arc<DecodedImage>`, so a page turn never copies the multi-MB RGBA buffer.
- Prefetch parallelism comes for free from `rayon` (already a transitive dep via `image`).

### Costs / trade-offs accepted
- Background prefetch over `Arc<Mutex<LruCache>>` introduces concurrency: a fixed lock order
  (`cache` → `in_flight`), an `InFlightGuard` RAII cleanup, and fire-and-forget error handling are
  required (documented in `cache.rs`). The rayon background paths are coverage-exempt (cannot be
  exercised deterministically); the synchronous core (`prefetch_indices`, `prefetch_blocking`)
  carries the coverage.
- Cache/prefetch timing (the < 50 ms target) is observed via `tracing` telemetry, never asserted in
  tests (no wall-clock assertions — they would be flaky).

## Implementation notes (as-built deltas)

- The values match the design doc: `lru = "0.18"`, `DEFAULT_CAPACITY = 50`,
  `DEFAULT_PREFETCH_RADIUS = 3`.
- The thumbnail strip (PR8a) is the deliberate **inverse** of this policy: it holds **all N pages**
  (non-LRU) but at thumbnail resolution, generated once on `rayon` and streamed to the UI. Peak RAM
  there ≈ rayon-pool-size full-res pages — the same bound as prefetch.
