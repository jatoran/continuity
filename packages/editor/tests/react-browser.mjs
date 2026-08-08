import { createElement } from "react";

import { ContinuityEditor } from "/node_modules/@continuity-editor/editor/react.js";

/** Exercise the packed React adapter against a real custom element. */
export async function runReactBrowserContract(check) {
  const mount = document.createElement("section");
  document.body.append(mount);
  const root = globalThis.ReactDOM.createRoot(mount);
  let editorElement;
  let changeCount = 0;

  const ready = deferred();
  render({ text: "# React note", revision: 5 }, {
    onReady: ready.resolve,
  });
  const readyDetail = await ready.promise;
  check(readyDetail.snapshot.text === "# React note", "React seeds controlled text before readiness");
  check(readyDetail.snapshot.revision === 5, "React seeds controlled revision before readiness");
  check(editorElement instanceof HTMLElement, "React forwards the editor element ref");
  check(editorElement.shadowRoot.querySelector("textarea").spellcheck === false,
    "React forwards spellcheck policy to the semantic input");

  const replacement = deferred();
  render({ text: "# Server replacement", revision: 5 }, {
    onChange: (detail) => {
      changeCount += 1;
      replacement.resolve(detail);
    },
  });
  const replacementDetail = await replacement.promise;
  check(replacementDetail.source === "hostReplacement", "React forwards host replacement events");
  check(replacementDetail.snapshot.revision === 6, "React host replacement advances revision");

  render(replacementDetail.snapshot, {
    onChange: () => {
      changeCount += 1;
    },
  });
  await animationFrames(2);
  check(changeCount === 1, "matching React snapshot does not echo a replacement");

  const conflict = deferred();
  render({ text: "stale server text", revision: 5 }, {
    onRevisionConflict: conflict.resolve,
  });
  const error = await conflict.promise;
  check(error.expectedRevision === 5, "React conflict reports expected revision");
  check(error.actualRevision === 6, "React conflict preserves newer editor revision");

  root.unmount();
  mount.remove();
  return 9;

  function render(snapshot, callbacks = {}) {
    root.render(createElement(ContinuityEditor, {
      "aria-label": "React contract editor",
      ref: (element) => {
        editorElement = element;
      },
      style: { display: "block", width: "480px", height: "240px" },
      spellcheck: false,
      value: snapshot.text,
      revision: snapshot.revision,
      ...callbacks,
    }));
  }
}

function deferred() {
  let resolve;
  const promise = new Promise((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

function animationFrames(count) {
  return new Promise((resolve) => {
    const next = () => count-- <= 0 ? resolve() : requestAnimationFrame(next);
    next();
  });
}
