# gashuu

[![CI](https://img.shields.io/github/actions/workflow/status/yasuflatland-lf/gashuu/ci.yml?branch=main&label=CI&logo=github)](https://github.com/yasuflatland-lf/gashuu/actions/workflows/ci.yml)
[![coverage](https://img.shields.io/codecov/c/github/yasuflatland-lf/gashuu?flag=rust&label=coverage&logo=codecov)](https://codecov.io/gh/yasuflatland-lf/gashuu)

A cross-platform manga viewer built with Rust and [Slint](https://slint.dev).

## Status (PR6 — ZIP/CBZ archive support)

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

**Navigation**

- **→ / Space** — next page (or spread)
- **← / Backspace** — previous page (or spread)
- **D** — cycle spread mode: single → double → auto
- **R** — toggle reading direction (LTR ↔ RTL)
- **C** — toggle cover layout (standalone ↔ paired)

Toggle changes are remembered (saved on exit).

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
- `cache_size` — number of decoded images held in the LRU cache (default `50`).
- `preload_pages` — background prefetch radius around the current page (default `3`).
- `recent_files` — list of recently opened folders and archives.  Recorded only when
  `track_recent_files` is `true`; it is **off by default** for privacy.  To enable,
  open the file and change `"track_recent_files": false` to `true`.

**Saved for forward-compatibility** (persisted now; wired up in later releases):

- `key_bindings` — custom keyboard shortcuts.

If the settings file is corrupt or unreadable, gashuu falls back to built-in defaults
and keeps running.

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
