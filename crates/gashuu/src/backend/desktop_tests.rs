use super::*;
use crate::book_path::request_id;
use gashuu_core::{FsThumbStore, MockPageSource, NullThumbStore, PageEntry};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

struct Harness {
    root: tempfile::TempDir,
    backend: DesktopBackend,
    posted: Receiver<Box<dyn FnOnce() + Send>>,
}

impl Harness {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(FsThumbStore::with_dir(root.path().join("cache")));
        Self::with_store(root, store)
    }

    fn with_store(root: tempfile::TempDir, thumbs: Arc<dyn ThumbStore>) -> Self {
        let (tx, posted) = mpsc::channel();
        let post = Arc::new(move |f| tx.send(f).unwrap());
        Self {
            root,
            backend: DesktopBackend::with_parts(post, thumbs),
            posted,
        }
    }

    fn pump(&self) {
        self.posted.recv_timeout(Duration::from_secs(5)).unwrap()();
    }

    fn book(&self, pages: usize) -> PathBuf {
        super::super::test_book(self.root.path(), pages)
    }

    fn open(&self, path: &Path, policy: ArchivePolicy) -> Result<OpenedBook, BackendError> {
        let (done, result) = super::super::test_callback();
        self.backend
            .open(&request_id(path)?, policy, CacheConfig::new(6, 0), done);
        self.pump();
        result.try_recv().unwrap()
    }

    fn spread(&self, session: SessionId, trailing: Option<usize>) -> SpreadPages {
        let (done, result) = super::super::test_callback();
        self.backend.decode_spread(session, 0, trailing, done);
        self.pump();
        result.try_recv().unwrap()
    }

    fn mock_session(&self, path: PathBuf, source: MockPageSource) -> SessionId {
        let source: Arc<dyn PageSource> = Arc::new(source);
        let cache = ImageCache::new(Arc::clone(&source), CacheConfig::new(6, 0));
        let id = SessionId::allocate();
        self.backend.inner.sessions.lock().unwrap().insert(
            id,
            Arc::new(Session {
                id: request_id(&path).unwrap(),
                source,
                cache,
            }),
        );
        id
    }

    fn thumbnail(&self, session: SessionId) -> Arc<DecodedImage> {
        let (done, result) = super::super::test_callback();
        self.backend.thumbnail(session, 0, 2, done);
        self.pump();
        result.try_recv().unwrap().unwrap()
    }

    fn check_cover(&self, path: &Path, expected_count: Option<usize>) -> ImageResult {
        let (on_cover, cover) = super::super::test_callback();
        let (on_count, count) = super::super::test_callback();
        self.backend.cover(
            &request_id(path).unwrap(),
            ArchivePolicy::default(),
            512,
            on_cover,
            Some(on_count),
        );
        self.pump();
        if let Some(expected) = expected_count {
            match count.try_recv() {
                Err(mpsc::TryRecvError::Empty) => {
                    self.pump();
                    assert_eq!(count.try_recv().unwrap().unwrap().get(), expected);
                }
                ready => assert_eq!(ready.unwrap().unwrap().get(), expected),
            }
        } else {
            assert!(count.try_recv().is_err());
        }
        cover.try_recv().unwrap()
    }
}

#[test]
fn done_is_never_invoked_synchronously() {
    let h = Harness::new();
    let id = request_id(&h.book(1)).unwrap();
    let ui_thread = std::thread::current().id();
    let (done, result) = super::super::test_callback();
    h.backend.open(
        &id,
        ArchivePolicy::default(),
        CacheConfig::new(6, 0),
        Box::new(move |r| {
            assert_eq!(std::thread::current().id(), ui_thread);
            done(r);
        }),
    );
    assert!(result.try_recv().is_err());
    h.pump();
    assert!(result.try_recv().unwrap().is_ok());
}

#[test]
fn open_folder_reports_count_skipped_is_dir_and_canonical_id() {
    let h = Harness::new();
    let path = h.book(2);
    // A sparse oversized image tests a nonzero skipped count without writing its bytes.
    std::fs::File::create(path.join("oversized.png"))
        .unwrap()
        .set_len(501 * 1024 * 1024)
        .unwrap();
    let raw = path.join("..").join("book");
    let opened = h.open(&raw, ArchivePolicy::default()).unwrap();
    assert_eq!(
        opened.id,
        request_id(&path.canonicalize().unwrap()).unwrap()
    );
    assert_eq!(opened.page_count, 2);
    assert_eq!(opened.skipped, 1);
    assert!(opened.is_dir);
    assert!(!opened.listing_truncated);
}

#[test]
fn open_missing_path_is_missing_error() {
    let h = Harness::new();
    assert!(matches!(
        h.open(&h.root.path().join("missing.cbz"), ArchivePolicy::default()),
        Err(BackendError::Missing { .. })
    ));
}

#[test]
fn open_disabled_rar_is_format_disabled() {
    let h = Harness::new();
    let path = h.root.path().join("disabled.cbr");
    std::fs::write(&path, []).unwrap();
    assert!(matches!(
        h.open(&path, ArchivePolicy { allow_rar: false }),
        Err(BackendError::FormatDisabled { .. })
    ));
}

#[test]
fn open_empty_folder_is_ok_with_zero_pages() {
    let h = Harness::new();
    assert_eq!(
        h.open(&h.book(0), ArchivePolicy::default())
            .unwrap()
            .page_count,
        0
    );
}

#[test]
fn cached_page_miss_reads_nothing_then_hits_after_decode_spread() {
    let h = Harness::new();
    let path = h.book(1);
    let bytes = std::fs::read(path.join("000.png")).unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reads);
    let mut source = MockPageSource::new();
    source.expect_list_pages().returning(|| {
        vec![PageEntry {
            name: "000.png".into(),
        }]
    });
    source.expect_read_bytes().once().returning(move |_| {
        counter.fetch_add(1, Relaxed);
        Ok(bytes.clone())
    });
    let session = h.mock_session(path, source);
    assert!(h.backend.cached_page(session, 0).is_none());
    assert_eq!(reads.load(Relaxed), 0);
    let page = h.spread(session, None).leading.1.unwrap();
    assert_eq!(reads.load(Relaxed), 1);
    assert!(Arc::ptr_eq(
        &page,
        &h.backend.cached_page(session, 0).unwrap()
    ));
}

#[test]
fn decode_spread_single_returns_decoded_dims() {
    let h = Harness::new();
    let session = h
        .backend
        .open_blocking(
            &request_id(&h.book(1)).unwrap(),
            ArchivePolicy::default(),
            CacheConfig::new(6, 0),
        )
        .unwrap()
        .session;
    let spread = h.spread(session, None);
    let (index, page) = spread.leading;
    let page = page.unwrap();
    assert_eq!((index, page.width(), page.height()), (0, 4, 2));
    assert!(spread.trailing.is_none());
}

#[test]
fn decode_spread_propagates_leading_error() {
    let h = Harness::new();
    let path = h.book(1);
    std::fs::write(path.join("000.png"), b"bad png").unwrap();
    let session = h.open(&path, ArchivePolicy::default()).unwrap().session;
    assert!(matches!(
        h.spread(session, None).leading.1,
        Err(BackendError::Other { .. })
    ));
}

#[test]
fn decode_spread_trailing_error_keeps_leading() {
    let h = Harness::new();
    let session = h
        .open(&h.book(1), ArchivePolicy::default())
        .unwrap()
        .session;
    let spread = h.spread(session, Some(1));
    assert!(spread.leading.1.is_ok());
    assert!(matches!(
        spread.trailing,
        Some((1, Err(BackendError::Other { .. })))
    ));
}

#[test]
fn close_drops_pending_callbacks() {
    let h = Harness::new();
    let session = h
        .open(&h.book(1), ArchivePolicy::default())
        .unwrap()
        .session;
    h.backend.decode_spread(
        session,
        0,
        None,
        Box::new(|_| panic!("closed spread delivered")),
    );
    let spread = h.posted.recv_timeout(Duration::from_secs(5)).unwrap();
    h.backend.thumbnail(
        session,
        0,
        2,
        Box::new(|_| panic!("closed thumbnail delivered")),
    );
    let thumbnail = h.posted.recv_timeout(Duration::from_secs(5)).unwrap();
    h.backend.close(session);
    spread();
    thumbnail();
    assert!(h.backend.cached_page(session, 0).is_none());
}

#[test]
fn close_unknown_session_is_noop() {
    Harness::new().backend.close(SessionId::for_test(0));
}

#[test]
fn thumbnail_respects_max_side_and_persists() {
    let h = Harness::new();
    let path = h.book(1).canonicalize().unwrap();
    let session = h.open(&path, ArchivePolicy::default()).unwrap().session;
    let image = h.thumbnail(session);
    assert_eq!((image.width(), image.height()), (2, 1));
    let mut source = MockPageSource::new();
    source.expect_list_pages().returning(Vec::new);
    source.expect_read_bytes().never();
    let session = h.mock_session(path, source);
    assert_eq!(h.thumbnail(session), image);
}

#[test]
fn null_store_backend_still_generates_covers_and_thumbnails() {
    let h = Harness::with_store(tempfile::tempdir().unwrap(), Arc::new(NullThumbStore));
    let path = h.book(1);
    let cover = h.check_cover(&path, Some(1)).unwrap();
    assert_eq!((cover.width(), cover.height()), (4, 2));
    let session = h.open(&path, ArchivePolicy::default()).unwrap().session;
    let thumb = h.thumbnail(session);
    assert_eq!((thumb.width(), thumb.height()), (2, 1));
}

#[test]
fn cover_miss_delivers_cover_and_count() {
    let h = Harness::new();
    assert!(h.check_cover(&h.book(2), Some(2)).is_ok());
}

#[test]
fn cover_miss_with_rar_disabled_reports_format_disabled_without_count() {
    let h = Harness::new();
    let path = h.root.path().join("disabled.cbr");
    std::fs::write(&path, []).unwrap();
    let (on_cover, cover) = super::super::test_callback();
    let (on_count, count) = super::super::test_callback();
    h.backend.cover(
        &request_id(&path).unwrap(),
        ArchivePolicy { allow_rar: false },
        512,
        on_cover,
        Some(on_count),
    );
    h.pump();
    assert_eq!(
        cover.try_recv().unwrap().unwrap_err(),
        BackendError::FormatDisabled { format: "rar/cbr" }
    );
    assert_eq!(
        count.recv_timeout(Duration::from_secs(5)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}

#[test]
fn cover_hit_with_rar_disabled_reports_format_disabled_through_count() {
    let h = Harness::new();
    let path = h.root.path().join("disabled.cbr");
    std::fs::write(&path, []).unwrap();
    let id = request_id(&path).unwrap();
    let key = h.backend.inner.thumbs.fingerprint(&id).cover_key(512);
    let image = DecodedImage::new(vec![0; 4], 1, 1).unwrap();
    h.backend.inner.cache_cover(key, image.clone());
    let (on_cover, cover) = super::super::test_callback();
    let (on_count, count) = super::super::test_callback();
    h.backend.cover(
        &id,
        ArchivePolicy { allow_rar: false },
        512,
        on_cover,
        Some(on_count),
    );
    h.pump();
    assert_eq!(*cover.try_recv().unwrap().unwrap(), image);
    h.pump();
    assert_eq!(
        count.try_recv().unwrap(),
        Err(BackendError::FormatDisabled { format: "rar/cbr" })
    );
}

#[test]
fn cover_empty_drops_count() {
    let h = Harness::new();
    assert!(matches!(
        h.check_cover(&h.book(0), None),
        Err(BackendError::EmptyBook)
    ));
}

#[test]
fn cover_unreadable_drops_count() {
    let h = Harness::new();
    let path = h.root.path().join("bad.cbz");
    std::fs::write(&path, b"invalid archive").unwrap();
    assert!(matches!(
        h.check_cover(&path, None),
        Err(BackendError::Other { .. } | BackendError::Missing { .. })
    ));
}

#[test]
fn cover_disk_hit_resolves_count() {
    let h = Harness::new();
    let path = h.book(2);
    let expected = h.check_cover(&path, Some(2)).unwrap();
    *h.backend.inner.cover_mem.lock().unwrap() = CoverMemCache::new();
    std::fs::write(path.join("000.png"), b"bad png").unwrap();
    assert_eq!(h.check_cover(&path, Some(2)).unwrap(), expected);
}

#[test]
fn cover_hit_reports_empty_only_through_count() {
    let h = Harness::new();
    let path = h.book(0);
    let key = h
        .backend
        .inner
        .thumbs
        .fingerprint(&request_id(&path).unwrap())
        .cover_key(512);
    h.backend
        .inner
        .cache_cover(key, DecodedImage::new(vec![0; 4], 1, 1).unwrap());
    let (on_cover, cover) = super::super::test_callback();
    let (on_count, count) = super::super::test_callback();
    h.backend.cover(
        &request_id(&path).unwrap(),
        ArchivePolicy::default(),
        512,
        on_cover,
        Some(on_count),
    );
    h.pump();
    assert!(cover.try_recv().unwrap().is_ok());
    assert!(matches!(count.try_recv(), Err(mpsc::TryRecvError::Empty)));
    h.pump();
    assert_eq!(count.try_recv().unwrap(), Err(BackendError::EmptyBook));
}

#[test]
fn cancel_covers_before_pump_drops_both_callbacks() {
    let h = Harness::new();
    let (on_cover, cover) = super::super::test_callback();
    let (on_count, count) = super::super::test_callback();
    h.backend.cover(
        &request_id(&h.book(2)).unwrap(),
        ArchivePolicy::default(),
        512,
        on_cover,
        Some(on_count),
    );
    let posted_cover = h.posted.recv_timeout(Duration::from_secs(5)).unwrap();
    let posted_count = h.posted.recv_timeout(Duration::from_secs(5)).unwrap();
    h.backend.cancel_covers();
    posted_cover();
    posted_count();
    assert!(cover.try_recv().is_err());
    assert!(count.try_recv().is_err());
}

#[test]
fn probe_page_count_classifies_counted_empty_disabled_unreadable() {
    let h = Harness::new();
    let book = h.book(2);
    let empty = h.root.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let disabled = h.root.path().join("disabled.cbr");
    std::fs::write(&disabled, []).unwrap();
    let bad = h.root.path().join("bad.cbz");
    std::fs::write(&bad, b"invalid archive").unwrap();

    let probe = |path: &Path| {
        let (done, result) = super::super::test_callback();
        h.backend.probe_page_count(
            &request_id(path).unwrap(),
            ArchivePolicy { allow_rar: false },
            done,
        );
        h.pump();
        result.try_recv().unwrap()
    };
    assert_eq!(probe(&book).unwrap().get(), 2);
    assert_eq!(probe(&empty), Err(BackendError::EmptyBook));
    assert!(matches!(
        probe(&disabled),
        Err(BackendError::FormatDisabled { .. })
    ));
    assert!(matches!(probe(&bad), Err(BackendError::Other { .. })));
}
