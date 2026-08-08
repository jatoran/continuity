// Scroll-extent contract for the touch surface.
//
// The textarea owns the scrollable extent but the projection owns what is
// visible, and the two diverge by construction — a long heading wraps into more
// projected rows than raw source rows. Every case here is about a document tail
// the reader could not reach.

import {
  animationFrames,
  mountEditor,
  scrollToBottom,
  settle,
} from "./browser-touch-helpers.mjs";

export async function runTouchScrollTests(ContinuityEditorElement, check) {
  const mount = document.querySelector("#mount");
  let assertions = 0;
  assertions += await runScrollExtentCases(ContinuityEditorElement, check, mount);
  assertions += await runCommandRailScrollCase(ContinuityEditorElement, check, mount);
  return assertions;
}


/**
 * The textarea owns the scroll extent but the projection owns what is visible.
 * A projection taller than the textarea's scrollable content strands its tail
 * below the fold, which is what makes a large paste unreachable on a phone.
 */
async function runScrollExtentCases(ContinuityEditorElement, check, mount) {
  const long = "The quick brown fox jumps over the lazy dog while the writer keeps typing";
  const corpora = {
    // Long headings wrap into more projected rows than raw source rows.
    longHeadings: Array.from({ length: 25 }, (_, i) => `# ${long} ${i}`).join("\n"),
    mixed: Array.from({ length: 15 }, (_, i) => `# ${long} ${i}\n${long} body ${i}.`).join("\n"),
    plain: Array.from({ length: 25 }, (_, i) => `${long} paragraph ${i}.`).join("\n"),
  };

  const { editor, input, projection, frame, dispose } = await mountEditor(
    ContinuityEditorElement, mount, "seed",
  );
  let count = 0;

  for (const [name, text] of Object.entries(corpora)) {
    editor.value = text;
    await settle();
    await scrollToBottom(input);

    const frameBounds = frame.getBoundingClientRect();
    const children = [...projection.children];
    const lastBounds = children[children.length - 1].getBoundingClientRect();

    // Padding is capped so it cannot inflate the textarea's own border box; any
    // surplus beyond that cap rides on the projection transform instead. The
    // two together must span the projection.
    const residual = Number.parseFloat(input.dataset.scrollExtentResidual ?? "0") || 0;
    check(
      projection.offsetHeight <= input.scrollHeight + residual + 1,
      `[${name}] scroll extent plus transform compensation spans the projection (projection ${projection.offsetHeight} vs ${input.scrollHeight}+${residual})`,
    );
    count += 1;
    check(
      lastBounds.bottom <= frameBounds.bottom + 1,
      `[${name}] the last projected line can be scrolled into view (overshoot ${Math.round(lastBounds.bottom - frameBounds.bottom)}px)`,
    );
    count += 1;

    const clipped = children.filter(
      (child) => child.getBoundingClientRect().top >= frameBounds.bottom - 1,
    ).length;
    check(clipped === 0, `[${name}] no trailing projected line is stranded below the fold`);
    count += 1;
  }

  dispose();
  return count;
}

/**
 * Android IMEs keep a composition open across taps. A tap that lands mid
 * composition is deferred until the engine matches the textarea; it must then
 * land on the projected glyph, not wherever the platform put the raw caret.
 */

/**
 * The command rail floats over the bottom of the frame, so the textarea is
 * inset above it. `min-height: 100%` used to hold the textarea at the full
 * frame height anyway, parking its final rows behind the rail where no amount
 * of scrolling could bring them out.
 */
async function runCommandRailScrollCase(ContinuityEditorElement, check, mount) {
  const long = "The quick brown fox jumps over the lazy dog while the writer keeps typing";
  const { editor, input, projection, frame, dispose } = await mountEditor(
    ContinuityEditorElement, mount,
    Array.from({ length: 30 }, (_, i) => `${long} paragraph ${i}.`).join("\n"),
    { commandRail: "on" },
  );
  let count = 0;

  const rail = editor.shadowRoot.querySelector(".command-rail");
  check(
    frame.classList.contains("command-rail-active") && rail && !rail.hidden,
    "the command rail is active for this case",
  );
  count += 1;

  const railBounds = rail.getBoundingClientRect();
  const inputBounds = input.getBoundingClientRect();
  check(
    inputBounds.bottom <= railBounds.top + 1,
    `the textarea box ends above the rail (bottom ${Math.round(inputBounds.bottom)} vs rail top ${Math.round(railBounds.top)})`,
  );
  count += 1;

  await scrollToBottom(input);
  const children = [...projection.children];
  const lastBounds = children[children.length - 1].getBoundingClientRect();
  check(
    lastBounds.bottom <= railBounds.top + 1,
    `the last line clears the rail once scrolled to the floor (overshoot ${Math.round(lastBounds.bottom - railBounds.top)}px)`,
  );
  count += 1;

  const hidden = children.filter(
    (child) => child.getBoundingClientRect().top >= railBounds.top - 1,
  ).length;
  check(hidden === 0, `no trailing line is stranded behind the rail (${hidden} hidden)`);
  count += 1;

  dispose();
  return count;
}

/**
 * Android IMEs hold a composition open continuously across ordinary typing
 * rather than briefly around one word, so every guard that treats composing as
 * an exceptional state is effectively permanent on a phone. Selection has to
 * keep working while one is open.
 */
