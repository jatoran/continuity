import { useRef, useState } from "react";

import { initialize, type ContinuityEditorElement } from "@continuity-editor/editor";
import { ContinuityEditor } from "@continuity-editor/editor/react";

void initialize();

export function PackedReactConsumer() {
  const editorRef = useRef<ContinuityEditorElement>(null);
  const [snapshot, setSnapshot] = useState({ text: "# Typed note", revision: 0 });
  return (
    <ContinuityEditor
      aria-label="Typed React contract"
      ref={editorRef}
      style={{ display: "block", height: "100%", width: "100%" }}
      spellcheck={false}
      value={snapshot.text}
      revision={snapshot.revision}
      onChange={(detail) => setSnapshot(detail.snapshot)}
      onRevisionConflict={(error) => {
        console.warn(error.expectedRevision, error.actualRevision);
      }}
    />
  );
}
