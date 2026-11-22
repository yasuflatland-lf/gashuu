//! Resolve book locations against the platform's source of bytes.
//!
//! Desktop IDs carry canonical filesystem locations. Web IDs carry session
//! registry locations until OPFS storage arrives in #660. The backend work
//! in #655–#660 moves location resolution behind the platform backend.

use gashuu_core::{ArchiveLoader, ArchivePolicy, BookId, CoreError, PageSource};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) fn open(path: &Path, policy: ArchivePolicy) -> Result<Arc<dyn PageSource>, CoreError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        ArchiveLoader::open_with_policy(path, policy)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let (name, bytes) = read_at(path)?;
        ArchiveLoader::open_read_at(name, bytes, policy)
    }
}

pub(crate) fn probe_page_count(
    path: &Path,
    policy: ArchivePolicy,
) -> Result<NonZeroUsize, CoreError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        ArchiveLoader::probe_page_count_with_policy(path, policy)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let (name, bytes) = read_at(path)?;
        ArchiveLoader::probe_page_count_read_at(name, bytes, policy)
    }
}

#[cfg(target_arch = "wasm32")]
fn read_at(path: &Path) -> Result<(&str, Arc<dyn gashuu_core::ReadAt>), CoreError> {
    let name = path.to_str().ok_or_else(|| CoreError::NonUtf8Path {
        path: path.to_string_lossy().into_owned(),
    })?;
    let bytes = crate::web_books::get(path).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "book data is unavailable in this browser session",
        )
    })?;
    Ok((name, Arc::new(bytes)))
}

pub(crate) fn exists(path: &Path) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        path.exists()
    }
    #[cfg(target_arch = "wasm32")]
    {
        crate::web_books::contains(path)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn is_dir(path: &Path) -> bool {
    path.is_dir()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn is_dir(_path: &Path) -> bool {
    false
}

fn identity(path: &Path) -> PathBuf {
    #[cfg(not(target_arch = "wasm32"))]
    {
        gashuu_core::canonical_identity(path)
    }
    #[cfg(target_arch = "wasm32")]
    {
        path.to_path_buf()
    }
}

/// Resolve an ID to a desktop path or a web session registry location.
pub(crate) fn locate(id: &BookId) -> PathBuf {
    PathBuf::from(id.as_str())
}

/// Invert `locate` for a picked or recent path before canonicalization.
pub(crate) fn request_id(path: &Path) -> Result<BookId, crate::backend::BackendError> {
    BookId::from_canonical_path(path)
        .map_err(|error| crate::backend::BackendError::from_core(&error))
}

/// Identify a picked location and derive its display title.
pub(crate) fn identify(path: &Path, is_dir: bool) -> Result<(BookId, String), CoreError> {
    let canonical = identity(path);
    let id = BookId::from_canonical_path(&canonical)?;
    Ok((id, gashuu_core::display_title(&canonical, is_dir)))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn locate_inverts_identify_for_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Some Book.cbz");
        std::fs::write(&path, []).unwrap();

        let (id, _) = identify(&path, is_dir(&path)).unwrap();
        assert_eq!(locate(&id), path.canonicalize().unwrap());
    }

    #[test]
    fn request_id_preserves_a_noncanonical_path() {
        let path = Path::new("books/../picked.cbz");
        assert_eq!(
            locate(&request_id(path).unwrap()).as_os_str(),
            path.as_os_str()
        );
    }

    #[cfg(unix)]
    #[test]
    fn request_id_rejects_a_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(std::ffi::OsStr::from_bytes(b"picked\xff.cbz"));
        assert!(matches!(
            request_id(path),
            Err(crate::backend::BackendError::Other { .. })
        ));
    }

    #[test]
    fn identify_title_uses_is_dir() {
        let path = Path::new("/manga/Series.1997");
        assert_eq!(identify(path, true).unwrap().1, "Series.1997");
        assert_eq!(identify(path, false).unwrap().1, "Series");
    }
}
