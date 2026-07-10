//! Page-membership policy and safety limits for archive/directory entries.
//!
//! Owns the rules that decide whether an entry becomes a page and how it may be
//! read safely: image-extension admission (`IMAGE_EXTS`, `has_image_ext`),
//! entry classification (`EntryClass`, `classify_entry`), zip-slip path
//! containment (`enclosed_name`), and the shared per-entry byte ceiling
//! (`MAX_ENTRY_BYTES`, `cap_or_reject`) that guards streaming reads. Used by
//! `FolderSource` (directory walk) and `ZipSource`/`RarSource` (archive
//! entries). Filename ordering is provided by `crate::ordering::natural_cmp`.

use crate::error::CoreError;
use std::io::Read;

/// Image extensions recognized (case-insensitive).
pub(crate) const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "avif"];

/// Per-file uncompressed ceiling shared by all archive sources. Entries
/// declaring more are skipped at open; reads are also capped to this many bytes
/// to defend against size-spoofing archive bombs (manga pages are images, far
/// under this in practice). Lives here (not in `zip`/`rar`) because the ceiling
/// is a property of the archive-entry domain, not of any one container format.
pub(crate) const MAX_ENTRY_BYTES: u64 = 500 * 1024 * 1024;

/// True when `path` has a recognized image extension (ASCII case-insensitive).
pub(crate) fn has_image_ext(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            IMAGE_EXTS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// True when `path` is macOS archive/filesystem metadata rather than a real
/// image page. Returns true when EITHER the final file-name component starts
/// with `.` (covers AppleDouble resource forks like `._cover.jpg` and dotfiles
/// like `.DS_Store`), OR any path component equals `__MACOSX`.
///
/// macOS resource forks carry the original file's image extension (`._x.jpg`)
/// and, because `natural_cmp` orders case-insensitively (`'_'` < `'d'`), the
/// `__MACOSX/...` tree sorts AHEAD of the real folder — so without this filter
/// such an entry can be decoded as page 0 (the cover) and fail. A real image
/// name never starts with `.`, so this exclusion is safe.
pub(crate) fn is_macos_metadata(path: &std::path::Path) -> bool {
    use std::path::Component;
    let starts_with_dot = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.starts_with('.'));
    let in_macosx = path
        .components()
        .any(|comp| matches!(comp, Component::Normal(p) if p == std::ffi::OsStr::new("__MACOSX")));
    starts_with_dot || in_macosx
}

/// The listing decision for one archive entry, shared by `ZipSource` and
/// `RarSource` so "what counts as a page vs. a skip vs. expected noise" is
/// single-owned rather than open-coded (and drifting) in each source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryClass {
    /// A real image page within the size ceiling: index it.
    Page,
    /// An oversized image entry: drop it and COUNT it as a user-visible skip.
    Skip,
    /// A directory, non-image, or macOS-metadata entry: drop it WITHOUT counting
    /// (expected noise, never a skip).
    Ignore,
}

/// Classify one archive entry from its safe (`enclosed_name`-resolved) `name`,
/// whether it `is_dir`, and its `declared_size` against `max`. Centralizes the
/// page-membership rule both archive sources share: directories, non-image
/// extensions, and macOS metadata (`__MACOSX/...`, dotfiles, AppleDouble
/// `._x.jpg` resource forks) are [`EntryClass::Ignore`]d as expected noise; an
/// image whose declared size exceeds `max` is an [`EntryClass::Skip`] (counted);
/// everything else is an [`EntryClass::Page`]. The membership check precedes the
/// size check, so an oversized NON-image is `Ignore`, never a `Skip`. The
/// format-specific zip-slip guard and header iteration stay in each source.
pub(crate) fn classify_entry(
    name: &std::path::Path,
    is_dir: bool,
    declared_size: u64,
    max: u64,
) -> EntryClass {
    if is_dir || !has_image_ext(name) || is_macos_metadata(name) {
        return EntryClass::Ignore;
    }
    if declared_size > max {
        return EntryClass::Skip;
    }
    EntryClass::Page
}

/// Resolve a relative, traversal-free path or reject it. Returns `None` when the
/// entry name is absolute, has a root/prefix component, or contains any `..`
/// component (path traversal). Mirrors `zip::read::ZipFile::enclosed_name`
/// semantics so RAR entries get the same zip-slip protection as ZIP entries.
pub(crate) fn enclosed_name(path: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::path::Component;
    if path.is_absolute() {
        return None;
    }
    let mut out = std::path::PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            Component::ParentDir => return None,
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// Read `src` to the end but capped at `max + 1` bytes, rejecting with
/// [`CoreError::EntryTooLarge`] (named `name`) when the result exceeds `max`.
/// Reading one byte past the ceiling makes an over-limit source detectable:
/// landing on exactly `max + 1` bytes means the real size is over the cap. This is
/// the actual streaming size defense (the constant lives here as `MAX_ENTRY_BYTES`
/// because the ceiling is an archive-entry-domain property). `capacity_hint`
/// pre-sizes the buffer purely as a growth hint — NOT the defense — so pass `0`
/// for none.
///
/// Shared by the streaming readers: `FolderSource` file reads and `ZipSource`
/// entry reads. `RarSource` cannot stream-cap (`unrar`'s `read()` materializes the
/// whole entry with no `take`), so it keeps its declared-`unpacked_size`
/// re-validation instead.
pub(crate) fn cap_or_reject(
    src: impl Read,
    name: &str,
    max: u64,
    capacity_hint: usize,
) -> Result<Vec<u8>, CoreError> {
    let mut buf = Vec::with_capacity(capacity_hint);
    src.take(max + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return Err(CoreError::EntryTooLarge {
            name: name.to_string(),
            max,
        });
    }
    Ok(buf)
}

#[cfg(test)]
mod image_ext_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn has_image_ext_recognizes_common_formats() {
        assert!(has_image_ext(Path::new("photo.png")));
        assert!(has_image_ext(Path::new("photo.jpg")));
        assert!(has_image_ext(Path::new("photo.jpeg")));
        assert!(has_image_ext(Path::new("photo.avif")));
    }

    #[test]
    fn has_image_ext_is_case_insensitive() {
        assert!(has_image_ext(Path::new("photo.PNG")));
        assert!(has_image_ext(Path::new("photo.JPG")));
        assert!(has_image_ext(Path::new("photo.Jpeg")));
        assert!(has_image_ext(Path::new("photo.AVIF")));
    }

    #[test]
    fn has_image_ext_rejects_non_images() {
        assert!(!has_image_ext(Path::new("notes.txt")));
        assert!(!has_image_ext(Path::new("archive.zip")));
        assert!(!has_image_ext(Path::new("noextension")));
        // `.avi` (video) must not prefix-match the `avif` entry.
        assert!(!has_image_ext(Path::new("video.avi")));
    }
}

#[cfg(test)]
mod macos_metadata_tests {
    use super::is_macos_metadata;
    use std::path::Path;

    #[test]
    fn apple_double_resource_fork_is_metadata() {
        assert!(is_macos_metadata(Path::new("._cover.jpg")));
    }

    #[test]
    fn macosx_tree_entry_is_metadata() {
        assert!(is_macos_metadata(Path::new("__MACOSX/foo/._x.jpg")));
    }

    #[test]
    fn ds_store_is_metadata() {
        assert!(is_macos_metadata(Path::new(".DS_Store")));
    }

    #[test]
    fn nested_real_image_is_not_metadata() {
        assert!(!is_macos_metadata(Path::new("sub/page1.jpg")));
    }

    #[test]
    fn plain_image_name_is_not_metadata() {
        assert!(!is_macos_metadata(Path::new("Cover.jpg")));
    }
}

#[cfg(test)]
mod enclosed_name_tests {
    use super::enclosed_name;
    use std::path::{Path, PathBuf};

    #[test]
    fn nested_relative_path_is_allowed() {
        assert_eq!(
            enclosed_name(Path::new("a/b.png")),
            Some(PathBuf::from("a/b.png"))
        );
    }

    #[test]
    fn cur_dir_component_is_stripped() {
        assert_eq!(
            enclosed_name(Path::new("./a.png")),
            Some(PathBuf::from("a.png"))
        );
    }

    #[test]
    fn parent_dir_traversal_is_rejected() {
        assert_eq!(enclosed_name(Path::new("../evil.png")), None);
    }

    #[test]
    fn absolute_path_is_rejected() {
        assert_eq!(enclosed_name(Path::new("/abs/x.png")), None);
    }

    #[test]
    fn interior_parent_dir_is_rejected() {
        assert_eq!(enclosed_name(Path::new("a/../b.png")), None);
    }
}

#[cfg(test)]
mod classify_entry_tests {
    use super::{classify_entry, EntryClass, MAX_ENTRY_BYTES};
    use std::path::Path;

    #[test]
    fn image_within_size_is_a_page() {
        assert_eq!(
            classify_entry(Path::new("sub/page1.jpg"), false, 1_000, MAX_ENTRY_BYTES),
            EntryClass::Page
        );
    }

    #[test]
    fn oversized_image_is_a_counted_skip() {
        assert_eq!(
            classify_entry(Path::new("page1.png"), false, 11, 10),
            EntryClass::Skip
        );
    }

    #[test]
    fn directory_is_ignored() {
        assert_eq!(
            classify_entry(Path::new("folder"), true, 0, MAX_ENTRY_BYTES),
            EntryClass::Ignore
        );
    }

    #[test]
    fn non_image_is_ignored() {
        assert_eq!(
            classify_entry(Path::new("notes.txt"), false, 1, MAX_ENTRY_BYTES),
            EntryClass::Ignore
        );
    }

    #[test]
    fn macos_metadata_is_ignored_even_when_image_named() {
        // AppleDouble forks / `__MACOSX` trees carry image extensions but are metadata,
        // not pages — Ignored (not a counted skip) via this shared classifier.
        assert_eq!(
            classify_entry(
                Path::new("__MACOSX/Manga/._001.jpg"),
                false,
                1,
                MAX_ENTRY_BYTES
            ),
            EntryClass::Ignore
        );
        assert_eq!(
            classify_entry(Path::new("._cover.jpg"), false, 1, MAX_ENTRY_BYTES),
            EntryClass::Ignore
        );
    }

    #[test]
    fn oversized_non_image_is_ignored_not_skipped() {
        // The membership check precedes the size check, so a giant non-image is
        // expected noise, never a user-visible skip.
        assert_eq!(
            classify_entry(Path::new("huge.bin"), false, u64::MAX, 10),
            EntryClass::Ignore
        );
    }
}

#[cfg(test)]
mod cap_or_reject_tests {
    use super::cap_or_reject;
    use crate::error::CoreError;

    #[test]
    fn under_cap_returns_all_bytes() {
        let data = b"hello".to_vec();
        let out = cap_or_reject(data.as_slice(), "x.png", 10, 0).expect("under cap");
        assert_eq!(out, data);
    }

    #[test]
    fn exactly_at_cap_is_accepted() {
        let data = vec![0u8; 10];
        let out = cap_or_reject(data.as_slice(), "x.png", 10, 0).expect("at cap");
        assert_eq!(out.len(), 10);
    }

    #[test]
    fn over_cap_rejects_with_entry_too_large() {
        let data = vec![0u8; 11];
        let err = cap_or_reject(data.as_slice(), "big.png", 10, 0).unwrap_err();
        match err {
            CoreError::EntryTooLarge { name, max } => {
                assert_eq!(name, "big.png");
                assert_eq!(max, 10);
            }
            other => panic!("expected EntryTooLarge, got {other:?}"),
        }
    }
}
