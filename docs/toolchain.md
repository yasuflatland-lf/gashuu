# Toolchain & build

Reference doc migrated from the CLAUDE.md "Toolchain & build" section.
All technical details are preserved verbatim from that source of truth.

### Rust pin & mise

Rust is pinned to **1.96.0** via `mise.toml`. Run every cargo command through the pin: `mise exec -- cargo <...>`.

### Fresh install: mise trust

**A fresh `mise install` fails with "Config files are not trusted."** Run `mise trust` once, then `mise install`. CI's `jdx/mise-action` handles trust automatically.

### Linux system libraries (Slint)

Slint links system libraries on **Linux** only: `libfontconfig1-dev libfreetype6-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev`. macOS/Windows need nothing extra.

### zip dependency: deflate-only, no default features

**`zip` is declared `{ version = "2", default-features = false, features = ["deflate"] }` — never enable its default features.** They pull native C `-sys` libs (bzip2-sys/lzma-sys/zstd-sys) that would require a C toolchain on every OS and BREAK the "macOS/Windows need nothing extra" promise above. CBZ/ZIP manga pages use only Stored (always available) or Deflate (pure-Rust via flate2/miniz_oxide), so `deflate`-only keeps the cross-platform build clean.

### unrar dependency: C++ toolchain (knowing exception)

**`unrar` is declared `unrar = "0.5"` (always-on, NO feature gate, per Issue #7) and DOES require a C++ compiler on all 3 OS** — it bundles C++ UnRAR built via `cc` (gcc/clang/MSVC, all standard on GitHub runners; macOS Apple clang suffices, no extra apt pkgs beyond the Slint set). This is a knowing exception to the `zip` "no native toolchain" stance: RAR has no pure-Rust decoder, so the C++ compile is unavoidable (build-time cost, cached by `rust-cache`). The non-free RARLAB license clause is recorded in [THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md) (repo root, an acceptance requirement). `base64 = "0.22"` is a core dev-dep (decodes the base64 RAR fixtures). **Intentional deviation from the plan-design doc:** it specified `unrar` (fine) but PR7 uses the SYNCHRONOUS `unrar` over the existing rayon pool — sync `read_bytes` + CPU-bound decode fit rayon naturally; async would force a `block_on` bridge and infect every layer with tokio (same rationale as PR6's sync `zip`).

### PR8a added no new dependencies

**PR8a (thumbnail strip) added NO new dependencies** — it reuses the existing `image` (`DynamicImage::thumbnail`) and `rayon` (already a direct dep). Contrast PR7's `unrar` C++-toolchain exception above: PR8a is dependency-free and adds no build cost.

### PR-V made `rayon` a direct dep of the `gashuu` UI crate (no new lockfile entry)

**PR-V (cover carousel) added `rayon` to the `gashuu` UI crate's manifest** for its fire-and-forget cover worker (`cover_loader.rs` `rayon::spawn`). This adds NO new crate to `Cargo.lock` — `rayon` was already in the tree as a direct dep of `gashuu-core` (and transitively via `image`). The nuance: "no new dependencies" means the LOCKFILE (no new third-party code, no build cost), NOT the per-crate manifest — promoting an already-present transitive/sibling crate to a direct dep of another workspace crate is free.

### image 0.25: RGBA → PNG bytes goes through `DynamicImage`

To encode raw RGBA into an in-memory PNG (`thumbnail_cache::put`), wrap the buffer in a `DynamicImage` and encode: `image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, bytes)?).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?`. Calling `write_to` directly on the `RgbaImage` (`ImageBuffer`) does NOT resolve against `image` 0.25 — `write_to` is reached via `DynamicImage`. `RgbaImage::from_raw` returns `Option` (`None` when the buffer is shorter than `w*h*4`), mapped to `CoreError::MalformedImage`. PNG is lossless, so a `put` → `get` round-trip is byte-exact.

### Never launch the GUI in a headless session

`cargo run` opens a GUI window — never launch the app from a non-interactive/headless session (it hangs). Verify with build + clippy + tests instead.
