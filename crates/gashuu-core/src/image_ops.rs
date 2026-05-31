//! Decoding helpers. Returns raw RGBA8 + dimensions so the presentation layer
//! can build a `slint::Image` without this crate depending on slint.

use crate::error::CoreError;

/// A decoded image as tightly packed RGBA8 bytes plus its dimensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    /// `width * height * 4` bytes, row-major, RGBA order.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Decode encoded image bytes (PNG/JPG/…) into RGBA8.
pub fn decode(bytes: &[u8]) -> Result<DecodedImage, CoreError> {
    let rgba = image::load_from_memory(bytes)?.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(DecodedImage {
        rgba: rgba.into_raw(),
        width,
        height,
    })
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
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 3);
        assert_eq!(decoded.rgba.len(), (2 * 3 * 4) as usize);
    }

    #[test]
    fn decode_invalid_bytes_errors() {
        let err = decode(b"not an image").unwrap_err();
        assert!(matches!(err, CoreError::Decode(_)));
    }
}
