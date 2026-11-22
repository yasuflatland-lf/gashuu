//! Show page counts as backend cover callbacks arrive on the UI thread.
//! Queue nonzero counts by book ID and persist changes together on the next
//! carousel refresh or when the application shuts down.

use crate::carousel::update_carousel_row;
use crate::library_model::clamp_to_i32;
use crate::{save_library, LibraryStoreHandle, ViewerWindow};
use gashuu_core::{BookId, Library};
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

/// A backend count awaiting persistence in the library.
pub(crate) struct ResolvedCount {
    /// The book's ID — the `Library::set_page_count` lookup key.
    id: BookId,
    count: NonZeroUsize,
}

/// Set the displayed `total` of carousel row `row`, on the UI thread. The cover
/// counterpart of `apply_loaded_cover`: same `!Send`-`VecModel`-via-`ui` re-fetch and same
/// row-bounds check (tolerating a model that shrank since the request was built),
/// swapping only the row's `total` so the focused-book counter reads "1 / N"
/// instead of "1 / 0" the moment the background count resolves. `progress` is left
/// untouched — an unread book is 0 % regardless of its (now known) total.
fn set_carousel_total(ui: &ViewerWindow, row: usize, total: i32) {
    update_carousel_row(ui, row, |item| item.total = total);
}

/// Counts awaiting the next library save. Backend callbacks must be Send.
pub(crate) struct PageCountPrefetch {
    pending: Arc<Mutex<Vec<ResolvedCount>>>,
}

impl PageCountPrefetch {
    pub(crate) fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A `Send` handle to the pending queue for a backend completion.
    pub(crate) fn queue(&self) -> Arc<Mutex<Vec<ResolvedCount>>> {
        Arc::clone(&self.pending)
    }

    /// Called on the UI thread inside an epoch-checked completion, so only the
    /// current generation can queue a count or update the visible row.
    pub(crate) fn record_count(
        pending: &Arc<Mutex<Vec<ResolvedCount>>>,
        ui: &ViewerWindow,
        row: usize,
        id: BookId,
        count: NonZeroUsize,
    ) {
        Self::queue_count(pending, id, count);
        set_carousel_total(ui, row, clamp_to_i32(count.get()));
    }

    fn queue_count(pending: &Mutex<Vec<ResolvedCount>>, id: BookId, count: NonZeroUsize) {
        pending
            .lock()
            .expect("cover pending_counts mutex poisoned")
            .push(ResolvedCount { id, count });
    }

    /// Drain resolved counts and save the library once if any changed.
    pub(crate) fn apply(&self, library: &RefCell<Library>, library_store: &LibraryStoreHandle) {
        let drained: Vec<ResolvedCount> = {
            let mut queue = self
                .pending
                .lock()
                .expect("cover pending_counts mutex poisoned");
            std::mem::take(&mut *queue)
        };
        let mut changed = false;
        for resolved in &drained {
            if library
                .borrow_mut()
                .set_page_count(&resolved.id, resolved.count)
            {
                changed = true;
            }
        }
        if changed {
            // Losing a page-count back-fill is harmless (re-persisted at the next leave point).
            if let Err(e) = save_library(library_store, &library.borrow()) {
                tracing::error!(error = %e, "cover: failed to save library after page-count prefetch");
            }
        }
    }

    /// Persist any still-pending prefetched page counts. Call once on shutdown.
    pub(crate) fn flush(&self, library: &RefCell<Library>, library_store: &LibraryStoreHandle) {
        self.apply(library, library_store);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_count_queues_nonzero_count() {
        let pending = PageCountPrefetch::new().queue();
        let id = BookId::new("book.cbz");
        let count = NonZeroUsize::new(7).unwrap();
        PageCountPrefetch::queue_count(&pending, id.clone(), count);
        let records = pending.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, id);
        assert_eq!(records[0].count, count);
    }
}
