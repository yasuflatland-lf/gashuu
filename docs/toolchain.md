# Toolchain & build

Reference doc migrated from the CLAUDE.md "Toolchain & build" section.
All technical details are preserved verbatim from that source of truth.

### Rust pin & mise

Rust is pinned to **1.96.0** via `mise.toml`. Run every cargo command through the pin: `mise exec -- cargo <...>`.

### Fresh install: mise trust

**A fresh `mise install` fails with "Config files are not trusted."** Run `mise trust` once, then `mise install`. CI's `jdx/mise-action` handles trust automatically.

### Dependency debuginfo trimmed to line tables (root-manifest `[profile.dev]`)

The workspace root `Cargo.toml` sets `[profile.dev] debug = "line-tables-only"` and puts the two members back to `debug = 2` via `[profile.dev.package.gashuu-core]` / `[profile.dev.package.gashuu]`, so the trim is **dependencies-only**: stepping through and backtracing gashuu's own code is unchanged. `line-tables-only` rather than `0` because dependency frames then still carry `file:line` in panic backtraces — that is what makes an `image` / `zip` / `slint` panic inside a test diagnosable. Measured with a fresh `CARGO_TARGET_DIR` on a cold `cargo nextest run --workspace --profile ci --no-run`: target output 3.52 GiB → 3.03 GiB (-13.7%), rustc CPU 290 s → 272 s, incremental rebuild after a `gashuu-core` edit 5.1 s → 3.7 s. `[profile.test]` inherits all of this, so there is no `test` twin. **Do not "simplify" this to `[profile.dev.package."*"]`** — a package override counts as an *explicit* debuginfo setting, so cargo stops suppressing debuginfo for the build graph and the 211 build-script / proc-macro units (`i-slint-compiler` and friends) gain line tables, which cuts the saving from -493 MiB to -118 MiB; `[profile.dev.build-override]` cannot cancel that, because package overrides are applied after it.

### Linux system libraries (Slint)

Slint links system libraries on **Linux** only: `libfontconfig1-dev libfreetype6-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev`. macOS/Windows need nothing extra **for Slint** — but the dav1d AVIF build dependency below applies on all 3 OS.

### zip dependency: deflate-only, no default features

**`zip` is declared `{ version = "2", default-features = false, features = ["deflate"] }` — never enable its default features.** They pull native C `-sys` libs (bzip2-sys/lzma-sys/zstd-sys) that would add a needless C toolchain burden on every OS (unlike the two justified exceptions below: `unrar` and dav1d). CBZ/ZIP manga pages use only Stored (always available) or Deflate (pure-Rust via flate2/miniz_oxide), so `deflate`-only keeps the cross-platform build clean.

### unrar dependency: C++ toolchain (knowing exception)

**`unrar` is declared `unrar = "0.5"` (always-on, NO feature gate, per Issue #7) and DOES require a C++ compiler on all 3 OS** — it bundles C++ UnRAR built via `cc` (gcc/clang/MSVC, all standard on GitHub runners; macOS Apple clang suffices, no extra apt pkgs beyond the Slint set). This is a knowing exception to the `zip` "no native toolchain" stance: RAR has no pure-Rust decoder, so the C++ compile is unavoidable (build-time cost, cached by `rust-cache`). The non-free RARLAB license clause is recorded in [THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md) (repo root, an acceptance requirement). `base64 = "0.22"` is a core dev-dep (decodes the base64 RAR fixtures). **Intentional deviation from the plan-design doc:** it specified `unrar` (fine) but the implementation uses the SYNCHRONOUS `unrar` over the existing rayon pool — sync `read_bytes` + CPU-bound decode fit rayon naturally; async would force a `block_on` bridge and infect every layer with tokio (same rationale as the sync `zip`).

### dav1d dependency: system C library for AVIF decode (knowing exception)

**`image` is declared `{ version = "0.25", default-features = false, features = ["png", "jpeg",
"qoi", "avif-native"] }`** — an explicit DECODE-oriented allowlist (defaults OFF, same stance as
`zip`). gashuu only ever DECODES pages (`png`/`jpeg`/`avif`) and ENCODES only to `png`
(cache/thumbnails) and `qoi` (thumbnail cache), so every other default format — and, crucially,
the `avif` **encode** feature that pulls the whole `ravif` -> `rav1e` AV1 encoder — is excluded,
keeping `rav1e` out of the shipped binary. **`avif-native` enables AVIF decode via dav1d and
requires `dav1d >= 1.3.0` at BUILD time on all 3 OS** — resolved by the `dav1d-sys`/`system-deps`
build chain (pkg-config, or the `SYSTEM_DEPS_DAV1D_*` env overrides CI uses). This is the second
knowing exception to the `zip` "no native toolchain" stance; the decoder choice and license
rationale live in [ADR-0010](ADRs/0010-avif-decode-via-dav1d.md) (dav1d is BSD-2-Clause, recorded
in [THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md)). **Caveat:** `rav1e` (and its `paste` /
RUSTSEC-2024-0436 advisory) still appears in `Cargo.lock` because Slint's build-time `.slint`
compiler (`i-slint-compiler`) depends on `image` with its DEFAULT features — outside our control;
trimming gashuu's own features does not remove it (see the ignore reason in `deny.toml`).

Dev setup — **macOS**: `brew install dav1d` (verify: `pkg-config --modversion dav1d`).
**Linux**: `sudo apt-get install -y libdav1d-dev` (Ubuntu 24.04 ships 1.4.1). **Windows**:
`vcpkg install dav1d:x64-windows-static-md`, then set the `SYSTEM_DEPS_DAV1D_*` variables as in
`.github/workflows/ci.yml`. Without dav1d the build fails loudly at `dav1d-sys` with a clear
"dav1d not found" probe error — intentional, since the dependency is not feature-gated.

**End users still install nothing**: release builds link dav1d STATICALLY — macOS builds it from
source per arch (meson/ninja/nasm) and lipo-merges one fat `libdav1d.a`; Windows uses the vcpkg
static triplet. Both release jobs assert the result (`otool -L` / `dumpbin /dependents` show no
dav1d dynamic reference). CI test jobs may link dynamically (brew/apt) — nothing ships from CI.

### slint pinned to `=1.16.1` for the `unstable-winit-030` feature (drag-and-drop)

**`slint` and `slint-build` are pinned to an EXACT version (`=1.16.1`), not `1`, because the UI crate enables the `unstable-winit-030` feature.** Slint has no stable file-drop API, so OS file/folder drag-and-drop (`handlers/drag_drop.rs`) reaches the winit backend's raw `WindowEvent` filter (`slint::winit_030::WinitWindowAccessor::on_winit_window_event`) to receive `HoveredFile`/`DroppedFile`. That module is gated behind `unstable-winit-030` and is documented as "may be removed or changed in future minor releases", so a slint minor bump must be a deliberate, tested step rather than an automatic `cargo update`. The two versions MUST stay in lockstep (the proc-macro and the runtime are one release). To upgrade slint: bump both pins together, re-verify drag-and-drop builds and the `winit_030` API still resolves, then run the three gates.

### Thumbnail strip added no new dependencies

**The thumbnail strip added NO new dependencies** — it reuses the existing `image` (`DynamicImage::thumbnail`) and `rayon` (already a direct dep). Contrast the `unrar` C++-toolchain exception above: the thumbnail strip is dependency-free and adds no build cost.

### Cover carousel made `rayon` a direct dep of the `gashuu` UI crate (no new lockfile entry)

**The cover carousel added `rayon` to the `gashuu` UI crate's manifest** for its fire-and-forget cover worker (`cover_loader.rs` `rayon::spawn`). This adds NO new crate to `Cargo.lock` — `rayon` was already in the tree as a direct dep of `gashuu-core` (and transitively via `image`). The nuance: "no new dependencies" means the LOCKFILE (no new third-party code, no build cost), NOT the per-crate manifest — promoting an already-present transitive/sibling crate to a direct dep of another workspace crate is free.

### Dependency updates: Renovate automerge policy

**Renovate merges non-major dependency updates on its own — no human review.**
[`renovate.json`](../renovate.json) sets `"automerge": true` on a final `packageRules` entry
matching the `minor`, `patch`, `pin` and `digest` update types, and on the `lockFileMaintenance`
object. That entry carries no `matchManagers` and no `matchPackageNames`, so it is unrestricted —
it applies to every manager Renovate detects here, today `cargo`, `github-actions`, `mise` and
`npm`, and to any manager added later. **`major` bumps are NOT
automerged** — they open a PR and wait for a person, deliberately, because a major release is
where the breaking change lives.

**Renovate performs the merge, not GitHub.** `"platformAutomerge": false` is set EXPLICITLY
(Renovate's default is `true`), and it is load-bearing: `main` has no branch protection and its
only ruleset blocks deletion and non-fast-forward, so there are **zero required status checks**
for GitHub's own auto-merge to wait on — it could merge a PR whose CI is red. With
`platformAutomerge: false`, Renovate merges the PR itself on a later run, and only after it has
confirmed every check is green. The accepted cost is latency: a PR merges on Renovate's next run
after its checks pass, not the instant they pass. `"automergeStrategy": "squash"` matches the
repo's squash-based history (`allow_rebase_merge` is false).

**Automerge does not weaken the supply-chain guards.** `"minimumReleaseAge": "7 days"` still
withholds any release younger than seven days, and `"internalChecksFilter": "strict"` still holds
an update back until that age check has cleared. An automerged PR is therefore one that both
waited out the age guard and went green on every check — the guards run before automerge, not
instead of it. Do not relax either key to make a PR land sooner.

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

**Known gap — pinned crates are not excluded.** The automerge rule has no per-package exclusion,
so a `minor` release of a crate this file pins on purpose (see the slint exact-pin section above,
where a minor bump is required to be a deliberate, tested step) is swept into automerge along with
everything else. If that matters for a given pin, add a `matchPackageNames` rule with
`"automerge": false` to `renovate.json`; nothing in the config does that today.

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

`.github/workflows/release.yml` builds the distributable executables and attaches them to the GitHub Release for a tag. Trigger: push a `v*` tag, or `workflow_dispatch` with a `tag` input (to re-attach to an existing tag). A `preflight` job asserts the tag matches `crates/gashuu/Cargo.toml` `version` before any build runs, so a mistyped tag fails fast. The GitHub Release must already exist — the workflow only uploads assets to it (`gh release upload --clobber`), it does not create it.

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
