# Embedded Markdown editor bake-off

## Purpose

The bake-off answers two separate questions: how competing Markdown editors
feel when embedded in the same small host, and what their observable browser
costs and Markdown round trips look like. It is evidence for product and SDK
decisions, not a benchmark designed to make Continuity win.

The runnable application is `apps/editor-bakeoff/`. A web host is the fairest
common denominator because every candidate already targets Chromium. Wrapping
the same pages in Electron would add a shared process shell without improving
the editor comparison; Electron-specific Continuity integration remains
covered by `apps/electron-example/` and the desktop web gates.

Run `npm ci`, then `npm run prepare:continuity` from that directory. The
prepare step builds the npm tarball and installs that exact packed artifact
without changing the lockfile; Vite resolves it from `node_modules` and has no
workspace-source alias. `npm run test:playground` therefore serves as a real
external visual-host acceptance check, including physical multi-cursor input.

## Candidates and models

| Candidate | Editing model | Integration surface |
|---|---|---|
| Continuity | canonical Markdown rope plus display projection | Web Component and WASM |
| Milkdown Crepe | ProseMirror tree serialized to Markdown | framework-neutral DOM |
| MDXEditor | Lexical tree bridged through MDAST | React |
| Wysimark | Slate tree serialized to Markdown | standalone React bundle |
| Tiptap Markdown | ProseMirror tree with beta Markdown conversion | headless DOM toolkit |
| Toast UI Editor | ProseMirror WYSIWYG tree and Markdown conversion | framework-neutral DOM |
| Vditor | Lute-backed editable DOM and Markdown conversion | framework-neutral DOM |
| CodeMirror 6 | canonical Markdown source with syntax decoration | framework-neutral DOM toolkit |

CodeMirror is deliberately a source-editor baseline rather than a WYSIWYG
competitor. It exposes the cost and fidelity floor of keeping Markdown text
canonical in a conventional browser control.

## Host contract

Each adapter implements the smallest shared contract needed for both manual
and automated use: mount initial Markdown, focus the end, insert benchmark
text, scroll to the end, return Markdown, replace Markdown, and destroy the
editor. Product-specific toolbars and documented default features remain
enabled where practical. The contract does not pretend that all editors expose
identical commands or input pipelines.

The playground isolates candidates in iframes. The benchmark uses a new browser
context for every candidate and corpus, blocks external requests, and measures
a production Vite build. This prevents one editor's framework globals, caches,
or retained state from becoming another editor's apparent cost.

## Corpora and observations

Three corpora serve different purposes:

- **Feature document:** headings, inline marks, links, nested and task lists,
  tables, code, raw HTML, comments, references, Unicode, and wrapping.
- **Source-fidelity traps:** alternative marker spellings, entities, escapes,
  reference links, tilde fences, raw HTML, and comments.
- **Large document:** 1,500 task-list lines for startup, editing, serialization,
  scrolling, heap, resource, DOM, and accessibility observations.

The runner records initialization, API edit-to-paint percentiles,
serialization p99, scroll duration, V8 JS heap, recursively counted light and
Shadow DOM nodes, cold transferred resource bytes, source round-trip changes,
and editable accessibility nodes. Exact values are in
`apps/editor-bakeoff/public/results/latest.json`; the adjacent Markdown file is
the readable snapshot.

## Interpretation limits

These numbers are observations from a single Chromium build and machine, not
release gates. Two animation frames impose an approximately 33 ms floor at
60 Hz, so close edit-to-paint results are effectively tied. JS heap excludes
WASM linear memory, native browser allocations, GPU resources, and process
working set. Cold resources charge each standalone page for its framework even
when a real host might already ship that dependency.

Source fidelity normalizes only CRLF to LF. A non-exact result shows that an
editor reserialized the document; changed lines can be benign normalization or
meaningful loss and must be inspected in the playground. Conversely, exact
round trip on the trap corpus does not prove every Markdown construct is
preserved.

Insertion uses each editor's supported programmatic path because their DOM and
selection models differ. Manual typing remains necessary for input method,
selection, keyboard, clipboard, and product-feel judgments. Continuity's native
keypress-to-pixel and durability budgets remain governed by the existing Rust
and browser suites, not this comparison.

## Current compatibility finding

Wysimark 3.0.20 failed to mount under React 19 with a production React error.
The standalone comparison host therefore pins React and React DOM 18.3.1,
which supports all React-based candidates in this set. Keep that pin explicit
until a later Wysimark version is verified with React 19.
