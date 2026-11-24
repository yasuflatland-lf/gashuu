//! Selection-over-projection joins: the operations that combine the pure
//! `LibrarySelectionState` ID set with the search projection
//! (`LibrarySearchState::visible_indices`) and the `Library`. These live OUTSIDE
//! `LibrarySelectionState` so that type owns only the ID set invariant (its
//! ORTHOGONAL-to-search guarantee), while "act on just the visible slice" is a
//! separate, composable concern.

use crate::library_model::{LibrarySearchState, LibrarySelectionState};
use gashuu_core::{BookId, Library};

/// The IDs of the currently visible books — the search projection
/// joined against the library. The shared join the four ops below are built on.
fn visible_ids<'a>(
    search: &'a LibrarySearchState,
    library: &'a Library,
) -> impl Iterator<Item = &'a BookId> {
    search
        .visible_indices()
        .iter()
        .filter_map(move |&index| library.books().get(index).map(|book| book.id()))
}

/// Select every CURRENTLY VISIBLE book, leaving already-selected non-visible
/// books untouched.
pub(crate) fn select_visible(
    selection: &mut LibrarySelectionState,
    search: &LibrarySearchState,
    library: &Library,
) {
    for id in visible_ids(search, library) {
        selection.insert(id.clone());
    }
}

/// Deselect every CURRENTLY VISIBLE book, leaving selected-but-not-visible books
/// untouched (selection is ORTHOGONAL to the search query).
pub(crate) fn deselect_visible(
    selection: &mut LibrarySelectionState,
    search: &LibrarySearchState,
    library: &Library,
) {
    for id in visible_ids(search, library) {
        selection.remove(id);
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
    let all = visible_ids(search, library).all(|id| {
        any = true;
        selection.contains(id)
    });
    any && all
}

/// How many of the currently visible books are selected.
pub(crate) fn visible_selected_count(
    selection: &LibrarySelectionState,
    search: &LibrarySearchState,
    library: &Library,
) -> usize {
    visible_ids(search, library)
        .filter(|id| selection.contains(id))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    proptest! {
        #[test]
        fn visible_selection_matches_set_operations_across_a_query_pivot(
            selected in proptest::collection::btree_set(0usize..10, 0..=10),
            before in proptest::collection::btree_set(0usize..10, 0..=10),
            after in proptest::collection::btree_set(0usize..10, 0..=10),
        ) {
            let ids: Vec<_> = (0..10).map(|i| BookId::new(format!("/manga/{i}.cbz"))).collect();
            let mut library = Library::new();
            library.add_many(ids.iter().map(|id| (id.clone(), id.to_string())).collect());
            let to_ids = |members: &BTreeSet<usize>| {
                members.iter().map(|&i| ids[i].clone()).collect::<BTreeSet<_>>()
            };
            let mut selection = LibrarySelectionState::default();
            let mut expected = to_ids(&selected);
            for id in &expected {
                selection.insert(id.clone());
            }

            let mut search = LibrarySearchState::default();
            for (membership, select) in [(&before, true), (&after, false)] {
                let visible = to_ids(membership);
                search.set_query("matches no book".into(), &library);
                search.force_visible(visible.iter().cloned(), &library);
                let expected_all = !visible.is_empty() && visible.is_subset(&expected);
                prop_assert_eq!(all_visible_selected(&selection, &search, &library), expected_all);
                prop_assert_eq!(
                    visible_selected_count(&selection, &search, &library),
                    expected.intersection(&visible).count()
                );

                if select {
                    select_visible(&mut selection, &search, &library);
                    expected.extend(visible.iter().cloned());
                } else {
                    deselect_visible(&mut selection, &search, &library);
                    expected.retain(|id| !visible.contains(id));
                }

                let actual: BTreeSet<_> = selection.selected().cloned().collect();
                prop_assert_eq!(&actual, &expected);
                prop_assert_eq!(
                    all_visible_selected(&selection, &search, &library),
                    !visible.is_empty() && visible.is_subset(&expected)
                );
                prop_assert_eq!(
                    visible_selected_count(&selection, &search, &library),
                    expected.intersection(&visible).count()
                );
            }
        }
    }
}
