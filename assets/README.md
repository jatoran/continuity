# assets

Screenshots and demo captures referenced by the public README and the marketing drafts.
`scripts/sync-public.ps1` copies this directory into the public repository so relative image paths resolve on GitHub.

| File | Shows | Used by |
|---|---|---|
| `media/hero.png` | A vault note with headings, bold, a link, checkboxes, a table, and a highlighted Rust block; the caret line shows its raw `#` source | Public README, engine blog draft |
| `media/live-markdown.gif` | A note typed from empty, each block rendering as the caret leaves it | Public README, durability blog draft |
| `media/crash-safety.gif` | A note typed with vault autosave off, the process killed with `TerminateProcess` mid-sentence, a bare relaunch restoring the note, tab, unsaved marker, and caret, then typing resuming | Public README, durability blog draft |
| `media/find.png` | Live match highlighting with the find bar's toggles | Public README |

The current set was captured on 2026-11-08 from the published `v0.4.13` portable build.

Every capture comes from a throwaway portable instance with its own empty database and a folder of invented notes.
No personal notes are involved and none should ever be.
Frames are taken from the editor window itself, so nothing else on the screen can appear in them.
The kill gap in `crash-safety.gif` is a generated card.
