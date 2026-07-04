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

/// Shared two-layer image-bomb guard: header pre-read + `check_pixel_limit` +
/// `Limits`-bounded full decode. Returns the decoded [`image::DynamicImage`] so
/// callers can choose the final pixel format without duplicating the guard logic.
///
/// `image::Limits` is `#[non_exhaustive]`, so struct-literal init is impossible;
/// we must use field assignment after `default()`, which triggers
/// `clippy::field_reassign_with_default`. The allow is scoped to this function only.
#[allow(clippy::field_reassign_with_default)]
fn decode_dynamic(bytes: &[u8]) -> Result<image::DynamicImage, CoreError> {
    // Pre-read header to check dimensions before allocating. AVIF excepted: its decoder
    // fully decodes in the ctor, so its pixel guard runs post-decode instead (ADR-0010).
    let header_reader =
        image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    if header_reader.format() != Some(image::ImageFormat::Avif) {
        let (w, h) = header_reader.into_dimensions()?;
        check_pixel_limit(w, h)?;
    }

    // Full decode with the existing Limits-based alloc cap (defense in depth).
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode()?;
    // Post-decode guard: the only pixel-limit check for AVIF (see above); for
    // other formats it is redundant with the pre-read, but cheap.
    check_pixel_limit(img.width(), img.height())?;
    Ok(img)
}

/// Decode encoded image bytes into RGBA8 using the `image` crate.
///
/// Supports any format recognized by `image::ImageReader` (PNG, JPEG, AVIF, …).
/// Decoder limits reject oversize / decompression-bomb images before allocation.
///
/// Defense in depth: a lightweight header-only pre-read via `into_dimensions()`
/// runs [`check_pixel_limit`] before the full decode allocates any pixel memory —
/// except for AVIF, whose decoder only learns dimensions by fully decoding, so
/// its guard runs once, post-decode (see `decode_dynamic`). The existing
/// `image::Limits` alloc cap is kept as a second layer.
pub fn decode(bytes: &[u8]) -> Result<DecodedImage, CoreError> {
    let img = decode_dynamic(bytes)?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    DecodedImage::new(rgba.into_raw(), width, height)
}

/// Decode `bytes` and downscale to a thumbnail whose longer edge is at most
/// `max_side` px, preserving aspect ratio. Reuses the same two-layer bomb guard
/// as `decode` (header pre-read + check_pixel_limit + Limits-bounded full decode)
/// via the shared `decode_dynamic`: the source page is fully decoded before
/// downscaling, so peak RAM per call is one full-res page — the same as `decode`,
/// which bounds memory under the rayon pool's parallelism.
///
/// JPEG scale-on-decode (DCT 1/2, 1/4, 1/8 reduction) was investigated as a
/// fast path for the common manga case. The pinned `image` 0.25 decodes JPEG via
/// `zune-jpeg` (see `Cargo.lock`: `image` 0.25.x → `zune-jpeg` 0.5.x), and that
/// backend exposes no scaled-decode API: `image::codecs::jpeg::JpegDecoder` has
/// only `new`, and `zune_core::options::DecoderOptions` offers max-dimension
/// rejection limits, not power-of-two reduction. The old `JpegDecoder::scale`
/// from the pre-0.25 `jpeg-decoder` backend was dropped in that migration. The
/// fast path is an optimization, never a correctness requirement, so we keep the
/// full-decode path for JPEG too until the pinned backend regains scaled decode.
pub fn decode_thumbnail(bytes: &[u8], max_side: u32) -> Result<DecodedImage, CoreError> {
    let img = decode_dynamic(bytes)?;
    // Downscale only: `DynamicImage::thumbnail` upscales small images to fill the
    // bounds, so guard explicitly and return already-fitting images unchanged.
    let rgba = if img.width() > max_side || img.height() > max_side {
        img.thumbnail(max_side, max_side).to_rgba8()
    } else {
        img.to_rgba8()
    };
    let (w, h) = rgba.dimensions();
    DecodedImage::new(rgba.into_raw(), w, h)
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

    /// The 8×6 AVIF fixture used by the decode tests below. gashuu builds `image`
    /// WITHOUT its `avif` encode feature (only `avif-native`/dav1d decode), so
    /// AVIF bytes can no longer be synthesized in-process like `png_bytes`; the
    /// fixture is committed as base64 text in `test_fixtures` alongside the RAR
    /// blobs (same "no in-tree encoder" rationale). Decoding it exercises the
    /// `avif-native` (dav1d) path.
    fn avif_bytes() -> Vec<u8> {
        crate::test_fixtures::avif_8x6_bytes()
    }

    /// Encode an RGB JPEG in-process, mirroring `png_bytes` — no committed binary
    /// fixtures. JPEG has no alpha channel, so this builds an `RgbImage`. JPEG is
    /// lossy, so callers assert structural properties (dimensions, aspect) rather
    /// than pixel-exact values.
    fn jpeg_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([120, 60, 30]));
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
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

    /// Standard CRC-32/ISO-HDLC (polynomial 0xEDB88320, the one PNG uses) over a
    /// byte slice. Used to repair the IHDR chunk CRC after forging its dimensions.
    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// Build a tiny valid PNG, then forge its IHDR width/height to `(w, h)`
    /// WITHOUT allocating a giant buffer, repairing the IHDR CRC so the decoder
    /// accepts the header on the dimension pre-read. Used to drive the early
    /// `check_pixel_limit` guard from a tiny fixture. The byte-offset literals for
    /// the IHDR chunk live here, in ONE place.
    ///
    /// PNG layout: 8-byte signature, then the IHDR chunk:
    ///   [8..12]  length (always 13 for IHDR)
    ///   [12..16] chunk type ("IHDR")
    ///   [16..20] width (big-endian u32)
    ///   [20..24] height (big-endian u32)
    ///   ... 5 more data bytes (bit depth, color type, etc.)
    ///   [29..33] IHDR CRC over the type + 13 data bytes ([12..29])
    fn forge_oversized_ihdr(w: u32, h: u32) -> Vec<u8> {
        let mut bytes = png_bytes(1, 1);
        bytes[16..20].copy_from_slice(&w.to_be_bytes());
        bytes[20..24].copy_from_slice(&h.to_be_bytes());
        // Recompute the IHDR CRC over the chunk-type + data bytes ([12..29]).
        let new_crc = crc32(&bytes[12..29]);
        bytes[29..33].copy_from_slice(&new_crc.to_be_bytes());
        bytes
    }

    #[test]
    fn decode_rejects_oversized_header_with_image_too_large() {
        // Guards the `check_pixel_limit` wiring inside `decode()` that the pure unit
        // test misses. The forged header declares 1 x (MAX_PIXELS + 1) pixels.
        let bytes = forge_oversized_ihdr(1, (MAX_PIXELS + 1) as u32);

        let err = decode(&bytes).unwrap_err();
        assert!(
            matches!(err, CoreError::ImageTooLarge { .. }),
            "expected ImageTooLarge, got {err:?}"
        );
    }

    // --- decode_thumbnail ---

    #[test]
    fn decode_thumbnail_wide_image_fits_max_side() {
        // 200x100: longer edge is width=200. With max_side=64, width should become 64
        // and height should be 32 (aspect ratio 2:1 preserved within ±1px).
        let bytes = png_bytes(200, 100);
        let thumb = decode_thumbnail(&bytes, 64).unwrap();
        assert!(thumb.width() <= 64, "width {} > max_side 64", thumb.width());
        assert!(
            thumb.height() <= 64,
            "height {} > max_side 64",
            thumb.height()
        );
        // Aspect ratio check: width should be roughly 2x height (within ±1px tolerance).
        let expected_height = thumb.width() / 2;
        let diff = (thumb.height() as i64 - expected_height as i64).unsigned_abs();
        assert!(
            diff <= 1,
            "aspect ratio not preserved: w={} h={} expected_h~={}",
            thumb.width(),
            thumb.height(),
            expected_height
        );
    }

    #[test]
    fn decode_thumbnail_tall_image_fits_max_side() {
        // 100x200: longer edge is height=200. With max_side=64, height should become 64
        // and width should be 32 (aspect ratio 1:2 preserved within ±1px).
        let bytes = png_bytes(100, 200);
        let thumb = decode_thumbnail(&bytes, 64).unwrap();
        assert!(thumb.width() <= 64, "width {} > max_side 64", thumb.width());
        assert!(
            thumb.height() <= 64,
            "height {} > max_side 64",
            thumb.height()
        );
        // Aspect ratio check: height should be roughly 2x width (within ±1px tolerance).
        let expected_width = thumb.height() / 2;
        let diff = (thumb.width() as i64 - expected_width as i64).unsigned_abs();
        assert!(
            diff <= 1,
            "aspect ratio not preserved: w={} h={} expected_w~={}",
            thumb.width(),
            thumb.height(),
            expected_width
        );
    }

    #[test]
    fn decode_thumbnail_square_image_fits_max_side() {
        // 100x100: square. With max_side=64, both sides should be 64.
        let bytes = png_bytes(100, 100);
        let thumb = decode_thumbnail(&bytes, 64).unwrap();
        assert!(thumb.width() <= 64, "width {} > max_side 64", thumb.width());
        assert!(
            thumb.height() <= 64,
            "height {} > max_side 64",
            thumb.height()
        );
    }

    #[test]
    fn decode_thumbnail_does_not_upscale() {
        // Source 20x10 fits within max_side=64, so the downscale-only guard returns it
        // unchanged (`DynamicImage::thumbnail` would otherwise upscale to fill).
        let bytes = png_bytes(20, 10);
        let thumb = decode_thumbnail(&bytes, 64).unwrap();
        assert_eq!(
            thumb.width(),
            20,
            "thumbnail upscaled width: got {}",
            thumb.width()
        );
        assert_eq!(
            thumb.height(),
            10,
            "thumbnail upscaled height: got {}",
            thumb.height()
        );
    }

    // --- JPEG thumbnails (the common manga case; see decode_thumbnail docs) ---

    #[test]
    fn decode_thumbnail_jpeg_wide_image_fits_max_side() {
        // 200x100 JPEG: with max_side=64 the longer edge is bounded by 64 and the 2:1
        // aspect preserved. JPEG is lossy (8/16-px MCU blocks), so allow ±2px.
        let bytes = jpeg_bytes(200, 100);
        let thumb = decode_thumbnail(&bytes, 64).unwrap();
        assert!(thumb.width() <= 64, "width {} > max_side 64", thumb.width());
        assert!(
            thumb.height() <= 64,
            "height {} > max_side 64",
            thumb.height()
        );
        let expected_height = thumb.width() / 2;
        let diff = (thumb.height() as i64 - expected_height as i64).unsigned_abs();
        assert!(
            diff <= 2,
            "aspect ratio not preserved: w={} h={} expected_h~={}",
            thumb.width(),
            thumb.height(),
            expected_height
        );
    }

    #[test]
    fn decode_thumbnail_jpeg_does_not_upscale() {
        // Source 24x16 JPEG already fits within max_side=64, so the downscale-only
        // guard must return it at its original decoded dimensions (no upscaling).
        let bytes = jpeg_bytes(24, 16);
        let thumb = decode_thumbnail(&bytes, 64).unwrap();
        assert_eq!(
            (thumb.width(), thumb.height()),
            (24, 16),
            "jpeg thumbnail upscaled: got {}x{}",
            thumb.width(),
            thumb.height()
        );
    }

    #[test]
    fn decode_thumbnail_invalid_bytes_errors() {
        let err = decode_thumbnail(b"not an image", 64).unwrap_err();
        assert!(
            matches!(err, CoreError::Decode(_)),
            "expected Decode error, got {err:?}"
        );
    }

    // --- AVIF (avif-native / dav1d) ---

    /// Load-bearing gate for the `avif-native` feature: without it,
    /// `decode` returns `Err(Decode(_))` for AVIF bytes and this test fails.
    /// AVIF is lossy, so only structural properties are asserted (dimensions
    /// and RGBA length), not pixel-exact values.
    #[test]
    fn decode_avif_reports_dimensions_and_rgba_length() {
        let decoded = decode(&avif_bytes()).unwrap();
        assert_eq!(decoded.width(), 8);
        assert_eq!(decoded.height(), 6);
        assert_eq!(decoded.rgba().len(), 8 * 6 * 4);
    }

    #[test]
    fn decode_thumbnail_avif_fits_max_side() {
        let thumb = decode_thumbnail(&avif_bytes(), 4).unwrap();
        assert!(thumb.width() <= 4, "width {} > max_side 4", thumb.width());
        assert!(
            thumb.height() <= 4,
            "height {} > max_side 4",
            thumb.height()
        );
    }

    #[test]
    fn decode_thumbnail_rejects_oversized_ihdr_with_image_too_large() {
        // Verifies decode_thumbnail routes through decode_dynamic and hits the same
        // check_pixel_limit guard as decode(), via the shared forge_oversized_ihdr harness.
        let bytes = forge_oversized_ihdr(1, (MAX_PIXELS + 1) as u32);

        let err = decode_thumbnail(&bytes, 64).unwrap_err();
        assert!(
            matches!(err, CoreError::ImageTooLarge { .. }),
            "expected ImageTooLarge, got {err:?}"
        );
    }
}
