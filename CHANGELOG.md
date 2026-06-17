# Changelog

## 0.4.1

- Fixed the viewport jumping and warping while you type in large documents.
  The 0.4.0 fix for this was incomplete — on long files (especially with a
  big table above where you're typing) the view could still lurch around or
  snap your current line to the top of the screen. Typing on a line that's
  already on screen now keeps the view steady.

## 0.4.0

- Fixed a frequent crash when pressing `Ctrl+Z` (undo), especially after
  pasting and typing — the editor no longer dies on undo, and a UI panic no
  longer takes down the whole app.
- Markdown lists are smarter: pressing Enter on a checkbox line continues a
  checkbox, ordered lists auto-renumber when you insert or reorder items, and
  `Ctrl+R` on a numbered list converts it to bullets.
- `Ctrl+B` (and italic/strikethrough/code) now *removes* the formatting when
  the caret is already inside it, instead of inserting empty markers.
- New `Ctrl+Shift+R`: toggle bullets on the selected lines and indent the
  continuation lines. (`Reload keymap` moved to `Ctrl+K Ctrl+M`.)
- New `Ctrl+K Ctrl+S`: strip leading and trailing whitespace.
- `Shift+Backspace` and `Shift+Delete` now delete instead of doing nothing.
- Tabs: titles now use the available width and shrink only when crowded, with
  `<>` scroll arrows (and Shift+mouse-wheel) when they overflow; the close
  button hides when tabs get small so you can't hit it by accident;
  double-clicking empty ribbon space opens a new tab; `Ctrl+W` on an unsaved
  untitled tab now warns (and notes the content stays recoverable); and each
  tab remembers its cursor and scroll position when you switch away and back.
- Clicking a rendered checkbox toggles it without moving the caret; `Ctrl` +
  double-click adds another word to a multi-selection.
- The current cursor line now has its own highlight, and the line you hover
  shows its number in the gutter.
- Fold markers in the gutter only appear on hover or where something is
  collapsed; indent guides, minimap click/drag, and outline-heading clicks now
  line up correctly; typing in large documents no longer makes the view jump;
  active vs inactive tabs are easier to tell apart in every theme.
- Paste: rich text / HTML copied from a browser is converted to Markdown, and
  pasted Markdown tables render correctly.
- Find & Replace: regex can now match across line breaks (use `\n`).
- The status bar no longer wraps; notifications dismiss on their own, sit below
  the tab bar, and no longer clip their text.
- Command palette: removed the "cycle theme" command; typing "theme" now
  surfaces "pick theme" first.
- Save dialog: defaults to Markdown and appends `.md` automatically, and
  pre-fills the file name from the tab's title (selected, so you can just type
  to replace it).

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
