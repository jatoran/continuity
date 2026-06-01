# Changelog

## 0.2.0

- Markdown links now render in the theme link colour and open in the default
  browser on a plain click; a scheme-less target such as `www.example.com`
  defaults to `https://`.
- `Ctrl+K` inserts or wraps a markdown link with smart caret placement — a
  selected URL becomes the target (caret in the label), selected text becomes
  the label (caret in the URL).
- Task checkboxes: `Ctrl+E` toggles a `- [ ]` task bullet; a `- [ ]` line now
  renders a single checkbox instead of a bullet *and* a box; clicking the box
  toggles it without corrupting the line; the box stays rendered while you edit
  the line's text, and the cursor shows a pointer over it.
- Tab dragging follows Chrome: tabs slide to reorder within the strip and only
  tear off into a new window on a deliberate vertical pull.
- Closing the app intentionally no longer reopens the previous buffers on the
  next launch (a crash still restores the session); bring closed windows back
  with `Ctrl+Shift+T`.
- `Ctrl+Backspace` on a whitespace-only line deletes back to the line start
  instead of merging into the line above.
- New application icon.

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
