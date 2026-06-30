//! Crash-safe file writes via the write-temp-then-rename pattern.
//!
//! Both persisted JSON documents (`settings.json`, `library.json`) are rewritten
//! in full on every save. A direct `std::fs::write` can leave the target file
//! truncated or half-written if the process dies (or the disk fills) mid-write,
//! corrupting state that loads fine until then. `write_atomic` instead writes the
//! new bytes to a temporary file in the SAME directory, fsyncs them to disk, and
//! then atomically renames it over the target. The rename either fully replaces
//! the previous contents or does nothing, so a reader never observes a partial
//! file. The temp file is created in the same directory as the target so the
//! rename stays on a single filesystem; cross-device renames would fail with
//! `EXDEV`.
//!
//! This module owns parent-directory creation, so call sites must NOT also call
//! `create_dir_all` for the target's parent (single owner of that invariant).

use crate::error::CoreError;
use std::io::Write;
use std::path::Path;

/// Atomically write `bytes` to `path`, creating parent directories as needed.
///
/// Steps: resolve the parent directory (current dir if `path` has none) and
/// `create_dir_all` it; create a `NamedTempFile` in that same directory; write
/// all bytes; flush and `sync_all` the file's data to disk; persist (rename) it
/// over `path`; then best-effort fsync the parent directory so the rename itself
/// is durable. All I/O failures surface as `CoreError::Io`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    // Defensive flush before fsync (no-op for NamedTempFile but harmless).
    tmp.flush()?;
    tmp.as_file().sync_all()?;

    // Rename the temp file over `path`. `PersistError` wraps the underlying
    // `io::Error`; extract it so callers see a uniform `CoreError::Io`.
    tmp.persist(path).map_err(|e| CoreError::Io(e.error))?;

    // Best-effort: fsync the directory so the rename is durable across a crash.
    // A failure here does not invalidate the freshly-renamed target, so it is
    // intentionally ignored rather than surfaced as an error.
    let _ = std::fs::File::open(parent).and_then(|d| d.sync_all());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_bytes_to_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        write_atomic(&path, b"hello world").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello world");
    }

    #[test]
    fn creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("c").join("out.txt");
        write_atomic(&path, b"nested").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"nested");
    }

    #[test]
    fn overwrites_existing_file_completely() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        // Seed a LONGER previous file so a truncating/partial write would leave a
        // trailing tail; a full atomic replace must drop it entirely.
        write_atomic(&path, b"AAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
        write_atomic(&path, b"BBB").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"BBB");
    }

    #[test]
    fn leaves_no_leftover_temp_files_in_target_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        write_atomic(&path, b"x").unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "only the target file should remain, found: {entries:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn returns_io_error_for_read_only_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ro = dir.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = write_atomic(&ro.join("out.json"), b"x");
        assert!(matches!(result, Err(CoreError::Io(_))));
    }
}
