# gashuu

[![CI](https://img.shields.io/github/actions/workflow/status/yasuflatland-lf/gashuu/ci.yml?branch=main&label=CI&logo=github)](https://github.com/yasuflatland-lf/gashuu/actions/workflows/ci.yml)
[![coverage](https://img.shields.io/codecov/c/github/yasuflatland-lf/gashuu?flag=rust&label=coverage&logo=codecov)](https://codecov.io/gh/yasuflatland-lf/gashuu)

A cross-platform manga viewer built with Rust and [Slint](https://slint.dev).

## Status (PR5 — Zoom/pan + fit modes · PR6 — ZIP/CBZ archive support)

Open a **folder** of PNG/JPG/JPEG images, or a **CBZ/ZIP comic archive**, and browse
every page with the keyboard. Pages are held in an LRU cache (up to 50 decoded images)
and the neighbours of the current page are prefetched in the background, so warmed page
turns are effectively instant. You can read in a two-page spread with right-to-left
(manga) or left-to-right binding, in addition to single-page browsing. An **auto**
spread mode picks single or double from the window's aspect ratio and follows resizes
live. User settings persist across restarts — gashuu saves your preferences on exit and
restores them on the next launch.

Arrow keys follow the active reading direction: in LTR mode **→** advances and **←**
goes back; in RTL mode the arrows are swapped (**←** advances / **→** goes back).
**Space** and **Backspace** are always next/prev in reading order regardless of
direction.

**Opening content**

- **Open Folder** button — pick a folder of PNG/JPG/JPEG images.
- **Open Archive** button — pick a `.cbz` or `.zip` file. Pages inside the archive are
  read in natural filename order; images nested in subfolders within the archive are
  included. Archives are extracted in-memory (no files written to disk); unsafe,
  oversized, or corrupt entries are skipped and the count is shown in the status bar.

Once a source is open, all navigation, spread, and layout controls work the same
regardless of whether you opened a folder or an archive.

You can also zoom and pan any page (or two-page spread). Zoom and pan apply to the whole
viewport — in two-page spread mode both pages zoom and pan together. Page turns keep
the current zoom and fit; only the pan position re-centers. Zoom/pan are GPU texture
transforms (no re-decode), designed for 60 fps at 4K. The zoom level and pan position
are session-only and are not saved; the fit mode is persisted.

**Page navigation**

- **→ / Space** — next page (or spread)
- **← / Backspace** — previous page (or spread)
- **D** — cycle spread mode: single → double → auto
- **R** — toggle reading direction (LTR ↔ RTL)
- **C** — toggle cover layout (standalone ↔ paired)

Toggle changes are remembered (saved on exit).

**Zoom & pan** (direction-independent)

- **Mouse wheel** — zoom in/out, centered at the cursor position. Zoom range: 1.0×–8.0× relative to the fit baseline.
- **Click-drag** — pan the viewport.
- **`+` / `=`** — zoom in
- **`-`** — zoom out
- **`0`** — reset view (zoom 1.0 × fit baseline, re-center)
- **`1`** — actual size (1:1 pixels, equivalent to `Actual` fit mode)
- **`f`** — cycle fit mode (`Whole` → `Width` → `Actual`)

Set `RUST_LOG=debug` to see per-turn latency (`page turn elapsed_ms=…`).

## Settings

Settings are stored as JSON in the OS config directory. On first run gashuu writes a
default file you can hand-edit; the file is loaded on startup and saved on exit.

| Platform | Path |
|----------|------|
| Linux    | `~/.config/gashuu/settings.json` |
| macOS    | `~/Library/Application Support/gashuu/settings.json` |
| Windows  | `%APPDATA%\gashuu\settings.json` |

**Active settings** (take effect today):

- `reading_direction` — `"ltr"` (default) or `"rtl"` (right-to-left / manga binding).
- `spread_mode` — `"single"` (default), `"double"` (two-page spread), or `"auto"`
  (chooses single or double from the window aspect ratio: landscape/square → double,
  portrait → single; follows window resizes live; composes with RTL/LTR and cover mode).
- `cover_mode` — `"standalone"` (default: cover shown alone, then pages pair up as
  {1,2}{3,4}…) or `"paired"` (pairing starts from the cover: {0,1}{2,3}…). Only
  affects double (or auto-resolved double) mode.
- `fit_mode` — initial fit applied when a page is displayed: `"whole"` (default, fit
  the whole page letterboxed), `"width"` (fill the viewport width; page may overflow
  vertically and be pannable), or `"actual"` (1:1 pixels). Cycle at runtime with **`f`**
  or jump to actual size with **`1`**. Zoom level and pan position are session-only and
  not saved.
- `cache_size` — number of decoded images held in the LRU cache (default `50`).
- `preload_pages` — background prefetch radius around the current page (default `3`).
- `recent_files` — list of recently opened folders and archives.  Recorded only when
  `track_recent_files` is `true`; it is **off by default** for privacy.  To enable,
  open the file and change `"track_recent_files": false` to `true`.

**Saved for forward-compatibility** (persisted now; wired up in later releases):

- `key_bindings` — custom keyboard shortcuts.

If the settings file is corrupt or unreadable, gashuu falls back to built-in defaults
and keeps running.

## Safety

gashuu rejects oversized images before allocating memory. In addition to the existing
16 384×16 384 pixel / 512 MiB per-image limits, any image whose total pixel count
exceeds ~128 megapixels is refused at decode time. Files that exceed either limit
surface a clear "image too large" error in the status bar instead of risking an
out-of-memory crash (defense-in-depth against decompression bombs).

## Develop

First-time setup (the `mise trust` step is required once for a fresh clone):

```bash
mise trust                                # trust ./mise.toml before installing
mise install                              # Rust 1.96.0 + cargo-nextest + cargo-llvm-cov
```

On Linux, install Slint's system libraries (macOS and Windows need nothing extra):

```bash
sudo apt-get install -y libfontconfig1-dev libfreetype6-dev libxcb1-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev
```

Then:

```bash
cargo nextest run --workspace             # tests
cargo clippy --workspace --all-targets -- -D warnings
RUST_LOG=info cargo run -p gashuu         # run the viewer
```

## Workspace

- `crates/gashuu-core` — Slint-independent domain + I/O (page sources including folder
  and ZIP/CBZ archive support, image decode).
- `crates/gashuu` — Slint presentation layer.

## License

MIT — see [LICENSE](LICENSE).
