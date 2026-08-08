import type {
  ContinuityChangeDetail,
  ContinuityEditorElement,
  ContinuityErrorDetail,
  ContinuityFrameDetail,
  ContinuityReadyDetail,
  ContinuityRequestDetail,
  ContinuityShortcutSuppressedDetail,
  RailAction,
  RevisionConflictError,
  ShortcutBindings,
  ShortcutPolicy,
} from "./index.js";

type EditorEvent<Detail> = CustomEvent<Detail>;

export interface ContinuityEditorProps extends Omit<
  ContinuityEditorHostAttributes,
  "children"
> {
  /** Complete controlled Continuity snapshot text. */
  value: string;
  /** Continuity engine revision corresponding exactly to `value`. */
  revision: number;
  readOnly?: boolean;
  spellcheck?: boolean;
  shortcutPolicy?: ShortcutPolicy;
  shortcutBindings?: ShortcutBindings;
  /** Host command-rail actions; assigning replaces the whole host set. */
  railActions?: readonly RailAction[];
  theme?: "light" | "dark";
  tabBehavior?: "indent" | "focus";
  indentGuides?: "on" | "off";
  syntax?: "markdown" | "plain";
  onChange?: (
    detail: ContinuityChangeDetail,
    event: EditorEvent<ContinuityChangeDetail>,
  ) => void;
  onHostReplacement?: (
    detail: ContinuityChangeDetail,
    event: EditorEvent<ContinuityChangeDetail>,
  ) => void;
  onRequest?: (
    detail: ContinuityRequestDetail,
    event: EditorEvent<ContinuityRequestDetail>,
  ) => void;
  onFrame?: (
    detail: ContinuityFrameDetail,
    event: EditorEvent<ContinuityFrameDetail>,
  ) => void;
  onReady?: (
    detail: ContinuityReadyDetail,
    event: EditorEvent<ContinuityReadyDetail>,
  ) => void;
  onDestroy?: (event: CustomEvent<Readonly<{ version: 1 }>>) => void;
  onError?: (
    error: unknown,
    event?: EditorEvent<ContinuityErrorDetail>,
  ) => void;
  onShortcutSuppressed?: (
    detail: ContinuityShortcutSuppressedDetail,
    event: EditorEvent<ContinuityShortcutSuppressedDetail>,
  ) => void;
  onRevisionConflict?: (error: RevisionConflictError) => void;
}

export type ContinuityEditorRef =
  | { current: ContinuityEditorElement | null }
  | ((element: ContinuityEditorElement | null) => void)
  | null;

export interface ContinuityEditorHostAttributes {
  [attribute: string]: unknown;
  id?: string;
  className?: string;
  style?: Readonly<Record<string, string | number | undefined>>;
  title?: string;
  role?: string;
  tabIndex?: number;
  hidden?: boolean;
  "aria-label"?: string;
  ref?: ContinuityEditorRef;
}

/** Controlled React surface over `<continuity-editor>`. */
export const ContinuityEditor: (props: ContinuityEditorProps) => any;
