# assets

Screenshots and demo captures referenced by the public README and the marketing drafts. `scripts/sync-public.ps1` copies this directory into the public repository so relative image paths resolve on GitHub.

| File | Used by |
|---|---|
| `media/hero.png` | Public README, engine blog draft |
| `media/live-markdown.gif` | Public README, durability blog draft |
| `media/crash-safety.gif` | Public README, durability blog draft |
| `media/find.png` | Public README |

## How these were captured

**No personal notes are involved and none should ever be.** The capture harness runs a throwaway instance:

- `--portable`, so the database lives in a `data\` folder beside a copied exe in a temp directory rather than in `%APPDATA%`.
- A `notes\` folder of invented content: a fictional project, a reading list, a meeting note.
- The database is deleted before every run, so captures are reproducible and never accumulate state.

Recording is `ffmpeg` with `gdigrab` over a fixed screen region, converted to GIF with a two-pass generated palette (`palettegen` / `paletteuse`). The default 216-colour web palette destroys anti-aliased text on a dark UI.

### Two things that will bite whoever does this next

**`SendKeys` cannot drive this application's key chords.** The window reads modifier state with `GetKeyState` on its wndproc thread, and `SendKeys`-synthesized modifiers are not visible there, so `Ctrl+F` and friends silently do nothing. Plain characters work fine. Use `keybd_event`, which goes through the driver layer and updates the key state table. This is the same constraint the pane-split e2e tests document.

**`SetForegroundWindow` from a background process is refused.** Windows' foreground lock means a script launched from another process cannot hand focus to the editor, so synthesized keys land nowhere and the recording captures an empty buffer. A synthetic mouse click grants foreground rights and works. This matters specifically after starting `ffmpeg`, which briefly takes the foreground even when started hidden.

Both cost a recording each to discover.

### Framing notes

- The caret line always reveals its own raw source. That is the point of the display map, but it means the final frame of a clip should move the caret away with `Ctrl+Home` so the closing image shows rendered output.
- List continuation already inserts the `- [ ] ` marker, so typing one on a continuation line produces `- [ ] [x] ...`.
- Crop away the empty lower half of the window before converting. The window is 1600x1000 and the content occupies roughly the top third.
