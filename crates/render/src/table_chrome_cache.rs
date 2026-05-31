//! Per-table chrome command-list cache (P14.1).
//!
//! Each markdown pipe-table records its chrome (cell fills, header /
//! alignment-row backgrounds, cell borders, per-column-aligned cell
//! text) into an `ID2D1CommandList`. While
//! `(table identity, layout content hash, theme revision, dpi scale,
//! font state, line height, base font size)` stays equal across paints,
//! the per-table chrome collapses to a single `DrawImage` replay.
//! Editing inside a table changes that table's `layout_content_hash`
//! and invalidates only its entry; theme / DPI / font shifts invalidate
//! every entry.
//!
//! Sibling of [`crate::chrome_command_list`]: that one retains the
//! static editor shell (status bar, ruler, sidebar, outline), this one
//! retains the table interior that P14 deliberately left out because
//! cell content can change.
//!
//! Thread ownership: UI thread (held in a `RefCell` on `Renderer`,
//! which is single-thread).

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use continuity_decorate::TableAlignment;
use windows::core::Interface;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COMPOSITE_MODE_SOURCE_OVER;
use windows::Win32::Graphics::Direct2D::{
    ID2D1CommandList, ID2D1DeviceContext, ID2D1Image, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat};

use crate::table_layout::TableLayout;
use crate::table_paint::{paint_table_visual_line, TableLinePlacement, TableVisualBrushes};
use crate::Error;

/// Default bound on retained per-table command lists. Each entry holds
/// one `ID2D1CommandList`; eviction is oldest-`last_used_frame`-first.
pub(crate) const DEFAULT_CAPACITY: usize = 64;

/// Stable per-document identity for one pipe-table. `block_start` is
/// the table's source byte offset; `document` is the buffer's u128 id
/// split into two u64s so the type stays `Hash` / `Eq` / `Copy`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TableId {
    document_lo: u64,
    document_hi: u64,
    block_start: u64,
}

impl TableId {
    /// Build an id from a document u128 and the table's source byte
    /// range start.
    #[must_use]
    pub(crate) fn new(document: u128, block_start: usize) -> Self {
        Self {
            document_lo: document as u64,
            document_hi: (document >> 64) as u64,
            block_start: block_start as u64,
        }
    }
}

/// Invalidation key compared on every paint. Equal value → replay;
/// any difference → rebuild.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TableChromeKey {
    block_end: u64,
    layout_content_hash: u64,
    theme_revision: u64,
    font_state: u64,
    dpi_scale_bits: u32,
    line_height_bits: u32,
    base_font_size_bits: u32,
}

impl TableChromeKey {
    /// Build the key for `layout` under the supplied per-frame inputs.
    #[must_use]
    pub(crate) fn for_layout(
        layout: &TableLayout,
        theme_revision: u64,
        font_state: u64,
        dpi_scale: f32,
        line_height_dip: f32,
        base_font_size_dip: f32,
    ) -> Self {
        Self {
            block_end: layout.block_range.end as u64,
            layout_content_hash: compute_layout_content_hash(layout),
            theme_revision,
            font_state,
            dpi_scale_bits: dpi_scale.to_bits(),
            line_height_bits: line_height_dip.to_bits(),
            base_font_size_bits: base_font_size_dip.to_bits(),
        }
    }
}

/// Whether the most recent paint of a single table rebuilt or replayed
/// its command list. Aggregated into [`TableChromePathStats`] across
/// every table painted in a frame.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TableChromePathMode {
    /// At least one table's command list was rebuilt this frame.
    Fresh,
    /// Every visible table's command list was replayed unchanged.
    #[default]
    Replay,
}

impl TableChromePathMode {
    /// Lowercase token emitted in `event:table_chrome_path`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Replay => "replay",
        }
    }
}

/// Aggregate record/replay stats for one paint's table chrome pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TableChromePathStats {
    /// Number of tables painted this frame.
    pub tables_painted: u32,
    /// Number of tables whose command list was rebuilt.
    pub fresh_count: u32,
    /// Number of tables whose command list was replayed unchanged.
    pub replay_count: u32,
    /// Microseconds spent recording command lists this frame.
    pub record_us: u64,
    /// Microseconds spent replaying command lists this frame.
    pub replay_us: u64,
}

impl TableChromePathStats {
    /// Total microseconds attributable to the table chrome path.
    #[must_use]
    pub fn elapsed_us(self) -> u64 {
        self.record_us.saturating_add(self.replay_us)
    }

    /// `Fresh` if any record happened this paint, `Replay` otherwise.
    /// Trace consumers use it to spot first-paint vs steady-state.
    #[must_use]
    pub fn dominant_mode(self) -> TableChromePathMode {
        if self.fresh_count > 0 {
            TableChromePathMode::Fresh
        } else {
            TableChromePathMode::Replay
        }
    }

    /// Format the TSV details column for `event:table_chrome_path`.
    #[must_use]
    pub fn trace_detail(self) -> String {
        format!(
            "mode={} tables={} fresh={} replay={} record_us={} replay_us={} elapsed_us={}",
            self.dominant_mode().as_str(),
            self.tables_painted,
            self.fresh_count,
            self.replay_count,
            self.record_us,
            self.replay_us,
            self.elapsed_us(),
        )
    }
}

struct CacheEntry {
    key: TableChromeKey,
    command_list: ID2D1CommandList,
    last_used_frame: u64,
}

/// Per-table cache.
///
/// Owning thread: UI thread (held inside a `RefCell` on `Renderer`).
pub(crate) struct TableChromeCache {
    entries: HashMap<TableId, CacheEntry>,
    capacity: usize,
    frame: u64,
}

impl Default for TableChromeCache {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            capacity: DEFAULT_CAPACITY,
            frame: 0,
        }
    }
}

impl TableChromeCache {
    /// Drop every entry. Called on device recreation and resize so no
    /// device-resident `ID2D1CommandList` crosses targets.
    pub(crate) fn invalidate(&mut self) {
        self.entries.clear();
    }

    /// Advance the frame counter so LRU eviction can tell visible from
    /// stale entries. Call once per paint, before any `prepare`.
    pub(crate) fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// Ensure the cache entry for `(table_id, key)` exists. Returns
    /// `(mode, record_elapsed_us)` — `Fresh` when this call rebuilt
    /// the list (with the rebuild time), `Replay` when an existing
    /// matching entry was reused (with `0`).
    pub(crate) fn prepare<F>(
        &mut self,
        table_id: TableId,
        key: TableChromeKey,
        device_context: &ID2D1DeviceContext,
        record: F,
    ) -> Result<(TableChromePathMode, u64), Error>
    where
        F: FnOnce(&ID2D1DeviceContext) -> Result<(), Error>,
    {
        if let Some(entry) = self.entries.get_mut(&table_id) {
            if entry.key == key {
                entry.last_used_frame = self.frame;
                return Ok((TableChromePathMode::Replay, 0));
            }
        }
        let started = Instant::now();
        let previous_target = unsafe { device_context.GetTarget()? };
        let command_list = unsafe { device_context.CreateCommandList()? };
        unsafe {
            device_context.SetTarget(&command_list);
            device_context.BeginDraw();
        }
        let record_result = record(device_context);
        let end_result = unsafe { device_context.EndDraw(None, None).map_err(Error::from) };
        unsafe {
            device_context.SetTarget(&previous_target);
        }
        record_result?;
        end_result?;
        unsafe {
            command_list.Close()?;
        }
        let elapsed = elapsed_us(started);
        self.evict_if_needed();
        self.entries.insert(
            table_id,
            CacheEntry {
                key,
                command_list,
                last_used_frame: self.frame,
            },
        );
        Ok((TableChromePathMode::Fresh, elapsed))
    }

    /// Replay `table_id`'s command list at the current device-context
    /// transform. Returns the microseconds spent inside `DrawImage`.
    /// Returns `0` when no entry exists for `table_id` (a caller
    /// invariant violation — `prepare` should always precede `replay`
    /// in the same frame).
    pub(crate) fn replay(
        &self,
        table_id: TableId,
        device_context: &ID2D1DeviceContext,
    ) -> Result<u64, Error> {
        let Some(entry) = self.entries.get(&table_id) else {
            return Ok(0);
        };
        let started = Instant::now();
        let image: ID2D1Image = entry.command_list.cast()?;
        unsafe {
            device_context.DrawImage(
                &image,
                None,
                None,
                D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
        }
        Ok(elapsed_us(started))
    }

    fn evict_if_needed(&mut self) {
        if self.entries.len() < self.capacity {
            return;
        }
        let Some(oldest) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used_frame)
            .map(|(id, _)| *id)
        else {
            return;
        };
        self.entries.remove(&oldest);
    }
}

/// Hash every input the painter consults to draw a table's chrome.
/// Cell text is the largest contributor; widths / alignments / row
/// flags follow.
pub(crate) fn compute_layout_content_hash(layout: &TableLayout) -> u64 {
    let mut hasher = DefaultHasher::new();
    layout.first_source_line.hash(&mut hasher);
    layout.last_source_line.hash(&mut hasher);
    layout.alignment_row_source_line.hash(&mut hasher);
    // Phase F — per-row display heights: a wrap / `<br>` change that
    // alters a row's height must rebuild the chrome (it changes the
    // cell-rect heights and the cumulative row offsets).
    layout.row_display_rows.hash(&mut hasher);
    hash_f32(&mut hasher, layout.total_width_dip);
    layout.col_widths_dip.len().hash(&mut hasher);
    for width in &layout.col_widths_dip {
        hash_f32(&mut hasher, *width);
    }
    layout.col_alignments.len().hash(&mut hasher);
    for alignment in &layout.col_alignments {
        alignment_tag(*alignment).hash(&mut hasher);
    }
    layout.cells.len().hash(&mut hasher);
    for cell in &layout.cells {
        cell.source_line.hash(&mut hasher);
        cell.col.hash(&mut hasher);
        cell.display_text.hash(&mut hasher);
        cell.is_header.hash(&mut hasher);
        cell.is_alignment_row.hash(&mut hasher);
        cell.is_formula.hash(&mut hasher);
        cell.inline_runs.len().hash(&mut hasher);
        for (range, style) in &cell.inline_runs {
            range.start.hash(&mut hasher);
            range.end.hash(&mut hasher);
            style.hash(&mut hasher);
        }
        // Phase F — the wrapped line texts, so a wrap-point shift that
        // keeps the same row height still rebuilds.
        cell.lines.len().hash(&mut hasher);
        for line in &cell.lines {
            line.text.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn alignment_tag(alignment: TableAlignment) -> u8 {
    match alignment {
        TableAlignment::Left => 0,
        TableAlignment::Center => 1,
        TableAlignment::Right => 2,
    }
}

fn hash_f32(hasher: &mut DefaultHasher, value: f32) {
    value.to_bits().hash(hasher);
}

/// Record every row of `layout` into the device context's currently-
/// bound target. Caller binds an `ID2D1CommandList` as the target and
/// brackets the call with `BeginDraw` / `EndDraw` (see
/// [`TableChromeCache::prepare`]).
///
/// Coordinates are table-local: row `r` sits at
/// `y = layout.display_row_offset_within_table(r) * line_height_dip`
/// (the cumulative height of every earlier row, so a `<br>` / wrapped
/// row pushes the rows below it down by its full height), column `c`
/// sits at `x = layout.cell_x_dip(c)`. The recorded list assumes the
/// replay transform aligns table-local `(0, 0)` with the table's
/// top-left in screen DIPs.
pub(crate) fn record_table_chrome(
    device_context: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    layout: &TableLayout,
    line_height_dip: f32,
    brushes: &TableVisualBrushes<'_>,
) {
    let first = layout.first_source_line;
    let last = layout.last_source_line;
    let single = std::slice::from_ref(layout);
    for row in first..=last {
        let translate = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: 0.0,
            M32: layout.display_row_offset_within_table(row) as f32 * line_height_dip,
        };
        unsafe {
            device_context.SetTransform(&translate);
        }
        paint_table_visual_line(
            device_context,
            dwrite,
            format,
            single,
            TableLinePlacement {
                source_line: row,
                row_display_rows: layout.row_height(row),
                line_height_dip,
                x_origin_dip: 0.0,
            },
            brushes,
        );
    }
    unsafe {
        device_context.SetTransform(&identity_matrix());
    }
}

fn identity_matrix() -> Matrix3x2 {
    Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: 0.0,
        M32: 0.0,
    }
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
