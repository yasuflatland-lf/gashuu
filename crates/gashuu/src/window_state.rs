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
use std::time::Duration;

/// Arrange to restore the saved geometry once the OS window exists. winit 0.30
/// creates the window lazily — it does not exist until the event loop spins — so
/// monitor enumeration and correct physical sizing only work AFTER `run()` has
/// started. We therefore defer the apply to a zero-delay single-shot timer, armed
/// here (before `ui.run()`) and firing on the first event-loop turn. A no-op when
/// nothing was saved (fresh install), so Slint's `preferred-*` boot size applies.
pub(crate) fn restore_geometry(ui: &ViewerWindow, settings: &Settings) {
    let Some(geom) = settings.window else {
        return;
    };
    // ~30 re-arms far exceeds the 1-2 ticks the window needs; the bound only prevents an
    // unbounded loop on a hypothetical non-winit build where the window never appears.
    arm_apply(ui.as_weak(), geom, 30);
}

/// Fire a zero-delay timer that applies `geom` once the winit window exists,
/// re-arming (up to `attempts` more times) if it is not up yet on this tick.
fn arm_apply(weak: slint::Weak<ViewerWindow>, geom: WindowGeometry, attempts: u8) {
    slint::Timer::single_shot(Duration::ZERO, move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.window().has_winit_window() && attempts > 0 {
            arm_apply(ui.as_weak(), geom, attempts - 1);
            return;
        }
        apply_geometry(&ui, geom);
    });
}

/// Apply size + position to the live window. Size is always applied (floored to
/// the legible minimum); position is applied only when it still lands on a
/// monitor, otherwise the window is centered on the primary monitor. No monitors
/// discoverable (non-winit build) → leave placement to the OS.
fn apply_geometry(ui: &ViewerWindow, geom: WindowGeometry) {
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
