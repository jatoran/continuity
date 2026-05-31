# Continuity

Continuity is a native Windows markdown notes editor for writing notes. Period. Code notes included, but this is not trying to be a code editor.

I wanted something with the speed, ephemerality, and safety I like in Sublime Text, but aimed at notes: better Windows virtual desktop behavior, WYSIWYG markdown editing, and no "did I save?" anxiety. Every keystroke is written to a local SQLite database; saving a file is export, not durability.

The markdown surface is my own flavor of WYSIWYG. The source stays plain markdown, but Continuity projects it through a custom live renderer so headings, lists, checkboxes, links, tables, inline code, and images can feel integrated instead of bolted on.

It is built in Rust as a small Win32 app with DirectWrite/Direct2D rendering, rope-backed text, explicit worker threads, bounded caches, and SQLite WAL persistence. WYSIWYG adds performance pressure, so I benchmark and optimize the projection/rendering path to keep large notes responsive.

## Status

Continuity is early, Windows-only software. It is usable enough to package, but it is still changing quickly.

## Features

- Native Win32 editor for Windows 10 and Windows 11.
- Plain text and markdown source stays canonical.
- Live markdown rendering for headings, emphasis, lists, checkboxes, links, code blocks, tables, and inline images.
- Integrated markdown table editing with a more spreadsheet-like feel, so pipe tables can be worked with without constantly fighting raw markdown alignment.
- Note-friendly markdown niceties such as inline code handling and copyable code snippets.
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
