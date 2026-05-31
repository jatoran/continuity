# keymap

TOML keymap loader, layered (default ⊕ user), with hot-reload and a
conflict checker exposed as a command.

Layer: glue. Depends on `command` and `input`.

The bundled default map includes find-bar-scoped chords for search mode
toggles and replace actions. Those bindings use `when = "find_bar.visible"`
so the same chords remain available to normal editor commands outside the
overlay.
