# ADR-0011: Decoder subprocess isolation for RAR/CBR and AVIF

- Status: Proposed
- Decided: 2026-06-08
- Related: [ADR-0004](0004-archive-abstraction-and-extraction.md) (`PageSource` / `ArchiveLoader`
  / `RarSource`), [ADR-0010](0010-avif-decode-via-dav1d.md) (dav1d C binding for AVIF),
  [ADR-0002](0002-layered-two-crate-architecture.md) (core↔UI boundary at RGBA bytes +
  dimensions)

## Context

Two decode paths invoke native C code with untrusted, potentially malicious input:

**RAR/CBR (unrar / libunrar)**

- `RarSource` delegates to the `unrar` crate, which wraps the libunrar C library. libunrar
  processes the full decompressed entry bytes in-process with no streaming actual-size cap
  equivalent to the `take(MAX_ENTRY_BYTES + 1)` guard used by `ZipSource`.
- A crafted RAR file can cause unbounded memory allocation, infinite loops, or native crashes
  before the application-level size guard can fire.
- Safe Mode (ADR-0011 phase 1, implemented in issue #175) can disable RAR at load time, but
  books already in the library still expose the RAR path unless the user explicitly removes them.

**AVIF (dav1d via dav1d-sys)**

- `image_ops::decode` dispatches to dav1d through the `image` crate's `avif-native` feature.
  Unlike PNG/JPEG, AVIF has no cheap header-only dimension pre-read path; the full AV1 bitstream
  must be decoded before the pixel-count guard in `check_pixel_limit` fires.
- A crafted AVIF bitstream can consume arbitrary CPU and memory in the dav1d C library before
  the application can interrupt it.
- The `MAX_PIXELS` bomb guard (ADR-0010) is a best-effort post-decode check, not a pre-decode
  resource cap.

Both decoders have a larger attack surface than the pure-Rust ZIP/PNG/JPEG paths, even in a
local-only viewer where the user selects files themselves. Local files should be treated as
untrusted input; a malicious file dropped into the user's comics directory (e.g. via another
application or a sync service) could trigger the decoder on the next library refresh.

## Decision

Move RAR/CBR archive extraction and AVIF image decoding into a dedicated helper process
(`gashuu-decode-worker`) in a future implementation. The main UI process remains responsible
for orchestration, settings, cache, library management, and display. The helper process
performs only the decode/extract operation and returns either the resulting RGBA bytes plus
metadata, or a typed error.

This follows the principle that native C decoders processing untrusted data belong in an
isolated, resource-limited process that can be killed if it misbehaves, rather than in the
main UI process where a crash would lose user state.

## Process boundary

### Helper binary

Name: `gashuu-decode-worker`  
Distribution: bundled alongside the main binary (same release artifact, not a separate
user-installed dependency).

### Inputs (main → worker)

| Field | Type | Notes |
|---|---|---|
| `operation` | enum | `ExtractRarEntry` or `DecodeAvif` |
| `source_path` | string | absolute, canonicalized path |
| `entry_identity` | string | archive entry name for RAR; empty for AVIF |
| `max_input_bytes` | u64 | ceiling on bytes read from source |
| `max_pixels` | u64 | ceiling on decoded width × height |
| `max_output_bytes` | u64 | ceiling on RGBA bytes returned |
| `timeout_ms` | u32 | wall-clock limit for the entire operation |

### Outputs (worker → main)

**Success:**

| Field | Type | Notes |
|---|---|---|
| `width` | u32 | decoded image width in pixels |
| `height` | u32 | decoded image height in pixels |
| `rgba_bytes` | bytes | raw RGBA8 buffer (`width × height × 4` bytes) |

**Failure (typed):**

| Code | Meaning |
|---|---|
| `EntryTooLarge` | source exceeded `max_input_bytes` |
| `ImageTooLarge` | decoded pixels exceeded `max_pixels` |
| `OutputTooLarge` | RGBA buffer exceeded `max_output_bytes` |
| `DecodeFailed` | malformed archive or image data |
| `Timeout` | operation exceeded `timeout_ms` |
| `Crash` | worker exited abnormally (detected by main on stdin/stdout close) |

### IPC approach: length-prefixed stdin/stdout (preferred)

Main spawns the worker with `std::process::Command`, writes a length-prefixed request frame
to the worker's stdin, and reads a length-prefixed response frame from the worker's stdout.
Frame format: 4-byte big-endian length header followed by a MessagePack or CBOR payload.

**Rationale over alternatives:**

- *Temp-file payloads*: avoided because temp-file creation races (TOCTOU) and cleanup
  failures complicate error handling. Length-prefixed stdin/stdout is simpler and keeps
  the data path in memory.
- *Unix domain sockets / named pipes*: not necessary for a single request/response per
  spawn; sockets add lifecycle complexity without benefit.
- *JSON*: excluded for the output path because RGBA bytes require base64 encoding, tripling
  the payload size. MessagePack or CBOR encodes bytes natively and is compact.

## Resource limits

The following limits are applied by the main process at spawn time and enforced by the helper
process itself before returning:

| Resource | Limit | Enforcement point |
|---|---|---|
| Wall-clock time | 30 s per operation | `tokio::time::timeout` or `std::thread` + `join` with timeout |
| Input bytes | `MAX_ENTRY_BYTES` (500 MiB, shared constant) | helper reads at most this many bytes |
| Decoded pixels | `MAX_PIXELS` (existing constant) | helper checks width × height before returning RGBA |
| Output bytes | `width × height × 4` (derived, no separate cap needed) | validated by main on receipt |
| Memory (best-effort) | OS-dependent (see below) | set before the decode call |

On timeout or crash, the main process kills the worker (if still running) via `Child::kill()`
and returns `CoreError::Timeout` or `CoreError::WorkerCrash` to the caller.

## OS-specific isolation notes

**Linux**

- `rlimit` via the `nix` crate: `RLIMIT_AS` (virtual memory) and `RLIMIT_CPU` (CPU seconds)
  set in the worker process immediately after `fork`, before the decode call.
- cgroups v2: optional future enhancement; attach the worker's PID to a memory-limited cgroup
  if available. Not required for phase 2.
- seccomp: optional future hardening; restrict the worker to a minimal syscall allowlist
  (`read`, `write`, `mmap`, `munmap`, `exit_group`). Not required for phase 2.

**macOS**

- `rlimit` via `libc::setrlimit`: `RLIMIT_AS` and `RLIMIT_CPU` available and recommended.
- `sandbox_init` / Seatbelt: macOS App Sandbox is not available to non-app-bundle binaries;
  `sandbox_init(3)` is available but deprecated and may be removed in a future OS release.
  Do not rely on it for phase 2. Harden runtime compatibility (`ENABLE_HARDENED_RUNTIME`)
  is required for notarization but does not itself restrict the worker's syscalls.
- macOS 14+ `Sandbox.framework` private API: not suitable for distribution.
- Recommended approach for macOS: `rlimit` only in phase 2; revisit when a stable replacement
  for `sandbox_init` is available.

**Windows**

- Job Object: create a Job Object, assign the worker process to it, and set
  `JobObjectExtendedLimitInformation.BasicLimitInformation.JobMemoryLimit` and
  `PeakProcessMemoryUsed`. Use `AssignProcessToJobObject` immediately after `CreateProcess`.
- CPU time limit: set `PerJobUserTimeLimit` in `JOBOBJECT_BASIC_LIMIT_INFORMATION`.
- The `windows` or `windows-sys` crate provides the relevant bindings without a separate
  native dependency.

## Migration plan

| Phase | Description | Prerequisite |
|---|---|---|
| 1 (done) | Safe Mode (`allow_rar_archives`) toggle disables RAR at open and import time; cover-loader gap documented (issue #175). **Default reversed to `true` on 2026-06-21 (see "Amendment" below)** — the toggle remains as an opt-out, but RAR/CBR now opens out of the box; the native-decoder risk above is accepted by default until this roadmap's later phases land. | issue #175 merged |
| 2 | Add `gashuu-decode-worker` binary crate and the IPC protocol behind a hidden/dev setting (`use_decode_worker = false` by default). Route RAR through the worker when enabled. All gates green; no behavior change at default settings. | — |
| 3 | Route AVIF through the worker in the same binary. Extend the IPC protocol to handle AVIF's lack of a cheap pre-decode dimension check. | Phase 2 stable |
| 4 | Enable `use_decode_worker = true` by default. Keep direct in-process decode as a fallback (toggled by the setting) for at least one release cycle to allow regression reports. Remove the fallback once the worker path is stable across platforms. | Phase 3 stable + one release |

## Testing plan

The following tests are required before the worker path is enabled by default (phase 4):

- **Malformed RAR archive**: worker returns `DecodeFailed`; main does not crash or hang.
- **RAR entry with declared size < actual size** (spoofed header): `max_input_bytes` cap fires
  in the worker, returning `EntryTooLarge` before the full decompressed content is read.
- **Huge AVIF** (pixel bomb): worker returns `ImageTooLarge` before RGBA bytes are produced.
- **Worker timeout** (slow-decode loop in a crafted AVIF): main kills the worker after
  `timeout_ms`, returns `CoreError::Timeout` to the caller; next operation works normally.
- **Worker crash** (SIGSEGV / abnormal exit): main detects the closed stdout, returns
  `CoreError::WorkerCrash`; main process continues.
- **Malformed IPC response** (truncated length header): main returns a parse error; no
  unbounded read.
- **Cancellation while worker is running**: main sends SIGTERM/`TerminateProcess`, waits for
  at most 1 s, then forces `SIGKILL`/`TerminateProcess`; resources are released.
- **Platform-specific resource-limit failure** (worker exceeds `RLIMIT_AS`): OS kills the
  worker; main handles the crash path and returns `CoreError::WorkerCrash`.
- **Regression**: existing ZIP/CBZ and PNG/JPEG decode paths are unchanged and continue to
  run in-process; no performance regression on warm cache.

## Non-goals

This ADR does not:

- Implement the `gashuu-decode-worker` binary or the IPC protocol.
- Implement macOS notarization or Windows code signing for the worker binary.
- Change ZIP/CBZ archive extraction behavior.
- Change the current decoded-image cache, thumbnail cache, or settings UI beyond the Safe Mode
  toggle already shipped in issue #175.
- Define the exact wire format (MessagePack vs CBOR) — that is deferred to the phase 2
  implementation PR.

## Amendment 2026-06-21: RAR/CBR enabled by default

Phase 1's `allow_rar_archives = false` default is reversed: the setting now defaults to `true`.
The toggle itself is unchanged and remains an **opt-out**; only the default flips.

**Why.** CBR/RAR is a primary manga distribution format, `RarSource` has extracted it in-process
since issue #22, and the product already advertises support for it (`site/index.html` lists
"CBZ/ZIP/CBR/RAR"). Yet Safe Mode left the format unreachable out of the box — a supported,
advertised format silently returning `CoreError::FormatDisabled` until the user found a hidden
toggle. For a local-only viewer where the user explicitly picks the files to open, that friction
outweighs the marginal protection of a default-off toggle that any motivated user disables anyway.
The durable mitigation for the native-decoder risk is the subprocess isolation in this ADR's phases
2–4, not a default that hides a working feature.

**What changed (settings only — extraction, `ArchivePolicy`, the toggle UI, and the error types are
untouched).**

- `Settings::default()` returns `allow_rar_archives: true`, and the field uses
  `#[serde(default = "default_allow_rar")]` (a named function returning `true`) instead of the bare
  `#[serde(default)]`, which would resolve a missing field to `bool::default()` (`false`). This
  mirrors the existing `default_cache_size` / `default_preload_pages` pattern so both fresh installs
  AND settings files written before the field existed adopt the new default.
- **A user's explicit `false` is preserved**: once a settings file records `allow_rar_archives:
  false` it round-trips unchanged (covered by a new `allow_rar_archives_explicit_false_round_trips`
  test). In practice the new default reaches fresh installs and pre-field files; an existing install
  that has ever saved settings already baked in the then-current `false` and keeps RAR disabled until
  the user toggles it on.

**Risk accepted.** The native-decoder attack surface analyzed above is now exposed by default. A
crafted RAR can still trigger unbounded allocation, an infinite loop, or a native crash in libunrar
before the application-level guards fire. This is a deliberate, documented acceptance for a local
viewer; phases 2–4 remain the path to removing it.

**Cover-loader gap, unchanged but lower-impact.** The `cover_loader.rs` `TODO(#175-followup)` still
uses the policy-less `ArchiveLoader::open` (effectively `allow_rar: true`). With the default now
`true` that matches the default policy, so the inconsistency only persists for a user who has
explicitly opted OUT (their already-added RAR books' covers still load). Closing it stays a
follow-up.
