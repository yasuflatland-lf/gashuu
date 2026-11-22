# ADR-0015: Web build architecture

- Status: Accepted
- Decided: 2026-11-22
- Related: [ADR-0014](0014-web-decodes-pages-with-the-browser-native-decoder.md)

## Context

The web build supports two uses. The install-free demo lets readers try gashuu
with a picked book. Daily reading requires keeping a library of about 50 books
on an iPad running Safari, with individual CBZ files typically 50–300 MB.
Reading those books needs bounded memory and responsive page turns.

## Decision

1. Build Slint web with Trunk and deploy to GitHub Pages. Slint uses winit and
   FemtoVG to render to a `<canvas>`.
2. Use a single-threaded wasm build. It requires neither wasm-bindgen-rayon nor
   COOP/COEP headers.
3. Copy books into OPFS. A dedicated Web Worker reads each page through
   positional `FileSystemSyncAccessHandle` reads (`ReadAt`, #646 [5/23]).
   The synchronous handle is available only in a Worker and matches the
   synchronous `ReadAt` interface. This keeps page I/O and decoding off the
   UI thread.
4. Decode pages in that Worker with `createImageBitmap`, `OffscreenCanvas`
   and `getImageData`, following
   [ADR-0014](0014-web-decodes-pages-with-the-browser-native-decoder.md).
   The same browser-native operation took about 56 ms in a Worker on an
   iPad mini 6. The wasm `image` crate remains the `PageDecoder` fallback
   for PNG and JPEG. A native decode failure for WebP or GIF marks that page
   as failed.
5. Access books through the async `BookBackend` port using `BookId` for
   library entries and `SessionId` for opens. Each `Done` runs at most once
   on the Slint event-loop thread and always after its request returns. The
   backend checks immediately before delivery and drops callbacks for closed
   sessions or cancelled cover requests. The interim `LocalWebBackend`
   preserves the in-memory demo during controller migration. Once #660
   removes the session registry, an unavailable Worker or OPFS uses
   `UnavailableBackend` and shows “cannot save in this browser”.
6. Exclude RAR/CBR and AVIF on web. Their unrar and dav1d C/C++ integrations
   cannot target `wasm32-unknown-unknown`.
7. Persist settings and library metadata through `BlobStore` backed by
   localStorage (#647 [6/23], #654 [13/23]).
8. Use a null `ThumbStore` on web until #661 [10/13]. Enable the strip as
   background-lane work in #659 [8/13] and verify page turns on the iPad.
9. Request `navigator.storage.persist()` and preflight imports with
   `navigator.storage.estimate()` (#660 [9/13], #662 [11/13]). Safari may
   evict script-written storage, including OPFS, for sites unused for a
   period unless the user adds the site to the home screen.

## Evidence

- The initial Chrome run reported “Status: PARTIAL.” It measured a cache-miss
  turn of about 45 ms on Chrome and left the iPad criteria open.
  [Chrome run](https://github.com/yasuflatland-lf/gashuu/issues/651#issuecomment-7057814231)
- Addendum 2 reported A1 and A2 as “triggered for the wasm decode path” and
  measured the browser-native operation at about 60 ms on the iPad main thread
  and 56 ms in a Worker. The wasm decoder took about 2.9 seconds per page.
  [Addendum 2](https://github.com/yasuflatland-lf/gashuu/issues/651#issuecomment-7071384979)
- The shipped demo resolved A1 and A2 with native decoding. A3 recorded
  “no crash at 348 MB” on the iPad with no peak-memory reading. The final
  report rated “B (Slint web viability): GO” including touch on iPad Safari,
  and “C (size): OK” at about 2.6 MB gzip.
  [Final verdicts](https://github.com/yasuflatland-lf/gashuu/issues/651#issuecomment-7072292390)

## Consequences

- Desktop reads retain their behavior. Once the controllers use the port, their
  rayon dispatch lives in `backend/desktop.rs`. Update checks and startup
  cover pruning retain their separate desktop jobs.
- The web `ReadAt` adapter's `unsafe impl Send + Sync` is valid only without
  the `atomics` target feature.
- The web decoded-page cache holds at most six pages, about 15 MB each for
  1600 × 2400 RGBA. The Worker retains decoded pages only for the request in flight.
  Web page prefetch stays disabled pending a separate measured change.

## Alternatives considered

- Whole-book in-memory storage scales poorly with book size and library size.
- Synchronous reads on the main thread block UI events. Asynchronous OPFS reads
  on the main thread cannot implement synchronous `ReadAt`.
- SharedArrayBuffer and wasm threads require cross-origin isolation headers that
  the GitHub Pages deployment does not supply.
- Moving wasm `image` decoding to the Worker would still cost about 2.9 seconds
  per page on the iPad.
