//! F-040 系统默认播放器入口。
//!
//! 核心只负责把文件交给操作系统的默认关联程序（Windows：`explorer`，等价于
//! ShellExecute 的“打开”动作；其它平台：`xdg-open`），不实现内置播放器、转码或
//! 批量视频打标。与 `kanalib-ui` 既有 `platform::{open_path, reveal_path}` 的
//! explorer 模式保持一致。

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::model::{is_media, is_video};

/// 用系统默认程序打开任意媒体文件。
///
/// 只做“启动默认程序”，不等待进程退出（explorer 即便成功也可能返回非零退出码）。
pub fn open_with_default_app(path: &Path) -> Result<()> {
    if !path.is_file() {
        bail!("文件不存在：{}", path.display());
    }
    let mut command = desktop_open_command(path);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console(&mut command);
    command
        .spawn()
        .with_context(|| format!("打开 {}", path.display()))?;
    Ok(())
}

/// 用系统默认播放器打开视频；拒绝非视频与不存在的路径。
pub fn open_video(path: &Path) -> Result<()> {
    if !is_video(path) {
        bail!("不是受支持的视频：{}", path.display());
    }
    open_with_default_app(path)
}

/// 仅判断是否能交给系统播放器（不做实际启动，供 UI 决定是否显示入口）。
pub fn can_open_externally(path: &Path) -> bool {
    is_media(path) && path.is_file()
}

#[cfg(windows)]
fn desktop_open_command(path: &Path) -> Command {
    let mut command = Command::new("explorer");
    command.arg(path);
    command
}

#[cfg(not(windows))]
fn desktop_open_command(path: &Path) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(path);
    command
}

fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // explorer 本身是 GUI 程序；这里只保证不弹出额外控制台窗口。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kanalib-open-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn open_video_rejects_missing_files_and_images_without_spawning() {
        let dir = temp_dir("validate");
        let missing = dir.join("missing.mp4");
        assert!(open_video(&missing).is_err());

        let image = dir.join("a.png");
        std::fs::write(&image, b"x").unwrap();
        assert!(open_video(&image).is_err());
        assert!(!can_open_externally(&missing));
        assert!(can_open_externally(&image), "图片仍可交给系统关联程序");

        let video = dir.join("a.mp4");
        std::fs::write(&video, b"x").unwrap();
        assert!(can_open_externally(&video));
        // 不在测试里真正启动播放器：只验证校验通过到 spawn 之前的分支。
        assert!(is_video(&video));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn desktop_open_command_targets_the_platform_handler() {
        // 只验证命令构造，不真正启动播放器。
        let path = PathBuf::from("C:\\图库 视频\\clip.mp4");
        let command = desktop_open_command(&path);
        #[cfg(windows)]
        assert_eq!(command.get_program(), "explorer");
        #[cfg(not(windows))]
        assert_eq!(command.get_program(), "xdg-open");
        let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
        assert_eq!(args, vec![path.as_os_str()]);
    }
}
