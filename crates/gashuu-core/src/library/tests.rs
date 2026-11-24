use super::*;

// --- display_title tests (the pure title rule) ---

#[test]
fn display_title_uses_dir_name_for_real_folder() {
    // A real existing folder: the rule takes the directory name, not the stem
    // (which would strip a dotted suffix).
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Series.1997");
    std::fs::create_dir(&folder).unwrap();
    assert_eq!(display_title(&folder, true), "Series.1997");
}

#[test]
fn display_title_uses_file_stem_for_archive_path() {
    // A file path (non-directory): the rule takes the file stem, dropping the
    // extension.
    assert_eq!(
        display_title(Path::new("/manga/Cool Title.cbz"), false),
        "Cool Title"
    );
}

#[test]
fn display_title_keeps_leading_dot_name_as_stem() {
    // A dotfile like `.cbz` keeps the whole `.cbz` as its (non-empty) stem, so the
    // empty filter never fires and the title is the dotfile name, not the fallback.
    assert_eq!(display_title(Path::new("/manga/.cbz"), false), ".cbz");
}

#[test]
fn display_title_falls_back_to_lossy_path_when_no_file_component() {
    // A non-dir path whose `file_stem` is None (a trailing `..`) hits the fallback
    // arm: the lossy full path string is used so the title is never empty.
    let title = display_title(Path::new("/manga/.."), false);
    assert!(!title.is_empty());
    assert_eq!(title, "/manga/..");
}

#[test]
fn display_title_falls_back_to_lossy_path_for_root() {
    // A root-ish path has no file_name / file_stem at all, so the rule falls
    // back to the lossy full path string.
    let title = display_title(Path::new("/"), false);
    assert!(!title.is_empty());
    assert_eq!(title, "/");
}

fn book_from_path(path: &Path, is_dir: bool) -> Book {
    Book::new(
        BookId::from_canonical_path(path).unwrap(),
        display_title(path, is_dir),
    )
}

fn add(lib: &mut Library, raw: &str) -> BookId {
    lib.add(BookId::new(raw), display_title(Path::new(raw), false))
        .unwrap()
        .clone()
}

fn title(id: &BookId) -> String {
    display_title(Path::new(id.as_str()), false)
}

#[test]
fn display_title_is_pure_given_is_dir() {
    let path = Path::new("/manga/Series.1997");
    assert_eq!(display_title(path, true), "Series.1997");
    assert_eq!(display_title(path, false), "Series");
}

#[test]
fn book_lookup_by_id() {
    let mut lib = Library::new();
    let id = add(&mut lib, "opaque-id");
    assert_eq!(lib.book(&id).unwrap().id(), &id);
    assert!(lib.book(&BookId::new("missing")).is_none());
}

#[test]
fn remove_many_report_order_matches_path_order() {
    let mut lib = Library::new();
    let spaced = add(&mut lib, "/m/A 2.cbz");
    let nested = add(&mut lib, "/m/A/1.cbz");
    let report = lib.remove_many(&[spaced.clone(), nested.clone()]);
    assert_eq!(report.removed, vec![nested, spaced]);
}

#[test]
fn register_opened_uses_caller_title_for_a_new_book_and_keeps_stored_title_on_reopen() {
    let mut lib = Library::new();
    let id = BookId::new("opaque-id");
    lib.register_opened(&id, "First title", None);
    assert_eq!(lib.book(&id).unwrap().title(), "First title");
    lib.register_opened(&id, "Changed title", None);
    assert_eq!(lib.book(&id).unwrap().title(), "First title");
}

#[test]
fn canonical_identity_returns_the_canonical_path_for_a_real_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.cbz");
    std::fs::write(&path, b"").unwrap();

    assert_eq!(canonical_identity(&path), path.canonicalize().unwrap());
}

#[test]
fn canonical_identity_falls_back_for_a_missing_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.cbz");

    assert_eq!(canonical_identity(&path), path);
}

#[cfg(unix)]
fn utf8_symlink_to_non_utf8_target() -> (tempfile::TempDir, PathBuf) {
    // Unix permits arbitrary path bytes and symlinks, which are needed to
    // reproduce a UTF-8 user selection resolving to a non-UTF-8 identity.
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let non_utf8_dir = dir.path().join(OsStr::from_bytes(b"bad\xffdir"));
    std::fs::create_dir(&non_utf8_dir).unwrap();
    let target = non_utf8_dir.join("book.cbz");
    std::fs::write(&target, b"").unwrap();
    let link = dir.path().join("book-link.cbz");
    symlink(target, &link).unwrap();
    (dir, link)
}

#[cfg(unix)]
#[test]
fn canonical_identity_falls_back_when_the_canonical_form_is_not_utf8() {
    // macOS filesystems reject invalid-UTF-8 names; the defect is exposed on
    // Unix platforms that permit arbitrary path bytes.
    if cfg!(target_os = "macos") {
        return;
    }
    let (_dir, link) = utf8_symlink_to_non_utf8_target();

    assert_eq!(canonical_identity(&link), link);
}

#[cfg(unix)]
#[test]
fn a_library_built_through_canonical_identity_always_serializes() {
    // macOS filesystems reject invalid-UTF-8 names; the defect is exposed on
    // Unix platforms that permit arbitrary path bytes.
    if cfg!(target_os = "macos") {
        return;
    }
    let (_dir, link) = utf8_symlink_to_non_utf8_target();
    let mut lib = Library::new();

    assert!(lib
        .add(
            BookId::from_canonical_path(&canonical_identity(&link)).unwrap(),
            display_title(&link, false)
        )
        .is_some());
    let value: serde_json::Value =
        serde_json::from_str(&lib.to_json().expect("UTF-8 identity must serialize")).unwrap();

    assert_eq!(value["books"][0]["path"].as_str(), link.to_str());
}

#[test]
fn new_library_is_empty() {
    let lib = Library::new();
    assert!(lib.books().is_empty());
}

#[test]
fn clear_empty_library_returns_false() {
    let mut lib = Library::new();

    assert!(
        !lib.clear(),
        "clearing an already-empty library changes no persisted state"
    );
    assert!(lib.books().is_empty());
    assert_eq!(lib.last_opened(), None);
}

#[test]
fn clear_non_empty_library_removes_books_and_last_opened() {
    let mut lib = Library::new();
    let first = BookId::new("/manga/a.cbz");
    let second = BookId::new("/manga/b.cbz");
    assert!(lib.add(first.clone(), title(&first)).is_some());
    assert!(lib.add(second.clone(), title(&second)).is_some());
    lib.register_opened(&second, &title(&second), NonZeroUsize::new(24));

    assert!(
        lib.clear(),
        "clearing a populated library changes persisted state"
    );
    assert!(lib.books().is_empty(), "all books are removed");
    assert_eq!(lib.last_opened(), None, "last_opened is cleared");
}

#[test]
fn add_orders_books_by_natural_title() {
    // Non-existent paths are already their caller-supplied identities.
    let mut lib = Library::new();
    add(&mut lib, "/manga/vol 10.cbz");
    add(&mut lib, "/manga/vol 1.cbz");
    add(&mut lib, "/manga/vol 2.cbz");
    let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
    assert_eq!(
        titles,
        vec!["vol 1", "vol 2", "vol 10"],
        "library order must use natural title sorting"
    );
}

#[test]
fn add_tie_breaks_same_title_by_canonical_path() {
    let dir = tempfile::tempdir().unwrap();
    let a_dir = dir.path().join("a");
    let b_dir = dir.path().join("b");
    std::fs::create_dir(&a_dir).unwrap();
    std::fs::create_dir(&b_dir).unwrap();
    let a = a_dir.join("Same.cbz");
    let b = b_dir.join("Same.cbz");
    std::fs::write(&a, []).unwrap();
    std::fs::write(&b, []).unwrap();
    let a = BookId::from_canonical_path(&a.canonicalize().unwrap()).unwrap();
    let b = BookId::from_canonical_path(&b.canonicalize().unwrap()).unwrap();

    let mut lib = Library::new();
    assert!(lib.add(b.clone(), title(&b)).is_some());
    assert!(lib.add(a.clone(), title(&a)).is_some());

    let paths: Vec<&BookId> = lib.books().iter().map(|book| book.id()).collect();
    assert_eq!(paths, vec![&a, &b]);
}

#[test]
fn add_dedups_by_path_and_returns_false() {
    let mut lib = Library::new();
    add(&mut lib, "/manga/a.cbz");
    let id = BookId::new("/manga/a.cbz");
    assert!(
        lib.add(id.clone(), title(&id)).is_none(),
        "adding an existing path must be a no-op returning None"
    );
    assert_eq!(lib.books().len(), 1);
}

#[test]
fn add_returns_stored_canonical_path_then_none_on_duplicate() {
    // A real temp file whose identity is produced caller-side; the first add
    // returns that canonical path and the second (duplicate) is None.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Book.cbz");
    std::fs::write(&file, []).unwrap();
    let canonical = BookId::from_canonical_path(&file.canonicalize().unwrap()).unwrap();

    let mut lib = Library::new();
    assert_eq!(
        lib.add(
            BookId::from_canonical_path(&canonical_identity(&file)).unwrap(),
            display_title(&file, false)
        ),
        Some(&canonical),
        "first add must return the stored canonical path"
    );
    assert!(
        lib.add(
            BookId::from_canonical_path(&canonical_identity(&file)).unwrap(),
            display_title(&file, false)
        )
        .is_none(),
        "adding the same path again must return None (duplicate)"
    );
    assert_eq!(lib.books().len(), 1);
}

#[test]
fn add_many_matches_repeated_add() {
    let already_present = BookId::new("/manga/already.cbz");
    let vol10 = BookId::new("/manga/vol 10.cbz");
    let vol2 = BookId::new("/manga/vol 2.cbz");
    let vol1 = BookId::new("/manga/vol 1.cbz");
    let inputs = vec![
        vol10.clone(),
        already_present.clone(),
        vol2.clone(),
        vol2.clone(),
        vol1.clone(),
    ];

    let mut batched = Library::new();
    assert!(batched
        .add(already_present.clone(), title(&already_present))
        .is_some());
    let added = batched.add_many(inputs.iter().map(|id| (id.clone(), title(id))).collect());

    let mut repeated = Library::new();
    assert!(repeated
        .add(already_present.clone(), title(&already_present))
        .is_some());
    let repeated_added: Vec<BookId> = inputs
        .into_iter()
        .filter_map(|path| repeated.add(path.clone(), title(&path)).cloned())
        .collect();

    let batched_paths: Vec<&BookId> = batched.books().iter().map(Book::id).collect();
    let repeated_paths: Vec<&BookId> = repeated.books().iter().map(Book::id).collect();
    assert_eq!(batched_paths, repeated_paths);
    assert_eq!(added, repeated_added);
    assert_eq!(added, vec![vol10, vol2, vol1]);
}

#[test]
fn add_many_sorts_once_into_natural_order() {
    let vol10 = BookId::new("/manga/vol 10.cbz");
    let vol2 = BookId::new("/manga/vol 2.cbz");
    let vol1 = BookId::new("/manga/vol 1.cbz");
    let mut lib = Library::new();

    assert_eq!(
        lib.add_many(
            [&vol10, &vol2, &vol1]
                .map(|id| (id.clone(), title(id)))
                .to_vec()
        ),
        vec![vol10.clone(), vol2.clone(), vol1.clone()]
    );

    let paths: Vec<&BookId> = lib.books().iter().map(Book::id).collect();
    assert_eq!(paths, vec![&vol1, &vol2, &vol10]);
}

#[test]
fn add_many_empty_input_is_a_no_op() {
    let existing = BookId::new("/manga/existing.cbz");
    let mut lib = Library::new();
    assert!(lib.add(existing.clone(), title(&existing)).is_some());
    let before = lib.to_json().unwrap();

    assert!(lib.add_many(Vec::new()).is_empty());

    assert_eq!(lib.to_json().unwrap(), before);
    assert_eq!(lib.books()[0].id(), &existing);
}

#[test]
fn add_stores_caller_supplied_canonical_identity() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Series.1997");
    std::fs::create_dir(&folder).unwrap();
    let canonical = BookId::from_canonical_path(&canonical_identity(&folder.join("."))).unwrap();

    let mut lib = Library::new();
    assert!(lib
        .add(canonical.clone(), display_title(&folder, true))
        .is_some());
    assert_eq!(lib.books().len(), 1);
    assert_eq!(lib.books()[0].id(), &canonical);
    assert_eq!(lib.books()[0].title(), "Series.1997");
}

#[test]
fn add_stores_the_given_identity_verbatim() {
    // The aggregate no longer canonicalises: whatever identity the caller
    // supplies is what gets stored, even when the filesystem could resolve it
    // to something shorter.
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Series.1997");
    std::fs::create_dir(&folder).unwrap();
    // Existing, resolvable, but deliberately NOT canonical.
    let non_canonical = dir
        .path()
        .join("..")
        .join(dir.path().file_name().expect("tempdir has a leaf name"));
    let non_canonical = non_canonical.join("Series.1997");
    assert_ne!(
        non_canonical,
        non_canonical.canonicalize().unwrap(),
        "the fixture must be genuinely non-canonical"
    );

    let mut lib = Library::new();
    assert!(lib
        .add(
            BookId::from_canonical_path(&non_canonical).unwrap(),
            display_title(&non_canonical, true)
        )
        .is_some());
    assert_eq!(lib.books().len(), 1);
    assert_eq!(
        lib.books()[0].id(),
        &BookId::from_canonical_path(&non_canonical).unwrap(),
        "add must store the caller-supplied identity verbatim"
    );
}

#[test]
fn remove_existing_returns_true_and_drops_book() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    assert!(lib.add(path.clone(), title(&path)).is_some());
    assert!(lib.remove(&path));
    assert!(lib.books().is_empty());
}

#[test]
fn remove_absent_returns_false() {
    let mut lib = Library::new();
    assert!(!lib.remove(&BookId::new("/manga/missing.cbz")));
}

#[test]
fn remove_many_removes_multiple_and_reports_them() {
    let mut lib = Library::new();
    for name in ["a.cbz", "b.cbz", "c.cbz"] {
        add(&mut lib, &format!("/manga/{name}"));
    }
    let report = lib.remove_many(&[BookId::new("/manga/a.cbz"), BookId::new("/manga/c.cbz")]);
    assert_eq!(
        report.removed,
        vec![BookId::new("/manga/a.cbz"), BookId::new("/manga/c.cbz")]
    );
    assert!(report.not_found.is_empty());
    let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
    assert_eq!(titles, vec!["b"], "only the unremoved book survives");
}

#[test]
fn remove_many_preserves_natural_order_of_survivors() {
    // Survivors must stay in natural title order after a bulk removal (the
    // retain keeps relative order, so no re-sort is needed).
    let mut lib = Library::new();
    for name in ["vol 1.cbz", "vol 2.cbz", "vol 10.cbz", "vol 11.cbz"] {
        add(&mut lib, &format!("/manga/{name}"));
    }
    // Remove the two middle volumes; vol 1 and vol 11 must remain in order.
    let report = lib.remove_many(&[
        BookId::new("/manga/vol 2.cbz"),
        BookId::new("/manga/vol 10.cbz"),
    ]);
    assert_eq!(report.removed.len(), 2);
    let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
    assert_eq!(
        titles,
        vec!["vol 1", "vol 11"],
        "survivors keep natural title order"
    );
}

#[test]
fn remove_many_splits_not_found() {
    let mut lib = Library::new();
    add(&mut lib, "/manga/a.cbz");
    let report = lib.remove_many(&[
        BookId::new("/manga/a.cbz"),
        BookId::new("/manga/missing.cbz"),
    ]);
    assert_eq!(report.removed, vec![BookId::new("/manga/a.cbz")]);
    assert_eq!(
        report.not_found,
        vec![BookId::new("/manga/missing.cbz")],
        "an input matching no book is reported, not removed"
    );
    assert!(lib.books().is_empty());
}

#[test]
fn remove_many_empty_input_yields_empty_report() {
    let mut lib = Library::new();
    let book = BookId::new("/manga/a.cbz");
    assert!(lib.add(book.clone(), title(&book)).is_some());
    // Register it as last-opened so we can verify it is preserved.
    lib.register_opened(&book, &title(&book), None);
    let report = lib.remove_many(&[]);
    assert_eq!(report, RemovalReport::default());
    assert!(report.removed.is_empty());
    assert!(report.not_found.is_empty());
    assert_eq!(lib.books().len(), 1, "no input removes nothing");
    assert_eq!(
        lib.last_opened(),
        Some(&book),
        "empty remove_many must not disturb last_opened"
    );
}

#[test]
fn remove_many_counts_duplicate_input_once() {
    let mut lib = Library::new();
    add(&mut lib, "/manga/a.cbz");
    // The same path passed twice must be removed once and reported once.
    let report = lib.remove_many(&[BookId::new("/manga/a.cbz"), BookId::new("/manga/a.cbz")]);
    assert_eq!(
        report.removed,
        vec![BookId::new("/manga/a.cbz")],
        "a duplicated input path is counted once"
    );
    assert!(report.not_found.is_empty());
    assert!(lib.books().is_empty());
}

#[test]
fn restore_reinserts_full_books_and_restores_natural_order() {
    // remove_many returns only paths; the rollback primitive must put back
    // WHOLE books (carrying resume_page/page_count/overrides) and re-sort.
    let mut lib = Library::new();
    for name in ["vol 1.cbz", "vol 2.cbz", "vol 10.cbz"] {
        add(&mut lib, &format!("/manga/{name}"));
    }
    let first_volume = BookId::new("/manga/vol 1.cbz");
    assert!(lib.set_resume_page(&first_volume, 7));
    assert!(lib.set_page_count(&first_volume, NonZeroUsize::new(20).unwrap()));

    // Clone the two outer volumes, remove them, then restore the clones.
    let removed: Vec<Book> = lib
        .books()
        .iter()
        .filter(|b| b.id() == &first_volume || b.id() == &BookId::new("/manga/vol 10.cbz"))
        .cloned()
        .collect();
    let report = lib.remove_many(&[first_volume.clone(), BookId::new("/manga/vol 10.cbz")]);
    assert_eq!(report.removed.len(), 2);
    let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
    assert_eq!(titles, vec!["vol 2"], "only the unremoved book survives");

    lib.restore(removed, None);
    let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
    assert_eq!(
        titles,
        vec!["vol 1", "vol 2", "vol 10"],
        "restore re-inserts and restores natural order"
    );
    let restored = lib.book(&first_volume).unwrap();
    assert_eq!(restored.resume_page(), 7);
    assert_eq!(restored.page_count_opt(), Some(20));
}

#[test]
fn restore_preserves_per_book_data_that_add_would_lose() {
    // The add()-trap: re-adding by path yields a FRESH book (resume_page 0, count None,
    // overrides all-None); restore must re-insert the WHOLE clone, keeping that data.
    let mut lib = Library::new();
    let p = BookId::new("/manga/a.cbz");
    assert!(lib.add(p.clone(), title(&p)).is_some());
    assert!(lib.set_resume_page(&p, 42));
    assert!(lib.set_page_count(&p, NonZeroUsize::new(100).unwrap()));
    let ov = crate::view_override::ViewOverride {
        reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
        ..crate::view_override::ViewOverride::none()
    };
    assert!(lib.set_overrides(&p, ov));

    let clone: Vec<Book> = lib.books().to_vec();
    assert_eq!(lib.remove_many(std::slice::from_ref(&p)).removed.len(), 1);
    assert!(lib.books().is_empty());

    lib.restore(clone, None);
    assert_eq!(lib.books().len(), 1);
    assert_eq!(
        lib.resume_page(&p),
        42,
        "restore keeps resume_page (add would reset to 0)"
    );
    assert_eq!(lib.books()[0].page_count_opt(), Some(100));
    assert_eq!(
        lib.overrides_for(&p),
        ov,
        "restore keeps overrides (add would reset to all-None)"
    );
}

#[test]
fn restore_is_duplicate_safe() {
    let mut lib = Library::new();
    let p = BookId::new("/manga/a.cbz");
    assert!(lib.add(p.clone(), title(&p)).is_some());
    assert!(lib.set_resume_page(&p, 2));
    let stale_books = lib.books().to_vec();
    assert!(lib.set_resume_page(&p, 9));

    lib.restore(stale_books, None);
    assert_eq!(
        lib.books().len(),
        1,
        "restore must not duplicate an existing path"
    );
    assert_eq!(lib.book(&p).unwrap().resume_page(), 9);
}

#[test]
fn restore_empty_input_is_noop() {
    let mut lib = Library::new();
    add(&mut lib, "/manga/a.cbz");
    lib.restore(Vec::new(), None);
    assert_eq!(lib.books().len(), 1, "empty restore changes nothing");
}

#[test]
fn restore_round_trips_to_byte_identical_json() {
    // The rollback contract: remove + restore must reproduce the EXACT pre-removal
    // JSON. A non-default override keeps the `overrides` comparison non-vacuous.
    let mut lib = Library::new();
    for name in ["a.cbz", "b.cbz", "c.cbz"] {
        add(&mut lib, &format!("/manga/{name}"));
    }
    assert!(lib.set_resume_page(&BookId::new("/manga/b.cbz"), 9));
    assert!(lib.set_page_count(&BookId::new("/manga/b.cbz"), NonZeroUsize::new(50).unwrap()));
    let ov = crate::view_override::ViewOverride {
        reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
        ..crate::view_override::ViewOverride::none()
    };
    assert!(lib.set_overrides(&BookId::new("/manga/b.cbz"), ov));
    let before = lib.to_json().unwrap();

    let removed: Vec<Book> = lib
        .books()
        .iter()
        .filter(|b| b.id() != &BookId::new("/manga/a.cbz"))
        .cloned()
        .collect();
    lib.remove_many(&[BookId::new("/manga/b.cbz"), BookId::new("/manga/c.cbz")]);
    lib.restore(removed, None);
    assert_eq!(
        lib.to_json().unwrap(),
        before,
        "remove + restore must be byte-identical"
    );
}

#[test]
fn restore_recovers_last_opened_bookmark() {
    // The Alloy counterexample: read B (last_opened = B), remove B (remove_many
    // clears the bookmark), then a failed save rolls back via restore — which must
    // hand the bookmark back so the "continue reading" target is not silently lost.
    let mut lib = Library::new();
    for name in ["a.cbz", "b.cbz"] {
        add(&mut lib, &format!("/manga/{name}"));
    }
    let b = BookId::new("/manga/b.cbz");
    lib.register_opened(&b, &title(&b), None);
    assert_eq!(lib.last_opened(), Some(&b), "B is the bookmark");

    let removed: Vec<Book> = lib
        .books()
        .iter()
        .filter(|book| book.id() == &b)
        .cloned()
        .collect();
    assert_eq!(lib.remove_many(std::slice::from_ref(&b)).removed.len(), 1);
    assert_eq!(lib.last_opened(), None, "remove_many clears the bookmark");

    lib.restore(removed, Some(b.clone()));
    assert_eq!(lib.books().len(), 2, "both books restored");
    assert_eq!(
        lib.last_opened(),
        Some(&b),
        "restore recovers the last_opened bookmark"
    );
}

#[test]
fn restore_drops_a_stale_bookmark_not_on_the_shelf() {
    // The membership guard (invariant S69): a last_opened that points at no
    // shelved book must be dropped, never re-installed as a dangling bookmark.
    let mut lib = Library::new();
    let a = BookId::new("/manga/a.cbz");
    assert!(lib.add(a.clone(), title(&a)).is_some());
    let removed: Vec<Book> = lib.books().to_vec();
    assert_eq!(lib.remove_many(std::slice::from_ref(&a)).removed.len(), 1);

    // Restore the book but point the bookmark at a book that was never re-inserted.
    lib.restore(removed, Some(BookId::new("/manga/ghost.cbz")));
    assert_eq!(lib.books().len(), 1, "the removed book is back");
    assert_eq!(
        lib.last_opened(),
        None,
        "a bookmark absent from the shelf must not be installed"
    );
}

#[test]
fn last_page_is_zero_for_unknown_path() {
    let lib = Library::new();
    assert_eq!(lib.resume_page(&BookId::new("/manga/missing.cbz")), 0);
}

#[test]
fn set_resume_page_updates_and_round_trips() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    assert!(lib.add(path.clone(), title(&path)).is_some());
    assert!(lib.set_resume_page(&path, 42));
    assert_eq!(lib.resume_page(&path), 42);
}

#[test]
fn set_resume_page_false_when_absent() {
    let mut lib = Library::new();
    assert!(!lib.set_resume_page(&BookId::new("/manga/missing.cbz"), 12));
}

#[test]
fn set_resume_page_false_when_unchanged() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    assert!(lib.add(path.clone(), title(&path)).is_some());
    assert!(lib.set_resume_page(&path, 3));
    assert!(!lib.set_resume_page(&path, 3));
}

#[test]
fn page_count_opt_is_none_for_fresh_book() {
    let book = book_from_path(Path::new("/manga/a.cbz"), false);
    assert_eq!(book.page_count_opt(), None);
}

#[test]
fn set_page_count_updates_and_returns_true() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    assert!(lib.add(path.clone(), title(&path)).is_some());
    assert!(lib.set_page_count(&path, NonZeroUsize::new(42).unwrap()));
    assert_eq!(lib.books()[0].page_count_opt(), Some(42));
}

#[test]
fn set_page_count_false_when_absent() {
    let mut lib = Library::new();
    assert!(!lib.set_page_count(
        &BookId::new("/manga/missing.cbz"),
        NonZeroUsize::new(12).unwrap()
    ));
}

#[test]
fn set_page_count_false_when_unchanged() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    assert!(lib.add(path.clone(), title(&path)).is_some());
    assert!(lib.set_page_count(&path, NonZeroUsize::new(3).unwrap()));
    assert!(!lib.set_page_count(&path, NonZeroUsize::new(3).unwrap()));
}

#[test]
fn set_page_count_repairs_a_stale_resume_page() {
    // A book read to page 41 whose archive later shrank to 5 pages: writing the
    // new count must also pull the stored resume back into the book.
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    assert!(lib.add(path.clone(), title(&path)).is_some());
    assert!(lib.set_resume_page(&path, 40));
    assert!(
        lib.set_page_count(&path, NonZeroUsize::new(5).unwrap()),
        "the count write changed something, so the caller must save"
    );
    assert_eq!(
        lib.resume_page(&path),
        4,
        "resume clamped to the last index"
    );
    assert_eq!(lib.books()[0].progress().current(), 5);
}

#[test]
fn set_page_count_reports_a_resume_only_repair() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    let count = NonZeroUsize::new(5).unwrap();
    assert!(lib.add(path.clone(), title(&path)).is_some());
    assert!(lib.set_resume_page(&path, 9));
    assert!(lib.set_page_count(&path, count));
    assert_eq!(lib.resume_page(&path), 4);
    assert!(!lib.set_page_count(&path, count));
}

#[test]
fn set_resume_page_clamps_into_a_known_total() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    assert!(lib.add(path.clone(), title(&path)).is_some());
    assert!(lib.set_page_count(&path, NonZeroUsize::new(5).unwrap()));
    assert!(lib.set_resume_page(&path, 9));
    assert_eq!(lib.resume_page(&path), 4);
    assert!(!lib.set_resume_page(&path, 4));
    assert!(!lib.set_resume_page(&path, 9));
}

#[test]
fn stale_pair_in_json_is_clamped_on_load() {
    let json = r#"{"path":"a.cbz","title":"a","last_page":40,"page_count":5}"#;
    let book: Book = serde_json::from_str(json).unwrap();
    assert_eq!(book.resume_page(), 4);
    assert_eq!(serde_json::to_value(book).unwrap()["last_page"], 4);
}

#[test]
fn book_json_shape_is_byte_identical() {
    let mut lib = Library::new();
    let path = BookId::new("a.cbz");
    assert!(lib.add(path.clone(), title(&path)).is_some());
    assert!(lib.set_resume_page(&path, 3));
    let unknown = r#"{
  "version": 1,
  "books": [
    {
      "path": "a.cbz",
      "title": "a",
      "last_page": 3,
      "page_count": 0
    }
  ]
}"#;
    assert_eq!(lib.to_json().unwrap(), unknown);
    assert!(lib.set_page_count(&path, NonZeroUsize::new(10).unwrap()));
    let known = unknown.replace(r#""page_count": 0"#, r#""page_count": 10"#);
    assert_eq!(lib.to_json().unwrap(), known);
}

#[test]
fn book_without_page_count_field_deserializes_to_unknown() {
    // Backward compat: an older `Book` JSON predating the `page_count` field must
    // still deserialize, the missing field defaulting to unknown (`None`).
    let value = serde_json::json!({
        "path": "/manga/a.cbz",
        "title": "a",
        "last_page": 5,
    });
    let book: Book = serde_json::from_value(value).unwrap();
    assert_eq!(book.id(), &BookId::new("/manga/a.cbz"));
    assert_eq!(book.resume_page(), 5);
    assert_eq!(book.page_count_opt(), None);
}

#[test]
fn progress_is_at_start_for_freshly_added_book() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    assert!(lib.add(path.clone(), title(&path)).is_some());
    let p = lib.books()[0].progress();
    assert!(p.is_at_start(), "freshly added book must be at the start");
    assert_eq!(p.last_viewed(), 0);
    assert_eq!(p.total(), None);
}

#[test]
fn progress_reflects_last_page_and_page_count() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    assert!(lib.add(path.clone(), title(&path)).is_some());
    assert!(lib.set_resume_page(&path, 3));
    assert!(lib.set_page_count(&path, NonZeroUsize::new(10).unwrap()));
    let p = lib.books()[0].progress();
    assert_eq!(p.last_viewed(), 3);
    assert_eq!(p.total(), Some(10));
    assert_eq!(p.current(), 4);
    let expected: f32 = 3.0 / 9.0;
    assert!(
        (p.fraction() - expected).abs() < 1e-6,
        "fraction should be 3/9, got {}",
        p.fraction()
    );
}

#[test]
fn register_opened_adds_fresh_book_and_back_fills_count() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    let reg = lib.register_opened(&path, &title(&path), NonZeroUsize::new(10));
    assert_eq!(lib.books().len(), 1, "fresh open must add the book");
    assert!(reg.count_changed, "0 -> 10 must report a count change");
    assert!(reg.resume.is_at_start(), "fresh book resumes at the start");
    assert_eq!(reg.resume.last_viewed(), 0);
    assert_eq!(reg.resume.total(), Some(10));
}

#[test]
fn register_opened_is_idempotent_and_reports_no_change_when_count_unchanged() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    lib.register_opened(&path, &title(&path), NonZeroUsize::new(10));
    let reg = lib.register_opened(&path, &title(&path), NonZeroUsize::new(10));
    assert_eq!(lib.books().len(), 1, "re-opening must not duplicate");
    assert!(
        !reg.count_changed,
        "an unchanged count must report no change"
    );
}

#[test]
fn register_opened_skips_back_fill_when_count_unknown() {
    // `None` means the total is unknown: the `is_some_and` guard skips set_page_count,
    // so the stored count stays unknown and surfaces as `total() == None`.
    let mut lib = Library::new();
    let path = BookId::new("/manga/b.cbz");
    let reg = lib.register_opened(&path, &title(&path), None);
    assert_eq!(lib.books().len(), 1, "the book is still added");
    assert!(
        !reg.count_changed,
        "an unknown count must not change the count"
    );
    assert_eq!(reg.resume.total(), None, "count stays unknown");
}

#[test]
fn register_opened_resumes_at_recorded_last_page() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    assert!(lib.add(path.clone(), title(&path)).is_some());
    assert!(lib.set_resume_page(&path, 5));
    let reg = lib.register_opened(&path, &title(&path), NonZeroUsize::new(10));
    assert_eq!(
        reg.resume.last_viewed(),
        5,
        "resume reflects the recorded page"
    );
    assert_eq!(reg.resume.current(), 6, "current is last_viewed + 1");
}

#[test]
fn register_opened_twice_preserves_recorded_resume_page() {
    // Re-opening a book must NOT reset its resume position: the idempotent
    // add inside register_opened keeps the existing entry (and its resume_page).
    // The resume is deliberately IN RANGE for the 10-page count — an out-of-range
    // one is now repaired by `set_page_count`, which would test the clamp rather
    // than the preservation this case is about.
    let mut lib = Library::new();
    let p = BookId::new("/manga/a.cbz");
    lib.register_opened(&p, &title(&p), NonZeroUsize::new(10));
    assert!(lib.set_resume_page(&p, 4));
    let reg = lib.register_opened(&p, &title(&p), NonZeroUsize::new(10));
    assert_eq!(lib.books().len(), 1, "re-open must not duplicate the book");
    assert_eq!(
        reg.resume.last_viewed(),
        4,
        "re-open must preserve resume_page"
    );
    assert!(!reg.count_changed, "same count on re-open is no change");
}

#[test]
fn register_opened_reports_count_change_on_new_nonzero_count() {
    // A legitimately changed (non-zero -> different non-zero) page count is
    // reported as count_changed, flowing set_page_count's "did it move" branch.
    let mut lib = Library::new();
    let p = BookId::new("/manga/a.cbz");
    assert!(
        lib.register_opened(&p, &title(&p), NonZeroUsize::new(10))
            .count_changed
    );
    let reg = lib.register_opened(&p, &title(&p), NonZeroUsize::new(12));
    assert!(reg.count_changed, "10 -> 12 is a real change");
    assert_eq!(reg.resume.total(), Some(12));
}

#[test]
fn register_opened_with_caller_supplied_identity_keeps_invariant() {
    // Invariant: register_opened always sets last_opened to the stored book when the
    // caller supplies the identity for a path containing a `..` detour.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Book.cbz");
    std::fs::write(&file, []).unwrap();

    // Build a non-canonical path: enter a subdirectory and navigate back up.
    // If the tempdir has no subdirectory yet, create one purely for the detour.
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let non_canonical = sub.join("..").join("Book.cbz");
    let canonical = BookId::from_canonical_path(&canonical_identity(&non_canonical)).unwrap();

    let mut lib = Library::new();
    lib.register_opened(&canonical, &title(&canonical), None);

    // last_opened is set, and its path is a book actually in the shelf.
    let lo = lib
        .last_opened()
        .expect("register_opened must always set last_opened")
        .clone();
    assert!(
        lib.books().iter().any(|b| b.id() == &lo),
        "last_opened must point at a book actually in the shelf"
    );
    assert_eq!(
        lo, canonical,
        "last_opened must equal the caller-supplied stored identity"
    );
}

#[test]
fn new_book_has_empty_overrides() {
    let book = book_from_path(Path::new("/manga/a.cbz"), false);
    assert!(book.overrides().is_empty());
}

#[test]
fn set_overrides_updates_and_reports_change() {
    let mut lib = Library::new();
    let p = BookId::new("/manga/a.cbz");
    assert!(lib.add(p.clone(), title(&p)).is_some());
    let ov = crate::view_override::ViewOverride {
        reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
        ..crate::view_override::ViewOverride::none()
    };
    // First set changes the value -> true.
    assert!(lib.set_overrides(&p, ov));
    assert_eq!(lib.overrides_for(&p), ov);
    // Setting the same value again is a no-op -> false (mirrors set_resume_page).
    assert!(!lib.set_overrides(&p, ov));
}

#[test]
fn set_overrides_clear_to_none_reports_change_and_empties() {
    let mut lib = Library::new();
    let p = BookId::new("/manga/a.cbz");
    assert!(lib.add(p.clone(), title(&p)).is_some());
    let ov = crate::view_override::ViewOverride {
        reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
        ..crate::view_override::ViewOverride::none()
    };
    assert!(lib.set_overrides(&p, ov));
    // Clearing back to none() is a real change -> true, and leaves it empty.
    assert!(lib.set_overrides(&p, crate::view_override::ViewOverride::none()));
    assert!(lib.overrides_for(&p).is_empty());
}

#[test]
fn set_overrides_false_when_path_absent() {
    let mut lib = Library::new();
    let ov = crate::view_override::ViewOverride::none();
    assert!(!lib.set_overrides(&BookId::new("/manga/missing.cbz"), ov));
}

#[test]
fn overrides_for_absent_path_is_empty() {
    let lib = Library::new();
    assert!(lib
        .overrides_for(&BookId::new("/manga/missing.cbz"))
        .is_empty());
}

// --- last_opened tests ---

#[test]
fn last_opened_is_none_for_fresh_library() {
    let lib = Library::new();
    assert_eq!(lib.last_opened(), None);
}

#[test]
fn register_opened_sets_last_opened_on_fresh_book() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    lib.register_opened(&path, &title(&path), None);
    assert_eq!(lib.last_opened(), Some(&BookId::new("/manga/a.cbz")));
}

#[test]
fn register_opened_updates_last_opened_on_reopen() {
    // Opening a different book after the first one replaces last_opened.
    let mut lib = Library::new();
    let a = BookId::new("/manga/a.cbz");
    let b = BookId::new("/manga/b.cbz");
    lib.register_opened(&a, &title(&a), None);
    lib.register_opened(&b, &title(&b), None);
    assert_eq!(lib.last_opened(), Some(&BookId::new("/manga/b.cbz")));
}

#[test]
fn register_opened_same_book_twice_preserves_last_opened() {
    // Re-opening the same book keeps last_opened pointing at it.
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    lib.register_opened(&path, &title(&path), None);
    lib.register_opened(&path, &title(&path), None);
    assert_eq!(lib.last_opened(), Some(&BookId::new("/manga/a.cbz")));
}

#[test]
fn remove_last_opened_book_clears_last_opened() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    lib.register_opened(&path, &title(&path), None);
    assert!(lib.remove(&path));
    assert_eq!(lib.last_opened(), None);
}

#[test]
fn remove_different_book_preserves_last_opened() {
    let mut lib = Library::new();
    let a = BookId::new("/manga/a.cbz");
    let b = BookId::new("/manga/b.cbz");
    lib.register_opened(&a, &title(&a), None);
    lib.add(b.clone(), title(&b));
    assert!(lib.remove(&b));
    assert_eq!(
        lib.last_opened(),
        Some(&BookId::new("/manga/a.cbz")),
        "removing a different book must not clear last_opened"
    );
}

#[test]
fn remove_many_containing_last_opened_clears_it() {
    let mut lib = Library::new();
    let a = BookId::new("/manga/a.cbz");
    let b = BookId::new("/manga/b.cbz");
    lib.register_opened(&a, &title(&a), None);
    lib.add(b.clone(), title(&b));
    lib.remove_many(&[a.clone(), b.clone()]);
    assert_eq!(lib.last_opened(), None);
}

#[test]
fn remove_many_not_containing_last_opened_preserves_it() {
    let mut lib = Library::new();
    let a = BookId::new("/manga/a.cbz");
    let b = BookId::new("/manga/b.cbz");
    lib.register_opened(&a, &title(&a), None);
    lib.add(b.clone(), title(&b));
    lib.remove_many(std::slice::from_ref(&b));
    assert_eq!(
        lib.last_opened(),
        Some(&BookId::new("/manga/a.cbz")),
        "removing an unrelated book must not clear last_opened"
    );
}

// --- bookmark tests (resume target = last_opened only when shelved) ---

#[test]
fn bookmark_is_some_when_last_opened_is_shelved() {
    let mut lib = Library::new();
    let path = BookId::new("/manga/a.cbz");
    lib.register_opened(&path, &title(&path), None);
    assert_eq!(
        lib.bookmark(),
        Some(&BookId::new("/manga/a.cbz")),
        "bookmark resolves to a shelved last_opened"
    );
}

#[test]
fn bookmark_is_none_when_last_opened_is_none() {
    let mut lib = Library::new();
    // A shelved book but nothing opened yet: no resume target.
    add(&mut lib, "/manga/a.cbz");
    assert_eq!(lib.last_opened(), None);
    assert_eq!(lib.bookmark(), None, "no last_opened means no bookmark");
}

#[test]
fn bookmark_is_none_for_orphan_last_opened() {
    // last_opened points at a path NOT in books, built via RAW serde deserialization
    // (not from_json, which normalize() would clear), so bookmark() guards membership.
    let json = serde_json::json!({
        "books": [{"path": "/manga/a.cbz", "title": "a", "last_page": 0, "page_count": 0}],
        "last_opened": "/manga/gone.cbz"
    })
    .to_string();
    let lib: Library = serde_json::from_str(&json).unwrap();
    assert_eq!(
        lib.last_opened(),
        Some(&BookId::new("/manga/gone.cbz")),
        "raw deserialization preserves the orphan (no normalize)"
    );
    assert_eq!(
        lib.bookmark(),
        None,
        "bookmark must reject a last_opened that is not a shelved book"
    );
}

fn library_with_exact_duplicate_ids() -> Library {
    serde_json::from_value(serde_json::json!({
        "books": [
            {"path": "books/u1/a.cbz", "title": "Original", "last_page": 3},
            {"path": "books/u1/a.cbz", "title": "Duplicate", "last_page": 7,
             "page_count": 20, "overrides": {"reading_direction": "rtl"}}
        ]
    }))
    .unwrap()
}

#[test]
fn normalize_merges_exact_duplicate_ids() {
    let mut lib = library_with_exact_duplicate_ids();

    lib.normalize();

    assert_eq!(lib.books().len(), 1);
    let book = lib.book(&BookId::new("books/u1/a.cbz")).unwrap();
    assert_eq!(book.title(), "Original");
    assert_eq!(book.resume_page(), 7);
    assert_eq!(book.page_count_opt(), Some(20));
    assert_eq!(
        book.overrides(),
        ViewOverride {
            reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
            ..ViewOverride::none()
        }
    );
}

#[test]
fn normalize_dedup_keeps_set_resume_page_on_the_only_copy() {
    let mut lib = library_with_exact_duplicate_ids();
    lib.normalize();
    let id = BookId::new("books/u1/a.cbz");

    assert!(lib.set_resume_page(&id, 5));
    assert_eq!(lib.book(&id).unwrap().resume_page(), 5);
    assert_eq!(lib.books().len(), 1);
}

#[test]
fn normalize_clears_orphan_last_opened() {
    // Build a library via serde with a last_opened that points at a path
    // NOT present in books, then confirm normalize() resets it to None.
    let json = serde_json::json!({
        "version": 1,
        "books": [{"path": "/manga/a.cbz", "title": "a", "last_page": 0, "page_count": 0}],
        "last_opened": "/manga/gone.cbz"
    })
    .to_string();
    let lib = Library::from_json(&json).unwrap();
    assert_eq!(
        lib.last_opened(),
        None,
        "normalize must clear an orphan last_opened"
    );
}

#[test]
fn normalize_keeps_valid_last_opened() {
    // A valid last_opened (path is in books) must survive normalize().
    let json = serde_json::json!({
        "version": 1,
        "books": [{"path": "/manga/a.cbz", "title": "a", "last_page": 0, "page_count": 0}],
        "last_opened": "/manga/a.cbz"
    })
    .to_string();
    let lib = Library::from_json(&json).unwrap();
    assert_eq!(
        lib.last_opened(),
        Some(&BookId::new("/manga/a.cbz")),
        "normalize must keep a valid last_opened"
    );
}

// recanonicalize_and_merge tests: each builds a `library.json` with path spellings
// crafted directly (bypassing add-time canonicalization) so two entries name one file.

/// Build a non-canonical spelling of `canonical` by routing through a `..`
/// detour (`<dir>/sub/../<name>`), creating the `sub` directory so the path
/// resolves. A `..` component is NOT folded away by `Path` comparison (unlike
/// `.`), so the result is genuinely distinct from `canonical` AND canonicalizes
/// back to it — faithfully reproducing the divergent-spelling bug (a plain `.`
/// segment would compare equal and the add-time dedup would already catch it).
fn detour_spelling(canonical: &Path) -> PathBuf {
    let parent = canonical.parent().unwrap();
    let sub = parent.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    sub.join("..").join(canonical.file_name().unwrap())
}

#[test]
fn recanonicalize_merges_two_spellings_of_same_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Book.cbz");
    std::fs::write(&file, []).unwrap();
    let canonical = file.canonicalize().unwrap();
    let non_canonical = detour_spelling(&canonical);
    assert_ne!(
        canonical, non_canonical,
        "the two spellings must differ as strings"
    );

    // Two entries naming the SAME file, fields set so the merge policy is non-vacuous:
    // max resume_page + first Some page_count from member[1]; override + title from member[0].
    let json = serde_json::json!({
        "version": 1,
        "books": [
            {"path": canonical.to_str().unwrap(), "title": "Book", "last_page": 3,
             "page_count": 0, "overrides": {"reading_direction": "rtl"}},
            {"path": non_canonical.to_str().unwrap(), "title": "Other", "last_page": 7,
             "page_count": 20},
        ]
    })
    .to_string();

    let lib = Library::from_json(&json).unwrap();

    assert_eq!(
        lib.books().len(),
        1,
        "two spellings of one file merge into a single book"
    );
    let book = &lib.books()[0];
    assert_eq!(
        book.id(),
        &BookId::from_canonical_path(&canonical).unwrap(),
        "survivor path is the canonical key"
    );
    assert_eq!(
        book.resume_page(),
        7,
        "resume_page is the max across members"
    );
    assert_eq!(
        book.page_count_opt(),
        Some(20),
        "page_count is the first Some in vec order"
    );
    assert_eq!(
        book.title(),
        "Book",
        "title is the first member's title (earliest-added identity)"
    );
    assert_eq!(
        book.overrides().reading_direction,
        Some(crate::view_modes::ReadingDirection::Rtl),
        "overrides come from the first non-empty member"
    );
}

#[test]
fn merge_group_clamps_the_survivor_position() {
    // The merge takes `resume_page` as the MAX and `page_count` as the first
    // `Some`, so the two fields can come from DIFFERENT members and pair a large
    // position with a small total. The survivor must be repaired.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Book.cbz");
    std::fs::write(&file, []).unwrap();
    let canonical = file.canonicalize().unwrap();
    let non_canonical = detour_spelling(&canonical);

    let json = serde_json::json!({
        "version": 1,
        "books": [
            {"path": canonical.to_str().unwrap(), "title": "Book", "last_page": 0,
             "page_count": 5},
            {"path": non_canonical.to_str().unwrap(), "title": "Other", "last_page": 40,
             "page_count": 0},
        ]
    })
    .to_string();

    let lib = Library::from_json(&json).unwrap();

    assert_eq!(lib.books().len(), 1, "the two spellings merge");
    let book = &lib.books()[0];
    assert_eq!(book.page_count_opt(), Some(5), "count is the first Some");
    assert_eq!(
        book.resume_page(),
        4,
        "the max resume is clamped into the surviving page count"
    );
}

#[test]
fn recanonicalize_upgrades_single_noncanonical_path() {
    // A lone entry whose stored path is non-canonical but now resolves must
    // have its path upgraded to canonical on load (group size 1, no merge).
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Book.cbz");
    std::fs::write(&file, []).unwrap();
    let canonical = file.canonicalize().unwrap();
    let non_canonical = detour_spelling(&canonical);

    let json = serde_json::json!({
        "version": 1,
        "books": [
            {"path": non_canonical.to_str().unwrap(), "title": "Book", "last_page": 5,
             "page_count": 0},
        ]
    })
    .to_string();

    let lib = Library::from_json(&json).unwrap();

    assert_eq!(lib.books().len(), 1);
    assert_eq!(
        lib.books()[0].id(),
        &BookId::from_canonical_path(&canonical).unwrap(),
        "a resolvable non-canonical path is upgraded to canonical on load"
    );
    assert_eq!(
        lib.books()[0].resume_page(),
        5,
        "the book's data survives the upgrade"
    );
}

#[test]
fn recanonicalize_preserves_missing_path_unchanged() {
    // A path that does not resolve (canonicalize fails) must be kept verbatim —
    // neither dropped nor merged. Regression guard for the missing-file case.
    let raw = "/manga/definitely/missing/Book.cbz";
    let json = serde_json::json!({
        "version": 1,
        "books": [
            {"path": raw, "title": "Book", "last_page": 9, "page_count": 0},
        ]
    })
    .to_string();

    let lib = Library::from_json(&json).unwrap();

    assert_eq!(lib.books().len(), 1, "a missing-path book is kept");
    assert_eq!(
        lib.books()[0].id(),
        &BookId::new(raw),
        "the raw path is preserved unchanged"
    );
    assert_eq!(lib.books()[0].resume_page(), 9);
}

#[test]
fn recanonicalize_repoints_last_opened_to_survivor() {
    // last_opened spelled as the merged-away path must be repointed to the surviving
    // canonical path; without the repoint, normalize's orphan-clear would reset it to None.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Book.cbz");
    std::fs::write(&file, []).unwrap();
    let canonical = file.canonicalize().unwrap();
    let non_canonical = detour_spelling(&canonical);

    let json = serde_json::json!({
        "version": 1,
        "books": [
            {"path": canonical.to_str().unwrap(), "title": "Book", "last_page": 0,
             "page_count": 0},
            {"path": non_canonical.to_str().unwrap(), "title": "Book", "last_page": 0,
             "page_count": 0},
        ],
        "last_opened": non_canonical.to_str().unwrap(),
    })
    .to_string();

    let lib = Library::from_json(&json).unwrap();

    assert_eq!(
        lib.books().len(),
        1,
        "the duplicate spelling is merged away"
    );
    assert_eq!(
        lib.last_opened(),
        Some(&BookId::from_canonical_path(&canonical).unwrap()),
        "last_opened is repointed to the surviving canonical path"
    );
}

#[test]
fn recanonicalize_keeps_books_in_natural_order() {
    // After re-canonicalization/merge the shelf must still obey book_order. Missing
    // paths are kept verbatim, so this also confirms the sort runs over survivors.
    let json = serde_json::json!({
        "version": 1,
        "books": [
            {"path": "/manga/vol 10.cbz", "title": "vol 10", "last_page": 0, "page_count": 0},
            {"path": "/manga/vol 1.cbz", "title": "vol 1", "last_page": 0, "page_count": 0},
            {"path": "/manga/vol 2.cbz", "title": "vol 2", "last_page": 0, "page_count": 0},
        ]
    })
    .to_string();

    let lib = Library::from_json(&json).unwrap();

    let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
    assert_eq!(
        titles,
        vec!["vol 1", "vol 2", "vol 10"],
        "post-merge books obey natural title order"
    );
}
