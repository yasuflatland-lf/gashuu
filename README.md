# gashuu

[![CI](https://img.shields.io/github/actions/workflow/status/yasuflatland-lf/gashuu/ci.yml?branch=main&label=CI&logo=github)](https://github.com/yasuflatland-lf/gashuu/actions/workflows/ci.yml)
[![coverage](https://img.shields.io/codecov/c/github/yasuflatland-lf/gashuu?flag=rust&label=coverage&logo=codecov)](https://codecov.io/gh/yasuflatland-lf/gashuu)

A cross-platform manga viewer built with Rust and [Slint](https://slint.dev).

## Status (PR2 — Cached viewer)

Open a folder of PNG/JPG images and browse every page with the keyboard. Pages are
held in an LRU cache (up to 50 decoded images) and the neighbours of the current
page are prefetched in the background, so warmed page turns are effectively instant.

- **→ / Space** — next page
- **← / Backspace** — previous page

Set `RUST_LOG=debug` to see per-turn latency (`page turn elapsed_ms=…`).

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
