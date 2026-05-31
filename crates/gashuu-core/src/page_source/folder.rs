use super::{PageEntry, PageSource};
use crate::error::CoreError;

use std::cmp::Ordering;
use std::iter::Peekable;
use std::path::Path;
use std::str::Chars;
use walkdir::WalkDir;

/// Compare two file names in natural order so embedded numbers sort by numeric
/// value (`2.png` < `10.png`). Non-digit runs compare case-insensitively (ASCII)
/// with the raw chars as a stable tiebreaker, giving a total order.
fn natural_cmp(a: &str, b: &str) -> Ordering {
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
                    match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                        Ordering::Equal => match ca.cmp(&cb) {
                            Ordering::Equal => {
                                ai.next();
                                bi.next();
                            }
                            ord => return ord,
                        },
                        ord => return ord,
                    }
                }
            }
        }
    }
}

/// Consume and return the maximal leading run of ASCII digits.
fn take_digits(it: &mut Peekable<Chars<'_>>) -> String {
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
fn cmp_numeric(a: &str, b: &str) -> Ordering {
    let ta = a.trim_start_matches('0');
    let tb = b.trim_start_matches('0');
    match ta.len().cmp(&tb.len()) {
        Ordering::Equal => match ta.cmp(tb) {
            Ordering::Equal => a.cmp(b),
            ord => ord,
        },
        ord => ord,
    }
}

/// Image extensions recognized in PR1 (case-insensitive).
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg"];

/// A page source backed by a single directory of image files.
///
/// PR1 walks the top level only (no recursion) and orders pages by natural
/// filename comparison so `2.png` precedes `10.png`.
pub struct FolderSource {
    entries: Vec<PageEntry>,
}

impl FolderSource {
    /// Walk `root`, collect top-level PNG/JPG files, and sort them naturally.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let mut entries: Vec<PageEntry> = WalkDir::new(root.as_ref())
            .min_depth(1)
            .max_depth(1)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file() && has_image_ext(e.path()))
            .map(|e| PageEntry {
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.path().to_path_buf(),
            })
            .collect();
        entries.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        Ok(Self { entries })
    }
}

/// True when `path` has a recognized image extension (ASCII case-insensitive).
fn has_image_ext(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(ext) if IMAGE_EXTS.iter().any(|known| ext.eq_ignore_ascii_case(known))
    )
}

impl PageSource for FolderSource {
    fn list_pages(&self) -> Vec<PageEntry> {
        self.entries.clone()
    }

    fn read_bytes(&self, index: usize) -> Result<Vec<u8>, CoreError> {
        let entry = self
            .entries
            .get(index)
            .ok_or(CoreError::IndexOutOfRange { index, len: self.entries.len() })?;
        std::fs::read(&entry.path).map_err(CoreError::from)
    }
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
}

#[cfg(test)]
mod folder_source_tests {
    use super::*;
    use crate::error::CoreError;
    use std::fs;
    use std::io::Cursor;

    /// Write a tiny valid PNG so reads/decodes have real content.
    fn write_png(path: &std::path::Path) {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn lists_only_images_in_natural_order() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["10.png", "2.png", "1.png", "a.jpg"] {
            write_png(&dir.path().join(name));
        }
        // Non-image files must be excluded.
        fs::write(dir.path().join("notes.txt"), b"ignore me").unwrap();

        let source = FolderSource::open(dir.path()).unwrap();
        let names: Vec<String> = source.list_pages().into_iter().map(|e| e.name).collect();

        assert_eq!(names, vec!["1.png", "2.png", "10.png", "a.jpg"]);
    }

    #[test]
    fn read_bytes_returns_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.png");
        write_png(&path);

        let source = FolderSource::open(dir.path()).unwrap();
        let bytes = source.read_bytes(0).unwrap();

        assert_eq!(bytes, fs::read(&path).unwrap());
        assert!(!bytes.is_empty());
    }

    #[test]
    fn read_bytes_out_of_range_errors() {
        let dir = tempfile::tempdir().unwrap();
        write_png(&dir.path().join("1.png"));

        let source = FolderSource::open(dir.path()).unwrap();
        let err = source.read_bytes(7).unwrap_err();

        assert!(matches!(err, CoreError::IndexOutOfRange { index: 7, len: 1 }));
    }

    #[test]
    fn empty_folder_lists_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let source = FolderSource::open(dir.path()).unwrap();
        assert!(source.list_pages().is_empty());
    }
}
