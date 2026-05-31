# theme

TOML theme files declaring a color table and font defaults; hot-reloaded.
Three modes: dark, light, follow-system.

Layer: glue. No dependencies on other internal crates.

Required-key set lives in `src/keys.rs::REQUIRED_KEYS`; every bundled
TOML (`assets/*.toml`) plus the neutral fallback in `src/assets.rs`
must declare each key or `Theme::validate_required` rejects the load.
Phase F4 added `markdown.formula.value` / `markdown.formula.error` for
the inline-table formula swap-in (foreground color of the rendered
computed value and of the `#DIV/0!` / `#ERR` sentinels respectively).
