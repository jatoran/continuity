//! Single-instance primitives: a named mutex that detects an already-running
//! process and a message-only "hub" window that receives forwarded command
//! lines over `WM_COPYDATA`.
//!
//! The mutex name and hub window class are supplied by the caller so the
//! same primitives serve any data-dir-scoped instance key. The hub HWND is
//! owned by its dedicated pump thread (single-writer); the receive callback
//! runs on that thread.

use std::ffi::c_void;
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, BOOL, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumWindows,
    FindWindowExW, GetMessageW, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, PostMessageW, PostQuitMessage, RegisterClassW, SendMessageTimeoutW,
    SetForegroundWindow, SetWindowLongPtrW, ShowWindow, TranslateMessage, UnregisterClassW,
    CREATESTRUCTW, GWLP_USERDATA, HMENU, HWND_MESSAGE, MSG, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    SW_RESTORE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COPYDATA, WM_DESTROY, WM_NCCREATE,
    WNDCLASSW,
};

use crate::Error;

/// `COPYDATASTRUCT::dwData` tag identifying a continuity instance handoff.
/// Foreign `WM_COPYDATA` traffic without this tag is ignored.
const COPYDATA_MAGIC: usize = 0x434F_4E54; // "CONT"

/// Callback invoked on the hub pump thread for each forwarded payload.
pub type HubCallback = Box<dyn Fn(&str) + Send>;

/// Holder of the named instance mutex. Keep it alive for the process
/// lifetime; dropping it (or process exit) releases the claim.
pub struct SingleInstanceMutex {
    handle: HANDLE,
}

impl SingleInstanceMutex {
    /// Create-or-open the named mutex. Returns `Ok(Some(_))` when this
    /// process created it (no other instance), `Ok(None)` when another
    /// process already holds the name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Win32`] if `CreateMutexW` fails outright.
    pub fn acquire(name: &str) -> Result<Option<Self>, Error> {
        let name = HSTRING::from(name);
        let handle = unsafe { CreateMutexW(None, false, &name) }
            .map_err(|e| Error::win32("CreateMutexW", e))?;
        // `CreateMutexW` succeeds even when the name exists; the
        // distinction arrives via last-error. Kernel objects die with
        // their owning processes, so a crashed instance never leaves a
        // stale claim behind.
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already_exists {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Ok(None);
        }
        Ok(Some(Self { handle }))
    }
}

impl Drop for SingleInstanceMutex {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

// SAFETY: the mutex handle is a kernel object reference; closing it from
// any thread is valid.
unsafe impl Send for SingleInstanceMutex {}

/// Message-only window that receives forwarded payloads. The HWND lives on
/// a dedicated pump thread (its single writer); dropping the hub posts
/// `WM_CLOSE` and joins that thread.
pub struct InstanceHub {
    hwnd_raw: isize,
    pump: Option<JoinHandle<()>>,
}

impl InstanceHub {
    /// Spawn the hub pump thread, registering `class_name` and creating a
    /// message-only window under `HWND_MESSAGE`. `on_payload` runs on the
    /// pump thread for every forwarded payload.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Win32`] when class registration or window
    /// creation fails on the pump thread.
    pub fn spawn(class_name: &str, on_payload: HubCallback) -> Result<Self, Error> {
        let class_name = class_name.to_owned();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<isize, Error>>();
        let pump = std::thread::Builder::new()
            .name("continuity-instance-hub".into())
            .spawn(move || hub_pump_main(&class_name, on_payload, &ready_tx))
            .map_err(|_| Error::win32("CreateThread", windows::core::Error::from_win32()))?;
        match ready_rx.recv() {
            Ok(Ok(hwnd_raw)) => Ok(Self {
                hwnd_raw,
                pump: Some(pump),
            }),
            Ok(Err(e)) => {
                let _ = pump.join();
                Err(e)
            }
            Err(_) => {
                let _ = pump.join();
                Err(Error::win32(
                    "InstanceHub::spawn",
                    windows::core::Error::from_win32(),
                ))
            }
        }
    }
}

impl Drop for InstanceHub {
    fn drop(&mut self) {
        let hwnd = HWND(self.hwnd_raw as *mut c_void);
        unsafe {
            // Posting (not sending) lets the wndproc drive its own
            // shutdown: WM_CLOSE → DestroyWindow → WM_DESTROY →
            // PostQuitMessage, which exits the pump loop cleanly.
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
    }
}

fn hub_pump_main(
    class_name: &str,
    on_payload: HubCallback,
    ready: &mpsc::Sender<Result<isize, Error>>,
) {
    let class_name = HSTRING::from(class_name);
    let hinstance = match unsafe { GetModuleHandleW(None) } {
        Ok(h) => h,
        Err(e) => {
            let _ = ready.send(Err(Error::win32("GetModuleHandleW", e)));
            return;
        }
    };
    let class = WNDCLASSW {
        style: Default::default(),
        lpfnWndProc: Some(hub_wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance.into(),
        hIcon: Default::default(),
        hCursor: Default::default(),
        hbrBackground: Default::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: PCWSTR(class_name.as_ptr()),
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        let _ = ready.send(Err(Error::win32(
            "RegisterClassW",
            windows::core::Error::from_win32(),
        )));
        return;
    }
    let callback_raw = Box::into_raw(Box::new(on_payload));
    let created = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            &HSTRING::from("continuity-instance-hub"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            // HWND_MESSAGE parent ⇒ message-only window: invisible,
            // excluded from EnumWindows, reachable via FindWindowExW.
            Some(HWND_MESSAGE),
            Option::<HMENU>::None,
            Some(hinstance.into()),
            Some(callback_raw as *const c_void),
        )
    };
    let _hwnd = match created {
        Ok(hwnd) => {
            let _ = ready.send(Ok(hwnd.0 as isize));
            hwnd
        }
        Err(e) => {
            // WM_NCCREATE never ran, so the callback box is still ours.
            drop(unsafe { Box::from_raw(callback_raw) });
            unsafe {
                let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(hinstance.into()));
            }
            let _ = ready.send(Err(Error::win32("CreateWindowExW", e)));
            return;
        }
    };
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(hinstance.into()));
    }
}

unsafe extern "system" fn hub_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let create = lp.0 as *const CREATESTRUCTW;
            if !create.is_null() {
                let params = unsafe { (*create).lpCreateParams };
                unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, params as isize) };
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_COPYDATA => {
            let callback = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const HubCallback;
            let data = lp.0 as *const COPYDATASTRUCT;
            if callback.is_null() || data.is_null() {
                return LRESULT(0);
            }
            let data = unsafe { &*data };
            if data.dwData != COPYDATA_MAGIC || data.lpData.is_null() {
                return LRESULT(0);
            }
            let bytes = unsafe {
                std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize)
            };
            let payload = String::from_utf8_lossy(bytes);
            (unsafe { &*callback })(&payload);
            LRESULT(1)
        }
        WM_DESTROY => {
            let raw = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut HubCallback;
            if !raw.is_null() {
                drop(unsafe { Box::from_raw(raw) });
            }
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Forward `payload` to a running instance's hub window. Returns
/// `Ok(false)` when no hub window with `class_name` exists; `Ok(true)`
/// when the hub received and acknowledged the payload.
///
/// Also grants the hub's process the right to take the foreground
/// (`AllowSetForegroundWindow`) so it may activate or spawn a window in
/// response — the sender is the freshly launched, foreground-entitled
/// process.
///
/// # Errors
///
/// This function reports unreachable hubs via `Ok(false)` rather than
/// errors; the `Result` exists for future payload-size validation only.
pub fn send_to_instance_hub(
    class_name: &str,
    payload: &str,
    timeout_ms: u32,
) -> Result<bool, Error> {
    let class = HSTRING::from(class_name);
    let hwnd = match unsafe { FindWindowExW(Some(HWND_MESSAGE), None, &class, PCWSTR::null()) } {
        Ok(hwnd) if !hwnd.is_invalid() => hwnd,
        _ => return Ok(false),
    };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid != 0 {
        unsafe {
            let _ = AllowSetForegroundWindow(pid);
        }
    }
    let bytes = payload.as_bytes();
    let Ok(len) = u32::try_from(bytes.len()) else {
        return Ok(false);
    };
    let data = COPYDATASTRUCT {
        dwData: COPYDATA_MAGIC,
        cbData: len,
        lpData: bytes.as_ptr() as *mut c_void,
    };
    let mut ack: usize = 0;
    let sent = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(std::ptr::addr_of!(data) as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            timeout_ms,
            Some(&mut ack),
        )
    };
    Ok(sent.0 != 0 && ack == 1)
}

/// Bring this process's top-most visible window to the foreground
/// (restoring it first when minimized). Returns `false` when the process
/// has no visible top-level window.
pub fn activate_first_visible_window_of_current_process() -> bool {
    struct EnumTarget {
        pid: u32,
        found_hwnd_raw: isize,
    }
    unsafe extern "system" fn enum_first_visible_for_pid(hwnd: HWND, lp: LPARAM) -> BOOL {
        let target = unsafe { &mut *(lp.0 as *mut EnumTarget) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == target.pid && unsafe { IsWindowVisible(hwnd) }.as_bool() {
            target.found_hwnd_raw = hwnd.0 as isize;
            return BOOL(0); // stop enumerating — EnumWindows walks top-down in z-order
        }
        BOOL(1)
    }
    let mut target = EnumTarget {
        pid: unsafe { GetCurrentProcessId() },
        found_hwnd_raw: 0,
    };
    unsafe {
        // EnumWindows returns Err when the callback stops it early —
        // that is the success path here, so the result is ignored.
        let _ = EnumWindows(
            Some(enum_first_visible_for_pid),
            LPARAM(std::ptr::addr_of_mut!(target) as isize),
        );
    }
    if target.found_hwnd_raw == 0 {
        return false;
    }
    let hwnd = HWND(target.found_hwnd_raw as *mut c_void);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd).as_bool()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn mutex_first_acquire_wins_second_sees_existing() {
        let name = format!("Local\\continuity-test-{}", std::process::id());
        let first = SingleInstanceMutex::acquire(&name).unwrap();
        assert!(first.is_some());
        let second = SingleInstanceMutex::acquire(&name).unwrap();
        assert!(second.is_none());
        drop(first);
        let third = SingleInstanceMutex::acquire(&name).unwrap();
        assert!(third.is_some());
    }

    #[test]
    fn hub_round_trips_payload() {
        let class = format!("ContinuityHubTest_{}", std::process::id());
        let received = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&received);
        let hub = InstanceHub::spawn(
            &class,
            Box::new(move |payload| {
                sink.lock().unwrap().push(payload.to_owned());
            }),
        )
        .unwrap();
        let delivered = send_to_instance_hub(&class, "{\"files\":[\"a.md\"]}", 2_000).unwrap();
        assert!(delivered);
        assert_eq!(
            received.lock().unwrap().as_slice(),
            ["{\"files\":[\"a.md\"]}"]
        );
        drop(hub);
        // After the hub is gone the class is unregistered and sends miss.
        let delivered = send_to_instance_hub(&class, "x", 500).unwrap();
        assert!(!delivered);
    }

    #[test]
    fn send_to_missing_hub_reports_not_found() {
        let delivered = send_to_instance_hub("ContinuityHubNeverRegistered", "x", 200).unwrap();
        assert!(!delivered);
    }
}
