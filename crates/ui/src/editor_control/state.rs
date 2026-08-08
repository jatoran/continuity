//! UI-thread-owned state behind one child HWND.

use continuity_buffer::{BufferId, Revision};
use continuity_decorate::Decorations;
use continuity_host::{EditorIntent, HostEvent, HostRuntime, Invalidation, Viewport};
use continuity_layout::{DWriteFactory, FontStateId};
use continuity_win::{ComGuard, WindowClass};
use std::sync::Arc;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Shell::DragAcceptFiles;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClientRect, IsWindow, HMENU, WINDOW_EX_STYLE, WINDOW_STYLE, WS_CHILD,
    WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_TABSTOP, WS_VISIBLE,
};

use super::{wndproc, ControlBounds, ControlEventSink, ControlOptions, ControlRuntime};
use crate::editor_surface::EditorSurface;
use crate::Error;

/// Mutable state owned exclusively by the child HWND's UI thread.
pub(super) struct EditorControlState {
    pub(super) hwnd: HWND,
    pub(super) parent: HWND,
    pub(super) is_live: bool,
    pub(super) runtime: HostRuntime,
    pub(super) buffer_id: BufferId,
    pub(super) options: ControlOptions,
    pub(super) event_sink: ControlEventSink,
    pub(super) surface: EditorSurface,
    pub(super) client_width: u32,
    pub(super) client_height: u32,
    pub(super) dpi: u32,
    pub(super) decorations: Option<Arc<Decorations>>,
    pub(super) decoration_revision: Option<Revision>,
    pub(super) drag_anchor: Option<continuity_text::Position>,
    pub(super) pending_high_surrogate: Option<u16>,
    pub(super) accessibility_provider:
        Option<windows::Win32::UI::Accessibility::IRawElementProviderSimple>,
    _com: ComGuard,
    _class: WindowClass,
}

impl EditorControlState {
    pub(super) fn create(
        parent: HWND,
        bounds: ControlBounds,
        runtime_mode: ControlRuntime,
        options: ControlOptions,
        event_sink: ControlEventSink,
    ) -> Result<Box<Self>, Error> {
        let _com = ComGuard::new()?;
        let class = WindowClass::register_unique_with_proc(
            "ContinuityEditorControl",
            Some(wndproc::editor_control_wndproc),
        )?;
        let (runtime, buffer_id) = match runtime_mode {
            ControlRuntime::Ephemeral { initial_text } => {
                let mut runtime = HostRuntime::new();
                let buffer_id = runtime.open_buffer(&initial_text)?;
                (runtime, buffer_id)
            }
            ControlRuntime::HostRuntime { runtime, buffer_id } => {
                runtime.snapshot(buffer_id)?;
                (runtime, buffer_id)
            }
            ControlRuntime::Engine { engine, buffer_id } => {
                let runtime = HostRuntime::from_engine(engine);
                runtime.snapshot(buffer_id)?;
                (runtime, buffer_id)
            }
        };
        runtime.snapshot(buffer_id)?;
        Self::create_with_runtime(
            parent, bounds, runtime, buffer_id, options, event_sink, _com, class,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn create_with_runtime(
        parent: HWND,
        bounds: ControlBounds,
        runtime: HostRuntime,
        buffer_id: BufferId,
        options: ControlOptions,
        event_sink: ControlEventSink,
        _com: ComGuard,
        class: WindowClass,
    ) -> Result<Box<Self>, Error> {
        let dwrite = DWriteFactory::new()?;
        let dpi = continuity_win::dpi_for_window(parent);
        let scale = dpi as f32 / 96.0;
        let font_state = FontStateId::from_parts(
            &options.font_family,
            options.font_size_dip,
            &options.font_locale,
            scale,
        );
        let mut surface = EditorSurface::new(dwrite, font_state);
        surface.view.soft_wrap = options.soft_wrap;
        surface.view.viewport_width_dip = bounds.width.max(1) as f32 / scale;
        surface.view.viewport_height_dip = bounds.height.max(1) as f32 / scale;
        Ok(Box::new(Self {
            hwnd: HWND::default(),
            parent,
            is_live: false,
            runtime,
            buffer_id,
            options,
            event_sink,
            surface,
            client_width: bounds.width.max(1) as u32,
            client_height: bounds.height.max(1) as u32,
            dpi,
            decorations: None,
            decoration_revision: None,
            drag_anchor: None,
            pending_high_surrogate: None,
            accessibility_provider: None,
            _com,
            _class: class,
        }))
    }

    pub(super) fn create_hwnd(&mut self, bounds: ControlBounds) -> Result<(), Error> {
        let mut style: WINDOW_STYLE = WS_CHILD | WS_TABSTOP | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
        if self.options.visible {
            style |= WS_VISIBLE;
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(self._class.name().as_ptr()),
                &HSTRING::from("Continuity editor control"),
                style,
                bounds.x,
                bounds.y,
                bounds.width.max(1),
                bounds.height.max(1),
                Some(self.parent),
                Option::<HMENU>::None,
                Some(self._class.hinstance().into()),
                Some((self as *mut Self).cast()),
            )
        }?;
        self.hwnd = hwnd;
        self.is_live = true;
        if self.options.accept_file_drop {
            unsafe { DragAcceptFiles(hwnd, true) };
        }
        self.refresh_geometry(hwnd)?;
        self.ensure_render_resources(hwnd)?;
        self.publish_accessibility();
        self.dispatch(EditorIntent::ViewportChanged(self.viewport()))?;
        Ok(())
    }

    pub(super) fn validate_live(&self) -> Result<(), Error> {
        if self.is_live && unsafe { IsWindow(Some(self.hwnd)).as_bool() } {
            Ok(())
        } else {
            Err(Error::ControlDestroyed)
        }
    }

    pub(super) fn dispatch(&mut self, intent: EditorIntent) -> Result<(), Error> {
        self.validate_live()?;
        let batch = self.runtime.dispatch(intent)?;
        let should_invalidate = batch.events.iter().any(|event| {
            matches!(
                event,
                HostEvent::Invalidate(
                    Invalidation::Content
                        | Invalidation::Selection
                        | Invalidation::Viewport
                        | Invalidation::InputState
                )
            )
        });
        self.event_sink.deliver(batch)?;
        if should_invalidate {
            self.invalidate();
        }
        self.publish_accessibility();
        Ok(())
    }

    pub(super) fn invalidate(&self) {
        if self.is_live {
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
    }

    pub(super) fn refresh_geometry(&mut self, hwnd: HWND) -> Result<(), Error> {
        let mut rect = RECT::default();
        unsafe { GetClientRect(hwnd, &mut rect)? };
        self.client_width = (rect.right - rect.left).max(1) as u32;
        self.client_height = (rect.bottom - rect.top).max(1) as u32;
        self.dpi = continuity_win::dpi_for_window(hwnd);
        let scale = self.scale();
        self.surface.view.viewport_width_dip = self.client_width as f32 / scale;
        self.surface.view.viewport_height_dip = self.client_height as f32 / scale;
        Ok(())
    }

    pub(super) fn viewport(&self) -> Viewport {
        Viewport {
            width_dip: self.surface.view.viewport_width_dip,
            height_dip: self.surface.view.viewport_height_dip,
            scale: self.scale(),
        }
    }

    pub(super) fn scale(&self) -> f32 {
        self.dpi.max(96) as f32 / 96.0
    }
}

impl Drop for EditorControlState {
    fn drop(&mut self) {
        if self.is_live {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
            }
        }
        let _ = self.runtime.close();
    }
}
