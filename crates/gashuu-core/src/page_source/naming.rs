//! Helpers for image-extension recognition and archive path/entry validation.
//!
//! Provides image-extension detection (`IMAGE_EXTS`, `has_image_ext`) and
//! archive entry path safety checks (`enclosed_name`), used by `FolderSource`
//! (directory walk) and `ZipSource` (ZIP/CBZ archive entries). Filename ordering
//! is provided by `crate::ordering::natural_cmp`.

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
