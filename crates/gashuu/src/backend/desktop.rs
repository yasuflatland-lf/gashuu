use super::cover_mem::CoverMemCache;
use super::{
    BackendError, BookBackend, CountResult, Done, ImageResult, OpenedBook, SessionId, SpreadPages,
};
use crate::book_path::locate;
use gashuu_core::{
    canonical_identity, generate_cover, generate_one_thumbnail, ArchiveLoader, ArchivePolicy,
    BookFingerprint, BookId, CacheConfig, DecodedImage, ImageCache, OpenedSource, PageSource,
    PageThumbContext, ThumbSlot, ThumbStore,
};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

/// Schedule a closure on the UI thread. Tests pump a channel in its place.
pub(crate) type Poster = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

pub(crate) struct DesktopBackend {
    inner: Arc<Inner>,
}

#[allow(dead_code)] // TODO(wasm-backend): remove when #656 ([6/13]) and #657 ([7/13]) have both merged
struct Inner {
    post: Poster,
    sessions: Mutex<HashMap<SessionId, Arc<Session>>>,
    cover_generation: AtomicU64,
    cover_mem: Mutex<CoverMemCache>,
    thumbs: Arc<dyn ThumbStore>,
}

#[allow(dead_code)] // TODO(wasm-backend): remove when #656 ([6/13]) and #657 ([7/13]) have both merged
struct Session {
    id: BookId,
    source: Arc<dyn PageSource>,
    cache: ImageCache,
}

impl DesktopBackend {
    pub(crate) fn new(thumbs: Arc<dyn ThumbStore>) -> Self {
        let post = Arc::new(|f| {
            if let Err(error) = slint::invoke_from_event_loop(f) {
                tracing::debug!(%error, "dropped backend result; event loop gone");
            }
        });
        Self::with_parts(post, thumbs)
    }

    pub(crate) fn with_parts(post: Poster, thumbs: Arc<dyn ThumbStore>) -> Self {
        Self {
            inner: Arc::new(Inner {
                post,
                sessions: Mutex::default(),
                cover_generation: AtomicU64::new(0),
                cover_mem: Mutex::new(CoverMemCache::new()),
                thumbs,
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn open_blocking(
        &self,
        id: &BookId,
        policy: ArchivePolicy,
        cache: CacheConfig,
    ) -> Result<OpenedBook, BackendError> {
        self.inner.open_session(id, policy, cache)
    }
}

/// Sweep the persistent cache on a worker after dispatching the startup covers.
pub(crate) fn spawn_thumb_prune(thumbs: Arc<dyn ThumbStore>, max_bytes: u64) {
    rayon::spawn(move || {
        let started = crate::clock::now();
        let report = thumbs.prune(max_bytes);
        tracing::debug!(
            removed_files = report.removed_files,
            removed_bytes = report.removed_bytes,
            retained_bytes = report.retained_bytes,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "cover cache pruned"
        );
    });
}

fn cover_open(path: &Path, policy: ArchivePolicy) -> Result<OpenedSource, BackendError> {
    ArchiveLoader::open_book(path, policy).map_err(|error| BackendError::for_open(path, &error))
}

#[allow(dead_code)] // TODO(wasm-backend): remove when #656 ([6/13]) and #657 ([7/13]) have both merged
impl Inner {
    fn session(&self, id: SessionId) -> Option<Arc<Session>> {
        self.sessions
            .lock()
            .expect("sessions mutex poisoned")
            .get(&id)
            .cloned()
    }

    fn deliver(self: &Arc<Self>, session: Option<SessionId>, f: impl FnOnce() + Send + 'static) {
        let inner = Arc::clone(self);
        (self.post)(Box::new(move || {
            if session.is_none_or(|id| inner.session(id).is_some()) {
                f();
            }
        }));
    }

    fn open_session(
        &self,
        id: &BookId,
        policy: ArchivePolicy,
        config: CacheConfig,
    ) -> Result<OpenedBook, BackendError> {
        let path = locate(id);
        let source = ArchiveLoader::open_with_policy(&path, policy)
            .map_err(|error| BackendError::for_open(&path, &error))?;
        let skipped = source.skipped_count();
        let listing_truncated = source.listing_truncated();
        if skipped > 0 {
            tracing::warn!(skipped, path = %path.display(), "entries skipped while opening path");
        }
        if listing_truncated {
            tracing::warn!(path = %path.display(), "archive listing truncated; later pages are missing");
        }
        let canonical = canonical_identity(&path);
        let id = BookId::from_canonical_path(&canonical)
            .map_err(|error| BackendError::from_core(&error))?;
        let session = SessionId::allocate();
        let opened = OpenedBook {
            session,
            id: id.clone(),
            page_count: source.list_pages().len(),
            skipped,
            listing_truncated,
            is_dir: path.is_dir(),
        };
        let cache = ImageCache::new(Arc::clone(&source), config);
        self.sessions
            .lock()
            .expect("sessions mutex poisoned")
            .insert(session, Arc::new(Session { id, source, cache }));
        Ok(opened)
    }

    fn cover_is_current(&self, generation: u64) -> bool {
        self.cover_generation.load(Relaxed) == generation
    }

    fn cache_cover(&self, key: String, image: DecodedImage) -> Arc<DecodedImage> {
        let image = Arc::new(image);
        if let Ok(mut mem) = self.cover_mem.lock() {
            mem.insert(key, Arc::clone(&image));
        }
        image
    }

    fn cached_cover(
        &self,
        fp: BookFingerprint,
        max_side: u32,
        key: &str,
    ) -> Option<Arc<DecodedImage>> {
        let cached = self.cover_mem.lock().ok().and_then(|mut mem| mem.get(key));
        cached.or_else(|| {
            self.thumbs
                .get(fp, ThumbSlot::Cover, max_side)
                .map(|image| self.cache_cover(key.to_owned(), image))
        })
    }

    fn deliver_cover_result<T: Send + 'static>(
        self: &Arc<Self>,
        generation: u64,
        done: Done<T>,
        result: T,
    ) {
        let inner = Arc::clone(self);
        self.deliver(None, move || {
            if inner.cover_is_current(generation) {
                done(result);
            }
        });
    }

    fn generate_cover(
        &self,
        path: &Path,
        policy: ArchivePolicy,
        fp: BookFingerprint,
        key: String,
        max_side: u32,
    ) -> Result<(Arc<DecodedImage>, NonZeroUsize), BackendError> {
        let (source, count) = cover_open(path, policy)?.into_parts();
        let generated = generate_cover(source, max_side).inspect_err(|error| {
            tracing::warn!(path = %path.display(), %error, "cover: generate failed");
        });
        generated
            .map(|image| {
                if let Err(error) = self.thumbs.put(fp, ThumbSlot::Cover, max_side, &image) {
                    tracing::warn!(path = %path.display(), %error, "cover: cache put failed");
                }
                (self.cache_cover(key, image), count)
            })
            .map_err(|error| BackendError::from_core(&error))
    }
}

impl BookBackend for DesktopBackend {
    fn open(
        &self,
        id: &BookId,
        policy: ArchivePolicy,
        cache: CacheConfig,
        done: Done<Result<OpenedBook, BackendError>>,
    ) {
        let inner = Arc::clone(&self.inner);
        let id = id.clone();
        rayon::spawn(move || {
            let result = inner.open_session(&id, policy, cache);
            let session = result.as_ref().ok().map(|opened| opened.session);
            inner.deliver(session, move || done(result));
        });
    }

    fn close(&self, session: SessionId) {
        let removed = self
            .inner
            .sessions
            .lock()
            .expect("sessions mutex poisoned")
            .remove(&session);
        drop(removed);
    }

    fn cached_page(&self, session: SessionId, index: usize) -> Option<Arc<DecodedImage>> {
        self.inner.session(session)?.cache.get_cached(index)
    }

    fn decode_spread(
        &self,
        session: SessionId,
        leading: usize,
        trailing: Option<usize>,
        done: Done<SpreadPages>,
    ) {
        let inner = Arc::clone(&self.inner);
        rayon::spawn(move || {
            let Some(opened) = inner.session(session) else {
                return;
            };
            let dispatch = opened.cache.dispatch_handle();
            let decode = |index| {
                (
                    index,
                    dispatch
                        .decode_and_cache(index)
                        .map_err(|error| BackendError::from_core(&error)),
                )
            };
            let (leading, trailing) = if let Some(trailing) = trailing {
                let (first, second) = rayon::join(|| decode(leading), || decode(trailing));
                (first, Some(second))
            } else {
                (decode(leading), None)
            };
            inner.deliver(Some(session), move || {
                done(SpreadPages { leading, trailing })
            });
        });
    }

    fn thumbnail(&self, session: SessionId, index: usize, max_side: u32, done: Done<ImageResult>) {
        let inner = Arc::clone(&self.inner);
        rayon::spawn(move || {
            let Some(opened) = inner.session(session) else {
                return;
            };
            let ctx = PageThumbContext {
                store: &*inner.thumbs,
                fingerprint: inner.thumbs.fingerprint(&opened.id),
            };
            let result = generate_one_thumbnail(&opened.source, max_side, index, Some(ctx))
                .map(Arc::new)
                .map_err(|error| BackendError::from_core(&error));
            inner.deliver(Some(session), move || done(result));
        });
    }

    fn cover(
        &self,
        id: &BookId,
        policy: ArchivePolicy,
        max_side: u32,
        on_cover: Done<ImageResult>,
        on_count: Option<Done<CountResult>>,
    ) {
        let inner = Arc::clone(&self.inner);
        let id = id.clone();
        let generation = inner.cover_generation.load(Relaxed);
        rayon::spawn(move || {
            if !inner.cover_is_current(generation) {
                return;
            }
            let path = locate(&id);
            let fp = inner.thumbs.fingerprint(&id);
            let key = fp.cover_key(max_side);
            if let Some(image) = inner.cached_cover(fp, max_side, &key) {
                inner.deliver_cover_result(generation, on_cover, Ok(image));
                if let Some(on_count) = on_count.filter(|_| inner.cover_is_current(generation)) {
                    let count = cover_open(&path, policy).map(|book| book.page_count());
                    inner.deliver_cover_result(generation, on_count, count);
                }
                return;
            }
            match inner.generate_cover(&path, policy, fp, key, max_side) {
                Ok((image, count)) => {
                    inner.deliver_cover_result(generation, on_cover, Ok(image));
                    if let Some(on_count) = on_count {
                        inner.deliver_cover_result(generation, on_count, Ok(count));
                    }
                }
                Err(error) => inner.deliver_cover_result(generation, on_cover, Err(error)),
            }
        });
    }

    fn cancel_covers(&self) {
        self.inner.cover_generation.fetch_add(1, Relaxed);
    }

    fn probe_page_count(&self, id: &BookId, policy: ArchivePolicy, done: Done<CountResult>) {
        let inner = Arc::clone(&self.inner);
        let path = locate(id);
        rayon::spawn(move || {
            let result = ArchiveLoader::probe_page_count_with_policy(&path, policy)
                .map_err(|error| BackendError::from_core(&error));
            inner.deliver(None, move || done(result));
        });
    }
}

#[cfg(test)]
#[path = "desktop_tests.rs"]
mod tests;
