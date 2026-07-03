//! Shared background-worker → UI-thread plumbing for the async loader controllers.
//!
//! The library carousel's cover loader, the page-count prefetch, the bulk-add
//! controller, the thumbnail strip, and the viewer page loader all hand a result
//! from a rayon worker back to the Slint event loop the same way, and the cover
//! loader and thumbnail strip rotate a per-generation cancel flag the same way.
//! Both patterns carry correctness invariants (drop a superseded generation's
//! late delivery; keep each `RefCell` borrow confined to one statement), so they
//! are single-homed here rather than re-spelled at every controller.

use crate::ViewerWindow;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;

/// Marshal `body` onto the Slint event loop for one background result, applying
/// the epoch-guard + weak-upgrade preamble shared by every worker → UI handoff.
///
/// `body` runs on the UI thread, so a `!Send` value (a `slint::Image`, a
/// `SharedString`) is still built INSIDE it — only the captured inputs must be
/// `Send`. The result of a SUPERSEDED generation is dropped: if `my_epoch` no
/// longer matches the live `epoch` (a library refresh / re-open bumped it since
/// this result was dispatched), the closure returns without touching the UI, so a
/// late delivery can never overwrite a fresh frame. A dead `Weak` (the window
/// closed) is likewise dropped. `what` tags the debug trace on the
/// event-loop-gone path so each call site stays identifiable. This is the ONE
/// spelling of the stale-generation guard (previously inline `epoch.load(Relaxed)
/// != my_epoch` at eight sites and the `is_current` helper at a ninth).
pub(crate) fn marshal_to_ui(
    weak: slint::Weak<ViewerWindow>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    what: &'static str,
    body: impl FnOnce(&ViewerWindow) + Send + 'static,
) {
    if let Err(e) = slint::invoke_from_event_loop(move || {
        // Drop results from a superseded generation (library refreshed since).
        if epoch.load(Relaxed) != my_epoch {
            return;
        }
        let Some(ui) = weak.upgrade() else {
            return;
        };
        body(&ui);
    }) {
        tracing::debug!(what, error = %e, "dropped UI update; event loop gone");
    }
}

/// Supersede the previous generation's cancel flag and install a fresh one,
/// returning a clone of the new flag for the just-started generation. Each
/// `RefCell` borrow is confined to its own statement (dropped at the `;`) so no
/// two overlap — collapsing these into one expression would compile but panic at
/// runtime with a double borrow. Shared by `CoverController` and
/// `ThumbnailController`.
pub(crate) fn rotate_cancel(cell: &RefCell<Arc<AtomicBool>>) -> Arc<AtomicBool> {
    cell.borrow().store(true, Relaxed);
    *cell.borrow_mut() = Arc::new(AtomicBool::new(false));
    Arc::clone(&cell.borrow())
}
