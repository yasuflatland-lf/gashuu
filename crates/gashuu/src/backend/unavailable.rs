use super::{
    BackendError, BookBackend, CountResult, Done, ImageResult, OpenedBook, SessionId, SpreadPages,
};
use gashuu_core::{ArchivePolicy, BookId, CacheConfig, DecodedImage};
use std::sync::{
    atomic::{AtomicU64, Ordering::Relaxed},
    Arc,
};

pub(super) struct UnavailableBackend {
    error: BackendError,
    cover_generation: Arc<AtomicU64>,
}

impl UnavailableBackend {
    pub(super) fn new(error: BackendError) -> Self {
        Self {
            error,
            cover_generation: Arc::default(),
        }
    }
}

fn defer<T: Send + 'static>(done: Done<T>, value: T) {
    let _ = slint::invoke_from_event_loop(move || done(value));
}

impl BookBackend for UnavailableBackend {
    fn open(
        &self,
        _: &BookId,
        _: ArchivePolicy,
        _: CacheConfig,
        done: Done<Result<OpenedBook, BackendError>>,
    ) {
        defer(done, Err(self.error.clone()));
    }

    fn close(&self, _: SessionId) {}

    fn cached_page(&self, _: SessionId, _: usize) -> Option<Arc<DecodedImage>> {
        None
    }

    fn decode_spread(
        &self,
        _: SessionId,
        leading: usize,
        trailing: Option<usize>,
        done: Done<SpreadPages>,
    ) {
        defer(
            done,
            SpreadPages {
                leading: (leading, Err(self.error.clone())),
                trailing: trailing.map(|index| (index, Err(self.error.clone()))),
            },
        );
    }

    fn thumbnail(&self, _: SessionId, _: usize, _: u32, done: Done<ImageResult>) {
        defer(done, Err(self.error.clone()));
    }

    fn cover(
        &self,
        _: &BookId,
        _: ArchivePolicy,
        _: u32,
        on_cover: Done<ImageResult>,
        _: Option<Done<CountResult>>,
    ) {
        let generation = self.cover_generation.clone();
        let current = generation.load(Relaxed);
        defer(
            Box::new(move |result| {
                if generation.load(Relaxed) == current {
                    on_cover(result);
                }
            }),
            Err(self.error.clone()),
        );
    }

    fn cancel_covers(&self) {
        self.cover_generation.fetch_add(1, Relaxed);
    }

    fn probe_page_count(&self, _: &BookId, _: ArchivePolicy, done: Done<CountResult>) {
        defer(done, Err(self.error.clone()));
    }
}
