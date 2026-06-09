# Toolchain & build

Reference doc migrated from the CLAUDE.md "Toolchain & build" section.
All technical details are preserved verbatim from that source of truth.

### Rust pin & mise

Rust is pinned to **1.96.0** via `mise.toml`. Run every cargo command through the pin: `mise exec -- cargo <...>`.

### Fresh install: mise trust

**A fresh `mise install` fails with "Config files are not trusted."** Run `mise trust` once, then `mise install`. CI's `jdx/mise-action` handles trust automatically.

### Linux system libraries (Slint)

Slint links system libraries on **Linux** only: `libfontconfig1-dev libfreetype6-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev`. macOS/Windows need nothing extra **for Slint** — but the dav1d AVIF build dependency below applies on all 3 OS.

### zip dependency: deflate-only, no default features

**`zip` is declared `{ version = "2", default-features = false, features = ["deflate"] }` — never enable its default features.** They pull native C `-sys` libs (bzip2-sys/lzma-sys/zstd-sys) that would add a needless C toolchain burden on every OS (unlike the two justified exceptions below: `unrar` and dav1d). CBZ/ZIP manga pages use only Stored (always available) or Deflate (pure-Rust via flate2/miniz_oxide), so `deflate`-only keeps the cross-platform build clean.

### unrar dependency: C++ toolchain (knowing exception)

**`unrar` is declared `unrar = "0.5"` (always-on, NO feature gate, per Issue #7) and DOES require a C++ compiler on all 3 OS** — it bundles C++ UnRAR built via `cc` (gcc/clang/MSVC, all standard on GitHub runners; macOS Apple clang suffices, no extra apt pkgs beyond the Slint set). This is a knowing exception to the `zip` "no native toolchain" stance: RAR has no pure-Rust decoder, so the C++ compile is unavoidable (build-time cost, cached by `rust-cache`). The non-free RARLAB license clause is recorded in [THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md) (repo root, an acceptance requirement). `base64 = "0.22"` is a core dev-dep (decodes the base64 RAR fixtures). **Intentional deviation from the plan-design doc:** it specified `unrar` (fine) but the implementation uses the SYNCHRONOUS `unrar` over the existing rayon pool — sync `read_bytes` + CPU-bound decode fit rayon naturally; async would force a `block_on` bridge and infect every layer with tokio (same rationale as the sync `zip`).

### dav1d dependency: system C library for AVIF decode (knowing exception)

**`image` is declared `{ version = "0.25", features = ["avif-native"] }` (always-on, NO feature
gate, mirroring the `unrar` stance) and requires `dav1d >= 1.3.0` at BUILD time on all 3 OS** —
resolved by the `dav1d-sys`/`system-deps` build chain (pkg-config, or the `SYSTEM_DEPS_DAV1D_*`
env overrides CI uses). This is the second knowing exception to the `zip` "no native toolchain"
stance; the decoder choice and license rationale live in
[ADR-0010](ADRs/0010-avif-decode-via-dav1d.md) (dav1d is BSD-2-Clause, recorded in
[THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md)).

Dev setup — **macOS**: `brew install dav1d` (verify: `pkg-config --modversion dav1d`).
**Linux**: `sudo apt-get install -y libdav1d-dev` (Ubuntu 24.04 ships 1.4.1). **Windows**:
`vcpkg install dav1d:x64-windows-static-md`, then set the `SYSTEM_DEPS_DAV1D_*` variables as in
`.github/workflows/ci.yml`. Without dav1d the build fails loudly at `dav1d-sys` with a clear
"dav1d not found" probe error — intentional, since the dependency is not feature-gated.

**End users still install nothing**: release builds link dav1d STATICALLY — macOS builds it from
source per arch (meson/ninja/nasm) and lipo-merges one fat `libdav1d.a`; Windows uses the vcpkg
static triplet. Both release jobs assert the result (`otool -L` / `dumpbin /dependents` show no
dav1d dynamic reference). CI test jobs may link dynamically (brew/apt) — nothing ships from CI.

### Thumbnail strip added no new dependencies

**The thumbnail strip added NO new dependencies** — it reuses the existing `image` (`DynamicImage::thumbnail`) and `rayon` (already a direct dep). Contrast the `unrar` C++-toolchain exception above: the thumbnail strip is dependency-free and adds no build cost.

### Cover carousel made `rayon` a direct dep of the `gashuu` UI crate (no new lockfile entry)

**The cover carousel added `rayon` to the `gashuu` UI crate's manifest** for its fire-and-forget cover worker (`cover_loader.rs` `rayon::spawn`). This adds NO new crate to `Cargo.lock` — `rayon` was already in the tree as a direct dep of `gashuu-core` (and transitively via `image`). The nuance: "no new dependencies" means the LOCKFILE (no new third-party code, no build cost), NOT the per-crate manifest — promoting an already-present transitive/sibling crate to a direct dep of another workspace crate is free.

### image 0.25: RGBA → PNG bytes goes through `DynamicImage`

To encode raw RGBA into an in-memory PNG (`thumbnail_cache::put`), wrap the buffer in a `DynamicImage` and encode: `image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, bytes)?).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?`. Calling `write_to` directly on the `RgbaImage` (`ImageBuffer`) does NOT resolve against `image` 0.25 — `write_to` is reached via `DynamicImage`. `RgbaImage::from_raw` returns `Option` (`None` when the buffer is shorter than `w*h*4`), mapped to `CoreError::MalformedImage`. PNG is lossless, so a `put` → `get` round-trip is byte-exact.

### Never launch the GUI in a headless session

`cargo run` opens a GUI window — never launch the app from a non-interactive/headless session (it hangs). Verify with build + clippy + tests instead.

### App icon / bundling

**Icon asset pipeline** — `app-icon.png` (1024×1024 master) and `app-icon.icns` are generated from the source logo using `sips` and `iconutil` (both ship with macOS; no extra install needed):

```sh
# Center-crop 2816×1536 logo to 1536×1536 square, then resize to 1024 master
sips -c 1536 1536 ~/Downloads/gashuu_logo.png --out /tmp/icon-sq.png
sips -z 1024 1024 /tmp/icon-sq.png --out crates/gashuu/ui/assets/app-icon.png

# Build .iconset (all required macOS sizes) and compile to .icns
ICONSET=/tmp/gashuu.iconset; mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s         crates/gashuu/ui/assets/app-icon.png --out "$ICONSET/icon_${s}x${s}.png"
  sips -z $((s*2)) $((s*2)) crates/gashuu/ui/assets/app-icon.png --out "$ICONSET/icon_${s}x${s}@2x.png"
done
iconutil -c icns -o crates/gashuu/ui/assets/app-icon.icns "$ICONSET"
```

**Producing the macOS .app bundle** — install `cargo-bundle` once (compiles under pinned toolchain; binary lands in `~/.cargo/bin`), then build from the crate root:

```sh
mise exec -- cargo install cargo-bundle   # one-time
cd crates/gashuu && mise exec -- cargo bundle --release   # emits target/release/bundle/osx/gashuu.app  ("osx" is cargo-bundle's fixed dir name)
```

`cargo bundle` is NOT wired into the default `cargo build` or CI gates — but the release workflow (below) drives it on the macOS runner.

### Release builds (GitHub Actions)

`.github/workflows/release.yml` builds the distributable executables and attaches them to the GitHub Release for a tag. Trigger: push a `v*` tag, or `workflow_dispatch` with a `tag` input (to re-attach to an existing tag). A `preflight` job asserts the tag matches `crates/gashuu/Cargo.toml` `version` before any build runs, so a mistyped tag fails fast. The GitHub Release must already exist — the workflow only uploads assets to it (`gh release upload --clobber`), it does not create it.

- **macOS (universal)**: builds `aarch64-apple-darwin` + `x86_64-apple-darwin`, `lipo`-merges them into a fat binary, runs `cargo bundle --release` for the `.app` scaffold (cargo-bundle has no `--target universal` support, so the scaffold is built once and the fat binary is spliced into `Contents/MacOS/`), and zips with `ditto -c -k --keepParent` (preserves symlinks/permissions). cargo-bundle is `cargo install`ed on the runner — deliberately NOT added to `mise.toml`, so the CI `app` matrix stays lean. Asset: `gashuu-<tag>-macos-universal.zip`.
- **Windows (x86_64)**: generates `app-icon.ico` from `app-icon.png` with the runner's preinstalled `magick`, builds `--release` (`build.rs` embeds the icon via `winresource`), and zips the `.exe`. Asset: `gashuu-<tag>-windows-x64.zip`.
- **Signing**: macOS `.app` is ad-hoc (self-signed) in CI; Developer ID signing + notarization are deferred. Windows `signtool` insertion point remains marked as a `SIGNING SEAM` comment in `release.yml`.

**Windows `.ico` embedding** is now wired (was deferred): `winresource` is a `[target.'cfg(windows)'.build-dependencies]` so it is never fetched on macOS/Linux; `build.rs` gates the embed on `cfg(windows)` AND `CARGO_CFG_TARGET_OS == "windows"` AND the `.ico` existing, so a dev `cargo build` without the (CI-generated, uncommitted) `.ico` is a no-op and never a build blocker. **Still deferred**: Linux release artifacts and a `.desktop` entry — Slint's Linux system-library deps make Linux distribution heavier (see "Linux system libraries" above).
