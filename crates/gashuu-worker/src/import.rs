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
    // Retain the extension when it fits alongside at least one stem character.
    let parts = clean.rsplit_once('.').and_then(|(stem, extension)| {
        let stem_budget = MAX_BYTES.checked_sub(extension.len() + 1)?;
        let end = stem.floor_char_boundary(stem_budget);
        (end > 0).then_some((&stem[..end], extension))
    });
    let truncated = clean[..clean.floor_char_boundary(MAX_BYTES)].trim_end();
    if let Some((stem, extension)) = parts.or_else(|| truncated.rsplit_once('.')) {
        let stem = stem.trim_end();
        if stem.is_empty() {
            return "book.cbz".into();
        }
        return format!("{stem}.{extension}");
    }
    truncated.into()
}

pub fn upload_title(sanitised: &str) -> String {
    gashuu_core::display_title(Path::new(sanitised), false)
}

/// Use the archive signature to catch RAR files renamed to an accepted extension.
pub fn probe_upload(
    name: &str,
    reader: std::sync::Arc<dyn gashuu_core::ReadAt>,
) -> Result<std::num::NonZeroUsize, crate::wire_error::WireError> {
    use gashuu_core::{ArchiveKind, ArchiveLoader, CoreError};
    let mut head = [0; 6];
    let bytes = reader.read_at(0, &mut head).map_err(CoreError::from)?;
    if ArchiveLoader::detect_kind("", &head[..bytes]) == Some(ArchiveKind::Rar) {
        return Err(CoreError::FormatDisabled {
            kind: ArchiveKind::Rar,
        }
        .into());
    }
    ArchiveLoader::probe_page_count_read_at(name, reader).map_err(Into::into)
}
