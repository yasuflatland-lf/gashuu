//! Self-update installation: extract the Windows portable `.exe` from a release
//! `.zip`, replace the running binary in place, and relaunch. All side-effecting;
//! the pure "which packaging → which strategy" decision lives in
//! `gashuu_core::update`. Only the AppImage and Windows-portable forms reach
//! here — macOS `.app` (unsigned + quarantine) and deb (package-manager-owned)
//! are deliberately guided-install only.

use crate::update::UpdateError;
use gashuu_core::Packaging;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

/// Extract the Windows portable executable from a release `.zip` into `out_dir`,
/// returning the written path. The Windows release archive contains a single
/// `gashuu.exe`; we take the first entry whose name ends with `.exe`
/// (case-insensitive) and reduce that name to its final path component so a
/// crafted archive can never write outside `out_dir`.
pub(crate) fn extract_exe_from_zip(
    zip_bytes: &[u8],
    out_dir: &Path,
) -> Result<PathBuf, UpdateError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))
        .map_err(|e| UpdateError::Io(format!("cannot open release zip: {e}")))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| UpdateError::Io(format!("cannot read zip entry {i}: {e}")))?;
        let name = entry.name().to_string();
        if !name.to_ascii_lowercase().ends_with(".exe") {
            continue;
        }
        let file_name = Path::new(&name)
            .file_name()
            .ok_or_else(|| UpdateError::Io("zip .exe entry has no file name".into()))?;
        let dest = out_dir.join(file_name);
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| UpdateError::Io(format!("cannot read zip .exe entry: {e}")))?;
        std::fs::write(&dest, &bytes).map_err(|e| UpdateError::Io(e.to_string()))?;
        return Ok(dest);
    }
    Err(UpdateError::Io("no .exe entry in release zip".into()))
}

/// Replace the running binary in place for a self-replaceable packaging form,
/// returning the executable path to relaunch.
///
/// - **Windows portable**: `verified` is the extracted `.exe`; `self_replace`
///   swaps it over the running `gashuu.exe` (handling the Windows restriction
///   that a running executable cannot be overwritten directly), and the relaunch
///   target is the now-updated `current_exe()`.
/// - **Linux AppImage**: `verified` is the downloaded `.AppImage`. The running
///   executable lives inside a read-only mounted squashfs, so `self_replace`
///   cannot be used; instead we replace the file `$APPIMAGE` points at — stage a
///   sibling `<name>.new`, mark it executable, then atomically rename it over
///   `$APPIMAGE`. The relaunch target is `$APPIMAGE`.
pub(crate) fn apply_self_replace(pkg: Packaging, verified: &Path) -> Result<PathBuf, UpdateError> {
    match pkg {
        Packaging::WindowsPortable => {
            self_replace::self_replace(verified)
                .map_err(|e| UpdateError::Io(format!("self-replace failed: {e}")))?;
            std::env::current_exe().map_err(|e| UpdateError::Io(e.to_string()))
        }
        Packaging::LinuxAppImage => apply_appimage(verified),
        other => Err(UpdateError::Io(format!(
            "self-replace is not supported for {other:?}"
        ))),
    }
}

/// AppImage in-place replace: copy the verified `.AppImage` to `<$APPIMAGE>.new`
/// (a sibling, so the final rename is same-filesystem and atomic), make it
/// executable, then rename it over `$APPIMAGE`.
#[cfg(unix)]
fn apply_appimage(verified: &Path) -> Result<PathBuf, UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let appimage = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .ok_or_else(|| {
            UpdateError::Io("$APPIMAGE is not set; not running as an AppImage".into())
        })?;
    // Copy rather than rename: `verified` may sit on a different mount (the
    // Downloads dir) than `$APPIMAGE`, and a cross-device rename would fail.
    let mut staged = appimage.clone().into_os_string();
    staged.push(".new");
    let staged = PathBuf::from(staged);
    std::fs::copy(verified, &staged).map_err(|e| UpdateError::Io(e.to_string()))?;
    let mut perms = std::fs::metadata(&staged)
        .map_err(|e| UpdateError::Io(e.to_string()))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&staged, perms).map_err(|e| UpdateError::Io(e.to_string()))?;
    std::fs::rename(&staged, &appimage).map_err(|e| UpdateError::Io(e.to_string()))?;
    Ok(appimage)
}

/// AppImages only exist on Linux; this stub keeps the crate compiling on
/// non-Unix targets where `PermissionsExt` is unavailable.
#[cfg(not(unix))]
fn apply_appimage(_verified: &Path) -> Result<PathBuf, UpdateError> {
    Err(UpdateError::Io(
        "AppImage self-replace is only supported on Unix".into(),
    ))
}

/// Spawn the freshly replaced executable and exit this process. Never returns.
/// A spawn failure is logged but still ends in `exit(0)`: the update is already
/// applied on disk, so exiting (and letting the user reopen the app) is better
/// than continuing to run the stale, now-deleted binary.
pub(crate) fn relaunch_and_exit(exe: &Path) -> ! {
    if let Err(e) = std::process::Command::new(exe).spawn() {
        tracing::warn!(error = %e, "failed to relaunch after self-update");
    }
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    /// Build a tiny in-memory zip from `(name, bytes)` entries. `Stored` avoids
    /// depending on any compression feature for the test itself.
    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, bytes) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_single_exe_entry() {
        let zip = zip_with(&[
            ("readme.txt", b"hello"),
            ("gashuu.exe", b"MZ\x90\x00fake-pe"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = extract_exe_from_zip(&zip, dir.path()).unwrap();
        assert_eq!(path.file_name().unwrap(), "gashuu.exe");
        assert_eq!(std::fs::read(&path).unwrap(), b"MZ\x90\x00fake-pe");
    }

    #[test]
    fn picks_exe_case_insensitively() {
        let zip = zip_with(&[("Gashuu.EXE", b"payload")]);
        let dir = tempfile::tempdir().unwrap();
        let path = extract_exe_from_zip(&zip, dir.path()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"payload");
    }

    #[test]
    fn errors_when_no_exe_entry() {
        let zip = zip_with(&[("readme.txt", b"hello")]);
        let dir = tempfile::tempdir().unwrap();
        assert!(extract_exe_from_zip(&zip, dir.path()).is_err());
    }

    #[test]
    fn strips_directory_components_from_entry_name() {
        // A crafted entry name with path segments must not escape out_dir.
        let zip = zip_with(&[("nested/dir/gashuu.exe", b"payload")]);
        let dir = tempfile::tempdir().unwrap();
        let path = extract_exe_from_zip(&zip, dir.path()).unwrap();
        assert_eq!(path.parent().unwrap(), dir.path());
        assert_eq!(path.file_name().unwrap(), "gashuu.exe");
    }
}
