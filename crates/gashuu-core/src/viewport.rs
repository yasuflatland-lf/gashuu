//! Pure viewport geometry for zoom/pan: fit-scale, pan clamping, and
//! cursor-anchored zoom. Slint/tracing-free and table-testable. Owns no state;
//! the presentation layer (`gashuu::viewport::ViewportState`) holds the live
//! zoom/pan/fit + viewport size and calls these functions.
//!
//! Coordinates are logical pixels (`f32`). Content has an intrinsic size
//! `(content_w, content_h)` (the aspect source); the viewport is `(vp_w, vp_h)`.
//! The displayed content box is `(content_w * scale, content_h * scale)` and the
//! offset is the content's top-left position within the viewport (may be negative
//! when zoomed in). The effective scale is `clamp_zoom(zoom) * fit_scale(...)`;
//! that composition is done by the CALLER, not here (no combined helper).

use crate::settings::FitMode;

/// Minimum zoom factor (relative to the fit baseline). 1.0 = cannot shrink below
/// fit; the FitMode itself provides "show the whole page".
pub const ZOOM_MIN: f32 = 1.0;
/// Maximum zoom factor.
pub const ZOOM_MAX: f32 = 8.0;

/// Clamp a requested zoom factor into `[ZOOM_MIN, ZOOM_MAX]`.
pub fn clamp_zoom(zoom: f32) -> f32 {
    zoom.clamp(ZOOM_MIN, ZOOM_MAX)
}

/// Baseline scale that fits `content` into the viewport under `mode`.
/// `Whole` = `min(vp_w/c_w, vp_h/c_h)`, `Width` = `vp_w/c_w`, `Actual` = `1.0`.
/// Non-positive inputs (`content_* <= 0 || vp_* <= 0`) return `1.0` to avoid a
/// division by zero; the caller will not render an empty view.
pub fn fit_scale(content_w: f32, content_h: f32, vp_w: f32, vp_h: f32, mode: FitMode) -> f32 {
    if content_w <= 0.0 || content_h <= 0.0 || vp_w <= 0.0 || vp_h <= 0.0 {
        return 1.0;
    }
    match mode {
        FitMode::Whole => (vp_w / content_w).min(vp_h / content_h),
        FitMode::Width => vp_w / content_w,
        FitMode::Actual => 1.0,
    }
}

/// Centered offset for a displayed content box (used on page turn / reset):
/// `((vp_w - disp_w)/2, (vp_h - disp_h)/2)`. May be negative when zoomed in
/// (the content overflows the viewport and only its middle is shown).
pub fn centered_offset(disp_w: f32, disp_h: f32, vp_w: f32, vp_h: f32) -> (f32, f32) {
    ((vp_w - disp_w) / 2.0, (vp_h - disp_h) / 2.0)
}

/// Clamp pan so content never drifts past the viewport edges. Per axis: if the
/// displayed size `<=` the viewport, center it at `(vp - disp)/2`; otherwise clamp
/// the offset into `[vp - disp, 0]` (the content fully covers the viewport).
pub fn clamp_offset(
    disp_w: f32,
    disp_h: f32,
    vp_w: f32,
    vp_h: f32,
    ox: f32,
    oy: f32,
) -> (f32, f32) {
    (clamp_axis(disp_w, vp_w, ox), clamp_axis(disp_h, vp_h, oy))
}

/// Clamp a single axis: center when the content fits, else pin into `[vp-disp, 0]`.
fn clamp_axis(disp: f32, vp: f32, o: f32) -> f32 {
    if disp <= vp {
        (vp - disp) / 2.0
    } else {
        // disp > vp, so `vp - disp < 0 <= 0`: a valid `[lo, hi]` range for clamp.
        o.clamp(vp - disp, 0.0)
    }
}

/// New offset that keeps the content point under `(anchor_x, anchor_y)` fixed
/// while scale changes old->new:
/// `new_o = anchor - (anchor - old_o) * (new_scale / old_scale)`.
/// `old_scale <= 0` returns the offset unchanged. The caller clamps the result.
pub fn anchored_zoom(
    anchor_x: f32,
    anchor_y: f32,
    old_scale: f32,
    new_scale: f32,
    ox: f32,
    oy: f32,
) -> (f32, f32) {
    if old_scale <= 0.0 {
        return (ox, oy);
    }
    let ratio = new_scale / old_scale;
    (
        anchor_x - (anchor_x - ox) * ratio,
        anchor_y - (anchor_y - oy) * ratio,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::FitMode;

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

    // ---- fit_scale ---------------------------------------------------------

    #[test]
    fn fit_scale_whole_wide_content_limited_by_width() {
        // Wide content (400x100) into a wide viewport (200x200):
        // width ratio 0.5 < height ratio 2.0 -> min = 0.5 (width-dominated).
        assert!(approx(
            fit_scale(400.0, 100.0, 200.0, 200.0, FitMode::Whole),
            0.5
        ));
    }

    #[test]
    fn fit_scale_whole_tall_content_limited_by_height() {
        // Tall content (100x400) into a tall viewport (200x200):
        // height ratio 0.5 < width ratio 2.0 -> min = 0.5 (height-dominated).
        assert!(approx(
            fit_scale(100.0, 400.0, 200.0, 200.0, FitMode::Whole),
            0.5
        ));
    }

    #[test]
    fn fit_scale_width_uses_width_ratio_only() {
        // Width = vp_w/c_w = 600/300 = 2.0, regardless of height overflow.
        assert!(approx(
            fit_scale(300.0, 100.0, 600.0, 200.0, FitMode::Width),
            2.0
        ));
    }

    #[test]
    fn fit_scale_actual_is_always_one() {
        assert!(approx(
            fit_scale(300.0, 100.0, 600.0, 200.0, FitMode::Actual),
            1.0
        ));
        assert!(approx(
            fit_scale(10.0, 10.0, 1000.0, 1000.0, FitMode::Actual),
            1.0
        ));
    }

    #[test]
    fn fit_scale_non_positive_inputs_return_one() {
        // Each non-positive input independently forces the 1.0 fallback.
        for mode in [FitMode::Whole, FitMode::Width, FitMode::Actual] {
            assert!(approx(fit_scale(0.0, 100.0, 200.0, 200.0, mode), 1.0));
            assert!(approx(fit_scale(100.0, 0.0, 200.0, 200.0, mode), 1.0));
            assert!(approx(fit_scale(100.0, 100.0, 0.0, 200.0, mode), 1.0));
            assert!(approx(fit_scale(100.0, 100.0, 200.0, 0.0, mode), 1.0));
            assert!(approx(fit_scale(-1.0, 100.0, 200.0, 200.0, mode), 1.0));
        }
    }

    // ---- centered_offset ---------------------------------------------------

    #[test]
    fn centered_offset_shrunk_content_is_positive() {
        // Content smaller than viewport: positive margins on both axes.
        let (ox, oy) = centered_offset(100.0, 50.0, 300.0, 200.0);
        assert!(approx(ox, 100.0)); // (300-100)/2
        assert!(approx(oy, 75.0)); // (200-50)/2
    }

    #[test]
    fn centered_offset_enlarged_content_is_negative() {
        // Content larger than viewport: negative offset (middle is shown).
        let (ox, oy) = centered_offset(500.0, 400.0, 300.0, 200.0);
        assert!(approx(ox, -100.0)); // (300-500)/2
        assert!(approx(oy, -100.0)); // (200-400)/2
    }

    // ---- clamp_offset ------------------------------------------------------

    #[test]
    fn clamp_offset_disp_fits_centers_both_axes() {
        // disp <= vp on both axes -> centered, ignoring the requested offset.
        let (ox, oy) = clamp_offset(100.0, 50.0, 300.0, 200.0, 999.0, -999.0);
        assert!(approx(ox, 100.0)); // (300-100)/2
        assert!(approx(oy, 75.0)); // (200-50)/2
    }

    #[test]
    fn clamp_offset_overflow_left_edge_stays() {
        // disp > vp; o = 0 is the right-most valid edge (content top-left at vp
        // top-left), within [vp-disp, 0] -> unchanged.
        let (ox, _) = clamp_offset(500.0, 400.0, 300.0, 200.0, 0.0, -50.0);
        assert!(approx(ox, 0.0));
    }

    #[test]
    fn clamp_offset_overflow_right_edge_stays() {
        // o = vp - disp = 300 - 500 = -200 is the left-most valid edge -> unchanged.
        let (ox, _) = clamp_offset(500.0, 400.0, 300.0, 200.0, -200.0, -50.0);
        assert!(approx(ox, -200.0));
    }

    #[test]
    fn clamp_offset_overflow_middle_value_pinned_into_range() {
        // Requested ox = 50 (> 0) is pinned to the high bound 0; oy = -500
        // (< vp-disp = -200) is pinned to the low bound -200.
        let (ox, oy) = clamp_offset(500.0, 400.0, 300.0, 200.0, 50.0, -500.0);
        assert!(approx(ox, 0.0));
        assert!(approx(oy, -200.0));
    }

    #[test]
    fn clamp_offset_single_axis_overflow_mixes_center_and_clamp() {
        // Width overflows (500 > 300) -> clamp; height fits (50 <= 200) -> center.
        let (ox, oy) = clamp_offset(500.0, 50.0, 300.0, 200.0, 999.0, 999.0);
        assert!(approx(ox, 0.0)); // clamped to high bound
        assert!(approx(oy, 75.0)); // centered (200-50)/2
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
        let (anchor_x, anchor_y) = (150.0, 100.0);
        let (old_scale, new_scale) = (1.0, 2.0);
        let (ox, oy) = (10.0, 20.0);
        let (nx, ny) = anchored_zoom(anchor_x, anchor_y, old_scale, new_scale, ox, oy);
        assert_anchor_invariant(anchor_x, old_scale, new_scale, ox, nx);
        assert_anchor_invariant(anchor_y, old_scale, new_scale, oy, ny);
    }

    #[test]
    fn anchored_zoom_out_preserves_point_at_center() {
        let (anchor_x, anchor_y) = (150.0, 100.0);
        let (old_scale, new_scale) = (4.0, 2.0);
        let (ox, oy) = (-30.0, -40.0);
        let (nx, ny) = anchored_zoom(anchor_x, anchor_y, old_scale, new_scale, ox, oy);
        assert_anchor_invariant(anchor_x, old_scale, new_scale, ox, nx);
        assert_anchor_invariant(anchor_y, old_scale, new_scale, oy, ny);
    }

    #[test]
    fn anchored_zoom_preserves_point_at_edge() {
        // Anchor at the viewport origin (top-left edge).
        let (anchor_x, anchor_y) = (0.0, 0.0);
        let (old_scale, new_scale) = (1.0, 3.0);
        let (ox, oy) = (25.0, -10.0);
        let (nx, ny) = anchored_zoom(anchor_x, anchor_y, old_scale, new_scale, ox, oy);
        assert_anchor_invariant(anchor_x, old_scale, new_scale, ox, nx);
        assert_anchor_invariant(anchor_y, old_scale, new_scale, oy, ny);
    }

    #[test]
    fn anchored_zoom_old_scale_non_positive_is_identity() {
        let (ox, oy) = (12.0, -34.0);
        assert_eq!(anchored_zoom(50.0, 60.0, 0.0, 2.0, ox, oy), (ox, oy));
        assert_eq!(anchored_zoom(50.0, 60.0, -1.0, 2.0, ox, oy), (ox, oy));
    }
}
