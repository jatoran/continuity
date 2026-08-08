//! Exported C functions.

use std::ffi::c_void;

use continuity_engine_core::SelectionEdit;

use crate::error::{boundary, last_error, AbiError};
use crate::handle::{
    checked_handle, checked_handle_mut, notify_change, null_error, set_callback, Handle,
};
use crate::{
    ContinuityEngineCapabilities, ContinuityEngineChangeCallback, ContinuityEngineDelta,
    ContinuityEngineHandle, ContinuityEnginePosition, ContinuityEngineStatus,
    ContinuityEngineString, ContinuityEngineUtf16String, CONTINUITY_ENGINE_ABI_MAJOR,
    CONTINUITY_ENGINE_ABI_MINOR, CONTINUITY_ENGINE_CAP_BRANCHING_UNDO,
    CONTINUITY_ENGINE_CAP_CALLBACK, CONTINUITY_ENGINE_CAP_MULTI_CURSOR,
    CONTINUITY_ENGINE_CAP_UTF16, CONTINUITY_ENGINE_SDK_MAJOR, CONTINUITY_ENGINE_SDK_MINOR,
    CONTINUITY_ENGINE_SDK_PATCH,
};

/// Query the supported ABI version and capabilities.
///
/// # Safety
/// `out` must be writable for one capabilities value.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_capabilities(
    out: *mut ContinuityEngineCapabilities,
) -> ContinuityEngineStatus {
    boundary(|| {
        let out = unsafe { out.as_mut() }.ok_or_else(null_error)?;
        *out = ContinuityEngineCapabilities {
            struct_size: std::mem::size_of::<ContinuityEngineCapabilities>() as u32,
            abi_major: CONTINUITY_ENGINE_ABI_MAJOR,
            abi_minor: CONTINUITY_ENGINE_ABI_MINOR,
            sdk_major: CONTINUITY_ENGINE_SDK_MAJOR,
            sdk_minor: CONTINUITY_ENGINE_SDK_MINOR,
            sdk_patch: CONTINUITY_ENGINE_SDK_PATCH,
            reserved: 0,
            flags: CONTINUITY_ENGINE_CAP_UTF16
                | CONTINUITY_ENGINE_CAP_CALLBACK
                | CONTINUITY_ENGINE_CAP_MULTI_CURSOR
                | CONTINUITY_ENGINE_CAP_BRANCHING_UNDO,
        };
        Ok(())
    })
}

/// Create a one-document handle from UTF-8 text and a host-owned revision.
///
/// # Safety
/// `text` must cover `text_len` bytes and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_create_utf8(
    requested_abi_major: u16,
    text: *const u8,
    text_len: usize,
    revision: u64,
    out: *mut *mut ContinuityEngineHandle,
) -> ContinuityEngineStatus {
    boundary(|| {
        validate_abi(requested_abi_major)?;
        let text = unsafe { read_utf8(text, text_len) }?;
        let out = unsafe { out.as_mut() }.ok_or_else(null_error)?;
        let handle = ContinuityEngineHandle(Handle::new(text, revision));
        *out = Box::into_raw(Box::new(handle));
        Ok(())
    })
}

/// Create a one-document handle from UTF-16 text and a host-owned revision.
///
/// # Safety
/// `text` must cover `text_len` code units and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_create_utf16(
    requested_abi_major: u16,
    text: *const u16,
    text_len: usize,
    revision: u64,
    out: *mut *mut ContinuityEngineHandle,
) -> ContinuityEngineStatus {
    boundary(|| {
        validate_abi(requested_abi_major)?;
        let text = unsafe { read_utf16(text, text_len) }?;
        let out = unsafe { out.as_mut() }.ok_or_else(null_error)?;
        let handle = ContinuityEngineHandle(Handle::new(&text, revision));
        *out = Box::into_raw(Box::new(handle));
        Ok(())
    })
}

/// Destroy a handle. It must be called exactly once on the creating thread.
///
/// # Safety
/// `handle` must be a live handle returned by this library.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_destroy(
    handle: *mut ContinuityEngineHandle,
) -> ContinuityEngineStatus {
    boundary(|| {
        unsafe { checked_handle_mut(handle) }?;
        drop(unsafe { Box::from_raw(handle) });
        Ok(())
    })
}

/// Register or clear the post-mutation change callback.
///
/// # Safety
/// `handle` must be live; `user_data` must remain valid while the callback is registered.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_set_change_callback(
    handle: *mut ContinuityEngineHandle,
    callback: ContinuityEngineChangeCallback,
    user_data: *mut c_void,
) -> ContinuityEngineStatus {
    boundary(|| {
        let handle = unsafe { checked_handle_mut(handle) }?;
        set_callback(handle, callback, user_data);
        Ok(())
    })
}

/// Replace the current selections with UTF-8 source carets.
///
/// # Safety
/// `handle` must be live and `carets` must cover `caret_count` values.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_set_carets(
    handle: *mut ContinuityEngineHandle,
    carets: *const ContinuityEnginePosition,
    caret_count: usize,
) -> ContinuityEngineStatus {
    boundary(|| {
        let carets = unsafe { read_slice(carets, caret_count) }?;
        let handle = unsafe { checked_handle_mut(handle) }?;
        handle.set_carets(carets)
    })
}

/// Insert UTF-8 text at every selection.
///
/// # Safety
/// `handle` must be live and `text` must cover `text_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_insert_utf8(
    handle: *mut ContinuityEngineHandle,
    text: *const u8,
    text_len: usize,
    timestamp_ms: i64,
) -> ContinuityEngineStatus {
    boundary(|| {
        let text = unsafe { read_utf8(text, text_len) }?.to_owned();
        let revision = unsafe { checked_handle_mut(handle) }?
            .apply_selection_edit(SelectionEdit::InsertText(text), timestamp_ms)?;
        unsafe { notify_change(handle, revision) };
        Ok(())
    })
}

/// Insert UTF-16 text at every selection.
///
/// # Safety
/// `handle` must be live and `text` must cover `text_len` code units.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_insert_utf16(
    handle: *mut ContinuityEngineHandle,
    text: *const u16,
    text_len: usize,
    timestamp_ms: i64,
) -> ContinuityEngineStatus {
    boundary(|| {
        let text = unsafe { read_utf16(text, text_len) }?;
        let revision = unsafe { checked_handle_mut(handle) }?
            .apply_selection_edit(SelectionEdit::InsertText(text), timestamp_ms)?;
        unsafe { notify_change(handle, revision) };
        Ok(())
    })
}

/// Delete backward at every selection.
///
/// # Safety
/// `handle` must be a live handle returned by this library.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_delete_backward(
    handle: *mut ContinuityEngineHandle,
    timestamp_ms: i64,
) -> ContinuityEngineStatus {
    apply_simple(handle, SelectionEdit::DeleteBack, timestamp_ms)
}

/// Undo the current edit group.
///
/// # Safety
/// `handle` must be a live handle returned by this library.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_undo(
    handle: *mut ContinuityEngineHandle,
    timestamp_ms: i64,
) -> ContinuityEngineStatus {
    boundary(|| {
        let revision = unsafe { checked_handle_mut(handle) }?.undo(timestamp_ms)?;
        unsafe { notify_change(handle, revision) };
        Ok(())
    })
}

/// Redo the preferred child edit group.
///
/// # Safety
/// `handle` must be a live handle returned by this library.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_redo(
    handle: *mut ContinuityEngineHandle,
    timestamp_ms: i64,
) -> ContinuityEngineStatus {
    apply_redo(handle, timestamp_ms, false)
}

/// Redo an alternate child edit group.
///
/// # Safety
/// `handle` must be a live handle returned by this library.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_redo_alternate(
    handle: *mut ContinuityEngineHandle,
    timestamp_ms: i64,
) -> ContinuityEngineStatus {
    apply_redo(handle, timestamp_ms, true)
}

/// Copy the current document as a Rust-allocated UTF-8 buffer.
///
/// # Safety
/// `handle` must be live and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_snapshot_utf8(
    handle: *const ContinuityEngineHandle,
    out: *mut ContinuityEngineString,
) -> ContinuityEngineStatus {
    boundary(|| {
        let text = unsafe { checked_handle(handle) }?.text()?;
        let out = unsafe { out.as_mut() }.ok_or_else(null_error)?;
        *out = allocate_bytes(text.into_bytes());
        Ok(())
    })
}

/// Copy the current document as a Rust-allocated UTF-16 buffer.
///
/// # Safety
/// `handle` must be live and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_snapshot_utf16(
    handle: *const ContinuityEngineHandle,
    out: *mut ContinuityEngineUtf16String,
) -> ContinuityEngineStatus {
    boundary(|| {
        let text = unsafe { checked_handle(handle) }?.text()?;
        let out = unsafe { out.as_mut() }.ok_or_else(null_error)?;
        *out = allocate_utf16(text.encode_utf16().collect());
        Ok(())
    })
}

/// Return the current revision.
///
/// # Safety
/// `handle` must be live and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_revision(
    handle: *const ContinuityEngineHandle,
    out: *mut u64,
) -> ContinuityEngineStatus {
    boundary(|| {
        let revision = unsafe { checked_handle(handle) }?.revision()?;
        *unsafe { out.as_mut() }.ok_or_else(null_error)? = revision;
        Ok(())
    })
}

/// Copy current caret heads into a Rust-allocated array.
///
/// # Safety
/// `handle` must be live and both output pointers must be writable.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_carets(
    handle: *const ContinuityEngineHandle,
    out_data: *mut *mut ContinuityEnginePosition,
    out_len: *mut usize,
) -> ContinuityEngineStatus {
    boundary(|| {
        let values = unsafe { checked_handle(handle) }?.carets()?;
        unsafe { write_array(values, out_data, out_len) }
    })
}

/// Copy edit deltas newer than the supplied revision into a Rust-allocated array.
///
/// # Safety
/// `handle` must be live and both output pointers must be writable.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_deltas_since(
    handle: *const ContinuityEngineHandle,
    since_revision: u64,
    out_data: *mut *mut ContinuityEngineDelta,
    out_len: *mut usize,
) -> ContinuityEngineStatus {
    boundary(|| {
        let values = unsafe { checked_handle(handle) }?.deltas(since_revision);
        unsafe { write_array(values, out_data, out_len) }
    })
}

/// Free a UTF-8 buffer returned by this library.
///
/// # Safety
/// `value` must be an unfreed value returned by `continuity_engine_snapshot_utf8`.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_string_free(
    value: ContinuityEngineString,
) -> ContinuityEngineStatus {
    boundary(|| unsafe { free_array(value.data, value.len) })
}

/// Free a UTF-16 buffer returned by this library.
///
/// # Safety
/// `value` must be an unfreed value returned by `continuity_engine_snapshot_utf16`.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_utf16_string_free(
    value: ContinuityEngineUtf16String,
) -> ContinuityEngineStatus {
    boundary(|| unsafe { free_array(value.data, value.len) })
}

/// Free a caret array returned by this library.
///
/// # Safety
/// `data` and `len` must be an unfreed pair returned by `continuity_engine_carets`.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_carets_free(
    data: *mut ContinuityEnginePosition,
    len: usize,
) -> ContinuityEngineStatus {
    boundary(|| unsafe { free_array(data, len) })
}

/// Free a delta array returned by this library.
///
/// # Safety
/// `data` and `len` must be an unfreed pair returned by `continuity_engine_deltas_since`.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_deltas_free(
    data: *mut ContinuityEngineDelta,
    len: usize,
) -> ContinuityEngineStatus {
    boundary(|| unsafe { free_array(data, len) })
}

/// Copy the current thread's last error as UTF-8. No handle is required.
///
/// # Safety
/// `out_required` must be writable and `buffer` must cover `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn continuity_engine_last_error_utf8(
    buffer: *mut u8,
    capacity: usize,
    out_required: *mut usize,
) -> ContinuityEngineStatus {
    let message = last_error();
    let required = message.len();
    if let Some(out_required) = unsafe { out_required.as_mut() } {
        *out_required = required;
    } else {
        return ContinuityEngineStatus::NullPointer;
    }
    if capacity < required {
        return ContinuityEngineStatus::NullPointer;
    }
    if required > 0 {
        let Some(buffer) = (unsafe { buffer.as_mut() }) else {
            return ContinuityEngineStatus::NullPointer;
        };
        unsafe { std::ptr::copy_nonoverlapping(message.as_ptr(), buffer, required) };
    }
    ContinuityEngineStatus::Ok
}

unsafe fn apply_simple(
    handle: *mut ContinuityEngineHandle,
    edit: SelectionEdit,
    timestamp_ms: i64,
) -> ContinuityEngineStatus {
    boundary(|| {
        let revision =
            unsafe { checked_handle_mut(handle) }?.apply_selection_edit(edit, timestamp_ms)?;
        unsafe { notify_change(handle, revision) };
        Ok(())
    })
}

unsafe fn apply_redo(
    handle: *mut ContinuityEngineHandle,
    timestamp_ms: i64,
    is_alternate: bool,
) -> ContinuityEngineStatus {
    boundary(|| {
        let revision = unsafe { checked_handle_mut(handle) }?.redo(timestamp_ms, is_alternate)?;
        unsafe { notify_change(handle, revision) };
        Ok(())
    })
}

fn validate_abi(requested: u16) -> Result<(), AbiError> {
    if requested == CONTINUITY_ENGINE_ABI_MAJOR {
        Ok(())
    } else {
        Err(AbiError::new(
            ContinuityEngineStatus::UnsupportedAbi,
            format!("unsupported ABI major {requested}"),
        ))
    }
}

unsafe fn read_utf8<'a>(data: *const u8, len: usize) -> Result<&'a str, AbiError> {
    let bytes = unsafe { read_slice(data, len) }?;
    std::str::from_utf8(bytes).map_err(|error| {
        AbiError::new(
            ContinuityEngineStatus::InvalidUtf8,
            format!("invalid UTF-8 input: {error}"),
        )
    })
}

unsafe fn read_utf16(data: *const u16, len: usize) -> Result<String, AbiError> {
    let units = unsafe { read_slice(data, len) }?;
    String::from_utf16(units).map_err(|error| {
        AbiError::new(
            ContinuityEngineStatus::InvalidUtf16,
            format!("invalid UTF-16 input: {error}"),
        )
    })
}

unsafe fn read_slice<'a, T>(data: *const T, len: usize) -> Result<&'a [T], AbiError> {
    if len == 0 {
        return Ok(&[]);
    }
    if data.is_null() {
        return Err(null_error());
    }
    Ok(unsafe { std::slice::from_raw_parts(data, len) })
}

fn allocate_bytes(values: Vec<u8>) -> ContinuityEngineString {
    let (data, len) = leak_array(values);
    ContinuityEngineString { data, len }
}

fn allocate_utf16(values: Vec<u16>) -> ContinuityEngineUtf16String {
    let (data, len) = leak_array(values);
    ContinuityEngineUtf16String { data, len }
}

fn leak_array<T>(values: Vec<T>) -> (*mut T, usize) {
    if values.is_empty() {
        return (std::ptr::null_mut(), 0);
    }
    let mut values = values.into_boxed_slice();
    let result = (values.as_mut_ptr(), values.len());
    std::mem::forget(values);
    result
}

unsafe fn write_array<T>(
    values: Vec<T>,
    out_data: *mut *mut T,
    out_len: *mut usize,
) -> Result<(), AbiError> {
    let out_data = unsafe { out_data.as_mut() }.ok_or_else(null_error)?;
    let out_len = unsafe { out_len.as_mut() }.ok_or_else(null_error)?;
    let (data, len) = leak_array(values);
    *out_data = data;
    *out_len = len;
    Ok(())
}

unsafe fn free_array<T>(data: *mut T, len: usize) -> Result<(), AbiError> {
    if len == 0 {
        return Ok(());
    }
    if data.is_null() {
        return Err(null_error());
    }
    let slice = std::ptr::slice_from_raw_parts_mut(data, len);
    drop(unsafe { Box::from_raw(slice) });
    Ok(())
}
