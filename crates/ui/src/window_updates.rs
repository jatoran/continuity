//! In-app update banner: offer, status, and action routing.
//!
//! The host (the `app` crate) owns everything that touches the network or
//! the installer — polling GitHub Releases, downloading and verifying the
//! asset, launching the MSI or swapping the portable executable. The
//! window only *presents*: it shows the offer as a sticky banner with
//! `Update now` / `Release notes` / `Skip this version` buttons, relays
//! the chosen action through the host callback, and shows the host's
//! progress and outcome text as banners. Banner, not modal — the writer
//! keeps typing.
//!
//! Thread ownership: UI thread of one window. `update_actions` is an
//! `Arc<dyn Fn>` the host installed at construction; calling it never
//! blocks (the host hands the work to its own thread).

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::window::Window;
use crate::window_control::{UpdateAction, UpdateOffer};
use crate::window_file::FileBanner;

/// How long the transient "up to date" / failure banners stay.
const UPDATE_STATUS_BANNER_MS: u64 = 6000;

impl Window {
    /// `WindowControl::UpdateAvailable` — show the sticky offer banner.
    /// An existing conflict banner (a decision the user still owes) is
    /// never replaced by an offer; the offer is re-sent on the next poll.
    pub(crate) fn on_update_available(&mut self, offer: UpdateOffer) {
        if self
            .file_banner
            .as_ref()
            .is_some_and(|banner| banner.pending.is_some())
        {
            return;
        }
        self.file_banner = Some(FileBanner::update_offer(offer));
    }

    /// `WindowControl::UpdateStatus` — progress or outcome text from the
    /// host. Sticky while a download/install is in flight (the app is
    /// about to exit anyway); transient for "up to date" and failures.
    pub(crate) fn on_update_status(&mut self, text: String, sticky: bool) {
        let now = self.now_ms();
        self.file_banner = Some(if sticky {
            FileBanner::new(text)
        } else {
            FileBanner::transient_for(text, now, UPDATE_STATUS_BANNER_MS)
        });
        self.start_motion_timer();
    }

    /// A click on one of the offer banner's buttons.
    pub(crate) fn apply_update_banner_action(&mut self, action: UpdateBannerAction) {
        let Some(offer) = self
            .file_banner
            .as_ref()
            .and_then(|banner| banner.update_offer.clone())
        else {
            return;
        };
        match action {
            UpdateBannerAction::Install => {
                self.file_banner = Some(FileBanner::new(format!(
                    "Downloading Continuity {}…",
                    offer.version
                )));
                self.send_update_action(UpdateAction::Install(offer));
            }
            UpdateBannerAction::ReleaseNotes => open_external_url(&offer.notes_url),
            UpdateBannerAction::Skip => {
                self.file_banner = None;
                self.send_update_action(UpdateAction::Skip(offer.version));
            }
        }
    }

    /// `help.check_for_updates` — ask the host to poll now.
    pub(crate) fn check_for_updates_impl(&mut self) -> Result<(), crate::Error> {
        if self.view_options.update_actions.is_none() {
            return Err(crate::Error::Command(
                continuity_command::Error::UnsupportedContext("check_for_updates"),
            ));
        }
        let now = self.now_ms();
        self.file_banner = Some(FileBanner::transient_for(
            "Checking for updates…".to_string(),
            now,
            UPDATE_STATUS_BANNER_MS,
        ));
        self.start_motion_timer();
        self.send_update_action(UpdateAction::CheckNow);
        Ok(())
    }

    fn send_update_action(&self, action: UpdateAction) {
        if let Some(callback) = self.view_options.update_actions.as_ref() {
            (callback.0)(action);
        }
    }
}

/// Which offer-banner button was pressed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpdateBannerAction {
    Install,
    ReleaseNotes,
    Skip,
}

/// Open `url` in the default browser. `ShellExecuteW` with the `open`
/// verb is the same path Ctrl+click on a link takes.
fn open_external_url(url: &str) {
    if !url.starts_with("https://") {
        return;
    }
    let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let target: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(
            None,
            windows::core::PCWSTR(verb.as_ptr()),
            windows::core::PCWSTR(target.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}
