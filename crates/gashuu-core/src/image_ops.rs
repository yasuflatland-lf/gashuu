//! Decoding helpers. Returns raw RGBA8 + dimensions so the presentation layer
//! can build a `slint::Image` without this crate depending on slint.

use crate::error::CoreError;

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
/// `image::Limits` is `#[non_exhaustive]`, so struct-literal init is impossible;
/// we must use field assignment after `default()`, which triggers
/// `clippy::field_reassign_with_default`. The allow is scoped to this function only.
#[allow(clippy::field_reassign_with_default)]
pub fn decode(bytes: &[u8]) -> Result<DecodedImage, CoreError> {
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
}
