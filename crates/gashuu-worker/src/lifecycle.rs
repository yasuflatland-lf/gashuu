//! Content fingerprints and retention policy for browser-owned book files.

use sha2::{Digest, Sha256};
use std::{collections::HashSet, ops::Range};

pub fn content_key(size: u64, head: &[u8], tail: &[u8]) -> String {
    let digest = Sha256::new()
        .chain_update(size.to_le_bytes())
        .chain_update(head)
        .chain_update(tail)
        .finalize();
    use std::fmt::Write;
    let mut key = String::with_capacity(64);
    for byte in digest {
        write!(key, "{byte:02x}").expect("writing to a string");
    }
    key
}

pub fn head_tail_ranges(size: u64) -> (Range<u64>, Range<u64>) {
    let n = size.min(1 << 20);
    (0..n, size - n..size)
}

pub fn is_recent(last_modified_ms: f64, now_ms: Option<f64>) -> bool {
    now_ms.is_some_and(|now| now - last_modified_ms < 3_600_000.)
}

#[derive(Default)]
pub struct SweepPlan {
    pub delete: Vec<String>,
    pub present: Vec<String>,
}

/// Incrementally collect retained IDs as the worker visits each directory.
#[derive(Default)]
pub struct SweepInventory {
    pub present: Vec<String>,
    pub skipped_recent: u32,
    pub skipped_locked: u32,
}

impl SweepInventory {
    pub fn should_delete(
        &mut self,
        id: Option<&str>,
        modified: f64,
        keep: &HashSet<String>,
        now_ms: Option<f64>,
    ) -> bool {
        let Some(id) = id else { return true };
        if !keep.contains(id) {
            if !is_recent(modified, now_ms) {
                return true;
            }
            self.skipped_recent += 1;
        }
        self.present.push(id.to_owned());
        false
    }

    pub fn locked(&mut self, id: Option<String>) {
        self.skipped_locked += 1;
        self.present.extend(id);
    }
}

pub fn plan_sweep(
    dirs: &[(String, Option<String>, f64)],
    keep: &HashSet<String>,
    now_ms: Option<f64>,
) -> (SweepPlan, u32) {
    let mut inventory = SweepInventory::default();
    let mut delete = Vec::new();
    for (uuid, name, modified) in dirs {
        let id = name.as_ref().map(|name| format!("books/{uuid}/{name}"));
        if inventory.should_delete(id.as_deref(), *modified, keep, now_ms) {
            delete.push(uuid.clone());
        }
    }
    (
        SweepPlan {
            delete,
            present: inventory.present,
        },
        inventory.skipped_recent,
    )
}
