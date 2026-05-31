//! Live-resize repaint flush and compositor fence helpers.
//!
//! Thread ownership: each helper runs on the owning [`Window`] UI thread
//! inside the Win32 sizing message path.

use std::time::Instant;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::UpdateWindow;

use crate::window::Window;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClientSize {
    width: u32,
    height: u32,
}

impl ClientSize {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClientResizeDelta {
    Same,
    Grow,
    Shrink,
    Mixed,
}

impl ClientResizeDelta {
    pub(crate) fn from_clients(old_client: ClientSize, new_client: ClientSize) -> Self {
        let has_shrink_axis =
            new_client.width < old_client.width || new_client.height < old_client.height;
        let has_grow_axis =
            new_client.width > old_client.width || new_client.height > old_client.height;
        match (has_shrink_axis, has_grow_axis) {
            (false, false) => Self::Same,
            (false, true) => Self::Grow,
            (true, false) => Self::Shrink,
            (true, true) => Self::Mixed,
        }
    }

    pub(crate) fn as_trace_label(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Grow => "grow",
            Self::Shrink => "shrink",
            Self::Mixed => "mixed",
        }
    }

    pub(crate) fn has_shrink_axis(self) -> bool {
        matches!(self, Self::Shrink | Self::Mixed)
    }
}

impl Window {
    pub(crate) fn handle_live_resize_update_window(
        &mut self,
        hwnd: HWND,
        old_width: u32,
        old_height: u32,
    ) {
        let old_client = ClientSize::new(old_width, old_height);
        let new_client = ClientSize::new(self.client_width, self.client_height);
        let delta = ClientResizeDelta::from_clients(old_client, new_client);
        let tracing = crate::paint_trace::is_trace_enabled();
        let update_started = tracing.then(Instant::now);
        let updated = unsafe { UpdateWindow(hwnd).as_bool() };
        let update_elapsed_us = update_started.map(elapsed_us).unwrap_or(0);
        let renderer_target = self.live_resize_renderer_target();

        if tracing {
            crate::paint_trace::log_event(
                "live_resize_update_window",
                &format!(
                    concat!(
                        "updated={} old_client={}x{} new_client={}x{} ",
                        "delta={} elapsed_us={} client={}x{} renderer_target={}x{}"
                    ),
                    updated,
                    old_client.width,
                    old_client.height,
                    new_client.width,
                    new_client.height,
                    delta.as_trace_label(),
                    update_elapsed_us,
                    new_client.width,
                    new_client.height,
                    renderer_target.width,
                    renderer_target.height,
                ),
            );
        }

        if delta.has_shrink_axis() {
            self.flush_live_resize_dwm(old_client, new_client, delta, renderer_target, tracing);
        }
    }

    fn flush_live_resize_dwm(
        &self,
        old_client: ClientSize,
        new_client: ClientSize,
        delta: ClientResizeDelta,
        renderer_target: ClientSize,
        tracing: bool,
    ) {
        let flush_started = tracing.then(Instant::now);
        let flush_result = unsafe { DwmFlush() };
        let elapsed_us = flush_started.map(elapsed_us).unwrap_or(0);

        if tracing {
            let (result_label, hresult) = match &flush_result {
                Ok(()) => ("ok", 0),
                Err(error) => ("err", error.code().0 as u32),
            };
            crate::paint_trace::log_event(
                "live_resize_dwm_flush",
                &format!(
                    concat!(
                        "attempted=true result={} hr=0x{:08X} elapsed_us={} ",
                        "old_client={}x{} new_client={}x{} delta={} renderer_target={}x{}"
                    ),
                    result_label,
                    hresult,
                    elapsed_us,
                    old_client.width,
                    old_client.height,
                    new_client.width,
                    new_client.height,
                    delta.as_trace_label(),
                    renderer_target.width,
                    renderer_target.height,
                ),
            );
        }
    }

    fn live_resize_renderer_target(&self) -> ClientSize {
        self.renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.back_buffer_size();
                ClientSize::new(width, height)
            })
            .unwrap_or_else(|| ClientSize::new(0, 0))
    }
}

fn elapsed_us(started: Instant) -> u64 {
    let micros = started.elapsed().as_micros();
    if micros > u128::from(u64::MAX) {
        u64::MAX
    } else {
        micros as u64
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientResizeDelta, ClientSize};

    #[test]
    fn classify_resize_delta_tracks_shrink_axes() {
        assert_eq!(
            ClientResizeDelta::from_clients(ClientSize::new(100, 100), ClientSize::new(100, 100)),
            ClientResizeDelta::Same
        );
        assert_eq!(
            ClientResizeDelta::from_clients(ClientSize::new(100, 100), ClientSize::new(120, 100)),
            ClientResizeDelta::Grow
        );
        assert_eq!(
            ClientResizeDelta::from_clients(ClientSize::new(100, 100), ClientSize::new(90, 100)),
            ClientResizeDelta::Shrink
        );
        assert_eq!(
            ClientResizeDelta::from_clients(ClientSize::new(100, 100), ClientSize::new(90, 120)),
            ClientResizeDelta::Mixed
        );
        assert!(ClientResizeDelta::Shrink.has_shrink_axis());
        assert!(ClientResizeDelta::Mixed.has_shrink_axis());
        assert!(!ClientResizeDelta::Grow.has_shrink_axis());
        assert!(!ClientResizeDelta::Same.has_shrink_axis());
    }
}
