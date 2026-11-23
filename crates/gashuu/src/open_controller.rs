//! Routes backend open completions to the current generation and reclaims stale sessions.
use crate::backend::{BackendError, BookBackend, OpenedBook, SessionId};
use crate::ui_marshal::Generation;
use crate::ViewerWindow;
use gashuu_core::{ArchivePolicy, BookId, CacheConfig};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

struct SessionGuard {
    session: Option<SessionId>,
    orphans: Arc<Mutex<Vec<SessionId>>>,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            self.orphans
                .lock()
                .expect("orphan queue poisoned")
                .push(session);
        }
    }
}

pub(crate) struct PendingOpen {
    request: BookId,
    result: Result<OpenedBook, BackendError>,
    guard: SessionGuard,
}

impl PendingOpen {
    pub(crate) fn into_result(mut self) -> (BookId, Result<OpenedBook, BackendError>) {
        self.guard.session = None;
        (self.request, self.result)
    }
}

pub(crate) struct OpenController {
    backend: Rc<dyn BookBackend>,
    generation: Generation<Option<PendingOpen>>,
    orphans: Arc<Mutex<Vec<SessionId>>>,
}

impl OpenController {
    pub(crate) fn new(backend: Rc<dyn BookBackend>) -> Self {
        Self {
            backend,
            generation: Generation::new(),
            orphans: Arc::default(),
        }
    }

    fn close_orphans(&self) {
        let sessions = std::mem::take(&mut *self.orphans.lock().expect("orphan queue poisoned"));
        for session in sessions {
            self.backend.close(session);
        }
    }

    pub(crate) fn start(
        &self,
        weak: slint::Weak<ViewerWindow>,
        request: BookId,
        policy: ArchivePolicy,
        cache: CacheConfig,
    ) {
        self.close_orphans();
        let notify = move |epoch| {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_open_finalize(epoch as i32);
            }
        };
        self.start_with(request, policy, cache, notify);
    }

    fn start_with(
        &self,
        request: BookId,
        policy: ArchivePolicy,
        cache: CacheConfig,
        notify: impl FnOnce(usize) + Send + 'static,
    ) {
        let ticket = self.generation.begin();
        let orphans = self.orphans.clone();
        let id = request.clone();
        self.backend.open(
            &id,
            policy,
            cache,
            Box::new(move |result| {
                let guard = SessionGuard {
                    session: result.as_ref().ok().map(|opened| opened.session),
                    orphans,
                };
                let pending = PendingOpen {
                    request,
                    result,
                    guard,
                };
                let _ = ticket.deliver(|slot| *slot = Some(pending));
                notify(ticket.id());
            }),
        );
    }

    pub(crate) fn take_outcome(&self, epoch: usize) -> Option<PendingOpen> {
        self.close_orphans();
        self.generation.take(epoch).flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fake::{Call, FakeBackend};

    #[test]
    fn superseded_completion_notifies_with_its_own_epoch() {
        let backend = FakeBackend::new();
        let ctrl = OpenController::new(backend.clone());
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let a = BookId::new("a");
        for id in [a.clone(), BookId::new("b")] {
            let notifications = Arc::clone(&notifications);
            ctrl.start_with(id, Default::default(), Default::default(), move |epoch| {
                notifications.lock().unwrap().push(epoch);
            });
        }
        backend.complete_open(
            0,
            Ok(OpenedBook {
                session: backend.new_session(),
                id: a,
                page_count: 3,
                skipped: 0,
                listing_truncated: false,
                is_dir: true,
            }),
        );
        assert_eq!(*notifications.lock().unwrap(), [1]);
    }

    #[test]
    fn superseded_session_is_closed_on_its_own_notification() {
        let backend = FakeBackend::new();
        let ctrl = OpenController::new(backend.clone());
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let a = BookId::new("a");
        let b = BookId::new("b");
        for id in [a.clone(), b.clone()] {
            let notifications = Arc::clone(&notifications);
            ctrl.start_with(id, Default::default(), Default::default(), move |epoch| {
                notifications.lock().unwrap().push(epoch);
            });
        }
        let current = backend.new_session();
        backend.complete_open(
            1,
            Ok(OpenedBook {
                session: current,
                id: b.clone(),
                page_count: 5,
                skipped: 0,
                listing_truncated: false,
                is_dir: true,
            }),
        );
        assert_eq!(*notifications.lock().unwrap(), [2]);
        let (request, result) = ctrl.take_outcome(2).unwrap().into_result();
        assert_eq!(request, b);
        assert_eq!(result.unwrap().session, current);

        let old = backend.new_session();
        backend.complete_open(
            0,
            Ok(OpenedBook {
                session: old,
                id: a,
                page_count: 3,
                skipped: 0,
                listing_truncated: false,
                is_dir: true,
            }),
        );
        assert_eq!(*notifications.lock().unwrap(), [2, 1]);
        let epoch = notifications.lock().unwrap()[1];
        assert!(ctrl.take_outcome(epoch).is_none());
        assert_eq!(
            backend
                .calls()
                .into_iter()
                .filter(|call| matches!(call, Call::Close(_)))
                .collect::<Vec<_>>(),
            [Call::Close(old)]
        );
        assert!(ctrl.take_outcome(2).is_none());
    }

    #[test]
    fn current_completion_still_delivers_once() {
        let backend = FakeBackend::new();
        let ctrl = OpenController::new(backend.clone());
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&notifications);
        let id = BookId::new("current");
        ctrl.start_with(
            id.clone(),
            Default::default(),
            Default::default(),
            move |epoch| recorder.lock().unwrap().push(epoch),
        );
        let session = backend.new_session();
        backend.complete_open(
            0,
            Ok(OpenedBook {
                session,
                id: id.clone(),
                page_count: 5,
                skipped: 0,
                listing_truncated: false,
                is_dir: true,
            }),
        );
        assert_eq!(*notifications.lock().unwrap(), [1]);
        let epoch = notifications.lock().unwrap()[0];
        let (request, result) = ctrl.take_outcome(epoch).unwrap().into_result();
        assert_eq!(request, id);
        assert_eq!(result.unwrap().session, session);
        assert!(ctrl.take_outcome(epoch).is_none());
        assert_eq!(*notifications.lock().unwrap(), [1]);
        assert!(!backend
            .calls()
            .iter()
            .any(|call| matches!(call, Call::Close(_))));
    }

    #[test]
    fn failed_superseded_open_notifies_without_closing() {
        let backend = FakeBackend::new();
        let ctrl = OpenController::new(backend.clone());
        let notifications = Arc::new(Mutex::new(Vec::new()));
        for id in [BookId::new("a"), BookId::new("b")] {
            let notifications = Arc::clone(&notifications);
            ctrl.start_with(id, Default::default(), Default::default(), move |epoch| {
                notifications.lock().unwrap().push(epoch);
            });
        }
        backend.complete_open(
            0,
            Err(BackendError::Missing {
                detail: "gone".into(),
            }),
        );
        assert_eq!(*notifications.lock().unwrap(), [1]);
        assert!(ctrl.orphans.lock().unwrap().is_empty());
        let epoch = notifications.lock().unwrap()[0];
        assert!(ctrl.take_outcome(epoch).is_none());
        assert!(!backend
            .calls()
            .iter()
            .any(|call| matches!(call, Call::Close(_))));
    }

    #[test]
    fn superseded_open_session_is_closed() {
        let backend = FakeBackend::new();
        let ctrl = OpenController::new(backend.clone());
        let a = BookId::new("a");
        let b = BookId::new("b");
        for id in [&a, &b] {
            ctrl.start(
                Default::default(),
                id.clone(),
                ArchivePolicy::default(),
                CacheConfig::default(),
            );
        }
        let old = backend.new_session();
        let current = backend.new_session();
        for (session, id, page_count) in [(old, a, 3), (current, b.clone(), 0)] {
            backend.complete_open(
                0,
                Ok(OpenedBook {
                    session,
                    id,
                    page_count,
                    skipped: 0,
                    listing_truncated: false,
                    is_dir: true,
                }),
            );
        }
        assert!(ctrl.take_outcome(1).is_none());
        let (request, result) = ctrl.take_outcome(2).unwrap().into_result();
        let opened = result.unwrap();
        assert_eq!(request, b);
        assert_eq!(opened.session, current);
        assert_eq!(opened.page_count, 0);
        assert!(ctrl.take_outcome(2).is_none());
        assert_eq!(
            backend
                .calls()
                .into_iter()
                .filter(|c| matches!(c, Call::Close(_)))
                .collect::<Vec<_>>(),
            [Call::Close(old)]
        );
    }

    #[test]
    fn take_outcome_drops_superseded_and_drains_current_once() {
        let backend = FakeBackend::new();
        let ctrl = OpenController::new(backend.clone());
        let old = backend.new_session();
        let first = BookId::new("first");
        ctrl.start(
            Default::default(),
            first.clone(),
            Default::default(),
            Default::default(),
        );
        backend.complete_open(
            0,
            Ok(OpenedBook {
                session: old,
                id: first,
                page_count: 3,
                skipped: 0,
                listing_truncated: false,
                is_dir: true,
            }),
        );
        let next = BookId::new("next");
        ctrl.start(
            Default::default(),
            next.clone(),
            Default::default(),
            Default::default(),
        );
        let session = backend.new_session();
        backend.complete_open(
            0,
            Ok(OpenedBook {
                session,
                id: next.clone(),
                page_count: 5,
                skipped: 1,
                listing_truncated: true,
                is_dir: false,
            }),
        );
        assert!(ctrl.take_outcome(1).is_none());
        let (request, result) = ctrl.take_outcome(2).unwrap().into_result();
        let opened = result.unwrap();
        assert_eq!(request, next);
        assert_eq!(opened.id, next);
        assert_eq!(opened.session, session);
        assert_eq!(opened.page_count, 5);
        assert_eq!(opened.skipped, 1);
        assert!(opened.listing_truncated);
        assert!(ctrl.take_outcome(2).is_none());
        assert_eq!(
            backend
                .calls()
                .iter()
                .filter(|c| matches!(c, Call::Close(s) if *s == old))
                .count(),
            1
        );
    }

    #[test]
    fn missing_open_preserves_backend_error() {
        let backend = FakeBackend::new();
        let ctrl = OpenController::new(backend.clone());
        ctrl.start(
            Default::default(),
            BookId::new("missing"),
            Default::default(),
            Default::default(),
        );
        backend.complete_open(
            0,
            Err(BackendError::Missing {
                detail: "gone".into(),
            }),
        );
        assert!(matches!(
            ctrl.take_outcome(1).unwrap().into_result().1,
            Err(BackendError::Missing { .. })
        ));
    }
}
