# ADR-0013: In-app auto-update via GitHub Releases, hybrid packaging-aware action

- Status: Accepted
- Decided: 2026-07-01
- Related: [ADR-0002](0002-layered-two-crate-architecture.md) (the core↔UI boundary — this feature's
  pure decision logic lives in `gashuu-core`, all I/O in `gashuu`), [ADR-0005](0005-settings-persistence.md)
  (the versioned-JSON `Settings` this feature adds three backward-compatible fields to)
- Spec / brainstorm: `.claude/plans/auto_update_design.md`, `.claude/plans/auto_update_plan.md`

## Context

gashuu ships unsigned, portable artifacts with no update mechanism — a user on an old version has
no in-app signal that a newer release exists, and no path to it besides remembering to check GitHub.
The distribution forms differ enough per platform that a single update mechanism cannot treat them
uniformly:

- **macOS**: a `.app` bundle, ad-hoc self-signed (`codesign --sign -`, see the macOS ad-hoc signing
  harness in memory), **not notarized**. A downloaded, swapped `.app` picks up the
  `com.apple.quarantine` extended attribute, and a freshly-swapped ad-hoc-signed bundle frequently
  fails Gatekeeper's re-check on relaunch ("gashuu is damaged and can't be opened") — in-place
  self-replace is unsafe here.
- **Windows**: a single portable `gashuu.exe`, no installer, unsigned. In-place self-replace is
  feasible via the standard "rename the running exe, write the new one, relaunch" trick.
- **Linux**: two artifacts. A `.deb` installs to `/usr/bin` and is apt/dpkg-managed — replacing it
  in-place needs root and fighting the package manager is unsafe. An `.AppImage` is a single
  portable file at a path exposed via `$APPIMAGE` — safe to self-replace.

A single "always self-replace" strategy is therefore unsafe on macOS and on the deb form; a single
"never self-replace, always open the browser" strategy leaves the two genuinely safe forms
(AppImage, Windows portable) with an unnecessarily manual update. Full silent auto-update (download
+ install with no consent, on every platform) is rejected outright: on the safe forms it removes the
user's ability to defer an update mid-session, and on macOS/deb it cannot be made safe without
notarization (Apple) and Authenticode (Microsoft) — both a signing-certificate cost and a build/CI
change that is out of scope for this decision.

The layered architecture (ADR-0002) is a hard constraint: `gashuu-core` stays headless (no `slint`,
no `tracing`, and — per this feature's added rule — no networking either). Everything this feature
needs to *decide* (is a version newer, which packaging form is this, which asset matches it, does a
downloaded byte string match its checksum) is pure and can be unit-tested without a network or a
filesystem; everything it needs to *do* (HTTP fetch, download, replace the running binary, open a
browser, reveal a file in Finder/Explorer) is I/O and belongs in the `gashuu` UI crate.

## Decision

An in-app updater that checks GitHub Releases on startup (throttled to once per 24h) and on demand,
notifies the user via a modal, and — on confirmation — takes a **hybrid, packaging-aware update
action**: self-replace on the two safe forms, a reliable guided/manual path everywhere else.

1. **Check timing.** On startup, `handlers::start_update_check` is called directly, right before
   `ui.run()`; its UI mutations only take effect once the Slint event loop is running, because they
   are queued via `slint::invoke_from_event_loop` (the background fetch dispatches its callback
   through that queue rather than touching the UI synchronously) — no `Timer::single_shot` idiom is
   involved here (that idiom is used elsewhere, e.g. window-geometry-persistence, per memory).
   gashuu-core's `should_check(last_check, now, CHECK_INTERVAL_SECS)` gates an
   automatic check to once per 24h (`CHECK_INTERVAL_SECS = 24 * 60 * 60`), and only if
   `Settings.auto_update_check` is enabled. A manual "Check for updates now" button in the Settings
   dialog's About section bypasses both the toggle and the throttle (`force = true`).
2. **Detection is pure core logic.** `gashuu_core::update::version::is_update_available(current,
   latest)` parses both strings as semver (tolerating a leading `v`/`V`), returns
   `latest > current`; `should_notify` additionally suppresses a version the user chose to skip
   (`Settings.skipped_version`). `release::parse_latest_release` parses the GitHub
   `releases/latest` JSON payload into a `ReleaseInfo { tag, version, html_url, body, assets }`.
   `releases/latest` (used, not `releases`) automatically excludes prereleases and drafts.
3. **Notification is a modal, not a passive badge.** `UpdateAvailableDialog.slint` (`GlassModal`-based,
   mirroring `ConfirmDialog`'s modal-key-trap idiom but extended to three stops) presents
   `Current vX → Latest vY`, a release-notes link, and three actions: **Update now**, **Later**
   (the default-focused, safe choice — also bound to Esc and Return so a reflexive keypress can
   never fire an update), and **Skip this version** (persists `skipped_version` so that release is
   never re-notified).
4. **Packaging detection drives the post-confirmation action — the hybrid core.**
   `gashuu_core::update::packaging::detect_packaging(exe_path, appimage_env)` classifies the running
   build from `std::env::current_exe()` and `$APPIMAGE` alone (no `cfg!(target_os)`, so every branch
   is unit-testable on any host): `$APPIMAGE` set → `LinuxAppImage`; exe path contains
   `.app/Contents/MacOS/` → `MacOsApp`; exe path ends in `.exe` → `WindowsPortable`; exe path starts
   with `/usr/` → `LinuxDeb`; anything else (e.g. `cargo run`'s `target/debug/gashuu`) → `Unknown`.
   `Packaging::strategy()` maps each form to an `UpdateStrategy`:
   - `SelfReplace` (Linux AppImage, Windows portable) — download the platform asset, verify it, and
     replace the running binary in place, then relaunch.
   - `RevealDownload` (macOS `.app`) — download + verify, then reveal the saved file in Finder for a
     manual drag-into-`/Applications` install (in-place replace is unsafe here per the Context).
   - `OpenReleasePage` (Linux deb, Unknown) — just open the GitHub release page in the browser; deb
     defers to apt/dpkg (never fights the package manager), and an unknown/dev build has no
     sensible self-update target.
   The **consent dialog is shown for every form** — "Update now" always requires an explicit click
   (or Tab→Space); only the action *behind* that click differs by packaging.
5. **Every download is verified before use.** The release's `SHA256SUMS` asset is parsed
   (`verify::parse_sha256sums`, accepting both the coreutils `"<hex>  <name>"` and binary-marker
   `"<hex> *<name>"` formats) and the downloaded asset's SHA-256 (`verify::sha256_hex`, `sha2` crate)
   is compared against it (`verify::verify`) before the bytes are written to disk or a replace is
   attempted. A checksum mismatch aborts with an error and falls back to opening the release page —
   never a half-verified install.
6. **Unauthenticated GitHub API.** The `releases/latest` endpoint is called without a token; GitHub's
   unauthenticated rate limit (60 requests/hour/IP) is comfortably inside the 24h automatic-check
   throttle, and a manual "Check now" click is a human-paced action, not a loop.
7. **Two shipping stages, same design.** PR1 ships the full detection/notify/consent flow and the
   guided paths (`RevealDownload`, `OpenReleasePage`) for every packaging form, including
   `SelfReplace` forms — which, until PR2 lands, fall back to `OpenReleasePage` rather than
   replacing anything. PR2 replaces that fallback arm with the real self-replace pipeline
   (download → extract → verify → atomic replace → relaunch) for Linux AppImage and Windows
   portable. Splitting this way keeps each PR under the ~1000 production LOC guideline and lets the
   lower-risk notify/guided-download path ship and be used before the higher-risk in-place binary
   replacement is added.

## Alternatives considered

- **Full silent auto-update on every platform (rejected).** Update-and-relaunch with no consent
  step. Rejected because it is unsafe under the current unsigned packaging: on macOS a swapped,
  ad-hoc-signed `.app` risks the quarantine/Gatekeeper "damaged" failure with no user step to
  recover from, and on Linux deb an unattended replace would need root and would fight dpkg's
  ownership of `/usr/bin`. Making it safe would require macOS notarization and Windows Authenticode
  signing — a real cost (paid certificates, CI changes) that is out of scope for this decision.
- **Uniform guided-download-only (rejected).** Always just open the release page, never self-replace.
  Simple and safe everywhere, but needlessly manual on the two forms (AppImage, Windows portable)
  where an in-place replace genuinely is safe — rejected in favor of the hybrid so the safe forms get
  a one-click update.
- **The `self_update` crate (rejected).** Targets a single loose binary; has no concept of a `.app`
  bundle or a `.deb`-managed install, so it does not fit the hybrid, packaging-aware strategy without
  being fought at every branch. A minimal hand-rolled flow plus the `self_replace` crate (PR2, for
  the atomic-replace primitive only) gives full control over the per-platform branching this decision
  needs.

## Consequences

### Positive

- Users are notified of new releases without leaving the app, on all three platforms, on a schedule
  that respects GitHub's rate limit and the user's control (toggle + skip-version).
- The two packaging forms that can safely self-update (AppImage, Windows portable) get a one-click
  in-place replace + relaunch; the two that cannot (macOS `.app`, deb) get a safe, reliable fallback
  instead of a broken or fought-with-the-OS auto-replace.
- Every decision (newer-version, which packaging, which asset, checksum match) is pure and
  nextest-covered; only the side-effecting glue (HTTP, download, replace, open/reveal) needs manual
  per-OS verification, which keeps the CI-provable surface as large as possible.
- `gashuu-core` stays headless: no networking, no TLS, no filesystem access was added to it — only
  `semver` (comparison) and `sha2` (hashing), both pure and light.

### Costs / trade-offs accepted

- **First networking/TLS dependency in the workspace.** `ureq` (with its default rustls backend, no
  OpenSSL) is the first HTTP client gashuu has ever depended on, and lives entirely in the `gashuu`
  UI crate. `opener` (open URLs / reveal files) is also new. The self-replace step adds the
  `self-replace` crate (atomic in-place binary replacement, imported as `self_replace`); `zip` was
  already a workspace dependency (CBZ/ZIP archive support, ADR-0004) and is reused to extract the
  downloaded Windows release zip rather than adding a second zip crate.
- **Self-replace is per-form and not CI-verifiable.** `UpdateStrategy::SelfReplace` is wired
  end-to-end (`on_update_accept` → `self_replace_update` → `prepare_self_replace` →
  `apply_self_replace` / `relaunch_and_exit`), but the two forms replace differently and neither can
  run on the CI/macOS-dev host: Windows uses the `self-replace` crate to swap `gashuu.exe`; AppImage
  replaces the `$APPIMAGE` file (the running exe is a read-only mounted squashfs) via a staged
  sibling `<name>.new` + `chmod +x` + atomic rename. Both are covered by unit tests (zip extraction)
  plus the gates and hands-on per-OS runs; on any failure the handler falls back to opening the
  release page, so a broken self-replace degrades to the guided path rather than stranding the user.
  See `docs/patterns.md`.
- **Unauthenticated GitHub API is a soft dependency on the 24h throttle holding.** If the throttle
  were ever bypassed or removed, unauthenticated calls could hit the 60 req/h/IP limit under heavy
  manual "Check now" use; accepted because the throttle is enforced in the same pure function
  (`should_check`) that gates the only automatic caller.
- **No delta updates, no update channels (beta/stable), no rollback.** Every update is a full-asset
  download of the latest full release; explicitly out of scope (YAGNI) alongside code
  signing/notarization.

## Implementation notes (as-built deltas)

- **Module layout matches the design 1:1.** `gashuu-core/src/update/{version,release,packaging,asset,
  check,verify}.rs` hold the pure decision logic; `gashuu/src/update/{mod,net}.rs` hold
  `fetch_latest_release_json` / `download_bytes` (blocking `ureq`, always called from a `rayon::spawn`
  thread, never the UI thread); `gashuu/src/handlers/update.rs` owns the Slint callback wiring
  (startup check, dialog actions, Settings About-section callbacks).
- **`Settings` gained three `#[serde(default)]` fields** (ADR-0005's versioned-JSON persistence,
  unchanged `SETTINGS_VERSION` — fully backward-compatible): `auto_update_check: bool` (default
  `true`), `skipped_version: Option<String>`, and `last_update_check: Option<i64>` (UNIX seconds).
- **The `!Send` boundary is bridged by a UI-thread-only stash, not by threading `Settings` into
  background closures.** `Rc<RefCell<Settings>>` is `!Send` and cannot cross into a `rayon::spawn`
  or `slint::invoke_from_event_loop` closure; the fetched `ReleaseInfo` is kept in a
  `thread_local!` `LATEST` cell on the UI thread, and the check timestamp is recorded on the UI
  thread *before* spawning the background fetch, so no background closure ever needs to capture the
  settings cell.
- **A Slint gotcha specific to this dialog is recorded in `docs/patterns.md`**: `UpdateAvailableDialog`
  declares a public `accept()` callback, which makes the bare `accept`/`reject` identifiers inside its
  `FocusScope.key-pressed` handler ambiguous with the `EventResult` enum variants of the same name;
  the fully qualified `EventResult.accept` / `EventResult.reject` is required there (see the patterns.md
  entry for the full explanation and the "Callback must be called" compiler error it otherwise
  produces).
