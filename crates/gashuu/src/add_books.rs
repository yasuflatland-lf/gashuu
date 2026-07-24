//! Bulk-add domain half (issue 206 / 240): the headless, UI-free core of the
//! "add books" use case. Holds the pure value type ([`AddReport`]), the `!Send`
//! `Library` mutation that applies already-probed sources ([`apply_outcomes`]),
//! and the pure status-routing decision ([`AddNotice`] / [`select_add_notice`]).
//!
//! The UI-finalize half — applying + persisting through
//! [`apply_outcomes_and_save`], rebuilding the carousel, and surfacing the notice
//! on the status line — lives in `crate::carousel_refresh::apply_add_report`.
//! The off-UI-thread PROBE half lives in `crate::add_controller`.

use crate::add_controller;
use gashuu_core::{CoreError, Library};

/// Outcome of an add batch: the canonical paths actually inserted (new books
/// only, in INPUT order) and the count of paths REJECTED because they could not
/// be opened as a book — either a source with zero image pages (the empty-book
/// rule) or an unreadable / unsupported source. Duplicates are NOT counted in
/// `skipped`: a path already in the library (or repeated within the batch) is
/// neither added nor rejected, mirroring `Library::add`'s `None`.
pub(crate) struct AddReport {
    pub(crate) added: Vec<std::path::PathBuf>,
    pub(crate) skipped: usize,
}

/// Apply already-probed sources to `lib`, the UI-thread APPLY half of the bulk
/// add (issue 206). The probe half runs off the UI thread (`add_controller::probe_path`
/// on rayon workers) so opening each archive never freezes the event loop; this
/// half takes the resulting [`add_controller::ProbeOutcome`]s — which the controller
/// has already re-sorted to INPUT order — and mutates the `!Send` `Library` here:
///
/// - `ProbeKind::Empty` — opened but zero image pages: skip and count in
///   `skipped` (the empty-book rule).
/// - `ProbeKind::FormatDisabled` / `ProbeKind::Unreadable` — could not be opened
///   as a book: skip, count in `skipped`, and log (the same level + detail the
///   old synchronous `add_paths` logged; logging is deferred to here so the probe
///   half stays pure).
/// - `ProbeKind::Counted(count)` — add via `Library::add` (canonicalizes, dedups,
///   re-sorts). On a genuine insert (`Some(canonical)`) the page count is recorded
///   immediately so a freshly added book shows "1 / N" without waiting for its
///   first open; a duplicate (`None`) is silently dropped (neither added nor
///   skipped).
///
/// Behaviour is byte-identical to the pre-206 synchronous `add_paths`; only the
/// probe was moved off-thread.
pub(crate) fn apply_outcomes(
    lib: &mut Library,
    outcomes: Vec<add_controller::ProbeOutcome>,
) -> AddReport {
    use add_controller::ProbeKind;
    let mut added = Vec::new();
    let mut skipped = 0usize;
    for add_controller::ProbeOutcome { path, kind, .. } in outcomes {
        match kind {
            ProbeKind::Empty => {
                skipped += 1;
                tracing::debug!(path = %path.display(), "skipping empty source (no image pages)");
            }
            ProbeKind::FormatDisabled { format } => {
                skipped += 1;
                tracing::info!(
                    path = %path.display(),
                    %format,
                    "skipping source: format disabled in safer mode"
                );
            }
            ProbeKind::Unreadable { error } => {
                skipped += 1;
                tracing::warn!(%error, path = %path.display(), "skipping unreadable source");
            }
            ProbeKind::Counted(count) => {
                if let Some(canonical) = lib.add(path).map(std::path::Path::to_path_buf) {
                    // Record the probed count so a freshly inserted book shows "1 / N"
                    // before its first open (set_page_count re-finds it by canonical path).
                    lib.set_page_count(&canonical, count);
                    added.push(canonical);
                }
                // `None` here means a duplicate (within the batch or already
                // present): neither added nor skipped, as before.
            }
        }
    }
    AddReport { added, skipped }
}

/// Apply a probed batch and persist the resulting library mutation as one
/// UI-thread operation. The injected `save` keeps the mutation/save boundary
/// headless and testable. A batch that adds no books performs no save, preserving
/// the add tail's existing duplicate/rejection short-circuit.
pub(crate) fn apply_outcomes_and_save(
    lib: &mut Library,
    outcomes: Vec<add_controller::ProbeOutcome>,
    save: impl Fn(&Library) -> Result<(), CoreError>,
) -> (AddReport, Result<(), CoreError>) {
    let report = apply_outcomes(lib, outcomes);
    let save_result = if report.added.is_empty() {
        Ok(())
    } else {
        save(lib)
    };
    (report, save_result)
}

/// Which status notice to surface after `apply_outcomes` applies a probed batch.
///
/// The four arms cover the full 2×2 of (added==0 vs added>0) × (skipped==0 vs
/// skipped>0).  The save-failure arm is handled separately in
/// `apply_add_report` and is NOT part of this enum.
#[derive(Debug, PartialEq)]
pub(crate) enum AddNotice {
    /// All picked paths were already in the library (no new additions, no rejections).
    AlreadyInLibrary,
    /// Every path was rejected (no images or unreadable); nothing was added.
    NoneAddedAllSkipped { skipped: usize },
    /// Some books were added and some paths were rejected.
    AddedWithSkips { added: usize, skipped: usize },
    /// All picked paths were added successfully; none were rejected.
    Added { added: usize },
}

/// Pure decision function: maps the `(added, skipped)` counts produced by
/// [`apply_outcomes`] (carried in [`AddReport`]) to the appropriate
/// [`AddNotice`] variant.  No I/O, no side-effects.
pub(crate) fn select_add_notice(added: usize, skipped: usize) -> AddNotice {
    match (added, skipped) {
        (0, 0) => AddNotice::AlreadyInLibrary,
        (0, s) => AddNotice::NoneAddedAllSkipped { skipped: s },
        (n, 0) => AddNotice::Added { added: n },
        (n, s) => AddNotice::AddedWithSkips {
            added: n,
            skipped: s,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::ArchivePolicy;

    /// Test convenience around the split add: probe `paths` synchronously in
    /// input order, then apply the outcomes. This is the pre-206 `add_paths`
    /// behaviour, retained so the apply-half tests below exercise the real
    /// `apply_outcomes` mutation path through one call. Production no longer has a
    /// synchronous `add_paths` — the probe runs off the UI thread (`add_controller`)
    /// and the apply runs in the `add-finalize` handler — but the probe + apply
    /// halves are unchanged in behaviour, so testing them composed is faithful.
    fn add_paths(
        lib: &mut Library,
        paths: Vec<std::path::PathBuf>,
        policy: ArchivePolicy,
    ) -> AddReport {
        let outcomes = paths
            .into_iter()
            .enumerate()
            .map(|(index, path)| add_controller::probe_path(index, path, policy))
            .collect();
        apply_outcomes(lib, outcomes)
    }

    // add_paths PROBES each source before insert (empty-book rule: a source needs >=1 image
    // page). These helpers build real temp dirs so probing sees a genuine filesystem.

    /// Create a fresh temp directory under `parent/<name>` holding `pages`
    /// zero-byte `*.png` files (so it probes to a `pages`-page book). With
    /// `pages == 0` the directory is empty and probes to `EmptyBook`. Returns the
    /// directory path (its canonical form is what `Library::add` stores).
    fn make_book_dir(parent: &std::path::Path, name: &str, pages: usize) -> std::path::PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).expect("create book dir");
        for i in 0..pages {
            std::fs::write(dir.join(format!("page{i:03}.png")), []).expect("write page");
        }
        dir
    }

    /// Canonicalize a path the same way `Library::add` does, so test expectations
    /// match the stored/returned canonical paths.
    fn canon(path: &std::path::Path) -> std::path::PathBuf {
        path.canonicalize().expect("canonicalize existing path")
    }

    #[test]
    fn add_paths_empty_vec_returns_zero() {
        let mut lib = gashuu_core::Library::new();
        let report = add_paths(&mut lib, vec![], ArchivePolicy::default());
        assert!(report.added.is_empty());
        assert_eq!(report.skipped, 0);
        assert_eq!(lib.books().len(), 0);
    }

    #[test]
    fn add_paths_new_paths_counted() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        let vol2 = make_book_dir(root.path(), "vol2", 2);
        let report = add_paths(
            &mut lib,
            vec![vol1.clone(), vol2.clone()],
            ArchivePolicy::default(),
        );
        assert_eq!(report.added.len(), 2);
        assert_eq!(report.skipped, 0);
        assert_eq!(lib.books().len(), 2);
        // The returned vec holds the CANONICAL paths in INPUT order.
        assert_eq!(report.added, vec![canon(&vol1), canon(&vol2)]);
    }

    #[test]
    fn add_paths_dedup_within_batch() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        let report = add_paths(
            &mut lib,
            vec![vol1.clone(), vol1.clone()],
            ArchivePolicy::default(),
        );
        assert_eq!(
            report.added.len(),
            1,
            "duplicate within the batch must not be double-counted"
        );
        // A duplicate is neither added nor rejected, so it is NOT counted as skipped.
        assert_eq!(
            report.skipped, 0,
            "a duplicate is not an empty/unreadable skip"
        );
        assert_eq!(lib.books().len(), 1);
    }

    #[test]
    fn add_paths_dedup_against_existing() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        let vol2 = make_book_dir(root.path(), "vol2", 1);
        lib.add(vol1.clone());
        let report = add_paths(
            &mut lib,
            vec![vol1.clone(), vol2.clone()],
            ArchivePolicy::default(),
        );
        assert_eq!(
            report.added.len(),
            1,
            "a path already in the library must not be counted"
        );
        assert_eq!(
            report.skipped, 0,
            "an existing path is not an empty/unreadable skip"
        );
        assert_eq!(lib.books().len(), 2);
    }

    #[test]
    fn add_paths_returns_canonical_paths_and_skips_duplicates() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        // `vol1/.` and `vol1` canonicalize to the same path, so the second is a
        // duplicate and dropped.
        let with_dot = vol1.join(".");
        let expected = canon(&vol1);
        let report = add_paths(
            &mut lib,
            vec![with_dot.clone(), with_dot.clone()],
            ArchivePolicy::default(),
        );
        assert_eq!(report.added, vec![expected.clone()]);
        assert_eq!(report.skipped, 0);
        assert_eq!(lib.books().len(), 1);
        assert_eq!(lib.books()[0].path(), expected.as_path());
    }

    #[test]
    fn add_paths_all_existing_returns_zero() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        let vol2 = make_book_dir(root.path(), "vol2", 1);
        lib.add(vol1.clone());
        lib.add(vol2.clone());
        let before = lib.books().len();
        let report = add_paths(
            &mut lib,
            vec![vol1.clone(), vol2.clone()],
            ArchivePolicy::default(),
        );
        assert!(report.added.is_empty(), "all-duplicate batch must add 0");
        assert_eq!(report.skipped, 0, "duplicates are not skips");
        assert_eq!(lib.books().len(), before, "books count must not change");
    }

    #[test]
    fn add_paths_mixed_batch_counts_added_and_skipped() {
        // A valid book, an empty folder, and a duplicate of the valid book:
        // 1 added, 1 skipped (the empty), and the duplicate dropped silently.
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let valid = make_book_dir(root.path(), "valid", 1);
        let empty = make_book_dir(root.path(), "empty", 0);
        let report = add_paths(
            &mut lib,
            vec![valid.clone(), empty.clone(), valid.clone()],
            ArchivePolicy::default(),
        );
        assert_eq!(
            report.added,
            vec![canon(&valid)],
            "only the valid book is added"
        );
        assert_eq!(report.skipped, 1, "the empty folder is the one skip");
        assert_eq!(lib.books().len(), 1);
        assert_eq!(lib.books()[0].path(), canon(&valid).as_path());
    }

    #[test]
    fn add_paths_all_empty_batch_adds_zero_skips_all() {
        // Every picked source is empty: nothing added, all counted as skipped.
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let e1 = make_book_dir(root.path(), "e1", 0);
        let e2 = make_book_dir(root.path(), "e2", 0);
        let e3 = make_book_dir(root.path(), "e3", 0);
        let report = add_paths(&mut lib, vec![e1, e2, e3], ArchivePolicy::default());
        assert!(
            report.added.is_empty(),
            "no book added from an all-empty batch"
        );
        assert_eq!(report.skipped, 3, "all three empty sources are skipped");
        assert_eq!(lib.books().len(), 0);
    }

    #[test]
    fn add_paths_unreadable_path_is_skipped() {
        // A nonexistent path is an I/O error (unreadable, NOT empty), so it is
        // rejected as a skip and kept out of the library rather than added.
        let mut lib = gashuu_core::Library::new();
        let report = add_paths(
            &mut lib,
            vec![std::path::PathBuf::from(
                "/nonexistent_gashuu_add_paths_unreadable",
            )],
            ArchivePolicy::default(),
        );
        assert!(report.added.is_empty(), "an unreadable path is never added");
        assert_eq!(
            report.skipped, 1,
            "the unreadable path is counted as skipped"
        );
        assert_eq!(lib.books().len(), 0);
    }

    #[test]
    fn add_paths_sets_page_count_immediately() {
        // A freshly added book carries its probed page count so the carousel can
        // show "1 / N" before the book is ever opened.
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let three = make_book_dir(root.path(), "three", 3);
        let report = add_paths(&mut lib, vec![three.clone()], ArchivePolicy::default());
        assert_eq!(report.added.len(), 1);
        assert_eq!(report.skipped, 0);
        let book = lib
            .books()
            .iter()
            .find(|b| b.path() == canon(&three))
            .expect("added book present");
        assert_eq!(
            book.page_count_opt(),
            Some(3),
            "the probed page count is recorded on add"
        );
    }

    #[test]
    fn apply_outcomes_and_save_returns_report_when_save_fails() {
        let root = tempfile::tempdir().expect("tempdir");
        let book = make_book_dir(root.path(), "book", 2);
        let outcomes = vec![add_controller::probe_path(
            0,
            book.clone(),
            ArchivePolicy::default(),
        )];
        let mut lib = Library::new();

        let (report, save_result) = apply_outcomes_and_save(&mut lib, outcomes, |_| {
            Err(gashuu_core::CoreError::Io(std::io::Error::other(
                "injected save failure",
            )))
        });

        assert_eq!(report.added, vec![canon(&book)]);
        assert_eq!(report.skipped, 0);
        assert_eq!(lib.books().len(), 1, "the in-memory add is retained");
        let error = save_result.expect_err("injected save must fail");
        assert!(
            error.to_string().contains("injected save failure"),
            "the save error is returned alongside the report"
        );
    }

    #[test]
    fn add_paths_returns_input_order_while_books_are_natural_order() {
        // `add_paths` returns inserted paths in INPUT order (focus follows the first),
        // whereas `lib.books()` keeps NATURAL order — vol1/vol10 leaf names drive the sort.
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol10 = canon(&make_book_dir(root.path(), "vol10", 1));
        let vol1 = canon(&make_book_dir(root.path(), "vol1", 1));
        let report = add_paths(
            &mut lib,
            vec![vol10.clone(), vol1.clone()],
            ArchivePolicy::default(),
        );

        // Returned vec is in INPUT order (vol10 first, vol1 second).
        assert_eq!(report.added[0], vol10);
        assert_eq!(report.added[1], vol1);

        // The library itself is in NATURAL order (vol1 before vol10).
        let books: Vec<_> = lib
            .books()
            .iter()
            .map(|book| book.path().to_path_buf())
            .collect();
        assert_eq!(books, vec![vol1, vol10]);
    }

    // `build_carousel_model` is headless and unit-tested in `carousel::tests`; the
    // Library -> carousel row invariants live in `library_model::tests` (carousel_data).

    // ---- select_add_notice (reject-empty-books status routing) --------------

    #[test]
    fn select_add_notice_already_in_library_when_both_zero() {
        assert_eq!(select_add_notice(0, 0), AddNotice::AlreadyInLibrary);
    }

    #[test]
    fn select_add_notice_none_added_all_skipped_when_added_zero_skipped_nonzero() {
        assert_eq!(
            select_add_notice(0, 3),
            AddNotice::NoneAddedAllSkipped { skipped: 3 }
        );
    }

    #[test]
    fn add_paths_rar_blocked_by_policy_is_skipped_not_added() {
        // A .cbr file is rejected at probe time when allow_rar=false; it must be
        // counted as skipped, not added, and must not enter the library.
        let root = tempfile::tempdir().expect("tempdir");
        let cbr = root.path().join("manga.cbr");
        // Extension check fires before any bytes are read; any content works.
        std::fs::write(&cbr, b"dummy").expect("write dummy cbr");

        let mut lib = gashuu_core::Library::new();
        let policy = ArchivePolicy { allow_rar: false };
        let report = add_paths(&mut lib, vec![cbr], policy);

        assert!(report.added.is_empty(), "blocked RAR must never be added");
        assert_eq!(report.skipped, 1, "blocked RAR must be counted as skipped");
        assert_eq!(lib.books().len(), 0);
    }

    #[test]
    fn select_add_notice_added_with_skips_when_both_nonzero() {
        assert_eq!(
            select_add_notice(2, 1),
            AddNotice::AddedWithSkips {
                added: 2,
                skipped: 1
            }
        );
    }

    #[test]
    fn select_add_notice_added_when_added_nonzero_skipped_zero() {
        assert_eq!(select_add_notice(5, 0), AddNotice::Added { added: 5 });
    }
}
