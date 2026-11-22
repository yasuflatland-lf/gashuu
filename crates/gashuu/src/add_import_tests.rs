use super::*;
use crate::backend::{fake::FakeBackend, DiscardUpload, Done, ImportedBook};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

type Completion = Done<Result<ImportedBook, BackendError>>;

#[derive(Default)]
struct FakeImporter {
    pending: RefCell<Vec<(UploadHandle, Completion)>>,
    discarded: Arc<Mutex<Vec<BookId>>>,
}

impl BookImporter for FakeImporter {
    fn import(&self, file: UploadHandle, _: ArchivePolicy, done: Completion) {
        self.pending.borrow_mut().push((file, done));
    }

    fn discard(&self, id: &BookId) {
        self.discarder()(id);
    }

    fn discarder(&self) -> DiscardUpload {
        let discarded = self.discarded.clone();
        Arc::new(move |id| discarded.lock().unwrap().push(id.clone()))
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
    importer.deliver(3, Err(BackendError::FormatDisabled { format: "RAR" }));
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
        ProbeKind::FormatDisabled { format: "RAR" }
    ));
    assert!(matches!(&outcomes[3].kind, ProbeKind::Unreadable { error } if error == "unreadable"));
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
    start(&[1, 2]);
    importer.deliver(1, Ok(imported(1)));
    start(&[3]);
    importer.deliver(2, Ok(imported(2)));
    importer.deliver(3, Ok(imported(3)));
    assert!(controller.take_outcomes(1).is_none());
    assert_eq!(controller.take_outcomes(2).unwrap().0.len(), 1);
    assert_eq!(
        *importer.discarded.lock().unwrap(),
        [imported(1).id, imported(2).id]
    );
}
