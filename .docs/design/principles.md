# UX principles

The longer-form rationale for the design ethos in `CLAUDE.md`. Read this once when you join the project; consult it when a UX decision feels under-specified.

Specific decisions (animation timings, modal-vs-banner per surface, keymap base, trash retention defaults, etc.) live in `defaults.md`. This doc is the *why*; that doc is the *what*.

---

## Saving = export

Every keystroke is durable to SQLite within 400 ms p99. Recovery on launch replays from snapshots + edit log with checksum verification, halting at the first mismatch behind a user-visible banner. Saving to a file is an *export* operation — the DB is the canonical store, files are derived artifacts.

**Implication.** The user never loses work by closing a tab, crashing, or losing power between explicit saves. We never have to write a "you have unsaved changes" modal, because there are no unsaved changes — there are exports that are behind, which is a separate concern.

**Source-bytes are canonical** (00_OVERVIEW global invariant): undo, persistence, search, and file I/O all speak source bytes. The display map is a derived projection; deleting it would yield a degraded but correct editor.

---

## Banner, not modal

Modal interruption is reserved for two cases:
- Genuinely destructive confirmations (close-with-unsaved-export, permanent-purge-from-trash).
- Settings dialog (an intentional context switch).

Everything else is a banner: file-watcher detected external change, recovery halted at checksum mismatch, validation error, encoding warning. Banners stack at the top of the pane, dismiss with click-or-Esc, and never block input.

**Why.** Flow-state HCI: task-switch ramp-up cost (~30 s of re-entry) is wildly disproportionate to the typical 2 s of modal content. A modal is a tax we charge against the user's attention. Most "errors" don't justify the tax; they justify a notification the user can address when they choose.

**Audit rule.** When adding any user-facing surface, ask: is this interruption *reversible*? If yes, it's a banner. If no, it might be a modal — but check first whether the destructive operation could instead be undoable (trash + restore is reversible; a modal becomes unnecessary).

---

## The rope is canonical; display map projects

The source rope (`Arc<Rope>`) is the only thing crossing thread boundaries and the only authority on content. The display map is a derived projection that hides, replaces, folds, and soft-wraps source bytes into display rows — but it never invents content the rope doesn't have.

**Implication for features.** Anything you add must work with the display map torn out. Markdown markers `==highlight==` are hidden via `Hidden` segments — the *bytes are still there* in the rope, in undo, in search, in file export. Same for fold ranges. Same for the alignment row in pipe tables.

**The wrong shape.** A feature that stores content somewhere other than the rope (a separate "tables" widget store, a "drawings" canvas, anything that isn't bytes-in-the-rope) breaks every downstream invariant: search doesn't find it, undo doesn't reach it, export drops it, recovery can't replay it. If you're tempted to add such a feature, the right question is "how do I encode this in markdown source bytes and project it richly?" — not "how do I bolt on a parallel storage model?"

---

## Motion is functional

Motion serves comprehension. It tells the user *this is the same thing, it just moved* (caret jump glow, tab activation, pane focus) or *this is a meaningful state transition* (theme swap or status-chip value change). It does not serve decoration or perceived "smoothness."

The canonical timing, curve, stagger, transient, and reduced-motion contract lives in [motion.md](motion.md). That contract is also where the change-blindness rule is applied: a silent color/text swap is not enough when a status value changes.

**The wrong shape.** Adding motion because a surface "feels static" without it. If the user doesn't need to track the change, easing in is theater. The cost is real (eye dwell, perceived sluggishness over many invocations).

---

## Layout shifts preserve caret-line screen y

When font scale, soft-wrap width, theme size, or pane size changes, the line containing the caret stays anchored at the same screen y. Content above or below reflows; the caret line does not move on screen.

**Why.** Per the Model Human Processor, re-targeting visual attention costs ~230 ms per shift. A reflow that moves the caret off its original screen y forces the user to re-acquire it. Multiply by every settings-tweak, theme-switch, or pane-resize, and the cost is the dominant friction of the editor. Anchoring removes it entirely.

**Audit rule.** Anything that can cause reflow must be evaluated against the anchor. New examples must explicitly think through the anchor contract.

**Implementation.** The contract is realized by `crates/ui/src/window_caret_anchor.rs::Window::with_caret_line_anchored`. Every reflow-causing call site routes through that helper; never write parallel anchor logic. The audit + remediation that landed the helper lives at `.docs/development/archive/audit_caret_anchor.md`.

**Explicit exception — live drag-resize.** Inside a Win32 modal sizing loop (`WM_ENTERSIZEMOVE` → `WM_EXITSIZEMOVE`), per-tick `WM_SIZE` deltas take an unanchored fast path; a single anchor captured at the loop's start is restored once at its end. The contract holds for the final frame the user settles on; intermediate frames during the drag are deliberately not anchored because (a) the per-tick anchor builds a full `FrameDisplay` projection that dominates resize CPU, and (b) the user's eye is tracking the resize handle, not the caret line. Detail in [`features/caret.md`](features/caret.md) §"Screen-y anchor across reflow".

---

## Trust the writer

The editor's job is to serve writing, not to police it.

- **No silent auto-correct.** If we transform what the user typed, there's an undo to undo it, and the transformation is opt-in via settings (not silent and clever).
- **No moralizing.** We don't lint prose. We don't underline "passive voice." We don't suggest synonyms.
- **No input traps.** A surface that opens (overlay, palette, modal) must always offer a clear way out (Esc, click outside, dismiss-and-undo). Reversibility is mandatory.
- **Conservative defaults.** Every potentially-annoying feature ships off. The user opts in. The list of "things we silently do" is short and well-justified.

**The wrong shape.** Features that "help" by transforming, suggesting, or interrupting. The bar for an opt-out-by-default feature is "this is genuinely universally desired and the small minority who don't want it can toggle it off without confusion."

---

## How these compose

These six principles aren't a checklist — they're a frame. A new feature passes if:

- Its content lives in the rope and survives display-map removal.
- It doesn't introduce a modal for a reversible operation.
- Its motion serves comprehension or is instant.
- It doesn't break the caret-line anchor on reflow.
- Its defaults are conservative and its interruptions are dismissable.
- It can be added, removed, or rebuilt without invalidating recovery.

If a feature feels off, walk it back to which principle it violates. Usually the answer is "this needs to be a banner, not a modal" or "this is silently transforming input."

---

## See also

- `CLAUDE.md` — the one-screen ethos summary (loaded every session).
- `defaults.md` — specific decisions derived from these principles.
- `00_OVERVIEW.md` — global invariants and key trade-offs (the architectural counterpart to this doc).
- `performance.md` — perf budgets that constrain motion and reflow choices.
