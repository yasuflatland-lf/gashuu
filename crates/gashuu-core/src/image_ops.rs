//! Decoding helpers. Returns raw RGBA8 + dimensions so the presentation layer
//! can build a `slint::Image` without this crate depending on slint.

use crate::error::CoreError;

/// Maximum allowed pixel count for decoded images.
///
/// Aligned with the existing 512 MiB alloc cap: 512 MiB / 4 bytes-per-RGBA-pixel
/// = 128 Mpx. An explicit pixel-count guard makes the rejection intent testable
/// without exercising the alloc path.
pub const MAX_PIXELS: u64 = 128 * 1024 * 1024;

/// Check that a `width × height` image does not exceed [`MAX_PIXELS`].
///
/// This is a pure, allocation-free function used as an early guard inside
/// [`decode`] (before the full decode allocates memory) and directly in tests.
pub fn check_pixel_limit(width: u32, height: u32) -> Result<(), CoreError> {
    let pixels = (width as u64) * (height as u64);
    if pixels > MAX_PIXELS {
        return Err(CoreError::ImageTooLarge {
            width,
            height,
            pixels,
            max: MAX_PIXELS,
        });
    }
    Ok(())
}

/// A decoded image as tightly packed RGBA8 bytes plus its dimensions.
///
/// The invariant `rgba.len() == width * height * 4` is enforced by the only
/// constructor, [`DecodedImage::new`]; fields are private so it cannot be broken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl DecodedImage {
    /// Build a decoded image, validating that `rgba` is exactly
    /// `width * height * 4` bytes (row-major, RGBA order).
    pub fn new(rgba: Vec<u8>, width: u32, height: u32) -> Result<Self, CoreError> {
        let expected = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if rgba.len() != expected {
            return Err(CoreError::MalformedImage {
                expected,
                actual: rgba.len(),
            });
        }
        Ok(Self {
            rgba,
            width,
            height,
        })
    }

    /// Tightly packed RGBA8 bytes (`width * height * 4`).
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

/// Decode encoded image bytes into RGBA8 using the `image` crate.
///
/// Supports any format recognized by `image::ImageReader` (PNG, JPEG, …). Decoder
/// limits reject oversize / decompression-bomb images before allocation.
///
/// Defense in depth: a lightweight header-only pre-read via `into_dimensions()`
/// runs [`check_pixel_limit`] before the full decode allocates any pixel memory.
/// The existing `image::Limits` alloc cap is kept as a second layer.
///
/// `image::Limits` is `#[non_exhaustive]`, so struct-literal init is impossible;
/// we must use field assignment after `default()`, which triggers
/// `clippy::field_reassign_with_default`. The allow is scoped to this function only.
#[allow(clippy::field_reassign_with_default)]
pub fn decode(bytes: &[u8]) -> Result<DecodedImage, CoreError> {
    // Pre-read: parse header only (cheap) to check dimensions before allocating.
    let header_reader =
        image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let (w, h) = header_reader.into_dimensions()?;
    check_pixel_limit(w, h)?;

    // Full decode with the existing Limits-based alloc cap (defense in depth).
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let rgba = reader.decode()?.to_rgba8();
    let (width, height) = rgba.dimensions();
    DecodedImage::new(rgba.into_raw(), width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CoreError;
    use std::io::Cursor;

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255]));
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn decode_reports_dimensions_and_rgba_length() {
        let decoded = decode(&png_bytes(2, 3)).unwrap();
        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 3);
        assert_eq!(decoded.rgba().len(), (2 * 3 * 4) as usize);
    }

    #[test]
    fn decode_invalid_bytes_errors() {
        let err = decode(b"not an image").unwrap_err();
        assert!(matches!(err, CoreError::Decode(_)));
    }

    #[test]
    fn new_rejects_mismatched_rgba_length() {
        let err = DecodedImage::new(vec![0u8; 3], 2, 2).unwrap_err();
        assert!(matches!(
            err,
            CoreError::MalformedImage {
                expected: 16,
                actual: 3
            }
        ));
    }

    #[test]
    fn new_accepts_matching_rgba_length() {
        let img = DecodedImage::new(vec![0u8; 16], 2, 2).unwrap();
        assert_eq!((img.width(), img.height(), img.rgba().len()), (2, 2, 16));
    }

    // --- check_pixel_limit ---

    #[test]
    fn check_pixel_limit_exact_max_is_ok() {
        // 16384 * 8192 = 134_217_728 = 128 * 1024 * 1024 = MAX_PIXELS
        assert_eq!((16_384_u64) * (8_192_u64), MAX_PIXELS);
        assert!(check_pixel_limit(16_384, 8_192).is_ok());
    }

    #[test]
    fn check_pixel_limit_one_over_errors() {
        // MAX_PIXELS = 134_217_728; u32::MAX = 4_294_967_295 so h = 134_217_729 fits in u32.
        let h = (MAX_PIXELS + 1) as u32;
        let err = check_pixel_limit(1, h).unwrap_err();
        if let CoreError::ImageTooLarge {
            width,
            height,
            pixels,
            max,
        } = err
        {
            assert_eq!(width, 1);
            assert_eq!(height, MAX_PIXELS as u32 + 1);
            assert_eq!(pixels, MAX_PIXELS + 1);
            assert_eq!(max, MAX_PIXELS);
        } else {
            panic!("expected ImageTooLarge, got something else");
        }
    }

    #[test]
    fn check_pixel_limit_small_size_is_ok() {
        assert!(check_pixel_limit(100, 100).is_ok());
    }

    #[test]
    fn decode_small_png_succeeds_through_new_prepath() {
        // Verifies the new header-pre-read path does not break normal small images.
        let decoded = decode(&png_bytes(4, 4)).unwrap();
        assert_eq!(decoded.width(), 4);
        assert_eq!(decoded.height(), 4);
        assert_eq!(decoded.rgba().len(), 4 * 4 * 4);
    }
}
