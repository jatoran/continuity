# win

Thin Win32 wrappers: HWND newtypes, COM init, `IVirtualDesktopManager`,
`PerMonitorV2` DPI awareness, monitor enumeration, and clipboard format I/O.
`WindowClass::register_unique_with_proc` is also used by external hosts and the
embeddable child control without importing desktop-window lifecycle.
`clipboard.rs` is the single text/HTML open-read-write boundary;
`clipboard_image.rs` owns DIB and dropped-file extraction. UI orchestration
does not call Win32 clipboard APIs directly.

Single-instance activation filters visible process windows through
`IsWindowOnCurrentVirtualDesktop`; failure reports no eligible target so the
app can create locally instead of switching desktops.
The same documented virtual-desktop query gates activation of known-vault
windows; query failure is fail-local and never switches desktops.

Layer: foundation. Depends only on `windows-sys`. Lives below `ui`,
`layout`, `render`, and `input`.
