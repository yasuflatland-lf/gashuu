# ADR-0014: Web decodes pages with the browser-native decoder

- Status: Accepted
- Decided: 2026-11-22
- Related: [ADR-0002](0002-layered-two-crate-architecture.md) (the core and UI boundary),
  [ADR-0003](0003-image-loading-and-caching.md) (page caching and prefetch),
  [ADR-0012](0012-async-page-decode.md) (asynchronous cache-miss decoding)

## Context

On an iPad mini 6 in Safari, wasm `image` decoding took about 2.9 seconds per
1600 × 2400 page and a cache-miss turn took about 3.0 seconds. Desktop Chrome and
Safari decoded the same size pages in about 40 ms. A Worker alone would keep the
UI responsive but would not remove the decode cost.

Addendum 2 to #651 measured the median of five runs on a 1600 × 2400 JPEG:

| Bench | iPad mini 6 | Chrome (Mac) |
| --- | --- | --- |
| wasm `image` decode | 2,881 ms | 104 ms |
| `createImageBitmap` + canvas + `getImageData`, main thread | 60 ms | 56 ms |
| Same operation in a Worker | 56 ms | 40 ms |

The native browser path makes the iPad demo practical without waiting for the
Worker implementation. The benchmark is not yet a measurement of a complete
cache-miss turn in the application.

## Decision

Put the `PageDecoder` port in the `gashuu` crate, at the encoded bytes to
straight-alpha RGBA8 pixels and dimensions boundary. It returns local `!Send`
futures. The change adds no crates and does not add `wasm-bindgen` to
`gashuu-core`.

The wasm32 composition root supplies `NativePageDecoder` as the primary adapter
and `ImageCratePageDecoder` as the fallback. If browser decoding fails, retry
with the `image` crate. Covers use the same pair to decode page zero, then
`image_ops::downscale_to_thumbnail` to produce thumbnails. Web cover requests
run sequentially, with one cover decode in flight, including across refreshes.
The web thumbnail strip remains a no-op.

The native adapter uses `createImageBitmap` and canvas readback. It requests
`premultiplyAlpha: none` for the straight-alpha RGBA contract. Canvas storage
is premultiplied internally, so readback can lose RGB precision through rounding
for partially transparent pixels. It requests `imageOrientation: none` to ignore
EXIF orientation and match the `image` crate's original pixel orientation.
`web-sys` 0.3.107 lacks the `none` enum variant, so the adapter sets this option
with `Reflect`. It also requests no color-space conversion.

The adapter now supports `Window` and `WorkerGlobalScope` (#697). The application
will use the adapter in a Worker with #658 (ADR-0015). Desktop decoding remains on
the `image` crate. Cache capacity and prefetch behavior remain unchanged.

**Amendment ([43/43]), 2026-11-24.** The Worker uses the browser decoder adapter (#658). The web viewer caches up to six pages with a 128 MiB budget and disables prefetch (ADR-0015).

## Consequences

- Browser-native decoding substantially reduces the measured iPad decode time;
  the image-crate fallback preserves a second path when native decoding fails.
- Canvas readback and full-size cover decoding have memory and processing costs,
  and main-thread work can still affect responsiveness. Sequential cover loading
  limits cover decode concurrency; it does not serialize covers with page loads.
- The port keeps browser APIs out of the core and gives the later Worker work an
  existing boundary, at the cost of maintaining two adapters and fallback logic.

## Verification plan

After merge, the maintainer will verify cache-miss page turns on the iPad, targeting
well under 150 ms per turn. Record the results on #651, then close that issue.

For application integration with the Worker, see #658 and ADR-0015.

## Implementation notes (as-built deltas)

The web thumbnail strip is enabled through `BookBackend::thumbnail`.
See [ADR-0015, Decision item 8](0015-web-build-architecture.md#decision) and #659.
