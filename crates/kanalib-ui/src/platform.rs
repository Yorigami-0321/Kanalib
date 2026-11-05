use std::path::{Path, PathBuf};

use gpui::{App, Bounds, Pixels, Window, WindowBounds, point, px, size};

/// 窗口状态文件相对数据根的目录；与数据库/缩略图分开放，避免打开 DB 之前
/// 无法应用的顺序问题（F-038）。
pub const WINDOW_STATE_DIR: &str = ".kanalib";
pub const WINDOW_STATE_FILE: &str = "window_state.json";
/// 与 `WindowOptions::window_min_size` 保持一致；恢复窗口大小时也按它夹取。
pub const MIN_WINDOW_WIDTH: f32 = 900.;
pub const MIN_WINDOW_HEIGHT: f32 = 660.;
/// 位置越界需要重新夹取时，给标题栏预留的逻辑像素。
const TITLEBAR_MARGIN: f32 = 32.;
const MAX_COORDINATE: f32 = 1_000_000.;
const MAX_WINDOW_SIZE: f32 = 100_000.;

/// 一块可用工作区（逻辑像素）；`visible_bounds` 已排除任务栏。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkArea {
    pub display_id: u64,
    pub bounds: Bounds<Pixels>,
    pub primary: bool,
}

/// 持久化的窗口几何信息。坐标是相对虚拟桌面原点的逻辑像素，与
/// `Window::window_bounds()` 返回的客户端区域一致。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
    pub display_id: Option<u64>,
}

/// 已选中显示器并完成越界夹取的恢复结果。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedWindowState {
    pub state: WindowState,
    pub display_id: u64,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            x: 0.,
            y: 0.,
            width: 1280.,
            height: 820.,
            maximized: false,
            display_id: None,
        }
    }
}

pub fn window_state_path(root: &Path) -> PathBuf {
    root.join(WINDOW_STATE_DIR).join(WINDOW_STATE_FILE)
}

impl WindowState {
    pub fn from_bounds(bounds: Bounds<Pixels>, maximized: bool, display_id: Option<u64>) -> Self {
        Self {
            x: bounds.origin.x.as_f32(),
            y: bounds.origin.y.as_f32(),
            width: bounds.size.width.as_f32(),
            height: bounds.size.height.as_f32(),
            maximized,
            display_id,
        }
    }

    /// 从当前窗口抓取；最大化/全屏都保存还原后的普通窗口位置。
    pub fn capture(window: &Window, cx: &App) -> Self {
        let bounds = window.window_bounds();
        let maximized = matches!(bounds, WindowBounds::Maximized(_));
        Self::from_bounds(
            bounds.get_bounds(),
            maximized,
            window.display(cx).map(|display| u64::from(display.id())),
        )
    }

    /// 普通（非最大化）窗口边界；最大化时作为还原尺寸使用。
    pub fn to_bounds(&self) -> Bounds<Pixels> {
        Bounds::new(
            point(px(self.x), px(self.y)),
            size(px(self.width), px(self.height)),
        )
    }

    pub fn to_json(&self) -> String {
        serde_json::json!({
            "x": self.x,
            "y": self.y,
            "width": self.width,
            "height": self.height,
            "maximized": self.maximized,
            "display_id": self.display_id,
        })
        .to_string()
    }

    pub fn from_json(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let number = |name: &str| {
            value
                .get(name)
                .and_then(serde_json::Value::as_f64)
                .map(|number| number as f32)
        };
        let state = Self {
            x: number("x")?,
            y: number("y")?,
            width: number("width")?,
            height: number("height")?,
            maximized: value
                .get("maximized")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            display_id: value.get("display_id").and_then(serde_json::Value::as_u64),
        };
        state.is_sane().then_some(state)
    }

    fn is_sane(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width >= 1.
            && self.height >= 1.
            && self.width <= MAX_WINDOW_SIZE
            && self.height <= MAX_WINDOW_SIZE
            && self.x.abs() <= MAX_COORDINATE
            && self.y.abs() <= MAX_COORDINATE
    }

    /// 读取持久化状态；文件缺失/损坏/字段异常都返回 `None`，调用方回退居中逻辑。
    pub fn load(root: &Path) -> Option<Self> {
        let path = window_state_path(root);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                log::warn!("read window state {}: {error}", path.display());
                return None;
            }
        };
        match Self::from_json(&text) {
            Some(state) => Some(state),
            None => {
                log::warn!("ignore invalid window state {}", path.display());
                None
            }
        }
    }

    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let path = window_state_path(root);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, self.to_json())
    }

    /// 选择显示器并夹取到工作区：
    /// 1. 优先原 `display_id`；
    /// 2. 其次包含原窗口中心的显示器（显示器句柄跨重启可能变化）；
    /// 3. 再回退主显示器/首个显示器。
    ///
    /// 原位置完全可见时保持不动（避免每次启动都微移）；越界时整体夹回工作区，
    /// 顶部预留标题栏高度，保证窗口可再次被拖动。
    pub fn resolve(self, areas: &[WorkArea]) -> Option<ResolvedWindowState> {
        let target = self
            .display_id
            .and_then(|id| areas.iter().find(|area| area.display_id == id))
            .or_else(|| {
                let center_x = self.x + self.width / 2.;
                let center_y = self.y + self.height / 2.;
                areas.iter().find(|area| {
                    let origin = area.bounds.origin;
                    let end_x = origin.x.as_f32() + area.bounds.size.width.as_f32();
                    let end_y = origin.y.as_f32() + area.bounds.size.height.as_f32();
                    center_x >= origin.x.as_f32()
                        && center_x <= end_x
                        && center_y >= origin.y.as_f32()
                        && center_y <= end_y
                })
            })
            .or_else(|| areas.iter().find(|area| area.primary))
            .or_else(|| areas.first())?;

        let area = target.bounds;
        let area_x = area.origin.x.as_f32();
        let area_y = area.origin.y.as_f32();
        let area_width = area.size.width.as_f32().max(1.);
        let area_height = area.size.height.as_f32().max(1.);
        let area_right = area_x + area_width;
        let area_bottom = area_y + area_height;

        let mut state = self;
        let min_width = MIN_WINDOW_WIDTH.min(area_width);
        let min_height = MIN_WINDOW_HEIGHT.min(area_height);
        if !state.width.is_finite() {
            state.width = min_width;
        }
        if !state.height.is_finite() {
            state.height = min_height;
        }
        state.width = state.width.clamp(min_width, area_width);
        state.height = state.height.clamp(min_height, area_height);
        let x = if state.x.is_finite() { state.x } else { area_x };
        let y = if state.y.is_finite() { state.y } else { area_y };

        let fully_inside = x >= area_x
            && y >= area_y
            && x + state.width <= area_right
            && y + state.height <= area_bottom;
        if fully_inside {
            state.x = x;
            state.y = y;
        } else {
            let margin = TITLEBAR_MARGIN.min((area_height - state.height).max(0.));
            let min_x = area_x;
            let max_x = (area_right - state.width).max(min_x);
            let min_y = area_y + margin;
            let max_y = (area_bottom - state.height).max(min_y);
            state.x = x.clamp(min_x, max_x);
            state.y = y.clamp(min_y, max_y);
        }
        state.display_id = Some(target.display_id);
        Some(ResolvedWindowState {
            state,
            display_id: target.display_id,
        })
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn open_path(path: &PathBuf) {
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn open_path(path: &PathBuf) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

#[cfg(target_os = "windows")]
pub(crate) fn reveal_path(path: &PathBuf) {
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn reveal_path(path: &PathBuf) {
    if let Some(parent) = path.parent() {
        open_path(&parent.to_path_buf());
    }
}

// ---- F-047：Windows 回收站入口 ----

/// 单独抽出命令，便于单测确认入口而不真正启动资源管理器。
pub(crate) fn recycle_bin_invocation() -> (&'static str, &'static str) {
    ("explorer", "shell:RecycleBinFolder")
}

pub(crate) fn open_recycle_bin() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let (program, argument) = recycle_bin_invocation();
        std::process::Command::new(program)
            .arg(argument)
            .spawn()
            .map(|_| ())
            .map_err(|error| if crate::localization::is_english() { format!("Failed to launch Explorer: {error}") } else { format!("启动资源管理器失败：{error}") })
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(if crate::localization::is_english() { "No Windows Recycle Bin on this platform".to_string() } else { "当前平台没有 Windows 回收站".to_string() })
    }
}

// ---- F-068：复制图片到 Windows 剪贴板（CF_DIB + PNG） ----

/// 复制图片失败时回退为复制路径的提示文案。
pub(crate) fn image_copy_fallback_message(path: &Path, error: &str) -> String {
    format!(
        "复制图片失败（{error}），已改为复制路径：{}",
        path.display()
    )
}

/// 把自上而下的 RGBA 像素转换成 CF_DIB：40 字节 BITMAPINFOHEADER +
/// 自下而上的 32 位 BGRA 行。纯函数，便于在任意平台单测。
pub(crate) fn dib_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err(if crate::localization::is_english() { "Image dimensions are empty".to_string() } else { "图片尺寸为空".to_string() });
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| if crate::localization::is_english() { "Image dimensions are too large".to_string() } else { "图片尺寸过大".to_string() })?;
    if rgba.len() < expected {
        return Err(format!(
            "RGBA 数据不足：需要 {expected} 字节，实际 {}",
            rgba.len()
        ));
    }
    let header_size = 40usize;
    let mut out = vec![0u8; header_size + expected];
    out[0..4].copy_from_slice(&40u32.to_le_bytes());
    out[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    out[8..12].copy_from_slice(&(height as i32).to_le_bytes());
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&32u16.to_le_bytes());
    out[16..20].copy_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out[20..24].copy_from_slice(&(expected as u32).to_le_bytes());
    let row_bytes = width as usize * 4;
    for row in 0..height as usize {
        let src_row = (height as usize - 1 - row) * row_bytes;
        let dst_row = header_size + row * row_bytes;
        for column in 0..width as usize {
            let src = src_row + column * 4;
            let dst = dst_row + column * 4;
            out[dst] = rgba[src + 2];
            out[dst + 1] = rgba[src + 1];
            out[dst + 2] = rgba[src];
            out[dst + 3] = rgba[src + 3];
        }
    }
    Ok(out)
}

/// 解码图片并写入剪贴板；解码失败由调用方回退为复制路径。
pub(crate) fn copy_image_to_clipboard(path: &Path) -> Result<(), String> {
    if crate::format::is_remote_asset(path) {
        return Err(crate::format::remote_readonly_note().to_string());
    }
    let image = image::open(path).map_err(|error| if crate::localization::is_english() { format!("Cannot decode image: {error}") } else { format!("无法解码图片：{error}") })?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let dib = dib_from_rgba(width, height, rgba.as_raw())?;
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| if crate::localization::is_english() { format!("PNG encoding failed: {error}") } else { format!("PNG 编码失败：{error}") })?;
    #[cfg(target_os = "windows")]
    {
        windows_clipboard::write_image(&dib, &png)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (dib, png);
        Err(if crate::localization::is_english() { "Writing images to the clipboard is not supported on this platform".to_string() } else { "当前平台不支持写入图片剪贴板".to_string() })
    }
}

#[cfg(target_os = "windows")]
mod windows_clipboard {
    use std::ffi::c_void;
    use std::ptr::null_mut;
    use std::time::Duration;

    type Handle = *mut c_void;
    const CF_DIB: u32 = 8;
    const GMEM_MOVEABLE: u32 = 0x0002;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn OpenClipboard(hwnd: *mut c_void) -> i32;
        fn EmptyClipboard() -> i32;
        fn CloseClipboard() -> i32;
        fn SetClipboardData(format: u32, data: Handle) -> Handle;
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
        fn GlobalLock(handle: Handle) -> *mut c_void;
        fn GlobalUnlock(handle: Handle) -> i32;
        fn GlobalFree(handle: Handle) -> Handle;
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn alloc(bytes: &[u8]) -> Result<Handle, String> {
        if bytes.is_empty() {
            return Err(if crate::localization::is_english() { "Clipboard data is empty".to_string() } else { "剪贴板数据为空".to_string() });
        }
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) };
        if handle.is_null() {
            return Err(if crate::localization::is_english() { "Failed to allocate clipboard memory".to_string() } else { "分配剪贴板内存失败".to_string() });
        }
        let pointer = unsafe { GlobalLock(handle) };
        if pointer.is_null() {
            unsafe { GlobalFree(handle) };
            return Err(if crate::localization::is_english() { "Failed to lock clipboard memory".to_string() } else { "锁定剪贴板内存失败".to_string() });
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer as *mut u8, bytes.len());
            GlobalUnlock(handle);
        }
        Ok(handle)
    }

    pub(crate) fn write_image(dib: &[u8], png: &[u8]) -> Result<(), String> {
        let png_format = unsafe { RegisterClipboardFormatW(wide("PNG").as_ptr()) };
        if png_format == 0 {
            return Err(if crate::localization::is_english() { "Failed to register the PNG clipboard format".to_string() } else { "注册 PNG 剪贴板格式失败".to_string() });
        }
        // 剪贴板可能被其它程序短暂占用；最多重试 5 次（约 75ms）。
        let mut opened = false;
        for _ in 0..5 {
            if unsafe { OpenClipboard(null_mut()) } != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        if !opened {
            return Err(if crate::localization::is_english() { "The clipboard is in use by another application".to_string() } else { "剪贴板被其他程序占用".to_string() });
        }
        let result = (|| {
            if unsafe { EmptyClipboard() } == 0 {
                return Err(if crate::localization::is_english() { "Failed to empty the clipboard".to_string() } else { "清空剪贴板失败".to_string() });
            }
            for (format, bytes) in [(CF_DIB, dib), (png_format, png)] {
                let handle = alloc(bytes)?;
                if unsafe { SetClipboardData(format, handle) }.is_null() {
                    unsafe { GlobalFree(handle) };
                    return Err(if crate::localization::is_english() { "Failed to write to the clipboard".to_string() } else { "写入剪贴板失败".to_string() });
                }
            }
            Ok(())
        })();
        unsafe { CloseClipboard() };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(display_id: u64, x: f32, y: f32, width: f32, height: f32, primary: bool) -> WorkArea {
        WorkArea {
            display_id,
            bounds: Bounds::new(point(px(x), px(y)), size(px(width), px(height))),
            primary,
        }
    }

    fn state(x: f32, y: f32, width: f32, height: f32) -> WindowState {
        WindowState {
            x,
            y,
            width,
            height,
            maximized: false,
            display_id: None,
        }
    }

    #[test]
    fn window_state_json_round_trip() {
        let saved = WindowState {
            x: -1280.,
            y: 120.5,
            width: 1280.,
            height: 820.,
            maximized: true,
            display_id: Some(65_537),
        };
        let text = saved.to_json();
        assert_eq!(WindowState::from_json(&text), Some(saved));
    }

    #[test]
    fn window_state_json_rejects_broken_payloads() {
        assert_eq!(WindowState::from_json("not json"), None);
        assert_eq!(WindowState::from_json("{}"), None);
        assert_eq!(
            WindowState::from_json(r#"{"x":0,"y":0,"width":0,"height":0}"#),
            None
        );
        assert_eq!(
            WindowState::from_json(r#"{"x":0,"y":0,"width":1280,"height":820}"#),
            Some(state(0., 0., 1280., 820.))
        );
    }

    #[test]
    fn resolve_keeps_visible_window_untouched() {
        let areas = [area(1, 0., 0., 1920., 1040., true)];
        let saved = WindowState {
            display_id: Some(1),
            ..state(100., 80., 1280., 820.)
        };
        let resolved = saved.resolve(&areas).expect("display available");
        assert_eq!(resolved.state, saved);
        assert_eq!(resolved.display_id, 1);
    }

    #[test]
    fn resolve_clamps_offscreen_window_into_primary() {
        let areas = [area(1, 0., 0., 1920., 1040., true)];
        let saved = state(5000., -400., 1280., 820.);
        let resolved = saved.resolve(&areas).expect("display available");
        assert_eq!(resolved.state.x, 640.);
        assert!(resolved.state.y >= 0.);
        assert!(resolved.state.y + resolved.state.height <= 1040.);
        assert_eq!(resolved.state.display_id, Some(1));
    }

    #[test]
    fn resolve_prefers_saved_display_id() {
        let areas = [
            area(1, 0., 0., 1920., 1040., true),
            area(2, 1920., 0., 2560., 1400., false),
        ];
        let saved = WindowState {
            display_id: Some(2),
            ..state(2000., 100., 1280., 820.)
        };
        let resolved = saved.resolve(&areas).expect("display available");
        assert_eq!(resolved.display_id, 2);
        assert_eq!(resolved.state.x, 2000.);
    }

    #[test]
    fn resolve_falls_back_to_display_containing_window_center() {
        let areas = [
            area(1, 0., 0., 1920., 1040., true),
            area(2, 1920., 0., 2560., 1400., false),
        ];
        // display_id 失效（显示器句柄重启后变化），但中心点仍在副屏。
        let saved = WindowState {
            display_id: Some(99),
            ..state(2500., 200., 1200., 800.)
        };
        let resolved = saved.resolve(&areas).expect("display available");
        assert_eq!(resolved.display_id, 2);
        assert_eq!(resolved.state.x, 2500.);
    }

    #[test]
    fn resolve_unknown_display_falls_back_to_primary_and_clamps() {
        let areas = [
            area(1, 0., 0., 1920., 1040., true),
            area(2, 1920., 0., 2560., 1400., false),
        ];
        let saved = WindowState {
            display_id: Some(99),
            ..state(-3000., -500., 1280., 820.)
        };
        let resolved = saved.resolve(&areas).expect("display available");
        assert_eq!(resolved.display_id, 1);
        assert!(resolved.state.x >= 0.);
        assert!(resolved.state.y >= 0.);
        assert!(resolved.state.x + resolved.state.width <= 1920.);
        assert!(resolved.state.y + resolved.state.height <= 1040.);
    }

    #[test]
    fn resolve_handles_125_percent_dpi_logical_work_area() {
        // 1920x1080 @125% 对应约 1536x864 逻辑像素；窗口不得越界。
        let areas = [area(1, 0., 0., 1536., 824., true)];
        // 保存的窗口比 125% 逻辑工作区更高，恢复时必须压回工作区内。
        let saved = WindowState {
            display_id: Some(1),
            ..state(1400., 700., 1280., 900.)
        };
        let resolved = saved.resolve(&areas).expect("display available");
        assert_eq!(resolved.state.width, 1280.);
        assert_eq!(resolved.state.height, 824.);
        assert_eq!(resolved.state.x, 256.);
        assert_eq!(resolved.state.y, 0.);
        assert!(resolved.state.x + resolved.state.width <= 1536.);
        assert!(resolved.state.y + resolved.state.height <= 824.);
    }

    #[test]
    fn resolve_keeps_maximized_flag() {
        let areas = [area(1, 0., 0., 1920., 1040., true)];
        let saved = WindowState {
            maximized: true,
            ..state(100., 100., 1280., 820.)
        };
        assert!(saved.resolve(&areas).expect("display").state.maximized);
    }

    #[test]
    fn resolve_without_displays_is_none() {
        assert_eq!(state(0., 0., 1280., 820.).resolve(&[]), None);
    }

    #[test]
    fn recycle_bin_invocation_targets_shell_folder() {
        assert_eq!(
            recycle_bin_invocation(),
            ("explorer", "shell:RecycleBinFolder")
        );
    }

    #[test]
    fn dib_from_rgba_writes_header_and_bottom_up_bgra() {
        // 2x2：顶行红、绿；底行蓝、白。
        let rgba = [
            255, 0, 0, 255, // 红
            0, 255, 0, 255, // 绿
            0, 0, 255, 255, // 蓝
            255, 255, 255, 255, // 白
        ];
        let dib = dib_from_rgba(2, 2, &rgba).expect("dib");
        assert_eq!(dib.len(), 40 + 16);
        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40);
        assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 2);
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(dib[14..16].try_into().unwrap()), 32);
        // 第一行是原图最后一行：蓝 (BGR 255,0,0)、白。
        assert_eq!(&dib[40..44], &[255, 0, 0, 255]);
        assert_eq!(&dib[44..48], &[255, 255, 255, 255]);
        // 第二行是原图第一行：红 (BGR 0,0,255)、绿 (BGR 0,255,0)。
        assert_eq!(&dib[48..52], &[0, 0, 255, 255]);
        assert_eq!(&dib[52..56], &[0, 255, 0, 255]);
        assert!(dib_from_rgba(0, 1, &[]).is_err());
        assert!(dib_from_rgba(2, 2, &[0, 0, 0, 255]).is_err());
    }

    #[test]
    fn image_copy_fallback_message_includes_path_and_error() {
        let message = image_copy_fallback_message(Path::new(r"C:\pics\a.png"), "解码失败");
        assert!(message.contains("解码失败"), "{message}");
        assert!(message.contains(r"C:\pics\a.png"), "{message}");
        assert!(message.contains("复制路径"), "{message}");
    }
}
