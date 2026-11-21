//! Shared background-worker → UI-thread plumbing for the async loader controllers.
//!
//! The library carousel's cover loader, the page-count prefetch, the bulk-add
//! controller, the thumbnail strip, and the viewer page loader all hand a result
//! from a rayon worker back to the Slint event loop the same way, and the cover
//! loader and thumbnail strip rotate a per-generation cancel flag the same way.
//! The open and bulk-add controllers use [`Generation`] to give each operation
//! its own result slot. Workers deliver through a [`Ticket`], and the UI thread
//! drains results for the current generation.

use crate::ViewerWindow;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

/// Marshal `body` onto the Slint event loop for one background result, applying
/// the epoch-guard + weak-upgrade preamble shared by every worker → UI handoff.
///
/// `body` runs on the UI thread, so a `!Send` value (a `slint::Image`, a
/// `SharedString`) is still built INSIDE it — only the captured inputs must be
/// `Send`. The result of a SUPERSEDED generation is dropped: if `my_epoch` no
/// longer matches the live `epoch` (a library refresh / re-open bumped it since
/// this result was dispatched), the closure returns without touching the UI, so a
/// late delivery can never overwrite a fresh frame. A dead `Weak` (the window
/// closed) is likewise dropped. `what` tags the debug trace on the
/// event-loop-gone path so each call site stays identifiable. This is the ONE
/// spelling of the stale-generation guard (previously inline `epoch.load(Relaxed)
/// != my_epoch` at eight sites and the `is_current` helper at a ninth).
pub(crate) fn marshal_to_ui(
    weak: slint::Weak<ViewerWindow>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    what: &'static str,
    body: impl FnOnce(&ViewerWindow) + Send + 'static,
) {
    if let Err(e) = slint::invoke_from_event_loop(move || {
        // Drop results from a superseded generation (library refreshed since).
        if epoch.load(Relaxed) != my_epoch {
            return;
        }
        let Some(ui) = weak.upgrade() else {
            return;
        };
        body(&ui);
    }) {
        tracing::debug!(what, error = %e, "dropped UI update; event loop gone");
    }
}

/// Supersede the previous generation's cancel flag and install a fresh one,
/// returning a clone of the new flag for the just-started generation. Each
/// `RefCell` borrow is confined to its own statement (dropped at the `;`) so no
/// two overlap — collapsing these into one expression would compile but panic at
/// runtime with a double borrow. Shared by `CoverController` and
/// `ThumbnailController`.
pub(crate) fn rotate_cancel(cell: &RefCell<Arc<AtomicBool>>) -> Arc<AtomicBool> {
    cell.borrow().store(true, Relaxed);
    *cell.borrow_mut() = Arc::new(AtomicBool::new(false));
    Arc::clone(&cell.borrow())
}

/// Epoch-guarded, take-once result slot for one async generation at a time.
pub(crate) struct Generation<T> {
    epoch: Arc<AtomicUsize>,
    slot: RefCell<Arc<Mutex<T>>>,
}

/// Worker handle for delivering results to one generation's slot.
pub(crate) struct Ticket<T> {
    epoch: Arc<AtomicUsize>,
    id: usize,
    slot: Arc<Mutex<T>>,
}

impl<T> Clone for Ticket<T> {
    fn clone(&self) -> Self {
        Self {
            epoch: Arc::clone(&self.epoch),
            id: self.id,
            slot: Arc::clone(&self.slot),
        }
    }
}

impl<T: Default> Generation<T> {
    pub(crate) fn new() -> Self {
        Self {
            epoch: Arc::new(AtomicUsize::new(0)),
            slot: RefCell::default(),
        }
    }

    pub(crate) fn begin(&self) -> Ticket<T> {
        let id = self.epoch.fetch_add(1, Relaxed) + 1;
        let slot = Arc::new(Mutex::new(T::default()));
        *self.slot.borrow_mut() = Arc::clone(&slot);
        Ticket {
            epoch: Arc::clone(&self.epoch),
            id,
            slot,
        }
    }

    pub(crate) fn take(&self, id: usize) -> Option<T> {
        if self.epoch.load(Relaxed) != id {
            return None;
        }
        let slot = Arc::clone(&self.slot.borrow());
        let mut value = slot.lock().expect("generation slot mutex poisoned");
        Some(std::mem::take(&mut *value))
    }
}

impl<T> Ticket<T> {
    pub(crate) fn id(&self) -> usize {
        self.id
    }

    pub(crate) fn is_current(&self) -> bool {
        self.epoch.load(Relaxed) == self.id
    }

    pub(crate) fn deliver(&self, f: impl FnOnce(&mut T)) -> bool {
        if !self.is_current() {
            return false;
        }
        // If begin races this lock, the ticket still holds only its original slot.
        f(&mut self.slot.lock().expect("generation slot mutex poisoned"));
        true
    }

    pub(crate) fn marshal(
        &self,
        weak: slint::Weak<ViewerWindow>,
        what: &'static str,
        body: impl FnOnce(&ViewerWindow) + Send + 'static,
    ) {
        marshal_to_ui(weak, Arc::clone(&self.epoch), self.id, what, body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_returns_current_value_once() {
        let generation = Generation::new();
        let ticket = generation.begin();
        assert!(ticket.deliver(|slot| *slot = Some(42)));
        assert_eq!(generation.take(ticket.id()), Some(Some(42)));
        assert_eq!(generation.take(ticket.id()), Some(None));
    }

    #[test]
    fn take_drains_accumulator_and_leaves_it_empty() {
        let generation = Generation::<Vec<u32>>::new();
        let ticket = generation.begin();
        for value in 1..=3 {
            assert!(ticket.deliver(|values| values.push(value)));
        }
        assert_eq!(generation.take(ticket.id()), Some(vec![1, 2, 3]));
        assert_eq!(generation.take(ticket.id()), Some(vec![]));
    }

    #[test]
    fn stale_ticket_does_not_deliver() {
        let generation = Generation::new();
        let first = generation.begin();
        let second = generation.begin();
        assert!(!first.deliver(|_| panic!("stale delivery ran")));
        assert!(second.deliver(|slot| *slot = Some(2)));
    }

    #[test]
    fn take_with_superseded_id_is_none() {
        let generation = Generation::new();
        let first = generation.begin();
        let second = generation.begin();
        assert!(second.deliver(|slot| *slot = Some(2)));
        assert_eq!(generation.take(first.id()), None);
        assert_eq!(generation.take(second.id()), Some(Some(2)));
    }

    #[test]
    fn ids_increase_by_one_from_one() {
        let generation = Generation::<()>::new();
        assert_eq!(generation.begin().id(), 1);
        assert_eq!(generation.begin().id(), 2);
    }

    #[test]
    fn ticket_is_send_and_clone() {
        fn assert_send<T: Send>() {}
        fn assert_clone<T: Clone>() {}
        struct NonClone;

        assert_send::<Ticket<Vec<u32>>>();
        assert_clone::<Ticket<Vec<NonClone>>>();
    }

    #[test]
    fn concurrent_delivers_all_land() {
        let generation = Generation::<Vec<u32>>::new();
        let ticket = generation.begin();
        std::thread::scope(|scope| {
            for i in 0..8 {
                let ticket = ticket.clone();
                scope.spawn(move || assert!(ticket.deliver(|values| values.push(i))));
            }
        });
        let mut values = generation.take(ticket.id()).expect("current generation");
        values.sort_unstable();
        assert_eq!(values, (0..8).collect::<Vec<_>>());
    }
}
