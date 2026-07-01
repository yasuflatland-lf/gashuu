//! Update-checker Rust glue: the startup/manual GitHub Releases check, the
//! update-available dialog's action callbacks, and the settings About-section
//! callbacks. Decision logic lives in `gashuu_core::update`; this module owns
//! the side-effecting glue (HTTP, download, verify, open/reveal).
//!
//! `!Send` constraint: `Rc<RefCell<Settings>>` is `!Send`, so it can never be
//! captured by a `rayon::spawn` closure nor a `slint::invoke_from_event_loop`
//! closure — both require `Send`. Only `slint::Weak<ViewerWindow>` and owned
//! data (`String`, `ReleaseInfo`, `Option<..>`, `bool`) may cross into those
//! closures. The fetched `ReleaseInfo` is stashed in a UI-thread `thread_local!`
//! so the dialog's action handlers (which run on the UI thread) can read it
//! without threading the settings cell through a background closure. The check
//! timestamp is recorded on the UI thread BEFORE spawning, so the background
//! path never needs the settings cell either. `reveal_download` /
//! `download_and_verify`'s rayon closures follow the same rule: they capture
//! only `weak` and the owned `Packaging`/`ReleaseInfo`, never `settings`.

use crate::update::net::{download_bytes, fetch_latest_release_json};
use crate::update::{UpdateError, CURRENT_VERSION, RELEASES_PAGE_URL};
use crate::ViewerWindow;
use gashuu_core::{
    detect_packaging, parse_latest_release, parse_sha256sums, select_asset, should_check,
    should_notify, verify, Packaging, ReleaseInfo, Settings, UpdateStrategy, CHECK_INTERVAL_SECS,
};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

thread_local! {
    /// UI-thread-only stash for the most recently fetched release, so the
    /// dialog's action handlers can read the download URLs / version without
    /// re-fetching or threading the (`!Send`) settings cell through a
    /// background closure. Only ever read/written from the UI thread.
    static LATEST: RefCell<Option<ReleaseInfo>> = const { RefCell::new(None) };
}

/// Stash the most recently fetched release for the dialog action handlers.
/// UI-thread only.
fn stash_release(info: ReleaseInfo) {
    LATEST.with(|c| *c.borrow_mut() = Some(info));
}

/// Read back the most recently fetched release (a clone) for the dialog's
/// action handlers. UI-thread only.
fn take_release() -> Option<ReleaseInfo> {
    LATEST.with(|c| c.borrow().clone())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Kick off a background update check. `force = true` bypasses the enabled
/// flag and the 24h throttle (used by the manual "Check now" button).
pub(crate) fn start_update_check(ui: &ViewerWindow, settings: &Rc<RefCell<Settings>>, force: bool) {
    let now = now_unix();
    let (enabled, last, skipped) = {
        let s = settings.borrow();
        (
            s.auto_update_check,
            s.last_update_check,
            s.skipped_version.clone(),
        )
    };
    if !force && (!enabled || !should_check(last, now, CHECK_INTERVAL_SECS)) {
        return;
    }
    // Record the attempt up-front on the UI thread so the background closures
    // never capture the !Send Rc<RefCell<Settings>>.
    {
        let mut s = settings.borrow_mut();
        s.last_update_check = Some(now);
        let _ = s.save();
    }
    let weak = ui.as_weak();
    let skipped_for_decision = if force { None } else { skipped };
    if force {
        ui.set_settings_update_status("Checking for updates…".into());
    }
    rayon::spawn(move || {
        let result = fetch_latest_release_json()
            .ok()
            .and_then(|j| parse_latest_release(&j).ok());
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            match result {
                Some(info)
                    if should_notify(
                        CURRENT_VERSION,
                        &info.version,
                        skipped_for_decision.as_deref(),
                    ) =>
                {
                    ui.set_update_current_version(CURRENT_VERSION.into());
                    ui.set_update_latest_version(info.version.clone().into());
                    ui.set_update_notes_available(!info.html_url.is_empty());
                    ui.set_update_in_progress(false);
                    ui.set_update_status_text(Default::default());
                    if force {
                        ui.set_settings_update_status(Default::default());
                    }
                    stash_release(info);
                    ui.set_show_update_available(true);
                }
                Some(_) => {
                    if force {
                        ui.set_settings_update_status("You're on the latest version.".into());
                    }
                }
                None => {
                    if force {
                        ui.set_settings_update_status("Couldn't check for updates.".into());
                    }
                }
            }
        });
    });
}

/// Wire the update-related callbacks. Called once at startup. Seeds the
/// About section's version + toggle from the current settings, then
/// registers the update-available dialog's action callbacks and the
/// settings check-now / auto-update-toggle callbacks.
pub(crate) fn wire_update_handlers(ui: &ViewerWindow, settings: &Rc<RefCell<Settings>>) {
    ui.set_settings_app_version(CURRENT_VERSION.into());
    ui.set_settings_auto_update_check(settings.borrow().auto_update_check);

    // Release notes → open the release page in the browser. No settings
    // capture needed; runs entirely on the UI thread.
    ui.on_update_notes(|| {
        let _ = opener::open(RELEASES_PAGE_URL);
    });

    // Later → the .slint already flips show-update-available = false before
    // firing this callback; nothing left to persist.
    ui.on_update_later(|| {});

    // Skip this version → persist skipped_version so it is never re-notified.
    {
        let settings = Rc::clone(settings);
        ui.on_update_skip(move || {
            if let Some(info) = take_release() {
                let mut s = settings.borrow_mut();
                s.skipped_version = Some(info.version);
                let _ = s.save();
            }
        });
    }

    // Update now → dispatch by packaging strategy. Only `weak` and the
    // stashed, owned `ReleaseInfo` cross into this closure — never `settings`.
    {
        let weak = ui.as_weak();
        ui.on_update_accept(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let Some(info) = take_release() else {
                return;
            };
            let exe = std::env::current_exe().unwrap_or_default();
            let appimage = std::env::var_os("APPIMAGE");
            let pkg = detect_packaging(&exe, appimage.as_deref());
            match pkg.strategy() {
                UpdateStrategy::OpenReleasePage => {
                    let _ = opener::open(RELEASES_PAGE_URL);
                    ui.set_show_update_available(false);
                }
                UpdateStrategy::RevealDownload => {
                    reveal_download(&ui, pkg, info);
                }
                UpdateStrategy::SelfReplace => {
                    // PR1: no in-place replace yet — fall back to the release
                    // page. A follow-up PR replaces this arm with the real
                    // self-replace pipeline for AppImage/Windows.
                    let _ = opener::open(RELEASES_PAGE_URL);
                    ui.set_show_update_available(false);
                }
            }
        });
    }

    // Settings: manual auto-update toggle persists immediately (unlike most
    // other settings, which batch-save on dialog close) so the preference
    // survives even if the app exits abnormally before the dialog is closed.
    {
        let settings = Rc::clone(settings);
        ui.on_settings_set_auto_update_check(move |v| {
            let mut s = settings.borrow_mut();
            s.auto_update_check = v;
            let _ = s.save();
        });
    }
    // Settings: "Check for updates now" — force = true bypasses the enabled
    // flag and the 24h throttle.
    {
        let weak = ui.as_weak();
        let settings = Rc::clone(settings);
        ui.on_settings_check_for_updates(move || {
            if let Some(ui) = weak.upgrade() {
                start_update_check(&ui, &settings, true);
            }
        });
    }
}

/// macOS: download the .zip, verify against SHA256SUMS, save to the Downloads
/// dir, and reveal it in Finder for a manual drag-install. Runs the download on
/// a rayon thread; marshals UI updates back. Only `weak` and owned data
/// (`Packaging`, `ReleaseInfo`) cross the thread boundary — the settings cell
/// is never captured here.
fn reveal_download(ui: &ViewerWindow, pkg: Packaging, info: ReleaseInfo) {
    ui.set_update_in_progress(true);
    ui.set_update_status_text("Downloading…".into());
    let weak = ui.as_weak();
    rayon::spawn(move || {
        let outcome = download_and_verify(pkg, &info);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            ui.set_update_in_progress(false);
            match outcome {
                Ok(path) => {
                    let _ = opener::reveal(&path);
                    ui.set_show_update_available(false);
                }
                Err(e) => {
                    ui.set_update_status_text(
                        format!("Download failed: {e}. Opening release page…").into(),
                    );
                    let _ = opener::open(RELEASES_PAGE_URL);
                }
            }
        });
    });
}

/// Download the platform asset + SHA256SUMS, verify, and write the asset into
/// the user's Downloads directory (falling back to the temp dir). Returns the
/// saved path. Shared by the macOS reveal path (PR1) and self-replace (a
/// future PR).
fn download_and_verify(pkg: Packaging, info: &ReleaseInfo) -> Result<PathBuf, UpdateError> {
    let asset = select_asset(pkg, &info.assets)
        .ok_or_else(|| UpdateError::Io("no matching release asset".into()))?;
    let sums_asset = info
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS")
        .ok_or_else(|| UpdateError::Io("no SHA256SUMS in release".into()))?;

    let bytes = download_bytes(&asset.download_url)?;
    let sums = download_bytes(&sums_asset.download_url)?;
    let sums = String::from_utf8_lossy(&sums);
    let map = parse_sha256sums(&sums);
    let expected = map
        .get(&asset.name)
        .ok_or_else(|| UpdateError::Verify(format!("{} missing from SHA256SUMS", asset.name)))?;
    if !verify(&bytes, expected) {
        return Err(UpdateError::Verify(format!(
            "checksum mismatch for {}",
            asset.name
        )));
    }

    let dir = directories_download_dir();
    let dest = dir.join(&asset.name);
    std::fs::write(&dest, &bytes).map_err(|e| UpdateError::Io(e.to_string()))?;
    Ok(dest)
}

/// The platform Downloads directory, falling back to the temp dir when it
/// cannot be resolved (e.g. a headless/minimal environment).
fn directories_download_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(|p| p.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
}
