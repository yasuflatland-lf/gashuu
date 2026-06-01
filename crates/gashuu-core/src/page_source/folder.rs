use super::naming::{has_image_ext, natural_cmp};
use super::{PageEntry, PageSource};
use crate::error::CoreError;

use std::path::Path;
use walkdir::WalkDir;

/// A page source backed by a single directory of image files.
///
/// PR1 walks the top level only (no recursion) and orders pages by natural
/// filename comparison so `2.png` precedes `10.png`.
pub struct FolderSource {
    entries: Vec<PageEntry>,
    skipped: usize,
}

impl FolderSource {
    /// Walk `root`, collect top-level PNG/JPG/JPEG files, and sort them naturally.
    ///
    /// Directory entries that error during the walk (e.g. permission denied,
    /// broken symlinks) are counted in [`PageSource::skipped_count`] rather than
    /// silently dropped, so the presentation layer can log them.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let mut entries: Vec<PageEntry> = Vec::new();
        let mut skipped = 0usize;
        for result in WalkDir::new(root.as_ref()).min_depth(1).max_depth(1) {
            match result {
                Ok(e) if e.file_type().is_file() && has_image_ext(e.path()) => {
                    entries.push(PageEntry {
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
        self.entries.clone()
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
}
