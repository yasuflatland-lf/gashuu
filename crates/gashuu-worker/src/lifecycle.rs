//! Content fingerprints and retention policy for browser-owned book files.

use sha2::{Digest, Sha256};
use std::{collections::HashSet, ops::Range};

/// Maximum sample size in bytes at each end of a book file (1 MiB).
const SAMPLE_BYTES: u64 = 1 << 20;

/// Retention window for recently modified book files (one hour in milliseconds).
const RECENT_WINDOW_MS: f64 = 3_600_000.;

/// Hash the little-endian file size and its head and tail samples as lowercase SHA-256 hex.
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

/// Return the byte ranges sampled from the beginning and end of the file.
pub fn head_tail_ranges(size: u64) -> (Range<u64>, Range<u64>) {
    let n = size.min(SAMPLE_BYTES);
    (0..n, size - n..size)
}

/// Check whether the modification is within the retention window when a clock is supplied.
pub fn is_recent(last_modified_ms: f64, now_ms: Option<f64>) -> bool {
    now_ms.is_some_and(|now| now - last_modified_ms < RECENT_WINDOW_MS)
}

/// Check for exactly 64 lowercase hexadecimal characters.
pub fn is_content_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
