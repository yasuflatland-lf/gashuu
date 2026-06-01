# gashuu

[![CI](https://img.shields.io/github/actions/workflow/status/yasuflatland-lf/gashuu/ci.yml?branch=main&label=CI&logo=github)](https://github.com/yasuflatland-lf/gashuu/actions/workflows/ci.yml)
[![coverage](https://img.shields.io/codecov/c/github/yasuflatland-lf/gashuu?flag=rust&label=coverage&logo=codecov)](https://codecov.io/gh/yasuflatland-lf/gashuu)

A cross-platform manga / comic viewer built with Rust and [Slint](https://slint.dev).
Open a folder of images or a comic archive and read with the keyboard — two-page
spreads, right-to-left binding, zoom/pan, a thumbnail strip, and persistent settings.

## Features

- **Sources** — a folder of PNG/JPG/JPEG images, or a `.cbz`/`.zip`/`.cbr`/`.rar`
  archive. The format is detected by extension or magic bytes, so a mis-named archive
  still opens.
- **Archives** — pages are read in natural filename order and images nested in
  subfolders are included. Extraction is in-memory (nothing is written to disk); unsafe,
  oversized, or corrupt entries are skipped and counted in the status bar.
- **Spreads** — single page, two-page spread, or **auto** (picks single/double from the
  window aspect ratio and follows resizes live). Right-to-left (manga) or left-to-right
  binding, with a standalone or paired cover layout.
- **Zoom & pan** — the wheel zooms at the cursor and drag pans; fit modes are Whole /
  Width / Actual. Zoom and pan are session-only; the fit mode is saved.
- **Fast page turns** — pages are held in an LRU cache and neighbours are prefetched in
  the background, so warmed turns are effectively instant.
- **Thumbnail strip** — previews of every page, generated in parallel so the strip fills
  in while you read. Click a thumbnail to jump; the current page is highlighted; a
  thumbnail that fails to generate shows a red ✕.
- **Page scrubber & counter** — a bottom scrub bar and a top-right page-counter chip
  appear on mouse-move, arrow-key press, or scrubber drag, then fade after idle. Drag the
  knob to scrub; a thumbnail preview (one or two pages for single/double spreads) pops up
  during the drag and the page only changes on release. RTL-aware: in manga mode dragging
  left advances pages.
- **Settings dialog & first-run guide** — change every active option from the toolbar
  without hand-editing config, and a one-time welcome overlay summarises the controls.
- **Safe decoding** — oversized images and decompression bombs are rejected before
  allocating memory (16 384×16 384 px / 512 MiB / ~128 Mpx limits), with a clear error
  in the status bar instead of an out-of-memory crash.

## Getting Started

Toolchain and tools are managed by [mise](https://mise.jdx.dev) (Rust 1.96.0 +
cargo-nextest + cargo-llvm-cov):

```bash
mise trust      # trust ./mise.toml (once per fresh clone)
mise install
```

On Linux, install Slint's system libraries (macOS and Windows need nothing extra):

```bash
sudo apt-get install -y libfontconfig1-dev libfreetype6-dev libxcb1-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev
```

A C++ compiler is also required — the RAR/CBR backend bundles the C++ UnRAR sources and
builds them via `cc`. This is standard on every platform (Xcode CLT on macOS, MSVC or
MinGW on Windows, `build-essential` on Linux) and adds nothing beyond the usual toolchain.

Then run the viewer and open a folder or archive from the toolbar:

```bash
cargo run -p gashuu
```

## Usage

Open content from the toolbar — **Open Folder…** (PNG/JPG/JPEG) or **Open Archive…**
(`.cbz`/`.zip`/`.cbr`/`.rar`). Navigation works the same for folders and archives.

**Navigation**

| Key | Action |
|-----|--------|
| `→` / `Space` | Next page or spread |
| `←` / `Backspace` | Previous page or spread |
| `D` | Cycle spread mode: single → double → auto |
| `R` | Toggle reading direction (LTR ↔ RTL) |
| `C` | Toggle cover layout (standalone ↔ paired) |
| `T` | Toggle the thumbnail strip |

Arrows follow the reading direction (LTR: `→` = next; RTL: `←` = next). `Space` and
`Backspace` are always next/previous in reading order. Mode changes are saved on exit.

The **page scrubber** (bottom bar) and **page-counter chip** (top-right) appear on
mouse-move, arrow-key press, or scrubber drag, then fade after idle. Drag the knob to
preview pages without turning; the page changes on release. In RTL mode dragging left
advances pages.

**Zoom & fit** (direction-independent)

| Input | Action |
|-------|--------|
| Mouse wheel | Zoom at the cursor (1.0×–8.0× of the fit baseline) |
| Click-drag | Pan the viewport |
| `+` / `=` | Zoom in |
| `-` | Zoom out |
| `0` | Reset view (fit baseline, re-centered) |
| `1` | Actual size (1:1 pixels) |
| `f` | Cycle fit mode (Whole → Width → Actual) |

Zoom and pan apply to the whole viewport (both pages in a spread move together). Page
turns keep the current zoom and fit and only re-center the pan. Set `RUST_LOG=debug` to
log per-turn latency.

**Settings dialog** — click **Settings…** to change reading direction, spread mode,
cover layout, fit mode, cache size, preload radius, and the recent-files toggle.
Display-mode changes apply immediately; cache size and preload radius take effect on the
next book you open. The dialog also lists the keyboard shortcuts (remapping is not yet
supported).

## Settings

Settings are stored as JSON in the OS config directory, loaded on startup and saved on
exit. The Settings dialog is the easiest way to change them, but the file can be
hand-edited:

| Platform | Path |
|----------|------|
| Linux    | `~/.config/gashuu/settings.json` |
| macOS    | `~/Library/Application Support/gashuu/settings.json` |
| Windows  | `%APPDATA%\gashuu\settings.json` |

| Key | Values | Notes |
|-----|--------|-------|
| `reading_direction` | `"ltr"` (default) / `"rtl"` | Right-to-left = manga binding |
| `spread_mode` | `"single"` (default) / `"double"` / `"auto"` | Auto chooses from the window aspect ratio |
| `cover_mode` | `"standalone"` (default) / `"paired"` | Applies to double mode only |
| `fit_mode` | `"whole"` (default) / `"width"` / `"actual"` | Initial fit; cycle with `f` |
| `cache_size` | int (default `50`) | LRU decoded-image cache; applies to the next book |
| `preload_pages` | int (default `3`) | Background prefetch radius; applies to the next book |
| `track_recent_files` | bool (default `false`) | Off for privacy; gates `recent_files` |
| `recent_files` | list | Recorded only when tracking is on |
| `key_bindings` | — | Persisted for forward-compatibility; not yet wired up |

If the settings file is corrupt or unreadable, gashuu falls back to built-in defaults
and keeps running.

## Project layout

- `crates/gashuu-core` — Slint-independent domain + I/O: folder, ZIP/CBZ, and RAR/CBR
  page sources, image decode, LRU cache + prefetch, thumbnails, and settings.
- `crates/gashuu` — Slint presentation layer (windows, dialogs, input, rendering).

## Development

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
```

## License

MIT — see [LICENSE](LICENSE). The RAR/CBR backend uses the UnRAR library, which carries
RARLAB's non-free license (read-only use is permitted; re-creating the RAR compression
algorithm is not). See [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md) for the full
UnRAR license text.
