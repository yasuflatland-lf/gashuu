//! Pure viewport geometry for zoom/pan: fit-scale, pan clamping, and
//! cursor-anchored zoom. Slint/tracing-free and table-testable. Owns no state;
//! the presentation layer (`gashuu::viewport::ViewportState`) holds the live
//! zoom/pan/fit + viewport size and calls these functions.
//!
//! Sizes and points are in logical pixels. The offset locates the displayed
//! content's top-left corner in the viewport and can be negative when zoomed in.
//! The caller scales the intrinsic content size by `clamp_zoom(zoom) * fit_scale(...)`.

use crate::view_modes::FitMode;

/// Width and height in logical pixels, as supplied by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ViewSize {
    pub width: f32,
    pub height: f32,
}

impl ViewSize {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    /// A renderable size has two finite, positive dimensions.
    pub fn is_renderable(self) -> bool {
        self.width.is_finite() && self.width > 0.0 && self.height.is_finite() && self.height > 0.0
    }

    /// Scale each dimension, using zero for a non-finite result.
    pub fn scaled(self, scale: f32) -> Self {
        Self::new(
            finite_or(self.width * scale, 0.0),
            finite_or(self.height * scale, 0.0),
        )
    }
}

/// Position or pan offset in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ViewPoint {
    pub x: f32,
    pub y: f32,
}

impl ViewPoint {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Translate each coordinate, using zero for a non-finite result.
    pub fn offset_by(self, dx: f32, dy: f32) -> Self {
        Self::new(finite_or(self.x + dx, 0.0), finite_or(self.y + dy, 0.0))
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

/// Minimum zoom factor (relative to the fit baseline). 1.0 = cannot shrink below
/// fit; the FitMode itself provides "show the whole page".
pub const ZOOM_MIN: f32 = 1.0;
/// Maximum zoom factor.
pub const ZOOM_MAX: f32 = 8.0;

/// Clamp zoom to `[ZOOM_MIN, ZOOM_MAX]`, returning `ZOOM_MIN` for non-finite input.
pub fn clamp_zoom(zoom: f32) -> f32 {
    finite_or(zoom, ZOOM_MIN).clamp(ZOOM_MIN, ZOOM_MAX)
}

/// Baseline scale for the chosen fit mode. Empty or non-finite dimensions and
/// unrepresentable scale factors fall back to actual size (1.0).
pub fn fit_scale(content: ViewSize, viewport: ViewSize, mode: FitMode) -> f32 {
    if !content.is_renderable() || !viewport.is_renderable() {
        return 1.0;
    }
    let scale = match mode {
        FitMode::Whole => (viewport.width / content.width).min(viewport.height / content.height),
        FitMode::Width => viewport.width / content.width,
        FitMode::Actual => 1.0,
    };
    finite_or(scale, 1.0)
}

/// Offset that centers the displayed content, including when it overflows.
/// An axis with a non-finite dimension gets zero offset.
pub fn centered_offset(displayed: ViewSize, viewport: ViewSize) -> ViewPoint {
    ViewPoint::new(
        centered_axis(displayed.width, viewport.width),
        centered_axis(displayed.height, viewport.height),
    )
}

fn centered_axis(displayed: f32, viewport: f32) -> f32 {
    // The half-difference fits in f32 even when the difference itself exceeds f32::MAX.
    finite_or(
        ((f64::from(viewport) - f64::from(displayed)) * 0.5) as f32,
        0.0,
    )
}

/// Center fitting axes and clamp overflowing axes to the viewport edges.
/// A non-finite dimension gives zero offset on that axis.
pub fn clamp_offset(displayed: ViewSize, viewport: ViewSize, offset: ViewPoint) -> ViewPoint {
    ViewPoint::new(
        clamp_axis(displayed.width, viewport.width, offset.x),
        clamp_axis(displayed.height, viewport.height, offset.y),
    )
}

fn clamp_axis(displayed: f32, viewport: f32, offset: f32) -> f32 {
    if !displayed.is_finite() || !viewport.is_finite() {
        return 0.0;
    }
    if displayed <= viewport {
        centered_axis(displayed, viewport)
    } else {
        finite_or(offset, 0.0).clamp(viewport - displayed, 0.0)
    }
}

/// Adjust the offset to keep the same content point under the anchor as scale changes.
/// A non-positive old scale or either non-finite scale preserves the offset.
/// Each axis retains its previous finite offset if anchoring fails, or zero if unavailable.
pub fn anchored_zoom(
    anchor: ViewPoint,
    old_scale: f32,
    new_scale: f32,
    offset: ViewPoint,
) -> ViewPoint {
    let offset = ViewPoint::new(finite_or(offset.x, 0.0), finite_or(offset.y, 0.0));
    if !old_scale.is_finite() || old_scale <= 0.0 || !new_scale.is_finite() {
        return offset;
    }
    let ratio = new_scale / old_scale;
    ViewPoint::new(
        finite_or(anchor.x - (anchor.x - offset.x) * ratio, offset.x),
        finite_or(anchor.y - (anchor.y - offset.y) * ratio, offset.y),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_modes::FitMode;

    /// Float comparison tolerance for geometry assertions.
    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    // ---- clamp_zoom --------------------------------------------------------

    #[test]
    fn clamp_zoom_below_min_pins_to_min() {
        assert!(approx(clamp_zoom(0.0), ZOOM_MIN));
        assert!(approx(clamp_zoom(0.5), ZOOM_MIN));
        assert!(approx(clamp_zoom(-3.0), ZOOM_MIN));
    }

    #[test]
    fn clamp_zoom_above_max_pins_to_max() {
        assert!(approx(clamp_zoom(100.0), ZOOM_MAX));
        assert!(approx(clamp_zoom(8.0001), ZOOM_MAX));
    }

    #[test]
    fn clamp_zoom_in_range_unchanged() {
        assert!(approx(clamp_zoom(1.0), 1.0));
        assert!(approx(clamp_zoom(3.5), 3.5));
        assert!(approx(clamp_zoom(8.0), 8.0));
    }

    #[test]
    fn clamp_zoom_nan_is_zoom_min() {
        for zoom in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(clamp_zoom(zoom), ZOOM_MIN);
        }
    }

    // ---- fit_scale ---------------------------------------------------------

    #[test]
    fn fit_scale_whole_wide_content_limited_by_width() {
        // Wide content (400x100) into a wide viewport (200x200):
        // width ratio 0.5 < height ratio 2.0 -> min = 0.5 (width-dominated).
        assert!(approx(
            fit_scale(
                ViewSize::new(400.0, 100.0),
                ViewSize::new(200.0, 200.0),
                FitMode::Whole
            ),
            0.5
        ));
    }

    #[test]
    fn fit_scale_whole_tall_content_limited_by_height() {
        // Tall content (100x400) into a tall viewport (200x200):
        // height ratio 0.5 < width ratio 2.0 -> min = 0.5 (height-dominated).
        assert!(approx(
            fit_scale(
                ViewSize::new(100.0, 400.0),
                ViewSize::new(200.0, 200.0),
                FitMode::Whole
            ),
            0.5
        ));
    }

    #[test]
    fn fit_scale_width_uses_width_ratio_only() {
        // Width = vp_w/c_w = 600/300 = 2.0, regardless of height overflow.
        assert!(approx(
            fit_scale(
                ViewSize::new(300.0, 100.0),
                ViewSize::new(600.0, 200.0),
                FitMode::Width
            ),
            2.0
        ));
    }

    #[test]
    fn fit_scale_actual_is_always_one() {
        assert!(approx(
            fit_scale(
                ViewSize::new(300.0, 100.0),
                ViewSize::new(600.0, 200.0),
                FitMode::Actual
            ),
            1.0
        ));
        assert!(approx(
            fit_scale(
                ViewSize::new(10.0, 10.0),
                ViewSize::new(1000.0, 1000.0),
                FitMode::Actual
            ),
            1.0
        ));
    }

    #[test]
    fn fit_scale_non_positive_inputs_return_one() {
        // Each non-positive input independently forces the 1.0 fallback.
        let content = ViewSize::new(100.0, 100.0);
        let viewport = ViewSize::new(200.0, 200.0);
        for (content, viewport) in [
            (ViewSize::new(0.0, 100.0), viewport),
            (ViewSize::new(100.0, 0.0), viewport),
            (content, ViewSize::new(0.0, 200.0)),
            (content, ViewSize::new(200.0, 0.0)),
            (ViewSize::new(-1.0, 100.0), viewport),
        ] {
            for mode in [FitMode::Whole, FitMode::Width, FitMode::Actual] {
                assert_eq!(fit_scale(content, viewport, mode), 1.0);
            }
        }
    }

    #[test]
    fn fit_scale_non_finite_inputs_return_one() {
        let content = ViewSize::new(300.0, 100.0);
        let viewport = ViewSize::new(600.0, 200.0);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for (content, viewport) in [
                (ViewSize::new(bad, content.height), viewport),
                (ViewSize::new(content.width, bad), viewport),
                (content, ViewSize::new(bad, viewport.height)),
                (content, ViewSize::new(viewport.width, bad)),
            ] {
                for mode in [FitMode::Whole, FitMode::Width, FitMode::Actual] {
                    assert_eq!(fit_scale(content, viewport, mode), 1.0);
                }
            }
        }
    }

    #[test]
    fn transposed_axes_are_distinct() {
        let tall = ViewSize::new(100.0, 400.0);
        let wide = ViewSize::new(400.0, 100.0);
        let square = ViewSize::new(200.0, 200.0);
        assert_eq!(fit_scale(tall, square, FitMode::Whole), 0.5);
        assert_eq!(fit_scale(wide, square, FitMode::Whole), 0.5);

        let landscape = ViewSize::new(400.0, 200.0);
        assert_eq!(fit_scale(tall, landscape, FitMode::Whole), 0.5);
        assert_eq!(fit_scale(wide, landscape, FitMode::Whole), 1.0);
    }

    // ---- centered_offset ---------------------------------------------------

    #[test]
    fn centered_offset_shrunk_content_is_positive() {
        // Content smaller than viewport: positive margins on both axes.
        let offset = centered_offset(ViewSize::new(100.0, 50.0), ViewSize::new(300.0, 200.0));
        assert!(approx(offset.x, 100.0)); // (300-100)/2
        assert!(approx(offset.y, 75.0)); // (200-50)/2
    }

    #[test]
    fn centered_offset_enlarged_content_is_negative() {
        // Content larger than viewport: negative offset (middle is shown).
        let offset = centered_offset(ViewSize::new(500.0, 400.0), ViewSize::new(300.0, 200.0));
        assert!(approx(offset.x, -100.0)); // (300-500)/2
        assert!(approx(offset.y, -100.0)); // (200-400)/2
    }

    // ---- clamp_offset ------------------------------------------------------

    #[test]
    fn clamp_offset_disp_fits_centers_both_axes() {
        // disp <= vp on both axes -> centered, ignoring the requested offset.
        let offset = clamp_offset(
            ViewSize::new(100.0, 50.0),
            ViewSize::new(300.0, 200.0),
            ViewPoint::new(999.0, -999.0),
        );
        assert!(approx(offset.x, 100.0)); // (300-100)/2
        assert!(approx(offset.y, 75.0)); // (200-50)/2
    }

    #[test]
    fn clamp_offset_overflow_left_edge_stays() {
        // disp > vp; o = 0 is the right-most valid edge (content top-left at vp
        // top-left), within [vp-disp, 0] -> unchanged.
        let offset = clamp_offset(
            ViewSize::new(500.0, 400.0),
            ViewSize::new(300.0, 200.0),
            ViewPoint::new(0.0, -50.0),
        );
        assert!(approx(offset.x, 0.0));
    }

    #[test]
    fn clamp_offset_overflow_right_edge_stays() {
        // o = vp - disp = 300 - 500 = -200 is the left-most valid edge -> unchanged.
        let offset = clamp_offset(
            ViewSize::new(500.0, 400.0),
            ViewSize::new(300.0, 200.0),
            ViewPoint::new(-200.0, -50.0),
        );
        assert!(approx(offset.x, -200.0));
    }

    #[test]
    fn clamp_offset_overflow_middle_value_pinned_into_range() {
        // Requested ox = 50 (> 0) is pinned to the high bound 0; oy = -500
        // (< vp-disp = -200) is pinned to the low bound -200.
        let offset = clamp_offset(
            ViewSize::new(500.0, 400.0),
            ViewSize::new(300.0, 200.0),
            ViewPoint::new(50.0, -500.0),
        );
        assert!(approx(offset.x, 0.0));
        assert!(approx(offset.y, -200.0));
    }

    #[test]
    fn clamp_offset_single_axis_overflow_mixes_center_and_clamp() {
        // Width overflows (500 > 300) -> clamp; height fits (50 <= 200) -> center.
        let offset = clamp_offset(
            ViewSize::new(500.0, 50.0),
            ViewSize::new(300.0, 200.0),
            ViewPoint::new(999.0, 999.0),
        );
        assert!(approx(offset.x, 0.0)); // clamped to high bound
        assert!(approx(offset.y, 75.0)); // centered (200-50)/2
    }

    #[test]
    fn clamp_offset_never_panics_on_nan_or_inf() {
        let values = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, 100.0];
        // Vary one axis at a time and keep the other at a known overflowing offset.
        for displayed in values {
            for viewport in values {
                for offset in values {
                    let x = clamp_offset(
                        ViewSize::new(displayed, 200.0),
                        ViewSize::new(viewport, 100.0),
                        ViewPoint::new(offset, -25.0),
                    );
                    let y = clamp_offset(
                        ViewSize::new(200.0, displayed),
                        ViewSize::new(100.0, viewport),
                        ViewPoint::new(-25.0, offset),
                    );
                    let expected = if displayed == 0.0 && viewport == 100.0 {
                        50.0
                    } else {
                        0.0
                    };
                    assert_eq!(x, ViewPoint::new(expected, -25.0));
                    assert_eq!(y, ViewPoint::new(-25.0, expected));
                }
            }
        }
    }

    // ---- anchored_zoom -----------------------------------------------------

    /// The content point under the cursor must be invariant across a scale
    /// change: `(anchor - new_o)/new_scale == (anchor - old_o)/old_scale`.
    fn assert_anchor_invariant(
        anchor: f32,
        old_scale: f32,
        new_scale: f32,
        old_o: f32,
        new_o: f32,
    ) {
        let before = (anchor - old_o) / old_scale;
        let after = (anchor - new_o) / new_scale;
        assert!(
            approx(before, after),
            "content point not preserved: before={before}, after={after}"
        );
    }

    #[test]
    fn anchored_zoom_in_preserves_point_at_center() {
        let anchor = ViewPoint::new(150.0, 100.0);
        let (old_scale, new_scale) = (1.0, 2.0);
        let offset = ViewPoint::new(10.0, 20.0);
        let next = anchored_zoom(anchor, old_scale, new_scale, offset);
        assert_anchor_invariant(anchor.x, old_scale, new_scale, offset.x, next.x);
        assert_anchor_invariant(anchor.y, old_scale, new_scale, offset.y, next.y);
    }

    #[test]
    fn anchored_zoom_out_preserves_point_at_center() {
        let anchor = ViewPoint::new(150.0, 100.0);
        let (old_scale, new_scale) = (4.0, 2.0);
        let offset = ViewPoint::new(-30.0, -40.0);
        let next = anchored_zoom(anchor, old_scale, new_scale, offset);
        assert_anchor_invariant(anchor.x, old_scale, new_scale, offset.x, next.x);
        assert_anchor_invariant(anchor.y, old_scale, new_scale, offset.y, next.y);
    }

    #[test]
    fn anchored_zoom_preserves_point_at_edge() {
        // Anchor at the viewport origin (top-left edge).
        let anchor = ViewPoint::new(0.0, 0.0);
        let (old_scale, new_scale) = (1.0, 3.0);
        let offset = ViewPoint::new(25.0, -10.0);
        let next = anchored_zoom(anchor, old_scale, new_scale, offset);
        assert_anchor_invariant(anchor.x, old_scale, new_scale, offset.x, next.x);
        assert_anchor_invariant(anchor.y, old_scale, new_scale, offset.y, next.y);
    }

    #[test]
    fn anchored_zoom_old_scale_non_positive_is_identity() {
        let offset = ViewPoint::new(12.0, -34.0);
        assert_eq!(
            anchored_zoom(ViewPoint::new(50.0, 60.0), 0.0, 2.0, offset),
            offset
        );
        assert_eq!(
            anchored_zoom(ViewPoint::new(50.0, 60.0), -1.0, 2.0, offset),
            offset
        );
    }

    #[test]
    fn anchored_zoom_non_finite_scale_keeps_offset() {
        let anchor = ViewPoint::new(50.0, 60.0);
        let offset = ViewPoint::new(12.0, -34.0);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(anchored_zoom(anchor, bad, 2.0, offset), offset);
            assert_eq!(anchored_zoom(anchor, 1.0, bad, offset), offset);
        }
    }

    #[test]
    fn centered_offset_handles_non_finite_and_extreme_sizes_per_axis() {
        let displayed = ViewSize::new(f32::NAN, -f32::MAX);
        let viewport = ViewSize::new(100.0, f32::MAX);
        let expected = ViewPoint::new(0.0, f32::MAX);
        assert_eq!(centered_offset(displayed, viewport), expected);
        assert_eq!(
            clamp_offset(displayed, viewport, ViewPoint::default()),
            expected
        );
    }

    #[test]
    fn geometry_stays_finite_when_arithmetic_overflows() {
        let size = ViewSize::new(f32::MAX, 100.0);
        assert!(size.scaled(2.0).width.is_finite());
        let offset = ViewPoint::new(f32::MAX, 12.0);
        assert!(offset.offset_by(f32::MAX, 0.0).x.is_finite());
        assert!(fit_scale(ViewSize::new(0.5, 1.0), size, FitMode::Width).is_finite());
        let next = anchored_zoom(ViewPoint::new(0.0, f32::NAN), 1.0, 2.0, offset);
        assert!(next.x.is_finite() && next.y.is_finite());
    }
}
