//! Asynchronous access to books from the UI thread.

use gashuu_core::{ArchivePolicy, BookId, CacheConfig, CoreError, DecodedImage};
use std::fmt;
use std::num::NonZeroUsize;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;

pub(crate) mod cover_mem;
#[cfg(not(target_arch = "wasm32"))]
mod desktop;
#[cfg(test)]
pub(crate) mod fake;
#[cfg(target_arch = "wasm32")]
mod unavailable;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(any(test, target_arch = "wasm32"))]
mod web_client;

/// Every `Done` passed to a backend is:
/// - invoked **at most once**;
/// - invoked **on the UI thread** (the Slint event-loop thread);
/// - **never invoked synchronously inside the call that received it**, even for an in-memory hit;
/// - **dropped without being invoked** if its session was closed (`close`) or its cover
///   generation was cancelled (`cancel_covers`) before delivery.
///
/// The last guarantee is checked again on the UI thread just before invocation.
/// So after `close(s)` returns, no `Done` for `s` ever runs. Callers keep their own
/// epoch guards: the backend does not know about UI generations.
pub(crate) type Done<T> = Box<dyn FnOnce(T) + Send + 'static>;

type ImageResult = Result<Arc<DecodedImage>, BackendError>;
type CountResult = Result<NonZeroUsize, BackendError>;

/// Backend-issued handle for one opened book. Never reused within a process.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct SessionId(u64);

#[allow(dead_code)] // TODO(wasm-backend): remove when #656 ([6/13]) and #657 ([7/13]) have both merged
impl SessionId {
    pub(super) fn new(raw: u64) -> Self {
        Self(raw)
    }

    fn allocate() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let raw = NEXT
            .try_update(Relaxed, Relaxed, |id| id.checked_add(1))
            .expect("session IDs exhausted");
        Self::new(raw)
    }

    #[cfg(test)]
    pub(crate) fn for_test(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackendError {
    /// The book has zero image pages.
    EmptyBook,
    /// Rejected by the archive policy.
    FormatDisabled { format: &'static str },
    /// The book's location no longer exists.
    Missing { detail: String },
    /// The display text of another error.
    Other { detail: String },
}

impl BackendError {
    #[cfg(not(target_arch = "wasm32"))]
    fn for_open(path: &Path, error: &CoreError) -> Self {
        if crate::book_path::exists(path) {
            Self::from_core(error)
        } else {
            Self::Missing {
                detail: error.to_string(),
            }
        }
    }

    pub(crate) fn from_core(error: &CoreError) -> Self {
        match error {
            CoreError::EmptyBook { .. } => Self::EmptyBook,
            CoreError::FormatDisabled { format } => Self::FormatDisabled { format },
            _ => Self::Other {
                detail: error.to_string(),
            },
        }
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyBook => f.write_str("no images found in book"),
            Self::FormatDisabled { format } => {
                write!(f, "{format} support is disabled in safer mode")
            }
            Self::Missing { detail } | Self::Other { detail } => f.write_str(detail),
        }
    }
}

/// A successful open, including a source with zero pages. The open use case
/// owns the rejection of empty books (ADR-0009).
#[allow(dead_code)] // TODO(wasm-backend): remove when #656 ([6/13]) and #657 ([7/13]) have both merged
pub(crate) struct OpenedBook {
    pub(crate) session: SessionId,
    /// Canonical identity resolved by the backend.
    pub(crate) id: BookId,
    pub(crate) page_count: usize,
    pub(crate) skipped: usize,
    pub(crate) listing_truncated: bool,
    /// Folder source. Always false on web.
    pub(crate) is_dir: bool,
}

#[allow(dead_code)] // TODO(wasm-backend): remove when #656 ([6/13]) and #657 ([7/13]) have both merged
pub(crate) struct SpreadPages {
    pub(crate) leading: (usize, ImageResult),
    pub(crate) trailing: Option<(usize, ImageResult)>,
}

#[allow(dead_code)] // TODO(wasm-backend): remove when #656 ([6/13]) and #657 ([7/13]) have both merged
pub(crate) trait BookBackend {
    /// Open an ID and create a session using the given cache configuration.
    fn open(
        &self,
        id: &BookId,
        policy: ArchivePolicy,
        cache: CacheConfig,
        done: Done<Result<OpenedBook, BackendError>>,
    );
    /// Release the source and cache and drop pending deliveries. Unknown IDs are ignored.
    fn close(&self, session: SessionId);
    /// Memory lookup only. A hit promotes the entry and warms neighbours.
    fn cached_page(&self, session: SessionId, index: usize) -> Option<Arc<DecodedImage>>;
    /// Decode or serve both requested slots from cache and deliver them together.
    fn decode_spread(
        &self,
        session: SessionId,
        leading: usize,
        trailing: Option<usize>,
        done: Done<SpreadPages>,
    );
    /// One strip thumbnail, scaled to at most `max_side` and persisted on desktop.
    fn thumbnail(&self, session: SessionId, index: usize, max_side: u32, done: Done<ImageResult>);
    /// Page zero scaled to at most `max_side`, with an optional count. The count
    /// is delivered only on cover success. A hit needs an extra open for the
    /// count and reports an empty book through the count callback. A miss
    /// reports an empty book through the cover callback. Order is unspecified.
    fn cover(
        &self,
        id: &BookId,
        max_side: u32,
        on_cover: Done<ImageResult>,
        on_count: Option<Done<CountResult>>,
    );
    /// Drop callbacks for cover requests issued before this call.
    fn cancel_covers(&self);
    /// Resolve the page count without creating a session.
    fn probe_page_count(&self, id: &BookId, policy: ArchivePolicy, done: Done<CountResult>);
}

/// Deliver both results of a cover operation, checking cancellation before
/// each callback because the cover callback can itself refresh the library.
#[allow(dead_code)] // TODO(wasm-backend): remove when #656 ([6/13]) and #657 ([7/13]) have both merged
fn deliver_cover(
    is_current: impl Fn() -> bool,
    on_cover: Done<ImageResult>,
    on_count: Option<Done<CountResult>>,
    cover: ImageResult,
    count: Option<CountResult>,
) {
    if is_current() {
        let succeeded = cover.is_ok();
        on_cover(cover);
        if let Some(on_count) = on_count.filter(|_| succeeded && is_current()) {
            on_count(count.expect("successful cover with a count callback has a count result"));
        }
    }
}

pub(crate) fn platform_backend() -> Rc<dyn BookBackend> {
    #[cfg(not(target_arch = "wasm32"))]
    return Rc::new(desktop::DesktopBackend::new());
    #[cfg(target_arch = "wasm32")]
    return web::shared()
        .map(|backend| backend as Rc<dyn BookBackend>)
        .unwrap_or_else(|| Rc::new(unavailable::UnavailableBackend::default()));
}

pub(crate) use importer::{BookImporter, DiscardUpload, ImportedBook};

mod importer {
    use super::{BackendError, Done};
    use crate::picker::UploadHandle;
    use gashuu_core::{ArchivePolicy, BookId};
    use std::{num::NonZeroUsize, sync::Arc};

    #[derive(Debug)]
    pub(crate) struct ImportedBook {
        pub id: BookId,
        pub title: String,
        pub page_count: NonZeroUsize,
    }

    pub(crate) type DiscardUpload = Arc<dyn Fn(&BookId) + Send + Sync>;

    pub(crate) trait BookImporter {
        fn import(
            &self,
            file: UploadHandle,
            policy: ArchivePolicy,
            done: Done<Result<ImportedBook, BackendError>>,
        );
        #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
        fn discard(&self, id: &BookId);
        /// A cleanup callback that a Send completion can invoke on the UI thread.
        fn discarder(&self) -> DiscardUpload;
    }
}

pub(crate) fn platform_importer() -> Option<Rc<dyn BookImporter>> {
    #[cfg(target_arch = "wasm32")]
    return web::shared().map(|backend| backend as Rc<dyn BookImporter>);
    #[cfg(not(target_arch = "wasm32"))]
    return None;
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) const STORAGE_UNAVAILABLE: &str = "this browser cannot store books";

#[cfg(test)]
#[path = "port_tests.rs"]
mod tests;

#[cfg(test)]
fn test_book(parent: &std::path::Path, pages: usize) -> std::path::PathBuf {
    let path = parent.join("book");
    std::fs::create_dir(&path).unwrap();
    for index in 0..pages {
        image::RgbaImage::new(4, 2)
            .save(path.join(format!("{index:03}.png")))
            .unwrap();
    }
    path
}

#[cfg(test)]
fn test_callback<T: Send + 'static>() -> (Done<T>, std::sync::mpsc::Receiver<T>) {
    let (tx, rx) = std::sync::mpsc::channel();
    (Box::new(move |result| tx.send(result).unwrap()), rx)
}
