import type {
  Change,
  ContinuityChangeDetail,
  HistoryExportOptions,
  HistoryState,
  ContinuityEditorElement,
  ContinuityErrorDetail,
  ContinuityFrameDetail,
  ContinuityReadyDetail,
  ContinuityRequestDetail,
  RailAction,
  Range,
  ScrollState,
  Selection,
  ContinuityShortcutSuppressedDetail,
  ShortcutBindings,
  ShortcutPolicy,
} from "./index.js";

type EditorEvent<Detail> = CustomEvent<Detail>;

export type ContinuityEditorCallbacks = Readonly<{
  onChange?: (detail: ContinuityChangeDetail, event: EditorEvent<ContinuityChangeDetail>) => void;
  onHostReplacement?: (
    detail: ContinuityChangeDetail,
    event: EditorEvent<ContinuityChangeDetail>,
  ) => void;
  onRequest?: (detail: ContinuityRequestDetail, event: EditorEvent<ContinuityRequestDetail>) => void;
  onFrame?: (detail: ContinuityFrameDetail, event: EditorEvent<ContinuityFrameDetail>) => void;
  onReady?: (detail: ContinuityReadyDetail, event: EditorEvent<ContinuityReadyDetail>) => void;
  onDestroy?: (event: CustomEvent<Readonly<{ version: 1 }>>) => void;
  onError?: (error: unknown, event: EditorEvent<ContinuityErrorDetail>) => void;
  onShortcutSuppressed?: (
    detail: ContinuityShortcutSuppressedDetail,
    event: EditorEvent<ContinuityShortcutSuppressedDetail>,
  ) => void;
}>;

export type ContinuityEditorConfiguration = Readonly<{
  readOnly?: boolean;
  spellcheck?: boolean;
  shortcutPolicy?: ShortcutPolicy;
  shortcutBindings?: ShortcutBindings;
  railActions?: readonly RailAction[];
  theme?: "light" | "dark";
  tabBehavior?: "indent" | "focus";
  syntax?: "markdown" | "plain";
  indentGuides?: "on" | "off";
}>;

export type ContinuityEditorControllerOptions = ContinuityEditorConfiguration & Readonly<{
  value: string;
  revision: number;
  callbacks?: ContinuityEditorCallbacks;
}>;

/** Framework-neutral owner of one controlled Continuity element attachment. */
export class ContinuityEditorController {
  constructor(element: ContinuityEditorElement, options: ContinuityEditorControllerOptions);
  readonly element: ContinuityEditorElement;
  setCallbacks(callbacks?: ContinuityEditorCallbacks): void;
  configure(configuration?: ContinuityEditorConfiguration): void;
  synchronize(value: string, revision: number): Promise<Change | null>;
  replaceCurrent(value: string): Promise<Change | null>;
  setSelections(selections: readonly Selection[], options?: Readonly<{ reveal?: boolean }>): void;
  revealRange(range: Range, options?: Readonly<{ align?: "nearest" | "center" }>): void;
  setDecorations(id: string, ranges: readonly Range[]): number;
  clearDecorations(id?: string): void;
  exportHistory(options?: HistoryExportOptions): HistoryState;
  importHistory(history: HistoryState | string): void;
  getScrollState(): ScrollState;
  restoreScrollState(state: ScrollState): void;
  linearMemoryBytes(): number;
  dispose(): void;
}

export function attachContinuityEditor(
  element: ContinuityEditorElement,
  options: ContinuityEditorControllerOptions,
): ContinuityEditorController;
