# text

The basic text types of the Continuity editor engine: positions, ranges,
selections, and edit operations (`Position`, `Range`, `Selection`, `EditOp`),
plus operations over a `ropey::Rope`.

Layer: foundation (no internal deps). Consumers: `buffer`, `decorate`,
`core`, `layout`, anywhere text is read or modified.
