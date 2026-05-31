# gashuu

[![CI](https://img.shields.io/github/actions/workflow/status/yasuflatland-lf/gashuu/ci.yml?branch=main&label=CI&logo=github)](https://github.com/yasuflatland-lf/gashuu/actions/workflows/ci.yml)
[![coverage](https://img.shields.io/codecov/c/github/yasuflatland-lf/gashuu?flag=rust&label=coverage&logo=codecov)](https://codecov.io/gh/yasuflatland-lf/gashuu)

A cross-platform manga viewer built with Rust and [Slint](https://slint.dev).

## Status (PR3 — Settings persistence)

Open a folder of PNG/JPG/JPEG images and browse every page with the keyboard. Pages are
held in an LRU cache (up to 50 decoded images) and the neighbours of the current
page are prefetched in the background, so warmed page turns are effectively instant.
User settings now persist across restarts — gashuu saves your preferences on exit and
restores them on the next launch.

- **→ / Space** — next page
- **← / Backspace** — previous page

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

- `cache_size` — number of decoded images held in the LRU cache (default `50`).
- `preload_pages` — background prefetch radius around the current page (default `3`).
- `recent_files` — list of recently opened folders.  Recorded only when
  `track_recent_files` is `true`; it is **off by default** for privacy.  To enable,
  open the file and change `"track_recent_files": false` to `true`.

**Saved for forward-compatibility** (persisted now; wired up in later releases):

- `reading_direction` — right-to-left reading mode is planned.
- `spread_mode` — two-page spread layout.
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

- `crates/gashuu-core` — Slint-independent domain + I/O (page sources, image decode).
- `crates/gashuu` — Slint presentation layer.

## License

MIT — see [LICENSE](LICENSE).
