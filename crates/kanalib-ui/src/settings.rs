use crate::*;
use crate::app::{KanalibApp, push_toast};
use crate::thumb_cache::CacheCleanup;
use crate::format::*;
use crate::platform::*;
use crate::sidebar::source_label;
use crate::status_bar::{format_scan_summary, scan_phase_label, scan_progress_counts};
use crate::tags::group_reviews_by_asset;
use crate::theme::*;

pub(crate) const SETTINGS_MAX_WIDTH: f32 = 1080.;
pub(crate) const TAG_ROW_HEIGHT: f32 = 54.;

/// 设置页阈值编辑器按 sidecar 的类别顺序展示；未知类别不会出现在这里。
pub(crate) const TAG_CATEGORY_LABELS: [(&str, &str); 6] = [
    ("general", "通用"),
    ("character", "角色"),
    ("copyright", "版权"),
    ("style", "风格"),
    ("meta", "元数据"),
    ("rating", "分级"),
];

/// 重打标超过这个数量先弹确认，避免误点把整个视图重新入队。
pub(crate) const TAG_RETAG_CONFIRM_THRESHOLD: usize = 500;

/// 队列区块最多展示的失败任务数。
pub(crate) const TAG_JOB_ERROR_LIMIT: usize = 5;

pub(crate) fn tag_category_label(category: &str) -> &'static str {
    TAG_CATEGORY_LABELS
        .iter()
        .find(|(key, _)| *key == category)
        .map(|(_, label)| *label)
        .unwrap_or("未知类别")
}

/// 阈值按钮按 0.01 步进；保留两位小数并夹在 0–1，避免浮点累积成 0.18000001。
pub(crate) fn threshold_step(current: f32, delta: f32) -> f32 {
    (((current + delta).clamp(0.0, 1.0)) * 100.0).round() / 100.0
}

pub(crate) fn parse_tag_threshold_text(text: &str) -> Result<f32, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("请输入 0–1 之间的阈值".to_string());
    }
    let value = trimmed
        .parse::<f32>()
        .map_err(|_| "阈值必须是数字".to_string())?;
    if !value.is_finite() {
        return Err("阈值必须是有限数字".to_string());
    }
    if !(0.0..=1.0).contains(&value) {
        return Err("阈值需在 0–1 之间".to_string());
    }
    Ok(value)
}

pub(crate) fn tag_queue_phase(status: &TagQueueStatus) -> &'static str {
    if status.cancelling {
        "取消中"
    } else if status.paused {
        "已暂停"
    } else if status.running > 0 {
        "运行中"
    } else if status.pending > 0 {
        "等待中"
    } else if status.error > 0 {
        "有空闲失败"
    } else {
        "空闲"
    }
}

pub(crate) fn format_tag_throughput(rate: f32) -> String {
    if rate > 0.0 && rate.is_finite() {
        format!("{rate:.1} 张/秒")
    } else {
        "—".to_string()
    }
}

pub(crate) fn format_tag_eta(eta: Option<u64>) -> String {
    match eta {
        None => "—".to_string(),
        Some(0) => "0 秒".to_string(),
        Some(seconds) if seconds < 60 => format!("{seconds} 秒"),
        Some(seconds) if seconds < 3_600 => {
            format!("{} 分 {:02} 秒", seconds / 60, seconds % 60)
        }
        Some(seconds) => format!("{} 小时 {:02} 分", seconds / 3_600, (seconds % 3_600) / 60),
    }
}

/// Debug 覆盖全部展示字段；只用于判断 1s 轮询结果是否有变化，避免空闲时反复重绘。
pub(crate) fn tag_queue_signature(status: &TagQueueStatus) -> String {
    format!("{status:?}")
}

/// 来源排除规则的一行只读摘要。
pub(crate) fn scan_filter_summary(filter: &ScanFilterConfig) -> String {
    let hidden = if filter.include_hidden {
        "包含隐藏"
    } else {
        "排除隐藏"
    };
    let min = filter
        .min_bytes
        .map(format_bytes)
        .unwrap_or_else(|| "不限".to_string());
    let max = filter
        .max_bytes
        .map(format_bytes)
        .unwrap_or_else(|| "不限".to_string());
    format!(
        "排除规则：{} 条 glob · {hidden} · 大小 {min} – {max}",
        filter.exclude_globs.len()
    )
}

/// 排除规则编辑器的大小输入：空串表示不限制，其余必须是整数字节数。
pub(crate) fn parse_optional_bytes(text: &str, label: &str) -> Result<Option<u64>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<u64>()
        .map(Some)
        .map_err(|_| format!("{label}必须是整数字节数（留空表示不限）"))
}

/// 自动备份计划的一行摘要（设置页只读展示）。
pub(crate) fn backup_plan_summary(plan: &backup::BackupPlan) -> String {
    if !plan.enabled {
        return "自动备份未启用；启用后按频率在后台复制数据库并校验".to_string();
    }
    let mut text = format!(
        "自动备份已启用：{} · 保留最近 {} 份",
        plan.frequency.label(),
        plan.keep_files
    );
    if let Some(days) = plan.keep_days {
        text.push_str(&format!(" · 仅保留 {days} 天内"));
    }
    text
}

/// 下次备份/待恢复信息的展示文本；纯函数便于单测。
pub(crate) fn backup_next_due_text(status: &backup::BackupStatus) -> String {
    if let Some(pending) = &status.pending_restore {
        return format!(
            "已安排恢复：{} · 重启 Kanalib 后应用",
            pending.candidate.display()
        );
    }
    if !status.plan.enabled {
        return "自动备份未启用".to_string();
    }
    match status.next_due {
        Some(due) if due <= SystemTime::now() => "自动备份已到期，将在扫描/打标空闲时执行".to_string(),
        Some(due) => format!("下次自动备份：{}", backup::format_utc_datetime(due)),
        None => "下次备份时间待计算".to_string(),
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn tag_threshold_step_rounds_and_clamps() {
        assert_eq!(super::threshold_step(0.17, 0.01), 0.18);
        assert_eq!(super::threshold_step(0.005, -0.01), 0.0);
        assert_eq!(super::threshold_step(0.99, 0.01), 1.0);
        assert_eq!(super::threshold_step(0.0, -0.01), 0.0);
    }

    #[test]
    fn tag_threshold_input_rejects_invalid_values() {
        assert_eq!(super::parse_tag_threshold_text("0.5").unwrap(), 0.5);
        assert_eq!(super::parse_tag_threshold_text(" 1 ").unwrap(), 1.0);
        for bad in ["", "-0.01", "1.01", "nan", "abc"] {
            assert!(
                super::parse_tag_threshold_text(bad).is_err(),
                "expected error for {bad:?}"
            );
        }
    }

    #[test]
    fn tag_eta_and_category_labels_are_stable() {
        assert_eq!(super::format_tag_eta(None), "—");
        assert_eq!(super::format_tag_eta(Some(0)), "0 秒");
        assert_eq!(super::format_tag_eta(Some(12)), "12 秒");
        assert_eq!(super::format_tag_eta(Some(65)), "1 分 05 秒");
        assert_eq!(super::format_tag_eta(Some(3_700)), "1 小时 01 分");
        assert_eq!(super::tag_category_label("general"), "通用");
        assert_eq!(super::tag_category_label("rating"), "分级");
        assert_eq!(super::tag_category_label("unknown"), "未知类别");
    }

    #[test]
    fn scan_filter_summary_reports_config() {
        let filter = kanalib_core::model::ScanFilterConfig {
            exclude_globs: vec!["**/.git/**".into(), "*_cache".into()],
            include_hidden: false,
            min_bytes: Some(1024),
            max_bytes: Some(2 * 1024 * 1024),
        };
        let summary = super::scan_filter_summary(&filter);
        assert!(summary.contains("2 条 glob"));
        assert!(summary.contains("排除隐藏"));
        assert!(summary.contains("1.0 KB"));
        assert!(summary.contains("2.0 MB"));
    }

    #[test]
    fn filter_size_input_accepts_empty_or_bytes() {
        assert_eq!(super::parse_optional_bytes("  ", "最小大小").unwrap(), None);
        assert_eq!(
            super::parse_optional_bytes("2048", "最小大小").unwrap(),
            Some(2048)
        );
        assert!(super::parse_optional_bytes("2MB", "最小大小").is_err());
        assert!(super::parse_optional_bytes("-1", "最小大小").is_err());
    }

    #[test]
    fn backup_summary_and_next_due_text_cover_plan_states() {
        let mut plan = kanalib_core::backup::BackupPlan::default();
        let mut status = kanalib_core::backup::BackupStatus::default();
        assert!(super::backup_plan_summary(&plan).contains("未启用"));
        assert_eq!(super::backup_next_due_text(&status), "自动备份未启用");

        plan.enabled = true;
        plan.frequency = kanalib_core::backup::BackupFrequency::Weekly;
        plan.keep_files = 5;
        plan.keep_days = Some(14);
        let summary = super::backup_plan_summary(&plan);
        assert!(summary.contains("每周"), "{summary}");
        assert!(summary.contains("5"), "{summary}");
        assert!(summary.contains("14"), "{summary}");

        status.plan = plan;
        status.next_due = Some(std::time::SystemTime::now() + std::time::Duration::from_secs(3600));
        assert!(super::backup_next_due_text(&status).contains("下次自动备份"));
        status.next_due = Some(std::time::SystemTime::UNIX_EPOCH);
        assert!(super::backup_next_due_text(&status).contains("已到期"));
    }

}

impl KanalibApp {
    /// 读取阈值、队列状态、CPU 开关与最近失败；设置页打开/操作后/轮询时调用。
    pub(crate) fn refresh_tag_settings(&mut self, cx: &mut Context<Self>) {
        match self.state.tag_thresholds() {
            Ok(thresholds) => self.tag_thresholds = Some(thresholds),
            Err(error) => {
                log::warn!("tag thresholds: {error:#}");
                self.tag_settings_notice = Some(format!("读取打标阈值失败：{error:#}"));
            }
        }
        self.tag_cpu_allowed = self.state.allow_cpu_tagging();
        self.refresh_tag_queue_status(cx);
        self.refresh_tag_job_errors();
        cx.notify();
    }

    /// 只刷新队列快照与失败首条；状态签名不变时不触发重绘。
    pub(crate) fn refresh_tag_queue_status(&mut self, cx: &mut Context<Self>) {
        let status = self.state.tag_queue_status();
        let signature = tag_queue_signature(&status);
        let changed = self.tag_queue_signature.as_deref() != Some(signature.as_str());
        self.tag_queue_status = Some(status);
        self.tag_queue_signature = Some(signature);
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn refresh_tag_job_errors(&mut self) {
        match self.state.tag_job_errors(TAG_JOB_ERROR_LIMIT) {
            Ok(errors) => self.tag_job_errors = errors,
            Err(error) => log::warn!("tag job errors: {error:#}"),
        }
    }

    pub(crate) fn category_threshold_value(&self, category: &str) -> f32 {
        self.tag_thresholds
            .as_ref()
            .and_then(|thresholds| thresholds.categories.get(category).copied())
            .unwrap_or_else(|| {
                default_category_thresholds()
                    .get(category)
                    .copied()
                    .unwrap_or(0.2)
            })
    }

    pub(crate) fn apply_category_threshold(
        &mut self,
        category: &str,
        value: f32,
        cx: &mut Context<Self>,
    ) {
        if !TAG_CATEGORY_LABELS
            .iter()
            .any(|(key, _)| *key == category)
        {
            self.tag_settings_notice = Some(format!("未知标签类别：{category}"));
            cx.notify();
            return;
        }
        match self.state.set_category_threshold(category, value) {
            Ok(saved) => {
                self.tag_settings_notice = Some(format!(
                    "{} 阈值已保存为 {saved:.2}；点击“重算”后应用到已存候选",
                    tag_category_label(category)
                ));
            }
            Err(error) => {
                let message = format!("设置 {} 阈值失败：{error:#}", tag_category_label(category));
                self.tag_settings_notice = Some(message.clone());
                push_toast(&self.toaster, cx, message, Severity::Danger);
            }
        }
        self.refresh_tag_settings(cx);
        cx.notify();
    }

    pub(crate) fn step_category_threshold(&mut self, category: &str, delta: f32, cx: &mut Context<Self>) {
        let current = self.category_threshold_value(category);
        let next = threshold_step(current, delta);
        if (next - current).abs() < 0.0005 {
            self.tag_settings_notice = Some(format!(
                "{} 阈值已在 {current:.2}，无法继续调整（范围 0–1）",
                tag_category_label(category)
            ));
            cx.notify();
            return;
        }
        self.apply_category_threshold(category, next, cx);
    }

    pub(crate) fn reset_category_threshold(&mut self, category: &str, cx: &mut Context<Self>) {
        let default = default_category_thresholds()
            .get(category)
            .copied()
            .unwrap_or(0.2);
        self.apply_category_threshold(category, default, cx);
    }

    pub(crate) fn apply_tag_override(&mut self, name: &str, value: &str, cx: &mut Context<Self>) {
        let tag = normalize_tag_name(name);
        if tag.is_empty() {
            self.tag_settings_notice = Some("请输入要覆盖的标签名".to_string());
            push_toast(
                &self.toaster,
                cx,
                "请输入要覆盖的标签名",
                Severity::Warning,
            );
            cx.notify();
            return;
        }
        let value = match parse_tag_threshold_text(value) {
            Ok(value) => value,
            Err(message) => {
                self.tag_settings_notice = Some(format!("单标签阈值无效：{message}"));
                push_toast(
                    &self.toaster,
                    cx,
                    format!("单标签阈值无效：{message}"),
                    Severity::Warning,
                );
                cx.notify();
                return;
            }
        };
        match self.state.set_tag_threshold(&tag, Some(value)) {
            Ok(saved) => {
                self.tag_settings_notice = Some(format!(
                    "标签 #{tag} 阈值已设为 {:.2}；重算后生效",
                    saved.unwrap_or(value)
                ));
            }
            Err(error) => {
                let message = format!("设置标签 #{tag} 阈值失败：{error:#}");
                self.tag_settings_notice = Some(message.clone());
                push_toast(&self.toaster, cx, message, Severity::Danger);
            }
        }
        self.refresh_tag_settings(cx);
        cx.notify();
    }

    pub(crate) fn clear_tag_override(&mut self, tag: &str, cx: &mut Context<Self>) {
        match self.state.set_tag_threshold(tag, None) {
            Ok(_) => {
                self.tag_settings_notice = Some(format!("标签 #{tag} 已恢复类别阈值；重算后生效"));
            }
            Err(error) => {
                let message = format!("清除标签 #{tag} 覆盖失败：{error:#}");
                self.tag_settings_notice = Some(message.clone());
                push_toast(&self.toaster, cx, message, Severity::Danger);
            }
        }
        self.refresh_tag_settings(cx);
        cx.notify();
    }

    /// 重算/重打标的目标路径：选中优先，否则当前可见视图。
    pub(crate) fn tag_target_paths(&self) -> (Vec<PathBuf>, &'static str) {
        let selected = self.selected_paths();
        if selected.is_empty() {
            let mut paths = self.visible_paths();
            paths.sort();
            paths.dedup();
            (paths, "当前视图")
        } else {
            let mut paths = selected;
            paths.sort();
            paths.dedup();
            (paths, "选中")
        }
    }

    pub(crate) fn start_recompute_tag_thresholds(&mut self, cx: &mut Context<Self>) {
        if self.tag_recompute_running {
            return;
        }
        let (paths, scope) = self.tag_target_paths();
        if paths.is_empty() {
            self.tag_settings_notice =
                Some("没有可重算的图片：先选择图片或打开有内容的视图".to_string());
            push_toast(
                &self.toaster,
                cx,
                "没有可重算的图片",
                Severity::Warning,
            );
            cx.notify();
            return;
        }
        let count = paths.len();
        self.tag_recompute_running = true;
        self.tag_settings_notice = Some(format!("正在按候选重算{scope} {count} 张的阈值…"));
        let state = self.state.clone();
        let task = cx
            .background_executor()
            .spawn(async move { state.recompute_tag_thresholds(&paths) });
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = task.await;
            let _ = weak.update(cx, |app, cx| {
                app.tag_recompute_running = false;
                match result {
                    Ok(TagRecomputeSummary { updated, skipped }) => {
                        let message = format!(
                            "阈值重算完成（{scope}）：更新 {updated} 张 · 跳过 {skipped} 张；跳过项可能缺候选或超出已存 top-K，需要重打标"
                        );
                        app.tag_settings_notice = Some(message.clone());
                        push_toast(&app.toaster, cx, message, Severity::Success);
                    }
                    Err(error) => {
                        let message = format!("阈值重算失败：{error:#}");
                        app.tag_settings_notice = Some(message.clone());
                        push_toast(&app.toaster, cx, message, Severity::Danger);
                    }
                }
                app.refresh_tag_settings(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn request_retag(&mut self, selected_only: bool, cx: &mut Context<Self>) {
        let (mut paths, scope) = if selected_only {
            (self.selected_paths(), "选中")
        } else {
            (self.visible_paths(), "当前视图")
        };
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            let message = if selected_only {
                "先选择要重打标的图片"
            } else {
                "当前视图没有可重打标的图片"
            };
            self.tag_settings_notice = Some(message.to_string());
            push_toast(&self.toaster, cx, message, Severity::Warning);
            cx.notify();
            return;
        }
        if paths.len() >= TAG_RETAG_CONFIRM_THRESHOLD {
            self.tag_retag_confirm = Some(TagRetagConfirm { paths, scope });
        } else {
            self.start_retag(paths, scope, cx);
        }
        cx.notify();
    }

    pub(crate) fn start_retag(&mut self, paths: Vec<PathBuf>, scope: &'static str, cx: &mut Context<Self>) {
        if self.tag_retag_running {
            self.tag_settings_notice = Some("已有重打标任务在运行，请稍候".to_string());
            cx.notify();
            return;
        }
        let count = paths.len();
        self.tag_retag_running = true;
        self.tag_settings_notice = Some(format!(
            "正在重打标{scope} {count} 张；已有候选的按阈值重算，缺候选的入队…"
        ));
        let state = self.state.clone();
        let task = cx
            .background_executor()
            .spawn(async move { state.retag_paths(&paths) });
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = task.await;
            let _ = weak.update(cx, |app, cx| {
                app.tag_retag_running = false;
                match result {
                    Ok(RetagSummary {
                        recomputed,
                        queued,
                        missing,
                    }) => {
                        let message = format!(
                            "重打标完成（{scope}）：按候选重算 {recomputed} 张 · 入队 {queued} 张 · 缺失 {missing} 张；手工标签已保留"
                        );
                        app.tag_settings_notice = Some(message.clone());
                        push_toast(&app.toaster, cx, message, Severity::Success);
                    }
                    Err(error) => {
                        let message = format!("重打标失败：{error:#}");
                        app.tag_settings_notice = Some(message.clone());
                        push_toast(&app.toaster, cx, message, Severity::Danger);
                    }
                }
                app.refresh_tag_settings(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn prioritize_selected(&mut self, cx: &mut Context<Self>) {
        let mut paths = self.selected_paths();
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            self.tag_settings_notice = Some("先选择图片，再提升打标优先级".to_string());
            push_toast(
                &self.toaster,
                cx,
                "先选择图片，再提升打标优先级",
                Severity::Warning,
            );
            cx.notify();
            return;
        }
        match self.state.prioritize_tag_jobs(&paths, 100) {
            Ok(0) => {
                self.tag_settings_notice =
                    Some("选中图片没有等待中的打标任务（可能已完成或正在运行）".to_string());
            }
            Ok(count) => {
                self.tag_settings_notice =
                    Some(format!("已把 {count} 个打标任务提升到高优先级"));
            }
            Err(error) => {
                let message = format!("提升打标优先级失败：{error:#}");
                self.tag_settings_notice = Some(message.clone());
                push_toast(&self.toaster, cx, message, Severity::Danger);
            }
        }
        self.refresh_tag_settings(cx);
        cx.notify();
    }

    pub(crate) fn toggle_tag_queue(&mut self, cx: &mut Context<Self>) {
        let paused = self.state.shared.lock().unwrap().tag_status.paused;
        if paused {
            match self.state.resume_tag_queue() {
                Ok(()) => {
                    self.tag_settings_notice = Some("已继续打标队列".to_string());
                }
                Err(error) => {
                    let message = format!("继续打标失败：{error:#}");
                    self.tag_settings_notice = Some(message.clone());
                    push_toast(&self.toaster, cx, message, Severity::Danger);
                }
            }
        } else {
            self.state.pause_tag_queue();
            self.tag_settings_notice =
                Some("已暂停打标；当前批完成后不再认领新任务".to_string());
        }
        self.refresh_tag_settings(cx);
        cx.notify();
    }

    pub(crate) fn cancel_tag_queue(&mut self, cx: &mut Context<Self>) {
        self.state.cancel_tag_queue();
        self.tag_settings_notice =
            Some("正在取消打标；已完成的结果会保留".to_string());
        self.refresh_tag_settings(cx);
        cx.notify();
    }

    pub(crate) fn toggle_cpu_tagging(&mut self, cx: &mut Context<Self>) {
        let allow = !self.tag_cpu_allowed;
        match self.state.set_allow_cpu_tagging(allow) {
            Ok(()) => {
                self.tag_settings_notice = Some(if allow {
                    "已允许 CPU 慢速打标；继续队列后生效".to_string()
                } else {
                    "已关闭 CPU 慢速打标；GPU 不可用时队列会暂停".to_string()
                });
            }
            Err(error) => {
                let message = format!("更新 CPU 打标设置失败：{error:#}");
                self.tag_settings_notice = Some(message.clone());
                push_toast(&self.toaster, cx, message, Severity::Danger);
            }
        }
        self.refresh_tag_settings(cx);
        cx.notify();
    }

    pub(crate) fn refresh_cache_stats(&mut self, cx: &mut Context<Self>) {
        match self.state.cache_stats() {
            Ok(stats) => self.cache_stats = Some(stats),
            Err(error) => {
                log::warn!("cache stats: {error:#}");
                push_toast(
                    &self.toaster,
                    cx,
                    format!("读取缓存统计失败：{error:#}"),
                    Severity::Danger,
                );
            }
        }
        cx.notify();
    }

    /// 采集只读诊断快照；包含资产/队列/来源/错误/日志路径。
    pub(crate) fn refresh_diagnostics(&mut self) {
        let db_path = self.state.db_path();
        let Some(data_dir) = db_path.parent().map(Path::to_path_buf) else {
            return;
        };
        let (asset_count, queue_pending, queue_running, queue_error, last_error, sidecar_device) = {
            let shared = self.state.shared.lock().unwrap();
            (
                shared.counts.total(),
                shared.queue_pending,
                shared.queue_running,
                shared.queue_error,
                shared.error.clone(),
                shared.sidecar_device.clone(),
            )
        };
        let log_path = std::env::var_os("KANALIB_LOG_FILE").map(PathBuf::from);
        let context = DiagnosticsContext {
            asset_count: Some(asset_count),
            queue_pending: Some(queue_pending),
            queue_running: Some(queue_running),
            queue_error: Some(queue_error),
            sources: self.state.source_statuses(),
            last_scan: None,
            last_error,
            sidecar_device,
            log_path,
        };
        match diagnostics::collect(&data_dir, context) {
            Ok(snapshot) => self.diagnostic_snapshot = Some(snapshot),
            Err(error) => log::warn!("diagnostics: {error:#}"),
        }
        self.sync_source_statuses();
    }

    /// 立即备份数据库；阻塞的备份在后台执行器上跑，不占 UI 线程。
    pub(crate) fn start_backup(&mut self, cx: &mut Context<Self>) {
        if self.backup_running {
            return;
        }
        if self.state.shared.lock().unwrap().scanning {
            self.settings_notice = Some("图库正在扫描，完成后再备份。".to_string());
            push_toast(
                &self.toaster,
                cx,
                "扫描进行中，完成后再备份",
                Severity::Warning,
            );
            cx.notify();
            return;
        }
        let state = self.state.clone();
        self.backup_running = true;
        self.settings_notice = Some("正在备份数据库…".to_string());
        let task = cx
            .background_executor()
            .spawn(async move { state.backup_now() });
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = task.await;
            let _ = weak.update(cx, |app, cx| {
                app.backup_running = false;
                match result {
                    Ok(report) => {
                        let mut summary = format!(
                            "最近备份：{} · {} · 耗时 {} ms · 完整性 {}",
                            report.destination.display(),
                            format_bytes(report.bytes),
                            report.elapsed_ms,
                            if report.integrity_ok { "通过" } else { "异常" }
                        );
                        if !report.warnings.is_empty() {
                            summary.push_str(&format!(
                                " · 警告：{}",
                                report.warnings.join("；")
                            ));
                        }
                        app.last_backup = Some(summary);
                        app.settings_notice = Some(format!(
                            "备份完成：{}",
                            report.destination.display()
                        ));
                        push_toast(
                            &app.toaster,
                            cx,
                            format!("备份完成：{}", report.destination.display()),
                            Severity::Success,
                        );
                        app.refresh_diagnostics();
                    }
                    Err(error) => {
                        let message = format!("最近备份失败：{error:#}");
                        app.last_backup = Some(message.clone());
                        app.settings_notice = Some(message.clone());
                        push_toast(&app.toaster, cx, message, Severity::Danger);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn open_backups_dir(&mut self, cx: &mut Context<Self>) {
        let data_dir = self.data_dir();
        match backup::ensure_backups_dir(&data_dir) {
            Ok(dir) => open_path(&dir),
            Err(error) => {
                self.settings_notice = Some(format!("打开备份目录失败：{error:#}"));
            }
        }
        cx.notify();
    }

    fn save_backup_plan(
        &mut self,
        plan: backup::BackupPlan,
        notice: String,
        cx: &mut Context<Self>,
    ) {
        match self.state.set_backup_plan(&plan) {
            Ok(()) => {
                self.settings_notice = Some(notice);
            }
            Err(error) => {
                self.settings_notice = Some(format!("保存备份计划失败：{error:#}"));
            }
        }
        cx.notify();
    }

    pub(crate) fn toggle_backup_plan(&mut self, cx: &mut Context<Self>) {
        let mut plan = self.state.backup_status().plan;
        plan.enabled = !plan.enabled;
        let summary = backup_plan_summary(&plan);
        self.save_backup_plan(plan, summary, cx);
    }

    pub(crate) fn set_backup_frequency(
        &mut self,
        frequency: backup::BackupFrequency,
        cx: &mut Context<Self>,
    ) {
        let mut plan = self.state.backup_status().plan;
        if plan.frequency == frequency {
            return;
        }
        plan.frequency = frequency;
        let summary = backup_plan_summary(&plan);
        self.save_backup_plan(plan, summary, cx);
    }

    pub(crate) fn adjust_backup_keep_files(&mut self, delta: i64, cx: &mut Context<Self>) {
        let mut plan = self.state.backup_status().plan;
        plan.keep_files = (plan.keep_files as i64 + delta).clamp(1, 365) as usize;
        let summary = backup_plan_summary(&plan);
        self.save_backup_plan(plan, summary, cx);
    }

    pub(crate) fn toggle_backup_keep_days(&mut self, cx: &mut Context<Self>) {
        let mut plan = self.state.backup_status().plan;
        plan.keep_days = if plan.keep_days.is_some() {
            None
        } else {
            Some(30)
        };
        let summary = backup_plan_summary(&plan);
        self.save_backup_plan(plan, summary, cx);
    }

    pub(crate) fn adjust_backup_keep_days(&mut self, delta: i64, cx: &mut Context<Self>) {
        let mut plan = self.state.backup_status().plan;
        let Some(days) = plan.keep_days else {
            return;
        };
        plan.keep_days = Some((days as i64 + delta).clamp(1, 3_650) as u32);
        let summary = backup_plan_summary(&plan);
        self.save_backup_plan(plan, summary, cx);
    }

    /// 异步选择恢复候选并确认；不阻塞 UI 线程。
    pub(crate) fn pick_restore_backup(&mut self, cx: &mut Context<Self>) {
        if self.backup_running {
            self.settings_notice = Some("已有备份或恢复任务在进行，请稍候".to_string());
            cx.notify();
            return;
        }
        let start_dir = backup::backups_dir(&self.data_dir());
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let Some(handle) = rfd::AsyncFileDialog::new()
                .set_title("选择要恢复的数据库备份")
                .set_directory(start_dir)
                .add_filter("SQLite 数据库", &["db", "sqlite", "sqlite3"])
                .pick_file()
                .await
            else {
                return;
            };
            let candidate = handle.path().to_path_buf();
            let confirmed = rfd::AsyncMessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("恢复数据库备份")
                .set_description(format!(
                    "将用以下备份替换当前图库数据库：\n{}\n\n恢复前会自动创建当前库的安全备份；恢复在重启 Kanalib 后应用。继续？",
                    candidate.display()
                ))
                .set_buttons(rfd::MessageButtons::YesNo)
                .show()
                .await;
            if confirmed != rfd::MessageDialogResult::Yes {
                return;
            }
            let _ = weak.update(cx, |app, cx| app.begin_restore(candidate, cx));
        })
        .detach();
        cx.notify();
    }

    /// 后台校验候选、创建安全备份并暂存恢复；完成后提示重启。
    pub(crate) fn begin_restore(&mut self, candidate: PathBuf, cx: &mut Context<Self>) {
        if self.backup_running {
            self.settings_notice = Some("已有备份或恢复任务在进行，请稍候".to_string());
            cx.notify();
            return;
        }
        self.backup_running = true;
        self.settings_notice = Some(format!(
            "正在校验备份并准备恢复：{}",
            candidate.display()
        ));
        let state = self.state.clone();
        let task = cx
            .background_executor()
            .spawn(async move { state.restore_backup(&candidate) });
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = task.await;
            let _ = weak.update(cx, |app, cx| {
                app.backup_running = false;
                match result {
                    Ok(report) => {
                        let safety = report
                            .safety_backup
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "未创建（当前无数据库）".to_string());
                        let mut summary = format!(
                            "已准备恢复：{} · 安全备份：{safety} · 重启后生效",
                            report.candidate.display()
                        );
                        for warning in &report.warnings {
                            summary.push_str(&format!(" · 注意：{warning}"));
                        }
                        app.last_backup = Some(summary);
                        app.settings_notice = Some(
                            "备份校验通过；关闭并重新启动 Kanalib 后应用恢复。".to_string(),
                        );
                        push_toast(
                            &app.toaster,
                            cx,
                            "备份校验通过：重启 Kanalib 后应用恢复",
                            Severity::Warning,
                        );
                        app.refresh_pending_restore_hint(cx);
                    }
                    Err(error) => {
                        let message = format!("恢复失败：{error:#}");
                        app.last_backup = Some(message.clone());
                        app.settings_notice = Some(message.clone());
                        push_toast(&app.toaster, cx, message, Severity::Danger);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// 取消已暂存、等待重启的恢复请求；当前库不受影响，下次启动不再替换。
    pub(crate) fn cancel_pending_restore(&mut self, cx: &mut Context<Self>) {
        let data_dir = self.data_dir();
        match backup::discard_pending_restore(&data_dir) {
            Ok(true) => {
                let message = "已取消待恢复备份；当前数据库保持不变，下次启动不会替换".to_string();
                self.last_backup = Some(message.clone());
                self.settings_notice = Some(message.clone());
                push_toast(&self.toaster, cx, message, Severity::Success);
            }
            Ok(false) => {
                let message = "没有待恢复的备份".to_string();
                self.settings_notice = Some(message.clone());
                push_toast(&self.toaster, cx, message, Severity::Warning);
            }
            Err(error) => {
                let message = format!("取消待恢复备份失败：{error:#}");
                self.settings_notice = Some(message.clone());
                push_toast(&self.toaster, cx, message, Severity::Danger);
            }
        }
        self.refresh_pending_restore_hint(cx);
        cx.notify();
    }

    pub(crate) fn copy_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.refresh_diagnostics();
        let text = self
            .diagnostic_snapshot
            .as_ref()
            .map(DiagnosticSnapshot::to_text);
        match text {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.settings_notice = Some("诊断信息已复制到剪贴板".to_string());
            }
            None => {
                self.settings_notice = Some("诊断信息采集失败，暂无法复制".to_string());
            }
        }
        cx.notify();
    }

    pub(crate) fn open_log_dir(&mut self, cx: &mut Context<Self>) {
        let log_path = diagnostics::default_log_path(&self.data_dir());
        if let Some(dir) = log_path.parent().map(Path::to_path_buf) {
            let _ = std::fs::create_dir_all(&dir);
            open_path(&dir);
        }
        cx.notify();
    }

    pub(crate) fn cleanup_cache(&mut self, cx: &mut Context<Self>) {
        if self.state.shared.lock().unwrap().scanning {
            push_toast(
                &self.toaster,
                cx,
                "扫描进行中，完成后再清理缓存",
                Severity::Warning,
            );
            return;
        }
        match self.state.cleanup_orphan_thumbs() {
            Ok((files, bytes)) => {
                push_toast(
                    &self.toaster,
                    cx,
                    format!("已清理 {files} 个孤儿缩略图 · {}", format_bytes(bytes)),
                    Severity::Success,
                );
                self.refresh_cache_stats(cx);
            }
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("清理缓存失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    pub(crate) fn set_thumb_budget(&mut self, gb: u64, cx: &mut Context<Self>) {
        self.thumb_budget_gb = gb.clamp(1, 1000);
        self.persist_setting("cache.thumb_budget_gb", self.thumb_budget_gb.to_string());
        cx.notify();
    }

    pub(crate) fn run_cache_budget_cleanup(
        &mut self,
        manual: bool,
        cx: &mut Context<Self>,
    ) -> Option<CacheCleanup> {
        if self.state.shared.lock().unwrap().scanning {
            if manual {
                push_toast(
                    &self.toaster,
                    cx,
                    "扫描进行中，完成后再清理缓存",
                    Severity::Warning,
                );
            }
            return None;
        }
        let budget = self.thumb_budget_gb.saturating_mul(1024 * 1024 * 1024);
        let report = self
            .thumb_cache
            .update(cx, |cache, cx| cache.cleanup_to_budget(budget, cx));
        self.cache_last_cleanup = Some(format!(
            "最近清理 {} 个文件 · {} · 剩余 {} / 预算 {}",
            report.removed_files,
            format_bytes(report.removed_bytes),
            format_bytes(report.after_bytes),
            format_bytes(report.budget_bytes)
        ));
        if report.removed_files > 0 && let Ok(stats) = self.state.cache_stats() {
            self.cache_stats = Some(stats);
        }
        if manual {
            push_toast(
                &self.toaster,
                cx,
                if report.removed_files == 0 {
                    format!(
                        "缓存未超预算 · {} / {}",
                        format_bytes(report.after_bytes),
                        format_bytes(report.budget_bytes)
                    )
                } else {
                    format!(
                        "已淘汰 {} 个缩略图 · {}",
                        report.removed_files,
                        format_bytes(report.removed_bytes)
                    )
                },
                Severity::Success,
            );
            cx.notify();
        }
        Some(report)
    }

    pub(crate) fn render_settings(
        &self,
        this: &Entity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
        roots: &[PathBuf],
        source_aliases: &HashMap<PathBuf, String>,
        protected_paths: &[PathBuf],
        similar_threshold: u8,
        queue_pending: usize,
        queue_running: usize,
        queue_error: usize,
        status: &str,
        error: Option<&str>,
        cache_hits: u64,
        cache_misses: u64,
        width: Pixels,
        colors: UiColors,
    ) -> AnyElement {
        let page_padding = px(24.);
        let tag_override_name = window.use_keyed_state(
            "kanalib-tag-override-name",
            cx,
            |window, cx| TextInput::new(window, cx).placeholder("标签名，如 long_hair"),
        );
        let tag_override_value = window.use_keyed_state(
            "kanalib-tag-override-value",
            cx,
            |window, cx| TextInput::new(window, cx).placeholder("0.00–1.00"),
        );
        let this_back = this.clone();
        let this_add = this.clone();
        let this_refresh_cache = this.clone();
        let this_cleanup_cache = this.clone();
        let this_budget_less = this.clone();
        let this_budget_more = this.clone();
        let this_budget_cleanup = this.clone();
        let this_backup = this.clone();
        let this_open_backups = this.clone();
        let this_restore = this.clone();
        let this_backup_toggle = this.clone();
        let this_backup_daily = this.clone();
        let this_backup_weekly = this.clone();
        let this_backup_keep_less = this.clone();
        let this_backup_keep_more = this.clone();
        let this_backup_days_toggle = this.clone();
        let this_backup_days_less = this.clone();
        let this_backup_days_more = this.clone();
        let this_backup_cancel = this.clone();
        let this_copy_diagnostics = this.clone();
        let this_open_logs = this.clone();
        let cache_stats = self.cache_stats.clone();
        let thumb_budget_gb = self.thumb_budget_gb;
        let cache_last_cleanup = self.cache_last_cleanup.clone();
        let cache_hit_rate = if cache_hits + cache_misses == 0 {
            0.0
        } else {
            cache_hits as f64 * 100.0 / (cache_hits + cache_misses) as f64
        };
        let db_path = self.state.db_path();
        let cache_dir = self.state.cache_dir();
        let db_bytes = std::fs::metadata(&db_path)
            .map(|meta| meta.len())
            .unwrap_or(0);
        let cache_summary = cache_stats
            .as_ref()
            .map(|stats| {
                format!(
                    "{} 个文件 · {} · 孤儿 {} 个 · {}",
                    stats.files,
                    format_bytes(stats.bytes),
                    stats.orphan_files,
                    format_bytes(stats.orphan_bytes)
                )
            })
            .unwrap_or_else(|| "尚未统计，点击“刷新统计”".to_string());
        let orphan_count = cache_stats.as_ref().map_or(0, |stats| stats.orphan_files);
        let data_dir = db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| db_path.clone());
        let data_dir_for_open = data_dir.clone();
        let backup_status = self.state.backup_status();
        let backup_plan = backup_status.plan.clone();
        let backup_next_text = backup_next_due_text(&backup_status);
        let backup_pending = backup_status.pending_restore.clone();
        let backup_has_pending = backup_pending.is_some();
        let backup_skipped = backup_status
            .skipped_reason
            .as_ref()
            .map(|reason| format!("上次自动备份已跳过：{reason}（空闲后自动重试）"));
        let backup_error = backup_status.last_error.clone();
        let backup_last_text = if self.last_backup.is_none() {
            backup_status
                .last_path
                .as_ref()
                .map(|path| format!("最近备份：{}", path.display()))
        } else {
            None
        };
        let notice = self
            .settings_notice
            .clone()
            .or_else(|| Some(status.to_string()));
        let banner = error
            .map(|message| (message.to_string(), colors.danger))
            .or_else(|| notice.map(|message| (message, colors.muted)))
            .map(|(message, color)| {
                div()
                    .px(page_padding)
                    .py(px(8.))
                    .border_b_1()
                    .border_color(colors.border)
                    .bg(colors.sunken)
                    .text_xs()
                    .text_color(color)
                    .child(message)
                    .into_any_element()
            });

        let protected_rows = if protected_paths.is_empty() {
            div()
                .text_xs()
                .text_color(colors.subtle)
                .child("尚未设置保护路径。保护路径下的文件永远不会被移入回收站。")
                .into_any_element()
        } else {
            let mut list = div().flex().flex_col().gap(px(6.));
            for path in protected_paths {
                let path = path.clone();
                let this_remove = this.clone();
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.))
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(6.))
                        .bg(colors.sunken)
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(colors.fg)
                                .child(path.display().to_string()),
                        )
                        .child(
                            Button::new(
                                SharedString::from(format!("remove-protected-{}", path.display())),
                                "移除",
                            )
                            .on_click(move |_, _, cx| {
                                let path = path.clone();
                                let _ = this_remove.update(cx, |app, cx| {
                                    match app.state.remove_protected_path(&path) {
                                        Ok(()) => {
                                            app.settings_notice =
                                                Some(format!("已移除保护路径：{}", path.display()));
                                        }
                                        Err(error) => {
                                            app.settings_notice =
                                                Some(format!("移除失败: {error:#}"));
                                        }
                                    }
                                    cx.notify();
                                });
                            }),
                        ),
                );
            }
            list.into_any_element()
        };

        let protected_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("去重保护路径"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("保护路径优先作为保留项；其下文件不会被移入回收站。"),
            )
            .child(protected_rows)
            .child(
                div().flex().child(
                    Button::new("add-protected", "添加保护文件夹")
                        .primary()
                        .on_click(move |_, _, cx| {
                            let weak = this_add.downgrade();
                            cx.spawn(async move |cx| {
                                let Some(handle) = rfd::AsyncFileDialog::new().pick_folder().await
                                else {
                                    return;
                                };
                                let folder = handle.path().to_path_buf();
                                let _ = weak.update(cx, |app, cx| {
                                    match app.state.add_protected_path(folder) {
                                        Ok(()) => {
                                            app.settings_notice =
                                                Some("已添加保护路径".to_string());
                                        }
                                        Err(error) => {
                                            app.settings_notice =
                                                Some(format!("添加失败: {error:#}"));
                                        }
                                    }
                                    cx.notify();
                                });
                            })
                            .detach();
                        }),
                ),
            );

        // F-045：0–32 滑杆（保留 6/10/14 快捷档）+ 临时值/默认值 + 后台预估。
        let threshold_editor = self.render_threshold_editor(
            this,
            similar_threshold,
            self.settings_threshold_draft,
            true,
            colors,
        );
        let similar_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("相似图片"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("默认阈值用于相似扫描与「查找相似」；滑杆先给临时值，确认后再设为默认。"),
            )
            .child(threshold_editor);

        let path_row = |label: &'static str, value: PathBuf, colors: UiColors| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .bg(colors.sunken)
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(label),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(colors.fg)
                        .child(value.display().to_string()),
                )
        };
        let info_row = |label: &'static str, value: String, color: Hsla| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .bg(colors.sunken)
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_xs()
                        .text_color(color)
                        .child(value),
                )
        };
        let this_add_source = this.clone();
        let mut source_rows = div().flex().flex_col().gap(px(6.));
        if roots.is_empty() {
            source_rows = source_rows.child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("还没有图库文件夹，添加后开始扫描。"),
            );
        } else {
            for root in roots {
                let root_for_remove = root.clone();
                let root_for_rename = root.clone();
                let root_for_rescan = root.clone();
                let this_remove = this.clone();
                let this_rename = this.clone();
                let this_rescan = this.clone();
                let status = self
                    .source_statuses
                    .iter()
                    .find(|status| &status.root == root)
                    .cloned()
                    .unwrap_or_else(|| SourceScanStatus {
                        root: root.clone(),
                        online: root.is_dir(),
                        ..SourceScanStatus::default()
                    });
                let last_scan_text = status
                    .last_scan
                    .map(relative_time)
                    .unwrap_or_else(|| "从未".to_string());
                let scan_counts_text = format!(
                    "最近 新增 {} · 更新 {} · 移除 {} · 失败 {}",
                    status.counts.imported,
                    status.counts.updated,
                    status.counts.removed,
                    status.counts.failed
                );
                let filter_summary = scan_filter_summary(&self.state.scan_filter(root));
                let asset_count = status.asset_count;
                let online = status.online;
                let error = status.error.clone();
                source_rows = source_rows.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(6.))
                        .bg(colors.sunken)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .children((!online).then(|| {
                                            div()
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .gap(px(3.))
                                                .child(
                                                    div()
                                                        .size(px(6.))
                                                        .rounded_full()
                                                        .bg(colors.danger),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(colors.danger)
                                                        .child("离线"),
                                                )
                                        }))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w(px(0.))
                                                .truncate()
                                                .text_xs()
                                                .text_color(colors.fg)
                                                .child(source_label(root, source_aliases)),
                                        ),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(colors.subtle)
                                        .child(root.display().to_string()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap(px(6.))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(if online {
                                                    colors.subtle
                                                } else {
                                                    colors.danger
                                                })
                                                .child(format!("上次扫描 {last_scan_text}")),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(colors.subtle)
                                                .child(format!("· {asset_count} 个资产")),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(colors.subtle)
                                                .child(scan_counts_text),
                                        ),
                                )
                                .children(error.map(|error| {
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(colors.danger)
                                        .child(format!("错误：{error}"))
                                }))
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(colors.subtle)
                                        .child(filter_summary),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(
                                    Button::new(
                                        SharedString::from(format!(
                                            "rescan-source-{}",
                                            root.display()
                                        )),
                                        "重扫",
                                    )
                                    .size(ControlSize::Sm)
                                    .on_click(move |_, _, cx| {
                                        let _ = this_rescan.update(cx, |app, cx| {
                                            app.rescan_source(root_for_rescan.clone(), cx);
                                        });
                                    }),
                                )
                                .child(
                                    Button::new(
                                        SharedString::from(format!(
                                            "rename-source-{}",
                                            root.display()
                                        )),
                                        "重命名",
                                    )
                                    .size(ControlSize::Sm)
                                    .on_click(move |_, _, cx| {
                                        let _ = this_rename.update(cx, |app, cx| {
                                            app.open_source_rename(
                                                root_for_rename.clone(),
                                                cx,
                                            );
                                        });
                                    }),
                                )
                                .child(
                                    Button::new(
                                        SharedString::from(format!(
                                            "remove-source-{}",
                                            root.display()
                                        )),
                                        "移除",
                                    )
                                    .size(ControlSize::Sm)
                                    .on_click(move |_, _, cx| {
                                        let _ = this_remove.update(cx, |app, cx| {
                                            app.source_remove_confirm =
                                                Some(root_for_remove.clone());
                                            cx.notify();
                                        });
                                    }),
                                ),
                        ),
                );
            }
        }
        let sources_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("本地来源"),
            )
            .child(source_rows)
            .child(
                div()
                    .flex()
                    .child(
                        Button::new("add-source", "添加文件夹")
                            .primary()
                            .on_click(move |_, _, cx| {
                                let _ = this_add_source
                                    .update(cx, |app, cx| app.choose_folder(cx));
                            }),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("移除来源只影响图库视图，不会删除磁盘文件。"),
            );

        let thresholds = self.tag_thresholds.clone().unwrap_or_default();
        let thresholds_loaded = self.tag_thresholds.is_some();
        let threshold_defaults = default_category_thresholds();
        let mut threshold_rows = div()
            .id("tag-thresholds")
            .aria_label("打标类别阈值")
            .flex()
            .flex_col()
            .gap(px(3.));
        for (category, label) in TAG_CATEGORY_LABELS {
            let current = thresholds
                .categories
                .get(category)
                .copied()
                .or_else(|| threshold_defaults.get(category).copied())
                .unwrap_or(0.2);
            let default = threshold_defaults.get(category).copied().unwrap_or(0.2);
            let minus_category = category.to_string();
            let plus_category = category.to_string();
            let reset_category = category.to_string();
            let this_minus = this.clone();
            let this_plus = this.clone();
            let this_reset = this.clone();
            threshold_rows = threshold_rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .w(px(64.))
                            .text_xs()
                            .text_color(colors.fg)
                            .child(label),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("tag-threshold-{category}")))
                            .aria_label(format!("{label}类别阈值 {current:.2}"))
                            .w(px(44.))
                            .text_xs()
                            .text_color(colors.muted)
                            .child(format!("{current:.2}")),
                    )
                    .child(
                        Button::new(
                            SharedString::from(format!("tag-threshold-{category}-minus")),
                            "−0.01",
                        )
                        .size(ControlSize::Sm)
                        .disabled(current <= 0.0)
                        .on_click(move |_, _, cx| {
                            let _ = this_minus.update(cx, |app, cx| {
                                app.step_category_threshold(&minus_category, -0.01, cx);
                            });
                        }),
                    )
                    .child(
                        Button::new(
                            SharedString::from(format!("tag-threshold-{category}-plus")),
                            "+0.01",
                        )
                        .size(ControlSize::Sm)
                        .disabled(current >= 1.0)
                        .on_click(move |_, _, cx| {
                            let _ = this_plus.update(cx, |app, cx| {
                                app.step_category_threshold(&plus_category, 0.01, cx);
                            });
                        }),
                    )
                    .child(
                        Button::new(
                            SharedString::from(format!("tag-threshold-{category}-reset")),
                            "重置",
                        )
                        .size(ControlSize::Sm)
                        .disabled((current - default).abs() < 0.0005)
                        .on_click(move |_, _, cx| {
                            let _ = this_reset.update(cx, |app, cx| {
                                app.reset_category_threshold(&reset_category, cx);
                            });
                        }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(format!("默认 {default:.2}")),
                    ),
            );
        }

        let mut overrides = thresholds.overrides.iter().collect::<Vec<_>>();
        overrides.sort_by(|(left, _), (right, _)| left.cmp(right));
        let override_count = overrides.len();
        let override_rows: AnyElement = if overrides.is_empty() {
            div()
                .text_xs()
                .text_color(colors.subtle)
                .child("暂无单标签覆盖；所有标签使用类别阈值。")
                .into_any_element()
        } else {
            let mut rows = div().flex().flex_col().gap(px(3.));
            for (tag, value) in overrides.into_iter().take(8) {
                let this_clear = this.clone();
                let tag_name = tag.clone();
                rows = rows.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(4.))
                        .bg(colors.sunken)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_xs()
                                .text_color(colors.fg)
                                .child(format!("#{tag} · {value:.2}")),
                        )
                        .child(
                            Button::new(
                                SharedString::from(format!("tag-override-clear-{tag}")),
                                "清除",
                            )
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ = this_clear.update(cx, |app, cx| {
                                    app.clear_tag_override(&tag_name, cx);
                                });
                            }),
                        ),
                );
            }
            if override_count > 8 {
                rows = rows.child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(format!("另有 {} 条覆盖未展开", override_count - 8)),
                );
            }
            rows.into_any_element()
        };

        let selected_target_count = if !self.selection.is_empty() {
            self.selection.len()
        } else if self.state.shared.lock().unwrap().selected.is_some() {
            1
        } else {
            0
        };
        let target_count = if selected_target_count > 0 {
            selected_target_count
        } else {
            self.view.visible_indices.len()
        };
        let this_recompute = this.clone();
        let this_apply_override = this.clone();
        let this_clear_override = this.clone();
        let override_name_field = tag_override_name.clone();
        let override_value_field = tag_override_value.clone();
        let clear_name_field = tag_override_name.clone();
        let override_inputs = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .w(px(220.))
                    .child(Input::new(&tag_override_name).least("标签名，如 long_hair")),
            )
            .child(
                div()
                    .w(px(110.))
                    .child(Input::new(&tag_override_value).least("0.00–1.00")),
            )
            .child(
                Button::new("tag-override-apply", "应用覆盖")
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        let name = override_name_field.read(cx).text().to_string();
                        let value = override_value_field.read(cx).text().to_string();
                        let _ = this_apply_override.update(cx, |app, cx| {
                            app.apply_tag_override(&name, &value, cx);
                        });
                    }),
            )
            .child(
                Button::new("tag-override-clear", "按标签名清除")
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        let name = clear_name_field.read(cx).text().to_string();
                        let _ = this_clear_override.update(cx, |app, cx| {
                            app.clear_tag_override(&name, cx);
                        });
                    }),
            );

        let queue = self.tag_queue_status.clone().unwrap_or_default();
        let queue_phase = tag_queue_phase(&queue);
        let queue_gpu = match queue.gpu_available {
            Some(true) => "GPU 可用",
            Some(false) => "GPU 不可用",
            None => "GPU 状态未知",
        };
        let queue_device = queue
            .device
            .clone()
            .unwrap_or_else(|| "未加载".to_string());
        let queue_status_rows = div()
            .id("tag-queue-status")
            .aria_label("打标队列状态")
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(info_row(
                "状态",
                format!(
                    "{queue_phase} · 等待 {} · 运行 {} · 失败 {} · 完成 {}",
                    queue.pending, queue.running, queue.error, queue.done
                ),
                if queue.error > 0 {
                    colors.warning
                } else {
                    colors.fg
                },
            ))
            .child(info_row(
                "本轮",
                format!(
                    "完成 {} / {} · 失败 {} · 吞吐 {} · ETA {}",
                    queue.completed,
                    queue.total,
                    queue.failed,
                    format_tag_throughput(queue.throughput_per_sec),
                    format_tag_eta(queue.eta_secs)
                ),
                colors.fg,
            ))
            .child(info_row(
                "设备",
                format!("{queue_device} · {queue_gpu}"),
                colors.muted,
            ));
        let queue_message = queue.message.clone().map(|message| {
            div()
                .text_xs()
                .text_color(colors.accent)
                .child(message)
        });
        let queue_last_error = queue.last_error.clone().map(|error| {
            let path = queue
                .last_error_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "未知路径".to_string());
            div()
                .truncate()
                .text_xs()
                .text_color(colors.danger)
                .child(format!("最近错误 · {path} · {error}"))
        });

        let this_pause = this.clone();
        let this_cancel = this.clone();
        let this_queue_refresh = this.clone();
        let this_cpu = this.clone();
        let this_retry = this.clone();
        let this_prioritize = this.clone();
        let this_retag_selected = this.clone();
        let this_retag_view = this.clone();
        let queue_actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(
                Button::new(
                    "tag-queue-pause",
                    if queue.paused {
                        "继续打标"
                    } else {
                        "暂停打标"
                    },
                )
                .primary()
                .disabled(queue.cancelling)
                .on_click(move |_, _, cx| {
                    let _ = this_pause.update(cx, |app, cx| app.toggle_tag_queue(cx));
                }),
            )
            .child(
                Button::new("tag-queue-cancel", "取消打标")
                    .size(ControlSize::Sm)
                    .disabled(queue.cancelling)
                    .on_click(move |_, _, cx| {
                        let _ = this_cancel.update(cx, |app, cx| app.cancel_tag_queue(cx));
                    }),
            )
            .child(
                Button::new("tag-queue-refresh", "刷新队列")
                    .size(ControlSize::Sm)
                    .icon(IconName::RefreshCw)
                    .on_click(move |_, _, cx| {
                        let _ = this_queue_refresh
                            .update(cx, |app, cx| app.refresh_tag_settings(cx));
                    }),
            )
            .child(
                Button::new(
                    "tag-queue-cpu-toggle",
                    if self.tag_cpu_allowed {
                        "允许 CPU 慢速打标：开"
                    } else {
                        "允许 CPU 慢速打标：关"
                    },
                )
                .size(ControlSize::Sm)
                .on_click(move |_, _, cx| {
                    let _ = this_cpu.update(cx, |app, cx| app.toggle_cpu_tagging(cx));
                }),
            )
            .child(
                Button::new("tag-queue-retry-failed", "重试失败")
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        let _ = this_retry.update(cx, |app, cx| app.retry_failed(cx));
                    }),
            )
            .child(
                Button::new("tag-prioritize-selected", "提升选中优先级")
                    .size(ControlSize::Sm)
                    .icon(IconName::ArrowUp)
                    .on_click(move |_, _, cx| {
                        let _ = this_prioritize
                            .update(cx, |app, cx| app.prioritize_selected(cx));
                    }),
            )
            .child(
                Button::new("tag-retag-selected", "重打标选中")
                    .size(ControlSize::Sm)
                    .icon(IconName::WandSparkles)
                    .on_click(move |_, _, cx| {
                        let _ = this_retag_selected
                            .update(cx, |app, cx| app.request_retag(true, cx));
                    }),
            )
            .child(
                Button::new("tag-retag-view", "重打标当前视图")
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        let _ = this_retag_view
                            .update(cx, |app, cx| app.request_retag(false, cx));
                    }),
            );

        let error_rows: AnyElement = if self.tag_job_errors.is_empty() {
            div()
                .text_xs()
                .text_color(colors.subtle)
                .child("最近失败：无")
                .into_any_element()
        } else {
            let mut rows = div().flex().flex_col().gap(px(3.));
            for error in &self.tag_job_errors {
                let file_name = error
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| error.path.display().to_string());
                rows = rows.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(4.))
                        .bg(colors.sunken)
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(colors.fg)
                                .child(format!(
                                    "#{} · 尝试 {} 次 · {}",
                                    error.job_id, error.attempts, file_name
                                )),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(colors.danger)
                                .child(
                                    error
                                        .last_error
                                        .clone()
                                        .unwrap_or_else(|| "没有错误详情".to_string()),
                                ),
                        ),
                );
            }
            rows.into_any_element()
        };

        let tag_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("打标与队列"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child(if thresholds_loaded {
                        "阈值改动只持久化设置；点击“重算”才按已存候选更新标签。"
                    } else {
                        "尚未读取到阈值，当前显示模型默认值；点击“刷新队列”重试。"
                    }),
            )
            .children(self.tag_settings_notice.clone().map(|notice| {
                div()
                    .text_xs()
                    .text_color(colors.accent)
                    .child(notice)
            }))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("类别阈值"),
            )
            .child(threshold_rows)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        Button::new(
                            "tag-recompute",
                            if self.tag_recompute_running {
                                "重算中…"
                            } else {
                                "重算当前视图/选中"
                            },
                        )
                        .size(ControlSize::Sm)
                        .disabled(self.tag_recompute_running)
                        .on_click(move |_, _, cx| {
                            let _ = this_recompute.update(cx, |app, cx| {
                                app.start_recompute_tag_thresholds(cx)
                            });
                        }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(format!("目标 {target_count} 张（选中优先）")),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(format!(
                        "候选只保存分数 ≥ {TAG_CANDIDATE_MIN_SCORE:.3} 的每类前若干名；阈值低于候选下限或超出已存 top-K 的资产需要重打标。"
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("单标签覆盖（{override_count}）")),
            )
            .child(override_inputs)
            .child(override_rows)
            .child(div().h(px(1.)).bg(colors.border))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("打标队列"),
            )
            .child(queue_status_rows)
            .children(queue_message)
            .children(queue_last_error)
            .child(queue_actions)
            .child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("暂停在当前批完成后生效；取消保留已完成结果；重打标会保留手工标签。"),
            )
            .child(error_rows);

        // 低置信度审核：按资产分组展示待审候选，支持单条与整张处理。
        let review_groups = group_reviews_by_asset(&self.tag_reviews);
        let review_total = self.tag_reviews.len();
        let this_review_refresh = this.clone();
        let review_actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(
                Button::new(
                    "tag-review-refresh",
                    if self.tag_review_running {
                        "读取中…"
                    } else {
                        "刷新待审候选"
                    },
                )
                .size(ControlSize::Sm)
                .icon(IconName::RefreshCw)
                .disabled(self.tag_review_running)
                .on_click(move |_, _, cx| {
                    let _ = this_review_refresh
                        .update(cx, |app, cx| app.refresh_tag_reviews(cx));
                }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(format!("待审候选 {review_total} 条")),
            );
        let review_rows: AnyElement = if review_groups.is_empty() {
            div()
                .text_xs()
                .text_color(colors.subtle)
                .child(if self.tag_review_running {
                    "正在读取待审候选…"
                } else {
                    "没有待审候选：所有低于阈值的候选都已处理。"
                })
                .into_any_element()
        } else {
            let mut rows = div().flex().flex_col().gap(px(6.));
            for (path, candidates) in review_groups {
                let file_name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                let mut tag_rows = div().flex().flex_col().gap(px(2.));
                for candidate in &candidates {
                    let this_accept = this.clone();
                    let this_ignore = this.clone();
                    let accept_path = path.clone();
                    let accept_tag = candidate.tag.clone();
                    let ignore_path = path.clone();
                    let ignore_tag = candidate.tag.clone();
                    let score = candidate.score;
                    let threshold = candidate.threshold;
                    let tag_name = candidate.tag.clone();
                    let category = candidate.category.clone();
                    tag_rows = tag_rows.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(8.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(colors.sunken)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_xs()
                                    .text_color(colors.fg)
                                    .child(format!("#{tag_name} · {}", tag_category_label(&category))),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.muted)
                                    .child(format!("分数 {score:.3} / 阈值 {threshold:.3}")),
                            )
                            .child(
                                Button::new(
                                    SharedString::from(format!(
                                        "tag-review-accept-{}-{accept_tag}",
                                        file_name
                                    )),
                                    "接受",
                                )
                                .size(ControlSize::Sm)
                                .on_click(move |_, _, cx| {
                                    let _ = this_accept.update(cx, |app, cx| {
                                        app.apply_tag_review(
                                            accept_path.clone(),
                                            accept_tag.clone(),
                                            TagReviewState::Accepted,
                                            cx,
                                        );
                                    });
                                }),
                            )
                            .child(
                                Button::new(
                                    SharedString::from(format!(
                                        "tag-review-ignore-{}-{ignore_tag}",
                                        file_name
                                    )),
                                    "忽略",
                                )
                                .size(ControlSize::Sm)
                                .on_click(move |_, _, cx| {
                                    let _ = this_ignore.update(cx, |app, cx| {
                                        app.apply_tag_review(
                                            ignore_path.clone(),
                                            ignore_tag.clone(),
                                            TagReviewState::Ignored,
                                            cx,
                                        );
                                    });
                                }),
                            ),
                    );
                }
                let this_accept_all = this.clone();
                let this_ignore_all = this.clone();
                let accept_all_path = path.clone();
                let ignore_all_path = path.clone();
                let candidate_count = candidates.len();
                rows = rows.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(colors.border)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .truncate()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.fg)
                                        .child(format!("{file_name} · {candidate_count} 条")),
                                )
                                .child(
                                    Button::new(
                                        SharedString::from(format!(
                                            "tag-review-accept-all-{file_name}"
                                        )),
                                        "整张接受",
                                    )
                                    .size(ControlSize::Sm)
                                    .on_click(move |_, _, cx| {
                                        let _ = this_accept_all.update(cx, |app, cx| {
                                            app.apply_asset_review(
                                                accept_all_path.clone(),
                                                TagReviewState::Accepted,
                                                cx,
                                            );
                                        });
                                    }),
                                )
                                .child(
                                    Button::new(
                                        SharedString::from(format!(
                                            "tag-review-ignore-all-{file_name}"
                                        )),
                                        "整张忽略",
                                    )
                                    .size(ControlSize::Sm)
                                    .on_click(move |_, _, cx| {
                                        let _ = this_ignore_all.update(cx, |app, cx| {
                                            app.apply_asset_review(
                                                ignore_all_path.clone(),
                                                TagReviewState::Ignored,
                                                cx,
                                            );
                                        });
                                    }),
                                ),
                        )
                        .child(tag_rows),
                );
            }
            rows.into_any_element()
        };
        let review_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("低置信度审核"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("分数低于有效阈值、尚未处理的候选。接受会写入标签，忽略会标记为已处理。"),
            )
            .children(self.tag_review_notice.clone().map(|notice| {
                div()
                    .text_xs()
                    .text_color(colors.accent)
                    .child(notice)
            }))
            .child(review_actions)
            .child(review_rows);

        let data_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("数据与缓存"),
            )
            .child(path_row("数据库", db_path, colors))
            .child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(format!("数据库大小 {}", format_bytes(db_bytes))),
            )
            .child(path_row("缩略图缓存", cache_dir, colors))
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child(cache_summary),
            )
            .child({
                let (label, color) = match kanalib_core::ffmpeg::status() {
                    Some(build) if build.license == kanalib_core::ffmpeg::License::Gpl => (
                        format!(
                            "FFmpeg · {} · GPL · 仅限本机使用，勿随包分发",
                            build.version
                        ),
                        colors.warning,
                    ),
                    Some(build) => (
                        format!(
                            "FFmpeg · {} · {}",
                            build.version,
                            build.license.label()
                        ),
                        colors.muted,
                    ),
                    None => ("FFmpeg · 未找到 · 使用内置解码回退".to_string(), colors.subtle),
                };
                div().text_xs().text_color(color).child(label)
            })
            .child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("超过磁盘预算时淘汰最少使用的缩略图组；当前可见页与最近访问的图不会被删，缺失的缓存会在滚动时按需重建。"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted)
                            .child("磁盘预算"),
                    )
                    .child(
                        Button::new("thumb-budget-less", "−")
                            .size(ControlSize::Sm)
                            .disabled(thumb_budget_gb <= 1)
                            .on_click(move |_, _, cx| {
                                let _ = this_budget_less.update(cx, |app, cx| {
                                    let value = app.thumb_budget_gb.saturating_sub(1);
                                    app.set_thumb_budget(value, cx);
                                });
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.fg)
                            .child(format!("{thumb_budget_gb} GB")),
                    )
                    .child(
                        Button::new("thumb-budget-more", "+")
                            .size(ControlSize::Sm)
                            .disabled(thumb_budget_gb >= 1000)
                            .on_click(move |_, _, cx| {
                                let _ = this_budget_more.update(cx, |app, cx| {
                                    let value = app.thumb_budget_gb.saturating_add(1);
                                    app.set_thumb_budget(value, cx);
                                });
                            }),
                    )
                    .child(
                        Button::new("thumb-budget-cleanup", "清理到预算")
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ = this_budget_cleanup.update(cx, |app, cx| {
                                    app.run_cache_budget_cleanup(true, cx);
                                });
                            }),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(format!(
                        "内存缩略图命中率 {cache_hit_rate:.1}% · {} 次命中 / {} 次未命中",
                        cache_hits, cache_misses
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(
                        cache_last_cleanup
                            .unwrap_or_else(|| "尚未执行预算淘汰".to_string()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(Button::new("open-data-dir", "打开数据目录").on_click(
                        move |_, _, _| {
                            open_path(&data_dir_for_open);
                        },
                    ))
                    .child(
                        Button::new("refresh-cache", "刷新统计")
                            .size(ControlSize::Sm)
                            .icon(IconName::RefreshCw)
                            .on_click(move |_, _, cx| {
                                let _ = this_refresh_cache
                                    .update(cx, |app, cx| app.refresh_cache_stats(cx));
                            }),
                    )
                    .child(
                        Button::new("cleanup-cache", "清理孤儿")
                            .size(ControlSize::Sm)
                            .disabled(orphan_count == 0)
                            .on_click(move |_, _, cx| {
                                let _ = this_cleanup_cache
                                    .update(cx, |app, cx| app.cleanup_cache(cx));
                            }),
                    ),
            );

        let plan_enabled = backup_plan.enabled;
        let plan_frequency = backup_plan.frequency;
        let plan_keep_files = backup_plan.keep_files;
        let plan_keep_days = backup_plan.keep_days;
        let daily_button = Button::new("backup-frequency-daily", "每天")
            .size(ControlSize::Sm)
            .disabled(!plan_enabled);
        let daily_button = if plan_frequency == backup::BackupFrequency::Daily {
            daily_button.primary()
        } else {
            daily_button
        }
        .on_click(move |_, _, cx| {
            let _ = this_backup_daily.update(cx, |app, cx| {
                app.set_backup_frequency(backup::BackupFrequency::Daily, cx);
            });
        });
        let weekly_button = Button::new("backup-frequency-weekly", "每周")
            .size(ControlSize::Sm)
            .disabled(!plan_enabled);
        let weekly_button = if plan_frequency == backup::BackupFrequency::Weekly {
            weekly_button.primary()
        } else {
            weekly_button
        }
        .on_click(move |_, _, cx| {
            let _ = this_backup_weekly.update(cx, |app, cx| {
                app.set_backup_frequency(backup::BackupFrequency::Weekly, cx);
            });
        });

        let mut backup_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("数据库备份"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("立即备份或按日/周自动备份数据库到数据目录下的 backups 子目录，并校验完整性；备份不包含缩略图缓存。"),
            )
            .child(path_row("备份目录", backup::backups_dir(&data_dir), colors))
            .child(
                div()
                    .text_xs()
                    .text_color(colors.fg)
                    .child(backup_plan_summary(&backup_plan)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        Button::new(
                            "backup-plan-toggle",
                            if plan_enabled {
                                "自动备份：开"
                            } else {
                                "自动备份：关"
                            },
                        )
                        .size(ControlSize::Sm)
                        .on_click(move |_, _, cx| {
                            let _ = this_backup_toggle
                                .update(cx, |app, cx| app.toggle_backup_plan(cx));
                        }),
                    )
                    .child(daily_button)
                    .child(weekly_button),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted)
                            .child("保留份数"),
                    )
                    .child(
                        Button::new("backup-keep-less", "−")
                            .size(ControlSize::Sm)
                            .disabled(!plan_enabled || plan_keep_files <= 1)
                            .on_click(move |_, _, cx| {
                                let _ = this_backup_keep_less.update(cx, |app, cx| {
                                    app.adjust_backup_keep_files(-1, cx);
                                });
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.fg)
                            .child(format!("{plan_keep_files} 份")),
                    )
                    .child(
                        Button::new("backup-keep-more", "+")
                            .size(ControlSize::Sm)
                            .disabled(!plan_enabled || plan_keep_files >= 365)
                            .on_click(move |_, _, cx| {
                                let _ = this_backup_keep_more.update(cx, |app, cx| {
                                    app.adjust_backup_keep_files(1, cx);
                                });
                            }),
                    )
                    .child(
                        Button::new(
                            "backup-keep-days-toggle",
                            if plan_keep_days.is_some() {
                                "按天清理：开"
                            } else {
                                "按天清理：关"
                            },
                        )
                        .size(ControlSize::Sm)
                        .disabled(!plan_enabled)
                        .on_click(move |_, _, cx| {
                            let _ = this_backup_days_toggle.update(cx, |app, cx| {
                                app.toggle_backup_keep_days(cx);
                            });
                        }),
                    ),
            );
        if let Some(days) = plan_keep_days {
            backup_section = backup_section.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted)
                            .child("仅保留最近"),
                    )
                    .child(
                        Button::new("backup-days-less", "−")
                            .size(ControlSize::Sm)
                            .disabled(days <= 1)
                            .on_click(move |_, _, cx| {
                                let _ = this_backup_days_less.update(cx, |app, cx| {
                                    app.adjust_backup_keep_days(-7, cx);
                                });
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.fg)
                            .child(format!("{days} 天")),
                    )
                    .child(
                        Button::new("backup-days-more", "+")
                            .size(ControlSize::Sm)
                            .disabled(days >= 3_650)
                            .on_click(move |_, _, cx| {
                                let _ = this_backup_days_more.update(cx, |app, cx| {
                                    app.adjust_backup_keep_days(7, cx);
                                });
                            }),
                    ),
            );
        }
        if let Some(pending) = &backup_pending {
            let cancel = this_backup_cancel.clone();
            let mut detail = format!("候选：{}", pending.candidate.display());
            if let Some(safety) = &pending.safety_backup {
                detail.push_str(&format!("\n当前库安全备份：{}", safety.display()));
            }
            if let Some(created) = pending.created_at {
                detail.push_str(&format!(
                    "\n请求时间：{}",
                    backup::format_utc_datetime(created)
                ));
            }
            detail.push_str(&format!("\n备份 schema 版本：{}", pending.user_version));
            backup_section = backup_section.child(
                div()
                    .id("backup-pending")
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(10.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(colors.warning)
                    .bg(colors.sunken)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.warning)
                            .child("待恢复备份 · 重启后生效"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.fg)
                            .child(detail),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted)
                            .child("取消只删除暂存请求与标记，当前数据库立即保持不变；不会删除候选文件或安全备份。"),
                    )
                    .child(
                        Button::new("backup-cancel-pending", "取消待恢复")
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ = cancel
                                    .update(cx, |app, cx| app.cancel_pending_restore(cx));
                            }),
                    ),
            );
        }
        backup_section = backup_section
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child(backup_next_text),
            )
            .children(backup_skipped.map(|text| {
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child(text)
            }))
            .children(backup_error.map(|text| {
                div()
                    .text_xs()
                    .text_color(colors.danger)
                    .child(format!("最近备份/恢复错误：{text}"))
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        Button::new(
                            "backup-now",
                            if self.backup_running {
                                "备份中…"
                            } else {
                                "立即备份"
                            },
                        )
                        .primary()
                        .disabled(self.backup_running)
                        .on_click(move |_, _, cx| {
                            let _ = this_backup.update(cx, |app, cx| app.start_backup(cx));
                        }),
                    )
                    .child(
                        Button::new("open-backups", "打开备份目录")
                            .size(ControlSize::Sm)
                            .icon(IconName::FolderOpen)
                            .on_click(move |_, _, cx| {
                                let _ =
                                    this_open_backups.update(cx, |app, cx| app.open_backups_dir(cx));
                            }),
                    )
                    .child(
                        Button::new("restore-backup", "恢复备份…")
                            .size(ControlSize::Sm)
                            .disabled(self.backup_running || backup_has_pending)
                            .on_click(move |_, _, cx| {
                                let _ = this_restore
                                    .update(cx, |app, cx| app.pick_restore_backup(cx));
                            }),
                    ),
            )
            .children(backup_last_text.map(|text| {
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child(text)
            }))
            .children(self.last_backup.clone().map(|summary| {
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child(summary)
            }))
            .child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("保留策略只清理本应用生成的 kanalib-*.db；安全备份与用户文件不会自动删除。恢复会先校验备份并保存当前库安全备份，重启 Kanalib 后应用。"),
            );

        let diagnostics_rows: AnyElement = if let Some(snapshot) = &self.diagnostic_snapshot {
            let mut rows = div().flex().flex_col().gap(px(4.));
            rows = rows.child(info_row("版本", snapshot.app_version.clone(), colors.fg));
            rows = rows.child(info_row(
                "数据目录",
                snapshot.data_dir.display().to_string(),
                colors.fg,
            ));
            rows = rows.child(info_row(
                "数据库",
                format!(
                    "{} · {}",
                    snapshot.db_path.display(),
                    format_bytes(snapshot.db_bytes)
                ),
                colors.fg,
            ));
            rows = rows.child(info_row(
                "缩略图缓存",
                format!(
                    "{} · {}",
                    snapshot.thumb_cache_dir.display(),
                    format_bytes(snapshot.thumb_cache_bytes)
                ),
                colors.fg,
            ));
            rows = rows.child(info_row(
                "资产总数",
                snapshot.asset_count.to_string(),
                colors.fg,
            ));
            rows = rows.child(info_row(
                "打标队列",
                format!(
                    "等待 {} · 运行 {} · 失败 {}",
                    snapshot.queue_pending, snapshot.queue_running, snapshot.queue_error
                ),
                colors.fg,
            ));
            rows = rows.child(info_row(
                "推理设备",
                snapshot
                    .sidecar_device
                    .clone()
                    .unwrap_or_else(|| "未知".to_string()),
                colors.fg,
            ));
            rows = rows.child(info_row(
                "最近扫描",
                snapshot
                    .last_scan
                    .map(|seconds| {
                        format!("{}（{}）", relative_time(seconds), format_date(seconds))
                    })
                    .unwrap_or_else(|| "从未".to_string()),
                colors.fg,
            ));
            rows = rows.child(info_row(
                "最近错误",
                snapshot
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "无".to_string()),
                if snapshot.last_error.is_some() {
                    colors.danger
                } else {
                    colors.fg
                },
            ));
            rows = rows.child(info_row(
                "日志",
                snapshot
                    .log_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "未启用".to_string()),
                colors.fg,
            ));
            if snapshot.sources.is_empty() {
                rows = rows.child(info_row("来源", "无".to_string(), colors.fg));
            } else {
                for source in &snapshot.sources {
                    let mut text = format!(
                        "{} · {} 个资产 · {}",
                        source_label(&source.root, source_aliases),
                        source.asset_count,
                        source
                            .last_scan
                            .map(relative_time)
                            .unwrap_or_else(|| "从未扫描".to_string())
                    );
                    if !source.online {
                        text.push_str(" · 离线");
                    }
                    if let Some(error) = &source.error {
                        text.push_str(&format!(" · 错误：{error}"));
                    }
                    rows = rows.child(info_row(
                        "来源",
                        text,
                        if source.online && source.error.is_none() {
                            colors.fg
                        } else {
                            colors.danger
                        },
                    ));
                }
            }
            rows.into_any_element()
        } else {
            div()
                .text_xs()
                .text_color(colors.subtle)
                .child("诊断信息尚未采集；重新打开设置页或点击“复制诊断信息”会刷新。")
                .into_any_element()
        };
        let diagnostics_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("诊断信息"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("只读快照：版本、数据目录、数据库与缓存大小、队列、来源状态与最近错误。"),
            )
            .child(diagnostics_rows)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        Button::new("copy-diagnostics", "复制诊断信息")
                            .size(ControlSize::Sm)
                            .icon(IconName::Copy)
                            .on_click(move |_, _, cx| {
                                let _ = this_copy_diagnostics
                                    .update(cx, |app, cx| app.copy_diagnostics(cx));
                            }),
                    )
                    .child(
                        Button::new("open-log-dir", "打开日志目录")
                            .size(ControlSize::Sm)
                            .icon(IconName::FolderOpen)
                            .on_click(move |_, _, cx| {
                                let _ =
                                    this_open_logs.update(cx, |app, cx| app.open_log_dir(cx));
                            }),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("诊断信息不包含凭据或完整环境变量；日志路径受 KANALIB_LOG_FILE 影响时以快照为准。"),
            );

        let safety_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("任务队列"),
            )
            .child(div().text_xs().text_color(colors.muted).child(format!(
                "等待 {queue_pending} · 运行 {queue_running} · 失败 {queue_error}"
            )))
            .child(div().text_xs().text_color(colors.muted).child(
                "打标任务持久化在 jobs 表；重启后自动恢复未完成任务，失败项可用侧栏“重试失败”。",
            ));

        let mut shortcut_rows = div().flex().flex_col().gap(px(4.));
        for (keys, description) in [
            ("Ctrl+K", "命令面板"),
            ("Ctrl+F", "聚焦搜索"),
            ("方向键 / Home / End", "移动选中项"),
            ("Enter / Space", "快速查看"),
            ("F", "切换收藏"),
            ("1–5 / 0", "设置 / 清除星级"),
            ("Ctrl+A", "全选当前可见"),
            ("Ctrl+Shift+1–8", "切换 8 种视图"),
            ("Esc", "清空选择"),
            ("双击 / 右键", "快速查看 / 上下文菜单"),
        ] {
            shortcut_rows = shortcut_rows.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.fg)
                            .child(description),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(keys),
                    ),
            );
        }
        let shortcut_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("键盘与鼠标"),
            )
            .child(shortcut_rows)
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("快捷键已绑定；方向键与快捷操作以网格获得焦点时生效。"),
            );

        let rules_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("安全规则"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("去重绝不自动删除；删除只进 Windows 系统回收站，且仅支持本地固定磁盘。"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("相似组不提供批量删除；受保护路径与只读来源不会被移入回收站。"),
            );

        // ---- F-063：扫描进度与结束摘要（只读快照，底栏同步展示）。 ----
        let scan_progress = self.state.scan_progress();
        let scan_summary = self.state.scan_summary();
        let scanning_now = self.state.shared.lock().unwrap().scanning;
        let mut scan_rows = div().flex().flex_col().gap(px(4.));
        scan_rows = scan_rows.child(info_row(
            "当前状态",
            if scanning_now {
                format!(
                    "扫描中 · {} · 吞吐 {} · ETA {}",
                    scan_progress_counts(&scan_progress),
                    if scan_progress.throughput > 0.0 {
                        format!("{:.1} 张/秒", scan_progress.throughput)
                    } else {
                        "—".to_string()
                    },
                    crate::settings::format_tag_eta(scan_progress.eta_secs)
                )
            } else {
                format!(
                    "空闲 · 最近阶段 {}",
                    scan_phase_label(scan_progress.phase)
                )
            },
            if scanning_now {
                colors.accent
            } else {
                colors.fg
            },
        ));
        if scanning_now {
            let source_text = scan_progress
                .source
                .as_ref()
                .map(|source| {
                    if scan_progress.source_total > 1 {
                        format!(
                            "{}/{} · {}",
                            scan_progress.source_index,
                            scan_progress.source_total,
                            source_label(source, source_aliases)
                        )
                    } else {
                        source_label(source, source_aliases)
                    }
                })
                .unwrap_or_else(|| "—".to_string());
            scan_rows = scan_rows.child(info_row("当前来源", source_text, colors.muted));
            if let Some(current) = &scan_progress.current {
                scan_rows = scan_rows.child(info_row(
                    "当前文件",
                    current
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| current.display().to_string()),
                    colors.subtle,
                ));
            }
        }
        if let Some(summary) = scan_summary.as_ref() {
            scan_rows = scan_rows.child(info_row(
                "上次扫描",
                format_scan_summary(summary, false),
                if summary.failed > 0 {
                    colors.warning
                } else {
                    colors.fg
                },
            ));
            for source in summary.sources.iter().take(8) {
                scan_rows = scan_rows.child(info_row(
                    "来源",
                    format!(
                        "{} · 新增 {} · 更新 {} · 跳过 {} · 移除 {} · 失败 {} · {:.1}s{}{}",
                        source_label(&source.root, source_aliases),
                        source.counts.imported,
                        source.counts.updated,
                        source.counts.skipped,
                        source.counts.removed,
                        source.counts.failed,
                        source.elapsed_secs,
                        if source.cancelled { " · 已停止" } else { "" },
                        source
                            .error
                            .as_ref()
                            .map(|error| format!(" · {error}"))
                            .unwrap_or_default()
                    ),
                    if source.error.is_some() {
                        colors.danger
                    } else {
                        colors.muted
                    },
                ));
            }
            if summary.sources.len() > 8 {
                scan_rows = scan_rows.child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(format!("另有 {} 个来源未展开", summary.sources.len() - 8)),
                );
            }
        } else {
            scan_rows = scan_rows.child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("尚未完成扫描；结束后这里会显示新增/更新/跳过/移除/失败与耗时。"),
            );
        }
        let scan_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("扫描进度与摘要"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("进度由核心线按节流写入；底栏只读快照，刷新不会拖慢扫描。"),
            )
            .child(scan_rows);

        // ---- F-032：来源排除/忽略规则编辑器 ----
        let mut filter_rows = div().flex().flex_col().gap(px(8.));
        if roots.is_empty() {
            filter_rows = filter_rows.child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("还没有来源；添加来源后才能配置排除规则。"),
            );
        }
        for root in roots {
            let is_open = self
                .scan_filter_draft
                .as_ref()
                .is_some_and(|draft| &draft.root == root);
            let summary = scan_filter_summary(&self.state.scan_filter(root));
            let this_toggle = this.clone();
            let root_toggle = root.clone();
            filter_rows = filter_rows.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .bg(colors.sunken)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(colors.fg)
                                    .child(source_label(root, source_aliases)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(colors.subtle)
                                    .child(summary),
                            ),
                    )
                    .child(
                        Button::new(
                            SharedString::from(format!(
                                "edit-scan-filter-{}",
                                root.display()
                            )),
                            if is_open { "收起" } else { "编辑排除规则" },
                        )
                        .size(ControlSize::Sm)
                        .on_click(move |_, _, cx| {
                            let _ = this_toggle.update(cx, |app, cx| {
                                app.open_scan_filter_editor(root_toggle.clone(), cx);
                            });
                        }),
                    ),
            );
            if !is_open {
                continue;
            }
            let Some(draft) = self.scan_filter_draft.as_ref() else {
                continue;
            };
            let globs = draft.config.exclude_globs.clone();
            let include_hidden = draft.config.include_hidden;
            let impact = draft.impact.clone();
            let notice = draft.notice.clone();
            let confirm_remove = draft.confirm_remove;
            let excluded_count = impact.as_ref().map_or(0, |impact| impact.excluded.len());
            let glob_field = window.use_keyed_state(
                SharedString::from(format!("kanalib-filter-glob-{}", root.display())),
                cx,
                |window, cx| TextInput::new(window, cx).placeholder("排除 glob，如 **/cache/**"),
            );
            let min_initial = draft
                .config
                .min_bytes
                .map(|value| value.to_string())
                .unwrap_or_default();
            let max_initial = draft
                .config
                .max_bytes
                .map(|value| value.to_string())
                .unwrap_or_default();
            let min_holder = window.use_keyed_state(
                SharedString::from(format!("kanalib-filter-min-{}", root.display())),
                cx,
                move |window, cx| {
                    let input =
                        cx.new(|cx| TextInput::new(window, cx).placeholder("最小字节，空=不限"));
                    if !min_initial.is_empty() {
                        input.update(cx, |input, cx| input.set_text(min_initial.clone(), cx));
                    }
                    input
                },
            );
            let min_field = min_holder.read(cx).clone();
            let max_holder = window.use_keyed_state(
                SharedString::from(format!("kanalib-filter-max-{}", root.display())),
                cx,
                move |window, cx| {
                    let input =
                        cx.new(|cx| TextInput::new(window, cx).placeholder("最大字节，空=不限"));
                    if !max_initial.is_empty() {
                        input.update(cx, |input, cx| input.set_text(max_initial.clone(), cx));
                    }
                    input
                },
            );
            let max_field = max_holder.read(cx).clone();
            let ignore_field = window.use_keyed_state(
                SharedString::from(format!("kanalib-ignore-glob-{}", root.display())),
                cx,
                |window, cx| TextInput::new(window, cx).placeholder("忽略 glob，如 *_tmp.png"),
            );
            let mut glob_chips = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.));
            if globs.is_empty() {
                glob_chips = glob_chips.child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child("还没有排除 glob"),
                );
            }
            for (index, glob) in globs.iter().enumerate() {
                let this_remove = this.clone();
                glob_chips = glob_chips.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .px(px(6.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .bg(colors.surface)
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors.fg)
                                .child(glob.clone()),
                        )
                        .child(
                            Button::new(
                                SharedString::from(format!(
                                    "filter-glob-remove-{}-{index}",
                                    root.display()
                                )),
                                "×",
                            )
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ = this_remove.update(cx, |app, cx| {
                                    app.remove_scan_filter_glob(index, cx);
                                });
                            }),
                        ),
                );
            }
            let this_hidden = this.clone();
            let this_add_glob = this.clone();
            let this_preview = this.clone();
            let this_keep = this.clone();
            let this_remove_hits = this.clone();
            let this_cancel = this.clone();
            let this_add_ignore = this.clone();
            let min_for_preview = min_field.clone();
            let max_for_preview = max_field.clone();
            let min_for_keep = min_field.clone();
            let max_for_keep = max_field.clone();
            let min_for_remove = min_field.clone();
            let max_for_remove = max_field.clone();
            let glob_for_add = glob_field.clone();
            let glob_for_clear = glob_field.clone();
            let ignore_for_add = ignore_field.clone();
            let ignore_for_clear = ignore_field.clone();
            let root_for_ignore = root.clone();
            let impact_rows = impact.as_ref().map(|impact| {
                let mut rows = div().flex().flex_col().gap(px(2.));
                rows = rows.child(
                    div()
                        .text_xs()
                        .text_color(colors.fg)
                        .child(format!(
                            "命中预览：已入库 {} 张 · 排除 {} 张 · 保留 {} 张",
                            impact.indexed,
                            impact.excluded.len(),
                            impact.kept
                        )),
                );
                for path in impact.excluded.iter().take(5) {
                    rows = rows.child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(colors.muted)
                            .child(path.display().to_string()),
                    );
                }
                if impact.excluded.len() > 5 {
                    rows = rows.child(
                        div()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(format!("另有 {} 张未展开", impact.excluded.len() - 5)),
                    );
                }
                rows
            });
            filter_rows = filter_rows.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted)
                            .child("草稿只影响下面的预览/保存；保存前不改动扫描配置。"),
                    )
                    .child(glob_chips)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .child(div().w(px(240.)).child(Input::new(&glob_field)))
                            .child(
                                Button::new("filter-glob-add", "添加 glob")
                                    .size(ControlSize::Sm)
                                    .on_click(move |_, _, cx| {
                                        let text = glob_for_add.read(cx).text().to_string();
                                        let _ = this_add_glob.update(cx, |app, cx| {
                                            app.add_scan_filter_glob(&text, cx);
                                        });
                                        glob_for_clear
                                            .update(cx, |input, cx| input.set_text("", cx));
                                    }),
                            )
                            .child(
                                Button::new(
                                    "filter-hidden-toggle",
                                    if include_hidden {
                                        "隐藏文件：包含"
                                    } else {
                                        "隐藏文件：排除"
                                    },
                                )
                                .size(ControlSize::Sm)
                                .on_click(move |_, _, cx| {
                                    let _ = this_hidden
                                        .update(cx, |app, cx| app.toggle_scan_filter_hidden(cx));
                                }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .child(div().w(px(150.)).child(Input::new(&min_field)))
                            .child(div().w(px(150.)).child(Input::new(&max_field)))
                            .child(
                                Button::new("filter-preview", "预览命中")
                                    .size(ControlSize::Sm)
                                    .icon(IconName::Eye)
                                    .on_click(move |_, _, cx| {
                                        let min = min_for_preview.read(cx).text().to_string();
                                        let max = max_for_preview.read(cx).text().to_string();
                                        let _ = this_preview.update(cx, |app, cx| {
                                            app.preview_scan_filter_draft(&min, &max, cx);
                                        });
                                    }),
                            )
                            .child(
                                Button::new("filter-save-keep", "保存 · 保留记录")
                                    .size(ControlSize::Sm)
                                    .on_click(move |_, _, cx| {
                                        let min = min_for_keep.read(cx).text().to_string();
                                        let max = max_for_keep.read(cx).text().to_string();
                                        let _ = this_keep.update(cx, |app, cx| {
                                            app.apply_scan_filter_draft(false, &min, &max, cx);
                                        });
                                    }),
                            )
                            .child(
                                Button::new(
                                    "filter-save-remove",
                                    if confirm_remove {
                                        format!("确认移出 {excluded_count} 张记录")
                                    } else {
                                        "保存 · 移出命中".to_string()
                                    },
                                )
                                .size(ControlSize::Sm)
                                .variant(ButtonVariant::Danger)
                                .on_click(move |_, _, cx| {
                                    let min = min_for_remove.read(cx).text().to_string();
                                    let max = max_for_remove.read(cx).text().to_string();
                                    let _ = this_remove_hits.update(cx, |app, cx| {
                                        app.apply_scan_filter_draft(true, &min, &max, cx);
                                    });
                                }),
                            )
                            .child(
                                Button::new("filter-editor-cancel", "取消")
                                    .size(ControlSize::Sm)
                                    .on_click(move |_, _, cx| {
                                        let _ = this_cancel.update(cx, |app, cx| {
                                            if app.scan_filter_draft.as_ref().is_some_and(|draft| {
                                                draft.confirm_remove
                                            }) {
                                                app.cancel_scan_filter_remove(cx);
                                            } else {
                                                app.close_scan_filter_editor(cx);
                                            }
                                        });
                                    }),
                            ),
                    )
                    .children(impact_rows)
                    .children(notice.map(|message| {
                        div()
                            .text_xs()
                            .text_color(colors.accent)
                            .child(message)
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .w(px(240.))
                                    .child(Input::new(&ignore_field).least("忽略 glob，如 *_tmp.png")),
                            )
                            .child(
                                Button::new("ignore-glob-add", "加入忽略列表")
                                    .size(ControlSize::Sm)
                                    .on_click(move |_, _, cx| {
                                        let text = ignore_for_add.read(cx).text().to_string();
                                        let root = root_for_ignore.clone();
                                        let _ = this_add_ignore.update(cx, |app, cx| {
                                            app.add_ignored_glob(root, text, cx);
                                        });
                                        ignore_for_clear
                                            .update(cx, |input, cx| input.set_text("", cx));
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.subtle)
                                    .child("忽略 glob 立即生效，可在下方「忽略列表」逐条恢复。"),
                            ),
                    ),
            );
        }
        let filter_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("排除与忽略规则"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("录入 glob 后先预览已入库命中，再选择保留记录或从图库移出；移出只删 DB，磁盘文件保留。"),
            )
            .child(filter_rows);

        // ---- F-037：忽略列表查看与恢复 ----
        let ignore_rules = self.state.ignore_rules();
        let ignore_total = ignore_rules.paths.len()
            + ignore_rules.sha1.len()
            + ignore_rules.globs.values().map(Vec::len).sum::<usize>();
        let mut ignore_rows = div().flex().flex_col().gap(px(4.));
        if ignore_total == 0 {
            ignore_rows = ignore_rows.child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("忽略列表为空。移除资产或添加忽略 glob 后会出现在这里。"),
            );
        } else {
            if !ignore_rules.paths.is_empty() {
                ignore_rows = ignore_rows.child(
                    div()
                        .text_xs()
                        .text_color(colors.fg)
                        .child(format!("路径忽略 · {} 条", ignore_rules.paths.len())),
                );
                for (index, path) in ignore_rules.paths.iter().take(12).enumerate() {
                    let this_restore = this.clone();
                    let restore_path = PathBuf::from(path);
                    ignore_rows = ignore_rows.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .px(px(8.))
                            .py(px(3.))
                            .rounded(px(4.))
                            .bg(colors.sunken)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_xs()
                                    .text_color(colors.muted)
                                    .child(path.clone()),
                            )
                            .child(
                                Button::new(
                                    SharedString::from(format!("restore-ignore-path-{index}")),
                                    "恢复",
                                )
                                .size(ControlSize::Sm)
                                .on_click(move |_, _, cx| {
                                    let path = restore_path.clone();
                                    let _ = this_restore.update(cx, |app, cx| {
                                        app.restore_ignored_paths(vec![path], cx);
                                    });
                                }),
                            ),
                    );
                }
                if ignore_rules.paths.len() > 12 {
                    ignore_rows = ignore_rows.child(
                        div()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(format!(
                                "另有 {} 条路径未展开",
                                ignore_rules.paths.len() - 12
                            )),
                    );
                }
            }
            if !ignore_rules.sha1.is_empty() {
                ignore_rows = ignore_rows.child(
                    div()
                        .text_xs()
                        .text_color(colors.fg)
                        .child(format!("内容哈希忽略 · {} 条", ignore_rules.sha1.len())),
                );
                for (index, hash) in ignore_rules.sha1.iter().take(8).enumerate() {
                    let this_restore = this.clone();
                    let restore_hash = hash.clone();
                    let short = hash.chars().take(12).collect::<String>();
                    ignore_rows = ignore_rows.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .px(px(8.))
                            .py(px(3.))
                            .rounded(px(4.))
                            .bg(colors.sunken)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_xs()
                                    .text_color(colors.muted)
                                    .child(format!("{short}…")),
                            )
                            .child(
                                Button::new(
                                    SharedString::from(format!("restore-ignore-hash-{index}")),
                                    "恢复",
                                )
                                .size(ControlSize::Sm)
                                .on_click(move |_, _, cx| {
                                    let hash = restore_hash.clone();
                                    let _ = this_restore.update(cx, |app, cx| {
                                        app.restore_ignored_hashes(vec![hash], cx);
                                    });
                                }),
                            ),
                    );
                }
                if ignore_rules.sha1.len() > 8 {
                    ignore_rows = ignore_rows.child(
                        div()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(format!(
                                "另有 {} 条哈希未展开",
                                ignore_rules.sha1.len() - 8
                            )),
                    );
                }
            }
            for (root, globs) in &ignore_rules.globs {
                let root_path = PathBuf::from(root);
                ignore_rows = ignore_rows.child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(colors.fg)
                        .child(format!(
                            "忽略 glob · {}（{} 条）",
                            source_label(&root_path, source_aliases),
                            globs.len()
                        )),
                );
                for (index, glob) in globs.iter().take(12).enumerate() {
                    let this_restore = this.clone();
                    let restore_root = root_path.clone();
                    let restore_glob = glob.clone();
                    ignore_rows = ignore_rows.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .px(px(8.))
                            .py(px(3.))
                            .rounded(px(4.))
                            .bg(colors.sunken)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_xs()
                                    .text_color(colors.muted)
                                    .child(glob.clone()),
                            )
                            .child(
                                Button::new(
                                    SharedString::from(format!(
                                        "restore-ignore-glob-{}-{index}",
                                        root_path.display()
                                    )),
                                    "恢复",
                                )
                                .size(ControlSize::Sm)
                                .on_click(move |_, _, cx| {
                                    let root = restore_root.clone();
                                    let glob = restore_glob.clone();
                                    let _ = this_restore.update(cx, |app, cx| {
                                        app.restore_ignored_glob(root, glob, cx);
                                    });
                                }),
                            ),
                    );
                }
                if globs.len() > 12 {
                    ignore_rows = ignore_rows.child(
                        div()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(format!("另有 {} 条 glob 未展开", globs.len() - 12)),
                    );
                }
            }
            let this_restore_all = this.clone();
            let this_rescan = this.clone();
            ignore_rows = ignore_rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        Button::new("restore-all-ignores", "全部恢复")
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ = this_restore_all
                                    .update(cx, |app, cx| app.restore_all_ignores(cx));
                            }),
                    )
                    .child(
                        Button::new("ignores-rescan", "重新扫描图库")
                            .size(ControlSize::Sm)
                            .icon(IconName::RefreshCw)
                            .on_click(move |_, _, cx| {
                                let _ = this_rescan.update(cx, |app, cx| {
                                    app.request_rescan(RescanOrigin::Library, cx)
                                });
                            }),
                    ),
            );
        }
        let ignore_section = div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("忽略列表"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("路径 / 内容哈希 / glob 三种粒度持久化在 settings JSON；恢复后需重扫才会重新导入，磁盘文件从未被删除。"),
            )
            .child(ignore_rows);

        let column_width = (width - page_padding * 2.).min(px(SETTINGS_MAX_WIDTH));

        div()
            .w(width)
            .h_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .child(
                div()
                    .h(px(58.))
                    .px(page_padding)
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("设置"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.muted)
                                    .child("数据、去重与安全"),
                            ),
                    )
                    .child(
                        Button::new("settings-back", "返回图库").on_click(move |_, _, cx| {
                            let _ = this_back.update(cx, |app, cx| {
                                app.screen = Screen::Library;
                                cx.notify();
                            });
                        }),
                    ),
            )
            .children(banner)
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .px(page_padding)
                    .py(px(24.))
                    .child(
                        div()
                            .w(column_width)
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .child(tag_section)
                            .child(review_section)
                            .child(sources_section)
                            .child(protected_section)
                            .child(similar_section)
                            .child(data_section)
                            .child(backup_section)
                            .child(diagnostics_section)
                            .child(safety_section)
                            .child(shortcut_section)
                            .child(rules_section)
                            .child(scan_section)
                            .child(filter_section)
                            .child(ignore_section),
                    ),
            )
            .into_any_element()
    }

}
