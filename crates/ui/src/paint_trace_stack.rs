//! Stall-stack capture and deferred symbol formatting for paint trace rows.
//!
//! Thread ownership: capture runs on the offending UI thread when a
//! `stall100` row is emitted. DbgHelp loading / symbol lookup is lazy
//! and only happens on the severe-stall path.

use std::ffi::{c_void, CStr};
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use windows::core::{PCSTR, PCWSTR};
use windows::Win32::Foundation::{BOOL, HANDLE, HMODULE, TRUE};
use windows::Win32::System::Diagnostics::Debug::{
    RtlCaptureStackBackTrace, IMAGEHLP_LINE64, IMAGEHLP_MODULE64, SYMBOL_INFO,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows::Win32::System::Threading::GetCurrentProcess;

const MAX_FRAMES: usize = 20;
const MAX_SYMBOL_NAME: usize = 255;

type SymInitializeFn = unsafe extern "system" fn(HANDLE, PCSTR, BOOL) -> BOOL;
type SymFromAddrFn = unsafe extern "system" fn(HANDLE, u64, *mut u64, *mut SYMBOL_INFO) -> BOOL;
type SymGetLineFromAddr64Fn =
    unsafe extern "system" fn(HANDLE, u64, *mut u32, *mut IMAGEHLP_LINE64) -> BOOL;
type SymGetModuleInfo64Fn = unsafe extern "system" fn(HANDLE, u64, *mut IMAGEHLP_MODULE64) -> BOOL;
type SymSetSearchPathWFn = unsafe extern "system" fn(HANDLE, PCWSTR) -> BOOL;
type SymSetOptionsFn = unsafe extern "system" fn(u32) -> u32;

const SYMOPT_UNDNAME: u32 = 0x0000_0002;
const SYMOPT_LOAD_LINES: u32 = 0x0000_0010;

struct DbgHelp {
    _module: isize,
    process: isize,
    initialized: AtomicBool,
    sym_initialize: SymInitializeFn,
    sym_set_options: SymSetOptionsFn,
    sym_set_search_path_w: SymSetSearchPathWFn,
    sym_from_addr: SymFromAddrFn,
    sym_get_module_info: SymGetModuleInfo64Fn,
    sym_get_line_from_addr: SymGetLineFromAddr64Fn,
}

/// Capture and format a stack row for a severe stall.
pub(crate) fn stall_stack_detail(
    label: &str,
    duration_us: u128,
    original_detail: &str,
) -> Option<String> {
    let frames = capture_stack_frames();
    if frames.is_empty() {
        return None;
    }
    let mut detail = format!("label={} duration_us={duration_us}", sanitize_value(label));
    if !original_detail.is_empty() {
        detail.push(' ');
        detail.push_str(original_detail);
    }
    for (idx, frame) in frames.iter().enumerate() {
        detail.push_str(&format!(
            " frame_{idx}={}",
            sanitize_value(&symbolize_frame(*frame))
        ));
    }
    Some(detail)
}

fn capture_stack_frames() -> Vec<u64> {
    let mut frames = [std::ptr::null_mut::<c_void>(); MAX_FRAMES];
    let captured = unsafe { RtlCaptureStackBackTrace(2, &mut frames, None) };
    frames
        .iter()
        .take(usize::from(captured))
        .filter_map(|frame| {
            let addr = *frame as usize as u64;
            (addr != 0).then_some(addr)
        })
        .collect()
}

fn symbolize_frame(addr: u64) -> String {
    let Some(dbghelp) = dbghelp() else {
        return format!("0x{addr:x}");
    };
    if !dbghelp.ensure_initialized() {
        return format!("0x{addr:x}");
    }
    let symbol = dbghelp.symbol_name(addr);
    let line = dbghelp.line_name(addr);
    let module_offset = dbghelp.module_offset_name(addr);
    match (symbol, line, module_offset) {
        (Some(symbol), Some(line), _) => format!("{symbol}@{line}"),
        (Some(symbol), None, _) => symbol,
        (None, Some(line), Some(module_offset)) => format!("{module_offset}@{line}"),
        (None, Some(line), None) => format!("0x{addr:x}@{line}"),
        (None, None, Some(module_offset)) => module_offset,
        (None, None, None) => format!("0x{addr:x}"),
    }
}

fn dbghelp() -> Option<&'static DbgHelp> {
    static DBGHELP: OnceLock<Option<DbgHelp>> = OnceLock::new();
    DBGHELP.get_or_init(load_dbghelp).as_ref()
}

fn load_dbghelp() -> Option<DbgHelp> {
    let module = unsafe { LoadLibraryA(PCSTR(c"dbghelp.dll".as_ptr().cast())).ok()? };
    let sym_initialize = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, SymInitializeFn>(load_proc(
            module,
            c"SymInitialize",
        )?)
    };
    let sym_from_addr = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, SymFromAddrFn>(load_proc(
            module,
            c"SymFromAddr",
        )?)
    };
    let sym_set_options = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, SymSetOptionsFn>(load_proc(
            module,
            c"SymSetOptions",
        )?)
    };
    let sym_set_search_path_w = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, SymSetSearchPathWFn>(load_proc(
            module,
            c"SymSetSearchPathW",
        )?)
    };
    let sym_get_module_info = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, SymGetModuleInfo64Fn>(
            load_proc(module, c"SymGetModuleInfo64")?,
        )
    };
    let sym_get_line_from_addr = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, SymGetLineFromAddr64Fn>(
            load_proc(module, c"SymGetLineFromAddr64")?,
        )
    };
    Some(DbgHelp {
        _module: module.0 as isize,
        process: unsafe { GetCurrentProcess() }.0 as isize,
        initialized: AtomicBool::new(false),
        sym_initialize,
        sym_set_options,
        sym_set_search_path_w,
        sym_from_addr,
        sym_get_module_info,
        sym_get_line_from_addr,
    })
}

fn load_proc(module: HMODULE, name: &CStr) -> Option<unsafe extern "system" fn() -> isize> {
    unsafe { GetProcAddress(module, PCSTR(name.as_ptr().cast())) }
}

impl DbgHelp {
    fn ensure_initialized(&self) -> bool {
        if self.initialized.load(Ordering::Acquire) {
            return true;
        }
        unsafe {
            (self.sym_set_options)(SYMOPT_UNDNAME | SYMOPT_LOAD_LINES);
        }
        let ok =
            unsafe { (self.sym_initialize)(self.process_handle(), PCSTR::null(), TRUE).as_bool() };
        if ok {
            self.apply_symbol_search_path();
            self.initialized.store(true, Ordering::Release);
        }
        ok
    }

    fn process_handle(&self) -> HANDLE {
        HANDLE(self.process as *mut c_void)
    }

    fn symbol_name(&self, addr: u64) -> Option<String> {
        #[repr(C)]
        struct SymbolInfoBuffer {
            symbol: SYMBOL_INFO,
            name: [i8; MAX_SYMBOL_NAME],
        }
        let symbol_info = SYMBOL_INFO {
            SizeOfStruct: std::mem::size_of::<SYMBOL_INFO>() as u32,
            MaxNameLen: MAX_SYMBOL_NAME as u32,
            ..SYMBOL_INFO::default()
        };
        let mut storage = SymbolInfoBuffer {
            symbol: symbol_info,
            name: [0; MAX_SYMBOL_NAME],
        };
        let symbol = &mut storage.symbol as *mut SYMBOL_INFO;
        let mut displacement = 0u64;
        let ok =
            unsafe { (self.sym_from_addr)(self.process_handle(), addr, &mut displacement, symbol) };
        if !ok.as_bool() {
            return None;
        }
        let len = unsafe { (*symbol).NameLen.min(MAX_SYMBOL_NAME as u32) as usize };
        let name = unsafe { std::slice::from_raw_parts((*symbol).Name.as_ptr().cast(), len) };
        Some(String::from_utf8_lossy(name).into_owned())
    }

    fn module_offset_name(&self, addr: u64) -> Option<String> {
        let mut module = IMAGEHLP_MODULE64 {
            SizeOfStruct: std::mem::size_of::<IMAGEHLP_MODULE64>() as u32,
            ..IMAGEHLP_MODULE64::default()
        };
        let ok = unsafe { (self.sym_get_module_info)(self.process_handle(), addr, &mut module) };
        if !ok.as_bool() {
            return None;
        }
        let name = fixed_c_string(&module.ModuleName)?;
        let offset = addr.saturating_sub(module.BaseOfImage);
        Some(format!("{name}!0x{offset:x}"))
    }

    fn line_name(&self, addr: u64) -> Option<String> {
        let mut line = IMAGEHLP_LINE64 {
            SizeOfStruct: std::mem::size_of::<IMAGEHLP_LINE64>() as u32,
            ..IMAGEHLP_LINE64::default()
        };
        let mut displacement = 0u32;
        let ok = unsafe {
            (self.sym_get_line_from_addr)(self.process_handle(), addr, &mut displacement, &mut line)
        };
        if !ok.as_bool() || line.FileName.is_null() {
            return None;
        }
        let file = unsafe { CStr::from_ptr(line.FileName.0.cast()) }
            .to_string_lossy()
            .into_owned();
        Some(format!("{file}:{}", line.LineNumber))
    }

    fn apply_symbol_search_path(&self) {
        let Some(search_path) = release_symbol_search_path() else {
            return;
        };
        unsafe {
            let _ =
                (self.sym_set_search_path_w)(self.process_handle(), PCWSTR(search_path.as_ptr()));
        }
    }
}

fn sanitize_value(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\t' | '\n' | '\r' | ' ' => '_',
            _ => ch,
        })
        .collect()
}

fn release_symbol_search_path() -> Option<Vec<u16>> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
    wide.push(0);
    Some(wide)
}

fn fixed_c_string(chars: &[i8]) -> Option<String> {
    let len = chars.iter().position(|ch| *ch == 0).unwrap_or(chars.len());
    if len == 0 {
        return None;
    }
    let bytes: Vec<u8> = chars[..len].iter().map(|ch| *ch as u8).collect();
    Some(String::from_utf8_lossy(&bytes).into_owned())
}
