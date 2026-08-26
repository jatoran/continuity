// Explicit soft-keyboard state for one editor.
//
// The gate in `soft_keyboard.js` has to answer one question its attribute
// cannot: is a keyboard up right now? Closing the gate keeps a dismissed
// keyboard down, and closing it with one up takes that keyboard away. Focus
// answers neither - the back gesture hides the IME without blurring, and a host
// `focus()` can leave a field focused with no IME at all. So it is tracked here,
// from the two things that do mean something:
//
//   - **Typing intent.** The editor resolved a gesture as "type here" and asked
//     for the IME. A request, not an observation, which is why it only moves the
//     state on a platform that has already proven it reports.
//   - **Visual-viewport occlusion.** The one signal Android gives for the IME:
//     the visual viewport loses height while the layout viewport does not. The
//     observation, and it corrects the request in both directions - a raise that
//     never arrived, and a dismissal that blurred nothing.
//
// With no evidence either way the answer is "down". That is the conservative
// side, not a guess: reporting "down" costs at most a keyboard the reader taps
// once more to raise, while reporting "up" wrongly takes away the one they are
// typing on and re-opens the defect this state exists to fix.
//
// This module is pure - no element, no global, every input through a method - so
// the transitions can be asserted directly rather than through a browser that
// has no IME to raise.

/** The keyboard is down, or there is no evidence that it is up. */
export const KEYBOARD_DOWN = "down";
/** The keyboard is up: taking the input mode away now would dismiss it. */
export const KEYBOARD_RAISED = "raised";

// Viewport height loss that reads as an IME rather than as a collapsing browser
// toolbar. Android's URL bar costs roughly 56px and pinch-zoom rather less;
// every soft keyboard worth the name takes several hundred.
const IME_OCCLUSION_PX = 120;

/**
 * Track one editor's keyboard state.
 *
 * @param {{occlusionPx?: number}} [options]
 */
export function createSoftKeyboardPolicy(options = {}) {
  const occlusionPx = Number.isFinite(options.occlusionPx)
    ? options.occlusionPx
    : IME_OCCLUSION_PX;
  let visibility = KEYBOARD_DOWN;
  let hasTypingIntent = false;
  // Whether this platform has ever reported IME occlusion. Until it has, a
  // request to raise is not evidence of anything and must not move the state.
  let isViewportObserved = false;
  let tallestViewportHeight = null;
  let preservedSelectionKey = null;

  return {
    get visibility() {
      return visibility;
    },
    /** Whether the editor has asked for the IME since the last dismissal. */
    get hasTypingIntent() {
      return hasTypingIntent;
    },
    /** Whether the visual viewport has ever reported an IME on this platform. */
    get isViewportObserved() {
      return isViewportObserved;
    },
    /** The selection a raise-only tap has already been spent on, if any. */
    get preservedSelectionKey() {
      return preservedSelectionKey;
    },

    isRaised() {
      return visibility === KEYBOARD_RAISED;
    },

    /**
     * Whether a selection gesture should close the gate.
     *
     * True is the state the gate exists for: nothing is up, so hold the surface
     * where the next touch cannot raise it. False is the case that gate could
     * never express - the keyboard is up, and closing it would be the dismissal
     * rather than the prevention.
     */
    shouldGateForSelection() {
      return visibility !== KEYBOARD_RAISED;
    },

    /**
     * The editor resolved a gesture as typing and asked for the IME.
     *
     * The optimistic raise covers the gap between asking and the viewport
     * reporting, during which a long-press would otherwise read the stale
     * "down" and dismiss a keyboard on its way up. It is taken only where the
     * viewport has already proven it reports, because that is the same signal
     * that will correct it if the keyboard never arrives.
     */
    noteTypingIntent() {
      hasTypingIntent = true;
      if (isViewportObserved) visibility = KEYBOARD_RAISED;
      return visibility;
    },

    /**
     * The keyboard is definitely gone: the surface blurred, or the host tore
     * the editor down. Clears the raise-only tap along with it, so the next
     * selection tapped into while the keyboard is down gets its own.
     */
    noteDismissed() {
      hasTypingIntent = false;
      visibility = KEYBOARD_DOWN;
      preservedSelectionKey = null;
      return visibility;
    },

    /**
     * Fold one visual-viewport height into the state and return the result.
     *
     * The tallest height ever seen is the baseline, because there is no other
     * way to know what "unoccluded" is for this window: the editor may be
     * mounted while the keyboard is already up, and the layout viewport does
     * not move for an IME.
     */
    observeViewportHeight(height) {
      if (!Number.isFinite(height) || height <= 0) return visibility;
      tallestViewportHeight = tallestViewportHeight === null
        ? height
        : Math.max(tallestViewportHeight, height);
      if (tallestViewportHeight - height >= occlusionPx) {
        isViewportObserved = true;
        visibility = KEYBOARD_RAISED;
        return visibility;
      }
      // The height came back: whatever was occluding the viewport is gone. This
      // is the back gesture, which blurs nothing and so raises no other event.
      if (visibility === KEYBOARD_RAISED) return this.noteDismissed();
      return visibility;
    },

    /**
     * Claim the one tap that may raise the keyboard without disturbing the
     * selection it lands in.
     *
     * Granted once per selection, and only while the keyboard is down. Once,
     * because the tap after it has to mean what a tap normally means - place a
     * caret - or a reader who selected a word could never tap their way out of
     * it. Keyed by the selection so the grant belongs to that range rather than
     * to a moment in time, and refused while the keyboard is up because there
     * the tap has nothing to raise and is simply a tap.
     */
    claimSelectionPreservation(selectionKey) {
      if (visibility === KEYBOARD_RAISED) return false;
      if (typeof selectionKey !== "string" || selectionKey.length === 0) return false;
      if (selectionKey === preservedSelectionKey) return false;
      preservedSelectionKey = selectionKey;
      return true;
    },

    /** A fresh selection gesture; the next tap into it earns its own grant. */
    releaseSelectionPreservation() {
      preservedSelectionKey = null;
    },
  };
}
