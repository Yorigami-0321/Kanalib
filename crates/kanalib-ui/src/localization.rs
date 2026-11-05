use ely_gpui_component::i18n::{self, I18n};
use gpui::{App, SharedString};

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
    ("common.back_to_library", "Back to library"),
];

/// 安装项目目录并保留 vendor 内置表作为未迁移键与组件默认值的来源。
pub(crate) fn install(cx: &mut App) {
    cx.set_global(
        I18n::new("zh-CN")
            .catalog("zh-CN", ZH_CN)
            .catalog("en-US", EN_US),
    );
}

pub(crate) fn is_supported(tag: &str) -> bool {
    SUPPORTED_LOCALES.iter().any(|(supported, _)| *supported == tag)
}

pub(crate) fn set_locale(tag: &str, cx: &mut App) {
    if is_supported(tag) {
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

pub(crate) fn t_args(cx: &App, key: &str, args: &[(&str, &str)]) -> SharedString {
    i18n::text(cx, key, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalogs_hold_the_same_keys() {
        let zh: HashSet<_> = ZH_CN.iter().map(|(key, _)| *key).collect();
        let en: HashSet<_> = EN_US.iter().map(|(key, _)| *key).collect();
        assert_eq!(zh, en);
        assert_eq!(ZH_CN.len(), EN_US.len());
    }
}
