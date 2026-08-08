# continuity-wasm

Thin `wasm-bindgen` adapter over the synchronous storage-neutral engine,
portable host command resolver, Markdown decoration, and display-map crates.
The generated JavaScript is an internal transport; `packages/editor` owns the
stable TypeScript API. `RawEditor::execute_command` accepts only editor-owned
command IDs, including shared word and line selection; host-owned desktop
commands are rejected before mutation.

The browser or JavaScript host owns scheduling, persistence, and presentation.
This crate creates no workers, files, database, or event loop.

Run `cargo xtask wasm-check` for the complete portable compile, native/WASM
parity, optimized npm pack, clean-consumer, and budget gate. The technical
contract and recorded baseline are in `.docs/technical/wasm-sdk.md`.
