# continuity-engine C ABI

Versioned Windows x86-64 preview ABI for the synchronous, storage-neutral
Continuity engine. The creating thread owns each handle. Rust owns returned
buffers until the matching free function is called. Change callbacks run only
after mutation finishes; every API call made reentrantly from a callback is
rejected.

The library is headless: it does not create windows, files, a database, or a
message loop. Build with the workspace `release-sdk` profile so panic
containment remains enabled.
