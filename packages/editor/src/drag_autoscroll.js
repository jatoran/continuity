// Edge auto-scroll for a selection drag.
//
// A claimed selection drag refuses the scroll so the surface cannot pan out
// from under it, which means the finger can no longer reach text beyond the
// viewport on its own. Native text fields solve this by scrolling once the
// finger enters a band at the top or bottom edge, at a rate that grows with how
// far past it the finger is; the selection keeps extending as content moves.

const EDGE_BAND_PX = 56;
const MINIMUM_SPEED_PX = 2;
const MAXIMUM_SPEED_PX = 22;

/** Scroll a surface while a drag rests near its edges, re-extending as it moves. */
export class DragAutoScroller {
  #frame;
  #onScrolled;
  #point;
  #scroller;

  /** Track the drag. Starts, adjusts, or stops the scroll to match the finger. */
  update(scroller, clientX, clientY, onScrolled) {
    this.#scroller = scroller;
    this.#point = { clientX, clientY };
    this.#onScrolled = onScrolled;
    if (this.#velocity() === 0) {
      this.stop();
      return;
    }
    this.#frame ??= requestAnimationFrame(this.#tick);
  }

  stop() {
    if (this.#frame !== undefined) cancelAnimationFrame(this.#frame);
    this.#frame = undefined;
  }

  /** Pixels per frame, signed, ramped by how far into the band the finger is. */
  #velocity() {
    if (!this.#scroller || !this.#point) return 0;
    const bounds = this.#scroller.getBoundingClientRect();
    const { clientY } = this.#point;
    const ramp = (depth) => Math.min(
      MAXIMUM_SPEED_PX,
      MINIMUM_SPEED_PX + (depth / EDGE_BAND_PX) * (MAXIMUM_SPEED_PX - MINIMUM_SPEED_PX),
    );
    if (clientY < bounds.top + EDGE_BAND_PX) return -ramp(bounds.top + EDGE_BAND_PX - clientY);
    if (clientY > bounds.bottom - EDGE_BAND_PX) return ramp(clientY - (bounds.bottom - EDGE_BAND_PX));
    return 0;
  }

  #tick = () => {
    this.#frame = undefined;
    const velocity = this.#velocity();
    if (velocity === 0) return;
    const before = this.#scroller.scrollTop;
    this.#scroller.scrollTop = before + velocity;
    // Already at the top or the bottom: stop rather than spin a frame loop.
    if (this.#scroller.scrollTop === before) return;
    this.#onScrolled?.(this.#point);
    this.#frame = requestAnimationFrame(this.#tick);
  };
}
