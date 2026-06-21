//! OS file/folder drag-and-drop onto the Library screen (feature/drag_drop_add).
//!
//! Slint exposes no stable file-drop API, so this bridges the winit backend's raw
//! `WindowEvent`s (behind the `unstable-winit-030` feature; see Cargo.toml) into
//! the EXISTING bulk-add pipeline: dropped paths are handed straight to
//! [`AddController::start`](add_loader::AddController::start), reusing dedup, the
//! empty/unreadable/format-disabled reject rules, the archive policy, the
//! "Adding… (k/N)" progress, and the "added N / skipped M" toast. `gashuu-core`
//! is untouched — drag-drop is just a new presentation-layer SOURCE of the same
//! `paths` the Add buttons already produce.
//!
//! Batching: winit delivers one `DroppedFile(PathBuf)` per file with NO
//! "drop complete" event, so a single drop of N files arrives as N events.
//! Calling `start` per event would bump the controller's supersede epoch N times
//! and only the LAST file would survive. The fix is a short debounce — each
//! `DroppedFile` buffers its path and (re)arms a single-shot timer; the timer's
//! tick drains the whole batch into ONE `start`.
//!
//! Hover feedback: `HoveredFile` / `HoveredFileCancelled` toggle the `drag-active`
//! Slint bool that gates the drop-zone overlay. The overlay is gated to
//! `screen == 0` in Slint and the add is gated to `screen == 0` here, so a drop
//! over the Viewer is ignored — it is not an "add to library" surface.
//!
//! Threading: unlike `add_loader`'s probe workers, nothing here crosses a thread
//! boundary. The winit event filter, the debounce tick, and `start` all run on
//! the UI thread, so plain `Rc<RefCell<_>>` sharing is enough.

use crate::{add_loader, ViewerWindow};
use gashuu_core::Settings;
use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use winit::event::WindowEvent;

/// Debounce window collapsing one OS drop's burst of per-file `DroppedFile`
/// events into a single batched add. Imperceptible to the user, yet comfortably
/// longer than the gap between events of one drop on macOS/Windows/X11.
const DROP_DEBOUNCE: Duration = Duration::from_millis(50);

/// The `screen` property value for the Library (Carousel) screen; `1` is the
/// Viewer. Drag-drop is a Library-only add surface, so both the overlay and the
/// add are gated to this value.
const SCREEN_LIBRARY: i32 = 0;

/// What a raw winit window event means for drag-drop. Extracted as a pure
/// mapping so the event routing is unit-testable without a live event loop.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DropAction {
    /// A file is being dragged over the window — show the drop-zone overlay.
    ShowOverlay,
    /// The drag left the window or was cancelled — hide the overlay.
    HideOverlay,
    /// A file was dropped — hide the overlay and buffer this path for the add.
    Buffer(PathBuf),
    /// Not a drag-drop event — do nothing.
    Ignore,
}

/// Pure winit-event → [`DropAction`] mapping (the testable core of the filter).
/// Only the three file-drop variants matter; everything else is `Ignore` and
/// propagates to Slint unchanged.
pub(crate) fn drop_action_for(event: &WindowEvent) -> DropAction {
    match event {
        WindowEvent::HoveredFile(_) => DropAction::ShowOverlay,
        WindowEvent::HoveredFileCancelled => DropAction::HideOverlay,
        WindowEvent::DroppedFile(path) => DropAction::Buffer(path.clone()),
        _ => DropAction::Ignore,
    }
}

/// Register the file-drop bridge onto `ui`. Installs ONE winit window-event
/// filter that toggles the overlay on hover and feeds dropped paths to the bulk
/// add. A no-op if the window is not winit-backed (the trait degrades silently),
/// so non-winit builds simply have no drag-drop.
pub(crate) fn wire_drag_drop_handlers(
    ui: &ViewerWindow,
    settings: &Rc<RefCell<Settings>>,
    adder: &Rc<add_loader::AddController>,
) {
    let settings = Rc::clone(settings);
    let adder = Rc::clone(adder);
    let ui_weak = ui.as_weak();

    // Shared between the winit-event closure (pushes) and the debounce tick
    // (drains). Both run on the UI thread → a plain Rc<RefCell<_>> suffices.
    let buffer: Rc<RefCell<Vec<PathBuf>>> = Rc::new(RefCell::new(Vec::new()));
    // Owned single-shot timer, re-armed on each DroppedFile so the batch only
    // flushes once events stop. Held by the closure so it outlives each tick.
    let timer = Rc::new(slint::Timer::default());

    ui.window().on_winit_window_event(move |_win, event| {
        let Some(ui) = ui_weak.upgrade() else {
            return EventResult::Propagate;
        };
        match drop_action_for(event) {
            DropAction::ShowOverlay => {
                if ui.get_screen() == SCREEN_LIBRARY {
                    ui.set_drag_active(true);
                }
            }
            DropAction::HideOverlay => ui.set_drag_active(false),
            DropAction::Buffer(path) => {
                ui.set_drag_active(false);
                // A drop over the Viewer is not an add surface — ignore it.
                if ui.get_screen() == SCREEN_LIBRARY {
                    buffer.borrow_mut().push(path);
                    arm_flush(&timer, &buffer, &ui_weak, &settings, &adder);
                }
            }
            DropAction::Ignore => {}
        }
        // Slint ignores file-drop events anyway; never swallow other events.
        EventResult::Propagate
    });
}

/// (Re)arm the debounce timer: a single-shot that, on fire, drains the whole
/// buffered batch into ONE [`AddController::start`](add_loader::AddController::start).
/// Re-arming on each drop collapses a multi-file drop's event burst into a single
/// add generation (one supersede epoch, one progress run, one notice).
fn arm_flush(
    timer: &Rc<slint::Timer>,
    buffer: &Rc<RefCell<Vec<PathBuf>>>,
    ui_weak: &slint::Weak<ViewerWindow>,
    settings: &Rc<RefCell<Settings>>,
    adder: &Rc<add_loader::AddController>,
) {
    let buffer = Rc::clone(buffer);
    let ui_weak = ui_weak.clone();
    let settings = Rc::clone(settings);
    let adder = Rc::clone(adder);
    timer.start(slint::TimerMode::SingleShot, DROP_DEBOUNCE, move || {
        let paths: Vec<PathBuf> = buffer.borrow_mut().drain(..).collect();
        if paths.is_empty() {
            return;
        }
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        // Re-check the screen at flush time: the drop set screen==0, but guard
        // anyway so a late flush after a screen change cannot add behind the Viewer.
        if ui.get_screen() != SCREEN_LIBRARY {
            return;
        }
        let policy = settings.borrow().archive_policy();
        adder.start(ui.as_weak(), paths, policy, "add-drop");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hovering a file maps to showing the overlay; the carried path is irrelevant
    /// to the hover cue, so it is not inspected.
    #[test]
    fn hovered_file_shows_overlay() {
        let event = WindowEvent::HoveredFile(PathBuf::from("/some/book.cbz"));
        assert_eq!(drop_action_for(&event), DropAction::ShowOverlay);
    }

    /// A cancelled / departed drag hides the overlay.
    #[test]
    fn hover_cancelled_hides_overlay() {
        assert_eq!(
            drop_action_for(&WindowEvent::HoveredFileCancelled),
            DropAction::HideOverlay
        );
    }

    /// A dropped file is buffered, carrying its exact path through for the add.
    #[test]
    fn dropped_file_buffers_its_path() {
        let path = PathBuf::from("/some/manga");
        let event = WindowEvent::DroppedFile(path.clone());
        assert_eq!(drop_action_for(&event), DropAction::Buffer(path));
    }

    /// Non-drag events (here a representative resize) are ignored so they keep
    /// propagating to Slint untouched.
    #[test]
    fn other_events_are_ignored() {
        let event = WindowEvent::Resized(winit::dpi::PhysicalSize::new(800, 600));
        assert_eq!(drop_action_for(&event), DropAction::Ignore);
    }
}
