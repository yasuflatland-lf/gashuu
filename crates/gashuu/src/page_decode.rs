//! Page decoding port for the presentation layer.

use gashuu_core::{cache::CacheDispatch, image_ops, CoreError, DecodedImage};
use std::cell::Cell;
use std::future::{ready, Future};
use std::pin::Pin;
use std::sync::Arc;

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

/// Resolve a cache miss with the primary decoder, retrying with the fallback on error.
pub async fn decode_page(
    dispatch: &CacheDispatch,
    index: usize,
    primary: &dyn PageDecoder,
    fallback: &dyn PageDecoder,
    warned: &Cell<bool>,
) -> Result<Arc<DecodedImage>, CoreError> {
    if let Some(hit) = dispatch.cached(index) {
        return Ok(hit);
    }
    let bytes = dispatch.read_raw(index)?;
    let result = primary.decode(bytes.clone()).await;
    if let Some(hit) = dispatch.cached(index) {
        return Ok(hit);
    }
    let decoded = match result {
        Ok(decoded) => decoded,
        Err(error) => {
            if !warned.replace(true) {
                tracing::warn!(index, %error, "primary page decode failed, retrying with fallback");
            }
            let result = fallback.decode(bytes).await;
            if let Some(hit) = dispatch.cached(index) {
                return Ok(hit);
            }
            result?
        }
    };
    Ok(dispatch.insert_decoded(index, decoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::{CacheConfig, ImageCache, MockPageSource, PageEntry};
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

    fn dispatch(pages: usize) -> CacheDispatch {
        let mut source = MockPageSource::new();
        source
            .expect_list_pages()
            .returning(move || vec![PageEntry { name: "p".into() }; pages]);
        source
            .expect_read_bytes()
            .times(pages)
            .returning(|index| Ok(vec![index as u8]));
        ImageCache::new(Arc::new(source), CacheConfig::new(pages, 0)).dispatch_handle()
    }

    fn pixel(value: u8) -> DecodedImage {
        DecodedImage::new(vec![value; 4], 1, 1).unwrap()
    }

    fn unexpected_decode(_: Vec<u8>) -> DecodeFuture {
        panic!("unexpected decoder call")
    }

    #[test]
    fn primary_success_is_cached_and_a_hit_skips_both_decoders() {
        let dispatch = dispatch(1);
        let warned = Cell::new(false);
        let primary = FakeDecoder(|bytes: Vec<u8>| ready(Ok(pixel(bytes[0]))));
        let unused = FakeDecoder(unexpected_decode);
        let page = resolve(decode_page(&dispatch, 0, &primary, &unused, &warned)).unwrap();
        assert_eq!(*page, pixel(0));
        assert!(Arc::ptr_eq(&page, &dispatch.cached(0).unwrap()));
        let hit = resolve(decode_page(&dispatch, 0, &unused, &unused, &warned)).unwrap();
        assert!(Arc::ptr_eq(&page, &hit));
        assert!(!warned.get());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn primary_failures_share_one_warning_and_retry_each_page() {
        use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
        let warnings = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&warnings);
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || {
                count.fetch_add(1, Relaxed);
                std::io::sink()
            })
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let dispatch = dispatch(3);
        let warned = Cell::new(false);
        let primary_calls = Cell::new(0);
        let fallback_calls = Cell::new(0);
        let primary = FakeDecoder(|_| {
            primary_calls.set(primary_calls.get() + 1);
            ready(Err(CoreError::UnsupportedFormat {
                label: "primary".into(),
            }))
        });
        let fallback = FakeDecoder(|bytes: Vec<u8>| {
            fallback_calls.set(fallback_calls.get() + 1);
            ready(Ok(pixel(bytes[0])))
        });
        for index in 0..3 {
            let page =
                resolve(decode_page(&dispatch, index, &primary, &fallback, &warned)).unwrap();
            assert_eq!(*page, pixel(index as u8));
            assert!(Arc::ptr_eq(&page, &dispatch.cached(index).unwrap()));
            assert!(warned.get());
        }
        assert_eq!((primary_calls.get(), fallback_calls.get()), (3, 3));
        assert_eq!(warnings.load(Relaxed), 1);
    }

    #[test]
    fn both_decoders_fail_with_the_fallback_error_and_leave_cache_empty() {
        let dispatch = dispatch(1);
        let primary = FakeDecoder(|_| {
            ready(Err(CoreError::UnsupportedFormat {
                label: "primary".into(),
            }))
        });
        let fallback = FakeDecoder(|_| {
            ready(Err(CoreError::MalformedImage {
                expected: 4,
                actual: 2,
            }))
        });
        let result = resolve(decode_page(
            &dispatch,
            0,
            &primary,
            &fallback,
            &Cell::new(false),
        ));
        assert!(matches!(
            result,
            Err(CoreError::MalformedImage {
                expected: 4,
                actual: 2
            })
        ));
        assert!(dispatch.cached(0).is_none());
    }

    #[test]
    fn cache_filled_during_either_await_wins_even_if_the_decoder_fails() {
        for fill_during_primary in [true, false] {
            let dispatch = dispatch(1);
            let warned = Cell::new(false);
            let fills_cache = FakeDecoder(|_| {
                let dispatch = dispatch.clone();
                async move {
                    dispatch.insert_decoded(0, pixel(42));
                    Err(CoreError::MalformedImage {
                        expected: 4,
                        actual: 0,
                    })
                }
            });
            let fails = FakeDecoder(|_| {
                ready(Err(CoreError::UnsupportedFormat {
                    label: "primary".into(),
                }))
            });
            let unused = FakeDecoder(unexpected_decode);
            let (primary, fallback): (&dyn PageDecoder, &dyn PageDecoder) = if fill_during_primary {
                (&fills_cache, &unused)
            } else {
                (&fails, &fills_cache)
            };
            let page = resolve(decode_page(&dispatch, 0, primary, fallback, &warned)).unwrap();
            assert!(Arc::ptr_eq(&page, &dispatch.cached(0).unwrap()));
            assert_eq!(*page, pixel(42));
            assert_eq!(warned.get(), !fill_during_primary);
        }
    }
}
