//! Dispatch a path to the right `PageSource`: directory -> FolderSource,
//! CBZ/ZIP (by extension, else magic bytes) -> ZipSource.

use crate::error::CoreError;
use crate::page_source::{FolderSource, PageSource, ZipSource};
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::sync::Arc;

/// ZIP local-file, end-of-central-directory, and data-descriptor signatures.
const ZIP_MAGICS: &[&[u8]] = &[b"PK\x03\x04", b"PK\x05\x06", b"PK\x07\x08"];

/// Opens any supported source by path, returning a type-erased `Arc<dyn PageSource>`.
///
/// Resolution order:
/// 1. If the path is a directory: [`FolderSource`].
/// 2. If the file extension is `cbz` or `zip` (case-insensitive): [`ZipSource`].
/// 3. If the first 4 bytes match a ZIP signature: [`ZipSource`].
/// 4. Otherwise: [`CoreError::UnsupportedFormat`].
pub struct ArchiveLoader;

impl ArchiveLoader {
    /// Open `path` as the most appropriate [`PageSource`].
    ///
    /// Returns `Err(CoreError::UnsupportedFormat)` when the path is a file that
    /// is neither a recognized archive extension nor starts with ZIP magic bytes.
    pub fn open(path: impl AsRef<Path>) -> Result<Arc<dyn PageSource>, CoreError> {
        let path = path.as_ref();
        if path.is_dir() {
            return Ok(Arc::new(FolderSource::open(path)?));
        }
        if is_zip(path)? {
            return Ok(Arc::new(ZipSource::open(path)?));
        }
        Err(CoreError::UnsupportedFormat {
            path: path.display().to_string(),
        })
    }
}

/// Return `true` when `path` looks like a ZIP archive.
///
/// Extension check is tried first (cheap, no I/O). If the extension is absent
/// or unrecognised the first 4 bytes are read and compared against the three
/// ZIP signatures (`PK\x03\x04`, `PK\x05\x06`, `PK\x07\x08`).
fn is_zip(path: &Path) -> Result<bool, CoreError> {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if ext.eq_ignore_ascii_case("cbz") || ext.eq_ignore_ascii_case("zip") {
            return Ok(true);
        }
    }
    let mut head = [0u8; 4];
    let mut f = std::fs::File::open(path)?;
    match f.read_exact(&mut head) {
        Ok(()) => Ok(ZIP_MAGICS.iter().any(|m| *m == &head[..])),
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    // ------------------------------------------------------------------
    // FolderSource path
    // ------------------------------------------------------------------

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

    // ------------------------------------------------------------------
    // ZipSource path — extension-based detection
    // ------------------------------------------------------------------

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

    // ------------------------------------------------------------------
    // ZipSource path — magic-byte fallback (no/wrong extension)
    // ------------------------------------------------------------------

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
        // A 22-byte minimal end-of-central-directory record starts with
        // PK\x05\x06 — the second ZIP_MAGICS entry. Written to a file with no
        // zip extension, it must be detected via the magic-byte fallback and
        // opened as a ZipSource (not UnsupportedFormat). The `zip` crate accepts
        // a bare EOCD as a valid empty archive, so list_pages() must return [].
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

    // ------------------------------------------------------------------
    // UnsupportedFormat path
    // ------------------------------------------------------------------

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
}
