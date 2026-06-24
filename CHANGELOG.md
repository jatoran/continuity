# Changelog

## 0.4.2

- Files that change on disk outside continuity now stay in sync. Reopening or
  reloading a file that another program changed shows its current content
  instead of an old cached copy. If you have no unsaved edits it updates
  silently; if you do, you are prompted instead of losing either version.
- Saving can no longer silently overwrite a file that changed on disk behind
  your back. If the file changed since you opened it, the save is held and you
  get a banner with clickable Reload / Keep mine / Show diff buttons (Show diff
  opens a real line-by-line comparison), so an outside edit can't be lost.
- Reopening a file that is already open now jumps to its existing tab instead
  of opening a second window for it.
- Every theme now has its own distinct text-selection color, so selected text
  no longer looks the same as the current-line highlight, and the current-line
  highlight is more subtle. Applies to all 17 built-in themes.

