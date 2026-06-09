//! A `PageSource` backed by a ZIP/CBZ archive. Lock-free across reads (each
//! `read_bytes` opens its own file + `ZipArchive`), so rayon prefetch threads
//! decompress entries fully in parallel.
//!
//! `ZipSource` holds NO archive-wide buffer: each `read_bytes` allocates only
//! the one entry it reads. Under concurrent rayon prefetch, multiple reads can
//! be in flight at once, each holding its own independent buffer (so resident
//! memory is not bounded to a single page while prefetch is running).
//!
//! The external `zip` crate is referenced as `::zip::` throughout because this
//! local module is also named `zip`; an unqualified `zip::` would resolve to
//! this module, not the crate.

use super::naming::{has_image_ext, is_macos_metadata, MAX_ENTRY_BYTES};
use super::{PageEntry, PageSource};
use crate::error::CoreError;
use crate::ordering::natural_cmp;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

/// Metadata for one image page resolved from the archive's central directory.
struct EntryMeta {
    /// Index into the archive's central directory for `by_index`.
    zip_index: usize,
    /// Safe display name resolved via `enclosed_name` (zip-slip rejected).
    /// Holds the full flattened path (e.g. `sub/3.png`), which is also the
    /// natural-sort key, so nested pages order intuitively against top-level ones.
    name: String,
}

/// A `PageSource` over a ZIP/CBZ file: the archive is scanned once at `open`
/// to build a stable, naturally ordered list of image entries; each read
/// re-opens the file so reads never contend on a shared handle.
// Intentionally does NOT derive `Debug` (tests rely on this; matches `FolderSource`).
pub struct ZipSource {
    path: PathBuf,
    entries: Vec<EntryMeta>,
    skipped: usize,
}

impl ZipSource {
    /// Open an archive and index its image pages with the default size ceiling.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CoreError> {
        Self::open_with_limit(path, MAX_ENTRY_BYTES)
    }

    /// Open an archive, skipping entries whose declared uncompressed size
    /// exceeds `max`.
    ///
    /// Test seam: a small `max` triggers skip+count deterministically without
    /// synthesizing a 500 MB entry.
    fn open_with_limit(path: impl AsRef<Path>, max: u64) -> Result<Self, CoreError> {
        let path = path.as_ref().to_path_buf();
        let file = std::fs::File::open(&path)?;
        let mut archive = ::zip::ZipArchive::new(BufReader::new(file))?;
        let mut entries = Vec::new();
        let mut skipped = 0usize;
        for i in 0..archive.len() {
            let entry = match archive.by_index(i) {
                Ok(e) => e,
                Err(_) => {
                    // A corrupt central-directory entry shouldn't doom the whole
                    // archive; skip it and surface the count (consistent with the
                    // zip-slip / oversized skip policy). core stays logging-free.
                    skipped += 1;
                    continue;
                }
            };
            // `enclosed_name` == None => the name escapes the base dir (zip-slip),
            // is absolute, or otherwise unsafe to materialize as a path.
            let Some(safe) = entry.enclosed_name() else {
                if has_image_ext(Path::new(entry.name())) {
                    // Surface only image-looking malicious entries as skips; a
                    // stray unsafe non-image (e.g. metadata) is not user-visible.
                    skipped += 1;
                }
                continue;
            };
            // Flatten: any image entry at any depth is a page (CBZs often wrap
            // pages in a folder, unlike FolderSource's max_depth(1)).
            if entry.is_dir() || !has_image_ext(&safe) {
                continue; // dirs / non-images are expected, not skips
            }
            // macOS resource forks (`__MACOSX/.../._x.jpg`) and dotfiles carry
            // image extensions and sort ahead of real pages via case-insensitive
            // ordering, so they can masquerade as page 0. Treat them as expected
            // noise like directories — drop without counting as a skip.
            if is_macos_metadata(&safe) {
                continue;
            }
            if entry.size() > max {
                skipped += 1;
                continue;
            }
            entries.push(EntryMeta {
                zip_index: i,
                name: safe.to_string_lossy().into_owned(),
            });
        }
        // Sort by the full flattened name with natural (digit-aware) ordering so
        // `2.png` precedes `10.png`.
        entries.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        Ok(Self {
            path,
            entries,
            skipped,
        })
    }

    /// Read the bytes of page `index`, capping the read at `max` uncompressed
    /// bytes regardless of the entry's declared size (defends against a header
    /// that lies about its size).
    fn read_entry(&self, index: usize, max: u64) -> Result<Vec<u8>, CoreError> {
        let meta = self.entries.get(index).ok_or(CoreError::IndexOutOfRange {
            index,
            len: self.entries.len(),
        })?;
        let file = std::fs::File::open(&self.path)?;
        let mut archive = ::zip::ZipArchive::new(BufReader::new(file))?;
        let mut entry = archive.by_index(meta.zip_index)?;
        // Pre-size the buffer to the smaller of the declared size and the cap
        // purely as a growth hint to avoid reallocations. This is NOT the size
        // defense: `open_with_limit` already skipped any entry whose declared
        // `size()` exceeds `max`, so by the time we read, the only remaining
        // threat is a header that lies about its size. That is caught below by
        // the `take(max + 1)` truncation plus the `buf.len() > max` check — the
        // actual security cap.
        let cap = entry.size().min(max) as usize;
        let mut buf = Vec::with_capacity(cap);
        // Read at most max+1 so an over-limit (spoofed) entry is detectable: if
        // we land on exactly max+1 bytes the real size exceeds the ceiling.
        entry.by_ref().take(max + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > max {
            return Err(CoreError::EntryTooLarge {
                name: meta.name.clone(),
                max,
            });
        }
        Ok(buf)
    }
}

impl PageSource for ZipSource {
    fn list_pages(&self) -> Vec<PageEntry> {
        self.entries
            .iter()
            .map(|m| PageEntry {
                // ZIP entries have no filesystem path; the flattened entry name is
                // the page's identity. Bytes are retrieved via `read_bytes(index)`.
                name: m.name.clone(),
            })
            .collect()
    }

    fn read_bytes(&self, index: usize) -> Result<Vec<u8>, CoreError> {
        self.read_entry(index, MAX_ENTRY_BYTES)
    }

    fn skipped_count(&self) -> usize {
        self.skipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use tempfile::NamedTempFile;

    /// Encode a 2x2 RGBA PNG into a byte vector for use as a fixture page.
    fn tiny_png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    /// Write a CBZ (ZIP) tempfile from `(name, bytes)` entries using `Stored`
    /// compression so byte sizes are predictable and no deflate feature is
    /// required. Returns the open temp file (kept alive by the caller so the
    /// path stays valid).
    fn write_cbz(entries: &[(&str, Vec<u8>)]) -> NamedTempFile {
        let tmp = NamedTempFile::new().unwrap();
        // Reopen the underlying file handle for writing without consuming the
        // NamedTempFile (so it isn't deleted while we still hold the path).
        let file = tmp.reopen().unwrap();
        let mut writer = ::zip::ZipWriter::new(file);
        let options = ::zip::write::SimpleFileOptions::default()
            .compression_method(::zip::CompressionMethod::Stored);
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        tmp
    }

    fn names(src: &ZipSource) -> Vec<String> {
        src.list_pages().into_iter().map(|p| p.name).collect()
    }

    /// Test helper: open an archive indexing EVERY non-dir, in-bounds entry
    /// regardless of extension, so size-boundary tests can use plain `.bin`
    /// fixtures with exact byte counts (Stored compression). Mirrors
    /// `open_with_limit` but without the image-extension filter.
    fn open_all_entries(path: &Path) -> ZipSource {
        let file = std::fs::File::open(path).unwrap();
        let mut archive = ::zip::ZipArchive::new(BufReader::new(file)).unwrap();
        let mut entries = Vec::new();
        for i in 0..archive.len() {
            let entry = archive.by_index(i).unwrap();
            let Some(safe) = entry.enclosed_name() else {
                continue;
            };
            if entry.is_dir() {
                continue;
            }
            entries.push(EntryMeta {
                zip_index: i,
                name: safe.to_string_lossy().into_owned(),
            });
        }
        entries.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        ZipSource {
            path: path.to_path_buf(),
            entries,
            skipped: 0,
        }
    }

    #[test]
    fn lists_images_only_flattened_in_natural_order() {
        let png = tiny_png();
        let cbz = write_cbz(&[
            ("10.png", png.clone()),
            ("2.png", png.clone()),
            ("1.png", png.clone()),
            ("a.jpg", png.clone()),
            ("notes.txt", b"hello".to_vec()),
            ("sub/3.png", png.clone()),
        ]);
        let src = ZipSource::open(cbz.path()).unwrap();

        // Build the expected order by applying the real `natural_cmp` to the
        // surviving image names, so the test pins "natural order" rather than a
        // hardcoded guess about how the `sub/` prefix sorts. `notes.txt` is
        // excluded (not an image); `sub/3.png` is kept (flattening).
        let mut expected = vec!["1.png", "2.png", "10.png", "a.jpg", "sub/3.png"];
        expected.sort_by(|a, b| natural_cmp(a, b));
        let expected: Vec<String> = expected.into_iter().map(String::from).collect();

        assert_eq!(names(&src), expected);
        assert_eq!(src.skipped_count(), 0);
        // Sanity: notes.txt never appears regardless of sort order.
        assert!(!names(&src).iter().any(|n| n == "notes.txt"));
        // Sanity: the nested image survives flattening.
        assert!(names(&src).iter().any(|n| n == "sub/3.png"));
    }

    #[test]
    fn avif_entries_are_indexed_as_pages() {
        // Open-time filtering is extension-only (bytes are never decoded at
        // open), so PNG bytes under an `.avif` name suffice — the same
        // precedent as the `a.jpg` fixture above.
        let cbz = write_cbz(&[("1.avif", tiny_png()), ("notes.txt", b"x".to_vec())]);
        let src = ZipSource::open(cbz.path()).unwrap();

        assert_eq!(names(&src), vec!["1.avif".to_string()]);
        assert_eq!(src.skipped_count(), 0);
    }

    #[test]
    fn macos_metadata_entries_are_excluded_without_counting() {
        let png = tiny_png();
        // The AppleDouble carries a `.jpg` name and (via case-insensitive
        // natural ordering) sorts AHEAD of `Manga/001.jpg`, so before the fix it
        // would become page 0 and fail to decode.
        let cbz = write_cbz(&[
            ("__MACOSX/Manga/._001.jpg", b"AppleDouble noise".to_vec()),
            ("Manga/001.jpg", png.clone()),
            ("Manga/002.jpg", png.clone()),
        ]);
        let src = ZipSource::open(cbz.path()).unwrap();

        let listed = names(&src);
        // Only the real images survive, in natural order.
        assert_eq!(
            listed,
            vec!["Manga/001.jpg".to_string(), "Manga/002.jpg".to_string()]
        );
        // The AppleDouble must be entirely absent from the page list.
        assert!(!listed.iter().any(|n| n.contains("__MACOSX")));
        assert!(!listed.iter().any(|n| n.contains("._")));
        // Page 0 is the real first page, not the resource fork.
        assert_eq!(listed[0], "Manga/001.jpg");
        // Metadata is expected noise, not a skip.
        assert_eq!(src.skipped_count(), 0);
    }

    #[test]
    fn zip_slip_entry_is_rejected_and_counted() {
        let png = tiny_png();
        let cbz = write_cbz(&[("1.png", png.clone()), ("../evil.png", png.clone())]);
        let src = ZipSource::open(cbz.path()).unwrap();

        let listed = names(&src);
        assert!(!listed.iter().any(|n| n.contains("evil")));
        assert_eq!(listed, vec!["1.png".to_string()]);
        // The fixture has exactly one unsafe image entry.
        assert_eq!(src.skipped_count(), 1);
    }

    #[test]
    fn unsafe_non_image_entry_is_not_counted_as_skipped() {
        // Only image-looking malicious entries inflate `skipped_count()`. A
        // zip-slip entry that is NOT an image (e.g. a stray text file) is
        // expected noise: it is silently ignored, not surfaced as a skip.
        let png = tiny_png();
        let cbz = write_cbz(&[("1.png", png), ("../secrets.txt", b"top secret".to_vec())]);
        let src = ZipSource::open(cbz.path()).unwrap();

        assert_eq!(names(&src), vec!["1.png".to_string()]);
        assert_eq!(src.skipped_count(), 0);
    }

    #[test]
    fn open_size_boundary_is_inclusive() {
        // Open-time mirror of `read_entry_size_boundary_is_inclusive`: an image
        // entry of EXACTLY `max` declared bytes is kept (the check is `> max`),
        // while `max + 1` is skipped. Use `.png`-named Stored entries padded to
        // a predictable byte length so the entry passes the image filter and the
        // ONLY reason to skip is the declared-size tier. With Stored compression
        // the entry's declared `size()` equals the buffer length.
        const MAX: u64 = 64;

        // Exactly `max` bytes → kept, not counted as skipped.
        let exact_cbz = write_cbz(&[("1.png", vec![0u8; MAX as usize])]);
        let exact = ZipSource::open_with_limit(exact_cbz.path(), MAX).unwrap();
        assert_eq!(exact.list_pages().len(), 1);
        assert_eq!(exact.skipped_count(), 0);

        // `max + 1` bytes → skipped at open.
        let over_cbz = write_cbz(&[("1.png", vec![0u8; MAX as usize + 1])]);
        let over = ZipSource::open_with_limit(over_cbz.path(), MAX).unwrap();
        assert!(over.list_pages().is_empty());
        assert_eq!(over.skipped_count(), 1);
    }

    #[test]
    fn read_bytes_round_trips_and_decodes() {
        let png = tiny_png();
        let cbz = write_cbz(&[("1.png", png.clone())]);
        let src = ZipSource::open(cbz.path()).unwrap();

        let bytes = src.read_bytes(0).unwrap();
        assert_eq!(bytes, png, "stored entry must round-trip byte-for-byte");
        // The bytes must remain decodable through the normal decode path.
        let decoded = crate::image_ops::decode(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
    }

    #[test]
    fn read_bytes_out_of_range_errors() {
        let png = tiny_png();
        let cbz = write_cbz(&[("1.png", png)]);
        let src = ZipSource::open(cbz.path()).unwrap();

        let err = src.read_bytes(99).unwrap_err();
        assert!(matches!(
            err,
            CoreError::IndexOutOfRange { index: 99, len: 1 }
        ));
    }

    #[test]
    fn open_skips_entry_over_declared_size_limit() {
        // A PNG is comfortably larger than 10 bytes, so a 10-byte ceiling at
        // open time skips it (declared-size tier).
        let png = tiny_png();
        assert!(png.len() as u64 > 10, "fixture must exceed the test limit");
        let cbz = write_cbz(&[("big.png", png)]);

        let src = ZipSource::open_with_limit(cbz.path(), 10).unwrap();
        assert!(src.list_pages().is_empty());
        assert_eq!(src.skipped_count(), 1);
    }

    #[test]
    fn read_entry_over_actual_size_limit_errors() {
        // Open with a generous limit so the entry passes open-time indexing,
        // then read it through a tiny per-read cap to exercise the read-time
        // actual-byte tier independently.
        let png = tiny_png();
        assert!(png.len() as u64 > 10);
        let cbz = write_cbz(&[("p.png", png)]);

        let src = ZipSource::open(cbz.path()).unwrap();
        assert_eq!(src.list_pages().len(), 1);

        let err = src.read_entry(0, 10).unwrap_err();
        match err {
            CoreError::EntryTooLarge { name, max } => {
                assert_eq!(name, "p.png");
                assert_eq!(max, 10);
            }
            other => panic!("expected EntryTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn read_entry_size_boundary_is_inclusive() {
        // An entry of exactly `max` bytes must read OK; `max + 1` must error.
        // Stored compression keeps the uncompressed size equal to our input
        // length, so we can pad to an exact byte count. We use non-image `.bin`
        // entries and index every entry via `open_all_entries` so we can target
        // both the exact and over-limit fixtures by page index.
        const MAX: u64 = 32;
        let exact = vec![7u8; MAX as usize];
        let over = vec![7u8; MAX as usize + 1];
        let cbz = write_cbz(&[("exact.bin", exact.clone()), ("over.bin", over)]);

        let src = open_all_entries(cbz.path());
        // entries are sorted by natural name: "exact.bin" < "over.bin".
        assert_eq!(names(&src), vec!["exact.bin", "over.bin"]);

        let ok = src.read_entry(0, MAX).unwrap();
        assert_eq!(ok.len() as u64, MAX);

        let err = src.read_entry(1, MAX).unwrap_err();
        assert!(matches!(err, CoreError::EntryTooLarge { max: MAX, .. }));
    }

    #[test]
    fn open_corrupt_archive_errors() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"this is not a zip archive at all").unwrap();
        tmp.flush().unwrap();

        // `ZipSource` does not implement `Debug`, so `unwrap_err` is unavailable;
        // match on the result to extract the error instead.
        let Err(err) = ZipSource::open(tmp.path()) else {
            panic!("expected a Zip error opening a corrupt archive");
        };
        assert!(matches!(err, CoreError::Zip(_)));
    }

    #[test]
    fn open_missing_file_errors_as_io() {
        // `ZipSource` does not implement `Debug`, so `unwrap_err` is unavailable;
        // match on the result to extract the error instead.
        let Err(err) = ZipSource::open("/nonexistent/does-not-exist.cbz") else {
            panic!("expected an I/O error opening a missing file");
        };
        assert!(matches!(err, CoreError::Io(_)));
    }
}
