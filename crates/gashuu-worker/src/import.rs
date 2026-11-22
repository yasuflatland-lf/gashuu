//! Portable rules used by the OPFS importer.

use std::path::Path;

pub const STORAGE_FULL: &str = "browser storage is full";
pub const UNREADABLE_UPLOAD: &str = "could not read selected file";

pub fn fits_in_quota(size: u64, usage: Option<u64>, quota: Option<u64>) -> bool {
    usage
        .zip(quota)
        .is_none_or(|(usage, quota)| size <= quota.saturating_sub(usage))
}

pub fn sanitise_upload_name(name: &str) -> String {
    const MAX_BYTES: usize = 120;
    let basename = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = basename.chars().filter(|c| !c.is_control()).collect();
    let clean = clean
        .trim_start_matches(|c: char| c == '.' || c.is_whitespace())
        .trim_end();
    if clean.is_empty() {
        return "book.cbz".into();
    }
    if clean.len() <= MAX_BYTES {
        return clean.into();
    }

    // Retain the extension when it fits alongside at least one stem character.
    if let Some((stem, extension)) = clean.rsplit_once('.') {
        let suffix_len = extension.len() + 1;
        if suffix_len < MAX_BYTES {
            let end = stem.floor_char_boundary(MAX_BYTES - suffix_len);
            if end > 0 {
                return format!("{}.{extension}", &stem[..end]);
            }
        }
    }
    clean[..clean.floor_char_boundary(MAX_BYTES)].into()
}

pub fn upload_title(sanitised: &str) -> String {
    gashuu_core::display_title(Path::new(sanitised), false)
}

/// Use the archive signature to catch RAR files renamed to an accepted extension.
pub fn probe_upload(
    name: &str,
    reader: std::sync::Arc<dyn gashuu_core::ReadAt>,
    policy: gashuu_core::ArchivePolicy,
) -> Result<std::num::NonZeroUsize, crate::wire_error::WireError> {
    use gashuu_core::{ArchiveKind, ArchiveLoader, CoreError};
    let mut head = [0; 6];
    let bytes = reader.read_at(0, &mut head).map_err(CoreError::from)?;
    if ArchiveLoader::detect_kind("", &head[..bytes]) == Some(ArchiveKind::Rar) {
        return Err(CoreError::FormatDisabled { format: "rar/cbr" }.into());
    }
    ArchiveLoader::probe_page_count_read_at(name, reader, policy).map_err(Into::into)
}
