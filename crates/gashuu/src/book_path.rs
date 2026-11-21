//! Resolve a book path against the platform's source of book bytes.

use gashuu_core::{ArchiveLoader, ArchivePolicy, CoreError, PageSource};
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

pub(crate) fn identity(path: &Path) -> PathBuf {
    #[cfg(not(target_arch = "wasm32"))]
    {
        gashuu_core::canonical_identity(path)
    }
    #[cfg(target_arch = "wasm32")]
    {
        path.to_path_buf()
    }
}
