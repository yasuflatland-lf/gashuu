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
    use proptest::prelude::*;

    fn pan_dimension() -> impl Strategy<Value = f32> {
        prop_oneof![9 => 0.0f32..1e5, 1 => Just(0.0)]
    }

    fn fit_dimension() -> impl Strategy<Value = f32> {
        prop_oneof![8 => 0.0f32..1e4, 1 => -1e4f32..0.0, 1 => Just(0.0)]
    }

    fn relative_tolerance(a: f64, b: f64) -> f64 {
        1e-4 * a.abs().max(b.abs()).max(1.0)
    }

    proptest! {
        #[test]
        fn pan_clamps_or_centers_each_axis(
            disp in (pan_dimension(), pan_dimension()),
            vp in (pan_dimension(), pan_dimension()),
            offset in (-2e5f32..2e5, -2e5f32..2e5),
        ) {
            let displayed = ViewSize::new(disp.0, disp.1);
            let viewport = ViewSize::new(vp.0, vp.1);
            let centered = centered_offset(displayed, viewport);
            let clamped = clamp_offset(displayed, viewport, ViewPoint::new(offset.0, offset.1));

            for (disp, vp, input, center, output) in [
                (disp.0, vp.0, offset.0, centered.x, clamped.x),
                (disp.1, vp.1, offset.1, centered.y, clamped.y),
            ] {
                let gap = vp - disp;
                prop_assert_eq!(center, gap / 2.0);
                prop_assert_eq!(center.signum(), gap.signum());
                if disp <= vp {
                    prop_assert_eq!(output, gap / 2.0);
                } else {
                    prop_assert!((gap..=0.0).contains(&output));
                    prop_assert!(output + disp >= vp - 1e-3 * disp.max(1.0));
                    let expected = if input < gap {
                        gap
                    } else if input > 0.0 {
                        0.0
                    } else {
                        input
                    };
                    prop_assert_eq!(output, expected);
                }
            }

            prop_assert_eq!(clamp_offset(displayed, viewport, clamped), clamped);
        }

        #[test]
        fn anchored_zoom_preserves_content_coordinates(
            old_scale in 0.01f32..100.0,
            new_scale in 0.01f32..100.0,
            anchor in (-1e4f32..1e4, -1e4f32..1e4),
            offset in (-1e4f32..1e4, -1e4f32..1e4),
        ) {
            let next = anchored_zoom(
                ViewPoint::new(anchor.0, anchor.1),
                old_scale,
                new_scale,
                ViewPoint::new(offset.0, offset.1),
            );
            for (anchor, offset, next) in [(anchor.0, offset.0, next.x), (anchor.1, offset.1, next.y)] {
                let before = (f64::from(anchor) - f64::from(offset)) / f64::from(old_scale);
                let after = (f64::from(anchor) - f64::from(next)) / f64::from(new_scale);
                // One rounded viewport offset can span several distinguishable content points.
                let rounding = f64::from((next.next_up() - next).max(next - next.next_down()))
                    / (2.0 * f64::from(new_scale));
                prop_assert!(
                    (before - after).abs() <= relative_tolerance(before, after) + rounding,
                    "content coordinate changed from {before} to {after} (rounding {rounding})",
                );
            }
        }

        #[test]
        fn anchored_zoom_with_non_positive_old_scale_keeps_offset(
            negative_scale in -100.0f32..0.0,
            new_scale in 0.01f32..100.0,
            anchor in (-1e4f32..1e4, -1e4f32..1e4),
            offset in (-1e4f32..1e4, -1e4f32..1e4),
        ) {
            let anchor = ViewPoint::new(anchor.0, anchor.1);
            let offset = ViewPoint::new(offset.0, offset.1);
            for old_scale in [0.0, -0.0, negative_scale] {
                prop_assert_eq!(anchored_zoom(anchor, old_scale, new_scale, offset), offset);
            }
        }

        #[test]
        fn clamp_zoom_pins_outside_values_and_preserves_inside_values(
            zoom in prop_oneof![
                any::<u32>().prop_map(f32::from_bits).prop_filter("non-NaN zoom", |z| !z.is_nan()),
                ZOOM_MIN..=ZOOM_MAX,
                Just(ZOOM_MAX.next_up()),
                Just(f32::INFINITY),
                Just(f32::NEG_INFINITY),
            ],
        ) {
            let clamped = clamp_zoom(zoom);
            prop_assert!((ZOOM_MIN..=ZOOM_MAX).contains(&clamped));
            let expected = if zoom.is_infinite() || zoom < ZOOM_MIN {
                ZOOM_MIN
            } else if zoom > ZOOM_MAX {
                ZOOM_MAX
            } else {
                zoom
            };
            prop_assert_eq!(clamped, expected);
        }

        #[test]
        fn fit_scale_follows_each_mode(
            content in (fit_dimension(), fit_dimension()),
            viewport in (fit_dimension(), fit_dimension()),
        ) {
            let content = ViewSize::new(content.0, content.1);
            let viewport = ViewSize::new(viewport.0, viewport.1);
            let actual = fit_scale(content, viewport, FitMode::Actual);
            let width = fit_scale(content, viewport, FitMode::Width);
            let whole = fit_scale(content, viewport, FitMode::Whole);
            prop_assert_eq!(actual, 1.0);

            if [content.width, content.height, viewport.width, viewport.height]
                .iter()
                .any(|&dimension| dimension <= 0.0)
            {
                prop_assert_eq!(width, 1.0);
                prop_assert_eq!(whole, 1.0);
            } else {
                prop_assert_eq!(width, viewport.width / content.width);
                prop_assert!(whole.is_finite() && whole > 0.0);
                let width_gap = f64::from(viewport.width) - f64::from(content.width) * f64::from(whole);
                let height_gap = f64::from(viewport.height) - f64::from(content.height) * f64::from(whole);
                let width_tolerance = relative_tolerance(f64::from(viewport.width), 0.0);
                let height_tolerance = relative_tolerance(f64::from(viewport.height), 0.0);
                prop_assert!(width_gap >= -width_tolerance);
                prop_assert!(height_gap >= -height_tolerance);
                prop_assert!(width_gap.abs() <= width_tolerance || height_gap.abs() <= height_tolerance);
            }
        }
    }

    #[test]
    fn clamp_zoom_nan_is_zoom_min() {
        for zoom in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(clamp_zoom(zoom), ZOOM_MIN);
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
