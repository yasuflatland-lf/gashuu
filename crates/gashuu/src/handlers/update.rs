//! Update-checker Rust glue: the startup/manual GitHub Releases check and (in
//! a follow-up commit) the update-available dialog's action callbacks plus the
//! settings About-section callbacks. Decision logic lives in
//! `gashuu_core::update`; this module owns the side-effecting glue (HTTP,
//! download, verify, open/reveal).
//!
//! `!Send` constraint: `Rc<RefCell<Settings>>` is `!Send`, so it can never be
//! captured by a `rayon::spawn` closure nor a `slint::invoke_from_event_loop`
//! closure — both require `Send`. Only `slint::Weak<ViewerWindow>` and owned
//! data (`String`, `ReleaseInfo`, `Option<..>`, `bool`) may cross into those
//! closures. The fetched `ReleaseInfo` is stashed in a UI-thread `thread_local!`
//! so the dialog's action handlers (which run on the UI thread) can read it
//! without threading the settings cell through a background closure. The check
//! timestamp is recorded on the UI thread BEFORE spawning, so the background
//! path never needs the settings cell either.

use crate::update::net::fetch_latest_release_json;
use crate::update::CURRENT_VERSION;
use crate::ViewerWindow;
use gashuu_core::{
    parse_latest_release, should_check, should_notify, ReleaseInfo, Settings, CHECK_INTERVAL_SECS,
};
use slint::ComponentHandle;
use std::cell::RefCell;
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
/// About section's version + toggle from the current settings; the dialog
/// action handlers (accept/later/skip/notes) and the settings check-now /
/// auto-update-toggle callbacks are registered here as they land.
pub(crate) fn wire_update_handlers(ui: &ViewerWindow, settings: &Rc<RefCell<Settings>>) {
    ui.set_settings_app_version(CURRENT_VERSION.into());
    ui.set_settings_auto_update_check(settings.borrow().auto_update_check);
}
