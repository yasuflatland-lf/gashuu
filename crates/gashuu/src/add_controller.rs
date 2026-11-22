//! Probe picked books through the shared backend, then apply the batch in
//! input order. Each UI-thread completion records one outcome and advances
//! progress. Only the current generation can finalize and update the library.

use crate::backend::{BackendError, BookBackend};
use crate::book_path as location;
use crate::ui_marshal::{Generation, Ticket};
use crate::ViewerWindow;
use crate::{
    backend::{BookImporter, DiscardUpload, ImportedBook},
    picker::{forget_uploads, UploadHandle},
};
use gashuu_core::ArchivePolicy;
use gashuu_core::BookId;
use std::cell::Cell;
use std::fmt;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::rc::Rc;

/// Classification and diagnostic details used when applying a bulk add.
pub(crate) enum ProbeKind {
    /// Opened cleanly with `n` image pages — add it and record the count.
    Counted(NonZeroUsize),
    Imported {
        duplicate: bool,
        id: BookId,
        title: String,
        count: NonZeroUsize,
    },
    /// Opened cleanly but zero image pages (the empty-book rule) — skip.
    Empty,
    /// A RAR/CBR rejected because `ArchivePolicy::allow_rar` is false — skip.
    FormatDisabled { format: &'static str },
    /// Could not be opened at all (I/O error, unsupported format) — skip.
    Unreadable { error: String },
}

/// One probed source awaiting the UI-thread apply: its INPUT-order `index` (so
/// the apply half can restore input order regardless of probe completion order),
/// its selected `path`, and its classified `kind`. All fields `Send`.
pub(crate) struct ProbeOutcome {
    /// Position in the picked-paths input, used to re-sort to input order before
    /// applying (probes complete out of order on parallel workers).
    pub index: usize,
    pub subject: ProbeSubject,
    pub kind: ProbeKind,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProbeSubject {
    Path(PathBuf),
    Upload(String),
}

impl fmt::Display for ProbeSubject {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => write!(fmt, "{}", path.display()),
            Self::Upload(name) => fmt.write_str(name),
        }
    }
}

impl ProbeKind {
    pub(crate) fn from_probe(result: Result<NonZeroUsize, BackendError>) -> Self {
        match result {
            Ok(count) => Self::Counted(count),
            Err(BackendError::EmptyBook) => Self::Empty,
            Err(BackendError::FormatDisabled { format }) => Self::FormatDisabled { format },
            Err(error) => Self::Unreadable {
                error: error.to_string(),
            },
        }
    }
}

// The apply-half tests construct outcomes from real filesystem fixtures.
#[cfg(test)]
pub(crate) fn probe_path(index: usize, path: PathBuf, policy: ArchivePolicy) -> ProbeOutcome {
    let result = location::probe_page_count(&path, policy).map_err(|e| BackendError::from_core(&e));
    ProbeOutcome {
        index,
        subject: ProbeSubject::Path(path),
        kind: ProbeKind::from_probe(result),
    }
}

/// Completed fraction of a bulk add for the progress bar fill, in `[0.0, 1.0]`.
/// Returns `0.0` for an empty batch (no divide-by-zero) and clamps so a stray
/// over-count never overfills the bar. Pure so the mapping is unit-testable
/// without a Slint event loop.
pub(crate) fn add_progress_ratio(done: usize, total: usize) -> f32 {
    if total == 0 {
        return 0.0;
    }
    (done as f32 / total as f32).clamp(0.0, 1.0)
}

/// Owns the bulk-add async bookkeeping: the supersede `epoch` and the current
/// generation's probe-outcome accumulator. The apply half re-acquires its
/// collaborators in the `add-finalize` handler on the UI thread. Held in an `Rc` so the add handlers
/// (`on_add_books` / `on_add_folder`) and the finalize handler share one.
pub(crate) struct AddController {
    backend: Rc<dyn BookBackend>,
    generation: Generation<Vec<ProbeOutcome>>,
    /// The op tag ("add-books" / "add-folder") of the current generation, used
    /// only in the apply half's save-failure trace. Replaced on every `start`.
    op: Cell<&'static str>,
    import_batch: std::cell::RefCell<Option<(usize, DiscardUpload)>>,
    picker_notice: Cell<Option<String>>,
}

impl AddController {
    /// Build the controller. Call once during UI setup.
    pub(crate) fn new(backend: Rc<dyn BookBackend>) -> Self {
        Self {
            backend,
            generation: Generation::new(),
            op: Cell::new("add"),
            import_batch: Default::default(),
            picker_notice: Cell::new(None),
        }
    }

    pub(crate) fn take_picker_notice(&self) -> Option<String> {
        self.picker_notice.take()
    }

    fn begin(&self, op: &'static str) -> Ticket<Vec<ProbeOutcome>> {
        {
            self.picker_notice.take();
            if let Some((epoch, discard)) = self.import_batch.take() {
                for outcome in self.generation.take(epoch).unwrap_or_default() {
                    if let ProbeKind::Imported {
                        id,
                        duplicate: false,
                        ..
                    } = outcome.kind
                    {
                        discard(&id);
                    }
                }
            }
        }
        self.op.set(op);
        self.generation.begin()
    }

    /// Begin a fresh batch. An empty pick finalizes immediately.
    pub(crate) fn start(
        &self,
        ui_weak: slint::Weak<ViewerWindow>,
        paths: Vec<PathBuf>,
        policy: ArchivePolicy,
        op: &'static str,
    ) {
        let ticket = self.begin(op);
        let total = paths.len();
        if !begin_progress(&ui_weak, &ticket, total) {
            return;
        }
        for (index, path) in paths.into_iter().enumerate() {
            let id = location::request_id(&path);
            let ticket = ticket.clone();
            let weak = ui_weak.clone();
            let complete = move |result| {
                complete_source(
                    &weak,
                    &ticket,
                    total,
                    ProbeOutcome {
                        index,
                        subject: ProbeSubject::Path(path),
                        kind: ProbeKind::from_probe(result),
                    },
                );
            };
            match id {
                Ok(id) => self
                    .backend
                    .probe_page_count(&id, policy, Box::new(complete)),
                Err(error) => complete(Err(error)),
            }
        }
    }

    pub(crate) fn start_import(
        &self,
        ui_weak: slint::Weak<ViewerWindow>,
        uploads: Vec<UploadHandle>,
        policy: ArchivePolicy,
        op: &'static str,
        notice: Option<String>,
        importer: &dyn BookImporter,
    ) {
        let ticket = self.begin(op);
        let discard = importer.discarder();
        self.import_batch
            .replace(Some((ticket.id(), discard.clone())));
        self.picker_notice.set(notice);
        let total = uploads.len();
        if !begin_progress(&ui_weak, &ticket, total) {
            forget_uploads(&uploads);
            return;
        }

        for (index, upload) in uploads.into_iter().enumerate() {
            let subject = ProbeSubject::Upload(upload.name.clone());
            let ticket = ticket.clone();
            let weak = ui_weak.clone();
            let discard = discard.clone();
            importer.import(
                upload,
                policy,
                Box::new(move |result| {
                    let kind = match result {
                        Ok(ImportedBook {
                            id,
                            title,
                            page_count,
                            duplicate,
                        }) => {
                            if !ticket.is_current() || weak.upgrade().is_none() {
                                if !duplicate {
                                    discard(&id);
                                }
                                return;
                            }
                            ProbeKind::Imported {
                                id,
                                title,
                                count: page_count,
                                duplicate,
                            }
                        }
                        Err(error) => ProbeKind::from_probe(Err(error)),
                    };
                    complete_source(
                        &weak,
                        &ticket,
                        total,
                        ProbeOutcome {
                            index,
                            subject,
                            kind,
                        },
                    );
                }),
            );
        }
    }

    /// Drain this generation's probe outcomes for the apply half, in INPUT
    /// order, IFF `my_epoch` is still current. Returns `None` for a superseded
    /// generation (a second add started since this batch was dispatched), so the
    /// finalize handler drops a stale batch instead of clobbering the live one.
    /// The `op` tag is returned alongside for the save-failure trace.
    pub(crate) fn take_outcomes(
        &self,
        my_epoch: usize,
    ) -> Option<(Vec<ProbeOutcome>, &'static str)> {
        let mut outcomes = self.generation.take(my_epoch)?;
        // Probes complete out of order on parallel workers; restore input order so
        // the apply half adds books and reports skips deterministically.
        outcomes.sort_by_key(|o| o.index);
        Some((outcomes, self.op.get()))
    }
}

fn begin_progress(
    weak: &slint::Weak<ViewerWindow>,
    ticket: &Ticket<Vec<ProbeOutcome>>,
    total: usize,
) -> bool {
    let Some(ui) = weak.upgrade() else {
        return false;
    };
    if total == 0 {
        ui.invoke_add_finalize(ticket.id() as i32);
        return false;
    }
    ui.invoke_add_progress(0, total as i32);
    ticket.is_current()
}

fn complete_source(
    weak: &slint::Weak<ViewerWindow>,
    ticket: &Ticket<Vec<ProbeOutcome>>,
    total: usize,
    outcome: ProbeOutcome,
) {
    let mut done = 0;
    if !ticket.deliver(|outcomes| {
        outcomes.push(outcome);
        done = outcomes.len();
    }) {
        return;
    }
    if let Some(ui) = weak.upgrade() {
        ui.invoke_add_progress(done as i32, total as i32);
        if done == total && ticket.is_current() {
            ui.invoke_add_finalize(ticket.id() as i32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::backend::{
        fake::{Call, FakeBackend},
        BackendError,
    };
    use slint::ComponentHandle;
    use std::cell::RefCell;

    #[test]
    fn probe_kind_from_probe_maps_all_four_arms() {
        let count = NonZeroUsize::new(3).unwrap();
        assert!(matches!(ProbeKind::from_probe(Ok(count)), ProbeKind::Counted(n) if n == count));
        assert!(matches!(
            ProbeKind::from_probe(Err(BackendError::EmptyBook)),
            ProbeKind::Empty
        ));
        assert!(matches!(
            ProbeKind::from_probe(Err(BackendError::FormatDisabled { format: "RAR" })),
            ProbeKind::FormatDisabled { format: "RAR" }
        ));
        assert!(
            matches!(ProbeKind::from_probe(Err(BackendError::Missing { detail: "missing".into() })), ProbeKind::Unreadable {error} if error == "missing")
        );
    }

    #[test]
    fn add_start_probes_every_path_once_and_finalizes_after_last() {
        let ui = crate::cover_loader::test_window();
        let fake = FakeBackend::new();
        let controller = AddController::new(fake.clone());
        let ticks = Rc::new(RefCell::new(Vec::new()));
        let finalized = Rc::new(RefCell::new(Vec::new()));
        ui.on_add_progress({
            let ticks = ticks.clone();
            move |done, total| ticks.borrow_mut().push((done, total))
        });
        ui.on_add_finalize({
            let finalized = finalized.clone();
            move |epoch| finalized.borrow_mut().push(epoch)
        });
        let paths = ["a", "b", "c"].map(PathBuf::from);
        controller.start(
            ui.as_weak(),
            paths.to_vec(),
            ArchivePolicy::default(),
            "add-books",
        );
        assert_eq!(
            fake.calls(),
            ["a", "b", "c"].map(|s| Call::ProbePageCount(gashuu_core::BookId::new(s)))
        );
        for index in [2, 0] {
            fake.complete_probe(index, Ok(NonZeroUsize::MIN));
            assert!(finalized.borrow().is_empty());
        }
        fake.complete_probe(0, Err(BackendError::EmptyBook));
        assert_eq!(*ticks.borrow(), [(0, 3), (1, 3), (2, 3), (3, 3)]);
        assert_eq!(*finalized.borrow(), [1]);
        let (outcomes, op) = controller.take_outcomes(1).unwrap();
        assert_eq!(op, "add-books");
        assert!(matches!(outcomes[1].kind, ProbeKind::Empty));
        assert_eq!(
            outcomes.into_iter().map(|o| o.subject).collect::<Vec<_>>(),
            paths.map(ProbeSubject::Path)
        );
    }

    #[test]
    fn add_superseded_batch_probe_completions_are_ignored() {
        let ui = crate::cover_loader::test_window();
        let fake = FakeBackend::new();
        let controller = AddController::new(fake.clone());
        let ticks = Rc::new(RefCell::new(Vec::new()));
        let finalized = Rc::new(RefCell::new(Vec::new()));
        ui.on_add_progress({
            let ticks = ticks.clone();
            move |done, total| ticks.borrow_mut().push((done, total))
        });
        ui.on_add_finalize({
            let finalized = finalized.clone();
            move |epoch| finalized.borrow_mut().push(epoch)
        });

        controller.start(
            ui.as_weak(),
            ["old-a", "old-b"].map(PathBuf::from).to_vec(),
            ArchivePolicy::default(),
            "add-books",
        );
        let current = PathBuf::from("current");
        controller.start(
            ui.as_weak(),
            vec![current.clone()],
            ArchivePolicy::default(),
            "add-books",
        );

        fake.complete_probe(0, Ok(NonZeroUsize::MIN));
        fake.complete_probe(0, Ok(NonZeroUsize::MIN));
        assert_eq!(*ticks.borrow(), [(0, 2), (0, 1)]);
        assert!(finalized.borrow().is_empty());
        assert!(controller.take_outcomes(1).is_none());

        fake.complete_probe(0, Ok(NonZeroUsize::MIN));
        assert_eq!(*ticks.borrow(), [(0, 2), (0, 1), (1, 1)]);
        assert_eq!(*finalized.borrow(), [2]);
        let (outcomes, _) = controller.take_outcomes(2).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].subject, ProbeSubject::Path(current));
    }

    #[cfg(unix)]
    #[test]
    fn add_non_utf8_path_is_unreadable_without_dispatch() {
        use std::os::unix::ffi::OsStrExt;
        let ui = crate::cover_loader::test_window();
        let fake = FakeBackend::new();
        let controller = AddController::new(fake.clone());
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(&[255]));
        controller.start(
            ui.as_weak(),
            vec![path.clone()],
            ArchivePolicy::default(),
            "add-folder",
        );
        assert!(fake.calls().is_empty());
        let (outcomes, _) = controller.take_outcomes(1).unwrap();
        assert_eq!(outcomes[0].subject, ProbeSubject::Path(path));
        assert!(matches!(&outcomes[0].kind, ProbeKind::Unreadable {error} if !error.is_empty()));
    }

    /// Deliver one worker result without a Slint event loop.
    fn push_outcome(ticket: &Ticket<Vec<ProbeOutcome>>, index: usize) {
        assert!(ticket.deliver(|acc| {
            acc.push(ProbeOutcome {
                index,
                subject: ProbeSubject::Path(PathBuf::from(format!("/p{index}"))),
                kind: ProbeKind::Empty,
            });
        }));
    }

    /// The PARALLEL concern: probes complete out of order on the workers, but
    /// `take_outcomes` restores INPUT order before the apply half runs. Pushing
    /// indices 2, 0, 1 (completion order) must drain as 0, 1, 2 (input order).
    /// Verified without any wall-clock dependency.
    #[test]
    fn take_outcomes_restores_input_order() {
        let ctrl = AddController::new(crate::backend::fake::FakeBackend::new());
        let ticket = ctrl.generation.begin();
        for index in [2usize, 0, 1] {
            push_outcome(&ticket, index);
        }
        let (outcomes, _op) = ctrl
            .take_outcomes(ticket.id())
            .expect("current generation drains");
        let order: Vec<usize> = outcomes.iter().map(|o| o.index).collect();
        assert_eq!(order, vec![0, 1, 2], "outcomes restored to input order");
        // A second drain of the same generation yields nothing (already taken).
        let (again, _) = ctrl.take_outcomes(ticket.id()).expect("still current");
        assert!(again.is_empty(), "outcomes drained exactly once");
    }

    /// The SUPERSEDE guard: a finalize from an older generation (its `my_epoch`
    /// no longer matches the bumped epoch) drains NOTHING, so a second add started
    /// mid-probe cannot be clobbered by the first's stale finalize. Verified via
    /// the generation counter, not timing.
    #[test]
    fn take_outcomes_drops_superseded_generation() {
        let ctrl = AddController::new(crate::backend::fake::FakeBackend::new());
        let first = ctrl.generation.begin();
        push_outcome(&first, 0);
        ctrl.generation.begin();
        assert!(
            ctrl.take_outcomes(first.id()).is_none(),
            "a superseded generation's finalize drains nothing"
        );
    }

    /// The bar fill fraction is done/total, with a zero-guard for the empty
    /// batch and a clamp so an over-count can never exceed a full bar.
    #[test]
    fn add_progress_ratio_maps_counts_to_fraction() {
        assert_eq!(add_progress_ratio(0, 18), 0.0, "start of batch is empty");
        assert_eq!(add_progress_ratio(9, 18), 0.5, "half done is half full");
        assert_eq!(add_progress_ratio(18, 18), 1.0, "all done is full");
        assert_eq!(
            add_progress_ratio(0, 0),
            0.0,
            "empty batch never divides by zero"
        );
        assert_eq!(add_progress_ratio(20, 18), 1.0, "over-count clamps to full");
    }
}

#[cfg(test)]
#[path = "add_import_tests.rs"]
mod import_tests;
