use super::cover_mem::CoverMemCache;
use super::{
    deliver_cover, BackendError, BookBackend, CountResult, Done, ImageResult, OpenedBook,
    SessionId, SpreadPages,
};
use crate::book_path;
use crate::page_decode::{decode_cover, decode_page, ImageCratePageDecoder, PageDecoder};
use gashuu_core::{cache_key, ArchivePolicy, BookId, CacheConfig, DecodedImage, ImageCache};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

type LocalTask = Pin<Box<dyn Future<Output = ()>>>;
pub(crate) type LocalSpawner = Rc<dyn Fn(LocalTask)>;

pub(crate) struct LocalWebBackend {
    inner: Rc<Inner>,
}

#[allow(dead_code)] // TODO(wasm-backend): remove when #656 ([6/13]) and #657 ([7/13]) have both merged
struct Inner {
    spawn: LocalSpawner,
    primary: Rc<dyn PageDecoder>,
    fallback: Rc<dyn PageDecoder>,
    fallback_warned: Cell<bool>,
    sessions: RefCell<HashMap<SessionId, ImageCache>>,
    cover_mem: RefCell<CoverMemCache>,
    covers: RefCell<VecDeque<CoverRequest>>,
    cover_running: Cell<bool>,
    cover_generation: Cell<u64>,
}

#[allow(dead_code)] // TODO(wasm-backend): remove when #656 ([6/13]) and #657 ([7/13]) have both merged
struct CoverRequest {
    id: BookId,
    max_side: u32,
    generation: u64,
    on_cover: Done<ImageResult>,
    on_count: Option<Done<CountResult>>,
}

// Reset the queue even when the event loop rejects or discards the task.
struct CoverDrain(Rc<Inner>);

impl Drop for CoverDrain {
    fn drop(&mut self) {
        self.0.cover_running.set(false);
        let abandoned = self.0.covers.take();
        drop(abandoned);
    }
}

impl LocalWebBackend {
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn new() -> Self {
        Self::with_parts(
            Rc::new(|task| {
                if let Err(error) = slint::spawn_local(task) {
                    tracing::debug!(%error, "dropped backend result; event loop gone");
                }
            }),
            Rc::new(crate::web_decode::NativePageDecoder),
            Rc::new(ImageCratePageDecoder),
        )
    }

    pub(crate) fn with_parts(
        spawn: LocalSpawner,
        primary: Rc<dyn PageDecoder>,
        fallback: Rc<dyn PageDecoder>,
    ) -> Self {
        Self {
            inner: Rc::new(Inner {
                spawn,
                primary,
                fallback,
                fallback_warned: Cell::new(false),
                sessions: RefCell::default(),
                cover_mem: RefCell::new(CoverMemCache::new()),
                covers: RefCell::default(),
                cover_running: Cell::new(false),
                cover_generation: Cell::new(0),
            }),
        }
    }

    fn schedule(&self, task: impl AsyncFnOnce(Rc<Inner>) + 'static) {
        let inner = Rc::clone(&self.inner);
        (self.inner.spawn)(Box::pin(async move { task(inner).await }));
    }
}

#[allow(dead_code)] // TODO(wasm-backend): remove when #656 ([6/13]) and #657 ([7/13]) have both merged
impl Inner {
    async fn drain_covers(&self) {
        loop {
            let request = self.covers.borrow_mut().pop_front();
            let Some(request) = request else {
                return;
            };
            let (cover, count) = self.load_cover(&request).await;
            deliver_cover(
                || self.cover_generation.get() == request.generation,
                request.on_cover,
                request.on_count,
                cover,
                count,
            );
        }
    }

    async fn load_cover(&self, request: &CoverRequest) -> (ImageResult, Option<CountResult>) {
        let path = book_path::locate(&request.id);
        let key = cache_key(&path, 0, request.max_side);
        let cached = self.cover_mem.borrow_mut().get(&key);
        if request.on_count.is_none() {
            if let Some(image) = cached {
                return (Ok(image), None);
            }
        }
        let opened = book_path::open(&path, ArchivePolicy::default())
            .map_err(|error| BackendError::for_open(&path, &error))
            .and_then(|source| {
                let count =
                    NonZeroUsize::new(source.list_pages().len()).ok_or(BackendError::EmptyBook)?;
                Ok((source, count))
            });
        if let Some(image) = cached {
            return (Ok(image), Some(opened.map(|(_, count)| count)));
        }
        let (source, count) = match opened {
            Ok(opened) => opened,
            Err(error) => return (Err(error), None),
        };
        let cover = decode_cover(
            source,
            self.primary.as_ref(),
            self.fallback.as_ref(),
            request.max_side,
        )
        .await
        .map(|image| {
            let image = Arc::new(image);
            self.cover_mem.borrow_mut().insert(key, Arc::clone(&image));
            image
        })
        .map_err(|error| BackendError::from_core(&error));
        (cover, request.on_count.is_some().then_some(Ok(count)))
    }
}

impl BookBackend for LocalWebBackend {
    fn open(
        &self,
        id: &BookId,
        policy: ArchivePolicy,
        cache: CacheConfig,
        done: Done<Result<OpenedBook, BackendError>>,
    ) {
        let id = id.clone();
        self.schedule(async move |inner| {
            let path = book_path::locate(&id);
            let result = book_path::open(&path, policy)
                .map(|source| {
                    let session = SessionId::allocate();
                    let opened = OpenedBook {
                        session,
                        id,
                        page_count: source.list_pages().len(),
                        skipped: source.skipped_count(),
                        listing_truncated: source.listing_truncated(),
                        is_dir: false,
                    };
                    inner
                        .sessions
                        .borrow_mut()
                        .insert(session, ImageCache::new(source, cache));
                    opened
                })
                .map_err(|error| BackendError::for_open(&path, &error));
            done(result);
        });
    }

    fn close(&self, session: SessionId) {
        self.inner.sessions.borrow_mut().remove(&session);
    }

    fn cached_page(&self, session: SessionId, index: usize) -> Option<Arc<DecodedImage>> {
        self.inner
            .sessions
            .borrow()
            .get(&session)?
            .get_cached(index)
    }

    fn decode_spread(
        &self,
        session: SessionId,
        leading: usize,
        trailing: Option<usize>,
        done: Done<SpreadPages>,
    ) {
        self.schedule(async move |inner| {
            let dispatch = inner
                .sessions
                .borrow()
                .get(&session)
                .map(ImageCache::dispatch_handle);
            let Some(dispatch) = dispatch else { return };
            let decode = async |index| {
                let page = decode_page(
                    &dispatch,
                    index,
                    inner.primary.as_ref(),
                    inner.fallback.as_ref(),
                    &inner.fallback_warned,
                )
                .await;
                (index, page.map_err(|error| BackendError::from_core(&error)))
            };
            let leading = decode(leading).await;
            let trailing = match trailing {
                Some(index) if inner.sessions.borrow().contains_key(&session) => {
                    Some(decode(index).await)
                }
                _ => None,
            };
            if inner.sessions.borrow().contains_key(&session) {
                done(SpreadPages { leading, trailing });
            }
        });
    }

    fn thumbnail(
        &self,
        session: SessionId,
        _index: usize,
        _max_side: u32,
        done: Done<ImageResult>,
    ) {
        self.schedule(async move |inner| {
            if inner.sessions.borrow().contains_key(&session) {
                done(Err(BackendError::Other {
                    detail: "thumbnails are unavailable on this build".into(),
                }));
            }
        });
    }

    fn cover(
        &self,
        id: &BookId,
        max_side: u32,
        on_cover: Done<ImageResult>,
        on_count: Option<Done<CountResult>>,
    ) {
        self.inner.covers.borrow_mut().push_back(CoverRequest {
            id: id.clone(),
            max_side,
            generation: self.inner.cover_generation.get(),
            on_cover,
            on_count,
        });
        if !self.inner.cover_running.replace(true) {
            let drain = CoverDrain(Rc::clone(&self.inner));
            (self.inner.spawn)(Box::pin(async move {
                drain.0.drain_covers().await;
                drop(drain);
            }));
        }
    }

    fn cancel_covers(&self) {
        self.inner
            .cover_generation
            .set(self.inner.cover_generation.get() + 1);
        let cancelled = self.inner.covers.take();
        drop(cancelled);
    }

    fn probe_page_count(&self, id: &BookId, policy: ArchivePolicy, done: Done<CountResult>) {
        let id = id.clone();
        self.schedule(async move |_| {
            let result = book_path::probe_page_count(&book_path::locate(&id), policy)
                .map_err(|error| BackendError::from_core(&error));
            done(result);
        });
    }
}

#[cfg(test)]
#[path = "local_web_tests.rs"]
mod tests;
