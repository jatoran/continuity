//! GPU video-memory accounting accessors on [`crate::Renderer`].
//!
//! Pulled out of `renderer.rs` so the draw orchestrator stays under the
//! file-length cap. Surfaces the process's DXGI video-memory usage
//! (`IDXGIAdapter3::QueryVideoMemoryInfo`) for memory-attribution
//! diagnostics emitted on `event:memory_breakdown`.
//!
//! GPU memory is *mostly not* part of `PROCESS_MEMORY_COUNTERS_EX::
//! PrivateUsage`: the local (VRAM) segment lives on the adapter, and the
//! non-local (shared) segment is system memory the GPU can address. The
//! trace consumer reports these separately so a memory regression can be
//! split into "going to GPU" vs "going to the CPU heap".
//!
//! **Thread ownership**: UI thread (owns the renderer and its D3D11
//! device).

use windows::core::Interface;
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter3, IDXGIDevice, DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
    DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
};

use crate::Renderer;

/// Process video-memory usage as reported by DXGI, in bytes.
///
/// `local_bytes` is current usage of the local (dedicated VRAM) memory
/// segment group; `nonlocal_bytes` is current usage of the non-local
/// (shared system) segment group. Both are `CurrentUsage` from
/// [`DXGI_QUERY_VIDEO_MEMORY_INFO`]. All zero when the adapter does not
/// expose `IDXGIAdapter3` (pre-Windows-10 / WDDM 2.0) or any query
/// fails — callers treat a zero pair as "no signal" rather than an
/// error.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GpuMemoryInfo {
    /// `CurrentUsage` of `DXGI_MEMORY_SEGMENT_GROUP_LOCAL` (VRAM), bytes.
    pub local_bytes: u64,
    /// `CurrentUsage` of `DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL` (shared
    /// system memory addressable by the GPU), bytes.
    pub nonlocal_bytes: u64,
}

impl Renderer {
    /// Query DXGI for this process's current GPU video-memory usage
    /// across the local (VRAM) and non-local (shared) segment groups.
    ///
    /// Returns [`GpuMemoryInfo::default`] (all zeros) when the adapter
    /// does not implement `IDXGIAdapter3` (older Windows) or any
    /// underlying call fails. This is a diagnostic surface, not a
    /// hot-path call, and never errors out to the caller — a missing
    /// signal is reported as zero so the trace stays well-formed.
    #[must_use]
    pub fn gpu_memory_info(&self) -> GpuMemoryInfo {
        // device → IDXGIDevice → IDXGIAdapter → (QI) IDXGIAdapter3.
        // Any step can fail on older runtimes; fall back to zeros.
        let Ok(dxgi_device) = self.device.cast::<IDXGIDevice>() else {
            return GpuMemoryInfo::default();
        };
        let Ok(adapter) = (unsafe { dxgi_device.GetAdapter() }) else {
            return GpuMemoryInfo::default();
        };
        let Ok(adapter3) = adapter.cast::<IDXGIAdapter3>() else {
            // Pre-WDDM-2.0 / older Windows: no QueryVideoMemoryInfo.
            return GpuMemoryInfo::default();
        };
        let local = query_segment(&adapter3, DXGI_MEMORY_SEGMENT_GROUP_LOCAL);
        let nonlocal = query_segment(&adapter3, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL);
        GpuMemoryInfo {
            local_bytes: local,
            nonlocal_bytes: nonlocal,
        }
    }
}

impl Renderer {
    /// Deterministic byte estimate of the swap-chain back buffers we
    /// own: `width × height × 4 (B8G8R8A8) × buffer_count`.
    ///
    /// `buffer_count` is the constant `2` configured in
    /// [`Renderer::for_hwnd`] (flip-discard double buffering). The back
    /// buffers live in GPU-addressable memory; their system-memory
    /// footprint (if any) is implementation-defined, so this is reported
    /// as a *separate* informational figure and is NOT folded into the
    /// `private_bytes` residual.
    #[must_use]
    pub fn swapchain_bytes_estimate(&self) -> u64 {
        /// Bytes per pixel for the `DXGI_FORMAT_B8G8R8A8_UNORM` back
        /// buffer format used by [`Renderer::for_hwnd`].
        const BYTES_PER_PIXEL: u64 = 4;
        /// Back-buffer count configured on the swap chain
        /// (`DXGI_SWAP_CHAIN_DESC1::BufferCount` in `for_hwnd`).
        const BUFFER_COUNT: u64 = 2;
        u64::from(self.target_width_px)
            * u64::from(self.target_height_px)
            * BYTES_PER_PIXEL
            * BUFFER_COUNT
    }
}

/// Query one memory segment group's `CurrentUsage`. Returns 0 on
/// failure so a single failing segment does not poison the whole
/// snapshot.
fn query_segment(
    adapter: &IDXGIAdapter3,
    group: windows::Win32::Graphics::Dxgi::DXGI_MEMORY_SEGMENT_GROUP,
) -> u64 {
    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
    // `node_index = 0`: single-GPU node. Multi-adapter / linked-node
    // setups would report only node 0, which is the correct primary for
    // a single swap-chain renderer.
    let ok = unsafe { adapter.QueryVideoMemoryInfo(0, group, &mut info).is_ok() };
    if ok {
        info.CurrentUsage
    } else {
        0
    }
}
