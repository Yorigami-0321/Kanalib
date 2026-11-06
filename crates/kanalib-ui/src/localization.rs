use ely_gpui_component::i18n::{self, I18n};
use gpui::{App, SharedString};
use std::sync::atomic::{AtomicBool, Ordering};

mod extra;

/// 项目侧语言设置键；保存在 settings JSON，与 vendor 内置目录分离。
pub(crate) const LOCALE_SETTING: &str = "ui.locale";

/// 第一批支持的语言。语言名按本语言书写，不进目录。
pub(crate) const SUPPORTED_LOCALES: [(&str, &str); 2] = [("zh-CN", "中文"), ("en-US", "English")];

/// 项目侧中文字典。键名规则：`区域.用途`，小写点分；占位符用 `{name}`。
pub(crate) const ZH_CN: &[(&str, &str)] = &[
    ("nav.library", "图库"),
    ("nav.duplicates", "重复项"),
    ("nav.tags", "标签"),
    ("nav.theme", "主题"),
    ("nav.settings", "设置"),
    ("page.tags", "标签管理"),
    ("page.settings", "设置"),
    ("page.duplicates", "重复项"),
    ("page.similar", "查找相似"),
    ("page.compare", "图片对比"),
    ("titlebar.search_placeholder", "搜索 / tag: / character: / size:>10mb"),
    ("titlebar.sort", "排序"),
    ("titlebar.view", "查看"),
    ("titlebar.export", "导出"),
    ("titlebar.history", "最近查询 {n}"),
    ("titlebar.history_empty", "还没有最近查询"),
    ("titlebar.history_clear", "清空最近查询"),
    ("titlebar.syntax_help", "搜索语法帮助"),
    ("settings.title", "设置"),
    ("settings.subtitle", "数据、去重与安全"),
    ("settings.language", "语言"),
    ("sidebar.explorer", "资源管理器"),
    ("sidebar.tags_nav", "标签导航"),
    ("sidebar.settings", "设置"),
    ("sidebar.duplicates", "重复项"),
    ("sidebar.section.sources", "来源"),
    ("sidebar.section.filters", "筛选"),
    ("sidebar.section.ratings", "评级"),
    ("sidebar.section.groups", "自动归类"),
    ("sidebar.section.tags", "标签"),
    ("sidebar.section.collections", "智能集合"),
    ("sidebar.section.albums", "相册"),
    ("sidebar.section.organize", "整理"),
    ("sidebar.filter.all", "全部图片"),
    ("sidebar.filter.untagged", "未打标"),
    ("sidebar.filter.favorites", "收藏"),
    ("sidebar.filter.failed", "失败"),
    ("sidebar.filter.damaged", "损坏"),
    ("sidebar.filter.rating", "评级 {rating}"),
    ("sidebar.tags.sort_usage", "按使用次数排序"),
    ("sidebar.tags.sort_name", "按名称排序"),
    ("sidebar.tags.manage", "标签管理"),
    ("sidebar.tags.summary", "有使用 {used} / 共 {total} / 已屏蔽 {blocked}"),
    ("sidebar.tags.search_placeholder", "筛选标签（有使用）"),
    ("sidebar.tags.category.general", "通用"),
    ("sidebar.tags.category.character", "角色"),
    ("sidebar.tags.category.copyright", "作品"),
    ("sidebar.tags.category.style", "风格"),
    ("sidebar.tags.category.meta", "元数据"),
    ("sidebar.tags.category.manual", "手工"),
    ("sidebar.tags.empty_category", "这个类别还没有标签"),
    ("sidebar.tags.empty_search", "没有匹配的标签"),
    ("sidebar.tags.collapse", "收起"),
    ("sidebar.tags.more", "查看更多（{n}）"),
    ("status.error", "错误 · {text}"),
    ("status.retry", "重试"),
    ("status.retry_failed", "重试失败：{error}"),
    ("status.scanning_stop", "扫描中 · 点击停止"),
    ("status.searching", "搜索中…"),
    ("status.pending_restore", "待恢复备份 · 重启后生效"),
    ("status.total", "{n} 张"),
    ("status.tagged", "已打标 {n}"),
    ("status.untagged", "未打标 {n}"),
    ("status.failed", "失败 {n}"),
    ("status.damaged", "损坏 {n}"),
    ("status.queue_idle", "队列空闲"),
    ("status.queue", "队列 等待 {waiting} · 运行 {running} · 失败 {failed}"),
    ("status.device_missing", "推理设备 · 未加载"),
    ("status.palette", "命令面板"),
    ("status.palette_shortcut", "命令面板 (Ctrl+K)"),
    ("scan.phase.idle", "空闲"),
    ("scan.phase.discovering", "遍历"),
    ("scan.phase.hashing", "哈希"),
    ("scan.phase.thumbnails", "缩略图"),
    ("scan.phase.tagging", "打标"),
    ("scan.phase.reconciling", "对账"),
    ("scan.phase.done", "完成"),
    ("scan.phase.failed", "失败"),
    ("scan.progress", "{phase} {done}/{total}"),
    ("scan.summary_compact", "上次扫描 · 新增 {n}"),
    ("scan.summary", "上次扫描 · 新增 {imported} · 更新 {updated} · 跳过 {skipped} · 移除 {removed} · 失败 {failed} · {elapsed}{stopped}"),
    ("scan.stopped", " · 已停止"),
    ("scan.throughput", "{rate} 张/秒"),
    ("scan.eta", "剩余 {eta}"),
    ("scan.source_multi", "来源 {index}/{total} · {source}"),
    ("scan.source", "来源 · {source}"),
    ("scan.idle_recent", "空闲 · 最近阶段 {phase}"),
    ("scan.current_scanning", "扫描中 · {progress} · 吞吐 {rate} · ETA {eta}"),
    ("status.undo", "可撤销 {n}"),
    ("status.undo_empty", "没有可撤销的操作"),
    ("status.undo_aria_labeled", "撤销「{label}」· {base} · Ctrl+Z"),
    ("status.undo_aria", "{base} · Ctrl+Z"),
    ("status.undo_done", "已撤销 · {label}"),
    ("status.undo_failed", "撤销失败：{error}"),
    ("status.undo_status", "已撤销「{label}」· 涉及 {n} 张"),
    ("undo.favorite", "收藏"),
    ("undo.unfavorite", "取消收藏"),
    ("undo.clear_stars", "清除星级"),
    ("undo.set_stars", "设置 {n} 星"),
    ("undo.add_tag", "添加标签 #{name}"),
    ("undo.remove_tag", "移除标签 #{name}"),
    ("group.original", "原创"),
    ("group.non_anime", "非动漫/实景"),
    ("group.screenshots", "截图/文本"),
    ("group.work", "作品: {name}"),
    ("common.back_to_library", "返回图库"),
];

/// 项目侧英文字典；键与中文目录一一对应。
pub(crate) const EN_US: &[(&str, &str)] = &[
    ("nav.library", "Library"),
    ("nav.duplicates", "Duplicates"),
    ("nav.tags", "Tags"),
    ("nav.theme", "Theme"),
    ("nav.settings", "Settings"),
    ("page.tags", "Tag manager"),
    ("page.settings", "Settings"),
    ("page.duplicates", "Duplicates"),
    ("page.similar", "Find similar"),
    ("page.compare", "Compare"),
    ("titlebar.search_placeholder", "Search / tag: / character: / size:>10mb"),
    ("titlebar.sort", "Sort"),
    ("titlebar.view", "View"),
    ("titlebar.export", "Export"),
    ("titlebar.history", "Recent searches {n}"),
    ("titlebar.history_empty", "No recent searches"),
    ("titlebar.history_clear", "Clear recent searches"),
    ("titlebar.syntax_help", "Search syntax help"),
    ("settings.title", "Settings"),
    ("settings.subtitle", "Data, dedup and safety"),
    ("settings.language", "Language"),
    ("sidebar.explorer", "Explorer"),
    ("sidebar.tags_nav", "Tag navigation"),
    ("sidebar.settings", "Settings"),
    ("sidebar.duplicates", "Duplicates"),
    ("sidebar.section.sources", "Sources"),
    ("sidebar.section.filters", "Filters"),
    ("sidebar.section.ratings", "Ratings"),
    ("sidebar.section.groups", "Auto groups"),
    ("sidebar.section.tags", "Tags"),
    ("sidebar.section.collections", "Smart collections"),
    ("sidebar.section.albums", "Albums"),
    ("sidebar.section.organize", "Organize"),
    ("sidebar.filter.all", "All images"),
    ("sidebar.filter.untagged", "Untagged"),
    ("sidebar.filter.favorites", "Favorites"),
    ("sidebar.filter.failed", "Failed"),
    ("sidebar.filter.damaged", "Damaged"),
    ("sidebar.filter.rating", "Rating {rating}"),
    ("sidebar.tags.sort_usage", "Sort by usage"),
    ("sidebar.tags.sort_name", "Sort by name"),
    ("sidebar.tags.manage", "Manage tags"),
    ("sidebar.tags.summary", "Used {used} / Total {total} / Blocked {blocked}"),
    ("sidebar.tags.search_placeholder", "Filter tags (used)"),
    ("sidebar.tags.category.general", "General"),
    ("sidebar.tags.category.character", "Character"),
    ("sidebar.tags.category.copyright", "Copyright"),
    ("sidebar.tags.category.style", "Style"),
    ("sidebar.tags.category.meta", "Metadata"),
    ("sidebar.tags.category.manual", "Manual"),
    ("sidebar.tags.empty_category", "No tags in this category"),
    ("sidebar.tags.empty_search", "No matching tags"),
    ("sidebar.tags.collapse", "Show less"),
    ("sidebar.tags.more", "Show more ({n})"),
    ("status.error", "Error · {text}"),
    ("status.retry", "Retry"),
    ("status.retry_failed", "Retry failed: {error}"),
    ("status.scanning_stop", "Scanning · click to stop"),
    ("status.searching", "Searching…"),
    ("status.pending_restore", "Backup restore pending · takes effect after restart"),
    ("status.total", "{n} images"),
    ("status.tagged", "Tagged {n}"),
    ("status.untagged", "Untagged {n}"),
    ("status.failed", "Failed {n}"),
    ("status.damaged", "Damaged {n}"),
    ("status.queue_idle", "Queue idle"),
    ("status.queue", "Queue waiting {waiting} · running {running} · failed {failed}"),
    ("status.device_missing", "Inference device · not loaded"),
    ("status.palette", "Command palette"),
    ("status.palette_shortcut", "Command palette (Ctrl+K)"),
    ("scan.phase.idle", "Idle"),
    ("scan.phase.discovering", "Discovering"),
    ("scan.phase.hashing", "Hashing"),
    ("scan.phase.thumbnails", "Thumbnails"),
    ("scan.phase.tagging", "Tagging"),
    ("scan.phase.reconciling", "Reconciling"),
    ("scan.phase.done", "Done"),
    ("scan.phase.failed", "Failed"),
    ("scan.progress", "{phase} {done}/{total}"),
    ("scan.summary_compact", "Last scan · added {n}"),
    ("scan.summary", "Last scan · added {imported} · updated {updated} · skipped {skipped} · removed {removed} · failed {failed} · {elapsed}{stopped}"),
    ("scan.stopped", " · stopped"),
    ("scan.throughput", "{rate} images/s"),
    ("scan.eta", "ETA {eta}"),
    ("scan.source_multi", "Source {index}/{total} · {source}"),
    ("scan.source", "Source · {source}"),
    ("scan.idle_recent", "Idle · last phase {phase}"),
    ("scan.current_scanning", "Scanning · {progress} · {rate} · ETA {eta}"),
    ("status.undo", "Undo {n}"),
    ("status.undo_empty", "Nothing to undo"),
    ("status.undo_aria_labeled", "Undo “{label}” · {base} · Ctrl+Z"),
    ("status.undo_aria", "{base} · Ctrl+Z"),
    ("status.undo_done", "Undone · {label}"),
    ("status.undo_failed", "Undo failed: {error}"),
    ("status.undo_status", "Undone “{label}” · {n} items affected"),
    ("undo.favorite", "Favorite"),
    ("undo.unfavorite", "Unfavorite"),
    ("undo.clear_stars", "Clear rating"),
    ("undo.set_stars", "Set {n} stars"),
    ("undo.add_tag", "Add tag #{name}"),
    ("undo.remove_tag", "Remove tag #{name}"),
    ("group.original", "Original"),
    ("group.non_anime", "Non-anime / real"),
    ("group.screenshots", "Screenshots / text"),
    ("group.work", "Work: {name}"),
    ("common.back_to_library", "Back to library"),
];

/// 安装项目目录并保留 vendor 内置表作为未迁移键与组件默认值的来源。
fn combine(
    base: &[(&'static str, &'static str)],
    parts: &[&'static [(&'static str, &'static str)]],
) -> Vec<(&'static str, &'static str)> {
    let mut all = base.to_vec();
    for part in parts {
        all.extend_from_slice(part);
    }
    let mut seen = std::collections::HashSet::new();
    all.retain(|(key, _)| seen.insert(*key));
    all
}

pub(crate) fn combined_zh() -> Vec<(&'static str, &'static str)> {
    combine(
        ZH_CN,
        &[
            extra::app::ZH,
            extra::compare::ZH,
            extra::dedup::ZH,
            extra::inspector::ZH,
            extra::misc::ZH,
            extra::settings::ZH,
            extra::sidebar::ZH,
            extra::status::ZH,
            extra::tags::ZH,
            extra::viewer::ZH,
            extra::views::ZH,
        ],
    )
}

pub(crate) fn combined_en() -> Vec<(&'static str, &'static str)> {
    combine(
        EN_US,
        &[
            extra::app::EN,
            extra::compare::EN,
            extra::dedup::EN,
            extra::inspector::EN,
            extra::misc::EN,
            extra::settings::EN,
            extra::sidebar::EN,
            extra::status::EN,
            extra::tags::EN,
            extra::viewer::EN,
            extra::views::EN,
        ],
    )
}

static ENGLISH: AtomicBool = AtomicBool::new(false);

/// 无法注入 `cx` 的纯标签/格式化 helper 用进程级语言开关。
pub(crate) fn is_english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

pub(crate) fn install(cx: &mut App) {
    ENGLISH.store(false, Ordering::Relaxed);
    let zh = combined_zh();
    let en = combined_en();
    cx.set_global(
        I18n::new("zh-CN")
            .catalog("zh-CN", &zh)
            .catalog("en-US", &en),
    );
}

pub(crate) fn is_supported(tag: &str) -> bool {
    SUPPORTED_LOCALES.iter().any(|(supported, _)| *supported == tag)
}

pub(crate) fn set_locale(tag: &str, cx: &mut App) {
    if is_supported(tag) {
        ENGLISH.store(tag == "en-US", Ordering::Relaxed);
        I18n::set_locale(tag, cx);
    } else {
        log::warn!("i18n: unsupported project locale {tag}; keeping current");
    }
}

/// 启动时应用保存的语言；未设置或不受支持时保持安装时的默认值。
pub(crate) fn apply_saved(state: &kanalib_core::state::AppState, cx: &mut App) {
    if let Some(tag) = state.setting(LOCALE_SETTING) {
        set_locale(&tag, cx);
    }
}

pub(crate) fn current(cx: &App) -> &'static str {
    cx.global::<I18n>().locale().tag
}

pub(crate) fn t(cx: &App, key: &str) -> SharedString {
    i18n::text(cx, key, &[])
}


/// 核心 `Filter::label` 是中文数据；UI 渲染时用目录映射，数据值保留原文。
pub(crate) fn filter_label(cx: &App, filter: &kanalib_core::model::Filter) -> String {
    use kanalib_core::model::Filter;
    match filter {
        Filter::All => t(cx, "sidebar.filter.all").to_string(),
        Filter::Untagged => t(cx, "sidebar.filter.untagged").to_string(),
        Filter::Failed => t(cx, "sidebar.filter.failed").to_string(),
        Filter::Damaged => t(cx, "sidebar.filter.damaged").to_string(),
        Filter::Favorites => t(cx, "sidebar.filter.favorites").to_string(),
        Filter::Rating(rating) => t_args(cx, "sidebar.filter.rating", &[("rating", rating)]).to_string(),
        Filter::Group(group) => localize_group_name(cx, group),
        Filter::Tag(tag) => format!("#{tag}"),
        Filter::Source(path) => path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string()),
    }
}
pub(crate) fn t_args(cx: &App, key: &str, args: &[(&str, &str)]) -> SharedString {
    i18n::text(cx, key, args)
}

/// 核心写入的结构化状态：有 key 走目录，无 key 保留原文（备份通知/UI 覆盖）。
pub(crate) fn core_status(cx: &App, status: &kanalib_core::model::StatusMessage) -> SharedString {
    match status.key() {
        Some(key) => {
            let args = status
                .args()
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect::<Vec<_>>();
            t_args(cx, key, &args)
        }
        None => SharedString::from(status.text()),
    }
}

/// 核心撤销栈记录的是中文动作标签；在 UI 层按固定四种语法映射，未知标签原样返回。
pub(crate) fn undo_action_label(cx: &App, label: &str) -> String {
    let text = match label {
        "收藏" => t(cx, "undo.favorite"),
        "取消收藏" => t(cx, "undo.unfavorite"),
        "清除星级" => t(cx, "undo.clear_stars"),
        _ => {
            if let Some(stars) = label
                .strip_prefix("设置 ")
                .and_then(|rest| rest.strip_suffix(" 星"))
            {
                t_args(cx, "undo.set_stars", &[("n", stars)])
            } else if let Some(name) = label.strip_prefix("添加标签 #") {
                t_args(cx, "undo.add_tag", &[("name", name)])
            } else if let Some(name) = label.strip_prefix("移除标签 #") {
                t_args(cx, "undo.remove_tag", &[("name", name)])
            } else {
                return label.to_string();
            }
        }
    };
    text.to_string()
}

/// 自动归类名由核心生成；只映射已知固定名与「作品:」前缀，其余保留数据原文。
pub(crate) fn localize_group_name(cx: &App, name: &str) -> String {
    let text = match name {
        "原创" => t(cx, "group.original"),
        "非动漫/实景" => t(cx, "group.non_anime"),
        "截图/文本" => t(cx, "group.screenshots"),
        _ => {
            if let Some(work) = name.strip_prefix("作品: ") {
                t_args(cx, "group.work", &[("name", work)])
            } else {
                return name.to_string();
            }
        }
    };
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalogs_hold_the_same_keys() {
        let zh_all = combined_zh();
        let en_all = combined_en();
        let zh: HashSet<_> = zh_all.iter().map(|(key, _)| *key).collect();
        let en: HashSet<_> = en_all.iter().map(|(key, _)| *key).collect();
        assert_eq!(zh, en);
    }
}
