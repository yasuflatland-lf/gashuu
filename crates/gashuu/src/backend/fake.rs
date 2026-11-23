use super::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Call {
    Open(BookId),
    Close(SessionId),
    Discard(BookId),
    Sweep(Vec<BookId>, bool),
    CachedPage(SessionId, usize),
    DecodeSpread(SessionId, usize, Option<usize>),
    Thumbnail(SessionId, usize, u32),
    Cover {
        id: BookId,
        policy: ArchivePolicy,
        max_side: u32,
        wants_count: bool,
    },
    CancelCovers,
    ProbePageCount(BookId),
}

type OpenResult = Result<OpenedBook, BackendError>;
type ImportResult = Result<ImportedBook, BackendError>;
type PendingCover = (u64, Done<ImageResult>, Option<Done<CountResult>>);
type PendingSession<T> = Vec<(SessionId, Done<T>)>;

/// A UI-thread script of requests. The test decides when each result arrives.
#[derive(Default)]
pub(crate) struct FakeBackend {
    calls: Arc<Mutex<Vec<Call>>>,
    cached: RefCell<HashMap<(SessionId, usize), Arc<DecodedImage>>>,
    closed: RefCell<HashSet<SessionId>>,
    cover_generation: Cell<u64>,
    opens: RefCell<Vec<Done<OpenResult>>>,
    spreads: RefCell<PendingSession<SpreadPages>>,
    thumbnails: RefCell<PendingSession<ImageResult>>,
    covers: RefCell<Vec<PendingCover>>,
    probes: RefCell<Vec<Done<CountResult>>>,
    imports: RefCell<Vec<Done<ImportResult>>>,
    sweeps: RefCell<Vec<Done<SweepReport>>>,
}

impl FakeBackend {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }

    pub(crate) fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    pub(crate) fn complete_import(&self, result: ImportResult) {
        let done = self.imports.borrow_mut().remove(0);
        done(result);
    }

    pub(crate) fn complete_sweep(&self, report: SweepReport) {
        let done = self.sweeps.borrow_mut().remove(0);
        done(report);
    }

    pub(crate) fn new_session(&self) -> SessionId {
        SessionId::allocate()
    }

    pub(crate) fn set_cached(&self, session: SessionId, index: usize, image: Arc<DecodedImage>) {
        self.cached.borrow_mut().insert((session, index), image);
    }

    pub(crate) fn complete_open(&self, nth: usize, result: OpenResult) {
        let done = self.opens.borrow_mut().remove(nth);
        if result
            .as_ref()
            .ok()
            .is_none_or(|opened| self.is_open(opened.session))
        {
            done(result);
        }
    }

    pub(crate) fn complete_spread(&self, nth: usize, pages: SpreadPages) {
        let (session, done) = self.spreads.borrow_mut().remove(nth);
        if self.is_open(session) {
            done(pages);
        }
    }

    pub(crate) fn complete_thumbnail(&self, nth: usize, result: ImageResult) {
        let (session, done) = self.thumbnails.borrow_mut().remove(nth);
        if self.is_open(session) {
            done(result);
        }
    }

    pub(crate) fn complete_cover(
        &self,
        nth: usize,
        cover: ImageResult,
        count: Option<CountResult>,
    ) {
        let (generation, on_cover, on_count) = self.covers.borrow_mut().remove(nth);
        deliver_cover(
            || self.cover_generation.get() == generation,
            on_cover,
            on_count,
            cover,
            count,
        );
    }

    /// Take the callbacks so callers can exercise their own stale-result guards.
    pub(crate) fn take_cover(&self, nth: usize) -> (Done<ImageResult>, Option<Done<CountResult>>) {
        let (_, on_cover, on_count) = self.covers.borrow_mut().remove(nth);
        (on_cover, on_count)
    }

    pub(crate) fn complete_probe(&self, nth: usize, result: CountResult) {
        let done = self.probes.borrow_mut().remove(nth);
        done(result);
    }

    fn is_open(&self, session: SessionId) -> bool {
        !self.closed.borrow().contains(&session)
    }

    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }
}

impl BookBackend for FakeBackend {
    fn open(
        &self,
        id: &BookId,
        _policy: ArchivePolicy,
        _cache: CacheConfig,
        done: Done<OpenResult>,
    ) {
        self.record(Call::Open(id.clone()));
        self.opens.borrow_mut().push(done);
    }

    fn close(&self, session: SessionId) {
        self.record(Call::Close(session));
        self.closed.borrow_mut().insert(session);
        self.cached.borrow_mut().retain(|(id, _), _| *id != session);
    }

    fn cached_page(&self, session: SessionId, index: usize) -> Option<Arc<DecodedImage>> {
        self.record(Call::CachedPage(session, index));
        self.cached.borrow().get(&(session, index)).cloned()
    }

    fn decode_spread(
        &self,
        session: SessionId,
        leading: usize,
        trailing: Option<usize>,
        done: Done<SpreadPages>,
    ) {
        self.record(Call::DecodeSpread(session, leading, trailing));
        self.spreads.borrow_mut().push((session, done));
    }

    fn thumbnail(&self, session: SessionId, index: usize, max_side: u32, done: Done<ImageResult>) {
        self.record(Call::Thumbnail(session, index, max_side));
        self.thumbnails.borrow_mut().push((session, done));
    }

    fn cover(
        &self,
        id: &BookId,
        policy: ArchivePolicy,
        max_side: u32,
        on_cover: Done<ImageResult>,
        on_count: Option<Done<CountResult>>,
    ) {
        self.record(Call::Cover {
            id: id.clone(),
            policy,
            max_side,
            wants_count: on_count.is_some(),
        });
        self.covers
            .borrow_mut()
            .push((self.cover_generation.get(), on_cover, on_count));
    }

    fn cancel_covers(&self) {
        self.record(Call::CancelCovers);
        self.cover_generation.set(self.cover_generation.get() + 1);
    }

    fn probe_page_count(&self, id: &BookId, _policy: ArchivePolicy, done: Done<CountResult>) {
        self.record(Call::ProbePageCount(id.clone()));
        self.probes.borrow_mut().push(done);
    }
}

#[path = "fake_tests.rs"]
mod tests;

impl BookImporter for FakeBackend {
    fn import(&self, _: crate::picker::UploadHandle, _: ArchivePolicy, done: Done<ImportResult>) {
        self.imports.borrow_mut().push(done);
    }
    fn discard(&self, id: &BookId) {
        self.calls.lock().unwrap().push(Call::Discard(id.clone()));
    }
    fn discarder(&self) -> DiscardUpload {
        let calls = Arc::clone(&self.calls);
        Arc::new(move |id| calls.lock().unwrap().push(Call::Discard(id.clone())))
    }
    fn sweep(&self, keep: Vec<BookId>, clear: bool, done: Done<SweepReport>) {
        self.calls.lock().unwrap().push(Call::Sweep(keep, clear));
        self.sweeps.borrow_mut().push(done);
    }
}
