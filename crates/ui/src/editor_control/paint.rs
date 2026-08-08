//! Shared DWrite/D2D paint path for the child editor surface.

use continuity_layout::FontStateId;
use continuity_render::{
    DrawParams, EditorColors, FrameDisplay, MarkdownColors, Renderer, Rgba, ViewOptionsDraw,
    DEFAULT_HEADING_SCALE,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};

use super::state::EditorControlState;
use crate::display_prewarm_cache::PrewarmQuery;
use crate::Error;

const LINE_HEIGHT_MULTIPLIER: f32 = 1.45;

impl EditorControlState {
    pub(super) fn ensure_render_resources(&mut self, hwnd: HWND) -> Result<(), Error> {
        if self.surface.render.renderer.is_none() {
            self.surface.render.renderer = Some(Renderer::for_hwnd(
                hwnd,
                self.client_width.max(1),
                self.client_height.max(1),
            )?);
        }
        if self.surface.render.text_format.is_none() {
            self.rebuild_text_format()?;
        }
        Ok(())
    }

    pub(super) fn resize_render_resources(&mut self, hwnd: HWND) -> Result<(), Error> {
        self.refresh_geometry(hwnd)?;
        if let Some(renderer) = self.surface.render.renderer.as_mut() {
            renderer.resize_for_hwnd(hwnd, self.client_width, self.client_height)?;
        } else {
            self.ensure_render_resources(hwnd)?;
        }
        Ok(())
    }

    pub(super) fn rebind_dpi(&mut self, hwnd: HWND) -> Result<(), Error> {
        self.refresh_geometry(hwnd)?;
        self.rebuild_text_format()?;
        self.surface.render.cache.clear();
        self.resize_render_resources(hwnd)
    }

    fn rebuild_text_format(&mut self) -> Result<(), Error> {
        let scale = self.scale();
        self.surface.render.font_state = FontStateId::from_parts(
            &self.options.font_family,
            self.options.font_size_dip,
            &self.options.font_locale,
            scale,
        );
        self.surface.render.text_format = Some(self.surface.render.dwrite.text_format(
            &self.options.font_family,
            self.options.font_size_dip,
            &self.options.font_locale,
        )?);
        Ok(())
    }

    pub(super) fn paint(&mut self, hwnd: HWND) -> Result<(), Error> {
        let mut paint = PAINTSTRUCT::default();
        unsafe {
            let _ = BeginPaint(hwnd, &mut paint);
        }
        let result = self.draw_frame(hwnd);
        unsafe {
            let _ = EndPaint(hwnd, &paint);
        }
        result
    }

    fn draw_frame(&mut self, hwnd: HWND) -> Result<(), Error> {
        self.ensure_render_resources(hwnd)?;
        let snapshot = self.runtime.snapshot(self.buffer_id)?;
        let revision = snapshot.rope.revision();
        if self.decoration_revision != Some(revision) {
            self.decorations = continuity_decorate::Decorations::compute(
                &snapshot.rope.rope().to_string(),
                revision.get(),
            )
            .map(std::sync::Arc::new);
            self.decoration_revision = Some(revision);
        }
        let rope = snapshot.rope.rope();
        let caret_bytes = snapshot
            .selections
            .iter()
            .filter_map(|selection| position_to_byte(rope, selection.head))
            .collect::<Vec<_>>();
        let font_size = self.options.font_size_dip * self.surface.view.font_size_scale;
        let char_width = font_size * 0.55;
        let wrap_width = if self.surface.view.soft_wrap {
            self.surface.view.viewport_width_dip.max(1.0).round() as u32
        } else {
            0
        };
        let projection_query = PrewarmQuery::new(
            self.buffer_id,
            revision.get(),
            self.decorations.as_ref().map(|_| revision.get()),
            &caret_bytes,
            &[],
            wrap_width,
            self.surface.render.font_state,
        );
        let frame_display = self
            .surface
            .projection
            .last_painted_frame_display
            .as_ref()
            .filter(|(query, _)| query == &projection_query)
            .map_or_else(
                || {
                    FrameDisplay::build(
                        rope,
                        revision.get(),
                        self.decorations.as_deref(),
                        &caret_bytes,
                        wrap_width,
                        char_width,
                    )
                },
                |(_, frame)| frame.clone(),
            );
        let format = self
            .surface
            .render
            .text_format
            .as_ref()
            .expect("invariant: text format initialized before child paint");
        let params = DrawParams {
            document: self.buffer_id.as_uuid().as_u128(),
            format,
            font_state: self.surface.render.font_state,
            theme_revision: 1,
            dpi_scale: self.scale(),
            scroll_velocity_dip_per_s: 0.0,
            scroll_target_pane_id: 0,
            scroll_focused_pane_id: 0,
            scroll_hover_routed: false,
            line_height: font_size * LINE_HEIGHT_MULTIPLIER,
            base_font_size_dip: font_size,
            heading_scale: DEFAULT_HEADING_SCALE,
            view: &self.surface.view,
            colors: control_colors(),
            markdown_colors: control_markdown_colors(),
            view_options: ViewOptionsDraw {
                caret_visible: self.surface.focus.has_keyboard_focus,
                tab_width: 4,
                render_highlight_bg: true,
                render_divider: true,
                ..Default::default()
            },
            decorations: self.decorations.as_deref(),
            inline_color_spans: self
                .decorations
                .as_deref()
                .map_or(&[], |decorations| decorations.inline_color_spans.as_slice()),
            table_overrides: &[],
            table_layouts: &[],
            overlay: None,
            overlay_motion: None,
            chord_hud: None,
            chord_hud_motion: None,
            jump_glow: None,
            edit_pulse: None,
            body_origin: (0.0, 0.0),
            pane_chrome: None,
            spell_spans: &[],
            pane_bodies: &[],
            frame_display: &frame_display,
            line_hover: None,
            client_width_dip: self.surface.view.viewport_width_dip,
            client_height_dip: self.surface.view.viewport_height_dip,
            status_bar: None,
            file_tree: None,
            breadcrumb: None,
            outline: None,
            search_minimap: None,
            images: None,
            time_machine_hud: None,
            loading_overlay: None,
            loading_overlay_motion: None,
            code_copy_button: None,
        };
        self.surface
            .render
            .renderer
            .as_ref()
            .expect("invariant: renderer initialized before child paint")
            .draw_buffer(
                rope,
                &snapshot.selections,
                &mut self.surface.render.cache,
                &params,
            )?;
        self.surface.projection.last_painted_decorations = self.decorations.clone();
        self.surface
            .projection
            .last_painted_decoration_parse_revision =
            self.decorations.as_ref().map(|_| revision.get());
        self.surface.projection.last_painted_frame_display =
            Some((projection_query, frame_display));
        Ok(())
    }
}

fn position_to_byte(rope: &ropey::Rope, position: continuity_text::Position) -> Option<usize> {
    let line = position.line as usize;
    if line >= rope.len_lines() {
        return None;
    }
    let line_start = rope.line_to_byte(line);
    let byte = line_start.checked_add(position.byte_in_line as usize)?;
    (byte <= rope.len_bytes()).then_some(byte)
}

fn control_colors() -> EditorColors {
    EditorColors {
        bg: rgba(0.075, 0.08, 0.09, 1.0),
        fg: rgba(0.88, 0.89, 0.92, 1.0),
        caret: rgba(0.98, 0.62, 0.24, 1.0),
        secondary_caret: rgba(0.45, 0.68, 0.98, 1.0),
        selection: rgba(0.24, 0.43, 0.78, 0.48),
        selection_inactive: rgba(0.24, 0.35, 0.55, 0.34),
        line_highlight: rgba(0.2, 0.22, 0.26, 0.24),
        caret_line_highlight: rgba(0.2, 0.22, 0.26, 0.18),
        ..Default::default()
    }
}

fn control_markdown_colors() -> MarkdownColors {
    let accent = rgba(0.48, 0.72, 1.0, 1.0);
    MarkdownColors {
        heading: [accent; 6],
        bold: rgba(0.96, 0.79, 0.42, 1.0),
        italic: rgba(0.70, 0.82, 0.98, 1.0),
        code_fg: rgba(0.88, 0.65, 0.84, 1.0),
        link: accent,
        url: accent,
        ..Default::default()
    }
}

const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Rgba {
    Rgba { r, g, b, a }
}
