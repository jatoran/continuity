# Changelog

## 0.3.0

- Single instance: opening a file or launching the shortcut while Continuity is
  already running no longer reopens a duplicate copy of every window — the file
  opens in the running instance instead.
- Default indentation is now tabs. `Shift+Tab` outdents tab- and space-indented
  lines alike.
- Soft-wrapped text no longer spills past the right edge, regardless of font,
  zoom, or indentation. Wrapped bullet and indented lines now hang-indent so the
  continuation lines line up under the content.
- Ordered list numbers (`1.`, `2.`) render as numbers instead of bullets.
- The window title (and taskbar / Alt-Tab entry) now shows the active tab's
  name instead of "continuity".
- A pane is only highlighted as active when its window actually has focus, and
  Alt-Tabbing away no longer leaves the hold-modifier hotkey panel stuck on
  screen.
- `Home` on a bullet line jumps to the start of the text after the bullet first,
  then to the true line start on a second press.
- `Ctrl+Shift+J` join is smarter: it joins lines with a space but only removes
  one blank line between paragraphs, preserving section breaks, and strips list
  markers from joined items.
- Toggling bullets (`Ctrl+R`) or checklists (`Ctrl+E`) across multiple lines
  skips blank lines and toggles the whole selection together.
- New command to strip all markdown formatting (bold, headings, bullets, links)
  from the selected text.
- The outline sidebar can be drag-resized.
- `Ctrl+F` / `Ctrl+H` pre-fill the find field from the selected text and select
  it so you can type over it; the field is always highlighted on open.
- Hold `Ctrl` while selecting to highlight multiple separate regions; copy now
  copies every highlighted region.
- Markdown links to in-document headings (`[text](#heading)`) jump to the
  heading.
- The copy button on a multi-line inline code block copies the whole block.

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
