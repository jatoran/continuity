export { ContinuityInitError, Editor, RevisionConflictError, initialize } from "./src/engine.js";
export { ContinuityEditorElement } from "./src/component.js";
export { ContinuityRendererElement } from "./src/static_renderer.js";
export { defineContinuityEditor } from "./src/definition.js";
export { listShortcutBindings } from "./src/shortcuts.js";
export { listBuiltInRailActions } from "./src/command_rail_registry.js";

import { defineContinuityEditor } from "./src/definition.js";

defineContinuityEditor();
//# sourceMappingURL=index.js.map
