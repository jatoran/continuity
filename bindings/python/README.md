# continuity-editor for Python

`continuity-editor` gives Python programs the Continuity editor engine without
a UI: document text, carets, inserting and deleting text, and undo history, for
scripts, tooling, and applications that handle their own saving and display.
It creates no files, database, threads, or GUI objects.

This package is not a Qt, Tk, or GTK widget. A Python desktop host that needs
the Continuity visual editor embeds the separately distributed
`@continuity-editor/editor` Web Component in its chosen web-view toolkit.

Each `Editor` is confined to its construction thread. Change callbacks run
after engine mutation finishes; reentrant calls to the same object are
rejected. Call `close()` or use the editor as a context manager for explicit
teardown.
