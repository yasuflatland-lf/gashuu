use super::naming::{has_image_ext, natural_cmp};
use super::{PageEntry, PageSource};
use crate::error::CoreError;

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Internal pairing of a page's real filesystem path with its display name.
///
/// The path is an implementation detail of `FolderSource` (used by `read_bytes`)
/// and is deliberately NOT part of the public [`PageEntry`], which carries the
/// name only — archive sources have no path to expose.
struct FolderEntry {
    path: PathBuf,
    name: String,
}

/// A page source backed by a single directory of image files.
///
/// PR1 walks the top level only (no recursion) and orders pages by natural
/// filename comparison so `2.png` precedes `10.png`.
pub struct FolderSource {
    entries: Vec<FolderEntry>,
    skipped: usize,
}

impl FolderSource {
    /// Walk `root`, collect top-level PNG/JPG/JPEG files, and sort them naturally.
    ///
    /// Directory entries that error during the walk (e.g. permission denied,
    /// broken symlinks) are counted in [`PageSource::skipped_count`] rather than
    /// silently dropped, so the presentation layer can log them.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let mut entries: Vec<FolderEntry> = Vec::new();
        let mut skipped = 0usize;
        for result in WalkDir::new(root.as_ref()).min_depth(1).max_depth(1) {
            match result {
                Ok(e) if e.file_type().is_file() && has_image_ext(e.path()) => {
                    entries.push(FolderEntry {
                        name: e.file_name().to_string_lossy().into_owned(),
                        path: e.path().to_path_buf(),
                    });
                }
                // Directories and non-image files are expected, not errors.
                Ok(_) => {}
                // Unreadable entry: record it so the UI can surface a warning.
                Err(_) => skipped += 1,
            }
        }
        entries.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        Ok(Self { entries, skipped })
    }
}

impl PageSource for FolderSource {
    fn list_pages(&self) -> Vec<PageEntry> {
        self.entries
            .iter()
            .map(|e| PageEntry {
                name: e.name.clone(),
            })
            .collect()
    }

    fn read_bytes(&self, index: usize) -> Result<Vec<u8>, CoreError> {
        let entry = self.entries.get(index).ok_or(CoreError::IndexOutOfRange {
            index,
            len: self.entries.len(),
        })?;
        std::fs::read(&entry.path).map_err(CoreError::from)
    }

    /// Number of directory entries that could not be read during `open`.
    fn skipped_count(&self) -> usize {
        self.skipped
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
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
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
        assert_eq!(source.skipped_count(), 0);
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

        assert!(matches!(
            err,
            CoreError::IndexOutOfRange { index: 7, len: 1 }
        ));
    }

    #[test]
    fn empty_folder_lists_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let source = FolderSource::open(dir.path()).unwrap();
        assert!(source.list_pages().is_empty());
    }

    #[test]
    fn includes_uppercase_extensions() {
        let dir = tempfile::tempdir().unwrap();
        write_png(&dir.path().join("1.PNG"));
        write_png(&dir.path().join("2.JPG"));
        write_png(&dir.path().join("3.Jpeg"));

        let source = FolderSource::open(dir.path()).unwrap();
        let names: Vec<String> = source.list_pages().into_iter().map(|e| e.name).collect();

        assert_eq!(names, vec!["1.PNG", "2.JPG", "3.Jpeg"]);
    }

    #[test]
    fn subdirectory_images_are_excluded() {
        let dir = tempfile::tempdir().unwrap();
        write_png(&dir.path().join("1.png"));
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        write_png(&sub.join("2.png"));

        let source = FolderSource::open(dir.path()).unwrap();
        let names: Vec<String> = source.list_pages().into_iter().map(|e| e.name).collect();

        assert_eq!(names, vec!["1.png"]);
    }

    /// Pins the load-bearing invariant of the refactor: after the natural-sort,
    /// `read_bytes(i)` returns the bytes of the file whose `name` sits at position
    /// `i` in `list_pages()`. WalkDir enumeration order is filesystem-dependent and
    /// differs from sorted order, so this test uses three files (`"2.png"`,
    /// `"10.png"`, `"1.png"`) whose natural order (`1, 2, 10`) diverges from their
    /// creation order. Each file has DISTINCT dimensions so identical bytes cannot
    /// mask a path/name pairing bug. Guards against a future list_pages filter or
    /// reorder that drifts out of sync with the internal `FolderEntry` ordering.
    #[test]
    fn read_bytes_matches_list_pages_after_natural_sort() {
        // Write a tiny PNG with a unique pixel dimension for each file so the encoded
        // bytes differ and a wrong-index read will produce a mismatch.
        fn write_sized_png(path: &std::path::Path, w: u32, h: u32) {
            let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
            let mut bytes = Vec::new();
            img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
                .unwrap();
            fs::write(path, bytes).unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        // Create files in a non-natural order so WalkDir enumeration ≠ sorted order.
        write_sized_png(&dir.path().join("2.png"), 2, 2);
        write_sized_png(&dir.path().join("10.png"), 10, 10);
        write_sized_png(&dir.path().join("1.png"), 1, 1);

        let source = FolderSource::open(dir.path()).unwrap();
        let pages = source.list_pages();

        // Confirm the natural-sort order first.
        let names: Vec<&str> = pages.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["1.png", "2.png", "10.png"]);

        // For every position i, read_bytes(i) must equal the bytes of the file
        // identified by pages[i].name — this is the core path↔name pairing contract.
        for (i, entry) in pages.iter().enumerate() {
            let expected = fs::read(dir.path().join(&entry.name)).unwrap();
            let actual = source.read_bytes(i).unwrap();
            assert_eq!(
                actual, expected,
                "read_bytes({i}) returned bytes for the wrong file (got {:?}, want {:?})",
                entry.name, pages[i].name,
            );
        }
    }
}
