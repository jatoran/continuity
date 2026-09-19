//! GitHub Releases API client for the desktop release train.
//!
//! `releases/latest` returns the newest non-prerelease, non-draft release.
//! SDK releases are tagged `sdk-v*` and published with `--latest=false`,
//! so "latest" is always the Windows desktop release (see
//! `EMBEDDING.md`).

use serde_json::Value;

use super::UpdateError;

/// GitHub REST endpoint for the newest published desktop release.
const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/jatoran/continuity/releases/latest";
/// User agent GitHub requires on every API call.
const USER_AGENT: &str = concat!("continuity-desktop/", env!("CARGO_PKG_VERSION"));

/// The assets of one desktop release the updater cares about.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReleaseInfo {
    /// Version without the `v` prefix.
    pub(crate) version: String,
    /// Release page for the *Release notes* button.
    pub(crate) notes_url: String,
    /// `continuity-<version>-setup.msi` download URL.
    pub(crate) msi_url: Option<String>,
    /// `continuity-<version>-standalone.zip` download URL.
    pub(crate) standalone_zip_url: Option<String>,
    /// `SHA256SUMS.txt` download URL.
    pub(crate) sums_url: Option<String>,
}

/// Fetch and parse the latest desktop release.
pub(crate) fn fetch_latest_release() -> Result<ReleaseInfo, UpdateError> {
    let body = continuity_win::https_get(LATEST_RELEASE_URL, USER_AGENT)?;
    parse_latest_release(&body)
}

/// Download one release asset (an MSI, a zip, the checksum list).
pub(crate) fn fetch_asset(url: &str) -> Result<Vec<u8>, UpdateError> {
    Ok(continuity_win::https_get(url, USER_AGENT)?)
}

/// Parse the `releases/latest` JSON payload.
pub(crate) fn parse_latest_release(json: &[u8]) -> Result<ReleaseInfo, UpdateError> {
    let value: Value =
        serde_json::from_slice(json).map_err(|e| UpdateError::Release(e.to_string()))?;
    if value["draft"].as_bool() == Some(true) || value["prerelease"].as_bool() == Some(true) {
        return Err(UpdateError::Release(
            "latest release is a draft or prerelease".to_string(),
        ));
    }
    let tag = value["tag_name"]
        .as_str()
        .ok_or_else(|| UpdateError::Release("missing tag_name".to_string()))?;
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    if super::version::parse_semver(&version).is_none() {
        return Err(UpdateError::Release(format!("unrecognized tag {tag}")));
    }
    let notes_url = value["html_url"]
        .as_str()
        .unwrap_or("https://github.com/jatoran/continuity/releases")
        .to_string();
    let asset_url = |name: &str| -> Option<String> {
        value["assets"].as_array()?.iter().find_map(|asset| {
            (asset["name"].as_str() == Some(name))
                .then(|| asset["browser_download_url"].as_str().map(str::to_string))
                .flatten()
        })
    };
    Ok(ReleaseInfo {
        msi_url: asset_url(&format!("continuity-{version}-setup.msi")),
        standalone_zip_url: asset_url(&format!("continuity-{version}-standalone.zip")),
        sums_url: asset_url("SHA256SUMS.txt"),
        version,
        notes_url,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_latest_release;

    const PAYLOAD: &str = r#"{
      "tag_name": "v0.4.12",
      "html_url": "https://github.com/jatoran/continuity/releases/tag/v0.4.12",
      "draft": false,
      "prerelease": false,
      "assets": [
        {"name": "continuity-0.4.12-setup.msi", "browser_download_url": "https://github.com/x/setup.msi"},
        {"name": "continuity-0.4.12-portable.zip", "browser_download_url": "https://github.com/x/portable.zip"},
        {"name": "continuity-0.4.12-standalone.zip", "browser_download_url": "https://github.com/x/standalone.zip"},
        {"name": "SHA256SUMS.txt", "browser_download_url": "https://github.com/x/SHA256SUMS.txt"}
      ]
    }"#;

    #[test]
    fn picks_the_desktop_assets_by_name() {
        let release = parse_latest_release(PAYLOAD.as_bytes()).expect("parse");
        assert_eq!(release.version, "0.4.12");
        assert_eq!(
            release.msi_url.as_deref(),
            Some("https://github.com/x/setup.msi")
        );
        assert_eq!(
            release.standalone_zip_url.as_deref(),
            Some("https://github.com/x/standalone.zip")
        );
        assert_eq!(
            release.sums_url.as_deref(),
            Some("https://github.com/x/SHA256SUMS.txt")
        );
        assert!(release.notes_url.ends_with("/v0.4.12"));
    }

    #[test]
    fn rejects_prereleases_and_foreign_tags() {
        let pre = PAYLOAD.replace("\"prerelease\": false", "\"prerelease\": true");
        assert!(parse_latest_release(pre.as_bytes()).is_err());
        let sdk = PAYLOAD.replace("v0.4.12", "sdk-v0.2.40");
        assert!(parse_latest_release(sdk.as_bytes()).is_err());
        assert!(parse_latest_release(b"not json").is_err());
    }
}
