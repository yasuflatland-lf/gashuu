use super::ArchiveKind as Kind;
use super::*;
use crate::test_fixtures::{write_cbr_with_suffix, SAMPLE_CBR_B64};
use image::RgbaImage;
use std::io::{Cursor, Write};

fn open(path: impl AsRef<Path>) -> Result<Arc<dyn PageSource>, CoreError> {
    ArchiveLoader::open_with_policy(path, ArchivePolicy::default())
}

#[test]
fn open_book_rejects_zero_page_folder_as_empty_book() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        ArchiveLoader::open_book(dir.path(), ArchivePolicy::default()),
        Err(CoreError::EmptyBook { label }) if label == dir.path().to_str().unwrap()
    ));
}

#[test]
fn open_book_counts_image_pages() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.png", "b.png", "c.png"] {
        std::fs::write(dir.path().join(name), []).unwrap();
    }
    let book = ArchiveLoader::open_book(dir.path(), ArchivePolicy::default())
        .unwrap_or_else(|error| panic!("expected an opened book: {error}"));
    assert_eq!(book.page_count().get(), 3);
}

#[test]
fn format_disabled_carries_archive_kind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.cbr");
    std::fs::write(&path, []).unwrap();
    assert!(matches!(
        ArchiveLoader::open_book(path, ArchivePolicy { allow_rar: false }),
        Err(CoreError::FormatDisabled {
            kind: ArchiveKind::Rar
        })
    ));
}

#[test]
fn open_book_read_at_empty_zip_is_empty_book() {
    let mut eocd = [0; 22];
    eocd[..4].copy_from_slice(ZIP_MAGICS[1]);
    assert!(matches!(
        ArchiveLoader::open_book_read_at(
            "empty.cbz",
            Arc::new(Arc::<[u8]>::from(eocd)),
        ),
        Err(CoreError::EmptyBook { label }) if label == "empty.cbz"
    ));
}

#[test]
fn detect_prefers_extension_over_magic_on_both_routes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.cbz");
    std::fs::write(&path, RAR_MAGIC).unwrap();
    let policy = ArchivePolicy { allow_rar: false };
    assert!(matches!(
        ArchiveLoader::open_with_policy(path, policy),
        Err(CoreError::Zip(_))
    ));
    assert!(matches!(
        ArchiveLoader::open_read_at("book.cbz", Arc::new(Arc::<[u8]>::from(RAR_MAGIC))),
        Err(CoreError::Zip(_))
    ));
}

#[cfg(unix)]
#[test]
fn open_with_policy_rejects_a_non_utf8_path() {
    // Unix permits arbitrary path bytes; macOS paths are always valid UTF-8.
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let path = Path::new(OsStr::from_bytes(b"bad\xffname.cbz"));
    let Err(CoreError::NonUtf8Path { path }) =
        ArchiveLoader::open_with_policy(path, ArchivePolicy::default())
    else {
        panic!("expected NonUtf8Path");
    };
    assert!(!path.is_empty(), "the lossy path must remain readable");
}

#[cfg(unix)]
#[test]
fn probe_page_count_propagates_non_utf8_rejection() {
    // Unix permits arbitrary path bytes; macOS paths are always valid UTF-8.
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let path = Path::new(OsStr::from_bytes(b"bad\xffname.cbz"));
    let error = ArchiveLoader::probe_page_count_with_policy(path, ArchivePolicy::default())
        .expect_err("expected rejection");
    assert!(
        matches!(error, CoreError::NonUtf8Path { .. }),
        "unreadable must not be classified as empty or unsupported: {error:?}"
    );
}

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
fn archive_kind_wire_round_trip() {
    for (kind, json, display) in [
        (ArchiveKind::Zip, r#""zip""#, "ZIP/CBZ"),
        (ArchiveKind::Rar, r#""rar""#, "RAR/CBR"),
    ] {
        assert_eq!(serde_json::to_string(&kind).unwrap(), json);
        assert_eq!(serde_json::from_str::<ArchiveKind>(json).unwrap(), kind);
        assert_eq!(kind.to_string(), display);
    }
}

#[test]
fn detect_kind_table() {
    let cases: [(&str, &[u8], Option<ArchiveKind>); 7] = [
        ("a.cbz", b"", Some(ArchiveKind::Zip)),
        ("A.CBR", b"", Some(ArchiveKind::Rar)),
        ("x.txt", b"PK\x03\x04", Some(ArchiveKind::Zip)),
        ("x", b"Rar!\x1A\x07\x00", Some(ArchiveKind::Rar)),
        ("x", b"PK", None),
        ("x", b"", None),
        ("x.cbz", b"Rar!\x1A\x07", Some(ArchiveKind::Zip)),
    ];
    for (name, head, expected) in cases {
        assert_eq!(ArchiveLoader::detect_kind(name, head), expected);
    }
}

#[test]
fn open_read_at_opens_zip_bytes() {
    let bytes = Arc::<[u8]>::from(tiny_zip_bytes());
    let source =
        ArchiveLoader::open_read_at("book.bin", Arc::new(bytes)).expect("open ZIP by magic");
    let pages = source.list_pages();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].name, "page.png");
    assert_eq!(source.read_bytes(0).unwrap(), tiny_png());
}

#[test]
fn open_read_at_rejects_rar_by_signature() {
    let bytes = Arc::<[u8]>::from(&b"Rar!\x1A\x07\x00"[..]);
    assert!(matches!(
        ArchiveLoader::open_read_at("book", Arc::new(bytes)),
        Err(CoreError::FormatDisabled {
            kind: ArchiveKind::Rar
        })
    ));
}

#[test]
fn open_read_at_unsupported_carries_name_label() {
    let bytes = Arc::<[u8]>::from(&b"PK"[..]);
    assert!(matches!(
        ArchiveLoader::open_read_at("notes.txt", Arc::new(bytes)),
        Err(CoreError::UnsupportedFormat { label }) if label == "notes.txt"
    ));
}

#[test]
fn probe_page_count_read_at_empty_zip_is_empty_book_with_name_label() {
    let mut eocd = [0u8; 22];
    eocd[..4].copy_from_slice(b"PK\x05\x06");
    let bytes = Arc::<[u8]>::from(eocd);
    assert!(matches!(
        ArchiveLoader::probe_page_count_read_at(
            "empty.cbz",
            Arc::new(bytes)
        ),
        Err(CoreError::EmptyBook { label }) if label == "empty.cbz"
    ));
}

#[test]
fn kind_from_name_cbr_lowercase_is_rar() {
    assert_eq!(kind_from_name("x.cbr"), Some(Kind::Rar));
}

#[test]
fn kind_from_name_rar_lowercase_is_rar() {
    assert_eq!(kind_from_name("x.rar"), Some(Kind::Rar));
}

#[test]
fn kind_from_name_cbr_uppercase_is_rar() {
    assert_eq!(kind_from_name("x.CBR"), Some(Kind::Rar));
}

#[test]
fn kind_from_name_cbz_lowercase_is_zip() {
    assert_eq!(kind_from_name("x.cbz"), Some(Kind::Zip));
}

#[test]
fn kind_from_name_zip_lowercase_is_zip() {
    assert_eq!(kind_from_name("x.zip"), Some(Kind::Zip));
}

#[test]
fn kind_from_name_txt_is_none() {
    assert_eq!(kind_from_name("x.txt"), None);
}

#[test]
fn kind_from_name_no_extension_is_none() {
    assert_eq!(kind_from_name("noext"), None);
}

#[test]
fn detect_reads_magic_for_unknown_extensions() {
    for (head, expected) in [
        (RAR_MAGIC, Some(Kind::Rar)),
        (ZIP_MAGICS[0], Some(Kind::Zip)),
        (&b"RIFF\x00\x00\x00\x00AVI "[..], None),
        (&b"AB"[..], None),
    ] {
        let kind = detect("book.txt", |buf| Ok(Cursor::new(head).read(buf)?)).unwrap();
        assert_eq!(kind, expected);
    }
}

#[test]
fn directory_with_image_returns_folder_source() {
    let dir = tempfile::tempdir().expect("tempdir");
    let img_path = dir.path().join("cover.png");
    std::fs::write(&img_path, tiny_png()).expect("write png");

    let source = open(dir.path()).expect("open dir");
    assert!(
        !source.list_pages().is_empty(),
        "FolderSource should list the PNG"
    );
}

#[test]
fn empty_directory_returns_folder_source_with_no_pages() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = open(dir.path()).expect("open empty dir");
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

    let source = open(f.path()).expect("open .cbz");
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

    let source = open(f.path()).expect("open .zip");
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

    let source = open(f.path()).expect("open .CBZ");
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

    let source = open(f.path()).expect("open no-ext zip by magic");
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

    let source = open(f.path()).expect("open .txt with zip magic");
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

    let Ok(src) = open(f.path()) else {
        panic!("expected ZipSource for bare EOCD file, got UnsupportedFormat");
    };
    assert!(
        src.list_pages().is_empty(),
        "empty ZIP (EOCD only) should yield no pages"
    );
}

#[cfg(feature = "rar")]
#[test]
fn cbr_extension_opens_rar_source() {
    let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".cbr");
    let source = open(cbr.path()).expect("open .cbr");
    assert_eq!(
        source.list_pages().len(),
        4,
        "RarSource should list the 4 image pages from the CBR fixture"
    );
}

#[cfg(feature = "rar")]
#[test]
fn rar_extension_opens_rar_source() {
    let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".rar");
    let source = open(cbr.path()).expect("open .rar");
    assert_eq!(
        source.list_pages().len(),
        4,
        "RarSource should list the 4 image pages from a .rar file"
    );
}

#[cfg(feature = "rar")]
#[test]
fn cbr_extension_uppercase_opens_rar_source() {
    let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".CBR");
    let source = open(cbr.path()).expect("open .CBR");
    assert_eq!(
        source.list_pages().len(),
        4,
        "RarSource dispatch must be case-insensitive for .CBR"
    );
}

#[cfg(feature = "rar")]
#[test]
fn spoofed_txt_extension_with_rar_magic_opens_rar_source() {
    // A file named *.txt whose first bytes are the RAR magic must be
    // dispatched to RarSource via the magic-byte fallback.
    let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".txt");
    let source = open(cbr.path()).expect("open .txt with rar magic");
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
    let Err(err) = open(f.path()) else {
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
    let Err(err) = open(f.path()) else {
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
    let Err(err) = open(f.path()) else {
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
    // error — magic detection treats partial reads as "no match".
    let mut f = tempfile::Builder::new()
        .suffix(".bin")
        .tempfile()
        .expect("tempfile");
    f.write_all(b"AB").expect("write 2 bytes");
    f.flush().expect("flush");

    let Err(err) = open(f.path()) else {
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

    let n = ArchiveLoader::probe_page_count_with_policy(dir.path(), ArchivePolicy::default())
        .expect("probe ok");
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

    let Err(err) =
        ArchiveLoader::probe_page_count_with_policy(dir.path(), ArchivePolicy::default())
    else {
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

    let n = ArchiveLoader::probe_page_count_with_policy(dir.path(), ArchivePolicy::default())
        .expect("probe ok");
    assert_eq!(n.get(), 1);
}

#[test]
fn probe_page_count_empty_folder_returns_empty_book() {
    let dir = tempfile::tempdir().expect("tempdir");

    let Err(err) =
        ArchiveLoader::probe_page_count_with_policy(dir.path(), ArchivePolicy::default())
    else {
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

    let n = ArchiveLoader::probe_page_count_with_policy(f.path(), ArchivePolicy::default())
        .expect("probe ok");
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

    let Err(err) = ArchiveLoader::probe_page_count_with_policy(f.path(), ArchivePolicy::default())
    else {
        panic!("expected EmptyBook for text-only zip");
    };
    assert!(
        matches!(err, CoreError::EmptyBook { .. }),
        "expected EmptyBook, got: {err:?}"
    );
}

#[test]
fn probe_page_count_nonexistent_path_returns_io_error() {
    let err = ArchiveLoader::probe_page_count_with_policy(
        "/nonexistent/path/that/cannot/exist.zip",
        ArchivePolicy::default(),
    )
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

    let Err(err) = ArchiveLoader::probe_page_count_with_policy(f.path(), ArchivePolicy::default())
    else {
        panic!("expected UnsupportedFormat for plain text file");
    };
    assert!(
        matches!(err, CoreError::UnsupportedFormat { .. }),
        "expected UnsupportedFormat, got: {err:?}"
    );
}

#[cfg(feature = "rar")]
#[test]
fn probe_page_count_cbr_fixture_returns_real_count() {
    // SAMPLE_CBR_B64 contains 4 image pages (verified by cbr_extension_opens_rar_source).
    let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".cbr");
    let n = ArchiveLoader::probe_page_count_with_policy(cbr.path(), ArchivePolicy::default())
        .expect("probe cbr ok");
    assert_eq!(n.get(), 4);
}

#[cfg(not(feature = "rar"))]
#[test]
fn cbr_extension_is_format_disabled_without_rar_feature() {
    let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".cbr");
    assert!(matches!(
        ArchiveLoader::open_with_policy(cbr.path(), ArchivePolicy { allow_rar: true }),
        Err(CoreError::FormatDisabled {
            kind: ArchiveKind::Rar
        })
    ));
}

#[cfg(not(feature = "rar"))]
#[test]
fn rar_magic_is_format_disabled_without_rar_feature() {
    let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".txt");
    assert!(matches!(
        ArchiveLoader::open_with_policy(cbr.path(), ArchivePolicy { allow_rar: true }),
        Err(CoreError::FormatDisabled {
            kind: ArchiveKind::Rar
        })
    ));
}

#[cfg(not(feature = "rar"))]
#[test]
fn probe_page_count_cbr_is_format_disabled_without_rar_feature() {
    let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".cbr");
    assert!(matches!(
        ArchiveLoader::probe_page_count_with_policy(cbr.path(), ArchivePolicy::default()),
        Err(CoreError::FormatDisabled {
            kind: ArchiveKind::Rar
        })
    ));
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

#[cfg(feature = "rar")]
#[test]
fn open_still_permits_rar_backward_compat() {
    let cbr = write_cbr_with_suffix(SAMPLE_CBR_B64, ".cbr");
    let src = open(cbr.path()).expect("open must permit rar by default");
    assert_eq!(src.list_pages().len(), 4);
}
