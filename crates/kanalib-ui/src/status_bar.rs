use crate::*;
use crate::app::KanalibApp;
use crate::theme::*;

pub(crate) fn status_bar_item(
    id: &'static str,
    label: SharedString,
    color: Hsla,
    colors: UiColors,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .h_full()
        .px(px(6.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .cursor_pointer()
        .hover(move |item| item.bg(colors.hover))
        .when(active, |item| item.bg(colors.selection))
        .on_click(on_click)
        .child(
            div()
                .truncate()
                .whitespace_nowrap()
                .text_xs()
                .text_color(color)
                .child(label),
        )
        .into_any_element()
}

impl KanalibApp {
    pub(crate) fn render_status_bar(
        &self,
        this: &Entity<Self>,
        status: &str,
        error: Option<&str>,
        scanning: bool,
        device: Option<&str>,
        queue_pending: usize,
        queue_running: usize,
        queue_error: usize,
        counts: &Counts,
        filter: &Filter,
        width: Pixels,
        colors: UiColors,
    ) -> AnyElement {
        let compact = width < px(1100.);
        let library = self.screen == Screen::Library;
        let mut left = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .items_center()
            .gap(px(2.));
        if let Some(message) = error {
            let this_error = this.clone();
            left = left.child(status_bar_item(
                "status-error",
                SharedString::from(format!(
                    "错误 · {}",
                    message.lines().next().unwrap_or_default()
                )),
                colors.danger,
                colors,
                false,
                move |_, _, cx| {
                    let _ = this_error.update(cx, |app, cx| app.open_settings(cx));
                },
            ));
        } else if scanning {
            let this_stop = this.clone();
            left = left.child(status_bar_item(
                "status-scanning",
                SharedString::from("扫描中 · 点击停止"),
                colors.accent,
                colors,
                false,
                move |_, _, cx| {
                    let _ = this_stop.update(cx, |app, cx| {
                        app.state.stop();
                        cx.notify();
                    });
                },
            ));
        } else {
            left = left.child(
                div()
                    .max_w(px(if compact { 150. } else { 260. }))
                    .truncate()
                    .px(px(6.))
                    .text_xs()
                    .text_color(colors.muted)
                    .child(status.to_string()),
            );
        }
        if self.search_running {
            left = left.child(
                div()
                    .px(px(6.))
                    .text_xs()
                    .text_color(colors.accent)
                    .child("搜索中…"),
            );
        }
        let this_all = this.clone();
        left = left.child(status_bar_item(
            "status-total",
            SharedString::from(format!("{} 张", counts.total)),
            colors.fg,
            colors,
            library && *filter == Filter::All,
            move |_, _, cx| {
                let _ = this_all.update(cx, |app, cx| app.set_filter(Filter::All, cx));
            },
        ));
        left = left
            .child(
                div()
                    .px(px(6.))
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(format!("已打标 {}", counts.tagged)),
            )
            .child({
                let this_untagged = this.clone();
                status_bar_item(
                    "status-untagged",
                    SharedString::from(format!("未打标 {}", counts.untagged)),
                    colors.muted,
                    colors,
                    library && *filter == Filter::Untagged,
                    move |_, _, cx| {
                        let _ = this_untagged
                            .update(cx, |app, cx| app.set_filter(Filter::Untagged, cx));
                    },
                )
            });
        if counts.failed > 0 {
            let this_failed = this.clone();
            left = left.child(status_bar_item(
                "status-failed",
                SharedString::from(format!("失败 {}", counts.failed)),
                colors.danger,
                colors,
                library && *filter == Filter::Failed,
                move |_, _, cx| {
                    let _ = this_failed.update(cx, |app, cx| app.set_filter(Filter::Failed, cx));
                },
            ));
        }
        if counts.damaged > 0 {
            let this_damaged = this.clone();
            left = left.child(status_bar_item(
                "status-damaged",
                SharedString::from(format!("损坏 {}", counts.damaged)),
                colors.warning,
                colors,
                library && *filter == Filter::Damaged,
                move |_, _, cx| {
                    let _ = this_damaged
                        .update(cx, |app, cx| app.set_filter(Filter::Damaged, cx));
                },
            ));
        }

        let mut right = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.));
        if !compact {
            let this_queue = this.clone();
            let queue_label = if queue_pending + queue_running + queue_error == 0 {
                "队列空闲".to_string()
            } else {
                format!(
                    "队列 等待 {queue_pending} · 运行 {queue_running} · 失败 {queue_error}"
                )
            };
            right = right.child(status_bar_item(
                "status-queue",
                SharedString::from(queue_label),
                if queue_error > 0 {
                    colors.warning
                } else {
                    colors.muted
                },
                colors,
                false,
                move |_, _, cx| {
                    let _ = this_queue.update(cx, |app, cx| app.open_settings(cx));
                },
            ));
            let this_device = this.clone();
            right = right.child(status_bar_item(
                "status-device",
                SharedString::from(
                    device
                        .map(str::to_string)
                        .unwrap_or_else(|| "推理设备 · 未加载".to_string()),
                ),
                colors.subtle,
                colors,
                false,
                move |_, _, cx| {
                    let _ = this_device.update(cx, |app, cx| app.open_settings(cx));
                },
            ));
        }
        let this_palette = this.clone();
        let palette = div()
            .id("status-palette")
            .role(Role::Button)
            .aria_label("命令面板 (Ctrl+K)")
            .h_full()
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(4.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(move |item| item.bg(colors.hover))
            .on_click(move |_, _, cx| {
                let _ = this_palette.update(cx, |app, cx| {
                    app.palette_open = true;
                    cx.notify();
                });
            })
            .child(
                Icon::new(IconName::Command)
                    .size(IconSize::Xs)
                    .color(colors.muted),
            )
            .children((!compact).then(|| {
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("命令面板")
            }))
            .children((!compact).then(|| {
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("Ctrl+K")
            }));
        right = right.child(palette);

        div()
            .flex_none()
            .w_full()
            .h(px(STATUS_BAR_HEIGHT))
            .px(px(8.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .bg(colors.surface)
            .border_t_1()
            .border_color(colors.border)
            .text_xs()
            .child(left)
            .child(right)
            .into_any_element()
    }

}
