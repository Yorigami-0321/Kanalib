use crate::app::KanalibApp;
use crate::settings::format_tag_eta;
use crate::sidebar::source_label;
use crate::theme::*;
use crate::*;
use kanalib_core::model::{ScanPhase, ScanProgress, ScanSummary, StatusMessage};

/// 扫描阶段标签；底栏与设置页共用。
pub(crate) fn scan_phase_label(cx: &App, phase: ScanPhase) -> SharedString {
    let key = match phase {
        ScanPhase::Idle => "scan.phase.idle",
        ScanPhase::Discovering => "scan.phase.discovering",
        ScanPhase::Hashing => "scan.phase.hashing",
        ScanPhase::Thumbnails => "scan.phase.thumbnails",
        ScanPhase::Tagging => "scan.phase.tagging",
        ScanPhase::Reconciling => "scan.phase.reconciling",
        ScanPhase::Done => "scan.phase.done",
        ScanPhase::Failed => "scan.phase.failed",
    };
    crate::localization::t(cx, key)
}

/// 当前扫描的快照地址（阶段 done/total）。
pub(crate) fn scan_progress_counts(cx: &App, progress: &ScanProgress) -> String {
    let phase = scan_phase_label(cx, progress.phase).to_string();
    if progress.total > 0 {
        crate::localization::t_args(
            cx,
            "scan.progress",
            &[
                ("phase", &phase),
                ("done", &progress.done.to_string()),
                ("total", &progress.total.to_string()),
            ],
        )
        .to_string()
    } else {
        phase
    }
}

/// 扫描结束摘要的一行文本；`compact` 时只保留新增数，避免底栏被挤满。
pub(crate) fn format_scan_summary(cx: &App, summary: &ScanSummary, compact: bool) -> String {
    if compact {
        return crate::localization::t_args(
            cx,
            "scan.summary_compact",
            &[("n", &summary.imported.to_string())],
        )
        .to_string();
    }
    let elapsed = format!("{:.1}s", summary.elapsed_secs);
    let stopped = if summary.cancelled {
        crate::localization::t(cx, "scan.stopped").to_string()
    } else {
        String::new()
    };
    crate::localization::t_args(
        cx,
        "scan.summary",
        &[
            ("imported", &summary.imported.to_string()),
            ("updated", &summary.updated.to_string()),
            ("skipped", &summary.skipped.to_string()),
            ("removed", &summary.removed.to_string()),
            ("failed", &summary.failed.to_string()),
            ("elapsed", &elapsed),
            ("stopped", &stopped),
        ],
    )
    .to_string()
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
        .tab_index(0)
        .focus(move |style| style.bg(colors.hover))
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

/// 可撤销条目的轻量文案；深度为 0 时不显示（返回 `None`）。
#[cfg(test)]
pub(crate) fn undo_status_label(depth: usize) -> Option<String> {
    (depth > 0).then(|| format!("可撤销 {depth}"))
}

/// 撤销条目的无障碍名称：带上栈顶操作名，读屏能听出将会撤销什么。
#[cfg(test)]
pub(crate) fn undo_status_aria(depth: usize, label: Option<&str>) -> String {
    let base = undo_status_label(depth).unwrap_or_else(|| "没有可撤销的操作".to_string());
    match label {
        Some(label) => format!("撤销「{label}」· {base} · Ctrl+Z"),
        None => format!("{base} · Ctrl+Z"),
    }
}

impl KanalibApp {
    pub(crate) fn render_status_bar(
        &self,
        this: &Entity<Self>,
        status: &StatusMessage,
        error: Option<&str>,
        scanning: bool,
        device: Option<&str>,
        queue_pending: usize,
        queue_running: usize,
        queue_error: usize,
        counts: &Counts,
        filter: &Filter,
        undo_depth: usize,
        undo_label: Option<&str>,
        width: Pixels,
        colors: UiColors,
        cx: &App,
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
                crate::localization::t_args(cx, "status.error", &[("text", &text)]),
                colors.danger,
                colors,
                false,
                move |_, _, cx| {
                    let _ = this_error.update(cx, |app, cx| app.open_settings(cx));
                },
            ));
            if self.error_retry_action(message).is_some() {
                let this_retry = this.clone();
                left = left.child(status_bar_item(
                    "status-error-retry",
                    crate::localization::t(cx, "status.retry"),
                    colors.accent,
                    colors,
                    false,
                    move |_, _, cx| {
                        let _ = this_retry.update(cx, |app, cx| app.retry_ui_error(cx));
                    },
                ));
            }
        } else if scanning {
            let this_stop = this.clone();
            left = left.child(status_bar_item(
                "status-scanning",
                crate::localization::t(cx, "status.scanning_stop"),
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
                    .child(SharedString::from(scan_progress_counts(cx, &progress))),
            );
            if progress.throughput > 0.0 && !compact {
                left = left.child(div().px(px(6.)).text_xs().text_color(colors.muted).child(
                    crate::localization::t_args(
                        cx,
                        "scan.throughput",
                        &[("rate", &format!("{:.1}", progress.throughput))],
                    ),
                ));
            }
            if let Some(eta) = progress.eta_secs {
                left = left.child(div().px(px(6.)).text_xs().text_color(colors.muted).child(
                    crate::localization::t_args(
                        cx,
                        "scan.eta",
                        &[("eta", &format_tag_eta(Some(eta)))],
                    ),
                ));
            }
            if let Some(source) = &progress.source {
                let source_name = source_label(source, &source_aliases);
                let label = if progress.source_total > 1 {
                    crate::localization::t_args(
                        cx,
                        "scan.source_multi",
                        &[
                            ("index", &progress.source_index.to_string()),
                            ("total", &progress.source_total.to_string()),
                            ("source", &source_name),
                        ],
                    )
                    .to_string()
                } else {
                    crate::localization::t_args(
                        cx,
                        "scan.source",
                        &[("source", &source_name)],
                    )
                    .to_string()
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
                    .child(crate::localization::core_status(cx, status)),
            );
        }
        if let Some(summary) = summary.as_ref().filter(|_| !scanning) {
            // 扫描结束摘要（新增/更新/跳过/移除/失败/耗时）；点击进入设置页的进度区。
            let this_summary = this.clone();
            left = left.child(status_bar_item(
                "status-scan-summary",
                SharedString::from(format_scan_summary(cx, summary, compact)),
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
        if let Some(hint) = &self.pending_restore_hint {
            let this_pending = this.clone();
            let hint = hint.clone();
            left = left.child(status_bar_item(
                "status-pending-restore",
                crate::localization::t(cx, "status.pending_restore"),
                colors.warning,
                colors,
                false,
                move |_, _, cx| {
                    let hint = hint.clone();
                    let _ = this_pending.update(cx, |app, cx| {
                        app.open_settings(cx);
                        app.settings_notice = Some(hint);
                        cx.notify();
                    });
                },
            ));
        }
        if self.search_running {
            left = left.child(
                div()
                    .px(px(6.))
                    .text_xs()
                    .text_color(colors.accent)
                    .child(crate::localization::t(cx, "status.searching")),
            );
        }
        // F-067：有可撤销项时显示条数并可点击撤销；空栈不占位。
        if undo_depth > 0 {
            let this_undo = this.clone();
            let label = crate::localization::t_args(
                cx,
                "status.undo",
                &[("n", &undo_depth.to_string())],
            )
            .to_string();
            let action = undo_label.map(|label| crate::localization::undo_action_label(cx, label));
            let aria = match action {
                Some(action) => crate::localization::t_args(
                    cx,
                    "status.undo_aria_labeled",
                    &[("label", &action), ("base", &label)],
                )
                .to_string(),
                None => crate::localization::t_args(
                    cx,
                    "status.undo_aria",
                    &[("base", &label)],
                )
                .to_string(),
            };
            left = left.child(
                div()
                    .id("status-undo")
                    .tab_index(0)
                    .focus(move |style| style.bg(colors.hover))
                    .role(Role::Button)
                    .aria_label(SharedString::from(aria))
                    .h_full()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(move |item| item.bg(colors.hover))
                    .on_click(move |_, _, cx| {
                        let _ = this_undo.update(cx, |app, cx| app.undo_last(cx));
                    })
                    .child(
                        Icon::new(IconName::Undo2)
                            .size(IconSize::Xs)
                            .color(colors.muted),
                    )
                    .child(
                        div()
                            .truncate()
                            .whitespace_nowrap()
                            .text_xs()
                            .text_color(colors.muted)
                            .child(SharedString::from(label)),
                    ),
            );
        }
        let this_all = this.clone();
        left = left.child(status_bar_item(
            "status-total",
            crate::localization::t_args(cx, "status.total", &[("n", &counts.total.to_string())]),
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
                    .child(crate::localization::t_args(
                        cx,
                        "status.tagged",
                        &[("n", &counts.tagged.to_string())],
                    )),
            )
            .child({
                let this_untagged = this.clone();
                status_bar_item(
                    "status-untagged",
                    crate::localization::t_args(
                        cx,
                        "status.untagged",
                        &[("n", &counts.untagged.to_string())],
                    ),
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
                crate::localization::t_args(
                    cx,
                    "status.failed",
                    &[("n", &counts.failed.to_string())],
                ),
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
                crate::localization::t_args(
                    cx,
                    "status.damaged",
                    &[("n", &counts.damaged.to_string())],
                ),
                colors.warning,
                colors,
                library && *filter == Filter::Damaged,
                move |_, _, cx| {
                    let _ = this_damaged.update(cx, |app, cx| app.set_filter(Filter::Damaged, cx));
                },
            ));
        }

        let mut right = div().flex_none().flex().items_center().gap(px(2.));
        if !compact {
            let this_queue = this.clone();
            let queue_label = if queue_pending + queue_running + queue_error == 0 {
                crate::localization::t(cx, "status.queue_idle").to_string()
            } else {
                crate::localization::t_args(
                    cx,
                    "status.queue",
                    &[
                        ("waiting", &queue_pending.to_string()),
                        ("running", &queue_running.to_string()),
                        ("failed", &queue_error.to_string()),
                    ],
                )
                .to_string()
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
                        .unwrap_or_else(|| {
                            crate::localization::t(cx, "status.device_missing").to_string()
                        }),
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
            .tab_index(0)
            .focus(move |style| style.bg(colors.hover))
            .role(Role::Button)
            .aria_label(crate::localization::t(cx, "status.palette_shortcut"))
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
            .children(
                (!compact).then(|| {
                    div()
                        .text_xs()
                        .text_color(colors.muted)
                        .child(crate::localization::t(cx, "status.palette"))
                }),
            )
            .children(
                (!compact).then(|| div().text_xs().text_color(colors.subtle).child("Ctrl+K")),
            );
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

#[cfg(test)]
mod tests {
    use super::{undo_status_aria, undo_status_label};

    #[test]
    fn undo_label_hidden_when_stack_empty() {
        assert_eq!(undo_status_label(0), None);
        assert_eq!(undo_status_label(1).as_deref(), Some("可撤销 1"));
        assert_eq!(undo_status_label(12).as_deref(), Some("可撤销 12"));
    }

    #[test]
    fn undo_aria_mentions_top_label() {
        assert_eq!(
            undo_status_aria(2, Some("设置 4 星")),
            "撤销「设置 4 星」· 可撤销 2 · Ctrl+Z"
        );
        assert_eq!(undo_status_aria(0, None), "没有可撤销的操作 · Ctrl+Z");
    }
}
