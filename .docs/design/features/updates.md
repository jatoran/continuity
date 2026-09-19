# In-app updates

Continuity for Windows checks GitHub Releases for a newer version and installs it in place.
The check is a banner, never a modal; nothing is downloaded until the user asks.

## Behaviour

- **Polling.** Twelve seconds after launch, and then whenever a day has passed since the last completed check, a background thread asks `https://api.github.com/repos/jatoran/continuity/releases/latest`.
  The call is unauthenticated (GitHub allows 60 per hour per address) and sends `continuity-desktop/<version>` as its user agent.
  SDK releases are tagged `sdk-v*` and published with `--latest=false`, so "latest" is always the desktop release.
- **Offer.** When the tag is strictly newer than the running binary and is not the version the user skipped, every live window shows a sticky banner: `Continuity 0.4.12 is available.` with `Update now`, `Release notes`, and `Skip this version`.
  A window that still owes a decision on an external-change conflict banner keeps that banner; the offer returns on the next poll.
- **Update now.** The host downloads the asset for the running install kind plus the release's `SHA256SUMS.txt`, verifies the digest, writes a small `.cmd` script to `%TEMP%\continuity-update-<version>\` that waits for this process id to exit, launches the script detached, shows `Installing Continuity … Continuity will close and reopen.`, and asks every window to close.
  Each window runs its ordinary close sequence (vault autosave flush, placement save, closed-history archive), so nothing is lost.
- **Release notes** opens the release page in the default browser. **Skip this version** records the version in `updates.json` and dismisses the banner; a later explicit check offers it again.
- **On demand.** `help.check_for_updates` (command palette: "Check for updates") polls immediately and reports `Continuity <version> is up to date.` or the failure reason as a transient banner.
- **Setting.** `[updates] check = true` (default). `false` disables the background poll entirely; the command still works. Read at launch.

## Install kinds

The running executable's origin decides how the new version lands (`crates/app/src/updater/install_kind.rs`):

| Kind | Detection | Install step in the script |
|---|---|---|
| MSI (also what winget installs) | exe directory equals `HKLM\Software\Continuity\InstallDir`, which the MSI records | `msiexec /i continuity-<v>-setup.msi /passive /norestart`, then relaunch. The package is per-machine, so Windows Installer raises the UAC consent itself; `MajorUpgrade` replaces the files and keeps shortcuts and file associations. winget tracks the Add/Remove Programs entry, so `winget upgrade` still agrees afterwards. |
| Portable zip | a `data\` directory beside the exe (the same rule `runtime_paths` uses) | extract `continuity.exe` from `continuity-<v>-standalone.zip` with `Expand-Archive`, rename the running exe to `.old`, move the new one in, relaunch, delete `.old`. `data\` is untouched. |
| Standalone zip | neither of the above | same swap as portable. |

## Pipeline

```
app::updater (poll thread / action threads)
   │  RegistryEvent::{UpdateAvailable, UpdateStatus, CloseAllWindows}
   ▼
app::registry loop ──► registry_updates::fan_out ──► WindowControl::{UpdateAvailable, UpdateStatus}
                                                          │  (per-window control channel + wake)
                                                          ▼
ui::window_updates: offer banner (window_file_banner_buttons::BannerButtonsKind::Update)
   │  button click → UpdateActions callback (WindowCommands.update_actions)
   ▼
app::updater::UpdateHost::handle_action → worker thread → github / checksum / install
```

- `continuity_win::https_get` is a blocking WinHTTP GET (the workspace forbids `reqwest`/`tokio`); it follows same-scheme redirects, which GitHub's asset downloads use.
- `continuity_win::read_hklm_string` reads the installer's `InstallDir` record.
- State (`last_check_ms`, `skipped_version`) is `updates.json` beside the database (`continuity_persist::paths::updates_state_path`), so portable installs keep it folder-local.
- Actions serialize through a mutex in `UpdateHost`: two clicks cannot race the state file or launch two installers.
- The install re-fetches `releases/latest` and refuses if the version moved since the offer, so a stale banner can never install something other than what it named.

## Failure modes

- No network / TLS / proxy refusal: the background poll stays silent and records the attempt time (it will not retry every wake); the on-demand check banners the error.
- Checksum mismatch or an asset missing from the release: `Update failed: …` banner, nothing launched, staging directory left for inspection.
- The user declines the UAC prompt on an MSI install: `msiexec` exits, the script relaunches the still-installed old version.
- SmartScreen: the MSI is unsigned, so `msiexec` itself never prompts SmartScreen (that is a browser-download affordance), but an organization policy that blocks unsigned installers blocks this path too. Signing is tracked in `.docs/development/embeddable_cross_platform_roadmap.md` milestone 12.

## Tests

- `crates/app/src/updater/version.rs` — tag parsing and strict comparison.
- `crates/app/src/updater/github.rs` — asset selection, prerelease and foreign-tag rejection.
- `crates/app/src/updater/install_kind.rs` — MSI / portable / standalone classification.
- `crates/app/src/updater/state.rs` — JSON round trip and corrupt-file default.
- `crates/app/src/updater/checksum.rs` — digest and sums-file parsing.
- `crates/app/src/updater/install.rs` — script contents and executable discovery.
- `crates/win/src/http.rs` — URL parsing. No test ever touches the network.

## Key files

- `crates/app/src/updater.rs` and `updater/*.rs` — host: poll, actions, download, verify, stage.
- `crates/app/src/registry_updates.rs` — fan-out and close-all.
- `crates/ui/src/window_updates.rs` — banner and callback routing.
- `crates/ui/src/window_control.rs` — `UpdateOffer`, `UpdateAction`, `UpdateActions`, `request_window_close`.
- `crates/win/src/http.rs`, `crates/win/src/registry.rs` — WinHTTP and registry read.
- `crates/config/src/updates.rs` — `[updates]` section.
- `crates/command/src/help.rs` — `help.check_for_updates`.
