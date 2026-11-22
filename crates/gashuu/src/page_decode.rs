//! Page cache and cover handling for the presentation layer.

use gashuu_core::{
    cache::CacheDispatch, decode_with_fallback_if, CoreError, DecodedImage, PageDecoder,
};
use std::cell::Cell;
use std::sync::Arc;

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
    let mut cached = None;
    let result = decode_with_fallback_if(bytes, primary, fallback, |error| {
        cached = dispatch.cached(index);
        if cached.is_some() {
            return false;
        }
        if !warned.replace(true) {
            tracing::warn!(index, %error, "primary page decode failed, retrying with fallback");
        }
        true
    })
    .await;
    if let Some(hit) = cached.or_else(|| dispatch.cached(index)) {
        return Ok(hit);
    }
    Ok(dispatch.insert_decoded(index, result?.image))
}

/// Read the first page of an already-opened book for its carousel cover.
#[cfg(test)]
pub(crate) async fn decode_cover(
    source: Arc<dyn gashuu_core::PageSource>,
    primary: &dyn PageDecoder,
    fallback: &dyn PageDecoder,
    max_side: u32,
) -> Result<DecodedImage, CoreError> {
    let decoded = decode_with_fallback_if(source.read_bytes(0)?, primary, fallback, |error| {
        tracing::warn!(%error, "primary cover decode failed, retrying with fallback");
        true
    })
    .await?;
    gashuu_core::image_ops::downscale_to_thumbnail(&decoded.image, max_side)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::{CacheConfig, DecodeFuture, ImageCache, MockPageSource, PageEntry};
    use std::future::{ready, Future};
    use std::task::{Context, Poll, Waker};

    fn resolve<F: Future>(future: F) -> F::Output {
        match std::pin::pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("expected an immediately ready decode"),
        }
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

    fn cover_source() -> Arc<dyn gashuu_core::PageSource> {
        let mut source = MockPageSource::new();
        source.expect_read_bytes().once().returning(|index| {
            assert_eq!(index, 0);
            Ok(vec![42])
        });
        Arc::new(source)
    }

    #[test]
    fn cover_downscales_page_zero_from_the_primary_decoder() {
        let primary = FakeDecoder(|bytes: Vec<u8>| {
            assert_eq!(bytes, [42]);
            ready(DecodedImage::new(vec![42; 8 * 4 * 4], 8, 4))
        });
        let cover = resolve(decode_cover(
            cover_source(),
            &primary,
            &FakeDecoder(unexpected_decode),
            4,
        ))
        .unwrap();
        assert_eq!((cover.width(), cover.height()), (4, 2));
        assert_eq!(cover.rgba(), &[42; 4 * 2 * 4]);
    }

    #[test]
    fn cover_retries_the_same_bytes_after_primary_failure() {
        let tried_primary = Cell::new(false);
        let primary = FakeDecoder(|bytes: Vec<u8>| {
            assert_eq!(bytes, [42]);
            tried_primary.set(true);
            ready(Err(CoreError::UnsupportedFormat {
                label: "primary".into(),
            }))
        });
        let fallback = FakeDecoder(|bytes: Vec<u8>| {
            assert!(tried_primary.get());
            assert_eq!(bytes, [42]);
            ready(Ok(pixel(bytes[0])))
        });
        let cover = resolve(decode_cover(cover_source(), &primary, &fallback, 4)).unwrap();
        assert_eq!(cover, pixel(42));
    }

    #[test]
    fn cover_propagates_the_fallback_decode_error() {
        let fails = |label: &'static str| {
            FakeDecoder(move |_| {
                ready(Err(CoreError::UnsupportedFormat {
                    label: label.into(),
                }))
            })
        };
        let result = resolve(decode_cover(
            cover_source(),
            &fails("primary"),
            &fails("fallback"),
            4,
        ));
        assert!(
            matches!(result, Err(CoreError::UnsupportedFormat { label }) if label == "fallback")
        );
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
