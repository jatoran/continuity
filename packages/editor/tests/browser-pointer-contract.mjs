/** Locate a known glyph on a wrapped projected row using real DOM geometry. */
export function projectedPointerBounds(page, evaluate) {
  return evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Pointer hit-test document"]');
    host.scrollIntoView({ block: 'center' });
    const input = host.shadowRoot.querySelector('textarea');
    const rect = input.getBoundingClientRect();
    const line = host.shadowRoot.querySelector('.projection [data-line="0"]');
    const targetIndex = line.textContent.indexOf('TARGET');
    const walker = document.createTreeWalker(line, NodeFilter.SHOW_TEXT);
    let consumed = 0;
    let targetRect;
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (targetIndex >= consumed && targetIndex < consumed + node.length) {
        const range = document.createRange();
        range.setStart(node, targetIndex - consumed); range.setEnd(node, targetIndex - consumed + 1);
        targetRect = range.getBoundingClientRect();
        break;
      }
      consumed += node.length;
    }
    const firstRange = document.createRange();
    firstRange.setStart(line.firstChild, 0); firstRange.setEnd(line.firstChild, 1);
    const firstRect = firstRange.getBoundingClientRect();
    return {
      left: rect.left, top: rect.top, width: rect.width, height: rect.height,
      targetX: targetRect.left + targetRect.width * .2,
      targetY: targetRect.top + targetRect.height / 2,
      expectedTarget: host.value.indexOf('TARGET'),
      targetIsWrapped: targetRect.top > firstRect.top + 2,
    };
  })()`);
}

/** Measure the custom primary caret against the active line's real glyph geometry. */
export function wrappedVisualCaretAlignment(page, evaluate) {
  return evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Pointer hit-test document"]');
    const root = host.shadowRoot;
    const input = root.querySelector('textarea');
    const line = root.querySelector('.projection [data-line="0"]');
    const caret = root.querySelector('.primary-caret');
    const targetOffset = input.selectionStart;
    const walker = document.createTreeWalker(line, NodeFilter.SHOW_TEXT);
    let consumed = 0;
    let targetRect;
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (targetOffset >= consumed && targetOffset < consumed + node.length) {
        const range = document.createRange();
        range.setStart(node, targetOffset - consumed);
        range.setEnd(node, targetOffset - consumed + 1);
        targetRect = range.getBoundingClientRect();
        break;
      }
      consumed += node.length;
    }
    const caretRect = caret?.getBoundingClientRect();
    return {
      caretExists: Boolean(caretRect),
      caretLeftDelta: caretRect && targetRect ? Math.abs(caretRect.left - targetRect.left) : null,
      caretTopDelta: caretRect && targetRect ? Math.abs(caretRect.top - targetRect.top) : null,
      sourceVisible: line?.dataset.sourceVisible,
      nativeCaretColor: getComputedStyle(input).caretColor,
    };
  })()`);
}

/** Require native Arrow movement to repaint the projected caret before editing. */
export async function assertArrowCaretRepaint(page, evaluate) {
  const before = await evaluate(page, `(() => {
    const root = document.querySelector('[aria-label="Pointer hit-test document"]').shadowRoot;
    return {
      offset: root.querySelector('textarea').selectionStart,
      left: root.querySelector('.primary-caret').getBoundingClientRect().left,
    };
  })()`);
  await page.send("Input.dispatchKeyEvent", {
    type: "keyDown", key: "ArrowRight", code: "ArrowRight", windowsVirtualKeyCode: 39,
  });
  await page.send("Input.dispatchKeyEvent", {
    type: "keyUp", key: "ArrowRight", code: "ArrowRight", windowsVirtualKeyCode: 39,
  });
  await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  const alignment = await wrappedVisualCaretAlignment(page, evaluate);
  const after = await evaluate(page, `(() => {
    const root = document.querySelector('[aria-label="Pointer hit-test document"]').shadowRoot;
    return {
      offset: root.querySelector('textarea').selectionStart,
      left: root.querySelector('.primary-caret').getBoundingClientRect().left,
    };
  })()`);
  if (after.offset !== before.offset + 1 || after.left === before.left
      || alignment.caretLeftDelta > 1 || alignment.caretTopDelta > 1) {
    throw new Error(`ArrowRight did not repaint projected caret: ${JSON.stringify({ before, after, alignment })}`);
  }
  return { offset: after.offset, leftDelta: after.left - before.left };
}

/** Exercise exact projected Shift+click and drag selection on a wrapped source line. */
export async function assertWrappedSelectionInteractions(page, evaluate) {
  const points = await evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Pointer hit-test document"]');
    const line = host.shadowRoot.querySelector('.projection [data-line="0"]');
    const pointAt = (offset) => {
      const walker = document.createTreeWalker(line, NodeFilter.SHOW_TEXT);
      let consumed = 0;
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        if (offset >= consumed && offset < consumed + node.length) {
          const range = document.createRange();
          range.setStart(node, offset - consumed); range.setEnd(node, offset - consumed + 1);
          const rect = range.getBoundingClientRect();
          return { x: rect.left + rect.width * .2, y: rect.top + rect.height / 2 };
        }
        consumed += node.length;
      }
      throw new Error('selection test offset is outside the projected line');
    };
    const start = 8;
    const end = host.value.indexOf('TARGET') + 4;
    return { start, end, startPoint: pointAt(start), endPoint: pointAt(end) };
  })()`);
  await clickAt(page, points.startPoint);
  await clickAt(page, points.endPoint, 8);
  await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  const shiftClick = await selectionVisualState(page, evaluate);
  assertExactVisualSelection(shiftClick, points, "Shift+click");

  await page.send("Input.dispatchMouseEvent", {
    type: "mousePressed", ...points.startPoint, button: "left", clickCount: 1,
  });
  await page.send("Input.dispatchMouseEvent", {
    type: "mouseMoved", ...points.endPoint, button: "left", buttons: 1,
  });
  await page.send("Input.dispatchMouseEvent", {
    type: "mouseReleased", ...points.endPoint, button: "left", clickCount: 1,
  });
  await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  const drag = await selectionVisualState(page, evaluate);
  assertExactVisualSelection(drag, points, "projected drag");
  return { length: drag.end - drag.start, visualRows: drag.visualRows };
}

/** Double-click selects the projected word; triple-click selects its source line. */
export async function assertProjectedMultiClickSelections(page, evaluate, point) {
  await clickAt(page, point, 0, 2);
  await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  const word = await selectionVisualState(page, evaluate);
  const wordStart = await evaluate(page, `document.querySelector('[aria-label="Pointer hit-test document"]')
    .value.indexOf('TARGET')`);
  const wordEnd = wordStart + "TARGET".length;
  if (word.start !== wordStart || word.end !== wordEnd
      || word.engine.anchor.line !== 0 || word.engine.anchor.byteInLine !== wordStart
      || word.engine.head.line !== 0 || word.engine.head.byteInLine !== wordEnd
      || word.engine.kind !== "caret") {
    throw new Error(`projected double-click did not select the word: ${JSON.stringify(word)}`);
  }

  await clickAt(page, point, 0, 3);
  await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  const line = await selectionVisualState(page, evaluate);
  const lineEnd = await evaluate(page, `document.querySelector('[aria-label="Pointer hit-test document"]')
    .value.indexOf('\\n')`);
  if (line.start !== 0 || line.end !== lineEnd
      || line.engine.anchor.line !== 0 || line.engine.anchor.byteInLine !== 0
      || line.engine.head.line !== 0 || line.engine.head.byteInLine !== lineEnd
      || line.engine.kind !== "lineWise") {
    throw new Error(`projected triple-click did not select the line: ${JSON.stringify(line)}`);
  }
  return { wordLength: word.end - word.start, lineLength: line.end - line.start };
}

async function clickAt(page, point, modifiers = 0, clickCount = 1) {
  await page.send("Input.dispatchMouseEvent", {
    type: "mousePressed", ...point, button: "left", modifiers, clickCount,
  });
  await page.send("Input.dispatchMouseEvent", {
    type: "mouseReleased", ...point, button: "left", modifiers, clickCount,
  });
}

function selectionVisualState(page, evaluate) {
  return evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Pointer hit-test document"]');
    const root = host.shadowRoot; const input = root.querySelector('textarea');
    const visualElements = [...root.querySelectorAll('.visual-selection')];
    const visual = visualElements.map((item) => item.getBoundingClientRect())
      .filter((rect) => rect.width > 0);
    return {
      start: input.selectionStart, end: input.selectionEnd,
      engine: host.snapshot().selections[0], visualRows: new Set(visual.map((rect) => Math.round(rect.top))).size,
      nativeSelectionBackground: getComputedStyle(input, '::selection').backgroundColor,
      nativeSelectionColor: getComputedStyle(input, '::selection').color,
      nativeSelectionFill: getComputedStyle(input, '::selection').getPropertyValue('-webkit-text-fill-color'),
    };
  })()`);
}

function assertExactVisualSelection(state, points, label) {
  const expected = { start: points.start, end: points.end };
  if (state.start !== expected.start || state.end !== expected.end
      || state.engine.anchor.byteInLine !== expected.start
      || state.engine.head.byteInLine !== expected.end
      || state.visualRows < 2
      || state.nativeSelectionBackground !== "rgba(0, 0, 0, 0)"
      || state.nativeSelectionColor !== "rgba(0, 0, 0, 0)"
      || state.nativeSelectionFill !== "rgba(0, 0, 0, 0)") {
    throw new Error(`${label} projection selection failed: ${JSON.stringify(state)}`);
  }
}

/** Exercise the physical Ctrl+E task-toggle selection contract. */
export async function assertTaskShortcutCarets(page, evaluate) {
  for (const [source, caretBefore, expectedText, expectedCaret] of [
    ["", 0, "- [ ] ", 6],
    ["buy milk", 4, "- [ ] buy milk", 10],
  ]) {
    await evaluate(page, `(() => {
      const host = document.querySelector('[aria-label="Pointer hit-test document"]');
      const input = host.shadowRoot.querySelector('textarea'); host.value = ${JSON.stringify(source)};
      input.focus(); input.setSelectionRange(${caretBefore}, ${caretBefore});
      input.dispatchEvent(new Event('select', { bubbles: true }));
    })()`);
    await page.send("Input.dispatchKeyEvent", { type: "keyDown", key: "e", code: "KeyE", modifiers: 2, windowsVirtualKeyCode: 69 });
    await page.send("Input.dispatchKeyEvent", { type: "keyUp", key: "e", code: "KeyE", modifiers: 2, windowsVirtualKeyCode: 69 });
    const state = await evaluate(page, `(() => {
      const host = document.querySelector('[aria-label="Pointer hit-test document"]');
      return { text: host.value, caret: host.shadowRoot.querySelector('textarea').selectionStart };
    })()`);
    if (state.text !== expectedText || state.caret !== expectedCaret) {
      throw new Error(`Ctrl+E task caret contract failed: ${JSON.stringify(state)}`);
    }
  }
}
