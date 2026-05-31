//! Process-level resource snapshot for the trace stream.
//!
//! Periodically queries Win32 for working-set memory, private commit
//! charge (`PROCESS_MEMORY_COUNTERS_EX::PrivateUsage`), GDI/User handle
//! counts, and the OS handle count, then emits an `event:process_state`
//! line via [`crate::paint_trace`]. Cadence is owned by the running-
//! summary flush timer (see [`crate::window_trace_summary_timer`]);
//! this module is the data-collection half.
//!
//! `private_bytes` is the process commit charge: every allocation
//! regardless of subsystem (heap, D3D resources, DWrite, SQLite, COM,
//! ...). It is the single number that bounds the "unattributed" delta
//! when reconciling working-set growth against the per-subsystem
//! `event:memory_breakdown` totals. Win32 does not provide a built-in
//! peak for `PrivateUsage`, so we track a process-wide
//! `AtomicU64::fetch_max` HWM and emit it alongside as
//! `private_bytes_hwm`, matching the HWM convention used by
//! [`crate::memory_trace`].
//!
//! All Win32 calls are read-only and cheap (~µs each); no allocations.

use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetGuiResources, GetProcessHandleCount, GR_GDIOBJECTS, GR_USEROBJECTS,
};

/// Process-wide high-water mark for `PrivateUsage` (commit charge).
/// Updated via `fetch_max` on every snapshot; mirrors the HWM
/// convention used in [`crate::memory_trace`].
static PRIVATE_BYTES_HWM: AtomicU64 = AtomicU64::new(0);

/// Emit one `event:process_state` line with the latest counters. No-op
/// when tracing is disabled. Captures working-set memory, private
/// commit charge (`PrivateUsage`), pagefile usage, OS handle count,
/// and GDI / USER object counts.
pub(crate) fn emit_snapshot() {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    let process = unsafe { GetCurrentProcess() };
    // Query `PROCESS_MEMORY_COUNTERS_EX` so we get `PrivateUsage` in
    // addition to the base counters. The struct is layout-compatible
    // at the head with `PROCESS_MEMORY_COUNTERS`; we cast through
    // that pointer type because `GetProcessMemoryInfo`'s signature
    // takes the base type. `cb` is the *full* `_EX` size — the
    // kernel uses it to decide how many bytes to write back.
    let mut mem = PROCESS_MEMORY_COUNTERS_EX::default();
    let mem_size = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>()).unwrap_or(0);
    let mem_ptr: *mut PROCESS_MEMORY_COUNTERS =
        (&mut mem as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>();
    let mem_ok = unsafe { GetProcessMemoryInfo(process, mem_ptr, mem_size).is_ok() };
    let mut handle_count: u32 = 0;
    let handles_ok = unsafe { GetProcessHandleCount(process, &mut handle_count).is_ok() };
    // `GetGuiResources` returns 0 on failure (and for processes that
    // have never created any GUI objects yet). Treat 0 as "no signal."
    let gdi = unsafe { GetGuiResources(process, GR_GDIOBJECTS) };
    let user = unsafe { GetGuiResources(process, GR_USEROBJECTS) };
    let power = power_state_fields();

    if !mem_ok && !handles_ok && gdi == 0 && user == 0 {
        return;
    }
    let private_bytes = mem.PrivateUsage as u64;
    let private_bytes_hwm = PRIVATE_BYTES_HWM.fetch_max(private_bytes, Ordering::Relaxed);
    let private_bytes_hwm = private_bytes_hwm.max(private_bytes);
    let detail = format!(
        "ws_bytes={ws} peak_ws_bytes={peak} private_bytes={pv} private_bytes_hwm={pv_hwm} \
         pagefile_bytes={pf} peak_pagefile_bytes={pf_peak} \
         handles={handles} gdi_objects={gdi} user_objects={user} \
         ac={ac} battery_pct={battery_pct} saver={saver}",
        ws = mem.WorkingSetSize as u64,
        peak = mem.PeakWorkingSetSize as u64,
        pv = private_bytes,
        pv_hwm = private_bytes_hwm,
        pf = mem.PagefileUsage as u64,
        pf_peak = mem.PeakPagefileUsage as u64,
        handles = handle_count,
        gdi = gdi,
        user = user,
        ac = power.ac,
        battery_pct = power.battery_pct,
        saver = power.saver,
    );
    crate::paint_trace::log_event("process_state", &detail);
}

struct PowerStateFields {
    ac: &'static str,
    battery_pct: i32,
    saver: &'static str,
}

fn power_state_fields() -> PowerStateFields {
    let mut status = SYSTEM_POWER_STATUS::default();
    let ok = unsafe { GetSystemPowerStatus(&mut status).is_ok() };
    if !ok {
        return PowerStateFields {
            ac: "unknown",
            battery_pct: -1,
            saver: "unknown",
        };
    }
    PowerStateFields {
        ac: match status.ACLineStatus {
            0 => "off",
            1 => "on",
            _ => "unknown",
        },
        battery_pct: match status.BatteryLifePercent {
            255 => -1,
            pct => i32::from(pct),
        },
        saver: if status.SystemStatusFlag == 1 {
            "on"
        } else {
            "off"
        },
    }
}
