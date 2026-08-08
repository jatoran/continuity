# Changelog

## 0.4.7

- In a folder or vault, middle-clicking a file in the sidebar now opens it in a
  new tab, matching Ctrl+click. Handy for opening several notes in quick
  succession without losing your current one.
- The sidebar now highlights the file you're currently viewing. The highlight
  follows the focused tab as you switch between notes, and clears when the tab
  isn't a file inside the open folder.

## 0.4.6

- Fixed vault autosave dying "after a while" once a second window was open. Save
  confirmations were delivered on a channel shared by every window, so a second
  window could consume another window's save acknowledgement — leaving the
  first window believing a save was still in progress and quietly skipping all
  further autosaves for that note. Save results are now delivered only to the
  window that made the request. A watchdog also clears any acknowledgement that
  is ever lost, so autosave can no longer get permanently stuck for a note.

## 0.4.5

- Vaults now remember their open tabs. Closing a vault and reopening it — via
  the vault launcher, a desktop shortcut, or `--vault` — restores the same open
  files, the focused tab, and each tab's scroll position. The state is stored
  portably beside the vault in `.continuity/workspace.toml`, so it travels with
  the folder. Only files inside the vault are remembered; untitled scratch
  buffers are not.

## 0.4.4

- Fixed vault autosave silently wedging mid-session. Previously a save that was
  briefly refused — because a manual Ctrl+S raced the automatic save, or an
  external program (cloud sync, antivirus, backup) touched the file — could
  permanently stop autosave for that note with no visible prompt: the unsaved
  indicator stayed on and the file quietly stopped updating. Autosave now only
  pauses when the file genuinely still differs on disk (with the reload / keep
  mine / show diff banner), and resumes on its own once a transient conflict
  clears.

## 0.4.3

- Vault autosave now keeps the file on disk in sync more reliably. Closing a
  tab, closing the window, switching tabs/panes, or clicking away now exports
  every changed note under the vault first, instead of only the ones already
  waiting on the debounce timer, so the file no longer lags behind what you
  typed just before leaving.
- Closing a tab in an autosave vault no longer shows an "unsaved changes"
  prompt. Because autosave owns the file, closing simply writes the latest
  content and closes. The prompt still appears for untitled buffers and for a
  note whose autosave is paused by an unresolved external-change conflict.

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

