# ADR-0004: Abstract page supply behind a `PageSource` trait, extract in memory

- Status: Accepted
- Decided: 2026-05-31 (transcribed: 2026-06-01)
- Related: [ADR-0002](0002-layered-two-crate-architecture.md) (core layering), [ADR-0003](0003-image-loading-and-caching.md) (prefetch)

## Context

Phase 1 ships folder browsing, but CBZ/ZIP (Phase 2) and CBR/RAR (Phase 4) must be addable without
reworking the abstraction. The choice of abstraction and extraction strategy also drives several
security and concurrency properties:

- **Extensibility** — adding a new container format should be a closed change.
- **Parallel prefetch** — `ImageCache` ([ADR-0003](0003-image-loading-and-caching.md)) prefetches on
  `rayon`, so the page source must be shareable across threads and ideally lock-free.
- **Security** — zip-slip path traversal, decompression bombs, and oversized entries must be
  defended.

## Decision

- Abstract page supply behind a **`PageSource` trait** (`Send + Sync`, so `Arc<dyn PageSource>` can
  be shared with rayon workers). Implementations: `FolderSource`, `ZipSource`, `RarSource`.
- Dispatch via `ArchiveLoader::open(path) -> Arc<dyn PageSource>`: directory → `FolderSource`;
  otherwise resolve a `Kind {Zip, Rar}` by extension (`ext_kind`, no I/O), falling back to a bounded
  magic-byte sniff (`magic_kind`), else `CoreError::UnsupportedFormat`.
- **Extract in memory** (`Vec<u8>` per entry), not to temp files — saves I/O and spares SSD write
  wear (to be re-evaluated in a future version).
- Defend the archive surface:
  - **zip-slip**: reject entries whose path escapes the root (`enclosed_name`); image-looking
    traversal entries are skip-and-counted (surfaced as "skipped N"), the container itself is not
    failed.
  - **Oversized entries**: a shared 500 MB per-entry ceiling (`MAX_ENTRY_BYTES` in `naming.rs`),
    enforced at open time (declared size) and at read time.
  - **Image bombs**: handled at decode (`check_pixel_limit` + `image::Limits`,
    [ADR-0003](0003-image-loading-and-caching.md)).

## Alternatives considered

- **`enum` dispatch instead of a trait** — fewer moving parts, but each new format edits every match
  site (open vs closed). Chose the trait to favor extensibility.
- **Temp-file extraction** — simpler streaming and lower peak RAM for huge entries, but extra I/O,
  SSD write wear, and a zip-slip *write* hazard. Chose in-memory extraction; in-memory means no disk
  write, so there is no zip-slip write hazard at all.

## Consequences

### Positive
- Adding a format is a closed change: implement `PageSource` + extend `ArchiveLoader` dispatch.
- Sources are **lock-free via reopen-per-read**: each `read_bytes` opens its own handle, so rayon
  prefetch threads decompress fully in parallel with no shared mutable state; resident RAM is one
  entry per in-flight read.
- Skipped/corrupt entries are surfaced (`last_open_skipped()` → status bar) rather than silently
  dropped; a fundamentally broken container hard-fails.

### Costs / trade-offs accepted
- Reopen-per-read trades reopen cost for parallelism (a shared `Mutex<archive>` would serialize
  prefetch; a whole-archive buffer would pin ~1 GB resident).
- RAR has no random access: `RarSource` walks entries front-to-back (`seq_index`), O(N) skip per
  read — cheap on a non-solid CBR, but a solid archive pays decompression per skip (accepted;
  cursor-cache deferred).

## Implementation notes (as-built deltas)

- **Synchronous crates over rayon, not async (intentional deviation).** The design doc specified
  `async_zip` + a `tokio` runtime; the as-built uses the **synchronous `zip`** crate
  (`{ version = "8", default-features = false, features = ["deflate"] }`) and the **synchronous
  `unrar` 0.5** over the existing rayon pool. A synchronous `read_bytes` + CPU-bound decode fit
  rayon naturally; async would force a `block_on` bridge and infect every layer with `tokio`. The
  `deflate`-only `zip` keeps the cross-platform build free of native C deps.
- **RAR requires a C++ compiler on all three OS** — `unrar` bundles C++ UnRAR built via `cc`. This
  is a knowing exception to the "no native toolchain" stance (RAR has no pure-Rust decoder;
  extraction only). The RARLAB non-free license clause is recorded in `THIRD_PARTY_LICENSES.md`.
- **`Arc<dyn PageSource>`, not `Box`** — the `Send + Sync` supertrait plus rayon sharing and
  `ArchiveLoader` returning a shareable handle make `Arc` the right type (the design doc said `Box`).
- **RAR's read-time size cap is weaker** than ZIP's: `unrar`'s `read()` materializes the whole entry
  with no streaming `take`, so it only re-validates the declared size; `image::Limits` is the final
  backstop. Documented at the call site.
