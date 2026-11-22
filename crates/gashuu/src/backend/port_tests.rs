use super::*;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::TryRecvError;

#[test]
fn backend_error_maps_core_variants_and_preserves_other_detail() {
    let core = CoreError::EmptyBook {
        label: "book".into(),
    };
    assert_eq!(BackendError::from_core(&core), BackendError::EmptyBook);
    let core = CoreError::FormatDisabled { format: "RAR" };
    let disabled = BackendError::from_core(&core);
    assert_eq!(disabled, BackendError::FormatDisabled { format: "RAR" });
    assert_eq!(disabled.to_string(), core.to_string());
    let core = CoreError::Io(std::io::Error::other("bad read"));
    assert_eq!(
        BackendError::from_core(&core),
        BackendError::Other {
            detail: core.to_string()
        }
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn backend_error_for_open_distinguishes_corrupt_and_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.cbz");
    std::fs::write(&path, b"invalid archive").unwrap();
    let error = crate::book_path::open(&path, ArchivePolicy::default())
        .err()
        .unwrap();
    let detail = error.to_string();
    assert_eq!(
        BackendError::for_open(&path, &error),
        BackendError::Other {
            detail: detail.clone()
        }
    );

    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        BackendError::for_open(&path, &error),
        BackendError::Missing { detail }
    );
}

#[test]
fn session_id_allocate_returns_distinct_increasing_ids() {
    let first = SessionId::allocate();
    let second = SessionId::allocate();
    let third = SessionId::allocate();
    assert!(first.0 < second.0 && second.0 < third.0);
}

#[test]
fn deliver_cover_drops_both_callbacks_when_not_current() {
    let (on_cover, cover) = test_callback();
    let (on_count, count) = test_callback();
    let image = Arc::new(DecodedImage::new(vec![0; 4], 1, 1).unwrap());
    deliver_cover(
        || false,
        on_cover,
        Some(on_count),
        Ok(image),
        Some(Ok(NonZeroUsize::MIN)),
    );
    assert_eq!(cover.try_recv(), Err(TryRecvError::Disconnected));
    assert_eq!(count.try_recv(), Err(TryRecvError::Disconnected));
}

#[test]
fn deliver_cover_rechecks_current_after_on_cover() {
    let current = Arc::new(AtomicBool::new(true));
    let current_on_cover = Arc::clone(&current);
    let (on_cover, cover) = test_callback();
    let (on_count, count) = test_callback();
    let image = Arc::new(DecodedImage::new(vec![0; 4], 1, 1).unwrap());
    deliver_cover(
        || current.load(Relaxed),
        Box::new(move |result| {
            current_on_cover.store(false, Relaxed);
            on_cover(result);
        }),
        Some(on_count),
        Ok(image),
        Some(Ok(NonZeroUsize::MIN)),
    );
    assert!(cover.try_recv().unwrap().is_ok());
    assert_eq!(count.try_recv(), Err(TryRecvError::Disconnected));
}

#[test]
fn deliver_cover_drops_count_callback_on_cover_error() {
    let (on_cover, cover) = test_callback();
    let (on_count, count) = test_callback();
    deliver_cover(
        || true,
        on_cover,
        Some(on_count),
        Err(BackendError::EmptyBook),
        None,
    );
    assert_eq!(cover.try_recv().unwrap(), Err(BackendError::EmptyBook));
    assert_eq!(count.try_recv(), Err(TryRecvError::Disconnected));
}

impl OpenedBook {
    /// Read a real fixture synchronously. Only the metadata escapes this backend.
    pub(crate) fn open_for_test(
        id: &BookId,
        policy: ArchivePolicy,
        cache: CacheConfig,
    ) -> Result<Self, BackendError> {
        let backend = desktop::DesktopBackend::new(Arc::new(gashuu_core::NullThumbStore));
        let mut opened = backend.open_blocking(id, policy, cache)?;
        backend.close(opened.session);
        // The fixture's fake backend owns a distinct handle for the installed metadata.
        opened.session = SessionId::allocate();
        Ok(opened)
    }
}
