import json
import sys
import threading

from continuity_editor import Editor, __version__


def snapshot(editor):
    value = editor.snapshot()
    return {
        "text": value.text,
        "revision": value.revision,
        "carets": [list(caret) for caret in value.carets],
    }


def main():
    fixture = json.loads(open(sys.argv[1], encoding="utf-8").read())
    multi = fixture["multiCursor"]
    callbacks = []
    editor = Editor(multi["initialText"])
    editor.set_change_callback(callbacks.append)
    editor.set_carets(
        [
            (selection["head"]["line"], selection["head"]["byteInLine"])
            for selection in multi["selections"]
        ]
    )
    editor.insert_text(multi["insertText"], 1000)
    multi_result = snapshot(editor)
    assert multi_result["text"] == multi["expectedText"]
    assert multi_result["revision"] == multi["expectedRevision"]
    assert multi_result["carets"] == multi["expectedCarets"]
    assert [list(delta) for delta in editor.deltas_since(0)] == [
        [delta["at"], delta["removedBytes"], delta["insertedBytes"]]
        for delta in multi["expectedDeltas"]
    ]
    assert callbacks == [multi["expectedRevision"]]
    wrong_thread_errors = []

    def wrong_thread_snapshot():
        try:
            editor.snapshot()
        except RuntimeError as error:
            wrong_thread_errors.append(str(error))

    thread = threading.Thread(target=wrong_thread_snapshot)
    thread.start()
    thread.join()
    assert wrong_thread_errors and "non-owner thread" in wrong_thread_errors[0]
    editor.close()
    try:
        editor.snapshot()
        raise AssertionError("closed editor accepted snapshot")
    except RuntimeError:
        pass

    deletion = fixture["deleteBackward"]
    with Editor(deletion["initialText"]) as editor:
        head = deletion["selection"]["head"]
        editor.set_carets([(head["line"], head["byteInLine"])])
        editor.delete_backward(2000)
        result = snapshot(editor)
        assert result["text"] == deletion["expectedText"]
        assert result["carets"] == [deletion["expectedCaret"]]

    undo = fixture["undo"]
    with Editor() as editor:
        for offset, text in enumerate(undo["typing"]):
            editor.insert_text(text, 3000 + offset * 100)
        assert editor.snapshot().text == undo["expectedText"]
        editor.undo(3400)
        assert editor.snapshot().text == undo["expectedAfterUndo"]
        editor.redo(3500)
        assert editor.snapshot().text == undo["expectedAfterRedo"]

    branch = fixture["undoBranch"]
    with Editor() as editor:
        editor.insert_text(branch["inputs"][0], 4000)
        editor.insert_text(branch["inputs"][1], 4001)
        editor.undo(4002)
        editor.insert_text(branch["inputs"][2], 4003)
        assert editor.snapshot().text == branch["expectedReplacement"]
        editor.undo(4004)
        editor.redo_alternate(4005)
        assert editor.snapshot().text == branch["expectedAlternate"]

    print(
        "CONTINUITY_PYTHON_PARITY "
        + json.dumps(
            {"version": __version__, "multiCursor": multi_result}, sort_keys=True
        )
    )


if __name__ == "__main__":
    main()
