//! Persist and restore the OS window geometry (size + position) across launches.
//!
//! Geometry is read ONCE at exit and applied ONCE at startup — never tracked
//! live — so this installs no winit event handler and cannot collide with the
//! single `on_winit_window_event` slot drag-drop owns (`handlers/drag_drop.rs`).
//! Everything is in physical pixels (winit/Slint native), matching how
//! `gashuu_core::WindowGeometry` is stored.

use crate::ViewerWindow;
use gashuu_core::{center_in, Rect, Settings, WindowGeometry};
use slint::winit_030::{winit, WinitWindowAccessor};
use slint::ComponentHandle;

/// Apply the saved geometry to `ui` before the event loop starts. The size is
/// always restored (floored to the legible minimum). The position is restored
/// only when it still lands on a monitor; otherwise the window is centered on
/// the primary monitor. No-op when nothing was saved (fresh install) so Slint's
/// `preferred-*` boot size applies.
pub(crate) fn restore_geometry(ui: &ViewerWindow, settings: &Settings) {
    let Some(geom) = settings.window else {
        return;
    };
    let (w, h) = geom.clamped_size();
    ui.window().set_size(slint::PhysicalSize::new(w, h));

    let monitors = monitor_rects(ui);
    if geom.is_position_visible(&monitors) {
        ui.window()
            .set_position(slint::PhysicalPosition::new(geom.x, geom.y));
    } else if let Some(primary) = monitors.first() {
        let (x, y) = center_in(*primary, (w, h));
        ui.window().set_position(slint::PhysicalPosition::new(x, y));
    }
    // No monitors discoverable (non-winit build): leave placement to the OS.
}

/// Capture the live window geometry into `settings` at exit. Called after
/// `run()` returns, while the window handle is still alive.
pub(crate) fn capture_geometry(ui: &ViewerWindow, settings: &mut Settings) {
    let size = ui.window().size();
    let pos = ui.window().position();
    settings.window = Some(WindowGeometry {
        width: size.width,
        height: size.height,
        x: pos.x,
        y: pos.y,
    });
}

/// Physical-pixel bounds of every monitor, primary first. Empty when the window
/// is not winit-backed (the accessor returns `None`).
fn monitor_rects(ui: &ViewerWindow) -> Vec<Rect> {
    ui.window()
        .with_winit_window(|win| {
            let mut rects: Vec<Rect> = Vec::new();
            // Primary first so the centering fallback targets it.
            if let Some(primary) = win.primary_monitor() {
                rects.push(monitor_rect(&primary));
            }
            for m in win.available_monitors() {
                let r = monitor_rect(&m);
                if !rects.contains(&r) {
                    rects.push(r);
                }
            }
            rects
        })
        .unwrap_or_default()
}

fn monitor_rect(m: &winit::monitor::MonitorHandle) -> Rect {
    let pos = m.position();
    let size = m.size();
    Rect {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
    }
}
