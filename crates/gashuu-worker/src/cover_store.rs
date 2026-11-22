//! Synchronous staging for the worker's asynchronous OPFS cover storage.
use gashuu_core::{
    BookFingerprint, BookId, ClearCacheReport, CoreError, DecodedImage, PruneReport, ThumbSlot,
    ThumbStore,
};
use std::{collections::HashMap, sync::Mutex};

pub fn cover_file_name(fp: BookFingerprint, max_side: u32) -> String {
    format!("{}.qoi", fp.cover_key(max_side))
}
pub fn plan_cover_prune(files: &[(String, u64, f64)], max_bytes: u64) -> Vec<String> {
    let mut total: u128 = files.iter().map(|(_, size, _)| u128::from(*size)).sum();
    let mut sorted: Vec<_> = files.iter().collect();
    sorted.sort_by(|a, b| a.2.total_cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
    let mut removed = Vec::new();
    for (name, size, _) in sorted {
        if total <= u128::from(max_bytes) {
            break;
        }
        total -= u128::from(*size);
        removed.push(name.clone());
    }
    removed
}
#[derive(Default)]
struct Staged {
    reads: HashMap<String, Vec<u8>>,
    writes: Vec<(String, Vec<u8>)>,
    removals: Vec<String>,
    clear: bool,
}
#[derive(Default)]
pub struct StagedCoverStore(Mutex<Staged>);
impl StagedCoverStore {
    pub fn preload(&self, name: String, bytes: Vec<u8>) {
        self.0.lock().unwrap().reads.insert(name, bytes);
    }
    pub fn forget(&self, name: &str) {
        self.0.lock().unwrap().reads.remove(name);
    }
    pub fn take_writes(&self) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut self.0.lock().unwrap().writes)
    }
    pub fn take_removals(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap().removals)
    }
    pub fn take_clear(&self) -> bool {
        std::mem::take(&mut self.0.lock().unwrap().clear)
    }
}
impl ThumbStore for StagedCoverStore {
    fn fingerprint(&self, id: &BookId) -> BookFingerprint {
        BookFingerprint::of_id(id)
    }
    fn get(&self, fp: BookFingerprint, slot: ThumbSlot, side: u32) -> Option<DecodedImage> {
        if slot != ThumbSlot::Cover {
            return None;
        }
        self.0
            .lock()
            .unwrap()
            .reads
            .get(&cover_file_name(fp, side))
            .and_then(|bytes| gashuu_core::decode(bytes).ok())
    }
    fn put(
        &self,
        fp: BookFingerprint,
        slot: ThumbSlot,
        side: u32,
        img: &DecodedImage,
    ) -> Result<(), CoreError> {
        if slot == ThumbSlot::Cover {
            self.0
                .lock()
                .unwrap()
                .writes
                .push((cover_file_name(fp, side), gashuu_core::encode_qoi(img)?));
        }
        Ok(())
    }
    fn purge_cover(&self, fp: BookFingerprint, sides: &[u32]) -> usize {
        let mut state = self.0.lock().unwrap();
        for side in sides {
            let name = cover_file_name(fp, *side);
            state.reads.remove(&name);
            state.writes.retain(|(n, _)| n != &name);
            state.removals.push(name);
        }
        sides.len()
    }
    fn purge_pages(&self, _: BookFingerprint, _: &[u32], _: usize) -> usize {
        0
    }
    fn clear(&self) -> ClearCacheReport {
        let mut state = self.0.lock().unwrap();
        state.reads.clear();
        state.writes.clear();
        state.removals.clear();
        state.clear = true;
        ClearCacheReport::default()
    }
    fn prune(&self, _: u64) -> PruneReport {
        PruneReport::default()
    }
}
