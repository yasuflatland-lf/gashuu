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

use crate::update::install::{apply_self_replace, extract_exe_from_zip, relaunch_and_exit};
use crate::update::net::{download_bytes, fetch_latest_release_json};
use crate::update::{UpdateError, CURRENT_VERSION, RELEASES_PAGE_URL};
use crate::{Strings, ViewerWindow};
use gashuu_core::{
    detect_packaging, is_verified, parse_latest_release, parse_sha256sums, select_asset,
    should_check, should_notify, Packaging, ReleaseInfo, Settings, UpdateStrategy,
    CHECK_INTERVAL_SECS,
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
fn latest_release() -> Option<ReleaseInfo> {
    LATEST.with(|c| c.borrow().clone())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Restore keyboard focus to whichever screen is underneath a just-dismissed
/// update dialog. Mirrors the screen-aware focus restore in
/// `handlers/settings.rs` (`on_close_settings`): screen 0 = Library, so focus
/// the carousel; screen 1 = Viewer, so focus the page area. Without this,
/// dismissing the dialog leaves no focused element and every key is dead until
/// the user clicks (issue #359). UI-thread only.
fn restore_focus_after_dialog(ui: &ViewerWindow) {
    if ui.get_screen() == 0 {
        ui.invoke_focus_carousel();
    } else {
        ui.invoke_focus_pages();
    }
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
        if let Err(e) = s.save() {
            tracing::warn!(error = %e, "failed to persist last update check timestamp");
        }
    }
    let weak = ui.as_weak();
    let skipped_for_decision = if force { None } else { skipped };
    if force {
        ui.set_settings_update_status(ui.global::<Strings>().get_update_status_checking());
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
                    ui.set_update_notes_available(!info.release_page_url.is_empty());
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
                        ui.set_settings_update_status(
                            ui.global::<Strings>().get_update_status_latest(),
                        );
                    }
                }
                None => {
                    if force {
                        ui.set_settings_update_status(
                            ui.global::<Strings>().get_update_status_failed(),
                        );
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
        if let Err(e) = opener::open(RELEASES_PAGE_URL) {
            tracing::warn!(error = %e, "failed to open release notes page");
        }
    });

    // Later → the .slint already flipped show-update-available=false; nothing to persist.
    // Restore keyboard focus to the screen underneath or key input is dead (issue #359).
    {
        let weak = ui.as_weak();
        ui.on_update_later(move || {
            if let Some(ui) = weak.upgrade() {
                restore_focus_after_dialog(&ui);
            }
        });
    }

    // Skip this version → persist skipped_version so it is never re-notified. Restore
    // keyboard focus to the screen underneath the dismissed dialog (issue #359).
    {
        let weak = ui.as_weak();
        let settings = Rc::clone(settings);
        ui.on_update_skip(move || {
            if let Some(info) = latest_release() {
                let mut s = settings.borrow_mut();
                s.skipped_version = Some(info.version);
                if let Err(e) = s.save() {
                    tracing::warn!(error = %e, "failed to persist skipped update version");
                }
            }
            if let Some(ui) = weak.upgrade() {
                restore_focus_after_dialog(&ui);
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
            let Some(info) = latest_release() else {
                return;
            };
            let exe = std::env::current_exe().unwrap_or_default();
            let appimage = std::env::var_os("APPIMAGE");
            let pkg = detect_packaging(&exe, appimage.as_deref());
            match pkg.strategy() {
                UpdateStrategy::ExternalInstall => {
                    if let Err(e) = opener::open(RELEASES_PAGE_URL) {
                        tracing::warn!(error = %e, "failed to open release page");
                    }
                    // This branch dismisses the dialog (ManualInstall/SelfReplace keep it
                    // open), so restore focus to the screen underneath (issue #359).
                    ui.set_show_update_available(false);
                    restore_focus_after_dialog(&ui);
                }
                UpdateStrategy::ManualInstall => {
                    reveal_download(&ui, pkg, info);
                }
                UpdateStrategy::SelfReplace => {
                    self_replace_update(&ui, pkg, info);
                }
            }
        });
    }

    // Auto-update toggle persists immediately (unlike other settings, which batch-save
    // on dialog close) so the preference survives an abnormal exit.
    {
        let settings = Rc::clone(settings);
        ui.on_settings_set_auto_update_check(move |v| {
            let mut s = settings.borrow_mut();
            s.auto_update_check = v;
            if let Err(e) = s.save() {
                tracing::warn!(error = %e, "failed to persist auto-update toggle");
            }
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

/// Shared setup for a download-driven update action (macOS reveal, self-replace):
/// mark the dialog busy with a localized "downloading" status and hand back a
/// weak handle for the background closure to marshal UI updates through. This does
/// NOT start the download — the caller spawns that; here it only flips the UI into
/// the in-progress state.
fn mark_download_in_progress(ui: &ViewerWindow) -> slint::Weak<ViewerWindow> {
    ui.set_update_in_progress(true);
    ui.set_update_status_text(ui.global::<Strings>().get_update_status_downloading());
    ui.as_weak()
}

/// Shared failure handling for a download-driven update action: clear the busy
/// flag, log `context`, surface the localized "download failed" status, and fall
/// back to opening the release page so the user is never stranded on a broken
/// update. Runs on the UI thread.
fn report_failure_and_open_release(ui: &ViewerWindow, context: &str, error: &UpdateError) {
    ui.set_update_in_progress(false);
    tracing::warn!(error = %error, "{context}");
    ui.set_update_status_text(ui.global::<Strings>().get_update_status_download_failed());
    if let Err(e) = opener::open(RELEASES_PAGE_URL) {
        tracing::warn!(error = %e, "failed to open release page");
    }
}

/// macOS: download the .zip, verify against SHA256SUMS, save to the Downloads
/// dir, and reveal it in Finder for a manual drag-install. Runs the download on
/// a rayon thread; marshals UI updates back. Only `weak` and owned data
/// (`Packaging`, `ReleaseInfo`) cross the thread boundary — the settings cell
/// is never captured here.
fn reveal_download(ui: &ViewerWindow, pkg: Packaging, info: ReleaseInfo) {
    let weak = mark_download_in_progress(ui);
    rayon::spawn(move || {
        let outcome = download_and_verify(pkg, &info);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            match outcome {
                Ok(path) => {
                    ui.set_update_in_progress(false);
                    if let Err(e) = opener::reveal(&path) {
                        tracing::warn!(error = %e, "failed to reveal downloaded update in file manager");
                    }
                    ui.set_show_update_available(false);
                }
                Err(e) => report_failure_and_open_release(&ui, "update download failed", &e),
            }
        });
    });
}

/// Linux AppImage / Windows portable: download + verify the new artifact,
/// replace the running binary in place, and relaunch. Runs the download and
/// replace on a rayon thread; marshals UI updates back. Only `weak` and owned
/// data (`Packaging`, `ReleaseInfo`) cross the thread boundary — the settings
/// cell is never captured here. On any failure the dialog surfaces the error
/// and falls back to opening the release page for a manual download.
fn self_replace_update(ui: &ViewerWindow, pkg: Packaging, info: ReleaseInfo) {
    let weak = mark_download_in_progress(ui);
    rayon::spawn(move || {
        let outcome = prepare_self_replace(pkg, &info);
        let _ = slint::invoke_from_event_loop(move || match outcome {
            Ok(exe) => {
                if let Some(ui) = weak.upgrade() {
                    ui.set_update_status_text(
                        ui.global::<Strings>().get_update_status_restarting(),
                    );
                }
                // Let the "restarting" note paint, then relaunch + exit.
                slint::Timer::single_shot(std::time::Duration::from_millis(900), move || {
                    relaunch_and_exit(&exe);
                });
            }
            Err(e) => {
                if let Some(ui) = weak.upgrade() {
                    report_failure_and_open_release(&ui, "self-update failed", &e);
                }
            }
        });
    });
}

/// Download + verify the platform artifact, then replace the running binary in
/// place, returning the executable path to relaunch. For Windows the verified
/// download is the release `.zip` (SHA256SUMS covers the zip, so it is already
/// verified by `download_and_verify`); the `.exe` is extracted from that
/// verified archive before the swap. For AppImage the verified download is the
/// `.AppImage` itself. Runs entirely on a background thread.
fn prepare_self_replace(pkg: Packaging, info: &ReleaseInfo) -> Result<PathBuf, UpdateError> {
    let verified = download_and_verify(pkg, info)?;
    let to_apply = match pkg {
        Packaging::WindowsPortable => {
            let zip_bytes = std::fs::read(&verified).map_err(|e| UpdateError::Io(e.to_string()))?;
            extract_exe_from_zip(&zip_bytes, &std::env::temp_dir())?
        }
        _ => verified,
    };
    apply_self_replace(pkg, &to_apply)
}

/// Download the platform asset + SHA256SUMS, verify, and write the asset into
/// the user's Downloads directory (falling back to the temp dir). Returns the
/// saved path. Shared by the macOS reveal path and the AppImage/Windows
/// self-replace path.
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
    if !is_verified(&bytes, expected) {
        return Err(UpdateError::Verify(format!(
            "checksum mismatch for {}",
            asset.name
        )));
    }

    // Defense-in-depth: the asset name is release-supplied. Strip path components before
    // joining onto the Downloads dir so a crafted name (e.g. `../`) can't escape it.
    let file_name = std::path::Path::new(&asset.name)
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new(&asset.name));
    let dir = downloads_dir();
    let dest = dir.join(file_name);
    std::fs::write(&dest, &bytes).map_err(|e| UpdateError::Io(e.to_string()))?;
    Ok(dest)
}

/// The platform Downloads directory, falling back to the temp dir when it
/// cannot be resolved (e.g. a headless/minimal environment).
fn downloads_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(|p| p.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
}
