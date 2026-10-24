use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{bail, Result};

#[derive(Clone, Debug, Default)]
pub struct TrashReport {
    pub moved: Vec<PathBuf>,
    pub failed: Vec<PathBuf>,
    pub error: Option<String>,
}

#[cfg(target_os = "windows")]
pub fn move_to_recycle_bin(paths: &[PathBuf]) -> Result<TrashReport> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};

    const FO_DELETE: u32 = 3;
    const FOF_SILENT: u16 = 0x0004;
    const FOF_NOCONFIRMATION: u16 = 0x0010;
    const FOF_ALLOWUNDO: u16 = 0x0040;
    const FOF_NOERRORUI: u16 = 0x0400;
    const DRIVE_FIXED: u32 = 3;

    #[repr(C)]
    struct ShFileOpStructW {
        hwnd: *mut c_void,
        w_func: u32,
        p_from: *const u16,
        p_to: *const u16,
        f_flags: u16,
        f_any_operations_aborted: i32,
        h_name_mappings: *mut c_void,
        lpsz_progress_title: *const u16,
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHFileOperationW(operation: *mut ShFileOpStructW) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(root_path_name: *const u16) -> u32;
    }

    fn fixed_drive(path: &std::path::Path) -> bool {
        if !path.is_absolute() {
            return false;
        }
        let Some(prefix) = path.components().next() else {
            return false;
        };
        let root = format!("{}\\", prefix.as_os_str().to_string_lossy());
        let wide = root
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        unsafe { GetDriveTypeW(wide.as_ptr()) == DRIVE_FIXED }
    }

    let mut seen = HashSet::new();
    let mut existing = Vec::new();
    let mut missing = Vec::new();
    for path in paths {
        if !seen.insert(path.clone()) {
            continue;
        }
        if path.exists() {
            if !path.is_file() {
                bail!("拒绝处理非文件路径: {}", path.display());
            }
            if !fixed_drive(path) {
                bail!("仅支持本地固定磁盘的回收站: {}", path.display());
            }
            existing.push(path.clone());
        } else {
            missing.push(path.clone());
        }
    }

    if existing.is_empty() {
        return Ok(TrashReport {
            moved: missing,
            ..Default::default()
        });
    }

    let mut from = Vec::<u16>::new();
    for path in &existing {
        let wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if wide.contains(&0) {
            bail!("路径包含 NUL 字符: {}", path.display());
        }
        from.extend(wide);
        from.push(0);
    }
    from.push(0);

    let mut operation = ShFileOpStructW {
        hwnd: null_mut(),
        w_func: FO_DELETE,
        p_from: from.as_ptr(),
        p_to: null(),
        f_flags: FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI,
        f_any_operations_aborted: 0,
        h_name_mappings: null_mut(),
        lpsz_progress_title: null(),
    };
    let code = unsafe { SHFileOperationW(&mut operation) };
    let aborted = operation.f_any_operations_aborted != 0;

    let mut moved = missing;
    let mut failed = Vec::new();
    for path in existing {
        if path.exists() {
            failed.push(path);
        } else {
            moved.push(path);
        }
    }

    if moved.is_empty() && !failed.is_empty() {
        bail!(
            "移入回收站失败 (code 0x{code:04x}{})",
            if aborted { ", 已取消" } else { "" }
        );
    }

    let error = if code != 0 || aborted || !failed.is_empty() {
        Some(format!(
            "回收站操作未完全成功 (code 0x{code:04x}{}{})",
            if aborted { ", 已取消" } else { "" },
            if failed.is_empty() {
                String::new()
            } else {
                format!(", {} 个文件未移动", failed.len())
            }
        ))
    } else {
        None
    };

    Ok(TrashReport {
        moved,
        failed,
        error,
    })
}

#[cfg(not(target_os = "windows"))]
pub fn move_to_recycle_bin(_paths: &[PathBuf]) -> Result<TrashReport> {
    bail!("当前平台暂不支持系统回收站")
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::move_to_recycle_bin;

    #[test]
    fn missing_files_are_reported_for_library_cleanup() {
        let path = std::env::temp_dir().join(format!(
            "kanalib-trash-missing-{}.bin",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let report = move_to_recycle_bin(&[path.clone()]).unwrap();
        assert_eq!(report.moved, vec![path]);
        assert!(report.failed.is_empty());
    }

    #[test]
    #[ignore = "writes to the user's recycle bin"]
    fn moves_temp_file_to_recycle_bin() {
        let path = std::env::temp_dir().join(format!(
            "kanalib-trash-test-{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, b"kanalib").unwrap();
        let report = move_to_recycle_bin(&[path.clone()]).unwrap();
        assert!(!path.exists());
        assert!(report.moved.contains(&path));
        assert!(report.failed.is_empty());
    }
}
