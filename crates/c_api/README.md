# continuity-engine C ABI

A C interface to the Continuity editor engine, built as a Windows x86-64 DLL
with a header. The ABI is versioned and still a preview. The engine never saves
anything itself; the host decides where text goes. The creating thread owns
each handle. Rust owns returned
buffers until the matching free function is called. Change callbacks run only
after mutation finishes; every API call made reentrantly from a callback is
rejected.

The library is headless: it does not create windows, files, a database, or a
message loop. Build with the workspace `release-sdk` profile so panic
containment remains enabled.
