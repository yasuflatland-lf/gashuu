//! Pick the release asset matching this platform's packaging, by filename
//! convention (`gashuu-<tag>-{macos-universal.zip|windows-x64.zip|amd64.deb|x86_64.AppImage}`).

use crate::update::packaging::Packaging;
use crate::update::release::Asset;

/// Select the asset appropriate for `pkg`, or `None` if the release has no
/// matching artifact.
pub fn select_asset(pkg: Packaging, assets: &[Asset]) -> Option<&Asset> {
    assets.iter().find(|a| {
        let n = a.name.to_ascii_lowercase();
        match pkg {
            Packaging::MacOsApp => n.contains("macos") && n.ends_with(".zip"),
            Packaging::WindowsPortable => n.contains("windows") && n.ends_with(".zip"),
            Packaging::LinuxAppImage => n.ends_with(".appimage"),
            Packaging::LinuxDeb => n.ends_with(".deb"),
            Packaging::Unknown => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_assets() -> Vec<Asset> {
        [
            "gashuu-v0.11.0-macos-universal.zip",
            "gashuu-v0.11.0-windows-x64.zip",
            "gashuu-v0.11.0-amd64.deb",
            "gashuu-v0.11.0-x86_64.AppImage",
            "SHA256SUMS",
        ]
        .iter()
        .map(|n| Asset {
            name: n.to_string(),
            download_url: format!("https://x/{n}"),
        })
        .collect()
    }

    #[test]
    fn selects_per_platform() {
        let a = sample_assets();
        assert_eq!(
            select_asset(Packaging::MacOsApp, &a).unwrap().name,
            "gashuu-v0.11.0-macos-universal.zip"
        );
        assert_eq!(
            select_asset(Packaging::WindowsPortable, &a).unwrap().name,
            "gashuu-v0.11.0-windows-x64.zip"
        );
        assert_eq!(
            select_asset(Packaging::LinuxDeb, &a).unwrap().name,
            "gashuu-v0.11.0-amd64.deb"
        );
        assert_eq!(
            select_asset(Packaging::LinuxAppImage, &a).unwrap().name,
            "gashuu-v0.11.0-x86_64.AppImage"
        );
    }

    #[test]
    fn unknown_selects_nothing() {
        assert!(select_asset(Packaging::Unknown, &sample_assets()).is_none());
    }

    #[test]
    fn missing_asset_returns_none() {
        let only_sums = vec![Asset {
            name: "SHA256SUMS".into(),
            download_url: "x".into(),
        }];
        assert!(select_asset(Packaging::LinuxAppImage, &only_sums).is_none());
    }
}
