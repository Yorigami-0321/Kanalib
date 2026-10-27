//! F-032 扫描排除规则：由任务 C 实现（按来源根目录应用 glob/隐藏/大小过滤）。
//! 配置结构见 [`crate::model::ScanFilterConfig`]。

/// settings 表中的配置键；值为 `{ root: ScanFilterConfig }` 的 JSON 对象。
pub const SCAN_FILTERS_SETTING_KEY: &str = "scan.filters";
