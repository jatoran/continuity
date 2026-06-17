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

pub(crate) fn save_file_dialog(
    hwnd: HWND,
    current: Option<&FileAssociation>,
    default_title: &str,
) -> Option<PathBuf> {
    let mut file = [0u16; 32768];
    if let Some(path) = current.map(|f| &f.path) {
        write_path_seed(&mut file, path);
    } else {
        // Untitled buffer: seed the name box with the tab title (sanitized
        // to a legal filename stem, or "untitled"). The common dialog
        // selects the seeded name on open, so the user can immediately
        // type to replace it; `lpstrDefExt = "md"` appends the extension.
        let stem = sanitize_filename_stem(default_title);
        write_path_seed(&mut file, std::path::Path::new(&stem));
    }
    let filter = wide_save_filter();
    // `lpstrDefExt` makes the common dialog append this extension whenever
    // the user types a name without one — so a bare "notes" with the
    // Markdown type selected saves as "notes.md". An explicitly-typed
    // extension (e.g. "notes.txt") is respected and left untouched. Must
    // outlive the `GetSaveFileNameW` call below; kept on the stack here.
    let default_ext: Vec<u16> = "md\0".encode_utf16().collect();
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        // 1-based; the first filter is "Markdown (*.md, *.markdown)" so the
        // dialog opens defaulting to Markdown.
        nFilterIndex: 1,
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrDefExt: PCWSTR(default_ext.as_ptr()),
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

/// Save-dialog type filter. Markdown is listed first (and selected via
/// `nFilterIndex = 1`) so saving defaults to Markdown; paired with
/// `lpstrDefExt = "md"` an extensionless name is saved as `.md`. Text and
/// All-files remain available for explicit choices.
fn wide_save_filter() -> Vec<u16> {
    "Markdown (*.md, *.markdown)\0*.md;*.markdown\0Text (*.txt)\0*.txt\0All files (*.*)\0*.*\0\0"
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

/// Longest seeded default save name (in chars). Generous but bounded so a
/// pathological first-line title doesn't produce an unwieldy filename.
const MAX_DEFAULT_STEM_CHARS: usize = 48;

/// Turn a tab title into a legal Windows filename stem (no extension).
/// Strips the pin-dot prefix and trailing ellipsis the tab label may
/// carry, drops characters illegal in filenames, trims trailing dots and
/// spaces (which Windows forbids), caps the length, and falls back to
/// "untitled" when nothing usable remains.
fn sanitize_filename_stem(title: &str) -> String {
    let title = title
        .trim_start_matches('\u{25CF}')
        .trim()
        .trim_end_matches('\u{2026}')
        .trim();
    let mut out = String::new();
    let mut count = 0usize;
    for ch in title.chars() {
        if count >= MAX_DEFAULT_STEM_CHARS {
            break;
        }
        match ch {
            // Reserved in Windows file names → swap for a space.
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => out.push(' '),
            c if c.is_control() => continue,
            c => out.push(c),
        }
        count += 1;
    }
    let trimmed = out.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_whitespace_title_falls_back_to_untitled() {
        assert_eq!(sanitize_filename_stem(""), "untitled");
        assert_eq!(sanitize_filename_stem("   "), "untitled");
    }

    #[test]
    fn plain_title_is_carried_verbatim() {
        assert_eq!(sanitize_filename_stem("Meeting Notes"), "Meeting Notes");
    }

    #[test]
    fn strips_pin_dot_prefix_and_trailing_ellipsis() {
        assert_eq!(sanitize_filename_stem("\u{25CF} Pinned"), "Pinned");
        assert_eq!(sanitize_filename_stem("Long title\u{2026}"), "Long title");
    }

    #[test]
    fn replaces_reserved_filename_characters() {
        assert_eq!(
            sanitize_filename_stem("a/b:c*d?e\"f<g>h|i\\j"),
            "a b c d e f g h i j",
        );
    }

    #[test]
    fn trims_trailing_dots_and_spaces() {
        assert_eq!(sanitize_filename_stem("report..."), "report");
        assert_eq!(sanitize_filename_stem("  spaced  "), "spaced");
    }

    #[test]
    fn caps_length_at_max() {
        let long = "x".repeat(200);
        let stem = sanitize_filename_stem(&long);
        assert_eq!(stem.chars().count(), MAX_DEFAULT_STEM_CHARS);
    }

    #[test]
    fn reserved_only_title_falls_back_to_untitled() {
        // A title that is only reserved chars becomes spaces, which trim to
        // empty → "untitled".
        assert_eq!(sanitize_filename_stem("///"), "untitled");
    }
}
