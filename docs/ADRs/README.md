# Architecture Decision Records (ADR)

This directory records the **architecturally significant decisions** for gashuu in ADR form.
The source is the approved design document (2026-05-31, "gashuu Manga Viewer — Implementation
Plan Design" and the original Design Doc it derives from). These ADRs transcribe that rationale
on a "one decision = one record" basis. Where the codebase intentionally diverged from the
source, the divergence is called out in each ADR's "Implementation notes (as-built deltas)" section.

## Index

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-gui-framework-slint.md) | Adopt Slint as the GUI framework | Accepted |
| [0002](0002-layered-two-crate-architecture.md) | Use Rust with a two-crate workspace, layered architecture | Accepted |
| [0003](0003-image-loading-and-caching.md) | Load images lazily with ±3 prefetch and an LRU cache | Accepted |
| [0004](0004-archive-abstraction-and-extraction.md) | Abstract page supply behind a `PageSource` trait, extract in memory | Accepted |
| [0005](0005-settings-persistence.md) | Persist settings as versioned JSON | Accepted |
| [0006](0006-reading-position-value-object.md) | Model reading position as a core value object (ReadingProgress) | Accepted |
| [0007](0007-per-book-view-overrides.md) | Per-book view overrides with a global fallback | Accepted |
| [0008](0008-fluent-i18n.md) | Adopt Fluent (.ftl) as the single i18n catalog via i18n-embed | Proposed |
| [0009](0009-reject-empty-books.md) | Validate book emptiness with a core probe at the boundaries | Accepted |

## Conventions

- File name: `NNNN-kebab-case-title.md` (sequence numbers are never reused).
- Status: `Proposed` / `Accepted` / `Deprecated` / `Superseded by ADR-XXXX`.
- To overturn a decision, do not delete the existing ADR: write a new one and set the old
  ADR's status to `Superseded by ADR-XXXX` (the decision history is kept as an immutable record).
- Prose and identifiers are in English, matching the project's code conventions.
