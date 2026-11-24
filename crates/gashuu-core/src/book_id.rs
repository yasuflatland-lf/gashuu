use crate::CoreError;
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, fmt, path::Path};

/// Opaque, UTF-8 identity of a book.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BookId(String);

impl BookId {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn from_canonical_path(path: &Path) -> Result<Self, CoreError> {
        path.to_str()
            .map(Self::new)
            .ok_or_else(|| CoreError::NonUtf8Path {
                path: path.to_string_lossy().into_owned(),
            })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Ord for BookId {
    fn cmp(&self, other: &Self) -> Ordering {
        // Preserve desktop path order, breaking component ties to agree with string equality.
        Path::new(self.as_str())
            .cmp(Path::new(other.as_str()))
            .then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for BookId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for BookId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_is_a_bare_string() {
        let json = r#""/m/a.cbz""#;
        let id: BookId = serde_json::from_str(json).unwrap();
        assert_eq!(id, BookId::new("/m/a.cbz"));
        assert_eq!(serde_json::to_string(&id).unwrap(), json);
    }

    #[test]
    fn from_canonical_path_accepts_utf8() {
        assert_eq!(
            BookId::from_canonical_path(Path::new("/m/本.cbz")).unwrap(),
            BookId::new("/m/本.cbz")
        );
    }

    #[test]
    #[cfg(unix)]
    fn from_canonical_path_rejects_non_utf8() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

        let path = Path::new(OsStr::from_bytes(b"/m/\xff.cbz"));
        assert!(matches!(
            BookId::from_canonical_path(path),
            Err(CoreError::NonUtf8Path { path: lossy }) if lossy == path.to_string_lossy()
        ));
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// Book IDs compare equal exactly when their raw strings match, and swapping IDs reverses the ordering.
            #[test]
            fn prop_book_id_order_agrees_with_equality(a in ".{0,40}", b in ".{0,40}") {
                let first = BookId::new(&a);
                let second = BookId::new(&b);
                let order = first.cmp(&second);

                prop_assert_eq!(order == Ordering::Equal, a == b);
                prop_assert_eq!(order, second.cmp(&first).reverse());
                prop_assert_eq!(first.cmp(&BookId::new(&a)), Ordering::Equal);
            }

            #[test]
            fn book_id_order_refines_path_component_order(
                (a, b) in prop_oneof![
                    4 => ("[a-c/ -]{0,8}", "[a-c/ -]{0,8}"),
                    1 => "[a-c/ -]{0,8}".prop_map(|a| (a.clone(), a)),
                    1 => "[a-c -]{1,3}".prop_map(|part| (format!("a//{part}"), format!("a/{part}"))),
                    1 => Just(("a/b".to_owned(), "a -/b".to_owned())),
                ],
            ) {
                let first = BookId::new(&a);
                let second = BookId::new(&b);
                let order = first.cmp(&second);
                let path_order = Path::new(&a).cmp(Path::new(&b));

                prop_assert_eq!(order == Ordering::Equal, first == second);
                prop_assert_eq!(order, second.cmp(&first).reverse());
                if path_order != Ordering::Equal {
                    prop_assert_eq!(order, path_order);
                }
            }
        }
    }

    #[test]
    fn display_is_raw_string() {
        assert_eq!(
            BookId::new("gashuu-web:session/a.cbz").to_string(),
            "gashuu-web:session/a.cbz"
        );
    }
}
