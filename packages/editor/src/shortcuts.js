const DEFAULT_SHORTCUTS = new Map([
  ["mod+z", shortcut("editor.undo")],
  ["mod+shift+z", shortcut("editor.redo")],
  ["mod+y", shortcut("editor.redo")],
  ["mod+enter", shortcut("editor.insert_newline_below")],
  ["mod+shift+enter", shortcut("editor.insert_newline_above")],
  ["alt+enter", shortcut("editor.insert_newline_smart")],
  ["mod+backspace", shortcut("editor.delete_word_backward")],
  ["mod+delete", shortcut("editor.delete_word_forward")],
  ["mod+shift+backspace", shortcut("editor.delete_to_line_start")],
  ["mod+shift+delete", shortcut("editor.delete_to_line_end")],
  ["mod+shift+m", shortcut("editor.delete_to_bracket")],
  ["mod+shift+y", shortcut("editor.duplicate_line")],
  ["mod+alt+y", shortcut("editor.duplicate_selection")],
  ["mod+shift+arrowup", shortcut("editor.move_line_up_block")],
  ["mod+shift+arrowdown", shortcut("editor.move_line_down_block")],
  ["mod+j", shortcut("editor.join_lines", false)],
  ["mod+shift+j", shortcut("editor.join_selected_lines", false)],
  ["mod+r", shortcut("editor.toggle_bullet_at_line_start", false)],
  ["mod+shift+r", shortcut("editor.toggle_bullet_indent_continuation", false)],
  ["mod+alt+s", shortcut("editor.sort_lines_asc")],
  ["mod+alt+r", shortcut("editor.reverse_lines")],
  ["mod+alt+u", shortcut("editor.unique_lines")],
  ["mod+alt+t", shortcut("editor.trim_trailing_whitespace")],
  ["mod+alt+q", shortcut("editor.reflow_paragraph")],
  ["mod+alt+shift+t", shortcut("editor.transpose_chars")],
  ["alt+t", shortcut("editor.transpose_words")],
  ["mod+u", shortcut("editor.change_case_upper", false)],
  ["mod+shift+u", shortcut("editor.change_case_lower")],
  ["mod+alt+c", shortcut("editor.change_case_toggle")],
  ["mod+b", shortcut("markdown.toggle_bold")],
  ["mod+i", shortcut("markdown.toggle_italic")],
  ["mod+shift+s", shortcut("markdown.toggle_strikethrough", false)],
  ["mod+`", shortcut("markdown.toggle_inline_code")],
  ["alt+1", shortcut("markdown.set_heading_1")],
  ["alt+2", shortcut("markdown.set_heading_2")],
  ["alt+3", shortcut("markdown.set_heading_3")],
  ["alt+4", shortcut("markdown.set_heading_4")],
  ["alt+5", shortcut("markdown.set_heading_5")],
  ["alt+6", shortcut("markdown.set_heading_6")],
  ["alt+`", shortcut("markdown.remove_heading")],
  ["alt+0", shortcut("markdown.cycle_heading_up")],
  ["alt+9", shortcut("markdown.cycle_heading_down")],
  ["mod+shift+.", shortcut("markdown.demote_section")],
  ["mod+shift+,", shortcut("markdown.promote_section")],
  ["mod+shift+pageup", shortcut("markdown.move_section_up")],
  ["mod+shift+pagedown", shortcut("markdown.move_section_down")],
  ["mod+shift+8", shortcut("markdown.toggle_bullet")],
  ["mod+shift+7", shortcut("markdown.toggle_numbered")],
  ["mod+shift+x", shortcut("markdown.toggle_checkbox")],
  ["mod+e", shortcut("markdown.toggle_task", false)],
  ["mod+shift+q", shortcut("markdown.wrap_in_blockquote")],
  ["mod+shift+c", shortcut("markdown.insert_code_fence", false)],
  ["mod+k", shortcut("markdown.insert_link", false)],
]);

const SHORTCUT_POLICIES = new Set(["browser-safe", "editor-first", "none"]);
const PUBLIC_SHORTCUTS = Object.freeze(Array.from(DEFAULT_SHORTCUTS, ([chord, binding]) => Object.freeze({
  chord,
  command: binding.command,
  isBrowserSafe: binding.isBrowserSafe,
})));

/** Return the built-in command bindings and their browser-safety classification. */
export function listShortcutBindings() {
  return PUBLIC_SHORTCUTS;
}

/** Validate a public shortcut policy name. */
export function normalizeShortcutPolicy(value) {
  const policy = value || "browser-safe";
  if (!SHORTCUT_POLICIES.has(policy)) {
    throw new RangeError(`shortcutPolicy must be browser-safe, editor-first, or none; received ${policy}`);
  }
  return policy;
}

/** Normalize a host shortcut overlay into canonical chord keys. */
export function normalizeShortcutBindings(bindings) {
  const entries = bindings instanceof Map ? bindings.entries() : Object.entries(bindings ?? {});
  const normalized = new Map();
  for (const [chord, command] of entries) {
    if (command !== null && (typeof command !== "string" || command.length === 0)) {
      throw new TypeError(`shortcut binding ${chord} must name a command or be null`);
    }
    normalized.set(normalizeChord(chord), command);
  }
  return normalized;
}

/** Map a shortcut command onto its direct engine operation, if it has one. */
export function directShortcutOperation(command) {
  return {
    "editor.indent": "indent",
    "editor.outdent": "outdent",
    "editor.redo": "redo",
    "editor.undo": "undo",
  }[command] ?? null;
}

/** Resolve a keyboard-like event under the selected host policy. */
export function resolveEditorShortcut(event, policy, overrides = new Map()) {
  const decision = resolveEditorShortcutDecision(event, policy, overrides);
  return decision.kind === "command" ? decision.command : null;
}

/** Explain whether a keyboard-like event executes, releases, or suppresses a command. */
export function resolveEditorShortcutDecision(event, policy, overrides = new Map()) {
  if (event.getModifierState?.("AltGraph")) {
    return { kind: "unmatched" };
  }
  const chords = eventChords(event);
  for (const chord of chords) {
    if (overrides.has(chord)) {
      const command = overrides.get(chord);
      return command === null ? { kind: "released" } : { kind: "command", command };
    }
  }
  const chord = chords.at(-1);
  const binding = DEFAULT_SHORTCUTS.get(chord);
  if (!binding) {
    return { kind: "unmatched" };
  }
  if (policy === "none" || (policy === "browser-safe" && !binding.isBrowserSafe)) {
    return { kind: "suppressed", chord, command: binding.command, policy };
  }
  return { kind: "command", command: binding.command };
}

function shortcut(command, isBrowserSafe = true) {
  return { command, isBrowserSafe };
}

function eventChords(event) {
  const key = keyFromEvent(event);
  const modifiers = [];
  if (event.altKey) modifiers.push("alt");
  if (event.shiftKey) modifiers.push("shift");
  const exact = [];
  if (event.ctrlKey) exact.push("ctrl");
  if (event.metaKey) exact.push("meta");
  exact.push(...modifiers, key);
  const chords = [exact.join("+")];
  if (event.ctrlKey !== event.metaKey) {
    chords.push(["mod", ...modifiers, key].join("+"));
  }
  return chords;
}

function keyFromEvent(event) {
  const codeKeys = {
    Backquote: "`",
    Comma: ",",
    Period: ".",
  };
  if (/^Digit\d$/u.test(event.code ?? "")) {
    return event.code.slice(-1);
  }
  return codeKeys[event.code] ?? normalizeKey(event.key);
}

function normalizeChord(chord) {
  if (typeof chord !== "string" || chord.trim().length === 0) {
    throw new TypeError("shortcut chord must be a non-empty string");
  }
  const aliases = { command: "meta", commandorcontrol: "mod", control: "ctrl", cmd: "meta", primary: "mod" };
  const parts = chord.split("+").map((part) => aliases[part.trim().toLowerCase()] ?? part.trim().toLowerCase());
  const key = normalizeKey(parts.pop());
  const modifiers = ["mod", "ctrl", "meta", "alt", "shift"].filter((part) => parts.includes(part));
  if (modifiers.length !== parts.length) {
    throw new RangeError(`invalid shortcut chord ${chord}`);
  }
  return [...modifiers, key].join("+");
}

function normalizeKey(key) {
  const aliases = { down: "arrowdown", left: "arrowleft", right: "arrowright", space: " ", up: "arrowup" };
  return aliases[key.toLowerCase()] ?? key.toLowerCase();
}
