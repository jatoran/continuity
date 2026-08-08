import type { ContinuityChangeDetail, Snapshot } from "./index.js";

export type CommitQueue<Value> = Readonly<{
  enqueue(value: Value): void;
  enqueueChange(detail: ContinuityChangeDetail): void;
  flush(): Promise<void>;
  dispose(options?: Readonly<{ flush?: boolean }>): Promise<void>;
  readonly isPending: boolean;
}>;

export function createCommitQueue<Value = Snapshot>(options: Readonly<{
  save(value: Value): void | Promise<void>;
  debounceMs?: number;
  maxDelayMs?: number;
  onError?: (error: unknown, value: Value) => void;
}>): CommitQueue<Value>;
