//! Session-only positional bytes for books selected in the browser.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) const SESSION_ROOT: &str = "gashuu-web:";

struct Registry {
    namespace: PathBuf,
    books: HashMap<PathBuf, Arc<[u8]>>,
}

impl Registry {
    fn new(session: &str) -> Self {
        Self {
            namespace: PathBuf::from(SESSION_ROOT).join(session),
            books: HashMap::new(),
        }
    }

    fn insert(&mut self, name: &str, bytes: Arc<[u8]>) -> PathBuf {
        let name = name.replace(['/', '\\', '\0'], "_");
        let mut path = self.namespace.join(&name);
        let (stem, extension) = name.rsplit_once('.').unwrap_or((&name, ""));
        let mut suffix = 2;
        while self.books.contains_key(&path) {
            path = self
                .namespace
                .join(format!("{stem} ({suffix}).{extension}"));
            suffix += 1;
        }
        self.books.insert(path.clone(), bytes);
        path
    }

    fn get(&self, path: &Path) -> Option<Arc<[u8]>> {
        self.books.get(path).cloned()
    }
}

pub(crate) fn supported_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cbz") || ext.eq_ignore_ascii_case("zip"))
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    // A new page load gets a fresh namespace, even if the same name is picked again.
    static BOOKS: std::cell::RefCell<Registry> = std::cell::RefCell::new(Registry::new(&format!(
        "{:x}-{:x}", crate::clock::unix_now_secs(), js_sys::Math::random().to_bits()
    )));
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn insert(name: &str, bytes: Arc<[u8]>) -> PathBuf {
    BOOKS.with_borrow_mut(|books| books.insert(name, bytes))
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn get(path: &Path) -> Option<Arc<[u8]>> {
    BOOKS.with_borrow(|books| books.get(path))
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn contains(path: &Path) -> bool {
    BOOKS.with_borrow(|books| books.books.contains_key(path))
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn remove(path: &Path) {
    BOOKS.with_borrow_mut(|books| books.books.remove(path));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_cbz_and_zip_regardless_of_case() {
        for name in ["book.cbz", "book.ZIP", "日本語.CbZ"] {
            assert!(supported_name(name), "{name}");
        }
        for name in [
            "book.cbr", "book.RAR", "book.7z", "book.cb7", "book.pdf", "book",
        ] {
            assert!(!supported_name(name), "{name}");
        }
    }

    #[test]
    fn same_name_gets_distinct_keys_without_replacing_bytes() {
        let mut registry = Registry::new("a");
        let first: Arc<[u8]> = Arc::from([1]);
        let second: Arc<[u8]> = Arc::from([2]);
        let path = registry.insert("日本語.CBZ", Arc::clone(&first));
        let duplicate = registry.insert("日本語.CBZ", Arc::clone(&second));
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("日本語.CBZ")
        );
        assert_eq!(
            duplicate.file_name().and_then(|name| name.to_str()),
            Some("日本語 (2).CBZ")
        );
        assert!(Arc::ptr_eq(&first, &registry.get(&path).unwrap()));
        assert!(Arc::ptr_eq(&second, &registry.get(&duplicate).unwrap()));
    }

    #[test]
    fn restored_keys_miss_even_after_adding_the_same_name_again() {
        let old = Registry::new("a").insert("book.zip", Arc::from([1]));
        let mut reloaded = Registry::new("b");
        let new = reloaded.insert("book.zip", Arc::from([2]));
        assert_ne!(old, new);
        assert!(reloaded.get(&old).is_none());
    }
}
