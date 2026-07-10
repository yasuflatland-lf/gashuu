//! Viewer async page-decode controller.
//!
//! This is the viewer's async-decode arm, kept as a presentation controller
//! rather than folded into `ViewerState` so it follows the same mental model as
//! `cover_loader.rs`: UI-thread bookkeeping here, heavy decode on rayon workers,
//! and scalar-only Slint callbacks back into the handlers that own `Rc`
//! viewport/localizer state.
//!
//! Thread-boundary rule: only `Send` values cross into the rayon job and the
//! event-loop closure. `slint::Image` is built inside the UI-thread closure, and
//! `Rc`, `VecModel`, viewport state, and the localizer are never captured.

#[cfg(not(test))]
use crate::ui_marshal::marshal_to_ui;
#[cfg(not(test))]
use crate::{apply_spread_images, to_slint_image, ViewerWindow};
#[cfg(not(test))]
use gashuu_core::DecodedImage;
#[cfg(not(test))]
use gashuu_core::{cache::CacheDispatch, CoreError};
use std::cell::RefCell;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;

/// Owns viewer page-decode dispatch bookkeeping.
///
/// `dispatched` is deliberately UI-thread-only state: it prevents duplicate
/// decode dispatches for a page while a result is in flight, and handlers clear
/// it when the corresponding scalar callback is applied. The `Rc` marker makes
/// the controller `!Send`/`!Sync`, matching that ownership rule.
pub struct PageController {
    epoch: Arc<AtomicUsize>,
    dispatched: RefCell<HashSet<usize>>,
    target: RefCell<Option<SpreadTarget>>,
    _ui_thread: PhantomData<Rc<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SpreadTarget {
    leading_idx: usize,
    trailing_idx: Option<usize>,
    single: bool,
}

impl Default for PageController {
    fn default() -> Self {
        Self::new()
    }
}

impl PageController {
    /// Build the controller. Call once during UI setup.
    pub fn new() -> Self {
        Self {
            epoch: Arc::new(AtomicUsize::new(0)),
            dispatched: RefCell::new(HashSet::new()),
            target: RefCell::new(None),
            _ui_thread: PhantomData,
        }
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
    ///
    /// The request carries already-decoded HIT slots as `Arc<DecodedImage>` and
    /// marks MISS slots with `None`. If every MISS in this request is newly
    /// reserved, one rayon job is spawned; when both leading and trailing are
    /// missing, that job uses `rayon::join` so the two decodes run in parallel.
    /// If any requested MISS is already in flight, this returns
    /// `blocked_by_in_flight = true` and leaves completion to the earlier worker.
    #[cfg(not(test))]
    pub fn dispatch_spread(
        &self,
        ui_weak: slint::Weak<ViewerWindow>,
        cache_dispatch: CacheDispatch,
        request: SpreadDecodeRequest,
    ) -> DispatchStatus {
        let leading_missing = request.leading.is_missing();

        let status = self.reserve_missing_slots(
            request.leading.index,
            leading_missing,
            request
                .trailing
                .as_ref()
                .map(|slot| (slot.index, slot.is_missing())),
        );

        if !status.any_reserved() {
            return status;
        }

        spawn_decode_spread(
            ui_weak,
            Arc::clone(&self.epoch),
            self.current_epoch(),
            cache_dispatch,
            request,
        );
        status
    }
}

/// One page slot in a spread-classification result.
#[cfg(not(test))]
#[derive(Clone)]
pub struct PageSlot {
    pub index: usize,
    /// `Some` means cache HIT; `None` means cache MISS and must be decoded by the
    /// controller.
    pub decoded: Option<Arc<DecodedImage>>,
}

#[cfg(not(test))]
impl PageSlot {
    pub fn hit(index: usize, decoded: Arc<DecodedImage>) -> Self {
        Self {
            index,
            decoded: Some(decoded),
        }
    }

    pub fn miss(index: usize) -> Self {
        Self {
            index,
            decoded: None,
        }
    }

    pub fn is_missing(&self) -> bool {
        self.decoded.is_none()
    }
}

/// A currently targeted spread whose MISS slots should be decoded off-thread.
#[cfg(not(test))]
#[derive(Clone)]
pub struct SpreadDecodeRequest {
    pub leading: PageSlot,
    pub trailing: Option<PageSlot>,
    pub single: bool,
}

#[cfg(not(test))]
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

#[cfg(not(test))]
fn spawn_decode_spread(
    weak: slint::Weak<ViewerWindow>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    cache_dispatch: CacheDispatch,
    request: SpreadDecodeRequest,
) {
    rayon::spawn(move || {
        let leading_idx = request.leading.index;
        let trailing_slot = request.trailing;
        let requested_single = request.single || trailing_slot.is_none();

        // Resolve each slot: a HIT wraps the cached image in Ok; a MISS decodes
        // on this worker. When both are missing, rayon::join runs them in parallel.
        let (leading_result, trailing_result) = match (request.leading.decoded, trailing_slot) {
            (leading_hit, None) => {
                let leading = leading_hit
                    .map(Ok)
                    .unwrap_or_else(|| cache_dispatch.decode_and_cache(leading_idx));
                (leading, None)
            }
            (Some(leading), Some(t)) => {
                let trailing = t
                    .decoded
                    .map(Ok)
                    .unwrap_or_else(|| cache_dispatch.decode_and_cache(t.index));
                (Ok(leading), Some((t.index, trailing)))
            }
            (None, Some(t)) => {
                let (leading, trailing) = match t.decoded {
                    Some(trailing) => (cache_dispatch.decode_and_cache(leading_idx), Ok(trailing)),
                    None => {
                        let trailing_dispatch = cache_dispatch.clone();
                        rayon::join(
                            || cache_dispatch.decode_and_cache(leading_idx),
                            || trailing_dispatch.decode_and_cache(t.index),
                        )
                    }
                };
                (leading, Some((t.index, trailing)))
            }
        };

        finish_decode_spread(
            weak,
            epoch,
            my_epoch,
            leading_idx,
            leading_result,
            trailing_result,
            requested_single,
        );
    });
}

#[cfg(not(test))]
fn finish_decode_spread(
    weak: slint::Weak<ViewerWindow>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    leading_idx: usize,
    leading: Result<Arc<DecodedImage>, CoreError>,
    trailing: Option<(usize, Result<Arc<DecodedImage>, CoreError>)>,
    requested_single: bool,
) {
    let leading = match leading {
        Ok(leading) => leading,
        Err(e) => {
            tracing::error!(index = leading_idx, error = %e, "failed to decode leading page");
            if let Some((trailing_idx, Err(e))) = trailing {
                tracing::error!(index = trailing_idx, error = %e, "failed to decode trailing page");
                marshal_page_decode_error(weak.clone(), Arc::clone(&epoch), my_epoch, trailing_idx);
            }
            marshal_page_decode_error(weak, epoch, my_epoch, leading_idx);
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

    marshal_spread(
        weak,
        epoch,
        my_epoch,
        leading_idx,
        trailing_idx,
        leading,
        trailing,
        final_single,
        failed_trailing_page,
    );
}

#[cfg(not(test))]
#[allow(clippy::too_many_arguments)]
fn marshal_spread(
    weak: slint::Weak<ViewerWindow>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    leading_idx: usize,
    trailing_idx: Option<usize>,
    leading: Arc<DecodedImage>,
    trailing: Option<Arc<DecodedImage>>,
    single: bool,
    failed_trailing_page: Option<usize>,
) {
    marshal_to_ui(weak, epoch, my_epoch, "page-decode", move |ui| {
        let (content_w, content_h) = content_size(&leading, trailing.as_deref());
        let leading = to_slint_image(&leading);
        let trailing = trailing.as_deref().map(to_slint_image);
        apply_spread_images(ui, leading, trailing, single);
        ui.invoke_spread_anchored(
            content_w,
            content_h,
            single,
            optional_page_i32(failed_trailing_page),
            page_i32(leading_idx),
            optional_page_i32(trailing_idx),
        );
    });
}

#[cfg(not(test))]
fn marshal_page_decode_error(
    weak: slint::Weak<ViewerWindow>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    index: usize,
) {
    marshal_to_ui(weak, epoch, my_epoch, "page-decode-error", move |ui| {
        ui.invoke_page_decode_error(page_i32(index));
    });
}

#[cfg(not(test))]
fn content_size(leading: &DecodedImage, trailing: Option<&DecodedImage>) -> (f32, f32) {
    match trailing {
        Some(trailing) => (
            leading.width().saturating_add(trailing.width()) as f32,
            leading.height().max(trailing.height()) as f32,
        ),
        None => (leading.width() as f32, leading.height() as f32),
    }
}

#[cfg(not(test))]
fn page_i32(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(i32::MAX)
}

#[cfg(not(test))]
fn optional_page_i32(index: Option<usize>) -> i32 {
    index.map_or(-1, page_i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatched_pages_are_deduplicated_cleared_and_reset_with_source() {
        let controller = PageController::new();

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
        let controller = PageController::new();

        assert!(controller.reserve_dispatch(4));
        assert!(!controller.reserve_dispatch(4));
        assert!(controller.reserve_dispatch(5));
        assert!(controller.is_dispatched(4));
        assert!(controller.is_dispatched(5));
    }

    #[test]
    fn spread_reservation_still_reserves_new_partner_when_one_slot_is_in_flight() {
        let controller = PageController::new();
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
        let controller = PageController::new();
        let epoch = controller.current_epoch();

        let first = controller.set_target(1, None, true);
        assert!(first > epoch);
        assert_eq!(controller.set_target(1, None, true), first);

        assert!(controller.reserve_dispatch(1));
        let second = controller.set_target(2, Some(3), false);

        assert!(second > first);
        assert!(!controller.is_dispatched(1));
    }
}
