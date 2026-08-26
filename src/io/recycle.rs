//! Safe-removal helpers via the Windows shell (PRD 6.7 清空存储卡).
//!
//! The 清空存储卡 feature moves successfully-imported source files on a memory
//! card to the recycle bin (NOT a permanent delete). This module wraps the
//! shell's `SHFileOperationW` so that the operation is undoable.

use std::path::PathBuf;

/// Move a list of files to the recycle bin (undoable delete). Returns an error
/// if the shell reports a failure. This is Windows-only.
#[cfg(windows)]
pub fn move_to_recycle_bin(paths: &[PathBuf]) -> anyhow::Result<()> {
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, SHFILEOPSTRUCTW, FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION,
        FOF_NOERRORUI, FOF_SILENT,
    };

    if paths.is_empty() {
        return Ok(());
    }

    // The pFrom field must be a double-null-terminated list of paths.
    let mut list: Vec<u16> = Vec::new();
    for p in paths {
        list.extend(p.to_string_lossy().into_owned().encode_utf16());
        list.push(0);
    }
    list.push(0); // trailing second null

    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = list.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_SILENT | FOF_NOCONFIRMATION | FOF_NOERRORUI) as u16;

    let ret = unsafe { SHFileOperationW(&mut op) };
    if ret == 0 {
        Ok(())
    } else {
        anyhow::bail!("移入回收站失败（错误码 {ret}）")
    }
}

/// Non-Windows fallback (the app is Windows-only; kept for API completeness).
#[cfg(not(windows))]
pub fn move_to_recycle_bin(_paths: &[PathBuf]) -> anyhow::Result<()> {
    anyhow::bail!("当前平台不支持移入回收站")
}
