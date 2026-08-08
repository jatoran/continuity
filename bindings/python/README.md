# continuity-editor for Python

Headless, synchronous Python access to the Continuity editor engine for
automation, language tooling, and applications that provide their own storage
and presentation. It creates no files, database, threads, or GUI objects.

This package is not a Qt, Tk, or GTK widget. A Python desktop host that needs
the Continuity visual editor embeds the separately distributed
`@continuity-editor/editor` Web Component in its chosen web-view toolkit.

Each `Editor` is confined to its construction thread. Change callbacks run
after engine mutation finishes; reentrant calls to the same object are
rejected. Call `close()` or use the editor as a context manager for explicit
teardown.
