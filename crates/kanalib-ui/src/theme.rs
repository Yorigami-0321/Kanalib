use crate::*;

pub(crate) const TITLEBAR_HEIGHT: f32 = 48.;
pub(crate) const STATUS_BAR_HEIGHT: f32 = 26.;
pub(crate) const ACTIVITY_BAR_WIDTH: f32 = 48.;
pub(crate) const ACTIVITY_ITEM_HEIGHT: f32 = 40.;
pub(crate) const PAGE_HEADER_HEIGHT: f32 = 58.;
pub(crate) const VIEWER_HEADER_HEIGHT: f32 = 48.;

// F-073：Kanalib 壳层 token。Ely 的 Theme 已覆盖颜色、控件/图标/文本尺寸；
// 这里只补间距、行高与圆角，组件不再各写一套字面量。
pub(crate) const SPACE_HALF: f32 = 2.;
pub(crate) const SPACE_1: f32 = 4.;
pub(crate) const SPACE_1_5: f32 = 6.;
pub(crate) const SPACE_2: f32 = 8.;
pub(crate) const SPACE_3: f32 = 12.;
pub(crate) const SPACE_4: f32 = 16.;
pub(crate) const SPACE_6: f32 = 24.;

pub(crate) const RADIUS_SM: f32 = 4.;
pub(crate) const RADIUS_MD: f32 = 6.;
pub(crate) const RADIUS_LG: f32 = 8.;

pub(crate) const ROW_SM: f32 = 28.;
pub(crate) const ROW_MD: f32 = 36.;

#[derive(Clone, Copy)]
pub(crate) struct UiColors {
    pub(crate) bg: Hsla,
    pub(crate) surface: Hsla,
    pub(crate) sunken: Hsla,
    pub(crate) border: Hsla,
    pub(crate) fg: Hsla,
    pub(crate) muted: Hsla,
    pub(crate) subtle: Hsla,
    pub(crate) accent: Hsla,
    pub(crate) selection: Hsla,
    pub(crate) danger: Hsla,
    pub(crate) hover: Hsla,
    pub(crate) success: Hsla,
    pub(crate) success_subtle: Hsla,
    pub(crate) warning: Hsla,
    pub(crate) warning_subtle: Hsla,
    pub(crate) backdrop: Hsla,
}

impl UiColors {
    pub(crate) fn from_theme(cx: &App) -> Self {
        let colors = &cx.theme().colors;
        Self {
            bg: colors.bg,
            surface: colors.surface,
            sunken: colors.sunken,
            border: colors.border,
            fg: colors.fg,
            muted: colors.fg_muted,
            subtle: colors.fg_subtle,
            accent: colors.accent,
            selection: colors.selection,
            danger: colors.danger,
            hover: colors.hover,
            success: colors.success,
            success_subtle: colors.success_subtle,
            warning: colors.warning,
            warning_subtle: colors.warning_subtle,
            backdrop: colors.backdrop,
        }
    }
}

#[derive(Clone, Copy)]
enum WindowButtonAction {
    Minimize,
    Maximize,
    Close,
}

pub(crate) fn window_controls(window: &Window, colors: UiColors) -> impl IntoElement {
    let maximized = window.is_maximized();
    div()
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .child(window_control_button(
            "window-min",
            IconName::Minus,
            WindowButtonAction::Minimize,
            false,
            colors,
        ))
        .child(window_control_button(
            "window-max",
            if maximized {
                IconName::Copy
            } else {
                IconName::Square
            },
            WindowButtonAction::Maximize,
            false,
            colors,
        ))
        .child(window_control_button(
            "window-close",
            IconName::X,
            WindowButtonAction::Close,
            true,
            colors,
        ))
}

fn window_control_button(
    id: &'static str,
    icon: IconName,
    action: WindowButtonAction,
    close: bool,
    colors: UiColors,
) -> impl IntoElement {
    let group = SharedString::from(format!("{id}-hover"));
    let label = match action {
        WindowButtonAction::Minimize => {
            if crate::localization::is_english() { "Minimize" } else { "最小化" }
        }
        WindowButtonAction::Maximize => {
            if crate::localization::is_english() { "Maximize / restore" } else { "最大化/还原" }
        }
        WindowButtonAction::Close => {
            if crate::localization::is_english() { "Close" } else { "关闭" }
        }
    };
    div()
        .id(id)
        .group(group.clone())
        .tab_index(0)
        .focus(move |style| style.bg(colors.hover))
        .role(Role::Button)
        .aria_label(SharedString::from(label))
        // F-073 6.3：窗口控制是纯图标控件，补悬停提示；aria 名称已在上面。
        .tooltip(Tooltip::text(label))
        .flex_none()
        .h_full()
        .w(px(46.))
        .flex()
        .items_center()
        .justify_center()
        .hover(move |style| style.bg(if close { colors.danger } else { colors.hover }))
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_click(move |_, window, _| match action {
            WindowButtonAction::Minimize => window.minimize_window(),
            WindowButtonAction::Maximize => window.zoom_window(),
            WindowButtonAction::Close => window.remove_window(),
        })
        .child(
            Icon::new(icon)
                .size(IconSize::Sm)
                .color(colors.muted)
                .group_hover_color(group, if close { white() } else { colors.fg }),
        )
}
