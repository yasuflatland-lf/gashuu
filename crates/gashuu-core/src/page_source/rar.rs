//! A `PageSource` backed by a RAR/CBR archive. Lock-free across reads: each
//! `read_bytes` opens its OWN `unrar` handle and sequentially skips forward to
//! the target entry, so rayon prefetch threads decompress entries in parallel
//! with no shared mutable state. Resident RAM per in-flight read is a single
//! entry's bytes (multiple reads in flight under prefetch each own one buffer).
//!
//! WHY sequential-skip (unlike `ZipSource`'s `by_index` random access): the RAR
//! format has no central directory of random-access offsets the `unrar` crate
//! exposes. The `unrar` typestate read path is strictly front-to-back —
//! `read_header()` yields the next header, and each header is either `skip()`ped
//! or `read()`. So we record each image entry's 0-based position in the FULL
//! sequential header stream (`seq_index`, counting dirs/non-images too) at open
//! time, then re-walk from the front on every read, skipping until we reach it.
//! Trade reopen + walk cost for parallelism and bounded memory (same philosophy
//! as `ZipSource` reopening the file per read).
//!
//! The external `unrar` crate is referenced as `::unrar::` throughout for
//! clarity even though the local module name (`rar`) does not collide with it.

use super::naming::{enclosed_name, has_image_ext, natural_cmp};
use super::zip::MAX_ENTRY_BYTES;
use super::{PageEntry, PageSource};
use crate::error::CoreError;
use std::path::{Path, PathBuf};

/// Metadata for one image page resolved from the archive's sequential header
/// stream during the listing pass.
struct EntryMeta {
    /// 0-based position in the archive's FULL sequential header stream (counts
    /// directories and non-image entries too), so a read can `skip()` exactly
    /// that many headers to reach this entry.
    seq_index: usize,
    /// Safe display name resolved via `enclosed_name` (zip-slip rejected). Holds
    /// the full flattened path (e.g. `sub/3.png`), which is also the natural-sort
    /// key, so nested pages order intuitively against top-level ones.
    name: String,
}

/// A `PageSource` over a RAR/CBR file: scanned once at `open` to build a stable,
/// naturally ordered list of image entries; each read re-opens the archive and
/// walks forward so reads never contend on a shared handle.
// Intentionally does NOT derive `Debug` (matches `ZipSource`/`FolderSource`;
// `Arc<dyn PageSource>` is not `Debug` either).
pub struct RarSource {
    path: PathBuf,
    entries: Vec<EntryMeta>,
    skipped: usize,
}

impl RarSource {
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
        let archive = ::unrar::Archive::new(&path).open_for_listing()?;
        let mut entries = Vec::new();
        let mut skipped = 0usize;
        for (seq_index, header) in archive.enumerate() {
            let header = header?;
            if header.is_directory() {
                continue; // directories are expected, not skips
            }
            // `enclosed_name` == None => the name escapes the base dir (zip-slip),
            // is absolute, or otherwise unsafe to materialize as a path.
            let Some(safe) = enclosed_name(&header.filename) else {
                if has_image_ext(&header.filename) {
                    // Surface only image-looking malicious entries as skips; a
                    // stray unsafe non-image is expected noise, not user-visible.
                    skipped += 1;
                }
                continue;
            };
            // Flatten: any image entry at any depth is a page (CBRs may wrap
            // pages in a folder, unlike FolderSource's max_depth(1)).
            if !has_image_ext(&safe) {
                continue; // non-images are expected, not skips
            }
            if header.unpacked_size > max {
                skipped += 1;
                continue;
            }
            entries.push(EntryMeta {
                seq_index,
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
        // Re-open the archive in processing mode and walk forward to the target
        // entry's sequential position (RAR has no random access — see module doc).
        let mut archive = ::unrar::Archive::new(&self.path).open_for_processing()?;
        let mut seq = 0usize;
        loop {
            let Some(cursor) = archive.read_header()? else {
                // End of archive before reaching `seq_index`: the file changed
                // under us since `open` listed it.
                return Err(CoreError::IndexOutOfRange {
                    index,
                    len: self.entries.len(),
                });
            };
            if seq == meta.seq_index {
                // The declared size was already screened at open time; this
                // re-check defends against a header lying about its size between
                // listing and reading (consistent with ZipSource's two-tier cap).
                if cursor.entry().unpacked_size > max {
                    return Err(CoreError::EntryTooLarge {
                        name: meta.name.clone(),
                        max,
                    });
                }
                // `read()` returns (decompressed bytes, rest-of-archive) — bytes
                // come FIRST in the tuple.
                let (data, _rest) = cursor.read()?;
                return Ok(data);
            }
            archive = cursor.skip()?;
            seq += 1;
        }
    }
}

impl PageSource for RarSource {
    fn list_pages(&self) -> Vec<PageEntry> {
        self.entries
            .iter()
            .map(|m| PageEntry {
                // RAR entries have no real filesystem path; the flattened entry
                // name doubles as both the identity path and the display name.
                path: PathBuf::from(&m.name),
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
    use base64::Engine;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Canonical RAR4 store-format `.cbr` fixture (529 bytes), generated by a
    /// hand-written RAR4 store generator and verified against the real `unrar`
    /// crate (0.5.8). RAR has no Rust encoder, so this committed *text* mirrors
    /// the insta `.snap` "committed text, not a binary fixture" exception.
    ///
    /// Entries in archive/insertion order: `1.png`(74B), `2.png`(74B),
    /// `10.png`(74B), `notes.txt`(12B), `sub/3.png`(74B). The four PNGs are valid
    /// 2x2 images; `notes.txt` is the non-image ASCII string "not an image".
    /// Provenance: `.claude/plans/pr7-fixture.md`.
    const SAMPLE_CBR_B64: &str = "UmFyIRoHAM+QcwAADQAAAAAAAABZOHQAgCUASgAAAEoAAAADEUpj3gAAoU4UMAUAIAAAADEucG5niVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEUlEQVR42mP4z8DA8B+MgBgAHfAD/a4/4jgAAAAASUVORK5CYIKJQnQAgCUASgAAAEoAAAADEUpj3gAAoU4UMAUAIAAAADIucG5niVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEUlEQVR42mP4z8DA8B+MgBgAHfAD/a4/4jgAAAAASUVORK5CYIK6h3QAgCYASgAAAEoAAAADEUpj3gAAoU4UMAYAIAAAADEwLnBuZ4lQTkcNChoKAAAADUlIRFIAAAACAAAAAggCAAAA/dSacwAAABFJREFUeNpj+M/AwPAfjIAYAB3wA/2uP+I4AAAAAElFTkSuQmCCBmd0AIApAAwAAAAMAAAAAy/dsscAAKFOFDAJACAAAABub3Rlcy50eHRub3QgYW4gaW1hZ2UqM3QAgCkASgAAAEoAAAADEUpj3gAAoU4UMAkAIAAAAHN1Yi8zLnBuZ4lQTkcNChoKAAAADUlIRFIAAAACAAAAAggCAAAA/dSacwAAABFJREFUeNpj+M/AwPAfjIAYAB3wA/2uP+I4AAAAAElFTkSuQmCCBLB7AAAHAA==";

    /// Decode the base64 fixture and write it to a `.cbr` tempfile. The returned
    /// `NamedTempFile` is kept alive by the caller so the path stays valid.
    fn sample_cbr() -> NamedTempFile {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(SAMPLE_CBR_B64)
            .expect("fixture base64 must decode");
        let tmp = tempfile::Builder::new().suffix(".cbr").tempfile().unwrap();
        // Reopen the handle for writing without consuming the NamedTempFile so it
        // isn't deleted while we still hold the path.
        let mut file = tmp.reopen().unwrap();
        file.write_all(&bytes).unwrap();
        file.flush().unwrap();
        tmp
    }

    fn names(src: &RarSource) -> Vec<String> {
        src.list_pages().into_iter().map(|p| p.name).collect()
    }

    #[test]
    fn lists_images_only_flattened_in_natural_order() {
        let cbr = sample_cbr();
        let src = RarSource::open(cbr.path()).unwrap();

        // Build the expected order by applying the real `natural_cmp` to the
        // surviving image names, so the test pins "natural order" rather than a
        // hardcoded guess about how the `sub/` prefix sorts. `notes.txt` is
        // excluded (not an image); `sub/3.png` is kept (flattening).
        let mut expected = vec!["1.png", "2.png", "10.png", "sub/3.png"];
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
    fn read_bytes_round_trips_and_decodes() {
        let cbr = sample_cbr();
        let src = RarSource::open(cbr.path()).unwrap();
        assert!(!src.list_pages().is_empty());

        for i in 0..src.list_pages().len() {
            let bytes = src.read_bytes(i).unwrap();
            // The bytes must remain decodable through the normal decode path.
            let decoded = crate::image_ops::decode(&bytes).unwrap();
            assert_eq!(
                (decoded.width(), decoded.height()),
                (2, 2),
                "page {i} must decode to a 2x2 image"
            );
        }
    }

    #[test]
    fn read_bytes_out_of_range_errors() {
        let cbr = sample_cbr();
        let src = RarSource::open(cbr.path()).unwrap();
        let len = src.list_pages().len();

        // `RarSource` does not implement `Debug`, so `unwrap_err` is unavailable;
        // match on the result to extract the error instead.
        let Err(e) = src.read_bytes(len) else {
            panic!("expected an out-of-range error reading past the last page");
        };
        assert!(
            matches!(e, CoreError::IndexOutOfRange { index, len: l } if index == len && l == len)
        );
    }

    #[test]
    fn open_skips_entries_over_declared_size_limit() {
        // A 10-byte ceiling at open time skips every 74-byte PNG (declared-size
        // tier), leaving no pages. `notes.txt` is filtered by extension before
        // the size check, so it does not inflate the count.
        let cbr = sample_cbr();
        let src = RarSource::open_with_limit(cbr.path(), 10).unwrap();
        assert!(src.list_pages().is_empty());
        assert_eq!(src.skipped_count(), 4);
    }

    #[test]
    fn read_entry_over_actual_size_limit_errors() {
        // Open with the default limit so entries pass open-time indexing, then
        // read through a tiny per-read cap to exercise the read-time tier.
        let cbr = sample_cbr();
        let src = RarSource::open(cbr.path()).unwrap();
        assert!(!src.list_pages().is_empty());

        let Err(e) = src.read_entry(0, 10) else {
            panic!("expected EntryTooLarge reading under a tiny per-read cap");
        };
        assert!(matches!(e, CoreError::EntryTooLarge { max: 10, .. }));
    }

    #[test]
    fn open_corrupt_archive_errors() {
        let mut tmp = tempfile::Builder::new().suffix(".cbr").tempfile().unwrap();
        tmp.write_all(b"this is not a rar archive at all, just garbage bytes")
            .unwrap();
        tmp.flush().unwrap();

        let Err(e) = RarSource::open(tmp.path()) else {
            panic!("expected a Rar error opening a corrupt archive");
        };
        assert!(matches!(e, CoreError::Rar(_)));
        // The Display prefix is the typed-error contract surfaced to the UI.
        assert!(e.to_string().starts_with("rar archive error: "));
    }

    #[test]
    fn open_missing_file_errors_as_rar() {
        // Unlike `ZipSource` (which opens the file itself and surfaces a
        // `CoreError::Io` on a missing path), `unrar` opens the file internally
        // and reports its own EOPEN failure, so a missing file surfaces as
        // `CoreError::Rar`, not `CoreError::Io`.
        let Err(e) = RarSource::open("/nonexistent-xyz.cbr") else {
            panic!("expected a Rar error opening a missing file");
        };
        assert!(matches!(e, CoreError::Rar(_)));
    }
}
