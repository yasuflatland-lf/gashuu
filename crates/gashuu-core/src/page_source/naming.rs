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
