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
    use std::path::PathBuf;

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

    #[test]
    fn ord_matches_path_component_order() {
        let names = ["/m/A/1.cbz", "/m/A 2.cbz", "/m/A-3.cbz", "/m/a.cbz"];
        let mut paths = names.map(PathBuf::from);
        paths.sort();
        let mut ids = names.map(BookId::new);
        ids.sort();
        assert_eq!(
            ids,
            paths.map(|path| BookId::from_canonical_path(&path).unwrap())
        );
    }

    #[test]
    fn ord_is_consistent_with_eq() {
        let first = BookId::new("/m//a");
        let second = BookId::new("/m/a");
        assert_ne!(first, second);
        assert_ne!(first.cmp(&second), Ordering::Equal);
    }

    #[test]
    fn display_is_raw_string() {
        assert_eq!(
            BookId::new("gashuu-web:session/a.cbz").to_string(),
            "gashuu-web:session/a.cbz"
        );
    }
}
