//! Book fingerprints and persistent storage for covers and strip thumbnails.

use crate::{BookId, CoreError, DecodedImage};
use std::path::Path;

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// The FNV-1a prefix shared by all thumbnails for one version of a book.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BookFingerprint(u64);

impl BookFingerprint {
    pub fn of_path_and_mtime(path: &Path, mtime_secs: i64) -> Self {
        let hash = fnv1a(FNV_OFFSET_BASIS, path.as_os_str().as_encoded_bytes());
        Self(fnv1a(hash, &mtime_secs.to_le_bytes()))
    }

    /// Fingerprint for an immutable book identified by its UTF-8 ID.
    pub fn of_id(id: &BookId) -> Self {
        Self(fnv1a(FNV_OFFSET_BASIS, id.as_str().as_bytes()))
    }

    /// Stable FNV-1a key for a cover at the requested size.
    pub fn cover_key(self, max_side: u32) -> String {
        let hash = fnv1a(self.0, &max_side.to_le_bytes());
        format!("{hash:016x}")
    }

    /// Stable key for a strip thumbnail, including the native-width page index.
    pub fn page_key(self, max_side: u32, page_index: usize) -> String {
        let hash = fnv1a(self.0, &max_side.to_le_bytes());
        let hash = fnv1a(hash, &page_index.to_le_bytes());
        format!("{hash:016x}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThumbSlot {
    Cover,
    Page(usize),
}

#[cfg(not(target_arch = "wasm32"))]
impl ThumbSlot {
    pub(crate) fn key(self, fp: BookFingerprint, max_side: u32) -> String {
        match self {
            Self::Cover => fp.cover_key(max_side),
            Self::Page(i) => fp.page_key(max_side, i),
        }
    }
}

/// Result of one [`ThumbStore::clear`] sweep. The cache is best-effort:
/// these counts describe entries actually unlinked, not merely discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClearCacheReport {
    /// Cache-owned files actually unlinked by the clear operation.
    pub removed_files: usize,
    /// Sum of the unlinked files' sizes, in bytes.
    pub removed_bytes: u64,
}

/// Result of one [`ThumbStore::prune`] sweep. A named struct (not a tuple)
/// so call sites read; the caller (UI) logs these — core stays log-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PruneReport {
    /// Files actually unlinked by the sweep.
    pub removed_files: usize,
    /// Sum of the unlinked files' sizes, in bytes.
    pub removed_bytes: u64,
    /// Post-sweep total of the surviving `*.qoi` payload, in bytes — lets the
    /// caller log "under the cap" without re-scanning the directory.
    pub retained_bytes: u64,
}

/// Best-effort persistent thumbnail storage. Implementations handle failures
/// without panicking. Reads miss on any failure and cleanup counts only files
/// actually removed.
pub trait ThumbStore: Send + Sync {
    /// Resolve the book's current version, including its mtime on desktop.
    fn fingerprint(&self, id: &BookId) -> BookFingerprint;
    fn get(&self, fp: BookFingerprint, slot: ThumbSlot, max_side: u32) -> Option<DecodedImage>;
    fn put(
        &self,
        fp: BookFingerprint,
        slot: ThumbSlot,
        max_side: u32,
        img: &DecodedImage,
    ) -> Result<(), CoreError>;
    fn purge_cover(&self, fp: BookFingerprint, max_sides: &[u32]) -> usize;
    fn purge_pages(&self, fp: BookFingerprint, max_sides: &[u32], page_count: usize) -> usize;
    fn clear(&self) -> ClearCacheReport;
    fn prune(&self, max_bytes: u64) -> PruneReport;
}

/// A store for platforms without persistent thumbnail storage.
pub struct NullThumbStore;

impl ThumbStore for NullThumbStore {
    fn fingerprint(&self, id: &BookId) -> BookFingerprint {
        BookFingerprint::of_id(id)
    }

    fn get(&self, _: BookFingerprint, _: ThumbSlot, _: u32) -> Option<DecodedImage> {
        None
    }

    fn put(
        &self,
        _: BookFingerprint,
        _: ThumbSlot,
        _: u32,
        _: &DecodedImage,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    fn purge_cover(&self, _: BookFingerprint, _: &[u32]) -> usize {
        0
    }

    fn purge_pages(&self, _: BookFingerprint, _: &[u32], _: usize) -> usize {
        0
    }

    fn clear(&self) -> ClearCacheReport {
        ClearCacheReport::default()
    }

    fn prune(&self, _: u64) -> PruneReport {
        PruneReport::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_key_is_stable_for_same_inputs() {
        let path = Path::new("/tmp/book/page-001.png");
        let fp = BookFingerprint::of_path_and_mtime(path, 1234);
        let key1 = fp.cover_key(160);
        let key2 = fp.cover_key(160);

        assert_eq!(key1, key2);
        assert_eq!(key1.len(), 16);
    }

    #[test]
    fn cover_key_differs_on_mtime() {
        let path = Path::new("/tmp/book/page-001.png");

        let key1 = BookFingerprint::of_path_and_mtime(path, 1234).cover_key(160);
        let key2 = BookFingerprint::of_path_and_mtime(path, 1235).cover_key(160);

        assert_ne!(key1, key2);
    }

    #[test]
    fn cover_key_differs_on_max_side() {
        let path = Path::new("/tmp/book/page-001.png");

        let fp = BookFingerprint::of_path_and_mtime(path, 1234);
        let key1 = fp.cover_key(160);
        let key2 = fp.cover_key(161);

        assert_ne!(key1, key2);
    }

    #[test]
    fn cover_key_differs_on_path() {
        let key1 = BookFingerprint::of_path_and_mtime(Path::new("/tmp/book/page-001.png"), 1234)
            .cover_key(160);
        let key2 = BookFingerprint::of_path_and_mtime(Path::new("/tmp/book/page-002.png"), 1234)
            .cover_key(160);

        assert_ne!(key1, key2);
    }

    #[test]
    fn page_key_is_stable_for_same_inputs() {
        let fp = BookFingerprint::of_path_and_mtime(Path::new("/manga/book.cbz"), 1234);
        let key1 = fp.page_key(160, 7);
        let key2 = fp.page_key(160, 7);

        assert_eq!(key1, key2);
        assert_eq!(key1.len(), 16);
    }

    #[test]
    fn page_key_differs_on_page_index() {
        let fp = BookFingerprint::of_path_and_mtime(Path::new("/manga/book.cbz"), 1234);
        let key0 = fp.page_key(160, 0);
        let key1 = fp.page_key(160, 1);

        assert_ne!(key0, key1, "different pages must not share a strip key");
    }

    #[test]
    fn page_key_is_disjoint_from_cover_key() {
        // A strip thumbnail folds the page index even at page 0, so it never
        // collides with the cover key at the same (path, mtime, max_side).
        let fp = BookFingerprint::of_path_and_mtime(Path::new("/manga/book.cbz"), 1234);
        let cover = fp.cover_key(160);
        let page0 = fp.page_key(160, 0);

        assert_ne!(cover, page0, "strip keys must be disjoint from cover keys");
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn fingerprint_keys_match_pinned_golden_values() {
        // Legacy outputs on 64-bit targets, where a page index contributes eight bytes.
        let fp = BookFingerprint::of_path_and_mtime(Path::new("/books/a.cbz"), 1_700_000_000);
        assert_eq!(fp.cover_key(512), "8888a774ce848046");
        assert_eq!(fp.page_key(160, 3), "6c298ddf8718b0b3");

        let cases = [
            (
                "book.cbz",
                0,
                160,
                "4b2bc7a601954c4b",
                ["a03203db845183ab", "81373cd27962398a", "687c32b6f3e9f023"],
            ),
            (
                "book.cbz",
                0,
                512,
                "592a42dbecfb2441",
                ["4bd70c0487897661", "2cdc44fb7c9a2c40", "b4aea77491a44cd9"],
            ),
            (
                "book.cbz",
                -19,
                160,
                "d6f712f5121481d1",
                ["3e90fe619e4a25f1", "1f963758935adbd0", "3c166e0ce030e869"],
            ),
            (
                "book.cbz",
                -19,
                512,
                "c24fa22ae9da5f1b",
                ["27507eaa1cd9107b", "0855b7a111e9c65a", "58eea7f1819a46f3"],
            ),
            (
                "book.cbz",
                i64::MAX,
                160,
                "bb8ce6e3c86874c3",
                ["7b41793aa7446b23", "5c46b2319c552102", "b91c6d34ef53a59b"],
            ),
            (
                "book.cbz",
                i64::MAX,
                512,
                "c98b6219b3ce4cb9",
                ["26e68163aa7c5dd9", "07ebba5a9f8d13b8", "321947ce709dda51"],
            ),
            (
                "/本/本.cbz",
                0,
                160,
                "64244d9c33cce0b6",
                ["d094e58ecc479376", "ef8fac97d736dd97", "5d89a589d7e4bfee"],
            ),
            (
                "/本/本.cbz",
                0,
                512,
                "4f7cdcd20b92be00",
                ["b95465d74ad67e00", "d84f2ce055c5c821", "7a61df6e794e1e78"],
            ),
            (
                "/本/本.cbz",
                -19,
                160,
                "17d46da89d51f37c",
                ["fbe3e69720b63afc", "1adeada02ba5851d", "c0a2f923e7d48374"],
            ),
            (
                "/本/本.cbz",
                -19,
                512,
                "2c7bde72c58c1632",
                ["1324664ea2275072", "321f2d57ad169a93", "4d2ed14b5ac862ea"],
            ),
            (
                "/本/本.cbz",
                i64::MAX,
                160,
                "0313372b907a312e",
                ["a42fd8fea7e9a2ee", "c32aa007b2d8ed0f", "9c43702e6a91df66"],
            ),
            (
                "/本/本.cbz",
                i64::MAX,
                512,
                "ee6bc66168400e78",
                ["8cef594726788d78", "abea20503167d799", "bcc197fd27a0a9f0"],
            ),
            (
                "/books/with spaces.cbz",
                0,
                160,
                "04f373d788c6d6cf",
                ["fc8d289fbcfa86af", "dd926196b20b3c8e", "1e8dac7985555f27"],
            ),
            (
                "/books/with spaces.cbz",
                0,
                512,
                "12f1ef0d742caec5",
                ["a83230c8c0327965", "893769bfb5432f44", "a23076669b230bdd"],
            ),
            (
                "/books/with spaces.cbz",
                -19,
                160,
                "409365c67fc79955",
                ["0cf20dd8a36855f5", "edf746cf98790bd4", "e18d35d0588c826d"],
            ),
            (
                "/books/with spaces.cbz",
                -19,
                512,
                "313feca621d0e59f",
                ["b22dedc653be8f7f", "933326bd48cf455e", "36b66c0c0b7bdff7"],
            ),
            (
                "/books/with spaces.cbz",
                i64::MAX,
                160,
                "7b9a06e1b0f49847",
                ["14c46e0a73d52727", "f5c9a70168e5dd06", "a1e3c918f33b0f9f"],
            ),
            (
                "/books/with spaces.cbz",
                i64::MAX,
                512,
                "899882179c5a703d",
                ["c0697633770d19dd", "a16eaf2a6c1dcfbc", "1ebc93060345d055"],
            ),
        ];
        for (path, mtime, side, cover, pages) in cases {
            let fp = BookFingerprint::of_path_and_mtime(Path::new(path), mtime);
            assert_eq!(fp.cover_key(side), cover, "{path:?}, {mtime}, {side}");
            for (index, expected) in [0, 1, usize::MAX].into_iter().zip(pages) {
                assert_eq!(
                    fp.page_key(side, index),
                    expected,
                    "{path:?}, {mtime}, {side}, page {index}"
                );
            }
        }
    }
}
