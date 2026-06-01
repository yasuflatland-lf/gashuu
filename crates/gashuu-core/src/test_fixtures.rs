//! Shared test-only RAR/CBR fixtures and helpers (PR7).
//!
//! RAR has no Rust encoder, so these `.cbr` archives are committed as base64
//! *text* — mirroring the insta `.snap` "committed text, not a binary fixture"
//! exception already used in the project. Each blob was produced by the
//! hand-written RAR4 store-format generator `.claude/plans/pr7-fixture-gen.py`
//! and verified byte-for-byte against the real `unrar` crate (0.5.8). Full
//! provenance + observed-value tables live in `.claude/plans/pr7-fixture.md`.
//!
//! Centralizing the blobs here removes the duplication that previously lived in
//! both `page_source::rar` and `archive_loader` test modules.

use base64::Engine;
use std::io::Write;
use tempfile::{Builder, NamedTempFile};

/// Fixture A (560 bytes): six entries in archive/insertion order — `1.png`(2x2),
/// `2.png`(2x3), `10.png`(3x2), `notes.txt`(non-image), an explicit `sub/`
/// DIRECTORY header (`is_directory() == true`), and `sub/3.png`(4x4). Through
/// `RarSource` the image pages are `["1.png","2.png","10.png","sub/3.png"]`
/// (natural order — `sub/3.png` sorts LAST) and `skipped_count() == 0`. The
/// distinct PNG dimensions let a read→decode prove the exact index→entry mapping.
pub(crate) const SAMPLE_CBR_B64: &str = "UmFyIRoHAM+QcwAADQAAAAAAAACrzHQAgCUASQAAAEkAAAADhbZecAAAoU4UMAUAIAAAADEucG5niVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR42mP4z8AARAwQCgAf7gP9Y167WwAAAABJRU5ErkJggqHydACAJQBJAAAASQAAAAOrJnVyAAChThQwBQAgAAAAMi5wbmeJUE5HDQoaCgAAAA1JSERSAAAAAgAAAAMIAgAAADaISdYAAAAQSURBVHjaY/jPwABEDCgUAETQBftMznEPAAAAAElFTkSuQmCCSnN0AIAmAEkAAABJAAAAAxi2XFQAAKFOFDAGACAAAAAxMC5wbmeJUE5HDQoaCgAAAA1JSERSAAAAAwAAAAIIAgAAABIW8U0AAAAQSURBVHjaY/jPwABBDHAWAEHSBftv8RbHAAAAAElFTkSuQmCCBmd0AIApAAwAAAAMAAAAAy/dsscAAKFOFDAJACAAAABub3Rlcy50eHRub3QgYW4gaW1hZ2VJZXTgACMAAAAAAAAAAAADAAAAAAAAoU4UMAMAEAAAAHN1YqlhdACAKQBJAAAASQAAAAPdJU2pAAChThQwCQAgAAAAc3ViLzMucG5niVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAAEElEQVR42mP4z8AARwzEcQCukw/xOF6MEQAAAABJRU5ErkJgggSwewAABwA=";

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
