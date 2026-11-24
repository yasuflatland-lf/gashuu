use super::*;
use crate::backend::{
    fake::{Call, FakeBackend},
    CancelPending, DiscardUpload, Done, ImportedBook,
};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

type Completion = Done<Result<ImportedBook, BackendError>>;

#[derive(Default)]
struct FakeImporter {
    pending: RefCell<Vec<(UploadHandle, Completion)>>,
    discarded: Arc<Mutex<Vec<BookId>>>,
    cancel_count: Rc<Cell<usize>>,
    cancel_counts_at_import: RefCell<Vec<usize>>,
}

impl BookImporter for FakeImporter {
    fn import(&self, file: UploadHandle, _: ArchivePolicy, done: Completion) {
        self.cancel_counts_at_import
            .borrow_mut()
            .push(self.cancel_count.get());
        self.pending.borrow_mut().push((file, done));
    }

    fn sweep(&self, _: Vec<BookId>, _: bool, _: Done<crate::backend::SweepReport>) {
        unreachable!()
    }
    fn discard(&self, id: &BookId) {
        self.discarder()(id);
    }

    fn discarder(&self) -> DiscardUpload {
        let discarded = self.discarded.clone();
        Arc::new(move |id| discarded.lock().unwrap().push(id.clone()))
    }

    fn pending_canceller(&self) -> CancelPending {
        let cancel_count = self.cancel_count.clone();
        Rc::new(move || cancel_count.set(cancel_count.get() + 1))
    }
}

impl FakeImporter {
    fn deliver(&self, key: u32, result: Result<ImportedBook, BackendError>) {
        let index = self
            .pending
            .borrow()
            .iter()
            .position(|(file, _)| file.key == key)
            .unwrap();
        let (_, done) = self.pending.borrow_mut().remove(index);
        done(result);
    }
}

fn upload(key: u32) -> UploadHandle {
    UploadHandle {
        key,
        name: format!("{key}.cbz"),
        size: 1,
    }
}

fn imported(key: u32) -> ImportedBook {
    ImportedBook {
        id: BookId::new(format!("books/{key}/book.cbz")),
        title: format!("Book {key}"),
        page_count: NonZeroUsize::MIN,
        duplicate: false,
    }
}

#[test]
fn import_dispatches_a_batch_and_finalizes_in_input_order() {
    let ui = crate::cover_loader::test_window();
    let controller = AddController::new(FakeBackend::new());
    let importer = FakeImporter::default();
    let ticks = Rc::new(RefCell::new(Vec::new()));
    let epochs = Rc::new(RefCell::new(Vec::new()));
    ui.on_add_progress({
        let ticks = ticks.clone();
        move |done, total| ticks.borrow_mut().push((done, total))
    });
    ui.on_add_finalize({
        let epochs = epochs.clone();
        move |epoch| epochs.borrow_mut().push(epoch)
    });
    controller.start_import(
        ui.as_weak(),
        (1..=4).map(upload).collect(),
        ArchivePolicy::default(),
        "add-books",
        Some("Unsupported selection".into()),
        &importer,
    );
    assert_eq!(importer.pending.borrow().len(), 4);
    importer.deliver(
        4,
        Err(BackendError::Other {
            detail: "unreadable".into(),
        }),
    );
    importer.deliver(2, Err(BackendError::EmptyBook));
    importer.deliver(
        3,
        Err(BackendError::FormatDisabled {
            kind: gashuu_core::ArchiveKind::Rar,
        }),
    );
    assert!(epochs.borrow().is_empty());
    importer.deliver(1, Ok(imported(1)));
    assert_eq!(*ticks.borrow(), [(0, 4), (1, 4), (2, 4), (3, 4), (4, 4)]);
    assert_eq!(*epochs.borrow(), [1]);
    let (outcomes, op) = controller.take_outcomes(1).unwrap();
    assert_eq!(op, "add-books");
    assert_eq!(
        controller.take_picker_notice().as_deref(),
        Some("Unsupported selection")
    );
    assert_eq!(
        outcomes
            .iter()
            .map(|o| o.subject.to_string())
            .collect::<Vec<_>>(),
        ["1.cbz", "2.cbz", "3.cbz", "4.cbz"]
    );
    assert!(matches!(&outcomes[0].kind, ProbeKind::Imported { id, .. } if id == &imported(1).id));
    assert!(matches!(outcomes[1].kind, ProbeKind::Empty));
    assert!(matches!(
        outcomes[2].kind,
        ProbeKind::FormatDisabled {
            kind: gashuu_core::ArchiveKind::Rar
        }
    ));
    assert!(
        matches!(&outcomes[3].kind, ProbeKind::Unreadable { error: BackendError::Other { detail } } if detail == "unreadable")
    );
}

#[test]
fn superseding_an_import_discards_early_and_late_successes() {
    let ui = crate::cover_loader::test_window();
    let controller = AddController::new(FakeBackend::new());
    let importer = FakeImporter::default();
    let start = |keys: &[u32]| {
        controller.start_import(
            ui.as_weak(),
            keys.iter().copied().map(upload).collect(),
            ArchivePolicy::default(),
            "add-books",
            None,
            &importer,
        );
    };
    start(&[1, 2, 4, 5]);
    let duplicate = |key| ImportedBook {
        duplicate: true,
        ..imported(key)
    };
    importer.deliver(4, Ok(duplicate(4)));
    importer.deliver(1, Ok(imported(1)));
    start(&[3]);
    importer.deliver(5, Ok(duplicate(5)));
    importer.deliver(2, Ok(imported(2)));
    importer.deliver(3, Ok(duplicate(3)));
    assert!(controller.take_outcomes(1).is_none());
    let mut library = gashuu_core::Library::new();
    library.add(imported(3).id, "Already on shelf".into());
    let (outcomes, _) = controller.take_outcomes(2).unwrap();
    assert_eq!(outcomes.len(), 1);
    let report = crate::add_books::apply_outcomes(&mut library, outcomes);
    assert!(report.added.is_empty());
    assert_eq!(report.skipped, 0);
    assert_eq!(
        crate::add_books::select_add_notice(report.added.len(), report.skipped),
        crate::add_books::AddNotice::AlreadyInLibrary
    );
    assert_eq!(
        *importer.discarded.lock().unwrap(),
        [imported(1).id, imported(2).id]
    );
}

#[test]
fn superseding_an_import_batch_cancels_its_queued_imports() {
    let ui = crate::cover_loader::test_window();
    let controller = AddController::new(FakeBackend::new());
    let importer = FakeImporter::default();
    let start = |keys: &[u32]| {
        controller.start_import(
            ui.as_weak(),
            keys.iter().copied().map(upload).collect(),
            ArchivePolicy::default(),
            "add-books",
            None,
            &importer,
        );
    };
    start(&[1, 2]);
    start(&[3, 4]);

    assert_eq!(importer.cancel_count.get(), 1);
    assert_eq!(*importer.cancel_counts_at_import.borrow(), [0, 0, 1, 1]);
}

#[test]
fn cancel_import_invokes_the_pending_canceller() {
    let ui = crate::cover_loader::test_window();
    let importer = FakeBackend::new();
    let controller = AddController::new(importer.clone());
    controller.start_import(
        ui.as_weak(),
        vec![upload(1), upload(2)],
        ArchivePolicy::default(),
        "add-books",
        None,
        importer.as_ref(),
    );
    importer.complete_import(Ok(imported(1)));

    assert!(controller.cancel_import());
    assert!(!controller.cancel_import());
    assert_eq!(
        importer.all_calls(),
        [Call::CancelPending, Call::Discard(imported(1).id)]
    );
}

#[test]
fn finished_batch_is_not_cancelled_by_the_next_batch() {
    let ui = crate::cover_loader::test_window();
    let controller = AddController::new(FakeBackend::new());
    let importer = FakeImporter::default();
    let start = |key| {
        controller.start_import(
            ui.as_weak(),
            vec![upload(key)],
            ArchivePolicy::default(),
            "add-books",
            None,
            &importer,
        );
    };
    start(1);
    importer.deliver(1, Ok(imported(1)));
    assert_eq!(controller.take_outcomes(1).unwrap().0.len(), 1);

    start(2);

    assert_eq!(importer.cancel_count.get(), 0);
    assert_eq!(*importer.cancel_counts_at_import.borrow(), [0, 0]);
}
