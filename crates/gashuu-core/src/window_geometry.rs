//! Window geometry (size + position) persisted across launches, plus the pure
//! restore-decision logic. Stored in PHYSICAL pixels (winit/Slint native units)
//! so size/position math needs no scale-factor conversion. Integer fields keep
//! `Settings`'s `Eq` derive intact.
//!
//! This module is headless: it defines its own [`Rect`] for monitor bounds; the
//! UI crate fills those from winit and converts back to Slint physical types.

use serde::{Deserialize, Serialize};

/// Sanity floor for a restored window size, mirroring the `min-width` /
/// `min-height` declared in `ViewerWindow.slint`. Guards against zero/garbage
/// stored values; Slint's own `min-*` re-enforces the exact minimum (for the
/// active DPI scale) when the size is applied.
pub const MIN_WINDOW_WIDTH: u32 = 480;
pub const MIN_WINDOW_HEIGHT: u32 = 600;

/// Sanity ceiling for a restored window size. A stored value above this is
/// treated as corrupt — not merely too large — and the whole geometry is
/// discarded for the default boot size rather than clamped (an off-screen window
/// is useless). 16384 is far beyond any real display yet well under the values a
/// scale-factor round-trip bug can inflate a size to across launches (the failure
/// that motivated this guard reached 110592).
pub const MAX_WINDOW_WIDTH: u32 = 16384;
pub const MAX_WINDOW_HEIGHT: u32 = 16384;

/// Vertical offset from the window top to a point on the title bar's grab area.
/// The on-screen test requires THIS point to land on a monitor, so the window is
/// always draggable even when its body extends past a screen edge.
const TITLE_BAR_GRAB: i32 = 16;

/// Persisted window geometry in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
}

/// A monitor's bounds in physical pixels. The UI crate builds these from winit;
/// `gashuu-core` stays headless and owns its own type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /// True when `(px, py)` lies inside this rectangle (left/top inclusive,
    /// right/bottom exclusive).
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && py >= self.y
            && px < self.x + self.width as i32
            && py < self.y + self.height as i32
    }
}

impl WindowGeometry {
    /// Size to apply, floored to the legible minimum.
    pub fn clamped_size(&self) -> (u32, u32) {
        (
            self.width.max(MIN_WINDOW_WIDTH),
            self.height.max(MIN_WINDOW_HEIGHT),
        )
    }

    /// True when the stored size is within the sane maximum. A larger value means
    /// the persisted geometry is corrupt (e.g. inflated by a HiDPI scale-factor
    /// round-trip) and should be discarded for the default boot size. The lower
    /// bound is intentionally NOT a sanity failure: a too-small size is floored by
    /// `clamped_size` rather than thrown away.
    pub fn is_size_sane(&self) -> bool {
        self.width <= MAX_WINDOW_WIDTH && self.height <= MAX_WINDOW_HEIGHT
    }

    /// Top-center grab point a little below the window's top edge. Uses
    /// saturating/u32-first arithmetic so absurd hand-edited values cannot
    /// overflow (and panic in debug).
    fn grab_point(&self) -> (i32, i32) {
        let half_w = (self.width / 2) as i32;
        (
            self.x.saturating_add(half_w),
            self.y.saturating_add(TITLE_BAR_GRAB),
        )
    }

    /// True when the title-bar grab point falls on some monitor (the window can
    /// be grabbed and moved). An empty monitor list returns `false` so the
    /// caller falls back to centering.
    pub fn is_position_visible(&self, monitors: &[Rect]) -> bool {
        let (gx, gy) = self.grab_point();
        monitors.iter().any(|m| m.contains(gx, gy))
    }
}

/// Top-left position centering `size` on `monitor`, clamped so the window never
/// starts above/left of the monitor origin (a window larger than the monitor
/// still keeps a grabbable title bar).
pub fn center_in(monitor: Rect, size: (u32, u32)) -> (i32, i32) {
    let (w, h) = size;
    let x = monitor.x + ((monitor.width as i32 - w as i32) / 2).max(0);
    let y = monitor.y + ((monitor.height as i32 - h as i32) / 2).max(0);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom(width: u32, height: u32, x: i32, y: i32) -> WindowGeometry {
        WindowGeometry {
            width,
            height,
            x,
            y,
        }
    }

    #[test]
    fn clamped_size_floors_below_minimum() {
        let g = geom(100, 200, 0, 0);
        assert_eq!(g.clamped_size(), (MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT));
    }

    #[test]
    fn clamped_size_passes_through_above_minimum() {
        let g = geom(1024, 768, 0, 0);
        assert_eq!(g.clamped_size(), (1024, 768));
    }

    #[test]
    fn position_visible_when_grab_point_on_a_monitor() {
        let monitors = [Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }];
        // Grab point = (100 + 400, 100 + 16) = (500, 116), inside the monitor.
        let g = geom(800, 600, 100, 100);
        assert!(g.is_position_visible(&monitors));
    }

    #[test]
    fn position_not_visible_when_off_all_monitors() {
        let monitors = [Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }];
        // A window placed far past the right edge (e.g. unplugged second display).
        let g = geom(800, 600, 5000, 100);
        assert!(!g.is_position_visible(&monitors));
    }

    #[test]
    fn position_visible_on_secondary_monitor_with_negative_origin() {
        // A monitor to the left of the primary (negative x), as winit reports it.
        let monitors = [
            Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            Rect {
                x: -1280,
                y: 0,
                width: 1280,
                height: 1024,
            },
        ];
        // Grab point = (-1000 + 400, 50 + 16) = (-600, 66), inside the second.
        let g = geom(800, 600, -1000, 50);
        assert!(g.is_position_visible(&monitors));
    }

    #[test]
    fn position_not_visible_with_no_monitors() {
        let g = geom(800, 600, 0, 0);
        assert!(!g.is_position_visible(&[]));
    }

    #[test]
    fn center_in_centers_window_on_monitor() {
        let monitor = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        // (1920-800)/2 = 560 ; (1080-600)/2 = 240
        assert_eq!(center_in(monitor, (800, 600)), (560, 240));
    }

    #[test]
    fn center_in_offsets_by_monitor_origin() {
        let monitor = Rect {
            x: -1280,
            y: 100,
            width: 1280,
            height: 1024,
        };
        // x = -1280 + (1280-800)/2 = -1280 + 240 = -1040
        // y =   100 + (1024-600)/2 =   100 + 212 =   312
        assert_eq!(center_in(monitor, (800, 600)), (-1040, 312));
    }

    #[test]
    fn center_in_clamps_when_window_larger_than_monitor() {
        let monitor = Rect {
            x: 10,
            y: 20,
            width: 640,
            height: 480,
        };
        // Window wider/taller than the monitor → clamp to the monitor origin.
        assert_eq!(center_in(monitor, (1024, 768)), (10, 20));
    }

    #[test]
    fn clamped_size_passes_through_at_minimum() {
        let g = geom(MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT, 0, 0);
        assert_eq!(g.clamped_size(), (MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT));
    }

    #[test]
    fn grab_point_does_not_overflow_on_extreme_values() {
        // Hand-edited garbage must not panic (debug) when computing the grab point.
        let g = geom(u32::MAX, 600, i32::MAX, 0);
        let monitors = [Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        }];
        assert!(!g.is_position_visible(&monitors));
    }

    #[test]
    fn size_sane_accepts_a_normal_window() {
        assert!(geom(1400, 900, 100, 100).is_size_sane());
    }

    #[test]
    fn size_sane_accepts_a_tiny_window() {
        // Below the legible minimum is still "sane" — `clamped_size` floors it
        // rather than discarding the geometry; only an absurdly large size is
        // treated as corrupt.
        assert!(geom(10, 10, 0, 0).is_size_sane());
    }

    #[test]
    fn size_sane_rejects_a_scale_factor_inflated_width() {
        // The real corruption that blanked the window: a HiDPI round-trip inflated
        // the width across launches until it reached 110592 (= 1728 * 2^6).
        let g = geom(110592, 1982, 0, 66);
        assert!(!g.is_size_sane());
    }

    #[test]
    fn size_sane_rejects_a_height_above_the_maximum() {
        assert!(!geom(1400, MAX_WINDOW_HEIGHT + 1, 0, 0).is_size_sane());
    }

    #[test]
    fn size_sane_accepts_exactly_the_maximum() {
        assert!(geom(MAX_WINDOW_WIDTH, MAX_WINDOW_HEIGHT, 0, 0).is_size_sane());
    }
}
