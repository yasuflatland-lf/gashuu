# ADR-0010: Decode AVIF pages via the dav1d native decoder

- Status: Accepted
- Decided: 2026-06-06
- Related: [ADR-0001](0001-gui-framework-slint.md) (origin of the "macOS/Windows need nothing
  extra" build policy this adds a second known exception to),
  [ADR-0002](0002-layered-two-crate-architecture.md) (core stays headless; the
  decode boundary is RGBA bytes + dimensions), [ADR-0003](0003-image-loading-and-caching.md)
  (the decode pipeline this plugs into), [ADR-0004](0004-archive-abstraction-and-extraction.md)
  (`PageSource` / the `IMAGE_EXTS` admission gate), [ADR-0009](0009-reject-empty-books.md)
  (`probe_page_count` counts only admitted extensions)

## Context

AVIF (AV1 Image File Format) is increasingly common in digitally distributed manga. Two
independent gates kept it out:

1. **Admission**: `IMAGE_EXTS` in `page_source/naming.rs` — the single extension whitelist
   consumed by `FolderSource`, `ZipSource`, `RarSource`, and `probe_page_count` — listed only
   `png`/`jpg`/`jpeg`. An `.avif` file was invisible to every source, so an all-AVIF folder or
   archive probed as `EmptyBook`.
2. **Decode**: `image_ops::decode_dynamic` dispatches on magic bytes via
   `image::ImageReader::with_guessed_format()`, so it needs no per-format code — but the `image`
   crate's default `avif` feature is ENCODE-only (ravif/rav1e, pure Rust). Decoding requires the
   `avif-native` feature, which binds the dav1d C library (`dav1d-sys` via `system-deps`,
   `libdav1d >= 1.3.0`). As of 2026-06 there is no production pure-Rust AVIF decoder under a
   license compatible with gashuu's MIT distribution (the rav1d-safe port is AGPL-3.0/commercial;
   plain rav1d exposes only a C API).

The decision question: accept a second native C dependency (after `unrar`), and if so, how to
keep the "end users install nothing" distribution promise on macOS (universal) and Windows.

## Decision

1. **Enable `avif-native` always-on** (`image = { version = "0.25", features = ["avif-native"] }`),
   no cargo feature gate — the `unrar` precedent (Issue #7). A "build without AVIF" variant would
   silently skip pages and double the test matrix for no practical use case.
2. **Add `"avif"` to `IMAGE_EXTS`** — the one-line admission change that propagates to all three
   sources and the emptiness probe. The magic-byte decode funnel, the PNG-only thumbnail cache,
   and the RGBA8 core↔UI boundary all apply to AVIF unchanged; the bomb guard needed one
   format branch (see Consequences and the as-built notes).
3. **Link dav1d statically in release builds** so shipped artifacts stay self-contained:
   macOS builds dav1d from source per arch (meson/ninja/nasm) and lipo-merges one fat
   `libdav1d.a`; Windows uses vcpkg's `x64-windows-static-md` triplet. Both release jobs ASSERT
   the absence of a dynamic dav1d reference (`otool -L` / `dumpbin /dependents`). CI test jobs
   link dynamically (apt/brew) — nothing ships from CI.
4. **Keep the license MIT.** dav1d is BSD-2-Clause (recorded in `THIRD_PARTY_LICENSES.md`);
   no relicensing is needed.
5. **Simplest behavior for special AVIFs** (user decision): 10/12-bit sources are converted to
   8-bit RGBA by the existing `to_rgba8()` funnel (the UI boundary is RGBA8), animated AVIFs
   render their first frame, and alpha is preserved as-is. No format-specific features.

## Alternatives considered

- **rav1d-safe + zenavif-parse (pure Rust).** Zero build-toolchain cost and a trivially
  self-contained binary, but AGPL-3.0/commercial dual licensing would force the distributed app
  under AGPL terms — rejected to keep MIT distribution (explicitly weighed with the user).
- **avif-decode / aom-decode (libaom static via cmake).** License-compatible (BSD-2) and
  end-user-clean, but it adds a parallel decode path OUTSIDE the `image` crate (container parse +
  YUV→RGB conversion in our code) — a real color-correctness risk that `avif-native` delegates to
  dav1d + `image` upstream. Rejected in favor of the zero-code-change integration.
- **`image`'s default `avif` feature alone.** Encode-only; decode fails. Not an option, recorded
  here because the feature name invites the mistake.
- **Defer AVIF.** Rejected by the user; AVIF books exist in their library today.

## Consequences

### Positive

- AVIF pages in folders and CBZ/CBR archives decode transparently next to PNG/JPEG; the
  production diff is the manifest feature, the whitelist entry, and one guard branch.
- PNG/JPEG keep their pre-allocation bomb guard unchanged; AVIF is pixel-capped once,
  post-decode, so an oversized AVIF is still rejected before RGBA conversion and the UI boundary.
- Test fixtures stay in-process (the ravif encoder already in the default feature set
  synthesizes tiny AVIFs), preserving the no-committed-binary-fixtures convention.

### Costs / trade-offs accepted

- Building gashuu now requires dav1d on every platform (brew/apt/vcpkg) — the second knowing
  exception in [docs/toolchain.md](../toolchain.md); a missing library fails the build loudly.
- The macOS release job gains a meson/ninja/nasm source build (~5-6 min) including an
  arm64→x86_64 cross-compile pinned by a meson cross-file; the Windows jobs gain a vcpkg build
  (~5-8 min cold, seconds when the archives cache hits).
- dav1d decode output flows through `DynamicImage::to_rgba8()`: 10/12-bit AVIFs lose precision
  by design (the UI boundary is RGBA8).
- **Residual risk accepted: AVIF has no pre-allocation dimension guard.** `image`'s
  `AvifDecoder::new` performs the full dav1d decode in its constructor, so dimensions cannot be
  read cheaply through the `image` API and `image::Limits` is not enforced by that decoder. The
  transient decode allocation for a malicious AVIF is bounded only by dav1d/AV1 spec limits; the
  post-decode `check_pixel_limit` then rejects it. A true pre-allocation guard (hand-parsing the
  container's `ispe` box) was considered and deferred.
- The release source build clones dav1d from the VideoLAN GitHub mirror at a pinned tag; if the
  mirror is unreachable the release job fails (vendoring the tarball is deferred hardening).

## Implementation notes (as-built deltas)

- **AVIF skips the dimension pre-read (review finding).** The plan assumed `into_dimensions()`
  was a cheap header read for every format; for AVIF it constructs `AvifDecoder`, which fully
  decodes — making the pre-read both a non-guard and a SECOND full decode per page. As built,
  `decode_dynamic` branches on the guessed format: AVIF decodes once and runs
  `check_pixel_limit` post-decode; every other format keeps the pre-allocation pre-read
  (and its forged-IHDR pin test) unchanged.
- Otherwise no divergence: the production diff is exactly Decision items 1 and 2 plus the
  guard branch above.
