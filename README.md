# gashuu

[![CI](https://img.shields.io/github/actions/workflow/status/yasuflatland-lf/gashuu/ci.yml?branch=main&label=CI&logo=github)](https://github.com/yasuflatland-lf/gashuu/actions/workflows/ci.yml)
[![coverage](https://img.shields.io/codecov/c/github/yasuflatland-lf/gashuu?flag=rust&label=coverage&logo=codecov)](https://codecov.io/gh/yasuflatland-lf/gashuu)

A cross-platform manga viewer built with Rust and [Slint](https://slint.dev).

## Status (PR1 — MVP)

Open a folder of PNG/JPG images and browse every page with the keyboard.

- **→ / Space** — next page
- **← / Backspace** — previous page

## Develop

```bash
mise install                              # Rust 1.96.0 + cargo-nextest + cargo-llvm-cov
cargo nextest run --workspace             # tests
cargo clippy --workspace --all-targets -- -D warnings
RUST_LOG=info cargo run -p gashuu         # run the viewer
```

## Workspace

- `crates/gashuu-core` — Slint-independent domain + I/O (page sources, image decode).
- `crates/gashuu` — Slint presentation layer.

## License

MIT — see [LICENSE](LICENSE).
