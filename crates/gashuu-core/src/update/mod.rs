//! Update-check domain logic. Pure and deterministic — no network, no
//! filesystem, no `slint`. The `gashuu` UI crate owns HTTP, download, and
//! self-replacement; this module owns "what should we decide" so it can be
//! unit-tested without side effects.

pub mod asset;
pub mod check;
pub mod packaging;
pub mod release;
pub mod version;
