# Image paste / drop / inline render

Phase F5. Pasted clipboard image bytes and dropped image files import into a hash-deduped shared store; the buffer references the file with a plain markdown image link; the renderer paints inline thumbnails.

## Surfaces

- **Shared store** — `%APPDATA%\continuity\images\<hash>.<ext>`. Pure
  filesystem; not part of the SQLite snapshot / edit-log replay. A
  deleted store file is just a broken markdown reference (identical
  to a broken external URL).
- **Reference form** — `![alt](images/<hash>.<ext>)`. The
  `alt|<width>` superset (`![logo|320](images/abc.png)`) carries an
  explicit DIP-width hint to the renderer.
- **Default behaviour** — `markdown.inline_images = true` by default
  (flips spec §9; spec-delta §L#3). Each `![](url)` reference paints
  as a **collapsed affordance** by default — a single-row strip with
  an 18-DIP thumbnail (decoded from the actual image), the
  filename label, and a `▸` chevron. Clicking the strip flips the
  image to **expanded**, where the renderer paints the full bitmap
  scaled to fit pane width (preserving aspect ratio). A second
  click collapses it again. Per-image `|<width>` overrides the
  expanded clamp.
- **Expand state** is keyed `(BufferId, URL)`, defaults to collapsed
  (absent entries collapse, explicit `true` expands), and persists
  across buffer close/reopen and full window restart via the
  per-window pane-tree JSON blob.

## Pipelines

### Drag-drop
1. `WM_DROPFILES` lands in `Window::on_drop_files`
   (`crates/ui/src/window_file.rs`).
2. Paths partition on
   `crate::window_file_image_drop::is_dropped_image_path` (PNG /
   JPG / JPEG / GIF / WEBP / BMP — SVG is intentionally excluded;
   vectors need a different render path).
3. Image paths route through `Window::import_dropped_images`
   (`crates/ui/src/window_file_image_drop.rs`): focus the
   drop-target pane, run each path through
   `crate::image_store::import_path`, insert the markdown reference
   at the caret via `SelectionEdit::InsertText`.
4. Non-image paths keep the legacy tab-open route via
   `FileIoClient::open_files`.

### Clipboard paste
1. `Window::paste_clipboard_impl` (in
   `crates/ui/src/window_clipboard.rs`) probes the image branches
   before the `CF_UNICODETEXT` path.
2. `Window::try_paste_clipboard_image`
   (`crates/ui/src/window_clipboard_image.rs`) probes
   `CF_DIBV5` then `CF_DIB` via
   `continuity_win::clipboard_image::read_dib_bytes`. On hit:
   `decode_dib_to_rgba(blob)` → `encode_rgba_to_png(img)` (via the
   `png` crate, encoder-only, `default-features = false`) →
   `image_store::import_bytes` → `SelectionEdit::InsertText` with
   `![](images/<hash>.png)`. Single undo group.
3. Falls through to `CF_HDROP` (Explorer "Copy" on selected
   files): filter by image extension and route each through
   `image_store::import_bytes` using the bytes already on disk
   (no re-encode; the on-disk format is preserved).
4. If neither image branch matches, falls through to the existing
   text paste.

Supported DIB pixel formats: 24bpp BI_RGB and 32bpp (BI_RGB +
canonical BI_BITFIELDS). Paletted (≤8bpp) and JPEG/PNG-in-DIB
surface a typed `ImagePasteError::UnsupportedFormat`. Bottom-up
DIBs are flipped to top-down inside the decoder.

## Hash + dedup

`crate::image_store::fnv1a_64` (matches the persist-layer checksum
style) produces the 64-bit dedup key. `import_bytes` is idempotent:
a second call with the same bytes returns `was_written: false` and
does NOT touch the file. The filename is the lowercase hex
representation of the hash plus the normalised extension
(`normalise_extension` rejects path-separator characters and falls
back to `"bin"` on malformed input).

## Settings

- `[markdown].inline_images` (default `true`) — whether the
  renderer paints inline thumbnails.
- `[markdown].images_dir` (default `"%APPDATA%\\continuity\\images"`)
  — the shared store path. Consumed via
  `MarkdownConfig::resolve_images_dir() -> Result<PathBuf, Error>`
  which expands the single supported `%APPDATA%` prefix; absolute
  paths pass through verbatim.
- `[ui].image_cache_bytes` (default `67_108_864` / 64 MiB) — upper
  bound on the renderer-side bitmap cache. `0` disables inline
  rendering as a memory-budget alternative to flipping
  `inline_images = false`.

Settings hot-reload: `Window::apply_settings` projects
`image_store_dir` and `inline_images_enabled` on every settings
event (`continuity_config::SettingsWatcher` → control channel →
`apply_settings`). Toggling at runtime takes effect without
restart.

## Render path

1. `crates/render/src/image_layout.rs` —
   `compute_image_layout(attrs, native_w, native_h,
   pane_width_dip) -> ImageLayoutRect`. Native if narrower than
   pane, otherwise scale to pane width preserving aspect ratio.
   `|<width>` override honoured up to the pane-width cap.
2. `crates/render/src/image_cache.rs` — `IWICImagingFactory` +
   `HashMap<PathBuf, CachedImage>` LRU bounded by total decoded
   bytes (configured from `[ui].image_cache_bytes`).
   `invalidate_for_new_device` drops bitmaps after D2D device
   loss. Held by `Renderer` behind a `RefCell` so the draw API
   stays on `&self`.
3. `crates/render/src/image_paint.rs::paint_inline_images` —
   iterates the per-frame placement slice, decodes each via the
   cache, dispatches on `is_expanded`:
   - `is_expanded == true` → lays out via `compute_image_layout`
     and `DrawBitmap`s the full image.
   - `is_expanded == false` (default) → paints the **collapsed
     affordance** (18-DIP thumbnail + filename label + `▸`
     chevron) and records the hit rect into the renderer's
     `last_image_hits` so the UI mouse handler can route clicks.
   Decode failures log to stderr and skip the image; the buffer
   keeps its `![](url)` text fallback.
4. `crates/ui/src/window_image_placements.rs` — per-frame
   builder. Iterates `decorations.inlines`, picks
   `InlineKind::ImageRef`, calls `parse_image_alt`, resolves the
   URL against `[markdown].images_dir`, looks up the
   `(BufferId, url)` expand state, and emits the placement slice
   the painter consumes.
5. `DrawParams::images: Option<&[InlineImagePlacement]>` is the
   plumbing surface; the renderer's image-paint pass runs after
   text and before overlays. After the pass, the UI calls
   `Renderer::image_hits()` on each `WM_LBUTTONDOWN` to test
   clicks against collapsed-affordance rects;
   `Window::try_image_hit_at` flips state via
   `Window::toggle_image_expand`.
6. Persistence: `pane_tree_codec::encode_with_state` /
   `decode_with_state` round-trip the `(BufferId, url) → bool`
   map alongside the existing pane tree + fold state. Default
   (collapsed) entries are stripped at encode time so the wire
   shape stays small. Legacy blobs decode with an empty state
   vec.

**Display-map**: collapsed placements stay single-row; expanded
placements reserve phantom rows so subsequent text flows beneath
the bitmap (see "Row reservation" below). Each image paints at its
*display*-line baseline (the UI builder calls
`FrameDisplay::first_display_line_index_for_source` so folded
source lines drop out and projections account for soft-wrap and
the table-hide / heading-fold providers); the painter additionally
offsets by `margins.left` (gutter) and subtracts `scroll_y` so
images stay glued to the surrounding text.

### Row reservation (γ)

When a placement is expanded, the display map injects
`ceil(image_display_height_dip / line_height_dip)` total display
rows on the image's source line — one natural row (carrying the
`![](url)` source bytes) followed by `N - 1` phantom rows that
carry no source bytes and no segments. Content below an expanded
image flows below the reserved space rather than being overdrawn.

Contract:

- **Provider** —
  `continuity_display_map::compute_image_row_reservations(inputs,
  line_height_dip, pane_width_dip)`. Inputs are plain data
  (`ImageRowReservationInput { source_line, is_expanded,
  native_dimensions, width_hint }`); collapsed images and expanded
  images with unknown native dimensions emit no entry. The
  sibling-provider design mirrors the existing fold /
  heading-fold / table-hide / backslash-escape providers.
- **Native dimensions** come from a per-window probe of the
  renderer's `ImageCache` via
  `Renderer::cached_image_dimensions(path)`. Cold-cache paths
  emit no reservation, so the first paint after expand falls back
  to one display row (matches the pre-γ behaviour, one frame of
  overdraw); the next frame, once decode has populated the cache,
  the reservation is in place.
- **Builder** —
  `DisplayMapBuilder::with_image_reservations(&reservations)`
  threads the slice through to the `lines` post-pass; each
  phantom row is an empty `DisplayLineSpec` whose
  `source_byte_start == source_byte_end == line_end`, with
  `is_wrap_continuation: true` so the renderer skips any leading-
  indent decoration.
- **Phantom semantics** — non-editable (zero source bytes), share
  `source_line` with the image's natural row so caret arrow-down
  from above-image skips them in a single keystroke and mouse
  hit-tests inside phantom-row pixel area resolve to the image's
  source line. Selection through them produces a valid Range
  covering the image's source line plus the next post-image
  source line, with zero-width hits on the phantom rows.
- **Painter** — `image_paint::paint_expanded` is unchanged; it
  draws at `placement.display_line * line_height` with the same
  `max_visible_height_dip` viewport-bottom guard. The guard is no
  longer load-bearing for overdraw (the reserved rows already
  carry no text) but still bounds painting against the status bar
  on scroll.
- **Hit-test** — the top-right chevron rect lives inside the
  image bitmap; it remains clickable regardless of which phantom
  row the user's cursor lands on, because the hit-test routes by
  pixel rect not by display row.

**Hit-test geometry**: `InlineImageHit::rect` is **pane-body-
relative** (`(x_pane, y_pane, w, h)`); the UI mouse handler tests
against `x - body.x` / `y - body.y` directly. Collapsed
placements record the full affordance row. Expanded placements
record **only** the top-right collapse chevron (22 DIP square,
4 DIP inset from the image's top-right corner, painted with a
55 %-opacity black backing so it stays readable over any image
content) — clicks inside the image body do not toggle, so the
user can drag-select / right-click the bitmap without accidentally
collapsing it.

## Persistence impact

None. Shared images are plain files; the SQLite snapshot /
edit-log machinery is untouched. Backup is filesystem-level
(included in the `%APPDATA%\continuity\` tree). Recovery does not
touch image files.

## Tests

- `crates/decorate/src/image_link.rs` — 8 parser unit tests
  (`parse_image_alt`, `is_shared_store_reference`).
- `crates/ui/src/image_store.rs` — 8 unit tests
  (`fnv1a_64`, `import_bytes`, `import_path`,
  `normalise_extension`, `is_supported_image_extension`).
- `crates/ui/src/window_file_image_drop.rs` — 2 unit tests
  on `is_dropped_image_path`.
- `crates/config/tests/settings_defaults_integration.rs` — 4
  tests covering `inline_images`, `images_dir`,
  `image_cache_bytes`, and `resolve_images_dir`.

- `crates/render/src/image_layout.rs` — 6 unit tests
  (clamp / hint / zero / aspect-ratio).
- `crates/render/src/image_cache.rs` — 3 unit tests
  (disabled / capacity / device-invalidate).
- `crates/render/src/image_paint.rs` — 2 unit tests
  (empty-slice + placement default).
- `crates/ui/src/window_image_placements.rs` — 7 unit tests
  (URL resolution branches + toggle + decorations).

**Pixel canary** —
`crates/render/tests/pixel_canary_inline_image.rs`. Generates a
deterministic 64×32 PNG via the `png` crate at test start,
activates the renderer's bitmap cache, and asserts the captured
back-buffer hash matches `inline_image_dark.hash`. Regenerate via
`cargo xtask snapshot-update`.
