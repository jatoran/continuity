//! Phase-17.6 cross-window registry: live Continuity HWNDs + a small
//! queue of pending tab adoptions used by the cross-window tab-drag
//! drop path.
//!
//! Each window inserts itself on first paint (when its `HWND` becomes
//! known) and removes itself on `WM_DESTROY`. A drag source can then
//! enumerate sibling HWNDs that live on the *current* virtual desktop
//! and pick a drop target without leaning on Win32 class-name filtering
//! (every Continuity window registers its own unique class so a global
//! class filter wouldn't work anyway).
//!
//! **Thread ownership**: the static is touched from multiple UI threads
//! (one per window) but only behind the inner `Mutex`. Lock scopes stay
//! short — push, pop, snapshot — so contention is irrelevant.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use continuity_buffer::BufferId;
use windows::Win32::Foundation::HWND;

/// One queued tab handoff from a source window to a target window.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingAdoption {
    /// HWND that should claim the buffer as a new tab. Stored as the
    /// raw `isize` so the struct is `Send` (HWND is just a pointer).
    pub target_hwnd_raw: isize,
    /// Buffer the target window should open as a fresh tab.
    pub buffer_id: BufferId,
}

#[derive(Default)]
struct Registry {
    /// HWNDs of every live Continuity window, in insertion order.
    hwnds: Vec<isize>,
    /// FIFO of adoptions waiting for the target window's wndproc to
    /// drain them on `WM_USER + 1`.
    adoptions: VecDeque<PendingAdoption>,
}

fn registry() -> &'static Mutex<Registry> {
    /// Shared across every UI thread in the process (one per window).
    /// Lock scopes stay short — see the module-level thread-ownership
    /// note — so contention is irrelevant in practice.
    static REG: OnceLock<Mutex<Registry>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(Registry::default()))
}

/// Insert `hwnd` into the global list. Idempotent — duplicate
/// registrations are merged.
pub(crate) fn register(hwnd: HWND) {
    let raw = hwnd.0 as isize;
    if raw == 0 {
        return;
    }
    let Ok(mut reg) = registry().lock() else {
        return;
    };
    if !reg.hwnds.contains(&raw) {
        reg.hwnds.push(raw);
    }
}

/// Remove `hwnd` from the global list. No-op if absent.
pub(crate) fn unregister(hwnd: HWND) {
    let raw = hwnd.0 as isize;
    let Ok(mut reg) = registry().lock() else {
        return;
    };
    reg.hwnds.retain(|h| *h != raw);
    reg.adoptions.retain(|a| a.target_hwnd_raw != raw);
}

/// Snapshot every live Continuity HWND *other than* `self_hwnd`. The
/// caller is responsible for filtering further (virtual-desktop, rect-
/// under-cursor) before delivering an adoption.
pub(crate) fn snapshot_others(self_hwnd: HWND) -> Vec<HWND> {
    let raw_self = self_hwnd.0 as isize;
    let Ok(reg) = registry().lock() else {
        return Vec::new();
    };
    reg.hwnds
        .iter()
        .copied()
        .filter(|h| *h != raw_self)
        .map(|h| HWND(h as *mut std::ffi::c_void))
        .collect()
}

/// Enqueue an adoption. The source window posts `WM_USER + 1` to the
/// target afterward to trigger the drain.
pub(crate) fn enqueue_adoption(target_hwnd: HWND, buffer_id: BufferId) {
    let raw = target_hwnd.0 as isize;
    if raw == 0 {
        return;
    }
    let Ok(mut reg) = registry().lock() else {
        return;
    };
    reg.adoptions.push_back(PendingAdoption {
        target_hwnd_raw: raw,
        buffer_id,
    });
}

/// Drain every queued adoption targeted at `hwnd`. Called from the
/// target window's wndproc on `WM_USER + 1`.
pub(crate) fn drain_adoptions_for(hwnd: HWND) -> Vec<PendingAdoption> {
    let raw = hwnd.0 as isize;
    let Ok(mut reg) = registry().lock() else {
        return Vec::new();
    };
    let mut kept = VecDeque::with_capacity(reg.adoptions.len());
    let mut taken = Vec::new();
    while let Some(a) = reg.adoptions.pop_front() {
        if a.target_hwnd_raw == raw {
            taken.push(a);
        } else {
            kept.push_back(a);
        }
    }
    reg.adoptions = kept;
    taken
}
