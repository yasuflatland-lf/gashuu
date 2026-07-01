//! Detect how this build was packaged, purely from the running executable path
//! and the `$APPIMAGE` environment variable — no `cfg!(target_os)`, so every
//! branch is unit-testable on any host.

use std::ffi::OsStr;
use std::path::Path;

/// Recognised distribution forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packaging {
    MacOsApp,
    WindowsPortable,
    LinuxAppImage,
    LinuxDeb,
    Unknown,
}

/// What the "Update now" action should do for a given packaging form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStrategy {
    /// Download + verify + replace the binary in place + relaunch (safe forms).
    SelfReplace,
    /// Download + verify, then reveal the file in the OS file manager for a
    /// manual drag-install (macOS `.app`).
    RevealDownload,
    /// Just open the GitHub release page in the browser (package-manager-owned
    /// or unknown installs).
    OpenReleasePage,
}

/// Detect packaging from `exe_path` (typically `std::env::current_exe()`) and
/// `appimage_env` (typically `std::env::var_os("APPIMAGE")`). `$APPIMAGE` is
/// checked first because an AppImage's `current_exe()` points inside the
/// read-only mounted squashfs.
pub fn detect_packaging(exe_path: &Path, appimage_env: Option<&OsStr>) -> Packaging {
    if appimage_env.map(|s| !s.is_empty()).unwrap_or(false) {
        return Packaging::LinuxAppImage;
    }
    let s = exe_path.to_string_lossy();
    if s.contains(".app/Contents/MacOS/") {
        return Packaging::MacOsApp;
    }
    if s.to_ascii_lowercase().ends_with(".exe") {
        return Packaging::WindowsPortable;
    }
    if s.starts_with("/usr/") {
        return Packaging::LinuxDeb;
    }
    Packaging::Unknown
}

impl Packaging {
    /// The update action appropriate for this packaging form.
    pub fn strategy(self) -> UpdateStrategy {
        match self {
            Packaging::LinuxAppImage | Packaging::WindowsPortable => UpdateStrategy::SelfReplace,
            Packaging::MacOsApp => UpdateStrategy::RevealDownload,
            Packaging::LinuxDeb | Packaging::Unknown => UpdateStrategy::OpenReleasePage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn detect(path: &str, appimage: Option<&str>) -> Packaging {
        let env = appimage.map(OsString::from);
        detect_packaging(&PathBuf::from(path), env.as_deref())
    }

    #[test]
    fn appimage_env_wins_over_path() {
        // Even though current_exe is under the mounted /tmp squashfs, $APPIMAGE marks it.
        assert_eq!(
            detect(
                "/tmp/.mount_gashuuAbc/usr/bin/gashuu",
                Some("/home/u/gashuu.AppImage")
            ),
            Packaging::LinuxAppImage
        );
    }

    #[test]
    fn empty_appimage_env_is_ignored() {
        assert_eq!(detect("/usr/bin/gashuu", Some("")), Packaging::LinuxDeb);
    }

    #[test]
    fn macos_app_bundle_path() {
        assert_eq!(
            detect("/Applications/gashuu.app/Contents/MacOS/gashuu", None),
            Packaging::MacOsApp
        );
    }

    #[test]
    fn windows_exe_path() {
        assert_eq!(
            detect(r"C:\Users\u\Downloads\gashuu.exe", None),
            Packaging::WindowsPortable
        );
    }

    #[test]
    fn linux_usr_bin_is_deb() {
        assert_eq!(detect("/usr/bin/gashuu", None), Packaging::LinuxDeb);
    }

    #[test]
    fn cargo_run_target_is_unknown() {
        assert_eq!(
            detect("/home/u/gashuu/target/debug/gashuu", None),
            Packaging::Unknown
        );
    }

    #[test]
    fn strategy_mapping() {
        assert_eq!(
            Packaging::LinuxAppImage.strategy(),
            UpdateStrategy::SelfReplace
        );
        assert_eq!(
            Packaging::WindowsPortable.strategy(),
            UpdateStrategy::SelfReplace
        );
        assert_eq!(
            Packaging::MacOsApp.strategy(),
            UpdateStrategy::RevealDownload
        );
        assert_eq!(
            Packaging::LinuxDeb.strategy(),
            UpdateStrategy::OpenReleasePage
        );
        assert_eq!(
            Packaging::Unknown.strategy(),
            UpdateStrategy::OpenReleasePage
        );
    }
}
