//! Book fingerprints and persistent storage for covers and strip thumbnails.

use crate::{BookId, ClearCacheReport, CoreError, DecodedImage, PruneReport};
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

    pub fn cover_key(self, max_side: u32) -> String {
        let hash = fnv1a(self.0, &max_side.to_le_bytes());
        format!("{hash:016x}")
    }

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

impl ThumbSlot {
    pub(crate) fn key(self, fp: BookFingerprint, max_side: u32) -> String {
        match self {
            Self::Cover => fp.cover_key(max_side),
            Self::Page(i) => fp.page_key(max_side, i),
        }
    }
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
