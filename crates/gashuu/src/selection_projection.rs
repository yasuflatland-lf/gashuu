//! Selection-over-projection joins: the operations that combine the pure
//! `LibrarySelectionState` path-set with the search projection
//! (`LibrarySearchState::visible_indices`) and the `Library`. These live OUTSIDE
//! `LibrarySelectionState` so that type owns only the path-set invariant (its
//! ORTHOGONAL-to-search guarantee), while "act on just the visible slice" is a
//! separate, composable concern.

use crate::library_model::{LibrarySearchState, LibrarySelectionState};
use gashuu_core::Library;
use std::path::Path;

/// The canonical paths of the currently visible books — the search projection
/// joined against the library. The shared join the four ops below are built on.
fn visible_paths<'a>(
    search: &'a LibrarySearchState,
    library: &'a Library,
) -> impl Iterator<Item = &'a Path> {
    search
        .visible_indices()
        .iter()
        .filter_map(move |&index| library.books().get(index).map(|book| book.path()))
}

/// Select every CURRENTLY VISIBLE book, leaving already-selected non-visible
/// books untouched.
pub(crate) fn select_visible(
    selection: &mut LibrarySelectionState,
    search: &LibrarySearchState,
    library: &Library,
) {
    for path in visible_paths(search, library) {
        selection.insert(path.to_path_buf());
    }
}

/// Deselect every CURRENTLY VISIBLE book, leaving selected-but-not-visible books
/// untouched (selection is ORTHOGONAL to the search query).
pub(crate) fn deselect_visible(
    selection: &mut LibrarySelectionState,
    search: &LibrarySearchState,
    library: &Library,
) {
    for path in visible_paths(search, library) {
        selection.remove(path);
    }
}

/// Whether every currently visible book is selected. `false` when there are no
/// visible books (an empty projection has nothing to consider "all selected").
pub(crate) fn all_visible_selected(
    selection: &LibrarySelectionState,
    search: &LibrarySearchState,
    library: &Library,
) -> bool {
    let mut any = false;
    let all = visible_paths(search, library).all(|path| {
        any = true;
        selection.contains(path)
    });
    any && all
}

/// How many of the currently visible books are selected.
pub(crate) fn visible_selected_count(
    selection: &LibrarySelectionState,
    search: &LibrarySearchState,
    library: &Library,
) -> usize {
    visible_paths(search, library)
        .filter(|path| selection.contains(path))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_model::{LibrarySearchState, LibrarySelectionState};
    use gashuu_core::Library;
    use std::path::PathBuf;

    #[test]
    fn select_visible_selects_only_the_visible_projection() {
        // With an active "alpha" filter, select_visible selects ONLY the visible
        // (alpha) book; the filtered-out beta is left untouched.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        let alpha_path = lib.books()[0].path().to_path_buf();
        let beta_path = lib.books()[1].path().to_path_buf();

        let mut search = LibrarySearchState::default();
        search.set_query("alpha".to_string(), &lib);
        let mut sel = LibrarySelectionState::default();
        select_visible(&mut sel, &search, &lib);

        assert!(sel.contains(&alpha_path), "visible book is selected");
        assert!(!sel.contains(&beta_path), "filtered-out book is not");
        assert_eq!(sel.count(), 1);
    }

    #[test]
    fn all_visible_selected_flips_with_the_visible_set() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        let alpha_path = lib.books()[0].path().to_path_buf();
        let beta_path = lib.books()[1].path().to_path_buf();

        let mut search = LibrarySearchState::default();
        search.set_query(String::new(), &lib); // both visible
        let mut sel = LibrarySelectionState::default();

        assert!(
            !all_visible_selected(&sel, &search, &lib),
            "nothing selected ⇒ not all-visible-selected"
        );
        sel.toggle(alpha_path.clone());
        assert!(
            !all_visible_selected(&sel, &search, &lib),
            "only one of two visible selected"
        );
        assert_eq!(visible_selected_count(&sel, &search, &lib), 1);

        sel.toggle(beta_path);
        assert!(
            all_visible_selected(&sel, &search, &lib),
            "both visible now selected ⇒ all-visible-selected"
        );
        assert_eq!(visible_selected_count(&sel, &search, &lib), 2);

        // Narrow to "alpha": only alpha visible and it IS selected ⇒ flips back to true.
        search.set_query("alpha".to_string(), &lib);
        assert!(all_visible_selected(&sel, &search, &lib));
        assert_eq!(visible_selected_count(&sel, &search, &lib), 1);
    }

    #[test]
    fn deselect_visible_removes_only_visible_selections_preserves_out_of_search() {
        // Orthogonality: deselect_visible must only remove visible selections;
        // a selected book that is filtered out of the visible projection must stay selected.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        let alpha_path = lib.books()[0].path().to_path_buf();
        let beta_path = lib.books()[1].path().to_path_buf();

        let mut search = LibrarySearchState::default();
        // Only alpha is visible under "alpha" filter.
        search.set_query("alpha".to_string(), &lib);
        assert_eq!(search.visible_indices(), &[0]);

        let mut sel = LibrarySelectionState::default();
        // Select both alpha (visible) and beta (out-of-search).
        sel.toggle(alpha_path.clone());
        sel.toggle(beta_path.clone());
        assert_eq!(sel.count(), 2);

        // deselect_visible must only remove alpha (visible); beta stays selected.
        deselect_visible(&mut sel, &search, &lib);
        assert!(
            !sel.contains(&alpha_path),
            "visible alpha must be deselected"
        );
        assert!(
            sel.contains(&beta_path),
            "out-of-search beta must remain selected (orthogonality)"
        );
        assert_eq!(sel.count(), 1);
    }

    #[test]
    fn deselect_visible_empty_projection_is_noop() {
        // An empty visible projection must leave the selection unchanged.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        let alpha_path = lib.books()[0].path().to_path_buf();

        let mut search = LibrarySearchState::default();
        search.set_query("no-match".to_string(), &lib);
        assert!(search.visible_indices().is_empty());

        let mut sel = LibrarySelectionState::default();
        sel.toggle(alpha_path.clone());
        assert_eq!(sel.count(), 1);

        // No visible books ⇒ no-op.
        deselect_visible(&mut sel, &search, &lib);
        assert_eq!(sel.count(), 1, "empty projection must be a no-op");
        assert!(
            sel.contains(&alpha_path),
            "alpha must still be selected after no-op deselect_visible"
        );
    }

    #[test]
    fn select_visible_then_deselect_visible_clears_all_visible() {
        // After select_visible then deselect_visible, all_visible_selected is false
        // and visible_selected_count is 0.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());

        let mut search = LibrarySearchState::default();
        search.set_query(String::new(), &lib); // both visible

        let mut sel = LibrarySelectionState::default();
        select_visible(&mut sel, &search, &lib);
        assert!(
            all_visible_selected(&sel, &search, &lib),
            "after select_visible, all visible must be selected"
        );

        deselect_visible(&mut sel, &search, &lib);
        assert!(
            !all_visible_selected(&sel, &search, &lib),
            "after deselect_visible, all_visible_selected must be false"
        );
        assert_eq!(
            visible_selected_count(&sel, &search, &lib),
            0,
            "visible_selected_count must be 0 after deselect_visible"
        );
    }

    #[test]
    fn deselect_visible_no_panic_when_some_visible_were_never_selected() {
        // Some visible books were never selected: deselect_visible must not panic
        // and must not over-remove (already-absent entries are silently skipped).
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        let alpha_path = lib.books()[0].path().to_path_buf();

        let mut search = LibrarySearchState::default();
        search.set_query(String::new(), &lib); // both visible

        let mut sel = LibrarySelectionState::default();
        // Only alpha is selected; beta is visible but was never selected.
        sel.toggle(alpha_path.clone());
        assert_eq!(sel.count(), 1);

        // Must not panic even though beta was never in the selection.
        deselect_visible(&mut sel, &search, &lib);
        assert_eq!(sel.count(), 0, "alpha must be deselected");
        assert!(
            !sel.contains(&alpha_path),
            "alpha must no longer be selected"
        );
    }

    #[test]
    fn all_visible_selected_false_for_empty_projection() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        let mut search = LibrarySearchState::default();
        search.set_query("no-match".to_string(), &lib); // empty projection
        assert!(search.visible_indices().is_empty());
        let sel = LibrarySelectionState::default();
        assert!(
            !all_visible_selected(&sel, &search, &lib),
            "an empty visible set is not all-selected"
        );
        assert_eq!(visible_selected_count(&sel, &search, &lib), 0);
    }

    #[test]
    fn deselect_visible_after_query_pivot_removes_only_new_projection() {
        // Production sequence: select_visible with broad/empty query (all books
        // selected), then set_query narrowing the projection to a subset, then
        // deselect_visible — only the narrowed projection's books must be removed;
        // books outside the narrowed projection must remain selected.
        //
        // Library: alpha, beta, gamma (natural sort order).
        // Step 1: empty query → all 3 visible → select_visible selects all.
        // Step 2: narrow to "alpha" → only alpha visible.
        // Step 3: deselect_visible → only alpha removed; beta and gamma stay.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/gamma.cbz")).is_some());
        let alpha_path = lib.books()[0].path().to_path_buf();
        let beta_path = lib.books()[1].path().to_path_buf();
        let gamma_path = lib.books()[2].path().to_path_buf();

        let mut search = LibrarySearchState::default();
        // Step 1: broad (empty) query — all books visible.
        search.set_query(String::new(), &lib);
        assert_eq!(search.visible_indices().len(), 3);

        let mut sel = LibrarySelectionState::default();
        select_visible(&mut sel, &search, &lib);
        assert_eq!(
            sel.count(),
            3,
            "all three books must be selected after select_visible"
        );

        // Step 2: narrow to "alpha" — only alpha is visible now.
        search.set_query("alpha".to_string(), &lib);
        assert_eq!(search.visible_indices(), &[0], "only alpha index visible");

        // Step 3: deselect_visible removes ONLY alpha (the new projection).
        deselect_visible(&mut sel, &search, &lib);

        assert!(
            !sel.contains(&alpha_path),
            "alpha (in narrowed projection) must be deselected"
        );
        assert!(
            sel.contains(&beta_path),
            "beta (outside narrowed projection) must remain selected"
        );
        assert!(
            sel.contains(&gamma_path),
            "gamma (outside narrowed projection) must remain selected"
        );
        // Exactly the two out-of-projection books remain.
        assert_eq!(
            sel.count(),
            2,
            "count must equal the books outside the narrowed projection"
        );
    }
}
