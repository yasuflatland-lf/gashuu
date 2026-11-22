//! Page decoding port for the presentation layer.

use gashuu_core::{image_ops, CoreError, DecodedImage};
use std::future::{ready, Future};
use std::pin::Pin;

/// A local decode future that can run on the UI thread.
pub type DecodeFuture = Pin<Box<dyn Future<Output = Result<DecodedImage, CoreError>>>>;

pub trait PageDecoder {
    /// Decode one encoded page (JPEG/PNG/WebP/AVIF/GIF) to straight-alpha RGBA8 and dimensions.
    fn decode(&self, bytes: Vec<u8>) -> DecodeFuture;
}

/// Synchronous image-crate decoder that returns an already-ready future.
pub struct ImageCratePageDecoder;

impl PageDecoder for ImageCratePageDecoder {
    fn decode(&self, bytes: Vec<u8>) -> DecodeFuture {
        Box::pin(ready(image_ops::decode(&bytes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::task::{Context, Poll, Waker};

    fn poll_decode(bytes: Vec<u8>) -> Result<DecodedImage, CoreError> {
        let decoder: &dyn PageDecoder = &ImageCratePageDecoder;
        match decoder
            .decode(bytes)
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("image crate decode must be ready on the first poll"),
        }
    }

    #[test]
    fn decodes_png_to_the_same_rgba_and_dimensions_as_core() {
        let img = image::RgbaImage::from_fn(2, 3, |x, y| image::Rgba([12, x as u8, y as u8, 64]));
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let expected = gashuu_core::image_ops::decode(&bytes).unwrap();
        assert_eq!(poll_decode(bytes).unwrap(), expected);
    }

    #[test]
    fn invalid_bytes_return_decode_error() {
        assert!(matches!(
            poll_decode(vec![1, 2, 3]),
            Err(CoreError::Decode(_))
        ));
    }
}
