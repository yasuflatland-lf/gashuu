//! Shared helpers for natural-sort ordering and image-extension detection.
//!
//! These helpers are shared by `FolderSource` (directory walk) and `ZipSource`
//! (ZIP/CBZ archive entries) so that all page sources sort filenames and
//! recognize image extensions identically.

use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

/// Image extensions recognized (case-insensitive).
pub(crate) const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg"];

/// Compare two file names in natural order so embedded numbers sort by numeric
/// value (`2.png` < `10.png`). Non-digit runs compare case-insensitively (ASCII)
/// with the raw chars as a stable tiebreaker, giving a total order.
pub(crate) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let run_a = take_digits(&mut ai);
                    let run_b = take_digits(&mut bi);
                    match cmp_numeric(&run_a, &run_b) {
                        Ordering::Equal => continue,
                        ord => return ord,
                    }
                } else {
                    // Compare case-insensitively, falling back to the raw chars
                    // as a stable tiebreaker so the order is total.
                    let ord = ca
                        .to_ascii_lowercase()
                        .cmp(&cb.to_ascii_lowercase())
                        .then_with(|| ca.cmp(&cb));
                    match ord {
                        Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        other => return other,
                    }
                }
            }
        }
    }
}

/// Consume and return the maximal leading run of ASCII digits.
pub(crate) fn take_digits(it: &mut Peekable<Chars<'_>>) -> String {
    let mut run = String::new();
    while let Some(&c) = it.peek() {
        if c.is_ascii_digit() {
            run.push(c);
            it.next();
        } else {
            break;
        }
    }
    run
}

/// Compare two digit runs by numeric value without integer overflow: strip
/// leading zeros, compare by length then lexically; equal value falls back to
/// the raw runs so padding differences stay deterministic.
pub(crate) fn cmp_numeric(a: &str, b: &str) -> Ordering {
    let ta = a.trim_start_matches('0');
    let tb = b.trim_start_matches('0');
    // Compare by magnitude (fewer significant digits = smaller), then lexically
    // among equal-length runs, then by the raw runs so padding stays deterministic.
    ta.len()
        .cmp(&tb.len())
        .then_with(|| ta.cmp(tb))
        .then_with(|| a.cmp(b))
}

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
mod natural_cmp_tests {
    use super::natural_cmp;
    use std::cmp::Ordering;

    #[test]
    fn numbers_sort_by_value_not_lexically() {
        assert_eq!(natural_cmp("2.png", "10.png"), Ordering::Less);
        assert_eq!(natural_cmp("10.png", "2.png"), Ordering::Greater);
    }

    #[test]
    fn mixed_text_and_numbers() {
        assert_eq!(natural_cmp("img1.png", "img2.png"), Ordering::Less);
        assert_eq!(natural_cmp("img2.png", "img10.png"), Ordering::Less);
    }

    #[test]
    fn case_insensitive_with_stable_tiebreak() {
        assert_eq!(natural_cmp("a.png", "B.png"), Ordering::Less);
        assert_eq!(natural_cmp("A.png", "a.png"), Ordering::Less);
    }

    #[test]
    fn equal_strings_are_equal() {
        assert_eq!(natural_cmp("005.png", "005.png"), Ordering::Equal);
    }

    #[test]
    fn same_value_different_padding_is_deterministic() {
        // Equal numeric value: more leading zeros sort first (stable, total order).
        assert_eq!(natural_cmp("001.png", "1.png"), Ordering::Less);
    }

    #[test]
    fn bare_numeric_strings_sort_numerically() {
        assert_eq!(natural_cmp("2", "10"), Ordering::Less);
        assert_eq!(natural_cmp("10", "2"), Ordering::Greater);
        assert_eq!(natural_cmp("7", "7"), Ordering::Equal);
    }
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
    }

    #[test]
    fn has_image_ext_is_case_insensitive() {
        assert!(has_image_ext(Path::new("photo.PNG")));
        assert!(has_image_ext(Path::new("photo.JPG")));
        assert!(has_image_ext(Path::new("photo.Jpeg")));
    }

    #[test]
    fn has_image_ext_rejects_non_images() {
        assert!(!has_image_ext(Path::new("notes.txt")));
        assert!(!has_image_ext(Path::new("archive.zip")));
        assert!(!has_image_ext(Path::new("noextension")));
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
