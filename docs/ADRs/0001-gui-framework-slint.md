# ADR-0001: Adopt Slint as the GUI framework

- Status: Accepted
- Decided: 2026-05-31 (transcribed: 2026-06-01)
- Related: [ADR-0002](0002-layered-two-crate-architecture.md) (core↔UI boundary), [ADR-0003](0003-image-loading-and-caching.md) (memory control)

## Context

We are building a cross-platform (Windows / macOS / Linux) desktop manga viewer as a personal
OSS project. The project has the following defining requirements, which make the choice of GUI
framework the single most architecture-shaping decision.

- **Leverage Rust's performance and memory safety** — crash resistance and 60fps rendering of
  4K-class (up to ~3000×4000px) images.
- **Image-heavy workload with strict memory control** — even with a large archive open, prefetch
  and eviction must be controllable at a fine grain, with a hard ceiling on memory usage
  (see [ADR-0003](0003-image-loading-and-caching.md)).
- **Treat the Japanese manga experience as a first-class citizen** — right-to-left, two-page
  spread reading must be comfortable by default.
- **Distribute as a single self-contained binary** — users should not need to pre-install a
  Python runtime or similar.

The target audience is primarily technical users and heavy readers, with Linux handhelds such as
the Steam Deck also in scope.

## Decision

Adopt **Slint** (pure Rust, declarative DSL, Skia GPU backend) as the GUI framework.

- Language is **Rust**, Edition 2021.
- Single process, declarative UI (`.slint`) plus Rust application logic.
- License under Slint's Royalty-free Desktop License; the application itself ships under a dual
  MIT / Apache-2.0 license.

The primary driver is **ease of native memory control**. A manga viewer handles many
high-resolution images, so a design that delegates rendering to a Web UI layer makes prefetch /
eviction control difficult. With Slint, decoded pixel buffers are fully owned and managed on the
Rust side, and the UI layer receives only RGBA byte slices
(see [ADR-0002](0002-layered-two-crate-architecture.md)).

## Alternatives considered

### Tauri (Rust backend + web frontend)
- **Pros**: free UI design via HTML/CSS, immediate use of the web ecosystem, large UI talent pool.
- **Cons**: memory management on the WebView side is hard with many high-resolution images,
  fine-grained prefetch/eviction control is awkward, type safety is lost across the IPC boundary.
- **Why rejected**: a manga viewer is image-heavy and memory control is a requirement; prefer
  Slint, which keeps that control native.

### Qt + PySide6
- **Pros**: abundant reference implementations among existing viewers (YACReader, MComix, …),
  visual UI building with Qt Designer, mature `QGraphicsView`.
- **Cons**: misaligned with the project's motivation (Rust performance/safety), the Python GIL
  caps parallel-decode throughput, distribution requires bundling a Python runtime.
- **Why rejected**: mismatch with the project's motivation.

### CXX-Qt (Rust + Qt bridge)
- **Pros**: can fuse the strengths of Rust and Qt, officially maintained by KDAB.
- **Cons**: requires knowledge of three stacks (Rust / C++ / Qt), complex build setup, KDAB
  itself states it is "for advanced users," plus Qt (LGPL/commercial) license handling.
- **Why rejected**: at a personal-project scale, the complexity outweighs the benefit.

### Flutter Desktop
- **Pros**: fast Impeller rendering, potential for future mobile expansion.
- **Cons**: Dart's `archive` package does not support RAR, forcing an FFI implementation;
  mismatch with the Rust motivation; desktop feel is somewhat weak.
- **Why rejected**: RAR support cost and motivation mismatch.

### Egui / Iced (other pure-Rust GUIs)
- **Pros**: a fully Rust ecosystem.
- **Cons**: Egui is immediate-mode, making complex two-page spread layout tuning cumbersome;
  Iced is pre-1.0 with breaking-change risk.
- **Why rejected**: Slint is stronger for an image-centric app, declarative layout, and API stability.

## Consequences

### Positive
- Decoded pixels are fully controlled on the Rust side, enabling a memory ceiling via LRU +
  prefetch (see [ADR-0003](0003-image-loading-and-caching.md)).
- A pure-Rust single-language stack keeps the domain layer Slint-independent
  (see [ADR-0002](0002-layered-two-crate-architecture.md)).
- A single executable can be shipped per OS with no bundled runtime.
- The license is compatible with OSS distribution.

### Costs / trade-offs accepted
- Smaller UI ecosystem and talent pool than a web frontend; design freedom is bounded by the DSL's
  expressiveness.
- **Linux only** requires Slint system libraries (fontconfig / freetype / xcb / xkbcommon family);
  macOS / Windows need nothing extra (this is the basis for the README's
  "macOS/Windows need nothing extra" policy).
- The Slint DSL must track Slint versions; version-fragile logic (e.g. empty-image detection) is
  done on the Rust side rather than in the DSL (see [docs/patterns.md](../patterns.md)).

## Implementation notes (as-built deltas)

- From the first implementation onward, the app runs exactly as decided on a Slint UI (`*.slint` +
  `main.rs` under `crates/gashuu`). No divergence from the source.
