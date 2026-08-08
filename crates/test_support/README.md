# test_support

Shared test fixtures: golden markdown buffers, recorded edit sessions,
proptest generators, fake clocks, and the frozen cross-implementation parity
corpus. Pulled in as `dev-dependencies` by other crates; never appears in
production binaries.

Disable default features for the portable parity corpus and percentile helpers
without pulling the native editor graph into focused crate tests. The default
`native-harness` feature retains the full Win32 fixtures and existing API.

`Win32Harness` drives the full durable desktop `Window` composition.
`EditorControlHarness` is the distinct non-Continuity embedding proof: a plain
parent HWND owns the message pump and hosts one or more storage-neutral
`WS_CHILD` controls, including independent destroy/recreate and event-channel
inspection.
