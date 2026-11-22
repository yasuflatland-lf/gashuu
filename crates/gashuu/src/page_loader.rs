//! Tracks in-flight spread requests and applies the current target's pages together.

use crate::backend::{BookBackend, SessionId, SpreadPages};
use crate::ui_marshal::ui_done;
use crate::{apply_spread_images, to_slint_image, ViewerWindow};
use gashuu_core::DecodedImage;
use std::cell::RefCell;
use std::collections::HashSet;
#[cfg(not(target_arch = "wasm32"))]
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;

/// Owns viewer page-decode dispatch bookkeeping.
///
/// `dispatched` is deliberately UI-thread-only state: it prevents duplicate
/// decode dispatches for a page while a result is in flight, and handlers clear
/// it when the corresponding scalar callback is applied.
pub struct PageController {
    epoch: Arc<AtomicUsize>,
    dispatched: RefCell<HashSet<usize>>,
    target: RefCell<Option<SpreadTarget>>,
    #[cfg(not(target_arch = "wasm32"))]
    _ui_thread: PhantomData<Rc<()>>,
    backend: Rc<dyn BookBackend>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SpreadTarget {
    leading_idx: usize,
    trailing_idx: Option<usize>,
    single: bool,
}

impl PageController {
    pub fn new(backend: Rc<dyn BookBackend>) -> Self {
        Self {
            backend,
            epoch: Arc::default(),
            dispatched: RefCell::default(),
            target: RefCell::default(),
            #[cfg(not(target_arch = "wasm32"))]
            _ui_thread: PhantomData,
        }
    }
    pub(crate) fn backend(&self) -> &dyn BookBackend {
        self.backend.as_ref()
    }

    /// Current generation number.
    pub fn current_epoch(&self) -> usize {
        self.epoch.load(Relaxed)
    }

    /// Start a fresh page-result generation and clear all dispatch reservations.
    ///
    /// Use this when the currently targeted spread/source changes and late
    /// marshals from the previous generation must be ignored.
    pub fn begin_generation(&self) -> usize {
        self.dispatched.borrow_mut().clear();
        self.epoch.fetch_add(1, Relaxed).wrapping_add(1)
    }

    /// Reset the controller for a different opened book/source.
    ///
    /// Opening a new source must both clear dispatch dedup state and advance the
    /// epoch so any late marshals from the previous book are dropped.
    pub fn reset_for_source(&self) -> usize {
        *self.target.borrow_mut() = None;
        self.begin_generation()
    }

    /// Set the currently targeted spread, advancing the epoch on a real target
    /// change so late decode marshals from the previous spread are dropped.
    pub fn set_target(
        &self,
        leading_idx: usize,
        trailing_idx: Option<usize>,
        single: bool,
    ) -> usize {
        let next = SpreadTarget {
            leading_idx,
            trailing_idx,
            single,
        };
        let mut target = self.target.borrow_mut();
        if target.as_ref() == Some(&next) {
            return self.current_epoch();
        }
        *target = Some(next);
        drop(target);
        self.begin_generation()
    }

    /// Clear the current target and advance the epoch once if a spread was
    /// active. Used when the viewer has no displayable source.
    pub fn clear_target(&self) -> usize {
        let had_target = self.target.borrow_mut().take().is_some();
        if had_target {
            self.begin_generation()
        } else {
            self.dispatched.borrow_mut().clear();
            self.current_epoch()
        }
    }

    /// Reserve `index` for async decode.
    ///
    /// Returns `true` only for the first reservation while the page is in flight;
    /// callers should dispatch only when this returns `true`.
    pub fn reserve_dispatch(&self, index: usize) -> bool {
        self.dispatched.borrow_mut().insert(index)
    }

    /// Clear one completed or failed page reservation.
    pub fn clear_dispatched(&self, index: usize) {
        self.dispatched.borrow_mut().remove(&index);
    }

    /// Clear the reservations for the displayed spread.
    pub fn clear_dispatched_spread(&self, leading_idx: usize, trailing_idx: Option<usize>) {
        let mut dispatched = self.dispatched.borrow_mut();
        dispatched.remove(&leading_idx);
        if let Some(trailing_idx) = trailing_idx {
            dispatched.remove(&trailing_idx);
        }
    }

    /// Query helper for handlers/tests that need to inspect dedup state.
    #[allow(dead_code)]
    pub fn is_dispatched(&self, index: usize) -> bool {
        self.dispatched.borrow().contains(&index)
    }

    /// Reserve each missing spread slot independently.
    ///
    /// `blocked_by_in_flight` is true only when there was MISS work requested
    /// but every missing page was already reserved by an earlier dispatch.
    pub fn reserve_missing_slots(
        &self,
        leading_idx: usize,
        leading_missing: bool,
        trailing: Option<(usize, bool)>,
    ) -> DispatchStatus {
        let mut status = DispatchStatus::default();
        let mut requested_missing = false;

        if leading_missing {
            requested_missing = true;
            status.leading_reserved = self.reserve_dispatch(leading_idx);
        }
        if let Some((trailing_idx, true)) = trailing {
            requested_missing = true;
            status.trailing_reserved = self.reserve_dispatch(trailing_idx);
        }

        status.blocked_by_in_flight = requested_missing && !status.any_reserved();
        status
    }

    /// Dispatch an atomic spread decode for any cache-missing slots.
    pub fn dispatch_spread(
        &self,
        ui_weak: slint::Weak<ViewerWindow>,
        session: SessionId,
        request: SpreadDecodeRequest,
    ) -> DispatchStatus {
        let leading_missing = request.leading.missing;

        let status = self.reserve_missing_slots(
            request.leading.index,
            leading_missing,
            request
                .trailing
                .as_ref()
                .map(|slot| (slot.index, slot.missing)),
        );

        if !status.any_reserved() {
            return status;
        }

        self.backend.decode_spread(
            session,
            request.leading.index,
            request.trailing.as_ref().map(|slot| slot.index),
            ui_done(
                ui_weak,
                self.epoch.clone(),
                self.current_epoch(),
                move |ui, pages| finish_decode_spread(ui, pages, request.single),
            ),
        );
        status
    }
}

/// One page slot in a spread-classification result.
#[derive(Clone)]
pub struct PageSlot {
    pub index: usize,
    pub missing: bool,
}

/// A currently targeted spread whose MISS slots should be decoded off-thread.
#[derive(Clone)]
pub struct SpreadDecodeRequest {
    pub leading: PageSlot,
    pub trailing: Option<PageSlot>,
    pub single: bool,
}

impl SpreadDecodeRequest {
    pub fn single(leading: PageSlot) -> Self {
        Self {
            leading,
            trailing: None,
            single: true,
        }
    }

    pub fn double(leading: PageSlot, trailing: PageSlot) -> Self {
        Self {
            leading,
            trailing: Some(trailing),
            single: false,
        }
    }
}

/// Result of a dispatch attempt.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DispatchStatus {
    pub leading_reserved: bool,
    pub trailing_reserved: bool,
    pub blocked_by_in_flight: bool,
}

impl DispatchStatus {
    pub fn any_reserved(self) -> bool {
        self.leading_reserved || self.trailing_reserved
    }
}

fn finish_decode_spread(ui: &ViewerWindow, pages: SpreadPages, requested_single: bool) {
    let (leading_idx, leading) = pages.leading;
    let trailing = pages.trailing;
    let leading = match leading {
        Ok(leading) => leading,
        Err(e) => {
            tracing::error!(index = leading_idx, error = %e, "failed to decode leading page");
            if let Some((trailing_idx, Err(e))) = trailing {
                tracing::error!(index = trailing_idx, error = %e, "failed to decode trailing page");
                ui.invoke_page_decode_error(page_i32(trailing_idx));
            }
            ui.invoke_page_decode_error(page_i32(leading_idx));
            return;
        }
    };

    let (trailing_idx, trailing, failed_trailing_page) = match trailing {
        Some((trailing_idx, Ok(trailing))) => (Some(trailing_idx), Some(trailing), None),
        Some((trailing_idx, Err(e))) => {
            tracing::error!(index = trailing_idx, error = %e, "failed to decode trailing page");
            (Some(trailing_idx), None, Some(trailing_idx))
        }
        None => (None, None, None),
    };
    let final_single = requested_single || trailing.is_none();

    let (content_w, content_h) = content_size(&leading, trailing.as_deref());
    let leading = to_slint_image(&leading);
    let trailing = trailing.as_deref().map(to_slint_image);
    apply_spread_images(ui, leading, trailing, final_single);
    ui.invoke_spread_anchored(
        content_w,
        content_h,
        final_single,
        optional_page_i32(failed_trailing_page),
        page_i32(leading_idx),
        optional_page_i32(trailing_idx),
    );
}

fn content_size(leading: &DecodedImage, trailing: Option<&DecodedImage>) -> (f32, f32) {
    match trailing {
        Some(trailing) => (
            leading.width().saturating_add(trailing.width()) as f32,
            leading.height().max(trailing.height()) as f32,
        ),
        None => (leading.width() as f32, leading.height() as f32),
    }
}

fn page_i32(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(i32::MAX)
}

fn optional_page_i32(index: Option<usize>) -> i32 {
    index.map_or(-1, page_i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatched_pages_are_deduplicated_cleared_and_reset_with_source() {
        let controller = PageController::new(crate::backend::fake::FakeBackend::new());

        assert!(controller.reserve_dispatch(4));
        assert!(!controller.reserve_dispatch(4));
        assert!(controller.is_dispatched(4));

        controller.clear_dispatched(4);
        assert!(!controller.is_dispatched(4));
        assert!(controller.reserve_dispatch(4));

        let epoch = controller.current_epoch();
        let next_epoch = controller.reset_for_source();

        assert!(next_epoch > epoch);
        assert!(!controller.is_dispatched(4));
        assert!(controller.reserve_dispatch(4));
    }

    #[test]
    fn reserve_dispatch_is_per_page_not_per_spread() {
        let controller = PageController::new(crate::backend::fake::FakeBackend::new());

        assert!(controller.reserve_dispatch(4));
        assert!(!controller.reserve_dispatch(4));
        assert!(controller.reserve_dispatch(5));
        assert!(controller.is_dispatched(4));
        assert!(controller.is_dispatched(5));
    }

    #[test]
    fn spread_reservation_still_reserves_new_partner_when_one_slot_is_in_flight() {
        let controller = PageController::new(crate::backend::fake::FakeBackend::new());
        assert!(controller.reserve_dispatch(4));

        let status = controller.reserve_missing_slots(4, true, Some((5, true)));

        assert!(!status.leading_reserved);
        assert!(status.trailing_reserved);
        assert!(!status.blocked_by_in_flight);
        assert!(controller.is_dispatched(4));
        assert!(controller.is_dispatched(5));
    }

    #[test]
    fn target_changes_advance_epoch_and_clear_dispatches() {
        let controller = PageController::new(crate::backend::fake::FakeBackend::new());
        let epoch = controller.current_epoch();

        let first = controller.set_target(1, None, true);
        assert!(first > epoch);
        assert_eq!(controller.set_target(1, None, true), first);

        assert!(controller.reserve_dispatch(1));
        let second = controller.set_target(2, Some(3), false);

        assert!(second > first);
        assert!(!controller.is_dispatched(1));
    }

    #[test]
    fn dispatch_spread_requests_both_indices_once() {
        let backend = crate::backend::fake::FakeBackend::new();
        let pages = PageController::new(backend.clone());
        let session = backend.new_session();
        let request = SpreadDecodeRequest::double(
            PageSlot {
                index: 0,
                missing: true,
            },
            PageSlot {
                index: 1,
                missing: true,
            },
        );
        assert!(pages
            .dispatch_spread(Default::default(), session, request.clone())
            .any_reserved());
        assert!(
            pages
                .dispatch_spread(Default::default(), session, request)
                .blocked_by_in_flight
        );
        assert_eq!(
            backend.calls(),
            vec![crate::backend::fake::Call::DecodeSpread(
                session,
                0,
                Some(1)
            )]
        );
    }
    #[test]
    fn stale_spread_done_is_ignored() {
        let backend = crate::backend::fake::FakeBackend::new();
        let pages = PageController::new(backend.clone());
        pages.set_target(0, None, true);
        pages.dispatch_spread(
            Default::default(),
            backend.new_session(),
            SpreadDecodeRequest::single(PageSlot {
                index: 0,
                missing: true,
            }),
        );
        pages.set_target(1, None, true);
        pages.reserve_dispatch(1);
        backend.complete_spread(
            0,
            SpreadPages {
                leading: (0, Err(crate::backend::BackendError::EmptyBook)),
                trailing: None,
            },
        );
        assert!(pages.is_dispatched(1));
        assert!(!pages.is_dispatched(0));
    }
}
