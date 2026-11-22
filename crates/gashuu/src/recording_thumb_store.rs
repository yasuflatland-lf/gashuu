use gashuu_core::{
    BookFingerprint, BookId, ClearCacheReport, CoreError, DecodedImage, FsThumbStore, PruneReport,
    ThumbSlot, ThumbStore,
};
use std::sync::Mutex;

#[derive(Debug, PartialEq)]
pub(crate) enum Call {
    PurgeCover(BookFingerprint, Vec<u32>),
    PurgePages(BookFingerprint, Vec<u32>, usize),
    Clear,
}

#[derive(Default)]
pub(crate) struct RecordingThumbStore {
    calls: Mutex<Vec<Call>>,
    disk: Option<FsThumbStore>,
}

impl RecordingThumbStore {
    pub(crate) fn with_disk(disk: FsThumbStore) -> Self {
        Self {
            disk: Some(disk),
            ..Self::default()
        }
    }

    pub(crate) fn take_calls(&self) -> Vec<Call> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

impl ThumbStore for RecordingThumbStore {
    fn fingerprint(&self, id: &BookId) -> BookFingerprint {
        BookFingerprint::of_id(id)
    }

    fn get(&self, _: BookFingerprint, _: ThumbSlot, _: u32) -> Option<DecodedImage> {
        unreachable!()
    }

    fn put(
        &self,
        _: BookFingerprint,
        _: ThumbSlot,
        _: u32,
        _: &DecodedImage,
    ) -> Result<(), CoreError> {
        unreachable!()
    }

    fn purge_cover(&self, fp: BookFingerprint, sides: &[u32]) -> usize {
        self.calls
            .lock()
            .unwrap()
            .push(Call::PurgeCover(fp, sides.to_vec()));
        0
    }

    fn purge_pages(&self, fp: BookFingerprint, sides: &[u32], count: usize) -> usize {
        self.calls
            .lock()
            .unwrap()
            .push(Call::PurgePages(fp, sides.to_vec(), count));
        0
    }

    fn clear(&self) -> ClearCacheReport {
        self.calls.lock().unwrap().push(Call::Clear);
        self.disk
            .as_ref()
            .map(ThumbStore::clear)
            .unwrap_or_default()
    }

    fn prune(&self, _: u64) -> PruneReport {
        unreachable!()
    }
}
