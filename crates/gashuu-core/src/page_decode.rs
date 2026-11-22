//! Decoders for encoded page images and their fallback order.
use crate::{image_ops, CoreError, DecodedImage};
use std::future::{ready, Future};
use std::pin::Pin;

/// A local decode future for a single-threaded executor.
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

#[derive(Debug)]
pub struct Decoded {
    pub image: DecodedImage,
    /// The primary failure when the fallback produced the image.
    pub primary_error: Option<CoreError>,
}

/// Retry the original bytes with the fallback if the primary decoder fails.
pub async fn decode_with_fallback(
    bytes: Vec<u8>,
    primary: &dyn PageDecoder,
    fallback: &dyn PageDecoder,
) -> Result<Decoded, CoreError> {
    decode_with_fallback_if(bytes, primary, fallback, |_| true).await
}

/// Consult the caller before retrying. A declined retry returns the primary error.
pub async fn decode_with_fallback_if(
    bytes: Vec<u8>,
    primary: &dyn PageDecoder,
    fallback: &dyn PageDecoder,
    should_retry: impl FnOnce(&CoreError) -> bool,
) -> Result<Decoded, CoreError> {
    match primary.decode(bytes.clone()).await {
        Ok(image) => Ok(Decoded {
            image,
            primary_error: None,
        }),
        Err(error) if should_retry(&error) => Ok(Decoded {
            image: fallback.decode(bytes).await?,
            primary_error: Some(error),
        }),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::task::{Context, Poll, Waker};

    fn resolve<F: Future>(future: F) -> F::Output {
        match std::pin::pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("expected an immediately ready decode"),
        }
    }

    fn poll_decode(bytes: Vec<u8>) -> Result<DecodedImage, CoreError> {
        let decoder: &dyn PageDecoder = &ImageCratePageDecoder;
        resolve(decoder.decode(bytes))
    }

    #[test]
    fn decodes_png_to_the_same_rgba_and_dimensions_as_core() {
        let img = image::RgbaImage::from_fn(2, 3, |x, y| image::Rgba([12, x as u8, y as u8, 64]));
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let expected = image_ops::decode(&bytes).unwrap();
        assert_eq!(poll_decode(bytes).unwrap(), expected);
    }

    #[test]
    fn invalid_bytes_return_decode_error() {
        assert!(matches!(
            poll_decode(vec![1, 2, 3]),
            Err(CoreError::Decode(_))
        ));
    }

    struct FakeDecoder<F>(F);

    impl<F, Fut> PageDecoder for FakeDecoder<F>
    where
        F: Fn(Vec<u8>) -> Fut,
        Fut: Future<Output = Result<DecodedImage, CoreError>> + 'static,
    {
        fn decode(&self, bytes: Vec<u8>) -> DecodeFuture {
            Box::pin((self.0)(bytes))
        }
    }

    fn success(bytes: Vec<u8>) -> std::future::Ready<Result<DecodedImage, CoreError>> {
        assert_eq!(bytes, [42]);
        ready(DecodedImage::new(vec![42; 4], 1, 1))
    }

    fn failure(label: &'static str) -> impl PageDecoder {
        FakeDecoder(move |bytes: Vec<u8>| {
            assert_eq!(bytes, [42]);
            ready(Err(CoreError::UnsupportedFormat {
                label: label.into(),
            }))
        })
    }

    #[test]
    fn primary_success_skips_fallback() {
        let unused = FakeDecoder(|_| -> DecodeFuture { panic!("unexpected fallback") });
        let decoded = resolve(decode_with_fallback(
            vec![42],
            &FakeDecoder(success),
            &unused,
        ))
        .unwrap();
        assert_eq!(decoded.image.rgba(), &[42; 4]);
        assert!(decoded.primary_error.is_none());
    }

    #[test]
    fn declined_retry_returns_primary_error_without_decoding_fallback() {
        let unused = FakeDecoder(|_| -> DecodeFuture { panic!("unexpected fallback") });
        let mut retry_calls = 0;
        let result = resolve(decode_with_fallback_if(
            vec![42],
            &failure("primary"),
            &unused,
            |error| {
                retry_calls += 1;
                assert!(
                    matches!(error, CoreError::UnsupportedFormat { label } if label == "primary")
                );
                false
            },
        ));
        assert_eq!(retry_calls, 1);
        assert!(
            matches!(result, Err(CoreError::UnsupportedFormat { label }) if label == "primary")
        );
    }

    #[test]
    fn primary_success_skips_retry_policy() {
        let unused = FakeDecoder(|_| -> DecodeFuture { panic!("unexpected fallback") });
        let decoded = resolve(decode_with_fallback_if(
            vec![42],
            &FakeDecoder(success),
            &unused,
            |_| panic!("unexpected retry policy call"),
        ))
        .unwrap();
        assert_eq!(decoded.image.rgba(), &[42; 4]);
        assert!(decoded.primary_error.is_none());
    }

    #[test]
    fn fallback_success_preserves_primary_error_and_bytes() {
        let decoded = resolve(decode_with_fallback(
            vec![42],
            &failure("primary"),
            &FakeDecoder(success),
        ))
        .unwrap();
        assert_eq!(decoded.image.rgba(), &[42; 4]);
        assert!(
            matches!(decoded.primary_error, Some(CoreError::UnsupportedFormat { label }) if label == "primary")
        );
    }

    #[test]
    fn both_fail_returns_fallback_error() {
        let result = resolve(decode_with_fallback(
            vec![42],
            &failure("primary"),
            &failure("fallback"),
        ));
        assert!(
            matches!(result, Err(CoreError::UnsupportedFormat { label }) if label == "fallback")
        );
    }
}
