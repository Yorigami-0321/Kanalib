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
            display_id: value
                .get("display_id")
                .and_then(serde_json::Value::as_u64),
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

        let fully_inside =
            x >= area_x && y >= area_y && x + state.width <= area_right && y + state.height <= area_bottom;
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
}
