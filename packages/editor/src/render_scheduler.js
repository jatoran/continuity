/** Coalesce fast and full projection work around browser paint frames. */
export class RenderScheduler {
  #fullProjectionTimer;
  #renderFrame;
  #renderStartedAt;

  constructor({ canRender, renderFast, renderFull, onFrame }) {
    this.canRender = canRender;
    this.renderFast = renderFast;
    this.renderFull = renderFull;
    this.onFrame = onFrame;
  }

  schedule(startedAt) {
    if (this.#fullProjectionTimer !== undefined) {
      clearTimeout(this.#fullProjectionTimer);
      this.#fullProjectionTimer = undefined;
    }
    this.#renderStartedAt = this.#renderStartedAt === undefined
      ? startedAt
      : Math.min(this.#renderStartedAt, startedAt);
    if (this.#renderFrame !== undefined || !this.canRender()) {
      return;
    }
    this.#renderFrame = requestAnimationFrame(() => {
      this.#renderFrame = undefined;
      // Re-check at fire time: composition can begin between schedule and
      // frame, and a stale engine render would overwrite the live preview.
      if (!this.canRender()) {
        return;
      }
      const inputStartedAt = this.#renderStartedAt ?? performance.now();
      this.#renderStartedAt = undefined;
      this.renderFast();
      const paintedAt = performance.now();
      this.onFrame(inputStartedAt, paintedAt);
      this.#scheduleFullProjection();
    });
  }

  destroy() {
    if (this.#renderFrame !== undefined) {
      cancelAnimationFrame(this.#renderFrame);
    }
    if (this.#fullProjectionTimer !== undefined) {
      clearTimeout(this.#fullProjectionTimer);
    }
  }

  #scheduleFullProjection() {
    if (this.#fullProjectionTimer !== undefined) {
      clearTimeout(this.#fullProjectionTimer);
    }
    this.#fullProjectionTimer = setTimeout(() => {
      this.#fullProjectionTimer = undefined;
      if (this.canRender()) {
        this.renderFull();
      }
    }, 250);
  }
}
