use crate::*;
use crate::app::KanalibApp;
use crate::settings::format_tag_eta;
use crate::sidebar::source_label;
use crate::theme::*;
use kanalib_core::model::{ScanPhase, ScanProgress, ScanSummary};

/// 扫描阶段的中文标签；底栏与设置页共用。
pub(crate) fn scan_phase_label(phase: ScanPhase) -> &'static str {
    match phase {
        ScanPhase::Idle => "空闲",
        ScanPhase::Discovering => "遍历",
        ScanPhase::Hashing => "哈希",
        ScanPhase::Thumbnails => "缩略图",
        ScanPhase::Tagging => "打标",
        ScanPhase::Reconciling => "对账",
        ScanPhase::Done => "完成",
        ScanPhase::Failed => "失败",
    }
}

/// 当前扫描的快照地址（阶段 done/total）。
pub(crate) fn scan_progress_counts(progress: &ScanProgress) -> String {
    let phase = scan_phase_label(progress.phase);
    if progress.total > 0 {
        format!("{phase} {}/{}", progress.done, progress.total)
    } else {
        phase.to_string()
    }
}

/// 扫描结束摘要的一行文本；`compact` 时只保留新增数，避免底栏被挤满。
pub(crate) fn format_scan_summary(summary: &ScanSummary, compact: bool) -> String {
    if compact {
        return format!("上次扫描 · 新增 {}", summary.imported);
    }
    let cancelled = if summary.cancelled { " · 已停止" } else { "" };
    format!(
        "上次扫描 · 新增 {} · 更新 {} · 跳过 {} · 移除 {} · 失败 {} · {:.1}s{}",
        summary.imported,
        summary.updated,
        summary.skipped,
        summary.removed,
        summary.failed,
        summary.elapsed_secs,
        cancelled
    )
}

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
        .min_w(px(0.))
        .overflow_hidden()
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
        // 有错误时错误行会占掉大半宽度，其余项一律走紧凑写法，避免互相挤压。
        let compact = width < px(1100.) || error.is_some();
        let library = self.screen == Screen::Library;
        // 扫描进度是核心线写入的只读快照；底栏只读不写，节流由核心负责。
        let (progress, summary, source_aliases) = {
            let shared = self.state.shared.lock().unwrap();
            (
                shared.scan_progress.clone(),
                shared.scan_summary.clone(),
                shared.source_aliases.clone(),
            )
        };
        let mut left = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .items_center()
            .gap(px(2.));
        if let Some(message) = error {
            let this_error = this.clone();
            let first = message.lines().next().unwrap_or_default();
            // 底栏只有一行：错误首行截到 56 字，完整信息在设置页诊断区。
            let mut text = first.chars().take(56).collect::<String>();
            if first.chars().count() > 56 {
                text.push('…');
            }
            left = left.child(status_bar_item(
                "status-error",
                SharedString::from(format!("错误 · {text}")),
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
            // F-063：阶段、done/total、吞吐、ETA 与当前来源都来自结构化进度快照。
            left = left.child(
                div()
                    .px(px(6.))
                    .text_xs()
                    .text_color(colors.accent)
                    .child(SharedString::from(scan_progress_counts(&progress))),
            );
            if progress.throughput > 0.0 && !compact {
                left = left.child(
                    div()
                        .px(px(6.))
                        .text_xs()
                        .text_color(colors.muted)
                        .child(SharedString::from(format!(
                            "{:.1} 张/秒",
                            progress.throughput
                        ))),
                );
            }
            if let Some(eta) = progress.eta_secs {
                left = left.child(
                    div()
                        .px(px(6.))
                        .text_xs()
                        .text_color(colors.muted)
                        .child(SharedString::from(format!(
                            "剩余 {}",
                            format_tag_eta(Some(eta))
                        ))),
                );
            }
            if let Some(source) = &progress.source {
                let label = if progress.source_total > 1 {
                    format!(
                        "来源 {}/{} · {}",
                        progress.source_index,
                        progress.source_total,
                        source_label(source, &source_aliases)
                    )
                } else {
                    format!("来源 · {}", source_label(source, &source_aliases))
                };
                left = left.child(
                    div()
                        .max_w(px(if compact { 150. } else { 240. }))
                        .truncate()
                        .px(px(6.))
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(SharedString::from(label)),
                );
            }
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
        if let Some(summary) = summary.as_ref().filter(|_| !scanning) {
            // 扫描结束摘要（新增/更新/跳过/移除/失败/耗时）；点击进入设置页的进度区。
            let this_summary = this.clone();
            left = left.child(status_bar_item(
                "status-scan-summary",
                SharedString::from(format_scan_summary(summary, compact)),
                if summary.failed > 0 {
                    colors.warning
                } else {
                    colors.subtle
                },
                colors,
                false,
                move |_, _, cx| {
                    let _ = this_summary.update(cx, |app, cx| app.open_settings(cx));
                },
            ));
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

        let progress_fraction = if scanning {
            if progress.total > 0 {
                (progress.done as f32 / progress.total as f32).clamp(0.0, 1.0)
            } else {
                0.0
            }
        } else {
            0.0
        };
        let progress_line = (progress_fraction > 0.0).then(|| {
            div()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .h(px(2.))
                .w(px(width.as_f32() * progress_fraction))
                .bg(colors.accent)
        });

        div()
            .flex_none()
            .w_full()
            .h(px(STATUS_BAR_HEIGHT))
            .relative()
            .px(px(8.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .bg(colors.surface)
            .border_t_1()
            .border_color(colors.border)
            .text_xs()
            .children(progress_line)
            .child(left)
            .child(right)
            .into_any_element()
    }

}
