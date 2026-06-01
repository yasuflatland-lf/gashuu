//! Presentation-layer zoom/pan/fit interaction state.
//!
//! `ViewportState` is a thin, testable wrapper holding the mutable zoom factor,
//! pan offset, fit mode, viewport size, and content size. It owns NO geometry:
//! every computation delegates to the pure `gashuu_core::viewport` functions
//! (fit-scale, clamping, cursor-anchored zoom). This mirrors the project pattern
//! of "pure compute in core, state/placement in the UI".
//!
//! Coordinates are logical pixels (`f32`). `zoom` is a factor in
//! `[ZOOM_MIN, ZOOM_MAX]` multiplied onto the fit baseline; the effective scale
//! handed to Slint is `clamp_zoom(zoom) * fit_scale(content, vp, fit_mode)`. The
//! offset is the content's top-left position within the viewport (may be negative
//! when zoomed in).

use gashuu_core::{viewport as vp, FitMode, Settings};

/// Per-step zoom multiplier for keyboard +/- and one wheel notch.
const ZOOM_STEP: f32 = 1.1;

/// Live zoom/pan/fit + viewport size for the displayed spread. Holds no geometry
/// of its own; all clamping and anchoring go through `gashuu_core::viewport`.
pub struct ViewportState {
    fit_mode: FitMode,
    /// Zoom factor in `[ZOOM_MIN, ZOOM_MAX]`, multiplied onto the fit baseline.
    zoom: f32,
    /// Content top-left position within the viewport.
    offset: (f32, f32),
    /// Latest viewport size pushed from Slint.
    vp_size: (f32, f32),
    /// Current content intrinsic px (from the spread).
    content_size: (f32, f32),
    /// Offset snapshot captured at drag start (absolute-delta panning).
    pan_origin: (f32, f32),
}

impl ViewportState {
    /// Construct from persisted settings: adopt `fit_mode`, start at `ZOOM_MIN`
    /// (1.0), zero-initialize offset / viewport size / content size / pan origin.
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            fit_mode: settings.fit_mode,
            zoom: vp::ZOOM_MIN,
            offset: (0.0, 0.0),
            vp_size: (0.0, 0.0),
            content_size: (0.0, 0.0),
            pan_origin: (0.0, 0.0),
        }
    }

    /// Fit baseline scale for the current content/viewport/fit mode (the zoom
    /// factor 1.0 scale). The effective scale multiplies this by the zoom factor.
    fn fit(&self) -> f32 {
        vp::fit_scale(
            self.content_size.0,
            self.content_size.1,
            self.vp_size.0,
            self.vp_size.1,
            self.fit_mode,
        )
    }

    /// Effective scale = clamped zoom factor times the fit baseline.
    fn scale(&self) -> f32 {
        vp::clamp_zoom(self.zoom) * self.fit()
    }

    /// Displayed content box `(content * scale)` for the current scale.
    fn displayed(&self) -> (f32, f32) {
        let s = self.scale();
        (self.content_size.0 * s, self.content_size.1 * s)
    }

    /// Re-center the offset for the current displayed box, then clamp.
    fn center_and_clamp(&mut self) {
        let (dw, dh) = self.displayed();
        let (cx, cy) = vp::centered_offset(dw, dh, self.vp_size.0, self.vp_size.1);
        self.offset = vp::clamp_offset(dw, dh, self.vp_size.0, self.vp_size.1, cx, cy);
    }

    /// Re-clamp the current offset for the current displayed box.
    fn reclamp(&mut self) {
        let (dw, dh) = self.displayed();
        self.offset = vp::clamp_offset(
            dw,
            dh,
            self.vp_size.0,
            self.vp_size.1,
            self.offset.0,
            self.offset.1,
        );
    }

    /// Page turn: update the content size, KEEP `fit_mode` and `zoom`, and reset
    /// the offset to centered (then clamped).
    pub fn set_content(&mut self, w: f32, h: f32) {
        self.content_size = (w, h);
        self.center_and_clamp();
    }

    /// Viewport resize: update `vp_size`, then re-clamp the current offset so the
    /// existing pan is preserved where the new size still allows it.
    pub fn resize(&mut self, vp_w: f32, vp_h: f32) {
        self.vp_size = (vp_w, vp_h);
        self.reclamp();
    }

    /// Wheel zoom anchored at `(anchor_x, anchor_y)`. `raw_delta` is normalized by
    /// SIGN ONLY so the step is platform-independent (raw wheel magnitudes differ
    /// wildly across OSes/devices).
    ///
    /// Sign convention: `raw_delta > 0` zooms IN, `< 0` zooms OUT, `0` is a no-op.
    /// Manual testing may need to flip this per platform (some backends report a
    /// downward wheel as positive); the flip belongs in the Slint callback, not
    /// here, so this pure step stays deterministic.
    pub fn zoom_at(&mut self, anchor_x: f32, anchor_y: f32, raw_delta: f32) {
        let step = if raw_delta > 0.0 {
            ZOOM_STEP
        } else if raw_delta < 0.0 {
            1.0 / ZOOM_STEP
        } else {
            1.0
        };
        let old_scale = self.scale();
        let new_zoom = vp::clamp_zoom(self.zoom * step);
        // Recompute the effective scale at the new zoom; the fit baseline is
        // unchanged, so only the (already-clamped) `new_zoom` factor differs.
        let new_scale = new_zoom * self.fit();
        let (nx, ny) = vp::anchored_zoom(
            anchor_x,
            anchor_y,
            old_scale,
            new_scale,
            self.offset.0,
            self.offset.1,
        );
        self.offset = (nx, ny);
        self.zoom = new_zoom;
        self.reclamp();
    }

    /// Keyboard +/- zoom, anchored at the viewport center. Equivalent to a wheel
    /// step at `(vp_w/2, vp_h/2)`.
    pub fn zoom_step(&mut self, zoom_in: bool) {
        let center = (self.vp_size.0 / 2.0, self.vp_size.1 / 2.0);
        let delta = if zoom_in { 1.0 } else { -1.0 };
        self.zoom_at(center.0, center.1, delta);
    }

    /// Reset zoom to 1.0 (`ZOOM_MIN`) and re-center the offset.
    pub fn reset(&mut self) {
        self.zoom = vp::ZOOM_MIN;
        self.center_and_clamp();
    }

    /// Adopt a new fit mode as the baseline: reset zoom to 1.0 and re-center.
    pub fn set_fit(&mut self, mode: FitMode) {
        self.fit_mode = mode;
        self.zoom = vp::ZOOM_MIN;
        self.center_and_clamp();
    }

    /// Cycle the fit mode Whole -> Width -> Actual -> Whole, then behave like
    /// `set_fit` (reset zoom + center).
    pub fn cycle_fit(&mut self) {
        let next = match self.fit_mode {
            FitMode::Whole => FitMode::Width,
            FitMode::Width => FitMode::Actual,
            FitMode::Actual => FitMode::Whole,
        };
        self.set_fit(next);
    }

    /// Current fit mode (the UI reflects this into the in-memory `Settings` for
    /// save-on-exit).
    pub fn fit_mode(&self) -> FitMode {
        self.fit_mode
    }

    /// Snapshot the current offset as the pan origin (called at drag start).
    pub fn begin_pan(&mut self) {
        self.pan_origin = self.offset;
    }

    /// Pan by an ABSOLUTE delta from the press point (no cumulative drift): the
    /// new offset is `pan_origin + total`, then clamped.
    pub fn pan_to(&mut self, total_dx: f32, total_dy: f32) {
        let (dw, dh) = self.displayed();
        self.offset = vp::clamp_offset(
            dw,
            dh,
            self.vp_size.0,
            self.vp_size.1,
            self.pan_origin.0 + total_dx,
            self.pan_origin.1 + total_dy,
        );
    }

    /// Slint render geometry: `(content_x, content_y, content_w, content_h)`.
    /// The offset is re-clamped here so the returned position is always valid even
    /// if state mutated without an explicit clamp.
    pub fn geometry(&self) -> (f32, f32, f32, f32) {
        let (dw, dh) = self.displayed();
        let (ox, oy) = vp::clamp_offset(
            dw,
            dh,
            self.vp_size.0,
            self.vp_size.1,
            self.offset.0,
            self.offset.1,
        );
        (ox, oy, dw, dh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Float comparison tolerance for geometry assertions.
    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// A state with a fixed viewport and content, ready for interaction tests.
    /// 400x300 content into a 200x200 viewport (Whole fit): width ratio 0.5 <
    /// height ratio ~0.667, so the fit baseline is 0.5 and the page fits.
    fn state_with(fit: FitMode) -> ViewportState {
        let mut s = ViewportState::from_settings(&Settings {
            fit_mode: fit,
            ..Default::default()
        });
        s.resize(200.0, 200.0);
        s.set_content(400.0, 300.0);
        s
    }

    /// A square-content state that overflows the viewport on BOTH axes once
    /// zoomed in. 600x600 content into 200x200 (Whole fit) -> baseline 1/3, so it
    /// fits EXACTLY at zoom 1.0; any zoom-in overflows both axes symmetrically.
    /// This isolates anchor/clamp assertions from the centering branch (which
    /// fires only when a displayed axis is `<=` the viewport).
    fn square_state() -> ViewportState {
        let mut s = ViewportState::from_settings(&Settings::default());
        s.resize(200.0, 200.0);
        s.set_content(600.0, 600.0);
        s
    }

    /// The content point under a viewport anchor: `(anchor - offset) / scale`.
    fn content_point_under(s: &ViewportState, ax: f32, ay: f32) -> (f32, f32) {
        let (ox, oy, _, _) = s.geometry();
        let scale = s.scale();
        ((ax - ox) / scale, (ay - oy) / scale)
    }

    // ---- from_settings -----------------------------------------------------

    #[test]
    fn from_settings_adopts_fit_mode_and_starts_at_zoom_min() {
        let s = ViewportState::from_settings(&Settings {
            fit_mode: FitMode::Width,
            ..Default::default()
        });
        assert_eq!(s.fit_mode(), FitMode::Width);
        assert!(approx(s.zoom, vp::ZOOM_MIN));
        assert_eq!(s.offset, (0.0, 0.0));
        assert_eq!(s.vp_size, (0.0, 0.0));
        assert_eq!(s.content_size, (0.0, 0.0));
        assert_eq!(s.pan_origin, (0.0, 0.0));
    }

    // ---- set_content -------------------------------------------------------

    #[test]
    fn set_content_centers_offset_and_preserves_fit_and_zoom() {
        let mut s = state_with(FitMode::Whole);
        // Whole fit of 400x300 into 200x200 -> scale 0.5, displayed 200x150,
        // centered offset (0, 25).
        let (ox, oy, dw, dh) = s.geometry();
        assert!(approx(dw, 200.0));
        assert!(approx(dh, 150.0));
        assert!(approx(ox, 0.0)); // (200-200)/2
        assert!(approx(oy, 25.0)); // (200-150)/2
        assert_eq!(s.fit_mode(), FitMode::Whole);
        assert!(approx(s.zoom, vp::ZOOM_MIN));

        // A second content set keeps fit + zoom and re-centers for the new size.
        s.zoom = 2.0; // pretend the user had zoomed
        s.set_content(100.0, 100.0);
        assert!(
            approx(s.zoom, 2.0),
            "zoom must be preserved across set_content"
        );
        assert_eq!(s.fit_mode(), FitMode::Whole);
    }

    // ---- resize ------------------------------------------------------------

    #[test]
    fn resize_reclamps_offset_into_range() {
        // Square content so the fit baseline keeps both axes overflowing after the
        // viewport shrinks (Whole fit shrinks the baseline with the viewport, so a
        // non-square page could start fitting on one axis and trip the centering
        // branch — not what this test means to exercise).
        let mut s = square_state();
        // Zoom in so the content overflows and the offset is meaningful.
        s.zoom_step(true);
        s.zoom_step(true);
        // Drag the content far past the edge, then shrink the viewport: the
        // offset must be re-clamped into the new (still-overflowing) valid range.
        s.begin_pan();
        s.pan_to(-9999.0, -9999.0);
        s.resize(100.0, 100.0);
        let (dw, dh) = s.displayed();
        assert!(dw > 100.0 && dh > 100.0, "both axes must still overflow");
        let (ox, oy, _, _) = s.geometry();
        // Overflowing axes clamp into [vp - disp, 0].
        let lo_x = 100.0_f32 - dw;
        let lo_y = 100.0_f32 - dh;
        assert!(ox >= lo_x - 1e-3 && ox <= 1e-3, "x out of range: {ox}");
        assert!(oy >= lo_y - 1e-3 && oy <= 1e-3, "y out of range: {oy}");
    }

    // ---- zoom_step / zoom_at -----------------------------------------------

    #[test]
    fn zoom_step_in_increases_scale_and_keeps_center_point() {
        let mut s = state_with(FitMode::Whole);
        let scale_before = s.scale();
        let (cx, cy) = (s.vp_size.0 / 2.0, s.vp_size.1 / 2.0);
        let point_before = content_point_under(&s, cx, cy);
        s.zoom_step(true);
        let scale_after = s.scale();
        assert!(scale_after > scale_before, "zoom-in must increase scale");
        let point_after = content_point_under(&s, cx, cy);
        assert!(
            approx(point_before.0, point_after.0) && approx(point_before.1, point_after.1),
            "center content point must stay put: {point_before:?} vs {point_after:?}"
        );
    }

    #[test]
    fn zoom_at_cursor_keeps_anchored_point_and_increases_scale() {
        // Square content that overflows both axes once zoomed, so the clamp's
        // centering branch never overrides the anchored offset on either axis.
        let mut s = square_state();
        // Zoom in first so the content already overflows before the anchored step.
        s.zoom_step(true);
        s.zoom_step(true);
        // Anchor at an off-center cursor position.
        let (ax, ay) = (160.0, 40.0);
        let scale_before = s.scale();
        let point_before = content_point_under(&s, ax, ay);
        s.zoom_at(ax, ay, 1.0);
        assert!(s.scale() > scale_before, "zoom-in must increase scale");
        let point_after = content_point_under(&s, ax, ay);
        assert!(
            approx(point_before.0, point_after.0) && approx(point_before.1, point_after.1),
            "cursor content point must stay put: {point_before:?} vs {point_after:?}"
        );
    }

    #[test]
    fn zoom_clamps_at_max_after_many_steps() {
        let mut s = state_with(FitMode::Whole);
        for _ in 0..100 {
            s.zoom_step(true);
        }
        assert!(approx(s.zoom, vp::ZOOM_MAX), "zoom must clamp to ZOOM_MAX");
    }

    #[test]
    fn zoom_clamps_at_min_after_many_steps_out() {
        let mut s = state_with(FitMode::Whole);
        s.zoom_step(true);
        s.zoom_step(true);
        for _ in 0..100 {
            s.zoom_step(false);
        }
        assert!(approx(s.zoom, vp::ZOOM_MIN), "zoom must clamp to ZOOM_MIN");
    }

    #[test]
    fn zoom_at_zero_delta_is_noop() {
        let mut s = state_with(FitMode::Whole);
        let before = s.geometry();
        let zoom_before = s.zoom;
        s.zoom_at(100.0, 100.0, 0.0);
        assert!(approx(s.zoom, zoom_before));
        let after = s.geometry();
        assert!(approx(before.0, after.0) && approx(before.1, after.1));
    }

    // ---- pan_to ------------------------------------------------------------

    #[test]
    fn pan_to_is_relative_to_press_point_no_drift() {
        let mut s = state_with(FitMode::Whole);
        // Zoom in so the content overflows the viewport and panning has room.
        s.zoom_step(true);
        s.zoom_step(true);
        s.begin_pan();
        s.pan_to(-10.0, -5.0);
        let first = s.offset;
        // Calling again with the SAME total delta must land on the same offset
        // (absolute-from-press, not cumulative).
        s.pan_to(-10.0, -5.0);
        let second = s.offset;
        assert!(
            approx(first.0, second.0) && approx(first.1, second.1),
            "pan_to must be absolute from the press point (no drift): {first:?} vs {second:?}"
        );
    }

    #[test]
    fn pan_to_on_fitting_content_stays_centered_regardless_of_delta() {
        // 400x300 content in 200x200 (Whole fit) -> scale 0.5, displayed 200x150;
        // both axes fit (200 <= 200, 150 <= 200) so clamp_offset takes the
        // centering branch, which overrides any pan delta.
        let mut s = state_with(FitMode::Whole);
        s.begin_pan();
        s.pan_to(500.0, -500.0);
        let (ox, oy) = s.offset;
        assert!(
            approx(ox, 0.0),
            "x stays centered (200-200)/2 = 0, got {ox}"
        );
        assert!(
            approx(oy, 25.0),
            "y stays centered (200-150)/2 = 25, got {oy}"
        );
    }

    #[test]
    fn zoom_in_at_max_leaves_offset_unchanged() {
        // Square content that overflows once zoomed, so the offset is anchored and
        // not overridden by the centering branch.
        let mut s = square_state();
        for _ in 0..100 {
            s.zoom_step(true);
        }
        assert!(
            approx(s.zoom, vp::ZOOM_MAX),
            "precondition: zoom at ZOOM_MAX"
        );
        let before = s.geometry();
        let offset_before = s.offset;
        // A further zoom-in clamps new_zoom back to ZOOM_MAX (no change), so scale
        // is unchanged, anchored_zoom is identity, and the offset cannot drift.
        s.zoom_at(s.vp_size.0 / 2.0, s.vp_size.1 / 2.0, 1.0);
        assert!(approx(s.zoom, vp::ZOOM_MAX), "zoom stays at ZOOM_MAX");
        assert!(
            approx(s.offset.0, offset_before.0) && approx(s.offset.1, offset_before.1),
            "offset must not drift after a no-op zoom-in: {offset_before:?} vs {:?}",
            s.offset
        );
        let after = s.geometry();
        assert!(
            approx(before.0, after.0)
                && approx(before.1, after.1)
                && approx(before.2, after.2)
                && approx(before.3, after.3),
            "geometry must be unchanged: {before:?} vs {after:?}"
        );
    }

    #[test]
    fn pan_to_clamps_at_boundary() {
        // Square content overflowing both axes -> both clamp (never center).
        let mut s = square_state();
        s.zoom_step(true);
        s.zoom_step(true);
        let (dw, dh) = s.displayed();
        assert!(
            dw > s.vp_size.0 && dh > s.vp_size.1,
            "both axes must overflow"
        );
        s.begin_pan();
        // Drag far past the left/top edge: offset clamps to the low bound
        // (vp - disp) on each overflowing axis.
        s.pan_to(-100000.0, -100000.0);
        let (ox, oy) = s.offset;
        let lo_x = s.vp_size.0 - dw;
        let lo_y = s.vp_size.1 - dh;
        assert!(
            approx(ox, lo_x),
            "x must clamp to low bound {lo_x}, got {ox}"
        );
        assert!(
            approx(oy, lo_y),
            "y must clamp to low bound {lo_y}, got {oy}"
        );

        // And the opposite edge clamps to the high bound 0 on both axes.
        s.begin_pan();
        s.pan_to(100000.0, 100000.0);
        let (ox2, oy2) = s.offset;
        assert!(approx(ox2, 0.0), "x must clamp to high bound 0, got {ox2}");
        assert!(approx(oy2, 0.0), "y must clamp to high bound 0, got {oy2}");
    }

    // ---- Width fit vertical overflow ---------------------------------------

    /// A tall page under Width fit: 200x600 content in a 200x200 viewport. Width
    /// fit baseline = vp_w/c_w = 200/200 = 1.0, so displayed = 200x600. The x axis
    /// fits exactly (centered to 0); the y axis overflows by 400 and is pannable
    /// in `[vp - disp, 0] = [-400, 0]`.
    fn tall_width_state() -> ViewportState {
        let mut s = ViewportState::from_settings(&Settings {
            fit_mode: FitMode::Width,
            ..Default::default()
        });
        s.resize(200.0, 200.0);
        s.set_content(200.0, 600.0);
        s
    }

    #[test]
    fn width_fit_tall_page_initial_geometry_centers_x_and_clamps_y() {
        let s = tall_width_state();
        let (ox, oy, dw, dh) = s.geometry();
        assert!(approx(dw, 200.0)); // 200 * 1.0
        assert!(approx(dh, 600.0)); // 600 * 1.0
                                    // x fits (200 <= 200) -> centered (200-200)/2 = 0.
        assert!(approx(ox, 0.0));
        // y overflows (600 > 200): centered_offset gives (200-600)/2 = -200, which
        // is inside [-400, 0] so clamp keeps it.
        assert!(approx(oy, -200.0));
    }

    #[test]
    fn width_fit_tall_page_pan_clamps_to_both_vertical_edges() {
        let mut s = tall_width_state();
        // pan_origin = current offset (0, -200). The valid y range is [-400, 0].
        // A large DOWNWARD drag is positive total_dy: -200 + big > 0 -> high bound 0.
        s.begin_pan();
        s.pan_to(0.0, 100000.0);
        let (_, oy_down, _, _) = s.geometry();
        assert!(
            approx(oy_down, 0.0),
            "downward (positive dy) drag clamps y to high bound 0, got {oy_down}"
        );

        // A large UPWARD drag is negative total_dy: -200 - big < -400 -> low bound.
        s.begin_pan();
        s.pan_to(0.0, -100000.0);
        let (_, oy_up, _, _) = s.geometry();
        assert!(
            approx(oy_up, -400.0),
            "upward (negative dy) drag clamps y to low bound -400, got {oy_up}"
        );
    }

    // ---- reset -------------------------------------------------------------

    #[test]
    fn reset_restores_zoom_one_and_centers() {
        let mut s = state_with(FitMode::Whole);
        s.zoom_step(true);
        s.zoom_step(true);
        s.begin_pan();
        s.pan_to(-30.0, -20.0);
        s.reset();
        assert!(approx(s.zoom, vp::ZOOM_MIN));
        // Centered: Whole fit 400x300 into 200x200 -> displayed 200x150, offset
        // (0, 25).
        let (ox, oy, _, _) = s.geometry();
        assert!(approx(ox, 0.0));
        assert!(approx(oy, 25.0));
    }

    // ---- set_fit / cycle_fit -----------------------------------------------

    #[test]
    fn set_fit_resets_zoom_and_centers() {
        let mut s = state_with(FitMode::Whole);
        s.zoom_step(true);
        s.set_fit(FitMode::Actual);
        assert_eq!(s.fit_mode(), FitMode::Actual);
        assert!(approx(s.zoom, vp::ZOOM_MIN));
        // Actual fit -> scale 1.0, displayed = content 400x300, centered offset
        // ((200-400)/2, (200-300)/2) = (-100, -50), but clamped: both axes
        // overflow so the offset clamps into [vp-disp, 0]; centered_offset gives
        // the high-magnitude middle which clamp keeps since it is in range.
        let (ox, oy, dw, dh) = s.geometry();
        assert!(approx(dw, 400.0));
        assert!(approx(dh, 300.0));
        assert!(approx(ox, -100.0));
        assert!(approx(oy, -50.0));
    }

    #[test]
    fn cycle_fit_transitions_whole_width_actual_whole() {
        let mut s = state_with(FitMode::Whole);
        assert_eq!(s.fit_mode(), FitMode::Whole);
        s.cycle_fit();
        assert_eq!(s.fit_mode(), FitMode::Width);
        s.cycle_fit();
        assert_eq!(s.fit_mode(), FitMode::Actual);
        s.cycle_fit();
        assert_eq!(s.fit_mode(), FitMode::Whole);
    }

    #[test]
    fn cycle_fit_resets_zoom_and_centers() {
        let mut s = state_with(FitMode::Whole);
        s.zoom_step(true);
        s.zoom_step(true);
        s.cycle_fit();
        assert!(
            approx(s.zoom, vp::ZOOM_MIN),
            "cycle_fit must reset zoom to 1.0"
        );
    }
}
