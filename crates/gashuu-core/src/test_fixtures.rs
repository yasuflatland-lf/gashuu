//! Shared test-only RAR/CBR and AVIF fixtures and helpers (PR7).
//!
//! RAR and AVIF both lack an in-tree Rust encoder, so their fixtures are
//! committed as base64 *text* — mirroring the insta `.snap` "committed text, not
//! a binary fixture" exception already used in the project. The `.cbr` blobs were
//! produced by the hand-written RAR4 store-format generator
//! `.claude/plans/pr7-fixture-gen.py` and verified byte-for-byte against the real
//! `unrar` crate (0.5.8). Full provenance + observed-value tables live in
//! `.claude/plans/pr7-fixture.md`. The AVIF blob (see [`SAMPLE_AVIF_8X6_B64`]) is
//! committed for a different reason: gashuu deliberately builds `image` WITHOUT
//! its `avif` encode feature, so `ravif`/`rav1e` can no longer synthesize AVIF
//! bytes in-memory the way `png_bytes` does.
//!
//! Centralizing the blobs here removes the duplication that previously lived in
//! both `page_source::rar` and `archive_loader` test modules.

use base64::Engine;
use std::io::Write;
use tempfile::{Builder, NamedTempFile};

/// Fixture A (560 bytes): six entries whose PHYSICAL (archive/insertion) order is
/// DELIBERATELY SCRAMBLED — `2.png`(2x3), `10.png`(3x2), `1.png`(2x2),
/// `notes.txt`(non-image), an explicit `sub/` DIRECTORY header
/// (`is_directory() == true`), and `sub/3.png`(4x4). Through `RarSource` the image
/// pages natural-sort to `["1.png","2.png","10.png","sub/3.png"]` (`sub/3.png`
/// sorts LAST) and `skipped_count() == 0`. Because the physical order differs from
/// the natural order, every top-level page maps to a DIFFERENT archive position
/// than its page index (page->seq_index = {0->2, 1->0, 2->1, 3->5}); the scramble
/// forces natural-sort to reorder the entries so the round-trip test genuinely
/// exercises the `seq_index` read walk. The distinct PNG dimensions let a
/// read→decode prove the exact page_index → entry mapping: a regression that read
/// `page_index` directly would return the wrong page (page 0 → `2.png`'s 2x3
/// instead of `1.png`'s 2x2) and fail.
pub(crate) const SAMPLE_CBR_B64: &str = "UmFyIRoHAM+QcwAADQAAAAAAAACh8nQAgCUASQAAAEkAAAADqyZ1cgAAoU4UMAUAIAAAADIucG5niVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAEElEQVR42mP4z8AARAwoFABE0AX7TM5xDwAAAABJRU5ErkJggkpzdACAJgBJAAAASQAAAAMYtlxUAAChThQwBgAgAAAAMTAucG5niVBORw0KGgoAAAANSUhEUgAAAAMAAAACCAIAAAASFvFNAAAAEElEQVR42mP4z8AAQQxwFgBB0gX7b/EWxwAAAABJRU5ErkJggqvMdACAJQBJAAAASQAAAAOFtl5wAAChThQwBQAgAAAAMS5wbmeJUE5HDQoaCgAAAA1JSERSAAAAAgAAAAIIAgAAAP3UmnMAAAAQSURBVHjaY/jPwABEDBAKAB/uA/1jXrtbAAAAAElFTkSuQmCCBmd0AIApAAwAAAAMAAAAAy/dsscAAKFOFDAJACAAAABub3Rlcy50eHRub3QgYW4gaW1hZ2VJZXTgACMAAAAAAAAAAAADAAAAAAAAoU4UMAMAEAAAAHN1YqlhdACAKQBJAAAASQAAAAPdJU2pAAChThQwCQAgAAAAc3ViLzMucG5niVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAAEElEQVR42mP4z8AARwzEcQCukw/xOF6MEQAAAABJRU5ErkJgggSwewAABwA=";

/// Fixture B (310 bytes): one safe `1.png`(2x2) plus two `..` traversal entries —
/// `../evil.png` (image-looking → COUNTED as skipped) and `../readme.txt`
/// (non-image → NOT counted). `unrar` preserves the `..` in `filename`, so
/// `enclosed_name` rejects both. Through `RarSource`: pages == `["1.png"]`,
/// `skipped_count() == 1`.
pub(crate) const HOSTILE_CBR_B64: &str = "UmFyIRoHAM+QcwAADQAAAAAAAACrzHQAgCUASQAAAEkAAAADhbZecAAAoU4UMAUAIAAAADEucG5niVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR42mP4z8AARAwQCgAf7gP9Y167WwAAAABJRU5ErkJgguHwdACAKwBJAAAASQAAAAOFtl5wAAChThQwCwAgAAAALi4vZXZpbC5wbmeJUE5HDQoaCgAAAA1JSERSAAAAAgAAAAIIAgAAAP3UmnMAAAAQSURBVHjaY/jPwABEDBAKAB/uA/1jXrtbAAAAAElFTkSuQmCCYxt0AIAtAAwAAAAMAAAAAy/dsscAAKFOFDANACAAAAAuLi9yZWFkbWUudHh0bm90IGFuIGltYWdlBLB7AAAHAA==";

/// Fixture C (137 bytes): a valid `1.png`(2x2) followed by a corrupt trailing
/// header. `unrar`'s List iterator yields `Ok("1.png")`, then `Ok("")` (a
/// phantom empty-name entry it salvages from the corrupt block, filtered as
/// neither image nor skip), then `Err(BadData)`. Under the skip+count+break
/// listing loop: pages == `["1.png"]`, `skipped_count() == 1`, and
/// `read_bytes(0)` still decodes the good 2x2 page.
pub(crate) const CORRUPT_TRAILING_CBR_B64: &str = "UmFyIRoHAM+QcwAADQAAAAAAAACrzHQAgCUASQAAAEkAAAADhbZecAAAoU4UMAUAIAAAADEucG5niVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR42mP4z8AARAwQCgAf7gP9Y167WwAAAABJRU5ErkJggu++dAAABwA=";

/// Fixture D (338 bytes): a macOS-metadata noise entry whose name carries an
/// image extension and (under case-insensitive natural ordering) sorts AHEAD of
/// the real pages, plus two real images. Physical order is
/// `__MACOSX/Manga/._001.jpg` (AppleDouble resource fork, NOT an image — must be
/// dropped without counting as a skip), `Manga/001.jpg`(2x2), `Manga/002.jpg`(2x2).
/// Through `RarSource`: pages == `["Manga/001.jpg", "Manga/002.jpg"]`,
/// `skipped_count() == 0`, and page 0 is the real first page, not the resource fork.
pub(crate) const MACOS_METADATA_CBR_B64: &str = "UmFyIRoHAM+QcwAADQAAAAAAAAA2MXQAgDgAEQAAABEAAAADzfqQj6vC0HMUMBgAIAAAAF9fTUFDT1NYL01hbmdhLy5fMDAxLmpwZ0FwcGxlRG91YmxlIG5vaXNlVJd0AIAtAEoAAABKAAAAA4E2aZ+rwtBzFDANACAAAABNYW5nYS8wMDEuanBniVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR42mPgEpH7D8IMMAYAJowE7d8n2J0AAAAASUVORK5CYIKE7XQAgC0ASgAAAEoAAAADgTZpn6vC0HMUMA0AIAAAAE1hbmdhLzAwMi5qcGeJUE5HDQoaCgAAAA1JSERSAAAAAgAAAAIIBgAAAHK2DSQAAAARSURBVHjaY+ASkfsPwgwwBgAmjATt3yfYnQAAAABJRU5ErkJgggSwewAABwA=";

/// Base64-decode `b64` and write the bytes to a `.cbr` tempfile. The returned
/// `NamedTempFile` is kept alive by the caller so the path stays valid.
pub(crate) fn write_cbr(b64: &str) -> NamedTempFile {
    write_cbr_with_suffix(b64, ".cbr")
}

/// Like [`write_cbr`] but lets the caller choose the filename `suffix` (e.g.
/// `.rar`, `.CBR`, `.txt`) so the `archive_loader` extension/magic tests can
/// drive each dispatch branch from the same fixture bytes.
pub(crate) fn write_cbr_with_suffix(b64: &str, suffix: &str) -> NamedTempFile {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .expect("fixture base64 must decode");
    let tmp = Builder::new().suffix(suffix).tempfile().expect("tempfile");
    // Reopen the handle for writing without consuming the NamedTempFile so it
    // isn't deleted while the caller still holds the path.
    let mut file = tmp.reopen().expect("reopen tempfile");
    file.write_all(&bytes).expect("write cbr bytes");
    file.flush().expect("flush cbr bytes");
    tmp
}

/// A tiny 8×6 AVIF page fixture, committed as base64 *text* (see the module docs
/// for why AVIF, like RAR, can no longer be synthesized in-memory: gashuu-core
/// builds `image` WITHOUT its `avif` encode feature, so `ravif`/`rav1e` is not
/// linkable from this crate's tests). NOTE: `rav1e` (and its `paste` /
/// RUSTSEC-2024-0436 advisory) still appears in `Cargo.lock` via Slint's
/// build-time compiler — see the ignore reason in `deny.toml` — but it is no
/// longer reachable from gashuu-core's own build to synthesize a fixture.
///
/// Produced once by encoding an 8×6 solid RGBA image through `image`'s ravif
/// encoder before that feature was dropped; decoding it exercises the
/// `avif-native` (dav1d) decode path used in production.
pub(crate) const SAMPLE_AVIF_8X6_B64: &str = "AAAAGGZ0eXBhdmlmAAAAAG1pZjFtaWFmAAAA0m1ldGEAAAAAAAAAIWhkbHIAAAAAAAAAAHBpY3QAAAAAAAAAAAAAAAAAAAAADnBpdG0AAAAAAAEAAAAeaWxvYwAAAABEAAABAAEAAAABAAAA8gAAACsAAAAjaWluZgAAAAAAAQAAABVpbmZlAgAAAAABAABhdjAxAAAAAFZpcHJwAAAAOGlwY28AAAAUaXNwZQAAAAAAAAAIAAAABgAAAAxhdjFDgT8AAAAAABBwaXhpAAAAAAMICAgAAAAWaXBtYQAAAAAAAAABAAEDAYIDAAAAM21kYXQSAAoIP8i9IQENBtAyHWWCL0MnE/zAACAAEAAAAAAAAAAABiPoE42RY8+Y";

/// Base64-decode [`SAMPLE_AVIF_8X6_B64`] into the raw AVIF file bytes.
pub(crate) fn avif_8x6_bytes() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(SAMPLE_AVIF_8X6_B64)
        .expect("avif fixture base64 must decode")
}
