//! Dispatch a path to the right `PageSource`: directory -> FolderSource,
//! CBZ/ZIP (by extension, else magic bytes) -> ZipSource,
//! CBR/RAR (by extension, else magic bytes) -> RarSource.
//! Opening RAR requires the `rar` feature. Without it, dispatch returns [`CoreError::FormatDisabled`].

use crate::error::CoreError;
#[cfg(feature = "rar")]
use crate::page_source::RarSource;
use crate::page_source::{FolderSource, PageSource, ReadAt, ZipSource};
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::io::Read;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;

/// ZIP local-file, end-of-central-directory, and data-descriptor signatures (4 bytes each).
const ZIP_MAGICS: &[&[u8]] = &[b"PK\x03\x04", b"PK\x05\x06", b"PK\x07\x08"];

/// RAR magic. RAR4 and RAR5 share the same 6-byte prefix `Rar!\x1A\x07`; they
/// differ only in the 7th version byte (`\x00` for RAR4, `\x01` for RAR5), which
/// is deliberately NOT tested — so this one constant matches both formats.
const RAR_MAGIC: &[u8] = b"Rar!\x1A\x07";

/// Archive format recognized by an extension or magic-byte probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveKind {
    Zip,
    Rar,
}

impl Display for ArchiveKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Zip => "ZIP/CBZ",
            Self::Rar => "RAR/CBR",
        })
    }
}

/// Policy controlling which archive formats `ArchiveLoader` is permitted to open.
///
/// The default allows RAR, matching the default settings. Production callers
/// that use application settings always pass `Settings::archive_policy()`.
/// Set `allow_rar: false` to reject RAR/CBR archives and return
/// [`CoreError::FormatDisabled`]. Without the `rar` feature, both policy settings return that error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchivePolicy {
    /// When `false`, RAR/CBR paths are rejected with `CoreError::FormatDisabled`.
    pub allow_rar: bool,
}

impl Default for ArchivePolicy {
    fn default() -> Self {
        Self { allow_rar: true }
    }
}

/// An opened page source with at least one image page.
pub struct OpenedSource {
    source: Arc<dyn PageSource>,
    page_count: NonZeroUsize,
}

impl OpenedSource {
    pub(crate) fn new(source: Arc<dyn PageSource>, label: impl Display) -> Result<Self, CoreError> {
        let page_count =
            NonZeroUsize::new(source.list_pages().len()).ok_or_else(|| CoreError::EmptyBook {
                label: label.to_string(),
            })?;
        Ok(Self { source, page_count })
    }

    pub fn page_count(&self) -> NonZeroUsize {
        self.page_count
    }

    pub fn source(&self) -> &Arc<dyn PageSource> {
        &self.source
    }

    pub fn into_parts(self) -> (Arc<dyn PageSource>, NonZeroUsize) {
        (self.source, self.page_count)
    }
}

/// Opens page sources by path or from a positional byte source.
///
/// Path resolution order:
/// 1. If the path is a directory: [`FolderSource`].
/// 2. Extension check (cheap, no I/O): `cbz`/`zip` → [`ZipSource`]; `cbr`/`rar` → `RarSource`.
/// 3. Magic-byte sniff (first bytes of the file): ZIP signatures → [`ZipSource`]; RAR prefix → `RarSource`.
/// 4. Otherwise: [`CoreError::UnsupportedFormat`].
///
/// Opening RAR requires the `rar` feature. Without it, either detection path returns [`CoreError::FormatDisabled`].
///
/// [`ArchiveLoader::detect_kind`] detects an archive format from a name and leading bytes.
/// [`ArchiveLoader::open_read_at`] and [`ArchiveLoader::probe_page_count_read_at`] accept
/// a ZIP byte source through [`ReadAt`].
pub struct ArchiveLoader;

impl ArchiveLoader {
    /// Open a book from `path`, respecting `policy`. Returns
    /// [`CoreError::EmptyBook`] if the source opens successfully but has no image pages.
    ///
    /// Open errors propagate unchanged.
    pub fn open_book(
        path: impl AsRef<Path>,
        policy: ArchivePolicy,
    ) -> Result<OpenedSource, CoreError> {
        let path = path.as_ref();
        OpenedSource::new(Self::open_with_policy(path, policy)?, path.display())
    }

    /// Count image pages under the same rules as [`Self::open_book`].
    pub fn probe_page_count_with_policy(
        path: impl AsRef<Path>,
        policy: ArchivePolicy,
    ) -> Result<NonZeroUsize, CoreError> {
        Self::open_book(path, policy).map(|book| book.page_count())
    }

    /// Open `path` as the most appropriate [`PageSource`], respecting `policy`.
    ///
    /// When the resolved kind is RAR and `policy.allow_rar` is `false`, returns
    /// `Err(CoreError::FormatDisabled)` instead of opening `RarSource`.
    pub fn open_with_policy(
        path: impl AsRef<Path>,
        policy: ArchivePolicy,
    ) -> Result<Arc<dyn PageSource>, CoreError> {
        let path = path.as_ref();
        // A path that cannot be serialized as a JSON string must never become a Book:
        // one such path breaks EVERY library save (CORE-S-1). Reject at the door.
        let Some(name) = path.to_str() else {
            return Err(CoreError::NonUtf8Path {
                path: path.to_string_lossy().into_owned(),
            });
        };
        if path.is_dir() {
            return Ok(Arc::new(FolderSource::open(path)?));
        }
        let kind = detect(name, |head| Ok(std::fs::File::open(path)?.read(head)?))?;
        match kind {
            #[cfg(not(target_arch = "wasm32"))]
            Some(ArchiveKind::Zip) => Ok(Arc::new(ZipSource::open(path)?)),
            #[cfg(target_arch = "wasm32")]
            Some(ArchiveKind::Zip) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "operation not supported on this platform",
            )
            .into()),
            Some(ArchiveKind::Rar) if !policy.allow_rar => Err(CoreError::FormatDisabled {
                kind: ArchiveKind::Rar,
            }),
            #[cfg(feature = "rar")]
            Some(ArchiveKind::Rar) => Ok(Arc::new(RarSource::open(path)?)),
            #[cfg(not(feature = "rar"))]
            Some(ArchiveKind::Rar) => Err(CoreError::FormatDisabled {
                kind: ArchiveKind::Rar,
            }),
            None => Err(CoreError::UnsupportedFormat {
                label: path.display().to_string(),
            }),
        }
    }

    /// Detect by extension first (`cbz`/`zip` or `cbr`/`rar`, case-insensitive),
    /// then by the magic bytes in `head`.
    pub fn detect_kind(name: &str, head: &[u8]) -> Option<ArchiveKind> {
        kind_from_name(name).or_else(|| kind_from_magic(head))
    }

    /// Open a ZIP page source from positional bytes, using `name` for detection and errors.
    ///
    /// The read-at route supports ZIP only. RAR always returns
    /// [`CoreError::FormatDisabled`], regardless of policy.
    pub fn open_read_at(
        name: &str,
        src: Arc<dyn ReadAt>,
    ) -> Result<Arc<dyn PageSource>, CoreError> {
        let kind = detect(name, |head| Ok(src.read_at(0, head)?))?;
        match kind {
            Some(ArchiveKind::Zip) => Ok(Arc::new(ZipSource::from_read_at(src)?)),
            Some(ArchiveKind::Rar) => Err(CoreError::FormatDisabled {
                kind: ArchiveKind::Rar,
            }),
            None => Err(CoreError::UnsupportedFormat {
                label: name.to_owned(),
            }),
        }
    }

    /// Open a book from a ZIP byte source, using `name` to label an empty book.
    pub fn open_book_read_at(name: &str, src: Arc<dyn ReadAt>) -> Result<OpenedSource, CoreError> {
        OpenedSource::new(Self::open_read_at(name, src)?, name)
    }

    /// Count image pages in a ZIP byte source, returning [`CoreError::EmptyBook`]
    /// with `name` if the archive opens successfully and contains no image pages.
    /// RAR is disabled regardless of policy.
    pub fn probe_page_count_read_at(
        name: &str,
        src: Arc<dyn ReadAt>,
    ) -> Result<NonZeroUsize, CoreError> {
        Self::open_book_read_at(name, src).map(|book| book.page_count())
    }
}

/// Prefer a known extension. Otherwise, one bounded read covers both signatures.
fn detect(
    name: &str,
    read_head: impl FnOnce(&mut [u8; 6]) -> Result<usize, CoreError>,
) -> Result<Option<ArchiveKind>, CoreError> {
    if let Some(kind) = kind_from_name(name) {
        return Ok(Some(kind));
    }
    let mut head = [0; 6];
    let n = read_head(&mut head)?;
    Ok(kind_from_magic(&head[..n]))
}

fn kind_from_name(name: &str) -> Option<ArchiveKind> {
    let ext = Path::new(name).extension().and_then(|e| e.to_str())?;
    if ext.eq_ignore_ascii_case("cbz") || ext.eq_ignore_ascii_case("zip") {
        Some(ArchiveKind::Zip)
    } else if ext.eq_ignore_ascii_case("cbr") || ext.eq_ignore_ascii_case("rar") {
        Some(ArchiveKind::Rar)
    } else {
        None
    }
}

fn kind_from_magic(head: &[u8]) -> Option<ArchiveKind> {
    if ZIP_MAGICS.iter().any(|magic| head.starts_with(magic)) {
        Some(ArchiveKind::Zip)
    } else if head.starts_with(RAR_MAGIC) {
        Some(ArchiveKind::Rar)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "archive_loader/tests.rs"]
mod tests;
