# Changelog

## 0.1.1 - Unreleased

- Portable zip launches now auto-detect a beside-the-exe `data\` directory, so
  double-clicking `continuity.exe` keeps settings, themes, keymap, notes, and
  backups inside the extracted folder.
- License changed to MIT.

## 0.1.0

- Native Win32 markdown editor with durable SQLite-backed editing.
- Plain-text source editing with live markdown projection and preview-oriented
  rendering.
- Multi-pane and multi-window session restoration.
- Configurable keymap, settings, themes, and portable mode.
- Large-buffer projection, row-index, and render-cache performance work.
- Release packaging through `cargo xtask package`, MSI installer assembly
  through `cargo xtask installer`, code signing through `cargo xtask sign`,
  and shippable artifact assembly through `cargo xtask release`.
- Default C-mark app icon embedded into the release executable and reused by
  the MSI for shortcuts, Programs & Features, and file associations.
- MSI major-upgrade support so installing a newer `continuity-setup.msi`
  replaces an older installed version in place.
- MSI install options include a checked desktop-shortcut checkbox before the
  install starts.
