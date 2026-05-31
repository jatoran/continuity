# Motion

Canonical contract for UI motion, status transients, staggered concurrent transitions, and reduced motion.

## Principle

Motion is functional, not decorative. It serves comprehension by answering one of two questions:

- Is this the same thing moving to a new place?
- Is this a meaningful state transition I might otherwise miss?

If a transition does not answer one of those questions, it should be instant. The wrong shape is adding motion because a surface feels static; that spends attention without improving comprehension.

Change-blindness applies to small chrome and status changes as much as document content. Any value change that can be missed as a pure color or text swap needs a localized retinal transient: a brief fade, slide, pulse, or equivalent local disturbance. This is the status-chip rule for the C1/C2/C3 surfaces.

The principle-level rationale lives in [principles.md](principles.md). This file is the source of truth for durations, curves, stagger, and reduced-motion behavior.

## Shared Contract

- Curve: ease-out cubic, `1 - (1 - t)^3`, with `t` clamped to `0..=1`.
- Structural duration: 160 ms. This sits inside the allowed 120-240 ms band.
- Acknowledgement/status duration: 180 ms.
- Stagger offset: 60 ms between surfaces that begin in the same scheduling batch.
- Frame cadence: 16 ms while an animation is alive.
- Reduced motion: duration resolves to 0, no tween state is kept, and no animation timer is armed.

Implementation anchors:

- `crates/ui/src/motion.rs::MotionPolicy`
- `crates/ui/src/motion.rs::StaggerScheduler`
- `crates/ui/src/window_motion.rs::Window::start_motion_timer`
- `crates/render/src/motion.rs::SurfaceMotion`
- `crates/ui/src/edit_pulse.rs::{EditPulse, EditPulseKind, EDIT_PULSE_DURATION_MS, SELECTION_EXPAND_PULSE_DURATION_MS}`
- `crates/render/src/edit_pulse_paint.rs::paint_edit_pulse`

## Surface Policy

| Surface | Default |
|---|---|
| Wheel scroll | Fractional inertia, 60 ms exponential decay, snap below 50 DIP/s; `[editor].mouse_wheel_scroll_speed = 2.0` multiplies the base 3-line notch distance; reduced motion uses the same multiplier for the instant whole-line jump |
| `PageUp` / `PageDown` / `Home` / `End` | 160 ms ease-out cubic when smooth scroll is enabled |
| Marker reveal / markdown marker replacement | Instant |
| Caret-jump glow | 180 ms ease-out fade on the destination row |
| Caret motion tween | 160 ms ease-out cubic, large-jump-only |
| Theme / font preview hover | Instant preview; any resulting region motion uses the shared contract |
| Banner appear / dismiss | 160 ms ease-out fade/slide |
| Overlay open / close | 160 ms ease-out fade/slide |
| Chord HUD appear / dismiss | 160 ms ease-out fade/slide |
| Pane focus | 160 ms ease-out border crossfade |
| Tab activation | 160 ms ease-out active-tab accent |
| Status chip value change | 180 ms localized fade/slide transient — **only** for motion-eligible kinds; live counters (Position / Chars / Words / Lines / Selection / NumericSum) repaint in place. See § Status Chips. |
| Jump glow | 180 ms ease-out fade |
| Edit-region pulse (paste, duplicate, move-line, structural commands) | 120 ms ease-out fade across affected source rows |
| Undo / redo target echo | 120 ms ease-out fade on the post-undo selection row range |
| Selection-expand bounce | 80 ms ease-out fade on the head/anchor rows |
| Save-confirm chip | Solid 1.4 s, then 400 ms ease-out fade-out; appearance fade-in via the standard status-chip transient |
| Persistence-queue chip | Visible while `PersistClient::unflushed_bytes > 0`; appearance fade-in via the status-chip transient, disappearance is instant (the chip is a confidence cue, not a value-change) |

Everything else is instant unless this file explicitly assigns a motion role.

## Stagger

When two or more visible regions change in the same batch, call sites must request a slot from `StaggerScheduler`. The first region starts immediately, the second starts 60 ms later, the third 120 ms later, and so on. This prevents a pane split plus tab activation, overlay open plus focus shift, or theme reload plus reflow from collapsing into one scene change.

The scheduler is UI-thread state on `Window`, not renderer state. Render receives already-projected `SurfaceMotion` values in `DrawParams`.

## Reduced Motion

`[ui].reduced_motion = true` is the application-level reduced-motion toggle. It must:

- clear active motion state when applied;
- skip scheduling new `MotionSpan`s;
- skip caret tweens and jump-glow fades;
- produce zero animation frames for overlay, banner, chord HUD, chrome, status-chip, scroll, caret-tween, and jump-glow surfaces;
- render final static state immediately instead of running a 1 ms or 16 ms animation.

The zero-frame behavior is part of the contract. Tests should assert absence of scheduled frames, not merely shorter durations.

## Status Chips

State-change segments and chips animate on value change. The transient stays localized to the changed segment; it must not flash the entire status bar.

**Eligibility carve-out — high-frequency counters do not animate.** The 180 ms acknowledgement transient paints the new segment text a second time at `top + translate_y_dip` (sliding from `−3 DIP` to `0` with alpha fading `1.0 → 0.0`) on top of the steady-state draw. For segments whose value updates every keystroke / every caret move / every selection-drag tick, consecutive changes spaced under 180 ms apart keep the transient continuously active — perpetual double-image with a vertical offset, perceived as ghost-offset blur, not as a tactile bump. The carve-out is enforced by `StatusMotionState::update`'s call to `is_motion_eligible_kind(StatusBarSegmentKind)`:

| Kind | Eligible? |
|---|---|
| `Position`, `Chars`, `Words`, `Lines`, `Selection`, `NumericSum` | **no** — repaints in place |
| `Encoding`, `LineEndings`, `Language`, `IdleStale` | yes — rare state flips |
| `Chip`, `NoticeChip`, `PersistQueueChip` | yes — chip motion is the point |

Implementation anchors:

- `crates/ui/src/status_motion.rs::StatusMotionState::update`
- `crates/ui/src/status_motion.rs::is_motion_eligible_kind`
- `crates/render/src/status_bar.rs::StatusTransientDraw`

## Ownership

All mutable motion state is owned by the UI thread through `Window`:

- `motion_policy`
- `stagger_scheduler`
- `overlay_motion`
- `chrome_motion`
- `status_motion`
- `chord_hud_motion`
- caret tween / jump glow / edit pulse state
- `status_notices` (one-shot status-bar chips — α.1 save-confirm notice lives here)

The render crate owns only immutable paint inputs and stateless projection helpers. It must not hold timers or mutable animation clocks.
