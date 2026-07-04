use super::naming::{cap_or_reject, has_image_ext, is_macos_metadata, MAX_ENTRY_BYTES};
use super::{PageEntry, PageSource};
use crate::error::CoreError;
use crate::ordering::natural_cmp;
use std::io::BufReader;
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
    max_bytes: u64,
}

impl FolderSource {
    /// Walk `root`, collect top-level PNG/JPG/JPEG/AVIF files, and sort them naturally.
    ///
    /// Directory entries that error during the walk (e.g. permission denied,
    /// broken symlinks) are counted in [`PageSource::skipped_count`] rather than
    /// silently dropped, so the presentation layer can log them.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        Self::open_with_limit(root, MAX_ENTRY_BYTES)
    }

    /// Walk `root`, skipping image files whose on-disk size exceeds `max`.
    ///
    /// Test seam: a small `max` triggers skip+count deterministically without
    /// synthesizing a huge file.
    fn open_with_limit(root: impl AsRef<Path>, max: u64) -> Result<Self, CoreError> {
        let mut entries: Vec<FolderEntry> = Vec::new();
        let mut skipped = 0usize;
        for result in WalkDir::new(root.as_ref()).min_depth(1).max_depth(1) {
            match result {
                // macOS resource forks (`._x.jpg`) and dotfiles (`.DS_Store`)
                // carry image extensions but are filesystem metadata, not pages.
                Ok(e)
                    if e.file_type().is_file()
                        && has_image_ext(e.path())
                        && !is_macos_metadata(e.path()) =>
                {
                    match e.metadata() {
                        Ok(meta) if meta.len() <= max => {
                            entries.push(FolderEntry {
                                name: e.file_name().to_string_lossy().into_owned(),
                                path: e.path().to_path_buf(),
                            });
                        }
                        // Oversized image, or metadata unreadable: count it so the
                        // UI can surface a warning.
                        _ => skipped += 1,
                    }
                }
                // Directories and non-image files are expected, not errors.
                Ok(_) => {}
                // Unreadable entry: record it so the UI can surface a warning.
                Err(_) => skipped += 1,
            }
        }
        entries.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        Ok(Self {
            entries,
            skipped,
            max_bytes: max,
        })
    }
}

/// Read `path` whole, capping the read at `max` bytes (via the shared
/// [`cap_or_reject`]) so a file that grew past the open-time ceiling is rejected
/// rather than buffered in full.
fn read_file_capped(path: &Path, name: &str, max: u64) -> Result<Vec<u8>, CoreError> {
    let file = std::fs::File::open(path)?;
    cap_or_reject(BufReader::new(file), name, max, 0)
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
        read_file_capped(&entry.path, &entry.name, self.max_bytes)
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
        // `b.avif` reuses PNG bytes: the walk filters by extension only, never
        // decoding (same precedent as `a.jpg` here).
        for name in ["10.png", "2.png", "1.png", "a.jpg", "b.avif"] {
            write_png(&dir.path().join(name));
        }
        // Non-image files must be excluded.
        fs::write(dir.path().join("notes.txt"), b"ignore me").unwrap();

        let source = FolderSource::open(dir.path()).unwrap();
        let names: Vec<String> = source.list_pages().into_iter().map(|e| e.name).collect();

        assert_eq!(names, vec!["1.png", "2.png", "10.png", "a.jpg", "b.avif"]);
        assert_eq!(source.skipped_count(), 0);
    }

    #[test]
    fn macos_metadata_files_are_excluded() {
        let dir = tempfile::tempdir().unwrap();
        // A real page, an AppleDouble resource fork (`.jpg`-named but metadata),
        // and a `.DS_Store` dotfile.
        write_png(&dir.path().join("1.png"));
        write_png(&dir.path().join("._1.png"));
        fs::write(dir.path().join(".DS_Store"), b"dotfile").unwrap();

        let source = FolderSource::open(dir.path()).unwrap();
        let names: Vec<String> = source.list_pages().into_iter().map(|e| e.name).collect();

        assert_eq!(names, vec!["1.png"]);
        // Metadata is expected noise, not a skip.
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
        write_png(&dir.path().join("4.AVIF"));

        let source = FolderSource::open(dir.path()).unwrap();
        let names: Vec<String> = source.list_pages().into_iter().map(|e| e.name).collect();

        assert_eq!(names, vec!["1.PNG", "2.JPG", "3.Jpeg", "4.AVIF"]);
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

    #[test]
    fn image_at_limit_is_included() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.png");
        write_png(&path);
        let size = fs::metadata(&path).unwrap().len();

        let source = FolderSource::open_with_limit(dir.path(), size).unwrap();
        assert_eq!(source.list_pages().len(), 1);
        assert_eq!(source.skipped_count(), 0);
    }

    #[test]
    fn image_above_limit_is_skipped_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.png");
        write_png(&path);
        let size = fs::metadata(&path).unwrap().len();

        let source = FolderSource::open_with_limit(dir.path(), size - 1).unwrap();
        assert!(source.list_pages().is_empty());
        assert_eq!(source.skipped_count(), 1);
    }

    #[test]
    fn read_bytes_returns_entry_too_large_when_file_grows_after_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grow.png");
        write_png(&path);
        let size = fs::metadata(&path).unwrap().len();

        // Open with a limit that accepts the file as-is.
        let source = FolderSource::open_with_limit(dir.path(), size).unwrap();
        assert_eq!(source.list_pages().len(), 1);

        // Grow the file beyond the limit after open.
        let extra = vec![0u8; 2];
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        use std::io::Write;
        f.write_all(&extra).unwrap();
        drop(f);

        // read_bytes must now reject the file; verify both error fields.
        let err = source.read_bytes(0).unwrap_err();
        match err {
            CoreError::EntryTooLarge { name, max } => {
                assert_eq!(name, "grow.png");
                assert_eq!(max, size);
            }
            other => panic!("expected EntryTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn read_file_capped_boundary_is_inclusive() {
        // A file of exactly `max` bytes must succeed; `max + 1` returns EntryTooLarge. Raw
        // bytes (not a valid PNG) for precise size, with `.png` so has_image_ext passes.
        const MAX: u64 = 32;
        let dir = tempfile::tempdir().unwrap();

        let exact_path = dir.path().join("exact.png");
        fs::write(&exact_path, vec![0u8; MAX as usize]).unwrap();
        let ok = read_file_capped(&exact_path, "exact.png", MAX).unwrap();
        assert_eq!(ok.len() as u64, MAX);

        let over_path = dir.path().join("over.png");
        fs::write(&over_path, vec![0u8; MAX as usize + 1]).unwrap();
        let err = read_file_capped(&over_path, "over.png", MAX).unwrap_err();
        match err {
            CoreError::EntryTooLarge { name, max } => {
                assert_eq!(name, "over.png");
                assert_eq!(max, MAX);
            }
            other => panic!("expected EntryTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn multiple_oversized_images_accumulate_skipped_count() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.png", "b.png", "c.png"] {
            write_png(&dir.path().join(name));
        }
        // Use limit 1 so all three images are oversized.
        let source = FolderSource::open_with_limit(dir.path(), 1).unwrap();
        assert!(source.list_pages().is_empty());
        assert_eq!(source.skipped_count(), 3);
    }

    #[test]
    fn oversized_non_image_does_not_increment_skipped() {
        let dir = tempfile::tempdir().unwrap();
        // A non-image file: must never affect skipped_count regardless of size.
        fs::write(dir.path().join("big.txt"), vec![0u8; 100]).unwrap();
        write_png(&dir.path().join("1.png"));

        // limit=1 means the image is oversized (skipped, count=1); big.txt is
        // not an image and must not contribute to the count (stays at 1, not 2).
        let source = FolderSource::open_with_limit(dir.path(), 1).unwrap();
        assert_eq!(source.skipped_count(), 1);
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
                "read_bytes({i}) returned bytes for the wrong file (expected {:?})",
                entry.name,
            );
        }
    }
}
