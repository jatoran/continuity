//! In-place updates from GitHub Releases.
//!
//! The host side of the update feature. Once shortly after launch and then
//! daily, a background thread asks the GitHub Releases API for the latest
//! desktop release; when it is newer than the running binary (and not a
//! version the user skipped) every live window receives an offer banner.
//! *Update now* downloads the matching asset, verifies it against the
//! release's `SHA256SUMS.txt`, writes a small script that waits for this
//! process to exit and then installs it — `msiexec` for the MSI (and
//! winget, which installs the same MSI), an executable swap for the
//! portable and standalone zips — and finally asks every window to close.
//!
//! Nothing is downloaded until the user clicks. The check itself is one
//! unauthenticated GET (60/hour/IP allowance) and can be disabled with
//! `[updates] check = false`; `help.check_for_updates` still works then.
//!
//! Thread ownership: [`UpdateHost`] is shared (`Arc`) between the poller
//! thread, the per-action worker threads, and the window callback. All
//! mutable state lives in the JSON state file and is read/written by one
//! worker at a time (actions serialize through a mutex).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use continuity_ui::{UpdateAction, UpdateActions, UpdateOffer};
use crossbeam_channel::Sender;

use crate::registry::RegistryEvent;
use crate::registry_time::unix_ms_now;

mod checksum;
mod github;
mod install;
mod install_kind;
mod state;
mod version;

pub(crate) use install_kind::InstallKind;

/// Wall-clock milliseconds as the unsigned value the state file stores.
fn now_ms() -> u64 {
    u64::try_from(unix_ms_now()).unwrap_or(0)
}

/// Delay between process start and the first background check, so the
/// launch paint and restore never compete with a network round trip.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(12);
/// Minimum gap between two background checks.
const CHECK_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;
/// How often the poller re-evaluates whether a check is due.
const POLL_WAKE: Duration = Duration::from_secs(60 * 60);

/// Everything an update check or install needs, shared across threads.
pub(crate) struct UpdateHost {
    tx: Sender<RegistryEvent>,
    current_version: String,
    install_kind: InstallKind,
    exe_path: PathBuf,
    state_path: Option<PathBuf>,
    /// `Mutex` justification: serializes the on-demand check and the
    /// install worker so two button clicks cannot race the state file or
    /// launch two installers.
    action_lock: Mutex<()>,
}

/// Failures surfaced to the user as banner text.
#[derive(Debug, thiserror::Error)]
pub(crate) enum UpdateError {
    /// The network call failed or the server refused.
    #[error("{0}")]
    Network(#[from] continuity_win::Error),
    /// The release payload could not be understood.
    #[error("unexpected release data: {0}")]
    Release(String),
    /// The downloaded asset did not match the published checksum.
    #[error("checksum mismatch for {0}")]
    Checksum(String),
    /// Writing or launching the installer failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl UpdateHost {
    /// Build the host for the running executable. Detects the install
    /// kind once (it cannot change while the process runs).
    pub(crate) fn new(tx: Sender<RegistryEvent>) -> Self {
        let exe_path = std::env::current_exe().unwrap_or_default();
        Self {
            tx,
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            install_kind: InstallKind::detect(&exe_path),
            exe_path,
            state_path: continuity_persist::paths::updates_state_path().ok(),
            action_lock: Mutex::new(()),
        }
    }

    /// Start the daily background poll. No-op when `enabled` is false.
    pub(crate) fn spawn_poller(self: &Arc<Self>, enabled: bool) {
        if !enabled {
            return;
        }
        let host = Arc::clone(self);
        let spawned = thread::Builder::new()
            .name("continuity-update-poll".into())
            .spawn(move || {
                thread::sleep(FIRST_CHECK_DELAY);
                loop {
                    let last = host.load_state().last_check_ms;
                    if now_ms().saturating_sub(last) >= CHECK_INTERVAL_MS {
                        host.check(false);
                    }
                    thread::sleep(POLL_WAKE);
                }
            });
        if let Err(e) = spawned {
            eprintln!("continuity: update poller failed to start: {e}");
        }
    }

    /// The callback handed to every window.
    pub(crate) fn window_callback(self: &Arc<Self>) -> UpdateActions {
        let host = Arc::clone(self);
        UpdateActions(Arc::new(move |action| host.handle_action(action)))
    }

    fn handle_action(self: &Arc<Self>, action: UpdateAction) {
        let host = Arc::clone(self);
        let spawned = thread::Builder::new()
            .name("continuity-update-action".into())
            .spawn(move || match action {
                UpdateAction::CheckNow => host.check(true),
                UpdateAction::Skip(version) => {
                    let mut state = host.load_state();
                    state.skipped_version = Some(version);
                    host.save_state(&state);
                }
                UpdateAction::Install(offer) => host.install(&offer),
            });
        if let Err(e) = spawned {
            eprintln!("continuity: update action thread failed to start: {e}");
        }
    }

    /// One check. `announce` reports "up to date" and failures as
    /// banners (the on-demand path); the background poll stays silent
    /// unless there is something to offer.
    fn check(&self, announce: bool) {
        let _guard = self
            .action_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self.load_state();
        state.last_check_ms = now_ms();
        let result = github::fetch_latest_release();
        match result {
            Ok(release) => {
                let is_newer = version::is_newer(&release.version, &self.current_version);
                let is_skipped = state.skipped_version.as_deref() == Some(release.version.as_str());
                if is_newer && (announce || !is_skipped) {
                    let _ = self.tx.send(RegistryEvent::UpdateAvailable(UpdateOffer {
                        version: release.version,
                        notes_url: release.notes_url,
                    }));
                } else if announce {
                    self.status(
                        format!("Continuity {} is up to date.", self.current_version),
                        false,
                    );
                }
            }
            Err(e) => {
                if announce {
                    self.status(format!("Update check failed: {e}"), false);
                }
            }
        }
        self.save_state(&state);
    }

    fn install(&self, offer: &UpdateOffer) {
        let _guard = self
            .action_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let outcome = github::fetch_latest_release().and_then(|release| {
            if release.version != offer.version {
                return Err(UpdateError::Release(format!(
                    "the latest release is now {}, not {}",
                    release.version, offer.version
                )));
            }
            install::download_and_stage(self.install_kind, &self.exe_path, &release)
        });
        match outcome {
            Ok(()) => {
                self.status(
                    format!(
                        "Installing Continuity {} ({} update)… Continuity will close and reopen.",
                        offer.version,
                        self.install_kind.label()
                    ),
                    true,
                );
                // Let the banner paint before the windows go away.
                thread::sleep(Duration::from_millis(900));
                let _ = self.tx.send(RegistryEvent::CloseAllWindows);
            }
            Err(e) => self.status(format!("Update failed: {e}"), false),
        }
    }

    fn status(&self, text: String, sticky: bool) {
        let _ = self.tx.send(RegistryEvent::UpdateStatus { text, sticky });
    }

    fn load_state(&self) -> state::UpdateState {
        self.state_path
            .as_deref()
            .map(state::UpdateState::load)
            .unwrap_or_default()
    }

    fn save_state(&self, state: &state::UpdateState) {
        if let Some(path) = self.state_path.as_deref() {
            if let Err(e) = state.save(path) {
                eprintln!("continuity: could not save update state: {e}");
            }
        }
    }
}
