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

use super::naming::{enclosed_name, has_image_ext, is_macos_metadata, MAX_ENTRY_BYTES};
use super::{PageEntry, PageSource};
use crate::error::CoreError;
use crate::ordering::natural_cmp;
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
            let header = match header {
                Ok(h) => h,
                Err(_) => {
                    // unrar's List iterator is non-resumable: after any per-entry error it
                    // sets `damaged` and yields None forever, so (unlike ZipSource's random-
                    // access by_index skip+count) we cannot skip an interior bad entry and
                    // continue. Surface the good pages already indexed and count the failure,
                    // matching the project's skip+count policy as far as the format allows
                    // (we can only drop the trailing remainder, not an interior entry).
                    skipped += 1;
                    break;
                }
            };
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
            // macOS resource forks (`__MACOSX/.../._x.jpg`) and dotfiles carry
            // image extensions and sort ahead of real pages via case-insensitive
            // ordering, so they can masquerade as page 0. Treat them as expected
            // noise like directories — drop without counting as a skip. Mirrors
            // the identical filter in `ZipSource` so the page-membership rule is
            // shared across both archive sources.
            if is_macos_metadata(&safe) {
                continue;
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

    /// Read the bytes of page `index`, re-validating the entry's declared size
    /// against `max` before materializing it.
    ///
    /// Honesty note (NOT parity with `ZipSource`'s two-tier cap): `unrar`'s
    /// `read()` materializes the whole entry into a `Vec` with no streaming
    /// `take`, so this read-time check only RE-VALIDATES the declared
    /// `unpacked_size` — it guards against the entry changing between the listing
    /// and reading passes, NOT against a header that under-reports its size.
    /// That residual risk is bounded only by `image::Limits` in
    /// `image_ops::decode`.
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
                // Postcondition guard: the reached entry's safe name MUST match
                // the name recorded for this `seq_index` during listing. If the
                // two passes ever desynchronize we would silently return the
                // wrong page's bytes; turn that into a loud dev/test failure
                // instead. (A true invariant check, not a legitimate zero-path.)
                debug_assert_eq!(
                    enclosed_name(&cursor.entry().filename)
                        .map(|p| p.to_string_lossy().into_owned())
                        .as_deref(),
                    Some(meta.name.as_str()),
                    "seq_index desynchronized between listing and processing passes",
                );
                // The declared size was already screened at open time; this
                // re-validates the declared `unpacked_size` against `max` in case
                // the entry changed between the listing and reading passes (see
                // the `read_entry` doc — this is NOT a streaming `take` cap).
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
                // RAR entries have no filesystem path; the flattened entry name is
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
    use crate::test_fixtures::{
        write_cbr, CORRUPT_TRAILING_CBR_B64, HOSTILE_CBR_B64, MACOS_METADATA_CBR_B64,
        SAMPLE_CBR_B64,
    };
    use std::io::Write;

    fn names(src: &RarSource) -> Vec<String> {
        src.list_pages().into_iter().map(|p| p.name).collect()
    }

    /// Fixture A: image pages are listed flattened (nested `sub/3.png` kept) in
    /// natural order; `notes.txt` and the `sub/` directory header are excluded,
    /// and nothing is skipped.
    #[test]
    fn lists_images_only_flattened_in_natural_order() {
        let cbr = write_cbr(SAMPLE_CBR_B64);
        let src = RarSource::open(cbr.path()).unwrap();

        // Derive the expected order by applying the real `natural_cmp` to the
        // surviving image names, so the test pins "natural order" rather than a
        // hardcoded guess about how the `sub/` prefix sorts. `notes.txt` is
        // excluded (not an image); `sub/3.png` is kept (flattening).
        let mut expected = vec!["1.png", "2.png", "10.png", "sub/3.png"];
        expected.sort_by(|a, b| natural_cmp(a, b));
        let expected: Vec<String> = expected.into_iter().map(String::from).collect();

        assert_eq!(names(&src), expected);
        assert_eq!(src.skipped_count(), 0);
        // Sanity: neither the non-image nor the directory header leaks in as a page.
        assert!(!names(&src).iter().any(|n| n == "notes.txt"));
        assert!(!names(&src).iter().any(|n| n == "sub"));
        // Sanity: the nested image survives flattening.
        assert!(names(&src).iter().any(|n| n == "sub/3.png"));
    }

    /// Round-trip IDENTITY: each page index reads back the entry whose DISTINCT
    /// dimensions prove the exact `seq_index` → content mapping. Reading index 3
    /// (`sub/3.png`, archive seq 5) also proves the read walk correctly skips
    /// past `notes.txt` and the `sub/` directory header.
    #[test]
    fn read_bytes_round_trips_to_expected_dimensions() {
        let cbr = write_cbr(SAMPLE_CBR_B64);
        let src = RarSource::open(cbr.path()).unwrap();

        // (page index, expected decoded WxH) — distinct per page on purpose.
        let expected = [(0, (2, 2)), (1, (2, 3)), (2, (3, 2)), (3, (4, 4))];
        assert_eq!(src.list_pages().len(), expected.len());
        for (i, (w, h)) in expected {
            let bytes = src.read_bytes(i).unwrap();
            let decoded = crate::image_ops::decode(&bytes).unwrap();
            assert_eq!(
                (decoded.width(), decoded.height()),
                (w, h),
                "page {i} must decode to a {w}x{h} image"
            );
        }
    }

    /// macOS metadata inside a RAR is never enumerated as a page. The
    /// AppleDouble resource fork (`__MACOSX/Manga/._001.jpg`) carries a `.jpg`
    /// name and sorts AHEAD of the real pages via case-insensitive natural
    /// ordering, so without the filter it would become an undecodable page 0.
    /// Mirrors `ZipSource::macos_metadata_entries_are_excluded_without_counting`.
    #[test]
    fn macos_metadata_entries_are_excluded_without_counting() {
        let cbr = write_cbr(MACOS_METADATA_CBR_B64);
        let src = RarSource::open(cbr.path()).unwrap();

        let listed = names(&src);
        // (a) the metadata entry is entirely absent from the page list.
        assert!(!listed.iter().any(|n| n.contains("__MACOSX")));
        assert!(!listed.iter().any(|n| n.contains("._")));
        // Only the real images survive, in natural order.
        assert_eq!(
            listed,
            vec!["Manga/001.jpg".to_string(), "Manga/002.jpg".to_string()]
        );
        // (b) page 0 is the real first page, not the resource fork.
        assert_eq!(listed[0], "Manga/001.jpg");
        // The surviving page still decodes.
        let decoded = crate::image_ops::decode(&src.read_bytes(0).unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
        // (c) metadata is expected noise, not a skip.
        assert_eq!(src.skipped_count(), 0);
    }

    /// zip-slip via `RarSource`: the two `..` traversal entries are excluded from
    /// the page list; only the image-looking one is counted as skipped.
    #[test]
    fn traversal_entries_are_skipped_and_counted() {
        let cbr = write_cbr(HOSTILE_CBR_B64);
        let src = RarSource::open(cbr.path()).unwrap();

        assert_eq!(names(&src), vec!["1.png".to_string()]);
        // `../evil.png` (image-looking) is counted; `../readme.txt` (non-image)
        // is silently excluded, NOT counted.
        assert_eq!(src.skipped_count(), 1);
        // The surviving safe page still decodes.
        let decoded = crate::image_ops::decode(&src.read_bytes(0).unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
    }

    /// skip+count+break: a corrupt trailing header does NOT fail the whole open.
    /// The good page already indexed survives, the failure is counted, and the
    /// good page still reads back.
    #[test]
    fn corrupt_trailing_header_surfaces_good_pages_and_counts_the_failure() {
        let cbr = write_cbr(CORRUPT_TRAILING_CBR_B64);
        let src = RarSource::open(cbr.path()).unwrap();

        assert_eq!(names(&src), vec!["1.png".to_string()]);
        assert_eq!(src.skipped_count(), 1);
        let decoded = crate::image_ops::decode(&src.read_bytes(0).unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
    }

    #[test]
    fn read_bytes_out_of_range_errors() {
        let cbr = write_cbr(SAMPLE_CBR_B64);
        let src = RarSource::open(cbr.path()).unwrap();
        let len = src.list_pages().len();
        assert_eq!(len, 4);

        // `RarSource` does not implement `Debug`, so `unwrap_err` is unavailable;
        // match on the result to extract the error instead.
        let Err(e) = src.read_bytes(len) else {
            panic!("expected an out-of-range error reading past the last page");
        };
        assert!(matches!(e, CoreError::IndexOutOfRange { index: 4, len: 4 }));
    }

    #[test]
    fn open_skips_entries_over_declared_size_limit() {
        // A 10-byte ceiling at open time skips every PNG (declared-size tier),
        // leaving no pages. `notes.txt` and the `sub/` directory header are
        // filtered before the size check, so they do not inflate the count.
        let cbr = write_cbr(SAMPLE_CBR_B64);
        let src = RarSource::open_with_limit(cbr.path(), 10).unwrap();
        assert!(src.list_pages().is_empty());
        assert_eq!(src.skipped_count(), 4);
    }

    #[test]
    fn read_entry_over_actual_size_limit_errors() {
        // Open with the default limit so entries pass open-time indexing, then
        // read index 3 (`sub/3.png`, archive seq 5) through a tiny per-read cap
        // — pairing the read-time ceiling with the sequential skip walk.
        let cbr = write_cbr(SAMPLE_CBR_B64);
        let src = RarSource::open(cbr.path()).unwrap();
        assert!(!src.list_pages().is_empty());

        let Err(e) = src.read_entry(3, 10) else {
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
        // `unrar` opens the file itself and reports its own open failure, so a
        // missing file surfaces as `CoreError::Rar`, not `CoreError::Io` (unlike
        // `ZipSource`, which opens the file and would surface `CoreError::Io`).
        let Err(e) = RarSource::open("/nonexistent-xyz.cbr") else {
            panic!("expected a Rar error opening a missing file");
        };
        assert!(matches!(e, CoreError::Rar(_)));
    }
}
