# text

Text-domain primitives: `Position`, `Range`, `Selection`, `EditOp`, and
operations over a `ropey::Rope`.

Layer: foundation (no internal deps). Consumers: `buffer`, `decorate`,
`core`, `layout`, anywhere text is read or modified.
