//! Which distribution the running executable came from.
//!
//! The three public artifacts install differently, so the updater must
//! know which one it is replacing:
//!
//! - **MSI** (also what winget installs): the executable sits under the
//!   directory the installer recorded in `HKLM\Software\Continuity\
//!   InstallDir`. Upgrading means running the new MSI; Windows Installer's
//!   `MajorUpgrade` replaces the files in place and keeps the file
//!   associations and shortcuts.
//! - **Portable zip**: a `data\` directory beside the executable (the
//!   same rule `runtime_paths` uses). Upgrading swaps the executable and
//!   leaves `data\` alone.
//! - **Standalone zip**: neither of the above. Same swap as portable.

use std::path::Path;

/// Distribution the running executable belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallKind {
    /// Installed by `continuity-<version>-setup.msi` (directly or via winget).
    Msi,
    /// `continuity-<version>-portable.zip` with a folder-local `data\`.
    Portable,
    /// `continuity-<version>-standalone.zip` (or any loose copy of the exe).
    Standalone,
}

impl InstallKind {
    /// Classify `exe_path` against the portable marker and the MSI's
    /// registry record.
    pub(crate) fn detect(exe_path: &Path) -> Self {
        let install_dir = continuity_win::read_hklm_string("Software\\Continuity", "InstallDir");
        Self::classify(exe_path, install_dir.as_deref())
    }

    /// Pure classification, unit-testable without the registry.
    pub(crate) fn classify(exe_path: &Path, msi_install_dir: Option<&str>) -> Self {
        let parent = exe_path.parent();
        if parent.is_some_and(|dir| dir.join("data").is_dir()) {
            return Self::Portable;
        }
        if let (Some(parent), Some(install_dir)) = (parent, msi_install_dir) {
            let recorded = normalize_dir(install_dir);
            let actual = normalize_dir(&parent.to_string_lossy());
            if !recorded.is_empty() && actual == recorded {
                return Self::Msi;
            }
        }
        Self::Standalone
    }

    /// User-facing name used in status text.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Msi => "installer",
            Self::Portable => "portable",
            Self::Standalone => "standalone",
        }
    }
}

/// Lower-case, forward-slash, no trailing separator — enough to compare
/// the registry's `C:\Program Files\Continuity\` with the executable's
/// `C:\Program Files\Continuity`.
fn normalize_dir(path: &str) -> String {
    path.replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::InstallKind;
    use std::path::Path;

    #[test]
    fn msi_when_exe_lives_in_the_recorded_install_dir() {
        let exe = Path::new("C:\\Program Files\\Continuity\\continuity.exe");
        assert_eq!(
            InstallKind::classify(exe, Some("C:\\Program Files\\Continuity\\")),
            InstallKind::Msi
        );
        assert_eq!(
            InstallKind::classify(exe, Some("c:/program files/continuity")),
            InstallKind::Msi
        );
    }

    #[test]
    fn standalone_when_no_marker_matches() {
        let exe = Path::new("D:\\tools\\continuity.exe");
        assert_eq!(
            InstallKind::classify(exe, Some("C:\\Program Files\\Continuity\\")),
            InstallKind::Standalone
        );
        assert_eq!(InstallKind::classify(exe, None), InstallKind::Standalone);
    }

    #[test]
    fn portable_when_a_data_dir_sits_beside_the_exe() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(root.path().join("data")).expect("data dir");
        let exe = root.path().join("continuity.exe");
        assert_eq!(
            InstallKind::classify(&exe, Some(&root.path().to_string_lossy())),
            InstallKind::Portable,
            "portable wins even when the registry points at the same directory"
        );
    }
}
