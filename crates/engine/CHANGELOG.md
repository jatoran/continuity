# Changelog

## 0.2.18

- Coordinate the browser touch-focus arbitration patch; synchronous engine
  contracts are unchanged.

## 0.2.13

- `SelectionEdit::MarkdownToggleBullet` (`markdown.toggle_bullet`) now shares the
  content-relative line-start bullet planner used by
  `ToggleBulletAtLineStart`, so toggling a bullet keeps the caret on the same
  content character (and converts an ordered `N. ` marker to `- `) instead of
  returning the pre-edit selections and stranding the caret under the inserted
  prefix. The redundant `plan_markdown_toggle_bullet` planner is removed.
- Coordinate the browser command-rail and IME pointer patch: the npm checkmark
  rail action now maps to `markdown.toggle_task` (creates `- [ ] `), and a
  pointer tap taken during an active composition is hit-tested against the live
  textarea line and applied only after `compositionend` reconciles. Engine
  command semantics are otherwise unchanged.

## 0.2.12

- Coordinate the touch pointer-contract patch: the npm package defers touch
  gestures to native scroll/long-press semantics while keeping projected tap
  mapping, and preserves the scroll viewport across host hide/show cycles.
  Synchronous engine contracts are unchanged.

## 0.2.11

- Coordinate the rail-sizing patch: the npm package sizes command-rail and
  copy-control chrome in host-independent px with touch-first defaults and
  host-tunable custom properties. Synchronous engine contracts are unchanged.

## 0.2.10

- Coordinate the mobile chrome patch: the npm package adds a configurable
  bottom quick-action command rail and always-visible touch code-copy
  controls with a hardened clipboard fallback. Synchronous engine contracts
  are unchanged.

## 0.2.9

- Coordinate the line-scoped browser IME composition preview patch: the npm
  package keeps the Markdown projection visible while a word composes and
  previews only the composing line. Synchronous engine contracts are
  unchanged.

## 0.2.8

- `reconcile_text_if_revision` now applies the minimal differing splice
  (shared prefix and suffix kept) instead of a whole-document replace, so
  selections and host line mirrors outside the changed range keep their
  positions during full-document host reconciliation.

## 0.2.7

- Coordinate the browser double-click word and triple-click line-selection
  patch; synchronous engine contracts are unchanged.

## 0.2.6

- Coordinate the mobile IME composition-hardening SDK patch; synchronous
  engine contracts are unchanged.

## 0.2.5

- Coordinate the browser selection-foreground and navigation-caret SDK patch;
  synchronous engine contracts are unchanged.

## 0.2.4

- Coordinate the projection-owned browser selection SDK patch release;
  synchronous engine contracts are unchanged.

## 0.2.3

- Coordinate the projected visual-caret SDK patch release; synchronous engine
  contracts are unchanged.

## 0.2.2

- Preserve content-relative selections when toggling task markers; a fresh-line
  task caret lands after the complete `- [ ] ` prefix.

## 0.2.1

- Coordinate the patch release containing shared list-wrap behavior.

## 0.2.0

- Coordinated embedded SDK release with expanded host and conformance contracts.

## 0.1.3

- Coordinated SDK patch release; synchronous engine contracts are unchanged.

## 0.1.2

- Coordinated SDK patch release; synchronous engine contracts are unchanged.

## 0.1.1

- Coordinated SDK patch release; synchronous engine contracts are unchanged.

## 0.1.0

- Initial synchronous, storage-neutral engine preview.
- Public changes return revisioned change batches; persistence remains host-owned.
