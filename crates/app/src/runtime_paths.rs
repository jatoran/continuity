//! Runtime path and CLI-mode resolution for the app binary.
//!
//! Thread ownership: resolved once on the main thread before any worker
//! thread starts. Portable mode is selected by `--portable` or by a
//! `data` directory beside the executable, then applies process
//! environment overrides before persistence/config owners are constructed.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use crate::error::Error;

type Result<T> = std::result::Result<T, Error>;

/// Startup paths passed on the command line after option parsing.
pub(crate) struct StartupPaths {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) folders: Vec<PathBuf>,
}

/// Filesystem roots the app should use for this process.
pub(crate) struct RuntimePaths {
    pub(crate) settings_path: PathBuf,
    pub(crate) keymap_path: PathBuf,
    pub(crate) themes_dir: PathBuf,
    pub(crate) backups_dir: Option<PathBuf>,
    portable_data_dir: Option<PathBuf>,
}

/// Parsed app startup mode.
pub(crate) struct StartupOptions {
    pub(crate) startup_paths: StartupPaths,
    pub(crate) runtime_paths: RuntimePaths,
}

impl StartupOptions {
    /// Parse process arguments and resolve runtime paths.
    pub(crate) fn from_env() -> Result<Self> {
        Self::from_args(env::args_os().skip(1))
    }

    fn from_args(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        Self::from_args_with_auto_portable(args, RuntimePaths::has_portable_data_dir())
    }

    fn from_args_with_auto_portable(
        args: impl IntoIterator<Item = OsString>,
        auto_portable: bool,
    ) -> Result<Self> {
        let mut portable = false;
        let mut paths = Vec::new();
        for arg in args {
            if arg == "--portable" {
                portable = true;
            } else if arg == "--" {
                continue;
            } else {
                paths.push(PathBuf::from(arg));
            }
        }

        let runtime_paths = if portable || auto_portable {
            RuntimePaths::portable()?
        } else {
            RuntimePaths::installed()
        };
        Ok(Self {
            startup_paths: split_startup_paths(paths),
            runtime_paths,
        })
    }
}

impl RuntimePaths {
    fn installed() -> Self {
        let base = env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("continuity");
        Self {
            settings_path: base.join("settings.toml"),
            keymap_path: base.join("keymap.toml"),
            themes_dir: base.join("themes"),
            backups_dir: None,
            portable_data_dir: None,
        }
    }

    fn portable() -> Result<Self> {
        let exe = env::current_exe().map_err(Error::CurrentExecutable)?;
        let root = exe.parent().ok_or(Error::CurrentExecutableMissingParent)?;
        let data_dir = root.join("data");
        Ok(Self {
            settings_path: data_dir.join("settings.toml"),
            keymap_path: data_dir.join("keymap.toml"),
            themes_dir: data_dir.join("themes"),
            backups_dir: Some(root.join("backups")),
            portable_data_dir: Some(data_dir),
        })
    }

    fn has_portable_data_dir() -> bool {
        env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|root| root.join("data")))
            .is_some_and(|data_dir| data_dir.is_dir())
    }

    /// Apply portable-mode overrides for crates that resolve paths
    /// internally, such as persistence tutorial sentinels.
    pub(crate) fn apply_process_overrides(&self) {
        if let Some(data_dir) = self.portable_data_dir.as_ref() {
            env::set_var("CONTINUITY_DATA_DIR", data_dir);
        }
        if let Some(backups_dir) = self.backups_dir.as_ref() {
            env::set_var("CONTINUITY_BACKUPS_DIR", backups_dir);
        }
    }
}

fn split_startup_paths(paths: Vec<PathBuf>) -> StartupPaths {
    let mut files = Vec::new();
    let mut folders = Vec::new();
    for path in paths {
        if path.is_dir() {
            folders.push(path);
        } else {
            files.push(path);
        }
    }
    StartupPaths { files, folders }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_mode_keeps_path_args() {
        let opts = StartupOptions::from_args_with_auto_portable(
            [OsString::from("note.md"), OsString::from("missing-dir")],
            false,
        )
        .expect("installed args parse");

        assert_eq!(opts.startup_paths.files.len(), 2);
        assert!(opts.runtime_paths.backups_dir.is_none());
    }

    #[test]
    fn portable_flag_is_not_treated_as_file() {
        let opts = StartupOptions::from_args_with_auto_portable(
            [OsString::from("--portable"), OsString::from("a.md")],
            false,
        )
        .expect("portable args parse");

        assert_eq!(opts.startup_paths.files, vec![PathBuf::from("a.md")]);
        assert!(opts.runtime_paths.backups_dir.is_some());
        assert!(opts
            .runtime_paths
            .settings_path
            .ends_with(PathBuf::from("data").join("settings.toml")));
    }

    #[test]
    fn beside_exe_data_dir_selects_portable_mode() {
        let opts =
            StartupOptions::from_args_with_auto_portable([OsString::from("a.md")], true)
                .expect("auto portable args parse");

        assert_eq!(opts.startup_paths.files, vec![PathBuf::from("a.md")]);
        assert!(opts.runtime_paths.backups_dir.is_some());
        assert!(opts
            .runtime_paths
            .settings_path
            .ends_with(PathBuf::from("data").join("settings.toml")));
    }
}
