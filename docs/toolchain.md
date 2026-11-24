# Toolchain & build

Reference doc migrated from the CLAUDE.md "Toolchain & build" section.
All technical details are preserved verbatim from that source of truth.

### Rust pin & mise

Rust is pinned to **1.97.1** via `mise.toml`. Run every cargo command through the pin: `mise exec -- cargo <...>`.

### wasm32 target (browser build)

The Rust pin in `mise.toml` includes `wasm32-unknown-unknown`. Run `mise run check-wasm-core`
to check the headless crate for that target. The task passes `--no-default-features` to omit
the native UnRAR and dav1d decoders, and uses `--target-dir target/wasm` so host checks and
tests can use their own build directories concurrently. The five local gates remain independent
of this task. The blocking `wasm (wasm32 check)` CI job explicitly installs the target with
`rustup target add wasm32-unknown-unknown` before running the task, including on mise cache hits.

On this target, core enables the `wasm_js` feature of getrandom 0.4, which is also used by
`zip`. The feature selects the browser backend by itself and requires no additional cfg flag.

#### UI crate on wasm32

`mise run check-wasm-ui` compile-checks the UI crate in `target/wasm`. The Worker depends on
`gashuu-web-decode`, which implements browser-native decoding for windows and workers
and uses `gashuu-core` with default features disabled.
`mise run check-wasm-decode` checks the adapter independently in the same target directory.
The CI `wasm` job runs all four wasm checks (`check-wasm-core`, `check-wasm-ui`,
`check-wasm-decode`, `check-wasm-worker`) with `RUSTFLAGS=-D warnings`, then builds
the browser bundle and checks for `index.html` and a `.wasm` file in `crates/gashuu/dist`.
A wasm-only warning fails CI.

The UI crate declares these dependencies in
`[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`.

| Dependency | Desktop use |
| --- | --- |
| `ureq` | Fetches release metadata and downloads self-update assets. Its rustls backend brings in `ring` and a `getrandom` version without a configured wasm backend. |
| `self-replace` | Replaces the running Windows portable executable during self-update. |
| `opener` | Opens release pages and reveals downloaded installers in the OS file manager. |
| `rfd` | Opens native file and folder dialogs for the Add buttons. |
| `color-eyre` | Installs the error reporter and provides the return type for desktop `main`. |
| `tracing-subscriber` | Initializes desktop logging and its environment filter. |
| `directories` | Resolves the Downloads folder through `UserDirs` for guided updates. |
| `zip` | Extracts the Windows executable from the portable release archive. |

Both targets use the same Slint version. The desktop table adds the winit API
used by drag-and-drop and window geometry.

```toml
[dependencies]
slint = { version = "=1.19.0" }

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
slint = { version = "=1.19.0", features = ["unstable-winit-030"] }
```

The desktop table uses the default `gashuu-core` features for RAR and AVIF.
The `[target.'cfg(target_arch = "wasm32")'.dependencies]` table sets
`default-features = false` on `gashuu-core`. Cargo resolver 2 keeps the
desktop defaults out of the wasm build.

### Web decoding

The wasm32 composition root uses `NativePageDecoder` as the primary decoder and
`ImageCratePageDecoder` as the fallback. The adapter in `gashuu-web-decode` uses
`createImageBitmap` and canvas readback in either a window or a worker. It checks dimensions
against the core pixel limit before allocating the canvas. It requests straight
alpha, no color-space conversion, and the original pixel orientation. Canvas
readback can round partially transparent RGB values. The Worker uses the same decoders
for covers and strip thumbnails, then downscales the decoded images in its background
lane. No new crates or browser bindings in `gashuu-core` are required. See
[ADR-0014](ADRs/0014-web-decodes-pages-with-the-browser-native-decoder.md).

### Web build (Trunk)

`mise.web.toml` pins Trunk 0.22.2 and is active with `MISE_ENV=web`. Run these
commands from the repository root to install the tools, build the release bundle
into `crates/gashuu/dist`, or serve the app at `http://127.0.0.1:8080/`.

```sh
MISE_ENV=web mise install
MISE_ENV=web mise run build-web
MISE_ENV=web mise run serve-web
```

Trunk builds the `gashuu` binary for `wasm32-unknown-unknown` using
`data-bin="gashuu"` in `crates/gashuu/index.html`. wasm-bindgen invokes the
binary's `main` at startup. Trunk fetches the matching wasm-bindgen CLI and
wasm-opt. Slint's winit backend attaches to the element with ID `canvas`, and
the page sizes `#canvas` to the viewport with CSS.
The page and canvas set `touch-action: none` and a non-scalable viewport so iPad
Safari does not consume double-taps or pinch gestures for page zoom and touch reaches the app.
The Slint window follows the browser viewport on load and resize.

The worker link in `crates/gashuu/index.html` points Trunk at
`../gashuu-worker/Cargo.toml` and selects the `gashuu-worker` binary.
Trunk emits `gashuu-worker.js`, `gashuu-worker_bg.wasm`, and
`gashuu-worker_loader.js` in `crates/gashuu/dist`. The loader uses
`importScripts` to load the worker glue and initializes the wasm binary.
Run `mise run check-wasm-worker` to check the worker crate on its own.
The OPFS reader and importer use synchronous `getSize()` and `close()` methods, available
as synchronous calls starting in Safari and iPadOS 16.4.
[WebKit release notes](https://webkit.org/blog/13966/webkit-features-in-safari-16-4/).

The Pages workflow publishes the Vite landing page at `/gashuu/` and the web
app at `/gashuu/app/` in one deployment. Its Trunk build uses
`--public-url /gashuu/app/` and the deploy job adds the bundle to `site/dist/app`.

The web app starts on the Library screen. Add Books opens a browser picker for
one or more CBZ or ZIP files. Folders, RAR/CBR, 7z/CB7, and PDF are unsupported.
On iOS and iPadOS the picker has no accept filter because the system has no registered file type for .cbz and greys such files out; unsupported formats are rejected after selection.
On coarse-pointer browsers, a single tap on the centered cover opens the book, while mouse users can double-click or press Return.
The Worker imports selected archives into OPFS and decodes pages on demand.
The viewer caches up to six decoded pages, with prefetch disabled. Thumbnail
requests run in the worker's background lane.

Action-time saves to `localStorage` preserve settings and library metadata
across reloads. Imported archives remain in OPFS.
If `localStorage` is unavailable at startup, the app opens with defaults and an empty library, shows
`storage (localStorage unavailable)`, and reports `NoConfigDir` or `NoDataDir`
on save. The web lifecycle flushes the reading position, pending page counts,
and settings when the page becomes hidden or receives a `pagehide` event.

Trunk copies the Japanese fallback font and its OFL license to `dist/fonts/`.
The browser preloads the TTF while the wasm downloads. After startup, gashuu
fetches it from a document-relative URL and registers it with Slint for
Hiragana, Katakana, and Han. Slint reshapes existing text after registration
and continues to use Inter for Latin glyphs. A load failure logs a warning and
the app continues running. The TTF budget is 1.5 MiB raw, and the wasm growth
budget is 30 KiB gzip compared with `main`. Regenerate the asset with
`bash scripts/subset-web-font.sh`. Japanese glyphs use weight 400, including
in UI elements that request heavier weights. Book titles can still show
missing glyphs for characters outside the subset. The catalog's U+2328
keyboard symbol is absent from both web fonts.

### Web font (Noto Sans JP subset)

- Upstream file `ofl/notosansjp/NotoSansJP[wght].ttf` at google/fonts commit
  `66a36c8c94b1a5d992ee4e7f392fccfe4945767c`. [Upstream font directory](https://github.com/google/fonts/tree/66a36c8c94b1a5d992ee4e7f392fccfe4945767c/ofl/notosansjp)
- Upstream TTF SHA-256 `c2f3b4d463500a2ddcd3849cded1fceeb9fd6d1c32e6cbecd568453ba50fc68f`.
- `fonttools` 4.62.1 produces a static weight-400 TTF. The input codepoint list
  is stored alongside the font in `subset-codepoints.txt`.
- The upstream OFL copyright line is `Copyright 2014-2021 Adobe (http://www.adobe.com/), with Reserved Font Name 'Source'`.
  The subset retains the `Noto Sans JP` family name. [Upstream OFL](https://github.com/google/fonts/blob/66a36c8c94b1a5d992ee4e7f392fccfe4945767c/ofl/notosansjp/OFL.txt)

### Fresh install: mise trust

**A fresh `mise install` fails with "Config files are not trusted."** Run `mise trust` once, then `mise install`. CI's `jdx/mise-action` handles trust automatically.

### Dependency debuginfo trimmed to line tables (root-manifest `[profile.dev]`)

The workspace root `Cargo.toml` sets `[profile.dev] debug = "line-tables-only"` and puts the two members back to `debug = 2` via `[profile.dev.package.gashuu-core]` / `[profile.dev.package.gashuu]`, so the trim is **dependencies-only**: stepping through and backtracing gashuu's own code is unchanged. `line-tables-only` rather than `0` because dependency frames then still carry `file:line` in panic backtraces — that is what makes an `image` / `zip` / `slint` panic inside a test diagnosable. Measured with a fresh `CARGO_TARGET_DIR` on a cold `cargo nextest run --workspace --profile ci --no-run`: target output 3.52 GiB → 3.03 GiB (-13.7%), rustc CPU 290 s → 272 s, incremental rebuild after a `gashuu-core` edit 5.1 s → 3.7 s. `[profile.test]` inherits all of this, so there is no `test` twin. **Do not "simplify" this to `[profile.dev.package."*"]`** — a package override counts as an *explicit* debuginfo setting, so cargo stops suppressing debuginfo for the build graph and the 211 build-script / proc-macro units (`i-slint-compiler` and friends) gain line tables, which cuts the saving from -493 MiB to -118 MiB; `[profile.dev.build-override]` cannot cancel that, because package overrides are applied after it.

### Linux system libraries (Slint)

Slint links system libraries on **Linux** only: `libfontconfig1-dev libfreetype6-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev`. macOS/Windows need nothing extra **for Slint** — but the dav1d AVIF build dependency below applies on all 3 OS.

### zip dependency: deflate-only, no default features

**`zip` is declared `{ version = "2", default-features = false, features = ["deflate"] }` — never enable its default features.** They pull native C `-sys` libs (bzip2-sys/lzma-sys/zstd-sys) that would add a needless C toolchain burden on every OS (unlike the two justified exceptions below: `unrar` and dav1d). CBZ/ZIP manga pages use only Stored (always available) or Deflate (pure-Rust via flate2/miniz_oxide), so `deflate`-only keeps the cross-platform build clean.

### unrar dependency: C++ toolchain (knowing exception)

**The default-on `rar` feature of `gashuu-core` enables the optional `unrar` 0.5 dependency and requires a C++ compiler on all three desktop platforms** — it bundles C++ UnRAR built via `cc` (gcc/clang/MSVC, all standard on GitHub runners; macOS Apple clang suffices, no extra apt pkgs beyond the Slint set). This is a knowing exception to the `zip` "no native toolchain" stance: RAR has no pure-Rust decoder, so the C++ compile is unavoidable (build-time cost, cached by `rust-cache`). The non-free RARLAB license clause is recorded in [THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md) (repo root, an acceptance requirement). `base64 = "0.22"` is a core dev-dep (decodes the base64 RAR fixtures). **Intentional deviation from the plan-design doc:** it specified `unrar` (fine) but the implementation uses the SYNCHRONOUS `unrar` over the existing rayon pool — sync `read_bytes` + CPU-bound decode fit rayon naturally; async would force a `block_on` bridge and infect every layer with tokio (same rationale as the sync `zip`).

### dav1d dependency: system C library for AVIF decode (knowing exception)

**`image` is declared `{ version = "0.25", default-features = false, features = ["png", "jpeg",
"qoi"] }`** — an explicit DECODE-oriented allowlist (defaults OFF, same stance as
`zip`). gashuu only ever DECODES pages (`png`/`jpeg`/`avif`) and ENCODES only to `png`
(cache/thumbnails) and `qoi` (thumbnail cache), so every other default format — and, crucially,
the `avif` **encode** feature that pulls the whole `ravif` -> `rav1e` AV1 encoder — is excluded,
keeping `rav1e` out of the shipped binary. **The default-on core `avif` feature enables `image/avif-native` for decoding and
requires `dav1d >= 1.3.0` at BUILD time on all 3 OS** — resolved by the `dav1d-sys`/`system-deps`
build chain (pkg-config, or the `SYSTEM_DEPS_DAV1D_*` env overrides CI uses). This is the second
knowing exception to the `zip` "no native toolchain" stance; the decoder choice and license
rationale live in [ADR-0010](ADRs/0010-avif-decode-via-dav1d.md) (dav1d is BSD-2-Clause, recorded
in [THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md)). **Note:** `rav1e` (and its `paste` /
RUSTSEC-2024-0436 advisory) no longer appears in `Cargo.lock` as of Slint 1.18, whose build-time
`.slint` compiler (`i-slint-compiler`) no longer enables `image`'s DEFAULT features (which include
the avif encoder), so the `deny.toml` ignore for it was removed. Earlier Slint releases pulled it
in regardless of gashuu's own feature trimming.

Dev setup — **macOS**: `brew install dav1d` (verify: `pkg-config --modversion dav1d`).
**Linux**: `sudo apt-get install -y libdav1d-dev` (Ubuntu 24.04 ships 1.4.1). **Windows**:
`vcpkg install dav1d:x64-windows-static-md` (Visual Studio 2026 / MSVC 14.52+ needs dav1d
>= 1.5.5 — older releases fail in `compat/msvc/stdatomic.h`), then set the `SYSTEM_DEPS_DAV1D_*`
variables as `.github/actions/build-dav1d-windows/action.yml` does. With `avif` enabled, a missing dav1d installation causes a
"dav1d not found" probe error from `dav1d-sys`.

**End users still install nothing**: release builds link dav1d STATICALLY, every OS from source
at ONE pinned tag — the workflow-level `DAV1D_VERSION` in `release.yml` (mirrored in `ci.yml`;
`scripts/check-dav1d-pin.sh` asserts they match and Renovate bumps both, never automerged).
macOS builds per arch (meson/ninja/nasm) and lipo-merges one fat `libdav1d.a`; Linux builds on
ubuntu-22.04; Windows builds with meson + MSVC through the composite action
`.github/actions/build-dav1d-windows`, which `ci.yml` also runs, so CI compiles the exact Windows
library the release links. Each release job asserts the result (`otool -L` / `ldd` /
`dumpbin /dependents` show no dav1d dynamic reference). CI's macOS/Linux legs link brew/apt
dav1d dynamically — nothing ships from CI.

**Why Windows no longer uses vcpkg** (v1.3.0 incident): the runner image's preinstalled vcpkg
decided both the dav1d version and, via the image's compiler, whether it built. When
`windows-latest` moved to VS 2026 (MSVC 14.52), dav1d <= 1.5.4 stopped compiling, yet CI stayed
green by restoring a frozen 1.5.3 install tree from cache — the tag build was the first cold
build. The action's cache key now includes `ImageOS` + `ImageVersion` (no `restore-keys`), so an
image update forces a cold build in CI first, and the AVIF decode tests run on the Windows leg
against that static library.

### slint pinned to `=1.19.0` for the `unstable-winit-030` feature (drag-and-drop)

**`slint` and `slint-build` are pinned to an EXACT version (`=1.19.0`), not `1`, because the UI crate enables the desktop-only `unstable-winit-030` feature.** Slint has no stable file-drop API, so OS file/folder drag-and-drop (`handlers/drag_drop.rs`) reaches the winit backend's raw `WindowEvent` filter (`slint::winit_030::WinitWindowAccessor::on_winit_window_event`) to receive `HoveredFile`/`DroppedFile`. That module is gated behind `unstable-winit-030` and is documented as "may be removed or changed in future minor releases", so a slint minor bump must be a deliberate, tested step rather than an automatic `cargo update`. The two versions MUST stay in lockstep (the proc-macro and the runtime are one release). To upgrade slint: bump both pins together, re-verify drag-and-drop builds and the `winit_030` API still resolves, then run the three gates.

### Thumbnail strip added no new dependencies

**The thumbnail strip added NO new dependencies** — it reuses the existing `image` (`DynamicImage::thumbnail`) and `rayon` (already a direct dep). Contrast the `unrar` C++-toolchain exception above: the thumbnail strip is dependency-free and adds no build cost.

### Cover carousel made `rayon` a direct dep of the `gashuu` UI crate (no new lockfile entry)

**The cover carousel added `rayon` to the `gashuu` UI crate's manifest** for its fire-and-forget cover worker (`cover_loader.rs` `rayon::spawn`). This adds NO new crate to `Cargo.lock` — `rayon` was already in the tree as a direct dep of `gashuu-core` (and transitively via `image`). The nuance: "no new dependencies" means the LOCKFILE (no new third-party code, no build cost), NOT the per-crate manifest — promoting an already-present transitive/sibling crate to a direct dep of another workspace crate is free.

### Dependency updates: Renovate automerge policy

**Renovate merges non-major dependency updates on its own — no human review.**
[`renovate.json`](../renovate.json) sets `"automerge": true` on the second-to-last `packageRules`
entry, matching the `minor`, `patch`, `pin` and `digest` update types, and on the
`lockFileMaintenance` object. That entry carries no `matchManagers` and no `matchPackageNames`, so
it is unrestricted — it applies to every manager Renovate detects here, today `cargo`,
`github-actions`, `mise` and `npm`, and to any manager added later. Exactly one entry follows it:
the `slint` exclusion described below. **`major` bumps are NOT automerged** — they open a PR and
wait for a person, deliberately, because a major release is where the breaking change lives.

**Renovate performs the merge, not GitHub.** `"platformAutomerge": false` is set EXPLICITLY
(Renovate's default is `true`), and it is load-bearing: `main` has no branch protection and its
only ruleset blocks deletion and non-fast-forward, so there are **zero required status checks**
for GitHub's own auto-merge to wait on — it could merge a PR whose CI is red. With
`platformAutomerge: false`, Renovate merges the PR itself on a later run, and only after it has
confirmed every check is green. The accepted cost is latency: a PR merges on Renovate's next run
after its checks pass, not the instant they pass. `"automergeStrategy"` must name a method the
repository actually permits, or GitHub rejects Renovate's merge call and automerge stops working
with no symptom other than its PRs sitting open. It is `"merge-commit"` (Renovate rejects the bare `"merge"` as an invalid value and stops opening PRs), matching `allow_merge_commit`
being the only enabled method (`allow_squash_merge` and `allow_rebase_merge` are both false).
`ops/worktree-stack/preflight.sh` asserts that pairing, along with the local git config that
keeps a fast-forwardable merge from landing with no merge commit at all.

**Automerge does not weaken the supply-chain guards.** `"minimumReleaseAge": "7 days"` still
withholds any release younger than seven days, and `"internalChecksFilter": "strict"` still holds
an update back until that age check has cleared. An automerged PR is therefore one that both
waited out the age guard and went green on every check — the guards run before automerge, not
instead of it. Do not relax either key to make a PR land sooner.

**Observed once, 2026-08-15.** The first unattended merge under this policy was `renovate/site`
(six npm patch/minor bumps), merged at 2026-08-15T07:40:02Z by `app/renovate` — the first merge
by a non-human actor in this repository; the 32 Renovate PRs merged before it were all merged by
hand. Three things it proves that the config alone cannot: **Renovate merged it, not GitHub** —
the merged PR reports `auto_merge: null`, so GitHub's native auto-merge was never armed, which
is exactly what `platformAutomerge: false` predicts; **the strategy held** — the merge commit
`a19f0cb` has a single parent, where every hand-merged Renovate PR around it is a two-parent
merge commit; and **the guards ran first** — all ten checks were green on the merged head,
including `renovate/stability-days`, which reached `success` 15m27s before the merge. Two things
it does NOT prove: no `major` update has been offered to this path since the policy landed, so
the `major` exclusion above is configured but still unobserved; and the Renovate run that
performed the merge was hand-triggered from the Dependency Dashboard — recorded at the time,
not re-derivable from the API afterwards — so the unattended *decision* is proven while
Renovate's unprompted cadence is not.

**Consequence for `deny.toml`: a stale ignore blocks the lockfile-maintenance PR, on purpose.**
[`.github/workflows/security.yml`](../.github/workflows/security.yml) runs
`cargo deny check advisories sources --deny advisory-not-detected`. That flag escalates an ignore
whose crate has LEFT the dependency tree from a warning into a hard error — and refreshing the
lockfile is exactly the operation that drops such crates, as their framework parents migrate away.
So a lockfile-maintenance branch is typically the one that goes red first, with every other check
green:

```
error[advisory-not-detected]: advisory was not encountered
  ┌─ deny.toml:<line>:11        # the offending ignore, wherever it sits
  │           no crate matched advisory criteria

advisories FAILED, sources ok
```

The PR then stops being automergeable and needs a human, which is the design working as intended:
`--deny advisory-not-detected` is the forcing function that keeps [`deny.toml`](../deny.toml)
pruned. **The fix is to delete the now-stale ignore — never to drop the flag, relax automerge, or
close the PR.**

**How to land that deletion: one commit that advances the lockfile AND deletes the ignore.**
There is no intermediate green state, so neither half can go in on its own:

- delete the ignore on `main` alone → the crate is still in `main`'s tree, so the advisory is
  detected with nothing ignoring it → `advisories FAILED`;
- advance the lockfile alone → the crate is gone, the ignore matches nothing →
  `error[advisory-not-detected]`.

And the deletion cannot come from the Renovate branch itself: **Renovate only authors `Cargo.lock`
and manifest edits — it will never write to `deny.toml`.** So a human opens a PR that carries both
halves in a single commit (bump the locked version that drops the crate, delete the matching
`ignore` line), and the stuck Renovate branches are rebased onto it afterwards.

**Deliberately pinned crates are excluded.** `slint` and `slint-build` are exact-pinned (`=`) on
purpose — see the slint exact-pin section above — so the LAST `packageRules` entry puts them in
their own `slint` group with `"automerge": false`. It sits AFTER the automerge entry, and that
ordering is what makes it work: Renovate applies matching rules in order and merges each match over
the previous one, so an exclusion placed before the automerge entry is silently overwritten. The
`config:recommended` preset contributes only earlier entries, so this repo's own rules always
resolve last. The separate group is load-bearing too — a grouped branch automerges only if EVERY
upgrade in it does, so leaving slint in the `rust` group would have stopped that whole group from
automerging whenever slint had a release. Nothing else is excluded: `image` and `zip` carry feature
allowlists rather than version pins (a bad bump is a hard cargo error, which a gate catches), the
`mise` tool versions are exact merely because mise requires a concrete version, and the SHA-pinned
GitHub Actions are supply-chain digest pins that Renovate exists to advance.

### image 0.25: RGBA → PNG bytes goes through `DynamicImage`

To encode raw RGBA into an in-memory PNG (`thumbnail_cache::put`), wrap the buffer in a `DynamicImage` and encode: `image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, bytes)?).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?`. Calling `write_to` directly on the `RgbaImage` (`ImageBuffer`) does NOT resolve against `image` 0.25 — `write_to` is reached via `DynamicImage`. `RgbaImage::from_raw` returns `Option` (`None` when the buffer is shorter than `w*h*4`), mapped to `CoreError::MalformedImage`. PNG is lossless, so a `put` → `get` round-trip is byte-exact.

### Never launch the GUI in a headless session

`cargo run` opens a GUI window — never launch the app from a non-interactive/headless session (it hangs). Verify with build + clippy + tests instead.

### App icon / bundling

**Icon asset pipeline** — `app-icon.svg` is the single committed source of truth for the app icon; no raster PNG master is committed. macOS bundling requires an `.icns`, so `app-icon.icns` is generated from the SVG and committed alongside it. Regenerate it whenever the SVG changes (`rsvg-convert` comes from `brew install librsvg`; `iconutil` ships with macOS):

```sh
# Render each required macOS iconset size straight from the SVG, then compile to .icns.
# The list is spelled out (not a variable) so it splits under both bash and zsh.
ICONSET=/tmp/gashuu.iconset; mkdir -p "$ICONSET"
for spec in 16:16x16 32:16x16@2x 32:32x32 64:32x32@2x 128:128x128 256:128x128@2x \
            256:256x256 512:256x256@2x 512:512x512 1024:512x512@2x; do
  px=${spec%%:*}; name=${spec#*:}
  rsvg-convert -w "$px" -h "$px" crates/gashuu/ui/assets/app-icon.svg -o "$ICONSET/icon_${name}.png"
done
iconutil -c icns -o crates/gashuu/ui/assets/app-icon.icns "$ICONSET"
```

Windows (`.ico`) and the AppImage (`.png`) icons are raster too, but those are generated on demand from the SVG during the release build (`magick` / `rsvg-convert`) and never committed — see the release-build notes below.

**Producing the macOS .app bundle** — install `cargo-bundle` once (compiles under pinned toolchain; binary lands in `~/.cargo/bin`), then build from the crate root:

```sh
mise exec -- cargo install cargo-bundle   # one-time
cd crates/gashuu && mise exec -- cargo bundle --release   # emits target/release/bundle/osx/gashuu.app  ("osx" is cargo-bundle's fixed dir name)
```

`cargo bundle` is NOT wired into the default `cargo build` or CI gates — but the release workflow (below) drives it on the macOS runner.

### Release builds (GitHub Actions)

`.github/workflows/release.yml` builds the distributable executables and attaches them to the GitHub Release for a tag. Trigger: push a `v*` tag, or `workflow_dispatch` with a `tag` input (to re-attach to an existing tag). **Dry run**: a PR touching `release.yml` or `.github/actions/**`, the weekly schedule, or a `workflow_dispatch` with an empty `tag` runs the same build jobs on the checked-out commit and skips `publish` (artifacts are named `v<version>-dryrun`). Before tagging, confirm the latest dry run on `main` is green, or dispatch one (`gh workflow run release.yml --ref main`). The steps come from the dispatching ref but the source and the local `.github/actions/` come from the tag, so a tag cut before a recipe fix cannot be rebuilt with it — re-tag instead. A `preflight` job asserts the tag matches `crates/gashuu/Cargo.toml` `version` before any build runs, so a mistyped tag fails fast. The GitHub Release must already exist — the workflow only uploads assets to it (`gh release upload --clobber`), it does not create it.

- **macOS (universal)**: builds `aarch64-apple-darwin` + `x86_64-apple-darwin`, `lipo`-merges them into a fat binary, runs `cargo bundle --release` for the `.app` scaffold (cargo-bundle has no `--target universal` support, so the scaffold is built once and the fat binary is spliced into `Contents/MacOS/`), and zips with `ditto -c -k --keepParent` (preserves symlinks/permissions). cargo-bundle is `cargo install`ed on the runner — deliberately NOT added to `mise.toml`, so the CI `app` matrix stays lean. Asset: `gashuu-<tag>-macos-universal.zip`.
- **Windows (x86_64)**: generates `app-icon.ico` from `app-icon.svg` with the runner's preinstalled `magick`, builds `--release` (`build.rs` embeds the icon via `winresource`), and zips the `.exe`. Asset: `gashuu-<tag>-windows-x64.zip`.
- **Linux (x86_64)**: builds on the oldest supported Ubuntu runner for wider glibc compatibility, packages a `.deb` with cargo-deb, and builds an AppImage with linuxdeploy. Assets: `gashuu-<tag>-amd64.deb` and `gashuu-<tag>-x86_64.AppImage`.
- **Signing**: macOS `.app` is ad-hoc (self-signed) in CI; Developer ID signing + notarization are deferred. Windows `signtool` insertion point remains marked as a `SIGNING SEAM` comment in `release.yml`.

**Windows `.ico` embedding** is now wired (was deferred): `winresource` is a `[target.'cfg(windows)'.build-dependencies]` so it is never fetched on macOS/Linux; `build.rs` gates the embed on `cfg(windows)` AND `CARGO_CFG_TARGET_OS == "windows"` AND the `.ico` existing, so a dev `cargo build` without the (CI-generated, uncommitted) `.ico` is a no-op and never a build blocker. **Linux packaging** ships a cargo-deb `.deb` and a linuxdeploy AppImage, built on the oldest supported Ubuntu for wider glibc compatibility; both include the `.desktop` entry and app icon.

### Post-release hands-on checklist: self-update + relaunch (NOT CI-verifiable)

`UpdateStrategy::SelfReplace` replaces the running binary and restarts the app. Neither the three
gates nor `release.yml` can exercise it — no runner accepts a GUI dialog or inspects the relaunched
process — so ADR-0013's "verified hands-on per-OS" obligation is discharged by running this
checklist ONCE per release, on a real Windows host (and, where available, a real Linux host).

Preconditions: the new release exists on GitHub with all five assets attached (macOS zip, Windows
zip, `.deb`, `.AppImage`, `SHA256SUMS`), and you have the PREVIOUS release's artifact to update FROM.

**Windows portable (the `current_exe()` relaunch-target question)**

1. Download the PREVIOUS release's `gashuu-<old-tag>-windows-x64.zip` and extract it into a clean,
   empty directory. Do not reuse a directory that has been updated before, and do not use a
   `cargo build` output — the checklist exercises the shipped artifact.
2. Launch that `gashuu.exe` and note its PID.
3. Set up observable session state so the relaunch's persistence can be checked: open a book, read
   to a distinctive page, toggle a per-book view mode (e.g. `d`), then MOVE and RESIZE the window.
4. Settings -> About -> "Check for updates now" (the manual check bypasses both the 24h throttle and
   any skipped version), then "Update now" in the dialog.
5. **Relaunch happened**: the app restarts on its own after the "Restarting…" note. No manual launch.
6. **The NEW version is running** — Settings -> About shows the new version number IMMEDIATELY after
   the automatic relaunch, without a manual restart. If it shows the OLD version and only a manual
   relaunch reveals the new one, `current_exe()` resolved to the renamed-away old executable: FILE A
   BUG against the Windows arm of `apply_self_replace` (this is the assumption this checklist
   exists to test).
7. **Session state survived** the relaunch (all four): the book reopens on the page from step 3, the
   per-book view mode from step 3 is in effect, the window has the size AND position from step 3,
   and the library shows page counts without a re-count pause.
8. **No zombie process**: `tasklist /FI "IMAGENAME eq gashuu.exe"` lists exactly ONE gashuu.exe, and
   its PID differs from step 2's.
9. **Leftovers, recorded not fixed**: note whether `%TEMP%\gashuu.exe` (the extraction target) and
   any `self_replace` rename artifact remain, and that the verified zip is in the Downloads folder.
   These are known, tracked gaps — record what you observe, do not fix them during a release.
10. Re-run the check (Settings -> About -> "Check for updates now") on the relaunched app: it must
    now report that you are on the latest version.

**Linux AppImage (same obligation, second artifact)**

Same shape, with the AppImage specifics: run the PREVIOUS release's `.AppImage` (so `$APPIMAGE` is
set), update, and verify (a) the app relaunched, (b) Settings -> About shows the new version (the
app parses no CLI arguments, so there is no `--version` flag to check), (c) the session state from
step 3 survived, (d) NO `<name>.AppImage.new` sibling is left behind, (e) the replaced file is still
executable (`ls -l` shows mode 0755), and (f) exactly one process remains (`pgrep -c gashuu`).

**Record the result in the release's notes or tracking issue** — "self-update verified on
Windows <version> / Ubuntu <version>" — so a release with no such note is visibly unverified.
