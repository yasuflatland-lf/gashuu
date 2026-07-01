//! Parse the GitHub `releases/latest` JSON payload into a small domain type.

use serde::Deserialize;

/// A downloadable release asset (one file attached to the GitHub release).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub download_url: String,
}

/// The subset of a GitHub release we care about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// Raw tag, e.g. "v0.11.0".
    pub tag: String,
    /// Tag with a leading `v` stripped, e.g. "0.11.0" (compared against CARGO_PKG_VERSION).
    pub version: String,
    /// Human-facing release page URL.
    pub html_url: String,
    /// Release notes (markdown).
    pub body: String,
    pub assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
struct RawAsset {
    name: String,
    browser_download_url: String,
}

/// Parse the `releases/latest` payload. Missing `body`/`assets` default to
/// empty; a missing `tag_name`/`html_url` is a hard error (malformed payload).
pub fn parse_latest_release(json: &str) -> Result<ReleaseInfo, serde_json::Error> {
    let raw: RawRelease = serde_json::from_str(json)?;
    let version = raw.tag_name.trim_start_matches(['v', 'V']).to_string();
    Ok(ReleaseInfo {
        tag: raw.tag_name,
        version,
        html_url: raw.html_url,
        body: raw.body,
        assets: raw
            .assets
            .into_iter()
            .map(|a| Asset {
                name: a.name,
                download_url: a.browser_download_url,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "tag_name": "v0.11.0",
        "html_url": "https://github.com/yasuflatland-lf/gashuu/releases/tag/v0.11.0",
        "body": "release notes here",
        "assets": [
            {"name": "gashuu-v0.11.0-x86_64.AppImage", "browser_download_url": "https://example/app"},
            {"name": "SHA256SUMS", "browser_download_url": "https://example/sums"}
        ]
    }"#;

    #[test]
    fn parses_tag_version_url_and_assets() {
        let info = parse_latest_release(SAMPLE).unwrap();
        assert_eq!(info.tag, "v0.11.0");
        assert_eq!(info.version, "0.11.0");
        assert!(info.html_url.ends_with("v0.11.0"));
        assert_eq!(info.body, "release notes here");
        assert_eq!(info.assets.len(), 2);
        assert_eq!(info.assets[0].name, "gashuu-v0.11.0-x86_64.AppImage");
        assert_eq!(info.assets[0].download_url, "https://example/app");
    }

    #[test]
    fn missing_body_and_assets_default_to_empty() {
        let json = r#"{"tag_name":"v1.0.0","html_url":"https://x"}"#;
        let info = parse_latest_release(json).unwrap();
        assert_eq!(info.body, "");
        assert!(info.assets.is_empty());
    }

    #[test]
    fn malformed_payload_errors() {
        assert!(parse_latest_release("not json").is_err());
        assert!(parse_latest_release(r#"{"html_url":"x"}"#).is_err()); // no tag_name
    }
}
