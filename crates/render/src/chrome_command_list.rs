//! Retained static-chrome command list.
//!
//! The renderer records chrome primitives whose inputs are stable across
//! warm body paints into an `ID2D1CommandList`, then replays that list
//! with `DrawImage` during the chrome stage. The renderer is UI-thread
//! owned, so the cache is not shared across threads.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use windows::core::Interface;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_COLOR_F, D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1CommandList, ID2D1DeviceContext, ID2D1Image, ID2D1RenderTarget, ID2D1SolidColorBrush,
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
};

use crate::chrome::{paint_ruler_columns, ContentMargins};
use crate::params::{DrawParams, Rgba};
use crate::{ChromePathMode, ChromePathStats, Error};

/// Stable key for the retained static-chrome command list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ChromeCommandListKey {
    theme_revision: u64,
    dpi_scale_bits: u32,
    pane_geometry_hash: u64,
    sidebar_visibility: bool,
    minimap_visible: bool,
    outline_visible: bool,
}

/// Geometry captured at paint entry for command-list recording.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChromeRecordingGeometry {
    /// Focused-pane body transform installed by the body paint pass.
    pub body_translate: Matrix3x2,
    /// Body clip rect used to keep ruler columns out of right-edge chrome.
    pub body_clip: D2D_RECT_F,
    /// Resolved body margins after gutter/minimap/sidebar accounting.
    pub margins: ContentMargins,
    /// Focused viewport width in DIPs.
    pub viewport_width_dip: f32,
    /// Focused viewport height in DIPs.
    pub viewport_height_dip: f32,
    /// Body text column width in DIPs.
    pub editor_width_dip: f32,
    /// Logical body line height in DIPs.
    pub line_height_dip: f32,
    /// Measured width of one space glyph in DIPs.
    pub column_advance_dip: f32,
}

/// Record/replay cache for static chrome.
#[derive(Default)]
pub(crate) struct ChromeCommandList {
    key: Option<ChromeCommandListKey>,
    command_list: Option<ID2D1CommandList>,
    current_mode: ChromePathMode,
    current_record_elapsed_us: u64,
}

impl ChromeCommandListKey {
    /// Build the invalidation key for the current paint.
    #[must_use]
    pub(crate) fn from_draw_params(
        params: &DrawParams<'_>,
        geometry: &ChromeRecordingGeometry,
    ) -> Self {
        Self {
            theme_revision: params.theme_revision,
            dpi_scale_bits: params.dpi_scale.to_bits(),
            pane_geometry_hash: compute_pane_geometry_hash(params, geometry),
            sidebar_visibility: params.view_options.search_minimap_active
                || params.view_options.show_outline_sidebar,
            minimap_visible: params.view_options.minimap,
            outline_visible: params.view_options.show_outline_sidebar && params.outline.is_some(),
        }
    }
}

impl ChromeCommandList {
    /// Drop the retained D2D command list. Called on resize and device
    /// recreation so device-resident resources never cross targets.
    pub(crate) fn invalidate(&mut self) {
        self.key = None;
        self.command_list = None;
        self.current_mode = ChromePathMode::Replay;
        self.current_record_elapsed_us = 0;
    }

    /// Ensure the command list exists for `key`, recording with
    /// `record` when the key changed.
    pub(crate) fn prepare<F>(
        &mut self,
        device_context: &ID2D1DeviceContext,
        key: ChromeCommandListKey,
        record: F,
    ) -> Result<(), Error>
    where
        F: FnOnce(&ID2D1DeviceContext) -> Result<(), Error>,
    {
        if self.key == Some(key) && self.command_list.is_some() {
            self.current_mode = ChromePathMode::Replay;
            self.current_record_elapsed_us = 0;
            return Ok(());
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

        self.key = Some(key);
        self.command_list = Some(command_list);
        self.current_mode = ChromePathMode::Fresh;
        self.current_record_elapsed_us = elapsed_us(started);
        Ok(())
    }

    /// Replay the cached command list and return the combined
    /// record-plus-replay timing for this paint.
    pub(crate) fn replay(
        &self,
        device_context: &ID2D1DeviceContext,
    ) -> Result<ChromePathStats, Error> {
        let Some(command_list) = self.command_list.as_ref() else {
            return Ok(ChromePathStats::new(ChromePathMode::Fresh, 0));
        };
        let started = Instant::now();
        let image: ID2D1Image = command_list.cast()?;
        unsafe {
            device_context.DrawImage(
                &image,
                None,
                None,
                D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
        }
        Ok(ChromePathStats::new(
            self.current_mode,
            self.current_record_elapsed_us
                .saturating_add(elapsed_us(started)),
        ))
    }
}

/// Record all static chrome primitives into the current command-list
/// target.
pub(crate) fn record_static_chrome(
    device_context: &ID2D1DeviceContext,
    params: &DrawParams<'_>,
    geometry: ChromeRecordingGeometry,
) -> Result<(), Error> {
    let render_target: ID2D1RenderTarget = device_context.cast()?;
    let make_brush = |rgba: Rgba| -> Result<ID2D1SolidColorBrush, Error> {
        Ok(unsafe { render_target.CreateSolidColorBrush(&D2D1_COLOR_F::from(rgba), None)? })
    };
    let identity = identity_matrix();
    record_ruler_columns(
        device_context,
        params,
        geometry,
        &make_brush(params.colors.indent_guide)?,
    );
    unsafe {
        device_context.SetTransform(&identity);
    }

    if params.view_options.show_status_bar {
        if let Some(data) = params.status_bar {
            let top = (params.client_height_dip - crate::STATUS_BAR_HEIGHT_DIP).max(0.0);
            let background_brush = make_brush(data.colors.bg)?;
            crate::status_bar::paint_status_bar_background(
                device_context,
                geometry.viewport_width_dip,
                top,
                &background_brush,
            );
        }
    }

    if params.view_options.show_outline_sidebar {
        if let Some(data) = params.outline {
            let background_brush = make_brush(data.colors.bg)?;
            let separator_brush = make_brush(data.colors.separator)?;
            let pane_rect = (
                params.body_origin.0,
                params.body_origin.1,
                geometry.viewport_width_dip,
                geometry.viewport_height_dip,
            );
            let _ = crate::outline_paint::paint_outline_shell(
                device_context,
                data,
                pane_rect,
                &background_brush,
                &separator_brush,
            );
        }
    }

    Ok(())
}

fn record_ruler_columns(
    device_context: &ID2D1DeviceContext,
    params: &DrawParams<'_>,
    geometry: ChromeRecordingGeometry,
    brush: &ID2D1SolidColorBrush,
) {
    if params.view_options.ruler_columns.is_empty() {
        return;
    }
    unsafe {
        device_context.SetTransform(&geometry.body_translate);
        device_context.PushAxisAlignedClip(&geometry.body_clip, D2D1_ANTIALIAS_MODE_ALIASED);
    }
    let body_content_translate = Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: geometry.body_translate.M31 + geometry.margins.left,
        M32: geometry.body_translate.M32,
    };
    unsafe {
        device_context.SetTransform(&body_content_translate);
    }
    let zero_left = ContentMargins {
        left: 0.0,
        right: geometry.margins.right,
    };
    paint_ruler_columns(
        device_context,
        params.view_options.ruler_columns,
        geometry.column_advance_dip,
        zero_left,
        geometry.viewport_height_dip,
        brush,
    );
    unsafe {
        device_context.PopAxisAlignedClip();
    }
}

fn compute_pane_geometry_hash(params: &DrawParams<'_>, geometry: &ChromeRecordingGeometry) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_f32(&mut hasher, params.view.viewport_width_dip);
    hash_f32(&mut hasher, params.view.viewport_height_dip);
    hash_f32(&mut hasher, params.client_height_dip);
    hash_f32(&mut hasher, params.base_font_size_dip);
    hash_f32(&mut hasher, geometry.line_height_dip);
    hash_f32(&mut hasher, geometry.column_advance_dip);
    hash_f32(&mut hasher, geometry.editor_width_dip);
    hash_f32(&mut hasher, geometry.margins.left);
    hash_f32(&mut hasher, geometry.margins.right);
    hash_f32(&mut hasher, params.body_origin.0);
    hash_f32(&mut hasher, params.body_origin.1);
    params.view_options.show_status_bar.hash(&mut hasher);
    params.status_bar.is_some().hash(&mut hasher);
    params.view_options.show_outline_sidebar.hash(&mut hasher);
    hash_f32(&mut hasher, params.view_options.outline_sidebar_width_dip);
    params.view_options.show_tab_strip.hash(&mut hasher);
    params.view_options.show_pane_borders.hash(&mut hasher);
    params.view_options.ruler_columns.hash(&mut hasher);
    if let Some(chrome) = params.pane_chrome {
        hash_f32(&mut hasher, chrome.strip_height);
        chrome.panes.len().hash(&mut hasher);
        for pane in &chrome.panes {
            hash_rect(&mut hasher, pane.outer);
            pane.focused.hash(&mut hasher);
            pane.active_index.hash(&mut hasher);
            pane.tabs.len().hash(&mut hasher);
        }
    }
    params.pane_bodies.len().hash(&mut hasher);
    for body in params.pane_bodies {
        hash_rect(&mut hasher, body.rect);
    }
    hasher.finish()
}

fn hash_rect(hasher: &mut DefaultHasher, rect: (f32, f32, f32, f32)) {
    hash_f32(hasher, rect.0);
    hash_f32(hasher, rect.1);
    hash_f32(hasher, rect.2);
    hash_f32(hasher, rect.3);
}

fn hash_f32(hasher: &mut DefaultHasher, value: f32) {
    value.to_bits().hash(hasher);
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
