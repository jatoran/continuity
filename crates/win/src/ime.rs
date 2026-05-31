//! Win32 IME (Input Method Editor) wrappers.
//!
//! Thread ownership: caller's UI thread (HIMC is a per-window resource).
//! Each entry point acquires an HIMC via `ImmGetContext`, performs one
//! query, and releases it before returning.

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::Input::Ime::{
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext, ImmSetCandidateWindow,
    ImmSetCompositionWindow, CANDIDATEFORM, CFS_POINT, COMPOSITIONFORM, GCS_COMPSTR, GCS_CURSORPOS,
    GCS_RESULTSTR, HIMC, IME_COMPOSITION_STRING,
};
use windows::Win32::UI::WindowsAndMessaging::WM_IME_REQUEST;

/// Result of a `WM_IME_COMPOSITION` query.
#[derive(Debug, Default, Clone)]
pub struct CompositionState {
    /// In-progress composition string (UTF-8). Empty when the IME has no
    /// pending composition (e.g., after a commit).
    pub comp: String,
    /// Caret offset within `comp`, in UTF-8 bytes.
    pub caret_byte: usize,
    /// Committed result string from `GCS_RESULTSTR` (UTF-8). Non-empty
    /// only when the user just committed the composition.
    pub result: String,
}

/// Read the current composition + result strings from the HIMC owned by
/// `hwnd`. Returns `None` when no IME is attached to the window.
#[must_use]
pub fn read_composition(hwnd: HWND, lparam: isize) -> Option<CompositionState> {
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_invalid() {
            return None;
        }
        let mut state = CompositionState::default();
        let lparam_u = lparam as usize;
        if (lparam_u & GCS_COMPSTR.0 as usize) != 0 {
            state.comp = read_compstr(himc, GCS_COMPSTR);
        }
        if (lparam_u & GCS_CURSORPOS.0 as usize) != 0 {
            let units = ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0);
            if units >= 0 {
                let utf16_off = units as usize;
                let comp_utf16: Vec<u16> = state.comp.encode_utf16().collect();
                let take = utf16_off.min(comp_utf16.len());
                let prefix = String::from_utf16_lossy(&comp_utf16[..take]);
                state.caret_byte = prefix.len();
            }
        }
        if (lparam_u & GCS_RESULTSTR.0 as usize) != 0 {
            state.result = read_compstr(himc, GCS_RESULTSTR);
        }
        let _ = ImmReleaseContext(hwnd, himc);
        Some(state)
    }
}

/// Move the IME composition window so it tracks the caret on screen.
///
/// `(x, y)` is in client-area pixels relative to `hwnd`.
pub fn set_composition_position(hwnd: HWND, x: i32, y: i32) {
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_invalid() {
            return;
        }
        let form = COMPOSITIONFORM {
            dwStyle: CFS_POINT,
            ptCurrentPos: POINT { x, y },
            rcArea: RECT::default(),
        };
        let _ = ImmSetCompositionWindow(himc, &form);
        let cand = CANDIDATEFORM {
            dwIndex: 0,
            dwStyle: CFS_POINT,
            ptCurrentPos: POINT { x, y },
            rcArea: RECT::default(),
        };
        let _ = ImmSetCandidateWindow(himc, &cand);
        let _ = ImmReleaseContext(hwnd, himc);
    }
}

unsafe fn read_compstr(himc: HIMC, flag: IME_COMPOSITION_STRING) -> String {
    let bytes = ImmGetCompositionStringW(himc, flag, None, 0);
    if bytes <= 0 {
        return String::new();
    }
    let bytes_u = bytes as u32;
    let units = bytes_u as usize / std::mem::size_of::<u16>();
    let mut buf = vec![0u16; units];
    let written = ImmGetCompositionStringW(
        himc,
        flag,
        Some(buf.as_mut_ptr() as *mut std::ffi::c_void),
        bytes_u,
    );
    if written <= 0 {
        return String::new();
    }
    let written_units = written as usize / std::mem::size_of::<u16>();
    String::from_utf16_lossy(&buf[..written_units.min(buf.len())])
}

/// Reconversion request marker (`WM_IME_REQUEST` / `IMR_RECONVERTSTRING`).
pub const IMR_RECONVERTSTRING_U: usize =
    windows::Win32::UI::Input::Ime::IMR_RECONVERTSTRING as usize;

/// Re-exported `WM_IME_REQUEST` so window-proc match arms can pattern
/// against this constant without touching `windows::*` directly.
pub const WM_IME_REQUEST_U32: u32 = WM_IME_REQUEST;
