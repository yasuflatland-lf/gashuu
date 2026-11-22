use super::*;
use crate::book_path::request_id;
use gashuu_core::{CoreError, DecodeFuture, ImageCratePageDecoder};
use std::future::{poll_fn, ready};
use std::sync::mpsc::TryRecvError;
use std::task::{Context, Poll, Waker};

#[derive(Default)]
struct Queue(RefCell<VecDeque<LocalTask>>);

impl Queue {
    fn backend(self: &Rc<Self>, primary: Rc<dyn PageDecoder>) -> LocalWebBackend {
        let queue = Rc::downgrade(self);
        LocalWebBackend::with_parts(
            Rc::new(move |task| queue.upgrade().unwrap().0.borrow_mut().push_back(task)),
            primary,
            Rc::new(ImageCratePageDecoder),
        )
    }

    fn pump(&self) -> bool {
        let mut task = self.0.borrow_mut().pop_front().unwrap();
        if task
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
        {
            self.0.borrow_mut().push_back(task);
            return false;
        }
        true
    }
}

struct Harness {
    dir: tempfile::TempDir,
    id: BookId,
    queue: Rc<Queue>,
    backend: LocalWebBackend,
}

impl Harness {
    fn new(pages: usize, primary: Rc<dyn PageDecoder>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let id = request_id(&super::super::test_book(dir.path(), pages)).unwrap();
        let queue = Rc::new(Queue::default());
        let backend = queue.backend(primary);
        Self {
            dir,
            id,
            queue,
            backend,
        }
    }

    fn png(pages: usize) -> Self {
        Self::new(pages, Rc::new(ImageCratePageDecoder))
    }

    fn open(&self) -> OpenedBook {
        let (done, result) = super::super::test_callback();
        self.backend.open(
            &self.id,
            ArchivePolicy::default(),
            CacheConfig::new(6, 0),
            done,
        );
        assert!(result.try_recv().is_err());
        assert!(self.queue.pump());
        result.try_recv().unwrap().unwrap()
    }

    fn spread(&self, session: SessionId, index: usize, trailing: Option<usize>) -> SpreadPages {
        let (done, result) = super::super::test_callback();
        self.backend.decode_spread(session, index, trailing, done);
        assert!(result.try_recv().is_err());
        assert!(self.queue.pump());
        result.try_recv().unwrap()
    }

    fn cover_without_count(&self) -> Arc<DecodedImage> {
        let (done, result) = super::super::test_callback();
        self.backend.cover(&self.id, 512, done, None);
        assert_eq!(result.try_recv(), Err(TryRecvError::Empty));
        assert!(self.queue.pump());
        result.try_recv().unwrap().unwrap()
    }
}

#[test]
fn local_open_reports_count_and_never_resolves_synchronously() {
    let h = Harness::png(2);
    let opened = h.open();
    assert_eq!(opened.page_count, 2);
    assert_eq!(opened.id, h.id);
    assert!(!opened.is_dir);
}

#[test]
fn local_open_reports_missing_location() {
    let h = Harness::png(1);
    let id = request_id(&h.dir.path().join("missing.cbz")).unwrap();
    let (done, result) = super::super::test_callback();
    h.backend
        .open(&id, ArchivePolicy::default(), CacheConfig::new(6, 0), done);
    assert!(h.queue.pump());
    assert!(matches!(
        result.try_recv().unwrap(),
        Err(BackendError::Missing { .. })
    ));
}

#[test]
fn local_decode_spread_delivers_both_slots_together() {
    let h = Harness::png(2);
    let session = h.open().session;
    let spread = h.spread(session, 0, Some(1));
    assert_eq!(spread.leading.0, 0);
    assert!(spread.leading.1.is_ok());
    let (index, page) = spread.trailing.unwrap();
    assert_eq!(index, 1);
    assert!(page.is_ok());
}

#[test]
fn decode_spread_trailing_error_keeps_leading() {
    let h = Harness::png(1);
    let session = h.open().session;
    let spread = h.spread(session, 0, Some(1));
    assert_eq!(spread.leading.0, 0);
    assert!(spread.leading.1.is_ok());
    assert!(matches!(
        spread.trailing,
        Some((1, Err(BackendError::Other { .. })))
    ));
}

struct FailingDecoder(Cell<usize>);

impl PageDecoder for FailingDecoder {
    fn decode(&self, _: Vec<u8>) -> DecodeFuture {
        self.0.set(self.0.get() + 1);
        Box::pin(ready(Err(CoreError::UnsupportedFormat {
            label: "primary".into(),
        })))
    }
}

#[test]
fn local_decode_falls_back_to_the_second_decoder() {
    let primary = Rc::new(FailingDecoder(Cell::new(0)));
    let h = Harness::new(1, primary.clone());
    let session = h.open().session;
    let page = h.spread(session, 0, None).leading.1.unwrap();
    assert_eq!(primary.0.get(), 1);
    assert_eq!((page.width(), page.height()), (4, 2));
}

#[derive(Default)]
struct GatedDecoder {
    started: Cell<usize>,
    ready: Rc<Cell<bool>>,
}

impl PageDecoder for GatedDecoder {
    fn decode(&self, bytes: Vec<u8>) -> DecodeFuture {
        self.started.set(self.started.get() + 1);
        let ready = Rc::clone(&self.ready);
        Box::pin(poll_fn(move |_| {
            if ready.get() {
                Poll::Ready(gashuu_core::image_ops::decode(&bytes))
            } else {
                Poll::Pending
            }
        }))
    }
}

#[test]
fn decode_spread_closed_before_trailing_drops_done() {
    let decoder = Rc::new(GatedDecoder::default());
    let h = Harness::new(2, decoder.clone());
    let session = h.open().session;
    let (done, result) = super::super::test_callback();
    h.backend.decode_spread(session, 0, Some(1), done);
    assert!(!h.queue.pump());
    assert_eq!(decoder.started.get(), 1);

    h.backend.close(session);
    decoder.ready.set(true);
    assert!(h.queue.pump());
    assert_eq!(decoder.started.get(), 1);
    assert!(matches!(result.try_recv(), Err(TryRecvError::Disconnected)));
}

#[test]
fn local_cover_mem_reuses_decoded_cover_without_count() {
    let decoder = Rc::new(GatedDecoder::default());
    decoder.ready.set(true);
    let h = Harness::new(1, decoder.clone());
    let cached = h.cover_without_count();
    assert_eq!(decoder.started.get(), 1);

    let hit = h.cover_without_count();
    assert!(Arc::ptr_eq(&cached, &hit));
    assert_eq!(decoder.started.get(), 1);

    std::fs::remove_dir_all(book_path::locate(&h.id)).unwrap();
    assert!(Arc::ptr_eq(&cached, &h.cover_without_count()));
}

#[test]
fn local_cover_mem_hit_with_count_reopens_book() {
    let h = Harness::png(1);
    let cached = h.cover_without_count();
    let path = book_path::locate(&h.id);
    std::fs::copy(path.join("000.png"), path.join("001.png")).unwrap();

    let (on_cover, cover) = super::super::test_callback();
    let (on_count, count) = super::super::test_callback();
    h.backend.cover(&h.id, 512, on_cover, Some(on_count));
    assert!(h.queue.pump());
    assert!(Arc::ptr_eq(&cached, &cover.try_recv().unwrap().unwrap()));
    assert_eq!(count.try_recv().unwrap().unwrap().get(), 2);
}

#[test]
fn local_cover_mem_hit_reports_empty_only_through_count() {
    let h = Harness::png(1);
    let cached = h.cover_without_count();
    std::fs::remove_file(book_path::locate(&h.id).join("000.png")).unwrap();

    let (on_cover, cover) = super::super::test_callback();
    let (on_count, count) = super::super::test_callback();
    h.backend.cover(&h.id, 512, on_cover, Some(on_count));
    assert!(h.queue.pump());
    assert!(Arc::ptr_eq(&cached, &cover.try_recv().unwrap().unwrap()));
    assert_eq!(count.try_recv().unwrap(), Err(BackendError::EmptyBook));
}

#[test]
fn local_cover_empty_miss_drops_count() {
    let h = Harness::png(0);
    let (on_cover, cover) = super::super::test_callback();
    let (on_count, count) = super::super::test_callback();
    h.backend.cover(&h.id, 512, on_cover, Some(on_count));
    assert!(h.queue.pump());
    assert_eq!(cover.try_recv().unwrap(), Err(BackendError::EmptyBook));
    assert_eq!(count.try_recv(), Err(TryRecvError::Disconnected));
}

#[test]
fn local_cover_unreadable_miss_drops_count() {
    let h = Harness::png(0);
    let id = request_id(&h.dir.path().join("missing")).unwrap();
    let (on_cover, cover) = super::super::test_callback();
    let (on_count, count) = super::super::test_callback();
    h.backend.cover(&id, 512, on_cover, Some(on_count));
    assert!(h.queue.pump());
    assert!(matches!(
        cover.try_recv().unwrap(),
        Err(BackendError::Missing { .. })
    ));
    assert_eq!(count.try_recv(), Err(TryRecvError::Disconnected));
}

#[test]
fn local_cover_queue_has_one_decode_in_flight_across_cancel() {
    let decoder = Rc::new(GatedDecoder::default());
    let h = Harness::new(2, decoder.clone());
    let (cancelled, cancelled_cover) = super::super::test_callback();
    let (queued, queued_cover) = super::super::test_callback();
    let (cancelled_on_count, cancelled_count) = super::super::test_callback();
    let (latest, latest_cover) = super::super::test_callback();
    let (latest_on_count, latest_count) = super::super::test_callback();

    h.backend
        .cover(&h.id, 512, cancelled, Some(cancelled_on_count));
    h.backend.cover(&h.id, 512, queued, None);
    assert!(!h.queue.pump());
    assert_eq!(decoder.started.get(), 1);
    h.backend.cancel_covers();
    h.backend.cover(&h.id, 256, latest, Some(latest_on_count));
    assert_eq!(h.queue.0.borrow().len(), 1);
    assert!(!h.queue.pump());
    assert_eq!(decoder.started.get(), 1);

    decoder.ready.set(true);
    assert!(h.queue.pump());
    assert_eq!(decoder.started.get(), 2);
    assert!(cancelled_cover.try_recv().is_err());
    assert!(cancelled_count.try_recv().is_err());
    assert!(queued_cover.try_recv().is_err());
    assert!(latest_cover.try_recv().unwrap().is_ok());
    assert_eq!(latest_count.try_recv().unwrap().unwrap().get(), 2);
}

#[test]
fn local_cover_queue_recovers_when_scheduling_drops_a_task() {
    let dir = tempfile::tempdir().unwrap();
    let id = request_id(&super::super::test_book(dir.path(), 1)).unwrap();
    let queue = Rc::new(Queue::default());
    let pending = Rc::clone(&queue);
    let drop_next = Cell::new(true);
    let backend = LocalWebBackend::with_parts(
        Rc::new(move |task| {
            if !drop_next.replace(false) {
                pending.0.borrow_mut().push_back(task);
            }
        }),
        Rc::new(ImageCratePageDecoder),
        Rc::new(ImageCratePageDecoder),
    );

    let (discarded, discarded_result) = super::super::test_callback();
    backend.cover(&id, 512, discarded, None);
    assert!(matches!(
        discarded_result.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    ));

    let (done, result) = super::super::test_callback();
    backend.cover(&id, 512, done, None);
    assert!(queue.pump());
    assert!(result.try_recv().unwrap().is_ok());
}

#[test]
fn local_close_drops_pending_callbacks() {
    let decoder = Rc::new(GatedDecoder::default());
    let h = Harness::new(1, decoder.clone());
    let session = h.open().session;
    let (done, result) = super::super::test_callback();
    h.backend.decode_spread(session, 0, None, done);
    assert!(!h.queue.pump());
    h.backend.close(session);
    decoder.ready.set(true);
    assert!(h.queue.pump());
    assert!(result.try_recv().is_err());
}

#[test]
fn thumbnail_returns_unavailable_then_drops_after_close() {
    let h = Harness::png(1);
    let session = h.open().session;
    let (done, result) = super::super::test_callback();
    h.backend.thumbnail(session, 0, 512, done);
    assert_eq!(result.try_recv(), Err(TryRecvError::Empty));
    assert!(h.queue.pump());
    assert_eq!(
        result.try_recv().unwrap(),
        Err(BackendError::Other {
            detail: "thumbnails are unavailable on this build".into()
        })
    );

    let (done, result) = super::super::test_callback();
    h.backend.thumbnail(session, 0, 512, done);
    h.backend.close(session);
    assert!(h.queue.pump());
    assert_eq!(result.try_recv(), Err(TryRecvError::Disconnected));
}

#[test]
fn probe_page_count_classifies() {
    let h = Harness::png(2);
    let empty = h.dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let disabled = h.dir.path().join("disabled.cbr");
    std::fs::write(&disabled, []).unwrap();
    let missing = h.dir.path().join("missing");

    let probe = |id: &BookId| {
        let (done, result) = super::super::test_callback();
        h.backend
            .probe_page_count(id, ArchivePolicy { allow_rar: false }, done);
        assert!(h.queue.pump());
        result.try_recv().unwrap()
    };
    assert_eq!(probe(&h.id).unwrap().get(), 2);
    assert_eq!(
        probe(&request_id(&empty).unwrap()),
        Err(BackendError::EmptyBook)
    );
    assert!(matches!(
        probe(&request_id(&disabled).unwrap()),
        Err(BackendError::FormatDisabled { .. })
    ));
    assert!(matches!(
        probe(&request_id(&missing).unwrap()),
        Err(BackendError::Other { .. })
    ));
}

#[test]
fn local_cache_config_is_not_raised() {
    let h = Harness::png(8);
    let session = h.open().session;
    for index in 0..7 {
        assert!(h.spread(session, index, None).leading.1.is_ok());
    }
    assert!(h.backend.cached_page(session, 0).is_none());
    for index in 1..7 {
        assert!(h.backend.cached_page(session, index).is_some());
    }
    assert!(h.backend.cached_page(session, 7).is_none());
}
