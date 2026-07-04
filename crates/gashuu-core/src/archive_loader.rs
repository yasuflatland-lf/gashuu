//! Dispatch a path to the right `PageSource`: directory -> FolderSource,
//! CBZ/ZIP (by extension, else magic bytes) -> ZipSource,
//! CBR/RAR (by extension, else magic bytes) -> RarSource.

use crate::error::CoreError;
use crate::page_source::{FolderSource, PageSource, RarSource, ZipSource};
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

/// Discriminator returned by the extension and magic probes.
#[cfg_attr(test, derive(Debug, PartialEq))]
enum Kind {
    Zip,
    Rar,
}

/// Policy controlling which archive formats `ArchiveLoader` is permitted to open.
///
/// The default (`allow_rar: true`) preserves backward-compatible behavior.
/// Set `allow_rar: false` to reject RAR/CBR archives and return
/// [`CoreError::FormatDisabled`] instead of opening [`RarSource`].
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

/// Opens any supported source by path, returning a type-erased `Arc<dyn PageSource>`.
///
/// Resolution order:
/// 1. If the path is a directory: [`FolderSource`].
/// 2. Extension check (cheap, no I/O): `cbz`/`zip` → [`ZipSource`]; `cbr`/`rar` → [`RarSource`].
/// 3. Magic-byte sniff (first bytes of the file): ZIP signatures → [`ZipSource`]; RAR prefix → [`RarSource`].
/// 4. Otherwise: [`CoreError::UnsupportedFormat`].
pub struct ArchiveLoader;

impl ArchiveLoader {
    /// Count the displayable image pages in `path`, enforcing the domain rule that
    /// a valid book has at least one image page.
    ///
    /// Returns `Ok(NonZeroUsize)` when one or more image pages are found.
    /// Returns `Err(CoreError::EmptyBook)` when the source opens successfully but
    /// contains zero image pages — an empty folder, a zip with only non-image
    /// entries, etc.
    ///
    /// I/O errors and `UnsupportedFormat` propagate unchanged. "Empty" and
    /// "unreadable" are strictly distinct: an unreadable source (I/O failure,
    /// corrupt archive header, unsupported file type) is never classified as empty.
    pub fn probe_page_count(path: impl AsRef<Path>) -> Result<NonZeroUsize, CoreError> {
        Self::probe_page_count_with_policy(path, ArchivePolicy::default())
    }

    /// Like [`probe_page_count`] but respects `policy`.
    pub fn probe_page_count_with_policy(
        path: impl AsRef<Path>,
        policy: ArchivePolicy,
    ) -> Result<NonZeroUsize, CoreError> {
        let path = path.as_ref();
        let source = Self::open_with_policy(path, policy)?;
        let count = source.list_pages().len();
        NonZeroUsize::new(count).ok_or_else(|| CoreError::EmptyBook {
            path: path.display().to_string(),
        })
    }

    /// Open `path` as the most appropriate [`PageSource`].
    ///
    /// Returns `Err(CoreError::UnsupportedFormat)` when the path is a file that
    /// is neither a recognized archive extension nor matches ZIP/RAR magic bytes.
    /// Equivalent to `open_with_policy(path, ArchivePolicy::default())`.
    pub fn open(path: impl AsRef<Path>) -> Result<Arc<dyn PageSource>, CoreError> {
        Self::open_with_policy(path, ArchivePolicy::default())
    }

    /// Open `path` as the most appropriate [`PageSource`], respecting `policy`.
    ///
    /// When the resolved kind is RAR and `policy.allow_rar` is `false`, returns
    /// `Err(CoreError::FormatDisabled)` instead of opening [`RarSource`].
    pub fn open_with_policy(
        path: impl AsRef<Path>,
        policy: ArchivePolicy,
    ) -> Result<Arc<dyn PageSource>, CoreError> {
        let path = path.as_ref();
        if path.is_dir() {
            return Ok(Arc::new(FolderSource::open(path)?));
        }
        let kind = match ext_kind(path) {
            Some(k) => Some(k),
            None => magic_kind(path)?,
        };
        match kind {
            Some(Kind::Zip) => Ok(Arc::new(ZipSource::open(path)?)),
            Some(Kind::Rar) if !policy.allow_rar => {
                Err(CoreError::FormatDisabled { format: "rar/cbr" })
            }
            Some(Kind::Rar) => Ok(Arc::new(RarSource::open(path)?)),
            None => Err(CoreError::UnsupportedFormat {
                path: path.display().to_string(),
            }),
        }
    }
}

/// Classify `path` by its file extension alone (no I/O).
///
/// Returns `None` when the extension is absent or not a recognised archive type.
fn ext_kind(path: &Path) -> Option<Kind> {
    let ext = path.extension().and_then(|e| e.to_str())?;
    if ext.eq_ignore_ascii_case("cbz") || ext.eq_ignore_ascii_case("zip") {
        Some(Kind::Zip)
    } else if ext.eq_ignore_ascii_case("cbr") || ext.eq_ignore_ascii_case("rar") {
        Some(Kind::Rar)
    } else {
        None
    }
}

/// Classify `path` by reading its leading magic bytes (fallback when extension is unknown).
///
/// Does ONE bounded `read` into a 6-byte buffer (`RAR_MAGIC` length, the longest
/// magic tested). A file shorter than the buffer is NOT an error: a single
/// `Read::read` returns `Ok(n)` with a small `n`, and the `filled.len() >= 4` /
/// `>= RAR_MAGIC.len()` length guards below treat too-few-bytes as "no match"
/// (`None`). Only a genuine I/O error propagates.
///
/// ZIP signatures are 4 bytes (`ZIP_MAGICS`); RAR uses the 6-byte `RAR_MAGIC` prefix.
/// The buffer is sized to the larger of the two so a single read covers both checks.
fn magic_kind(path: &Path) -> Result<Option<Kind>, CoreError> {
    // Buffer sized to cover the longest magic we test (RAR_MAGIC = 6 bytes).
    let mut head = [0u8; 6];
    let mut f = std::fs::File::open(path)?;
    // A single `read` never returns `UnexpectedEof` for a short file — it just
    // yields a small `n`, which the length guards below treat as "no match".
    let n = f.read(&mut head)?;
    let filled = &head[..n];
    // ZIP: 4-byte signatures — check first.
    if filled.len() >= 4 && ZIP_MAGICS.iter().any(|m| *m == &filled[..4]) {
        return Ok(Some(Kind::Zip));
    }
    // RAR: 6-byte prefix (`Rar!\x1A\x07`).
    if filled.len() >= RAR_MAGIC.len() && filled[..RAR_MAGIC.len()] == *RAR_MAGIC {
        return Ok(Some(Kind::Rar));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{write_cbr_with_suffix, SAMPLE_CBR_B64};
    use image::RgbaImage;
    use std::io::{Cursor, Write};

    /// Build a minimal valid 2x2 PNG in memory.
    fn tiny_png() -> Vec<u8> {
        let img = RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode 2x2 PNG");
        buf
    }

    /// Build a ZIP archive in memory containing a single `page.png`.
    fn tiny_zip_bytes() -> Vec<u8> {
        use zip::write::SimpleFileOptions;
        use zip::{CompressionMethod, ZipWriter};

        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut zw = ZipWriter::new(cursor);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("page.png", opts).expect("start_file");
            zw.write_all(&tiny_png()).expect("write png into zip");
            // finish() consumes zw and returns the underlying writer (cursor).
            zw.finish().expect("finish zip");
        }
        buf
    }

    #[test]
    fn ext_kind_cbr_lowercase_is_rar() {
        assert_eq!(ext_kind(Path::new("x.cbr")), Some(Kind::Rar));
    }

    #[test]
    fn ext_kind_rar_lowercase_is_rar() {
        assert_eq!(ext_kind(Path::new("x.rar")), Some(Kind::Rar));
    }

    #[test]
    fn ext_kind_cbr_uppercase_is_rar() {
        assert_eq!(ext_kind(Path::new("x.CBR")), Some(Kind::Rar));
    }

    #[test]
    fn ext_kind_cbz_lowercase_is_zip() {
        assert_eq!(ext_kind(Path::new("x.cbz")), Some(Kind::Zip));
    }

    #[test]
    fn ext_kind_zip_lowercase_is_zip() {
        assert_eq!(ext_kind(Path::new("x.zip")), Some(Kind::Zip));
    }

    #[test]
    fn ext_kind_txt_is_none() {
        assert_eq!(ext_kind(Path::new("x.txt")), None);
    }

    #[test]
    fn ext_kind_no_extension_is_none() {
        assert_eq!(ext_kind(Path::new("noext")), None);
    }

    #[test]
    fn magic_kind_rar_bytes_in_txt_file_resolves_to_rar() {
        // A file whose content starts with the RAR magic but whose extension is
        // `.txt` must resolve to `Kind::Rar` through the magic-byte path.
        let mut f = tempfile::Builder::new()
            .suffix(".txt")
            .tempfile()
            .expect("tempfile");
        // Write RAR4 magic prefix followed by padding bytes.
        f.write_all(b"Rar!\x1A\x07\x00\x00\x00\x00")
            .expect("write rar magic");
        f.flush().expect("flush");

        assert_eq!(
            magic_kind(f.path()).expect("magic_kind must not error"),
            Some(Kind::Rar)
        );
    }

    #[test]
    fn magic_kind_zip_bytes_in_txt_file_resolves_to_zip() {
        // A file whose content starts with `PK\x03\x04` (ZIP local-file header)
        // and whose extension is `.txt` must resolve to `Kind::Zip`.
        let mut f = tempfile::Builder::new()
            .suffix(".txt")
            .tempfile()
            .expect("tempfile");
        f.write_all(&tiny_zip_bytes()).expect("write zip bytes");
        f.flush().expect("flush");

        assert_eq!(
            magic_kind(f.path()).expect("magic_kind must not error"),
            Some(Kind::Zip)
        );
    }

    #[test]
    fn magic_kind_plaintext_is_none() {
        let mut f = tempfile::Builder::new()
            .suffix(".bin")
            .tempfile()
            .expect("tempfile");
        f.write_all(b"RIFF\x00\x00\x00\x00AVI ")
            .expect("write non-archive bytes");
        f.flush().expect("flush");

        assert_eq!(
            magic_kind(f.path()).expect("magic_kind must not error"),
            None
        );
    }

    #[test]
    fn magic_kind_too_short_file_is_none_not_error() {
        // A 2-byte file must produce None (UnsupportedFormat at dispatch), not an I/O
        // error — the "short read → not a match" contract documented in CLAUDE.md.
        let mut f = tempfile::Builder::new()
            .suffix(".bin")
            .tempfile()
            .expect("tempfile");
        f.write_all(b"AB").expect("write 2 bytes");
        f.flush().expect("flush");

        assert_eq!(
            magic_kind(f.path()).expect("magic_kind must not error on a short file"),
            None
        );
    }

    #[test]
    fn directory_with_image_returns_folder_source() {
        let dir = tempfile::tempdir().expect("tempdir");
        let img_path = dir.path().join("cover.png");
        std::fs::write(&img_path, tiny_png()).expect("write png");

        let source = ArchiveLoader::open(dir.path()).expect("open dir");
        assert!(
            !source.list_pages().is_empty(),
            "FolderSource should list the PNG"
        );
    }

    #[test]
    fn empty_directory_returns_folder_source_with_no_pages() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = ArchiveLoader::open(dir.path()).expect("open empty dir");
        assert!(
            source.list_pages().is_empty(),
            "empty dir yields empty page list"
        );
    }

    #[test]
    fn cbz_extension_opens_zip_source() {
        let mut f = tempfile::Builder::new()
            .suffix(".cbz")
            .tempfile()
            .expect("tempfile");
        f.write_all(&tiny_zip_bytes()).expect("write cbz");
        f.flush().expect("flush");

        let source = ArchiveLoader::open(f.path()).expect("open .cbz");
        assert!(
            !source.list_pages().is_empty(),
            "ZipSource should list page.png"
        );
    }

    #[test]
    fn zip_extension_opens_zip_source() {
        let mut f = tempfile::Builder::new()
            .suffix(".zip")
            .tempfile()
            .expect("tempfile");
        f.write_all(&tiny_zip_bytes()).expect("write zip");
        f.flush().expect("flush");

        let source = ArchiveLoader::open(f.path()).expect("open .zip");
        assert!(
            !source.list_pages().is_empty(),
            "ZipSource should list page.png"
        );
    }

    #[test]
    fn cbz_extension_case_insensitive() {
        let mut f = tempfile::Builder::new()
            .suffix(".CBZ")
            .tempfile()
            .expect("tempfile");
        f.write_all(&tiny_zip_bytes()).expect("write CBZ");
        f.flush().expect("flush");

        let source = ArchiveLoader::open(f.path()).expect("open .CBZ");
        assert!(!source.list_pages().is_empty());
    }

    #[test]
    fn no_extension_zip_magic_opens_zip_source() {
        // Write real ZIP bytes to a file with no extension.
        let mut f = tempfile::Builder::new()
            .prefix("manga_no_ext_")
            .tempfile()
            .expect("tempfile");
        f.write_all(&tiny_zip_bytes()).expect("write zip bytes");
        f.flush().expect("flush");

        let source = ArchiveLoader::open(f.path()).expect("open no-ext zip by magic");
        assert!(
            !source.list_pages().is_empty(),
            "magic-byte fallback should yield ZipSource pages"
        );
    }

    #[test]
    fn spoofed_txt_extension_with_zip_magic_opens_zip_source() {
        // A file named *.txt whose contents are a real ZIP should still open.
        let mut f = tempfile::Builder::new()
            .suffix(".txt")
            .tempfile()
            .expect("tempfile");
        f.write_all(&tiny_zip_bytes()).expect("write zip bytes");
        f.flush().expect("flush");

        let source = ArchiveLoader::open(f.path()).expect("open .txt with zip magic");
        assert!(
            !source.list_pages().is_empty(),
            "magic-byte fallback should yield ZipSource pages for .txt with PK header"
        );
    }

    #[test]
    fn eocd_magic_no_extension_opens_zip_source_with_empty_pages() {
        // A bare 22-byte EOCD (starts with PK\x05\x06) with no zip extension must be
        // detected via magic fallback; `zip` accepts it as a valid empty archive → [].
        let mut f = tempfile::Builder::new()
            .prefix("eocd_only_")
            .tempfile()
            .expect("tempfile");
        // 22-byte minimal EOCD: signature (4) + 18 zero bytes
        let mut eocd = [0u8; 22];
        eocd[0..4].copy_from_slice(b"PK\x05\x06");
        f.write_all(&eocd).expect("write EOCD");
        f.flush().expect("flush");

        let Ok(src) = ArchiveLoader::open(f.path()) else {
            panic!("expected ZipSource for bare EOCD file, got UnsupportedFormat");
        };
        assert!(
            src.list_pages().is_empty(),
            "empty ZIP (EOCD only) should yield no pages"
        );
    }

    #[test]
    fn cbr_extension_opens_rar_source() {
        let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".cbr");
        let source = ArchiveLoader::open(cbr.path()).expect("open .cbr");
        assert_eq!(
            source.list_pages().len(),
            4,
            "RarSource should list the 4 image pages from the CBR fixture"
        );
    }

    #[test]
    fn rar_extension_opens_rar_source() {
        let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".rar");
        let source = ArchiveLoader::open(cbr.path()).expect("open .rar");
        assert_eq!(
            source.list_pages().len(),
            4,
            "RarSource should list the 4 image pages from a .rar file"
        );
    }

    #[test]
    fn cbr_extension_uppercase_opens_rar_source() {
        let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".CBR");
        let source = ArchiveLoader::open(cbr.path()).expect("open .CBR");
        assert_eq!(
            source.list_pages().len(),
            4,
            "RarSource dispatch must be case-insensitive for .CBR"
        );
    }

    #[test]
    fn spoofed_txt_extension_with_rar_magic_opens_rar_source() {
        // A file named *.txt whose first bytes are the RAR magic must be
        // dispatched to RarSource via the magic-byte fallback.
        let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".txt");
        let source = ArchiveLoader::open(cbr.path()).expect("open .txt with rar magic");
        assert_eq!(
            source.list_pages().len(),
            4,
            "magic-byte RAR fallback should yield the 4 RarSource pages for .txt with Rar! header"
        );
    }

    #[test]
    fn non_zip_txt_returns_unsupported_format() {
        let mut f = tempfile::Builder::new()
            .suffix(".txt")
            .tempfile()
            .expect("tempfile");
        f.write_all(b"this is plain text, not a zip")
            .expect("write text");
        f.flush().expect("flush");

        // `Arc<dyn PageSource>` does not implement `Debug`, so `expect_err` is
        // unavailable; match on the result to extract the error instead.
        let Err(err) = ArchiveLoader::open(f.path()) else {
            panic!("expected UnsupportedFormat error");
        };
        assert!(
            matches!(err, CoreError::UnsupportedFormat { .. }),
            "expected UnsupportedFormat, got: {err:?}"
        );
    }

    #[test]
    fn non_zip_no_extension_returns_unsupported_format() {
        let mut f = tempfile::Builder::new()
            .prefix("notazip_")
            .tempfile()
            .expect("tempfile");
        f.write_all(b"RIFF\x00\x00\x00\x00AVI ")
            .expect("write non-zip bytes");
        f.flush().expect("flush");

        // `Arc<dyn PageSource>` does not implement `Debug`, so `expect_err` is
        // unavailable; match on the result to extract the error instead.
        let Err(err) = ArchiveLoader::open(f.path()) else {
            panic!("expected UnsupportedFormat error");
        };
        assert!(
            matches!(err, CoreError::UnsupportedFormat { .. }),
            "expected UnsupportedFormat, got: {err:?}"
        );
    }

    #[test]
    fn unsupported_format_error_contains_path() {
        let mut f = tempfile::Builder::new()
            .suffix(".txt")
            .tempfile()
            .expect("tempfile");
        f.write_all(b"not a zip").expect("write text");
        f.flush().expect("flush");

        // `Arc<dyn PageSource>` does not implement `Debug`, so `expect_err` is
        // unavailable; match on the result to extract the error instead.
        let Err(err) = ArchiveLoader::open(f.path()) else {
            panic!("expected an error for a non-zip file");
        };
        let path_str = f.path().display().to_string();
        assert!(
            err.to_string().contains(&path_str),
            "error message should mention the path"
        );
    }

    #[test]
    fn too_short_file_unknown_extension_returns_unsupported_format() {
        // A 2-byte file with unknown extension must yield UnsupportedFormat, not an I/O
        // error — magic_kind's short-file guard treats partial reads as "no match".
        let mut f = tempfile::Builder::new()
            .suffix(".bin")
            .tempfile()
            .expect("tempfile");
        f.write_all(b"AB").expect("write 2 bytes");
        f.flush().expect("flush");

        let Err(err) = ArchiveLoader::open(f.path()) else {
            panic!("expected UnsupportedFormat for a too-short file");
        };
        assert!(
            matches!(err, CoreError::UnsupportedFormat { .. }),
            "expected UnsupportedFormat, got: {err:?}"
        );
    }

    /// Build a text-only ZIP (no image entries) in memory.
    fn text_only_zip_bytes() -> Vec<u8> {
        use zip::write::SimpleFileOptions;
        use zip::{CompressionMethod, ZipWriter};

        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut zw = ZipWriter::new(cursor);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("readme.txt", opts).expect("start_file");
            zw.write_all(b"this is not an image").expect("write text");
            zw.finish().expect("finish zip");
        }
        buf
    }

    #[test]
    fn probe_page_count_folder_with_images_returns_count() {
        // A folder with N image files → Ok(N). Zero-byte .png files count as pages
        // because FolderSource detects by extension only.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.png"), b"").expect("write a.png");
        std::fs::write(dir.path().join("b.png"), b"").expect("write b.png");
        std::fs::write(dir.path().join("c.jpg"), b"").expect("write c.jpg");

        let n = ArchiveLoader::probe_page_count(dir.path()).expect("probe ok");
        assert_eq!(n.get(), 3);
    }

    #[test]
    fn probe_page_count_folder_images_in_subfolder_only_returns_empty_book() {
        // Images in a subdirectory only: FolderSource uses max_depth(1) so they are
        // excluded, making the top level effectively empty → EmptyBook.
        let dir = tempfile::tempdir().expect("tempdir");
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).expect("mkdir sub");
        std::fs::write(sub.join("hidden.png"), b"").expect("write hidden.png");

        let Err(err) = ArchiveLoader::probe_page_count(dir.path()) else {
            panic!("expected EmptyBook for images-only-in-subfolder");
        };
        assert!(
            matches!(err, CoreError::EmptyBook { .. }),
            "expected EmptyBook, got: {err:?}"
        );
    }

    #[test]
    fn probe_page_count_folder_uppercase_extension_is_counted() {
        // Uppercase .PNG must be detected by FolderSource (eq_ignore_ascii_case).
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("COVER.PNG"), b"").expect("write COVER.PNG");

        let n = ArchiveLoader::probe_page_count(dir.path()).expect("probe ok");
        assert_eq!(n.get(), 1);
    }

    #[test]
    fn probe_page_count_empty_folder_returns_empty_book() {
        let dir = tempfile::tempdir().expect("tempdir");

        let Err(err) = ArchiveLoader::probe_page_count(dir.path()) else {
            panic!("expected EmptyBook for empty folder");
        };
        assert!(
            matches!(err, CoreError::EmptyBook { .. }),
            "expected EmptyBook, got: {err:?}"
        );
    }

    #[test]
    fn probe_page_count_zip_with_image_returns_one() {
        let mut f = tempfile::Builder::new()
            .suffix(".zip")
            .tempfile()
            .expect("tempfile");
        f.write_all(&tiny_zip_bytes()).expect("write zip");
        f.flush().expect("flush");

        let n = ArchiveLoader::probe_page_count(f.path()).expect("probe ok");
        assert_eq!(n.get(), 1);
    }

    #[test]
    fn probe_page_count_zip_without_images_returns_empty_book() {
        // A ZIP containing only a .txt entry → EmptyBook.
        let mut f = tempfile::Builder::new()
            .suffix(".zip")
            .tempfile()
            .expect("tempfile");
        f.write_all(&text_only_zip_bytes())
            .expect("write text-only zip");
        f.flush().expect("flush");

        let Err(err) = ArchiveLoader::probe_page_count(f.path()) else {
            panic!("expected EmptyBook for text-only zip");
        };
        assert!(
            matches!(err, CoreError::EmptyBook { .. }),
            "expected EmptyBook, got: {err:?}"
        );
    }

    #[test]
    fn probe_page_count_nonexistent_path_returns_io_error() {
        let err = ArchiveLoader::probe_page_count("/nonexistent/path/that/cannot/exist.zip")
            .expect_err("should fail for nonexistent path");
        assert!(
            matches!(err, CoreError::Io(_)),
            "expected CoreError::Io, got: {err:?}"
        );
    }

    #[test]
    fn probe_page_count_plain_text_file_returns_unsupported_format() {
        let mut f = tempfile::Builder::new()
            .suffix(".txt")
            .tempfile()
            .expect("tempfile");
        f.write_all(b"not an archive").expect("write text");
        f.flush().expect("flush");

        let Err(err) = ArchiveLoader::probe_page_count(f.path()) else {
            panic!("expected UnsupportedFormat for plain text file");
        };
        assert!(
            matches!(err, CoreError::UnsupportedFormat { .. }),
            "expected UnsupportedFormat, got: {err:?}"
        );
    }

    #[test]
    fn probe_page_count_cbr_fixture_returns_real_count() {
        // SAMPLE_CBR_B64 contains 4 image pages (verified by cbr_extension_opens_rar_source).
        let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".cbr");
        let n = ArchiveLoader::probe_page_count(cbr.path()).expect("probe cbr ok");
        assert_eq!(n.get(), 4);
    }

    #[test]
    fn rar_extension_blocked_when_allow_rar_false() {
        let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".rar");
        let policy = ArchivePolicy { allow_rar: false };
        let Err(err) = ArchiveLoader::open_with_policy(cbr.path(), policy) else {
            panic!("expected FormatDisabled for .rar with allow_rar=false");
        };
        assert!(
            matches!(err, CoreError::FormatDisabled { .. }),
            "expected FormatDisabled, got: {err:?}"
        );
    }

    #[test]
    fn cbr_extension_blocked_when_allow_rar_false() {
        let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".cbr");
        let policy = ArchivePolicy { allow_rar: false };
        let Err(err) = ArchiveLoader::open_with_policy(cbr.path(), policy) else {
            panic!("expected FormatDisabled for .cbr with allow_rar=false");
        };
        assert!(
            matches!(err, CoreError::FormatDisabled { .. }),
            "expected FormatDisabled, got: {err:?}"
        );
    }

    #[test]
    fn rar_magic_blocked_when_allow_rar_false() {
        // A file with RAR magic bytes but a .txt extension must also be blocked
        // via the magic-byte fallback path.
        let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".txt");
        let policy = ArchivePolicy { allow_rar: false };
        let Err(err) = ArchiveLoader::open_with_policy(cbr.path(), policy) else {
            panic!("expected FormatDisabled for RAR magic + .txt with allow_rar=false");
        };
        assert!(
            matches!(err, CoreError::FormatDisabled { .. }),
            "expected FormatDisabled, got: {err:?}"
        );
    }

    #[test]
    fn zip_still_opens_when_allow_rar_false() {
        let mut f = tempfile::Builder::new()
            .suffix(".cbz")
            .tempfile()
            .expect("tempfile");
        f.write_all(&tiny_zip_bytes()).expect("write cbz");
        f.flush().expect("flush");
        let policy = ArchivePolicy { allow_rar: false };
        let src = ArchiveLoader::open_with_policy(f.path(), policy).expect("cbz must open");
        assert!(!src.list_pages().is_empty());
    }

    #[test]
    fn directory_still_opens_when_allow_rar_false() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("cover.png"), tiny_png()).expect("write png");
        let policy = ArchivePolicy { allow_rar: false };
        let src = ArchiveLoader::open_with_policy(dir.path(), policy).expect("dir must open");
        assert!(!src.list_pages().is_empty());
    }

    #[test]
    fn open_still_permits_rar_backward_compat() {
        // ArchiveLoader::open uses ArchivePolicy::default() (allow_rar=true).
        let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".cbr");
        let src = ArchiveLoader::open(cbr.path()).expect("open must permit rar by default");
        assert_eq!(src.list_pages().len(), 4);
    }
}
