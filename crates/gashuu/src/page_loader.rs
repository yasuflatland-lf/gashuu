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
    use crate::backend::fake::{Call, FakeBackend};
    use crate::backend::BackendError;
    use proptest::prelude::*;
    use slint::ComponentHandle;
    use std::cell::{Cell, RefCell};

    fn headless_window() -> ViewerWindow {
        struct HeadlessPlatform;
        impl slint::platform::Platform for HeadlessPlatform {
            fn create_window_adapter(
                &self,
            ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
                use slint::platform::software_renderer::{
                    MinimalSoftwareWindow, RepaintBufferType,
                };
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(HeadlessPlatform)).unwrap();
        ViewerWindow::new().unwrap()
    }

    #[derive(Clone, Debug)]
    enum DispatchOp {
        Reserve(usize),
        Clear(usize),
        SetTarget(usize, Option<usize>),
        ResetSource,
    }

    proptest! {
        #[test]
        fn dispatch_reservations_match_the_active_target_epoch(
            ops in proptest::collection::vec(
                prop_oneof![
                    (0usize..6).prop_map(DispatchOp::Reserve),
                    (0usize..6).prop_map(DispatchOp::Clear),
                    (0usize..3, proptest::option::of(0usize..3))
                        .prop_map(|(leading, trailing)| DispatchOp::SetTarget(leading, trailing)),
                    Just(DispatchOp::ResetSource),
                ],
                0..=40,
            ),
        ) {
            let controller = PageController::new(FakeBackend::new());
            let mut reserved = HashSet::new();
            let mut target = None;
            let mut epoch = 0;
            prop_assert_eq!(controller.current_epoch(), epoch);
            for op in ops {
                match op {
                    DispatchOp::Reserve(page) => {
                        prop_assert_eq!(controller.reserve_dispatch(page), reserved.insert(page));
                    }
                    DispatchOp::Clear(page) => {
                        controller.clear_dispatched(page);
                        reserved.remove(&page);
                    }
                    DispatchOp::SetTarget(leading, trailing) => {
                        let next = Some((leading, trailing));
                        if target != next {
                            epoch += 1;
                            reserved.clear();
                            target = next;
                        }
                        prop_assert_eq!(controller.set_target(leading, trailing, trailing.is_none()), epoch);
                    }
                    DispatchOp::ResetSource => {
                        epoch += 1;
                        target = None;
                        reserved.clear();
                        prop_assert_eq!(controller.reset_for_source(), epoch);
                    }
                }
                prop_assert_eq!(controller.current_epoch(), epoch);
                prop_assert_eq!(&*controller.dispatched.borrow(), &reserved);
            }
        }
    }

    #[test]
    fn spread_reservation_still_reserves_new_partner_when_one_slot_is_in_flight() {
        let controller = PageController::new(FakeBackend::new());
        assert!(controller.reserve_dispatch(4));

        let status = controller.reserve_missing_slots(4, true, Some((5, true)));

        assert!(!status.leading_reserved);
        assert!(status.trailing_reserved);
        assert!(!status.blocked_by_in_flight);
        assert!(controller.dispatched.borrow().contains(&4));
        assert!(controller.dispatched.borrow().contains(&5));
    }

    #[test]
    fn target_changes_advance_epoch_and_clear_dispatches() {
        let controller = PageController::new(FakeBackend::new());
        let epoch = controller.current_epoch();

        let first = controller.set_target(1, None, true);
        assert!(first > epoch);
        assert_eq!(controller.set_target(1, None, true), first);

        assert!(controller.reserve_dispatch(1));
        let second = controller.set_target(2, Some(3), false);

        assert!(second > first);
        assert!(!controller.dispatched.borrow().contains(&1));
    }

    #[test]
    fn dispatch_spread_requests_both_indices_once() {
        let ui = headless_window();
        let backend = FakeBackend::new();
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
            .dispatch_spread(ui.as_weak(), session, request.clone())
            .any_reserved());
        assert!(
            pages
                .dispatch_spread(ui.as_weak(), session, request)
                .blocked_by_in_flight
        );
        assert_eq!(
            backend.calls(),
            vec![Call::DecodeSpread(session, 0, Some(1))]
        );
    }

    #[test]
    fn stale_spread_done_is_ignored() {
        let ui = headless_window();
        let decode_errors = Rc::new(Cell::new(0));
        ui.on_page_decode_error({
            let decode_errors = Rc::clone(&decode_errors);
            move |_| decode_errors.set(decode_errors.get() + 1)
        });
        let anchored_spreads = Rc::new(Cell::new(0));
        ui.on_spread_anchored({
            let anchored_spreads = Rc::clone(&anchored_spreads);
            move |_, _, _, _, _, _| anchored_spreads.set(anchored_spreads.get() + 1)
        });

        let backend = FakeBackend::new();
        let pages = PageController::new(backend.clone());
        let epoch = pages.set_target(0, None, true);
        pages.dispatch_spread(
            ui.as_weak(),
            backend.new_session(),
            SpreadDecodeRequest::single(PageSlot {
                index: 0,
                missing: true,
            }),
        );
        assert!(pages.set_target(1, None, true) > epoch);
        assert!(pages.reserve_dispatch(1));
        backend.complete_spread(
            0,
            SpreadPages {
                leading: (0, Err(BackendError::EmptyBook)),
                trailing: None,
            },
        );
        assert_eq!(decode_errors.get(), 0);
        assert_eq!(anchored_spreads.get(), 0);
        assert!(pages.dispatched.borrow().contains(&1));
        assert!(!pages.dispatched.borrow().contains(&0));
    }

    #[test]
    fn current_spread_leading_error_reports_page_index() {
        let ui = headless_window();
        let failed_page = Rc::new(Cell::new(None));
        ui.on_page_decode_error({
            let failed_page = Rc::clone(&failed_page);
            move |index| failed_page.set(Some(index))
        });

        let backend = FakeBackend::new();
        let pages = PageController::new(backend.clone());
        pages.set_target(3, None, true);
        pages.dispatch_spread(
            ui.as_weak(),
            backend.new_session(),
            SpreadDecodeRequest::single(PageSlot {
                index: 3,
                missing: true,
            }),
        );
        backend.complete_spread(
            0,
            SpreadPages {
                leading: (3, Err(BackendError::EmptyBook)),
                trailing: None,
            },
        );

        assert_eq!(failed_page.get(), Some(3));
    }

    type Anchor = (f32, f32, bool, i32, i32, i32);

    fn capture_anchor(ui: &ViewerWindow) -> Rc<RefCell<Option<Anchor>>> {
        let anchor = Rc::new(RefCell::new(None));
        ui.on_spread_anchored({
            let anchor = Rc::clone(&anchor);
            move |w, h, single, failed, leading, trailing| {
                *anchor.borrow_mut() = Some((w, h, single, failed, leading, trailing));
            }
        });
        anchor
    }

    fn image(w: u32, h: u32) -> Arc<DecodedImage> {
        Arc::new(DecodedImage::new(vec![0; (w * h * 4) as usize], w, h).unwrap())
    }

    fn dispatch_double(ui: &ViewerWindow, backend: &FakeBackend, pages: &PageController) {
        pages.set_target(4, Some(5), false);
        pages.dispatch_spread(
            ui.as_weak(),
            backend.new_session(),
            SpreadDecodeRequest::double(
                PageSlot {
                    index: 4,
                    missing: true,
                },
                PageSlot {
                    index: 5,
                    missing: true,
                },
            ),
        );
    }

    #[test]
    fn single_page_anchor_reports_its_own_size() {
        let ui = headless_window();
        let anchor = capture_anchor(&ui);

        let backend = FakeBackend::new();
        let pages = PageController::new(backend.clone());
        pages.set_target(0, None, true);
        pages.dispatch_spread(
            ui.as_weak(),
            backend.new_session(),
            SpreadDecodeRequest::single(PageSlot {
                index: 0,
                missing: true,
            }),
        );
        backend.complete_spread(
            0,
            SpreadPages {
                leading: (0, Ok(image(3, 5))),
                trailing: None,
            },
        );

        assert_eq!(*anchor.borrow(), Some((3.0, 5.0, true, -1, 0, -1)));
    }

    #[test]
    fn double_spread_anchor_sums_widths_and_takes_max_height() {
        let ui = headless_window();
        let anchor = capture_anchor(&ui);
        let backend = FakeBackend::new();
        let pages = PageController::new(backend.clone());
        dispatch_double(&ui, &backend, &pages);
        backend.complete_spread(
            0,
            SpreadPages {
                leading: (4, Ok(image(3, 5))),
                trailing: Some((5, Ok(image(2, 7)))),
            },
        );

        assert_eq!(*anchor.borrow(), Some((5.0, 7.0, false, -1, 4, 5)));
    }

    #[test]
    fn failed_trailing_page_degrades_to_single_and_names_the_page() {
        let ui = headless_window();
        let anchor = capture_anchor(&ui);
        let decode_errors = Rc::new(Cell::new(0));
        ui.on_page_decode_error({
            let decode_errors = Rc::clone(&decode_errors);
            move |_| decode_errors.set(decode_errors.get() + 1)
        });
        let backend = FakeBackend::new();
        let pages = PageController::new(backend.clone());
        dispatch_double(&ui, &backend, &pages);
        backend.complete_spread(
            0,
            SpreadPages {
                leading: (4, Ok(image(3, 5))),
                trailing: Some((5, Err(BackendError::EmptyBook))),
            },
        );

        assert_eq!(*anchor.borrow(), Some((3.0, 5.0, true, 5, 4, 5)));
        assert_eq!(decode_errors.get(), 0);
    }
}
