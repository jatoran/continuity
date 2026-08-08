/**
 * Command-rail action catalog, host registration, and arrangement persistence.
 *
 * The rail hosts two kinds of action. Built-in entries name a storage-neutral
 * engine command and are resolved by the shared Rust command resolver. Host
 * entries are registered by the embedding application through
 * `registerRailAction()`; they either name an engine command, run a host
 * callback, or (naming neither) raise a `continuity-request` of kind
 * `railAction` so a declarative host can answer from an event listener.
 *
 * Host ids are namespaced (`vendor:action`) so a future built-in can never
 * collide with an arrangement a host already persisted, and a persisted id
 * whose action is not registered yet is retained rather than pruned: hosts
 * register asynchronously, and dropping the id would silently reset the user's
 * arrangement on every reload.
 */

/** Every built-in quick action the rail can host, in default order. */
export const COMMAND_RAIL_CATALOG = [
  { id: "undo", command: "editor.undo", glyph: "↺", label: "Undo" },
  { id: "redo", command: "editor.redo", glyph: "↻", label: "Redo" },
  { id: "caret-up", operation: "caretUp", glyph: "↑", label: "Move caret up one visible line" },
  { id: "caret-down", operation: "caretDown", glyph: "↓", label: "Move caret down one visible line" },
  { id: "move-line-up", command: "editor.move_line_up_block", glyph: "⤒", label: "Move lines up" },
  { id: "move-line-down", command: "editor.move_line_down_block", glyph: "⤓", label: "Move lines down" },
  { id: "indent", command: "editor.indent", glyph: "⇥", label: "Indent" },
  { id: "outdent", command: "editor.outdent", glyph: "⇤", label: "Outdent" },
  { id: "bullet", command: "markdown.toggle_bullet", glyph: "•", label: "Toggle bullet" },
  { id: "checkbox", command: "markdown.toggle_task", glyph: "☑", label: "Toggle task" },
  { id: "numbered", command: "markdown.toggle_numbered", glyph: "1.", label: "Toggle numbered list" },
  { id: "bold", command: "markdown.toggle_bold", glyph: "B", label: "Toggle bold" },
  { id: "italic", command: "markdown.toggle_italic", glyph: "I", label: "Toggle italic" },
  { id: "strikethrough", command: "markdown.toggle_strikethrough", glyph: "S", label: "Toggle strikethrough" },
  { id: "inline-code", command: "markdown.toggle_inline_code", glyph: "</>", label: "Toggle inline code" },
  { id: "heading", command: "markdown.cycle_heading_up", glyph: "H", label: "Cycle heading" },
  { id: "quote", command: "markdown.wrap_in_blockquote", glyph: "❝", label: "Wrap in blockquote" },
  { id: "link", command: "markdown.insert_link", glyph: "\u{1F517}", label: "Insert link" },
];

const DEFAULT_DISABLED = new Set(["numbered", "strikethrough", "quote", "link"]);
const BUILT_IN_IDS = new Set(COMMAND_RAIL_CATALOG.map((entry) => entry.id));
const HOST_ID_PATTERN = /^[a-z0-9][a-z0-9-]*:[a-z0-9][a-z0-9-]*$/iu;
const STORAGE_KEY = "continuity-editor.command-rail";

/** The built-in actions a host can enable, disable, or reorder by id. */
export function listBuiltInRailActions() {
  return Object.freeze(COMMAND_RAIL_CATALOG.map((entry) => Object.freeze({
    id: entry.id,
    label: entry.label,
    // Caret motion is measured against wrapped browser layout, so those two
    // actions have no storage-neutral engine command a host could call.
    command: entry.command ?? null,
    isEnabledByDefault: !DEFAULT_DISABLED.has(entry.id),
  })));
}

/** Storage key for one editor, optionally scoped by the host. */
export function railStorageKey(scope) {
  return scope ? `${STORAGE_KEY}:${scope}` : STORAGE_KEY;
}

/**
 * Per-element action registry. Built-in entries always come first; host
 * entries follow in registration order until a stored arrangement moves them.
 */
export function createRailRegistry() {
  const hostActions = new Map();
  const listeners = new Set();
  const notify = () => {
    for (const listener of listeners) listener();
  };
  return {
    entries() {
      return [...COMMAND_RAIL_CATALOG.map(builtInEntry), ...hostActions.values()];
    },
    list() {
      return Object.freeze([...hostActions.values()].map((entry) => entry.descriptor));
    },
    register(descriptor) {
      const entry = normalizeRailAction(descriptor);
      if (hostActions.has(entry.id)) {
        throw new RangeError(`rail action ${entry.id} is already registered`);
      }
      hostActions.set(entry.id, entry);
      notify();
      return () => {
        if (hostActions.delete(entry.id)) notify();
      };
    },
    replaceAll(descriptors) {
      if (!Array.isArray(descriptors)) {
        throw new TypeError("railActions must be an array of rail action descriptors");
      }
      const replacement = new Map();
      for (const descriptor of descriptors) {
        const entry = normalizeRailAction(descriptor);
        if (replacement.has(entry.id)) {
          throw new RangeError(`rail action ${entry.id} is listed twice`);
        }
        replacement.set(entry.id, entry);
      }
      hostActions.clear();
      for (const [id, entry] of replacement) hostActions.set(id, entry);
      notify();
    },
    onChange(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

function builtInEntry(entry) {
  return {
    id: entry.id,
    label: entry.label,
    glyph: entry.glyph,
    icon: null,
    command: entry.command ?? null,
    operation: entry.operation ?? null,
    run: null,
    isEnabled: null,
    isBuiltIn: true,
    isEnabledByDefault: !DEFAULT_DISABLED.has(entry.id),
  };
}

/** Validate one host action descriptor into a rail entry. */
export function normalizeRailAction(descriptor) {
  if (!descriptor || typeof descriptor !== "object") {
    throw new TypeError("a rail action must be an object");
  }
  const { id, label, glyph, icon, command, run, isEnabled, isEnabledByDefault } = descriptor;
  if (typeof id !== "string" || !HOST_ID_PATTERN.test(id)) {
    throw new RangeError(
      `rail action id ${String(id)} must be namespaced as vendor:action (letters, digits, dashes)`,
    );
  }
  if (BUILT_IN_IDS.has(id)) {
    throw new RangeError(`rail action id ${id} is reserved by a built-in action`);
  }
  if (typeof label !== "string" || label.trim().length === 0) {
    throw new TypeError(`rail action ${id} must have a non-empty label`);
  }
  if (command !== undefined && (typeof command !== "string" || command.length === 0)) {
    throw new TypeError(`rail action ${id} command must be a non-empty command id`);
  }
  if (run !== undefined && typeof run !== "function") {
    throw new TypeError(`rail action ${id} run must be a function`);
  }
  if (icon !== undefined && typeof icon !== "function") {
    throw new TypeError(`rail action ${id} icon must be a function returning an element`);
  }
  if (isEnabled !== undefined && typeof isEnabled !== "function") {
    throw new TypeError(`rail action ${id} isEnabled must be a function`);
  }
  if (glyph !== undefined && typeof glyph !== "string") {
    throw new TypeError(`rail action ${id} glyph must be a string`);
  }
  return {
    id,
    label,
    // A glyph is optional: an action with neither glyph nor icon still needs a
    // visible mark, and the label's first character is the honest default.
    glyph: glyph || (icon ? "" : [...label][0].toUpperCase()),
    icon: icon ?? null,
    command: command ?? null,
    operation: null,
    run: run ?? null,
    isEnabled: isEnabled ?? null,
    isBuiltIn: false,
    isEnabledByDefault: isEnabledByDefault !== false,
    descriptor: Object.freeze({ ...descriptor }),
  };
}

/**
 * Resolve the persisted arrangement against the registry. Ids with no
 * registered action are retained (`unknownOrder` / `unknownDisabled`) so a
 * late registration returns to its saved slot instead of the rail's tail.
 */
export function resolveRailConfiguration(registry, storageKey, storage = defaultStorage()) {
  let stored = null;
  try {
    stored = JSON.parse(storage?.getItem(storageKey) ?? "null");
  } catch {
    stored = null;
  }
  const storedOrder = Array.isArray(stored?.order) ? stored.order.filter(isId) : [];
  const storedDisabled = Array.isArray(stored?.disabled) ? stored.disabled.filter(isId) : null;
  return applyStoredArrangement(registry, storageKey, storedOrder, storedDisabled);
}

function applyStoredArrangement(registry, storageKey, storedOrder, storedDisabled) {
  const entries = registry.entries();
  const byId = new Map(entries.map((entry) => [entry.id, entry]));
  const order = storedOrder.map((id) => byId.get(id)).filter(Boolean);
  for (const entry of entries) {
    if (!order.includes(entry)) order.push(entry);
  }
  const seen = new Set(storedOrder);
  const disabled = new Set();
  const unknownDisabled = new Set();
  for (const id of storedDisabled ?? []) {
    (byId.has(id) ? disabled : unknownDisabled).add(id);
  }
  for (const entry of entries) {
    // An action the arrangement has never seen falls back to its own default;
    // one it has seen keeps whatever the user last chose.
    if (!entry.isEnabledByDefault && (storedDisabled === null || !seen.has(entry.id))) {
      disabled.add(entry.id);
    }
  }
  const unknownOrder = storedOrder
    .map((id, index) => ({ id, index }))
    .filter(({ id }) => !byId.has(id));
  return { order, disabled, unknownOrder, unknownDisabled, storageKey };
}

/** Re-resolve a live arrangement after the registry gained or lost actions. */
export function reconcileRailConfiguration(state, registry) {
  const stored = serializeRailConfiguration(state);
  return applyStoredArrangement(registry, state.storageKey, stored.order, stored.disabled);
}

/** Flatten a live arrangement back to its persisted shape. */
export function serializeRailConfiguration(state) {
  const order = state.order.map((entry) => entry.id);
  for (const { id, index } of state.unknownOrder) {
    order.splice(Math.min(index, order.length), 0, id);
  }
  return { order, disabled: [...state.disabled, ...state.unknownDisabled] };
}

/** Persist the rail arrangement; storage rejection is non-fatal. */
export function saveRailConfiguration(state, storage = defaultStorage()) {
  try {
    storage?.setItem(state.storageKey, JSON.stringify(serializeRailConfiguration(state)));
  } catch {
    // Blocked storage (private mode, embedder policy) keeps a session-local rail.
  }
}

/** Restore the default arrangement in place, host actions included. */
export function resetRailConfiguration(state, registry) {
  const defaults = applyStoredArrangement(registry, state.storageKey, [], null);
  state.order.splice(0, state.order.length, ...defaults.order);
  state.disabled.clear();
  for (const id of defaults.disabled) state.disabled.add(id);
  state.unknownOrder.splice(0, state.unknownOrder.length);
  state.unknownDisabled.clear();
}

function isId(value) {
  return typeof value === "string" && value.length > 0;
}

function defaultStorage() {
  try {
    return globalThis.localStorage;
  } catch {
    return null;
  }
}
