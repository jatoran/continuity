# Continuity

Continuity is a native Windows markdown notes editor.

I wanted a notes app that treats writing like the important part: plain text, fast rendering, no project scaffolding, no cloud account, no plugin runtime, and no "did I save?" anxiety. Every keystroke is written to a local SQLite database; saving a file is export, not durability.

It is built as a small Win32 Rust app with DirectWrite/Direct2D rendering and a live markdown projection that keeps the source text canonical.

## Status

Continuity is early, Windows-only software. It is usable enough to package, but it is still changing quickly.

## Features

- Native Win32 editor for Windows 10 and Windows 11.
- Plain text and markdown source stays canonical.
- Live markdown rendering for headings, emphasis, lists, checkboxes, links, code blocks, tables, and inline images.
- Integrated markdown table editing, so pipe tables can be worked with without constantly fighting raw markdown alignment.
- Every keystroke is durable to a local SQLite WAL database.
- Saving is export. The database is the truth.
- Multi-pane, multi-tab, multi-window session restore.
- Portable mode that keeps settings, themes, keymap, notes, and backups beside the executable.
- Installed mode with Start Menu shortcut, optional desktop shortcut, uninstall support, and Windows Default Apps registration for markdown/text files.
- Configurable themes, keymap, settings, fonts, wrapping, and view behavior.
- Fast large-buffer projection and soft-wrap work aimed at keeping writing responsive.

## Performance notes

- Text input: `WM_CHAR` p99 around 2-4 ms in recent release traces.
- Edit application: p99 around 4 ms for normal typing/edit paths.
- Large-buffer row counts: roughly 10k-line soft-wrapped buffers cold-walk in about 50-55 ms in recent local traces.
- Rendering: viewport-first projection keeps large notes visible and editable while the full document index catches up.

## Downloads

GitHub Releases are the normal way to get builds.

- `continuity-<version>-setup.msi`: recommended for normal use. Installs under Program Files and supports in-place upgrades.
- `continuity-<version>-portable.zip`: no-install build. Extract the folder and run `continuity.exe`; app data stays in that folder.
- `continuity-<version>-standalone.zip`: just `continuity.exe`; settings, themes, and notes use your normal Windows AppData.
- `SHA256SUMS.txt`: hashes for release assets.

Unsigned builds may trigger Windows SmartScreen until release signing is in place.

## Build

Requirements:

- Windows 10 or Windows 11
- Rust stable with the `x86_64-pc-windows-msvc` target
- Visual Studio Build Tools or another MSVC toolchain

Build the app:

```powershell
cargo build --release -p continuity-app
```

Build local release artifacts:

```powershell
cargo xtask release --skip-sign
```

The MSI path also requires WiX v7:

```powershell
dotnet tool install --global wix --add-source https://api.nuget.org/v3/index.json
wix eula accept wix7
wix extension add --global WixToolset.UI.wixext/7.0.0
cargo xtask installer
```

## License

See [LICENSE](LICENSE).
