//! File-open/save dialogs and `HDROP` path extraction.
//!
//! Dialogs run on the owning window's UI thread. File reads/writes still go
//! through the file-I/O worker after these helpers return selected paths.

use std::path::PathBuf;

use continuity_buffer::FileAssociation;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
    OPENFILENAMEW,
};
use windows::Win32::UI::Shell::{
    DragQueryFileW, FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST,
    FOS_PICKFOLDERS, HDROP, SIGDN_FILESYSPATH,
};

pub(crate) fn dropped_paths(hdrop: HDROP) -> Vec<PathBuf> {
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
    let mut paths = Vec::new();
    for idx in 0..count {
        let len = unsafe { DragQueryFileW(hdrop, idx, None) };
        if len == 0 {
            continue;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let written = unsafe { DragQueryFileW(hdrop, idx, Some(&mut buf)) };
        if written == 0 {
            continue;
        }
        paths.push(PathBuf::from(String::from_utf16_lossy(
            &buf[..written as usize],
        )));
    }
    paths
}

pub(crate) fn open_file_dialog(hwnd: HWND) -> Option<PathBuf> {
    let mut file = [0u16; 32768];
    let filter = wide_filter();
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    if unsafe { GetOpenFileNameW(&mut ofn).as_bool() } {
        path_from_wide_buf(&file)
    } else {
        None
    }
}

pub(crate) fn save_file_dialog(hwnd: HWND, current: Option<&FileAssociation>) -> Option<PathBuf> {
    let mut file = [0u16; 32768];
    if let Some(path) = current.map(|f| &f.path) {
        write_path_seed(&mut file, path);
    }
    let filter = wide_filter();
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        Flags: OFN_PATHMUSTEXIST | OFN_OVERWRITEPROMPT,
        ..Default::default()
    };
    if unsafe { GetSaveFileNameW(&mut ofn).as_bool() } {
        path_from_wide_buf(&file)
    } else {
        None
    }
}

pub(crate) fn open_folder_dialog(hwnd: HWND) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dialog
            .SetOptions(FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)
            .ok()?;
        dialog.Show(Some(hwnd)).ok()?;
        let item = dialog.GetResult().ok()?;
        let path = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let value = path.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(path.as_ptr() as *const core::ffi::c_void));
        value
    }
}

fn wide_filter() -> Vec<u16> {
    "Markdown and text\0*.md;*.markdown;*.txt\0All files\0*.*\0\0"
        .encode_utf16()
        .collect()
}

fn path_from_wide_buf(buf: &[u16]) -> Option<PathBuf> {
    let end = buf.iter().position(|&c| c == 0)?;
    if end == 0 {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..end])))
}

fn write_path_seed(buf: &mut [u16], path: &std::path::Path) {
    let s = path.to_string_lossy();
    for (idx, unit) in s
        .encode_utf16()
        .take(buf.len().saturating_sub(1))
        .enumerate()
    {
        buf[idx] = unit;
    }
}
