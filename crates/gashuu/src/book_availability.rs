use gashuu_core::Book;

/// Derived availability: whether the book's path currently resolves to a source.
///
/// Availability is not stored. An unavailable book is kept with its reading
/// position preserved; removal is an explicit user action.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn book_path_exists(book: &Book) -> bool {
    crate::book_path::exists(&crate::book_path::locate(book.id()))
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) use web::WebAvailability;

#[cfg(any(test, target_arch = "wasm32"))]
mod web {
    use gashuu_core::BookId;
    use std::{
        collections::HashSet,
        sync::{Arc, Mutex},
    };

    #[derive(Clone, Default)]
    pub(crate) struct WebAvailability(Arc<Mutex<Option<HashSet<String>>>>);

    impl WebAvailability {
        pub(crate) fn contains(&self, id: &BookId) -> bool {
            self.0
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(|set| set.contains(id.as_str()))
        }
        pub(crate) fn replace(&self, present: Vec<BookId>) {
            *self.0.lock().unwrap() = Some(present.into_iter().map(|id| id.to_string()).collect());
        }
        pub(crate) fn insert(&self, id: &BookId) {
            if let Some(set) = self.0.lock().unwrap().as_mut() {
                set.insert(id.to_string());
            }
        }
        pub(crate) fn remove(&self, id: &BookId) {
            if let Some(set) = self.0.lock().unwrap().as_mut() {
                set.remove(id.as_str());
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn web_inventory() -> WebAvailability {
    thread_local! { static INVENTORY:WebAvailability=WebAvailability::default(); }
    INVENTORY.with(Clone::clone)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn book_path_exists(book: &Book) -> bool {
    web_inventory().contains(book.id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::{BookId, Library};

    #[test]
    fn book_path_exists_reflects_path_existence() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        let mut library = Library::new();
        let (present_id, title) = crate::book_path::identify(&folder, true).unwrap();
        let missing_id = BookId::new("/manga/missing.cbz");
        library.add(present_id.clone(), title);
        library.register_opened(&missing_id, "missing", None);
        let present = library
            .books()
            .iter()
            .find(|book| book.id() == &present_id)
            .unwrap();
        let missing = library
            .books()
            .iter()
            .find(|book| book.id() == &missing_id)
            .unwrap();
        assert!(book_path_exists(present));
        assert!(!book_path_exists(missing));
    }
}

#[cfg(test)]
#[test]
fn web_inventory_tracks_import_and_removal_after_sweep() {
    use gashuu_core::BookId;
    let inventory = WebAvailability::default();
    let a = BookId::new("books/a/book");
    let b = BookId::new("books/b/book");
    assert!(inventory.contains(&a));
    inventory.replace(vec![b.clone()]);
    assert!(!inventory.contains(&a));
    assert!(inventory.contains(&b));
    inventory.insert(&a);
    assert!(inventory.contains(&a));
    inventory.remove(&b);
    assert!(!inventory.contains(&b));
}
