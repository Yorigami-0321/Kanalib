use crate::app::{KanalibApp, UiRetryAction, push_toast};
use crate::components::*;
use crate::format::*;
use crate::localization::{t, t_args};
use crate::theme::*;
use crate::views::GRID_CONTEXT;
use crate::*;

/// 核心生成的中文建议原因在 UI 层映射；未知原因保留原文。
fn localize_dedup_reason(cx: &App, reason: &str) -> String {
    let reason = reason.trim();
    if let Some(n) = reason
        .strip_prefix("标签最多（并列 ")
        .and_then(|rest| rest.strip_suffix(" 个）"))
    {
        return t_args(cx, "dedup.reason.tags_tied", &[("n", n)]).to_string();
    }
    if let Some(n) = reason
        .strip_prefix("标签最多（")
        .and_then(|rest| rest.strip_suffix(" 个）"))
    {
        return t_args(cx, "dedup.reason.tags_most", &[("n", n)]).to_string();
    }
    let key = match reason {
        "分辨率最高" => Some("dedup.reason.resolution"),
        "文件更大" => Some("dedup.reason.larger_file"),
        "最早导入" => Some("dedup.reason.earliest"),
        "路径最短" => Some("dedup.reason.shortest_path"),
        "保护路径" => Some("dedup.reason.protected"),
        _ => None,
    };
    key.map(|key| t(cx, key).to_string())
        .unwrap_or_else(|| reason.to_string())
}
use ely_gpui_component::overlays::Dialog;
use kanalib_core::model::{SimilarHit, SimilarThresholdPreview};

pub(crate) const DEDUP_RAIL_WIDTH: f32 = 272.;
pub(crate) const DEDUP_GROUP_ROW_HEIGHT: f32 = 78.;
pub(crate) const DEDUP_CARD_WIDTH: f32 = 214.;
pub(crate) const DEDUP_MAX_VISIBLE: usize = 4;

/// F-046：去重组排序维度；默认保持核心分组顺序。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum DedupSort {
    #[default]
    Default,
    Path,
    Name,
    Size,
    Reclaimable,
}

impl DedupSort {
    pub(crate) const ALL: [Self; 5] = [
        Self::Default,
        Self::Path,
        Self::Name,
        Self::Size,
        Self::Reclaimable,
    ];

    pub(crate) fn label(self, cx: &App) -> SharedString {
        let key = match self {
            Self::Default => "dedup.sort.default",
            Self::Path => "dedup.sort.path",
            Self::Name => "dedup.sort.name",
            Self::Size => "dedup.sort.size",
            Self::Reclaimable => "dedup.sort.reclaim",
        };
        t(cx, key)
    }

    /// 大小/可释放默认从大到小；路径/名称默认升序。
    pub(crate) fn default_descending(self) -> bool {
        matches!(self, Self::Size | Self::Reclaimable)
    }
}

/// F-047：最近一次移入回收站的审计摘要；只写 settings JSON，不做应用内撤销。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrashAudit {
    pub at: u64,
    pub groups: usize,
    pub assets: usize,
    pub failed: usize,
    pub protected_skipped: usize,
    pub bytes: u64,
    pub scope: String,
}

impl TrashAudit {
    pub(crate) fn to_json(&self) -> String {
        serde_json::json!({
            "at": self.at,
            "groups": self.groups,
            "assets": self.assets,
            "failed": self.failed,
            "protected_skipped": self.protected_skipped,
            "bytes": self.bytes,
            "scope": self.scope,
        })
        .to_string()
    }

    pub(crate) fn from_json(raw: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(raw).ok()?;
        Some(Self {
            at: value.get("at")?.as_u64()?,
            groups: value.get("groups")?.as_u64()? as usize,
            assets: value.get("assets")?.as_u64()? as usize,
            failed: value
                .get("failed")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as usize,
            protected_skipped: value
                .get("protected_skipped")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as usize,
            bytes: value
                .get("bytes")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            scope: value
                .get("scope")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("group")
                .to_string(),
        })
    }

    /// 审计摘要；界面文案走 `dedup.audit.*` 目录。
    pub(crate) fn summary(&self, cx: &App) -> String {
        let scope = if self.scope == "batch" {
            t_args(
                cx,
                "dedup.audit.batch",
                &[("groups", &self.groups.to_string())],
            )
            .to_string()
        } else {
            t(cx, "dedup.audit.single").to_string()
        };
        let mut text = t_args(
            cx,
            "dedup.audit.line",
            &[
                ("scope", &scope),
                ("assets", &self.assets.to_string()),
                ("bytes", &format_bytes(self.bytes)),
            ],
        )
        .to_string();
        if self.failed > 0 {
            text.push_str(&t_args(
                cx,
                "dedup.audit.failed",
                &[("n", &self.failed.to_string())],
            ));
        }
        if self.protected_skipped > 0 {
            text.push_str(&t_args(
                cx,
                "dedup.audit.skipped",
                &[("n", &self.protected_skipped.to_string())],
            ));
        }
        text
    }

    /// 测试用的中文快照；界面文本走 `dedup.audit.*` 目录。
    #[cfg(test)]
    pub(crate) fn describe(&self) -> String {
        let mut text = format!(
            "{} · 移入 {} 张 · 释放约 {}",
            if self.scope == "batch" {
                format!("批量 {} 组", self.groups)
            } else {
                "单组".to_string()
            },
            self.assets,
            format_bytes(self.bytes)
        );
        if self.failed > 0 {
            text.push_str(&format!(" · {} 张失败", self.failed));
        }
        if self.protected_skipped > 0 {
            text.push_str(&format!(" · 跳过 {} 个受保护组", self.protected_skipped));
        }
        text
    }
}

pub(crate) fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// F-047：预估可释放字节：单组用组内可释放量，批量汇总所有未审组的可释放量。
pub(crate) fn planned_reclaim_bytes(groups: &[DuplicateGroup], key: Option<&str>) -> u64 {
    match key {
        Some(key) => groups
            .iter()
            .find(|group| group.key == key)
            .map(DuplicateGroup::reclaimable)
            .unwrap_or(0),
        None => groups
            .iter()
            .filter(|group| group.state == ReviewState::Pending)
            .map(DuplicateGroup::reclaimable)
            .sum(),
    }
}

fn dedup_group_path_key(group: &DuplicateGroup) -> String {
    group
        .keeper_asset()
        .or_else(|| group.suggested_asset())
        .or_else(|| group.assets.first())
        .map(|asset| asset.path.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| group.key.to_lowercase())
}

fn dedup_group_name_key(group: &DuplicateGroup) -> String {
    group.title().to_lowercase()
}

fn parse_human_bytes(raw: &str) -> Option<u64> {
    let raw = raw.trim().to_lowercase();
    let (number, scale) = if let Some(value) = raw.strip_suffix("gb") {
        (value, 1024.0 * 1024.0 * 1024.0)
    } else if let Some(value) = raw.strip_suffix("mb") {
        (value, 1024.0 * 1024.0)
    } else if let Some(value) = raw.strip_suffix("kb") {
        (value, 1024.0)
    } else if let Some(value) = raw.strip_suffix('b') {
        (value, 1.0)
    } else {
        (raw.as_str(), 1.0)
    };
    let number = number.trim().parse::<f64>().ok()?;
    (number.is_finite() && number >= 0.0).then_some((number * scale) as u64)
}

fn parse_size_comparison(raw: &str) -> Option<(&'static str, u64)> {
    let raw = raw.trim();
    let (op, value) = if let Some(value) = raw.strip_prefix(">=") {
        (">=", value)
    } else if let Some(value) = raw.strip_prefix("<=") {
        ("<=", value)
    } else if let Some(value) = raw.strip_prefix('>') {
        (">", value)
    } else if let Some(value) = raw.strip_prefix('<') {
        ("<", value)
    } else if let Some(value) = raw.strip_prefix('=') {
        ("=", value)
    } else {
        ("=", raw)
    };
    Some((op, parse_human_bytes(value)?))
}

fn compare_bytes(value: u64, op: &str, threshold: u64) -> bool {
    match op {
        ">=" => value >= threshold,
        "<=" => value <= threshold,
        ">" => value > threshold,
        "<" => value < threshold,
        _ => value == threshold,
    }
}

/// F-046：组筛选。普通词匹配名称或任一资产路径；支持 `size:`/`reclaim:`
/// 以及 `name:`/`path:` 前缀，多个词为 AND，和搜索框语义一致。
pub(crate) fn dedup_group_matches_query(group: &DuplicateGroup, query: &str) -> bool {
    for term in query.split_whitespace() {
        let lower = term.to_lowercase();
        let matched = if let Some(raw) = lower.strip_prefix("size:") {
            parse_size_comparison(raw)
                .is_some_and(|(op, threshold)| compare_bytes(group.total_bytes, op, threshold))
        } else if let Some(raw) = lower.strip_prefix("reclaim:") {
            parse_size_comparison(raw)
                .is_some_and(|(op, threshold)| compare_bytes(group.reclaimable(), op, threshold))
        } else if let Some(raw) = lower.strip_prefix("name:") {
            dedup_group_name_key(group).contains(raw)
        } else if let Some(raw) = lower.strip_prefix("path:") {
            group
                .assets
                .iter()
                .any(|asset| asset.path.to_string_lossy().to_lowercase().contains(raw))
        } else {
            dedup_group_name_key(group).contains(&lower)
                || group
                    .assets
                    .iter()
                    .any(|asset| asset.path.to_string_lossy().to_lowercase().contains(&lower))
        };
        if !matched {
            return false;
        }
    }
    true
}

/// F-046：同时应用状态筛选、文本筛选与排序，返回核心组下标。
pub(crate) fn dedup_visible_indices(
    groups: &[DuplicateGroup],
    filter: DedupFilter,
    query: &str,
    sort: DedupSort,
    descending: bool,
) -> Vec<usize> {
    let mut indices = groups
        .iter()
        .enumerate()
        .filter(|(_, group)| dedup_group_matches_query(group, query) && filter.matches(group.state))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match sort {
        DedupSort::Default => {}
        DedupSort::Path => {
            indices.sort_by_cached_key(|&index| {
                (
                    dedup_group_path_key(&groups[index]),
                    groups[index].key.clone(),
                )
            });
            if descending {
                indices.reverse();
            }
        }
        DedupSort::Name => {
            indices.sort_by_cached_key(|&index| {
                (
                    dedup_group_name_key(&groups[index]),
                    groups[index].key.clone(),
                )
            });
            if descending {
                indices.reverse();
            }
        }
        DedupSort::Size => {
            indices.sort_by_cached_key(|&index| {
                (groups[index].total_bytes, groups[index].key.clone())
            });
            if descending {
                indices.reverse();
            }
        }
        DedupSort::Reclaimable => {
            indices.sort_by_cached_key(|&index| {
                (groups[index].reclaimable(), groups[index].key.clone())
            });
            if descending {
                indices.reverse();
            }
        }
    }
    indices
}

/// 单次锚点查询保留的结果上限；结果视图只保存这一份，不写库、不新开缓存。
pub(crate) const SIMILAR_RESULT_LIMIT: usize = 500;
const SIMILAR_ROW_HEIGHT: f32 = 64.;
const SIMILAR_REFRESH_DEBOUNCE_MS: u64 = 180;
const SIMILAR_PREVIEW_DEBOUNCE_MS: u64 = 220;

/// F-004：一次单锚点相似查询的临时结果；离开视图即丢弃。
#[derive(Clone)]
pub(crate) struct SimilarView {
    pub(crate) anchor: PathBuf,
    pub(crate) threshold: u8,
    pub(crate) scanned: usize,
    pub(crate) truncated: bool,
    /// 核心已按距离升序、同距离按路径升序返回。
    pub(crate) hits: Vec<SimilarHit>,
    pub(crate) error: Option<String>,
}

fn dedup_filter_key(filter: DedupFilter) -> &'static str {
    match filter {
        DedupFilter::Pending => "dedup.chip.pending",
        DedupFilter::Ignored => "dedup.chip.ignored",
        DedupFilter::Related => "dedup.chip.related",
        DedupFilter::Done => "dedup.chip.done",
        DedupFilter::All => "dedup.chip.all",
    }
}

pub(crate) fn dedup_filter_chip(
    cx: &App,
    filter: DedupFilter,
    count: usize,
    active: bool,
    this: Entity<KanalibApp>,
) -> AnyElement {
    let key = dedup_filter_key(filter);
    filter_chip(
        SharedString::from(format!("dedup-filter-{key}")),
        format!("{} {}", t(cx, key), count),
        active,
        move |_, _, cx| {
            let _ = this.update(cx, |app, cx| {
                app.dedup_filter = filter;
                app.dedup_selected = None;
                app.dedup_show_all = false;
                app.dedup_scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            });
        },
    )
}

pub(crate) fn small_badge(label: String, fg: Hsla, bg: Hsla) -> AnyElement {
    div()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(3.))
        .bg(bg)
        .text_xs()
        .text_color(fg)
        .child(label)
        .into_any_element()
}

pub(crate) fn group_state_key(state: ReviewState) -> &'static str {
    match state {
        ReviewState::Pending => "dedup.state.pending",
        ReviewState::Ignored => "dedup.state.ignored",
        ReviewState::Related => "dedup.state.related",
        ReviewState::Done => "dedup.state.done",
    }
}

pub(crate) fn group_state_label(cx: &App, state: ReviewState) -> SharedString {
    t(cx, group_state_key(state))
}

pub(crate) fn dedup_group_row(
    cx: &App,
    group: &DuplicateGroup,
    selected: bool,
    this: Entity<KanalibApp>,
    thumb_cache: Entity<ThumbCache>,
    colors: UiColors,
    protected_paths: &[PathBuf],
) -> AnyElement {
    let key = group.key.clone();
    let title = group.title();
    let count = group.assets.len();
    let reclaimable = format_bytes(group.reclaimable());
    let thumb = group
        .suggested_asset()
        .or_else(|| group.assets.first())
        .and_then(|asset| asset.thumb.clone());
    let media: AnyElement = match thumb {
        Some(path) => img(path)
            .image_cache(&thumb_cache)
            .size_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        None => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(colors.subtle)
            .child("…")
            .into_any_element(),
    };
    let (state_fg, state_bg) = match group.state {
        ReviewState::Pending => (colors.warning, colors.warning_subtle),
        ReviewState::Ignored => (colors.muted, colors.sunken),
        ReviewState::Related => (colors.accent, colors.selection),
        ReviewState::Done => (colors.success, colors.success_subtle),
    };
    let has_protected = group
        .assets
        .iter()
        .any(|asset| is_protected(&asset.path, protected_paths));
    let has_remote = any_remote_asset(group.assets.iter().map(|asset| asset.path.as_path()));
    let (kind_label, kind_fg, kind_bg) = match group.kind {
        GroupKind::Exact => (
            t(cx, "dedup.kind.exact").to_string(),
            colors.success,
            colors.success_subtle,
        ),
        GroupKind::Similar => (
            t_args(
                cx,
                "dedup.kind.similar",
                &[("pct", &group.similarity_pct().unwrap_or(0).to_string())],
            )
            .to_string(),
            colors.warning,
            colors.warning_subtle,
        ),
    };
    let border = if selected {
        colors.accent
    } else {
        colors.border
    };
    div()
        .id(SharedString::from(format!("dedup-group-{key}")))
        .h(px(DEDUP_GROUP_ROW_HEIGHT))
        .w_full()
        .p(px(5.))
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            let _ = this.update(cx, |app, cx| {
                app.dedup_selected = Some(key.clone());
                app.dedup_show_all = false;
                cx.notify();
            });
        })
        .child(
            div()
                .w_full()
                .h_full()
                .p(px(7.))
                .rounded(px(7.))
                .border_1()
                .border_color(border)
                .bg(if selected {
                    colors.selection
                } else {
                    colors.bg
                })
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .w(px(50.))
                        .h(px(50.))
                        .flex_none()
                        .rounded(px(5.))
                        .overflow_hidden()
                        .bg(colors.sunken)
                        .child(media),
                )
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
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(small_badge(
                                    t_args(cx, "dedup.badge.count", &[("n", &count.to_string())])
                                        .to_string(),
                                    colors.muted,
                                    colors.sunken,
                                ))
                                .child(small_badge(kind_label, kind_fg, kind_bg))
                                .children(has_protected.then(|| {
                                    small_badge(
                                        t(cx, "dedup.badge.protected").to_string(),
                                        colors.accent,
                                        colors.selection,
                                    )
                                }))
                                .children(has_remote.then(|| {
                                    small_badge(
                                        t(cx, "dedup.badge.remote").to_string(),
                                        colors.accent,
                                        colors.selection,
                                    )
                                }))
                                .child(small_badge(
                                    group_state_label(cx, group.state).to_string(),
                                    state_fg,
                                    state_bg,
                                )),
                        )
                        .child(
                            div().truncate().text_xs().text_color(colors.subtle).child(
                                {
                                    let reason = localize_dedup_reason(cx, &group.suggested_reason);
                                    t_args(
                                        cx,
                                        "dedup.row.reclaim",
                                        &[("bytes", &reclaimable), ("reason", &reason)],
                                    )
                                }
                                .to_string(),
                            ),
                        ),
                ),
        )
        .into_any_element()
}

pub(crate) fn dedup_asset_card(
    cx: &App,
    group: &DuplicateGroup,
    asset: &Asset,
    is_keeper: bool,
    is_suggested: bool,
    editable: bool,
    this: Entity<KanalibApp>,
    thumb_cache: &Entity<ThumbCache>,
    colors: UiColors,
    protected_paths: &[PathBuf],
) -> AnyElement {
    let group_key = group.key.clone();
    let group_kind = group.kind;
    let path = asset.path.clone();
    let pending = group.state == ReviewState::Pending;
    let is_protected_path = is_protected(&asset.path, protected_paths);
    let is_remote = is_remote_asset(&asset.path);
    let similarity = group
        .distance_to_keeper(asset)
        .map(similarity_pct_from_distance);
    let media: AnyElement = match asset.thumb.clone() {
        Some(thumb) => img(thumb)
            .image_cache(thumb_cache)
            .w_full()
            .h(px(176.))
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        None => div()
            .w_full()
            .h(px(176.))
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(colors.subtle)
            .child(t(cx, "dedup.card.no_thumb"))
            .into_any_element(),
    };
    let border = if is_keeper {
        colors.success
    } else if is_suggested {
        colors.warning
    } else {
        colors.border
    };
    let extension = asset
        .path
        .extension()
        .map(|ext| ext.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "?".to_string());
    let dimensions = if asset.width == 0 || asset.height == 0 {
        t(cx, "dedup.card.dimensions_unknown").to_string()
    } else {
        format!("{}×{}", asset.width, asset.height)
    };
    let rating = if asset.rating.is_empty() {
        t(cx, "dedup.rating.none").to_string()
    } else {
        t_args(
            cx,
            "dedup.rating.value",
            &[("rating", &asset.rating.to_uppercase())],
        )
        .to_string()
    };
    let mut badge = div()
        .absolute()
        .top(px(7.))
        .left(px(7.))
        .flex()
        .items_center()
        .gap(px(4.));
    if is_keeper {
        badge = badge.child(small_badge(
            t(cx, "dedup.badge.keeper").to_string(),
            colors.success,
            colors.success_subtle,
        ));
    }
    if is_suggested && !is_keeper {
        badge = badge.child(small_badge(
            t(cx, "dedup.badge.recommended").to_string(),
            colors.warning,
            colors.warning_subtle,
        ));
    }
    if is_protected_path {
        badge = badge.child(small_badge(
            t(cx, "dedup.badge.protected").to_string(),
            colors.accent,
            colors.selection,
        ));
    }
    if is_remote {
        badge = badge.child(small_badge(
            t(cx, "dedup.badge.remote").to_string(),
            colors.accent,
            colors.selection,
        ));
    }

    let mut card = div()
        .id(SharedString::from(format!(
            "dedup-card-{group_key}-{}",
            asset.path.display()
        )))
        .w_full()
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .rounded(px(9.))
        .overflow_hidden()
        .border_1()
        .border_color(border)
        .bg(colors.surface);
    if editable {
        card = card
            .cursor_pointer()
            .hover(move |style| style.border_color(colors.accent))
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.state
                        .set_dedup_keeper(&group_key, path.clone(), group_kind);
                    cx.notify();
                });
            });
    }

    let similarity_badge = similarity.map(|pct| {
        div()
            .absolute()
            .left(px(7.))
            .bottom(px(7.))
            .px(px(5.))
            .py(px(1.))
            .rounded(px(3.))
            .bg(colors.bg)
            .text_xs()
            .text_color(colors.warning)
            .child(t_args(
                cx,
                "dedup.kind.similar",
                &[("pct", &pct.to_string())],
            ))
            .into_any_element()
    });

    card = card
        .child(
            div()
                .relative()
                .w_full()
                .h(px(176.))
                .flex_none()
                .overflow_hidden()
                .bg(colors.sunken)
                .child(media)
                .child(badge)
                .children(similarity_badge),
        )
        .child(
            div()
                .h(px(124.))
                .min_h(px(124.))
                .overflow_hidden()
                .p(px(8.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(asset.title()),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(colors.muted)
                        .child(format!(
                            "{dimensions} · {extension} · {}",
                            format_bytes(asset.bytes)
                        )),
                )
                .child(
                    div().truncate().text_xs().text_color(colors.subtle).child(
                        t_args(
                            cx,
                            "dedup.card.meta",
                            &[
                                ("date", &format_date(asset.mtime)),
                                ("count", &asset.all_tags().count().to_string()),
                                ("rating", &rating),
                            ],
                        )
                        .to_string(),
                    ),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(asset.path.display().to_string()),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(if is_keeper {
                            colors.success
                        } else if is_suggested {
                            colors.warning
                        } else {
                            colors.subtle
                        })
                        .child(if is_keeper {
                            t(cx, "dedup.card.keeper_current").to_string()
                        } else if is_suggested {
                            {
                                let reason = localize_dedup_reason(cx, &group.suggested_reason);
                                t_args(
                                    cx,
                                    "dedup.card.recommended_reason",
                                    &[("reason", &reason)],
                                )
                            }
                            .to_string()
                        } else if editable {
                            t(cx, "dedup.card.click_to_keep").to_string()
                        } else if pending {
                            t(cx, "dedup.card.scanning_locked").to_string()
                        } else {
                            t(cx, "dedup.card.group_done").to_string()
                        }),
                ),
        );

    div()
        .w(px(DEDUP_CARD_WIDTH))
        .h(px(300.))
        .flex_none()
        .child(card)
        .into_any_element()
}

pub(crate) struct DiffRow {
    label: SharedString,
    values: Vec<String>,
    best: Option<usize>,
}

pub(crate) fn best_max<T: Ord + Copy>(
    assets: &[&Asset],
    value: impl Fn(&Asset) -> T,
) -> Option<usize> {
    let mut best = 0;
    for (index, asset) in assets.iter().enumerate() {
        if value(asset) > value(assets[best]) {
            best = index;
        }
    }
    let max = value(assets[best]);
    let min = assets.iter().map(|asset| value(asset)).min()?;
    (max != min).then_some(best)
}

pub(crate) fn best_min<T: Ord + Copy>(
    assets: &[&Asset],
    value: impl Fn(&Asset) -> T,
) -> Option<usize> {
    let mut best = 0;
    for (index, asset) in assets.iter().enumerate() {
        if value(asset) < value(assets[best]) {
            best = index;
        }
    }
    let min = value(assets[best]);
    let max = assets.iter().map(|asset| value(asset)).max()?;
    (min != max).then_some(best)
}

pub(crate) fn push_diff_row(
    rows: &mut Vec<DiffRow>,
    label: SharedString,
    values: Vec<String>,
    best: Option<usize>,
) {
    if values.len() > 1 && values.iter().any(|value| value != &values[0]) {
        rows.push(DiffRow {
            label,
            values,
            best,
        });
    }
}

pub(crate) fn dedup_diff(
    cx: &App,
    group: &DuplicateGroup,
    assets: &[&Asset],
    colors: UiColors,
) -> AnyElement {
    if assets.is_empty() {
        return div().into_any_element();
    }
    let mut rows = Vec::new();
    push_diff_row(
        &mut rows,
        t(cx, "dedup.diff.resolution"),
        assets
            .iter()
            .map(|asset| {
                if asset.width == 0 || asset.height == 0 {
                    t(cx, "dedup.diff.unknown").to_string()
                } else {
                    format!("{}×{}", asset.width, asset.height)
                }
            })
            .collect(),
        best_max(assets, |asset| asset.width as u64 * asset.height as u64),
    );
    push_diff_row(
        &mut rows,
        t(cx, "dedup.diff.size"),
        assets
            .iter()
            .map(|asset| format_bytes(asset.bytes))
            .collect(),
        best_max(assets, |asset| asset.bytes),
    );
    push_diff_row(
        &mut rows,
        t(cx, "dedup.diff.format"),
        assets
            .iter()
            .map(|asset| {
                asset
                    .path
                    .extension()
                    .map(|ext| ext.to_string_lossy().to_uppercase())
                    .unwrap_or_default()
            })
            .collect(),
        None,
    );
    push_diff_row(
        &mut rows,
        t(cx, "dedup.diff.modified"),
        assets
            .iter()
            .map(|asset| format_date(asset.mtime))
            .collect(),
        best_min(assets, |asset| {
            if asset.mtime == 0 {
                u64::MAX
            } else {
                asset.mtime
            }
        }),
    );
    push_diff_row(
        &mut rows,
        t(cx, "dedup.diff.tags"),
        assets
            .iter()
            .map(|asset| {
                let top = asset
                    .top_tags(2)
                    .into_iter()
                    .map(|hit| hit.tag.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ");
                let count = asset.all_tags().count().to_string();
                if top.is_empty() {
                    t_args(cx, "dedup.diff.tag_count", &[("n", &count)]).to_string()
                } else {
                    t_args(
                        cx,
                        "dedup.diff.tag_count_top",
                        &[("n", &count), ("top", &top)],
                    )
                    .to_string()
                }
            })
            .collect(),
        best_max(assets, |asset| asset.all_tags().count()),
    );
    push_diff_row(
        &mut rows,
        t(cx, "dedup.diff.rating"),
        assets
            .iter()
            .map(|asset| {
                if asset.rating.is_empty() {
                    t(cx, "dedup.rating.none").to_string()
                } else {
                    asset.rating.to_uppercase()
                }
            })
            .collect(),
        None,
    );
    if group.kind == GroupKind::Similar {
        push_diff_row(
            &mut rows,
            t(cx, "dedup.diff.similarity"),
            assets
                .iter()
                .map(|asset| {
                    group
                        .distance_to_keeper(asset)
                        .map(|distance| {
                            t_args(
                                cx,
                                "dedup.diff.similarity_value",
                                &[
                                    ("pct", &similarity_pct_from_distance(distance).to_string()),
                                    ("distance", &distance.to_string()),
                                ],
                            )
                            .to_string()
                        })
                        .unwrap_or_else(|| t(cx, "dedup.diff.unknown").to_string())
                })
                .collect(),
            best_min(assets, |asset| {
                group.distance_to_keeper(asset).unwrap_or(u32::MAX)
            }),
        );
    }
    push_diff_row(
        &mut rows,
        t(cx, "dedup.diff.path"),
        assets
            .iter()
            .map(|asset| asset.path.display().to_string())
            .collect(),
        best_min(assets, |asset| asset.path.as_os_str().len()),
    );

    let label_width = 84.;
    let header = div()
        .flex()
        .items_center()
        .bg(colors.sunken)
        .border_b_1()
        .border_color(colors.border)
        .child(
            div()
                .w(px(label_width))
                .flex_none()
                .px(px(7.))
                .py(px(5.))
                .text_xs()
                .text_color(colors.subtle)
                .child(t(cx, "dedup.diff.field")),
        )
        .children(assets.iter().map(|asset| {
            div()
                .flex_1()
                .min_w(px(0.))
                .px(px(7.))
                .py(px(5.))
                .truncate()
                .text_xs()
                .text_color(colors.subtle)
                .child(asset.title())
        }));

    let table = div()
        .rounded(px(8.))
        .border_1()
        .border_color(colors.border)
        .overflow_hidden()
        .child(header)
        .children(rows.into_iter().map(|row| {
            div()
                .flex()
                .items_center()
                .border_t_1()
                .border_color(colors.border)
                .child(
                    div()
                        .w(px(label_width))
                        .flex_none()
                        .px(px(7.))
                        .py(px(5.))
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(row.label),
                )
                .children(row.values.iter().enumerate().map(|(index, value)| {
                    let mut cell = div()
                        .flex_1()
                        .min_w(px(0.))
                        .px(px(7.))
                        .py(px(5.))
                        .truncate()
                        .text_xs()
                        .text_color(colors.muted);
                    if row.best == Some(index) {
                        cell = cell.bg(colors.success_subtle).text_color(colors.success);
                    }
                    cell.child(value.clone())
                }))
        }));

    let recommendation = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_xs()
        .text_color(colors.muted)
        .child(
            t_args(
                cx,
                "dedup.diff.recommend",
                &[(
                    "name",
                    &group
                        .suggested_asset()
                        .map(Asset::title)
                        .unwrap_or_default(),
                )],
            )
            .to_string(),
        )
        .child(
            div()
                .text_color(colors.warning)
                .child(localize_dedup_reason(cx, &group.suggested_reason)),
        );

    div()
        .id("dedup-diff")
        .w_full()
        .px(px(16.))
        .pb(px(12.))
        .flex()
        .flex_col()
        .gap(px(7.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t(cx, "dedup.diff.title")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(t(cx, "dedup.diff.only_changed")),
                ),
        )
        .child(recommendation)
        .child(table)
        .child(div().text_xs().text_color(colors.subtle).child(
            if group.kind == GroupKind::Similar {
                t(cx, "dedup.diff.note_similar")
            } else {
                t(cx, "dedup.diff.note_exact")
            },
        ))
        .into_any_element()
}

impl KanalibApp {
    pub(crate) fn open_dedup_mode(&mut self, mode: GroupKind, cx: &mut Context<Self>) {
        self.screen = Screen::Duplicates;
        self.tag_return_pending = false;
        self.dedup_mode = mode;
        self.dedup_filter = DedupFilter::Pending;
        self.dedup_selected = None;
        self.dedup_confirm = None;
        self.dedup_show_all = false;
        self.dedup_notice = None;
        self.dedup_threshold_draft = None;
        self.dedup_focus_pending = true;
        self.dedup_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    pub(crate) fn visible_dedup_groups(&self, groups: &[DuplicateGroup]) -> Vec<usize> {
        dedup_visible_indices(
            groups,
            self.dedup_filter,
            &self.dedup_query,
            self.dedup_sort,
            self.dedup_sort_descending,
        )
    }

    pub(crate) fn advance_dedup(&mut self, groups: &[DuplicateGroup], forward: bool) {
        let visible = self.visible_dedup_groups(groups);
        if visible.is_empty() {
            self.dedup_selected = None;
            return;
        }
        let index = self.dedup_selected.as_ref().and_then(|selected| {
            visible
                .iter()
                .position(|&group_index| &groups[group_index].key == selected)
        });
        let next = match (index, forward) {
            (Some(index), true) => (index + 1) % visible.len(),
            (Some(index), false) => (index + visible.len() - 1) % visible.len(),
            (None, _) => 0,
        };
        self.dedup_selected = Some(groups[visible[next]].key.clone());
        self.dedup_show_all = false;
        // 键盘/按钮切换后把当前组滚到可见区域中间。
        self.dedup_scroll
            .scroll_to_item(next, ScrollStrategy::Center);
    }

    pub(crate) fn render_dedup(
        &mut self,
        this: &Entity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
        exact_groups: Arc<Vec<DuplicateGroup>>,
        exact_summary: DedupSummary,
        similar_groups: Arc<Vec<DuplicateGroup>>,
        similar_summary: DedupSummary,
        similar_scanned: bool,
        similar_scanning: bool,
        similar_threshold: u8,
        protected_paths: &[PathBuf],
        status: &kanalib_core::model::StatusMessage,
        error: Option<&str>,
        library_scanning: bool,
        width: Pixels,
        colors: UiColors,
    ) -> impl IntoElement {
        // F-046：结果视图内的筛选输入；只影响本页可见组，不触发图库查询。
        let filter_input = window.use_keyed_state("dedup-group-filter", cx, |window, cx| {
            TextInput::new(window, cx).placeholder_key("dedup.filter.placeholder")
        });
        if self.dedup_query_input.is_none() {
            self.dedup_query_input = Some(filter_input.clone());
        }
        let query_text = filter_input.read(cx).text().to_string();
        if query_text != self.dedup_query {
            self.dedup_query = query_text;
            self.dedup_selected = None;
        }
        if self.dedup_focus_pending && self.dedup_confirm.is_none() {
            window.focus(&self.dedup_focus, cx);
            self.dedup_focus_pending = false;
        }
        let similar_mode = self.dedup_mode == GroupKind::Similar;
        let groups = if similar_mode {
            similar_groups
        } else {
            exact_groups
        };
        let summary = if similar_mode {
            similar_summary.clone()
        } else {
            exact_summary.clone()
        };
        let busy = library_scanning || (similar_mode && similar_scanning);
        let has_remote_pending = groups.iter().any(|group| {
            group.state == ReviewState::Pending
                && any_remote_asset(group.assets.iter().map(|asset| asset.path.as_path()))
        });

        let visible_indices = dedup_visible_indices(
            &groups,
            self.dedup_filter,
            &self.dedup_query,
            self.dedup_sort,
            self.dedup_sort_descending,
        );
        if self.dedup_selected.as_ref().is_none_or(|key| {
            !visible_indices
                .iter()
                .any(|&index| &groups[index].key == key)
        }) {
            self.dedup_selected = visible_indices
                .first()
                .map(|&index| groups[index].key.clone());
        }
        let selected = self
            .dedup_selected
            .as_ref()
            .and_then(|key| {
                visible_indices
                    .iter()
                    .copied()
                    .find(|&index| &groups[index].key == key)
            })
            .and_then(|index| groups.get(index))
            .cloned();

        let this_rescan = this.clone();
        let this_preselect = this.clone();
        let this_batch = this.clone();
        let this_library = this.clone();
        let this_similar_scan = this.clone();
        let preselect_mode = self.dedup_mode;
        // F-070 方案 A：模式切换移到上下文侧栏，页头只显示当前模式与组数，避免第二套控件。
        let tabs = div()
            .text_xs()
            .text_color(colors.muted)
            .child(if similar_mode {
                t_args(
                    cx,
                    "dedup.tabs.similar",
                    &[("n", &similar_summary.total.to_string())],
                )
            } else {
                t_args(
                    cx,
                    "dedup.tabs.exact",
                    &[("n", &exact_summary.total.to_string())],
                )
            });

        let more_menu = Menu::new().item(
            MenuItem::new(t(cx, "dedup.rescan"))
                .disabled(busy)
                .disabled_reason(t(cx, "dedup.rescan_disabled"))
                .on_click(move |_, cx| {
                    let _ = this_rescan.update(cx, |app, cx| {
                        app.request_rescan(RescanOrigin::Dedup, cx);
                    });
                }),
        );
        let header = div()
            .h(px(PAGE_HEADER_HEIGHT))
            .px(px(SPACE_4))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(SPACE_3))
            .border_b_1()
            .border_color(colors.border)
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
                            .gap(px(SPACE_2))
                            .min_w(px(0.))
                            .child(
                                div()
                                    .flex_none()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(t(cx, "page.duplicates")),
                            )
                            .child(tabs),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(colors.muted)
                            .child(t_args(
                                cx,
                                "dedup.summary",
                                &[
                                    ("pending", &summary.pending.to_string()),
                                    ("assets", &summary.pending_assets.to_string()),
                                    ("bytes", &format_bytes(summary.pending_bytes)),
                                    (
                                        "status",
                                        &if library_scanning {
                                            t(cx, "dedup.summary.library_scanning").to_string()
                                        } else if similar_mode && similar_scanning {
                                            t(cx, "dedup.summary.similar_scanning").to_string()
                                        } else {
                                            crate::localization::core_status(cx, status).to_string()
                                        },
                                    ),
                                ],
                            )),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(SPACE_1_5))
                    .children(similar_mode.then(|| {
                        Button::new(
                            "dedup-similar-scan",
                            if similar_scanning {
                                t(cx, "dedup.similar.scanning_button")
                            } else if similar_scanned {
                                t(cx, "dedup.similar.rescan")
                            } else {
                                t(cx, "dedup.similar.scan")
                            },
                        )
                        .primary()
                        // F-073 6.2：相似扫描进行中由按钮自己显示 spinner；图库扫描
                        // 是另一项任务的禁用条件，两者不混成「不可用」。
                        .loading(similar_scanning)
                        .disabled(library_scanning)
                        .on_click(move |_, _, cx| {
                            let _ = this_similar_scan.update(cx, |app, cx| {
                                app.state.start_similar_scan();
                                app.dedup_selected = None;
                                app.dedup_confirm = None;
                                app.dedup_notice = None;
                                cx.notify();
                            });
                        })
                    }))
                    .child(
                        Button::new("dedup-preselect", t(cx, "dedup.preselect"))
                            .disabled(busy || summary.pending == 0)
                            .on_click(move |_, _, cx| {
                                let _ = this_preselect.update(cx, |app, cx| {
                                    let selected = app.state.preselect_dedup(preselect_mode);
                                    app.dedup_notice = Some(if selected == 0 {
                                        t(cx, "dedup.preselect.empty").to_string()
                                    } else {
                                        t_args(
                                            cx,
                                            "dedup.preselect.done",
                                            &[("n", &selected.to_string())],
                                        )
                                        .to_string()
                                    });
                                    cx.notify();
                                });
                            }),
                    )
                    .children((!similar_mode).then(|| {
                        Button::new("dedup-batch", t(cx, "dedup.batch_trash"))
                            .disabled(busy || summary.pending == 0 || has_remote_pending)
                            .on_click(move |_, _, cx| {
                                let _ = this_batch.update(cx, |app, cx| {
                                    app.dedup_confirm = Some(DedupConfirm::Batch);
                                    cx.notify();
                                });
                            })
                    }))
                    .child(DropdownMenu::new(
                        "dedup-more",
                        t(cx, "dedup.more"),
                        more_menu,
                    ))
                    .child(
                        Button::new("dedup-library", t(cx, "common.back_to_library")).on_click(
                            move |_, _, cx| {
                                let _ = this_library.update(cx, |app, cx| {
                                    app.screen = Screen::Library;
                                    app.dedup_confirm = None;
                                    cx.notify();
                                });
                            },
                        ),
                    ),
            );

        let threshold_bar: Option<AnyElement> = similar_mode.then(|| {
            div()
                .px(px(16.))
                .py(px(6.))
                .border_b_1()
                .border_color(colors.border)
                .child(self.render_threshold_editor(
                    cx,
                    this,
                    similar_threshold,
                    self.dedup_threshold_draft,
                    false,
                    colors,
                ))
                .into_any_element()
        });

        let filter_bar = div()
            .px(px(SPACE_4))
            .py(px(SPACE_2))
            .flex()
            .items_center()
            .gap(px(SPACE_1_5))
            .border_b_1()
            .border_color(colors.border)
            .child(dedup_filter_chip(
                cx,
                DedupFilter::Pending,
                summary.pending,
                self.dedup_filter == DedupFilter::Pending,
                this.clone(),
            ))
            .child(dedup_filter_chip(
                cx,
                DedupFilter::Ignored,
                summary.ignored,
                self.dedup_filter == DedupFilter::Ignored,
                this.clone(),
            ))
            .children(similar_mode.then(|| {
                dedup_filter_chip(
                    cx,
                    DedupFilter::Related,
                    summary.related,
                    self.dedup_filter == DedupFilter::Related,
                    this.clone(),
                )
            }))
            .child(dedup_filter_chip(
                cx,
                DedupFilter::Done,
                summary.done,
                self.dedup_filter == DedupFilter::Done,
                this.clone(),
            ))
            .child(dedup_filter_chip(
                cx,
                DedupFilter::All,
                summary.total,
                self.dedup_filter == DedupFilter::All,
                this.clone(),
            ))
            .child(div().flex_1())
            .child(div().text_xs().text_color(colors.subtle).child(if busy {
                t(cx, "dedup.hint.scanning")
            } else if similar_mode {
                t(cx, "dedup.hint.similar")
            } else {
                t(cx, "dedup.hint.exact")
            }));

        // F-046：组筛选与排序；键盘 n/j/↓ 下一组，p/k/↑ 上一组。
        let sort_menu = {
            let mut menu = Menu::new();
            for sort in DedupSort::ALL {
                let this = this.clone();
                menu = menu.item(
                    MenuItem::radio(sort.label(cx), self.dedup_sort == sort).on_click(
                        move |_, cx| {
                            let _ = this.update(cx, |app, cx| {
                                app.dedup_sort = sort;
                                app.dedup_sort_descending = sort.default_descending();
                                app.dedup_selected = None;
                                cx.notify();
                            });
                        },
                    ),
                );
            }
            menu
        };
        let this_direction = this.clone();
        let direction = IconButton::new(
            "dedup-sort-direction",
            if self.dedup_sort_descending {
                IconName::ArrowDown
            } else {
                IconName::ArrowUp
            },
        )
        .size(ControlSize::Sm)
        .tooltip(if self.dedup_sort_descending {
            t(cx, "dedup.sort.desc_tooltip")
        } else {
            t(cx, "dedup.sort.asc_tooltip")
        })
        .disabled(self.dedup_sort == DedupSort::Default)
        .on_click(move |_, _, cx| {
            let _ = this_direction.update(cx, |app, cx| {
                app.dedup_sort_descending = !app.dedup_sort_descending;
                app.dedup_selected = None;
                cx.notify();
            });
        });
        let this_recycle = this.clone();
        let open_recycle_bin = Button::new("dedup-recycle-bin", t(cx, "dedup.recycle_bin"))
            .size(ControlSize::Sm)
            .icon(IconName::FolderOpen)
            .on_click(move |_, _, cx| {
                if let Err(error) = crate::platform::open_recycle_bin() {
                    let message = t_args(
                        cx,
                        "dedup.recycle_bin_failed",
                        &[("error", &error.to_string())],
                    );
                    let _ = this_recycle.update(cx, move |app, cx| {
                        push_toast(&app.toaster, cx, message, Severity::Danger);
                    });
                }
            });
        let sort_bar = div()
            .px(px(16.))
            .py(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .border_b_1()
            .border_color(colors.border)
            .child(
                div().w(px(260.)).child(
                    Input::new(&filter_input)
                        .clearable()
                        .least(t(cx, "dedup.filter.least")),
                ),
            )
            .child(
                DropdownMenu::new("dedup-sort", t(cx, "dedup.sort.menu"), sort_menu)
                    .icon(IconName::ArrowDownUp)
                    .variant(ButtonVariant::Ghost),
            )
            .child(direction)
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(t(cx, "dedup.keyboard_hint")),
            )
            .child(open_recycle_bin);

        let audit_bar = self.last_trash_audit.as_ref().map(|audit| {
            div()
                .px(px(16.))
                .py(px(5.))
                .border_b_1()
                .border_color(colors.border)
                .bg(colors.sunken)
                .text_xs()
                .text_color(colors.muted)
                .child(t_args(
                    cx,
                    "dedup.audit.recent",
                    &[
                        ("time", &relative_time(audit.at)),
                        ("summary", &audit.summary(cx)),
                    ],
                ))
                .into_any_element()
        });

        let rail_header = div()
            .px(px(12.))
            .py(px(9.))
            .border_b_1()
            .border_color(colors.border)
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t(cx, "dedup.rail.groups")),
            )
            .child(div().text_xs().text_color(colors.subtle).child(t_args(
                cx,
                "dedup.rail.count",
                &[("n", &visible_indices.len().to_string())],
            )));

        let rail_list: AnyElement = if visible_indices.is_empty() {
            div()
                .flex_1()
                .p(px(16.))
                .text_xs()
                .text_color(colors.subtle)
                .child(if similar_mode && !similar_scanned && !similar_scanning {
                    t(cx, "dedup.empty.unscanned")
                } else if similar_scanning {
                    t(cx, "dedup.empty.scanning")
                } else if library_scanning {
                    t(cx, "dedup.empty.library")
                } else {
                    t(cx, "dedup.empty.filtered")
                })
                .into_any_element()
        } else {
            let groups_for_list = groups.clone();
            let indices_for_list = Arc::new(visible_indices.clone());
            let selected_key = self.dedup_selected.clone();
            let this_for_list = this.clone();
            let thumb_cache_for_list = self.thumb_cache.clone();
            let protected_for_list = protected_paths.to_vec();
            uniform_list(
                "dedup-groups",
                indices_for_list.len(),
                move |range, _, cx| {
                    range
                        .map(|row| {
                            let index = indices_for_list[row];
                            let group = &groups_for_list[index];
                            dedup_group_row(
                                cx,
                                group,
                                selected_key.as_deref() == Some(group.key.as_str()),
                                this_for_list.clone(),
                                thumb_cache_for_list.clone(),
                                colors,
                                &protected_for_list,
                            )
                        })
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&self.dedup_scroll)
            .flex_1()
            .w_full()
            .min_h(px(0.))
            .into_any_element()
        };

        let rail = div()
            .w(px(DEDUP_RAIL_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(colors.surface)
            .border_r_1()
            .border_color(colors.border)
            .child(rail_header)
            .child(rail_list);

        let detail: AnyElement = if let Some(group) = selected {
            let detail_width = (width - px(DEDUP_RAIL_WIDTH)).max(px(0.));
            self.render_dedup_detail(
                cx,
                this,
                &group,
                groups.clone(),
                busy,
                detail_width,
                protected_paths,
                colors,
            )
        } else {
            let (icon, title, description) =
                if similar_mode && !similar_scanned && !similar_scanning {
                    (
                        IconName::Search,
                        t(cx, "dedup.state.title_unscanned"),
                        t(cx, "dedup.state.desc_unscanned"),
                    )
                } else if similar_scanning {
                    (
                        IconName::RefreshCw,
                        t(cx, "dedup.state.title_computing"),
                        t(cx, "dedup.state.desc_computing"),
                    )
                } else if library_scanning {
                    (
                        IconName::RefreshCw,
                        t(cx, "dedup.state.title_library"),
                        t(cx, "dedup.state.desc_library"),
                    )
                } else if similar_mode {
                    (
                        IconName::SearchX,
                        t(cx, "dedup.state.title_similar_filtered"),
                        t(cx, "dedup.state.desc_filtered"),
                    )
                } else {
                    (
                        IconName::SearchX,
                        t(cx, "dedup.state.title_exact_filtered"),
                        t(cx, "dedup.state.desc_filtered"),
                    )
                };
            empty_state(icon, title, description, None, colors)
        };

        let notice = self.dedup_notice.clone();
        let remote_note =
            has_remote_pending.then(|| (t(cx, "dedup.remote_note").to_string(), colors.warning));
        let banner: Option<AnyElement> = error
            .map(|message| {
                (
                    message.to_string(),
                    colors.danger,
                    self.error_retry_action(message).is_some(),
                )
            })
            .or_else(|| notice.map(|message| (message, colors.muted, false)))
            .or_else(|| remote_note.map(|(message, color)| (message, color, false)))
            .map(|(message, color, retry)| {
                let this_retry = this.clone();
                div()
                    .px(px(16.))
                    .py(px(7.))
                    .border_b_1()
                    .border_color(colors.border)
                    .bg(colors.sunken)
                    .text_xs()
                    .text_color(color)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().min_w(px(0.)).child(message))
                    .children(retry.then(|| {
                        Button::new(
                            "dedup-error-retry",
                            crate::localization::t(cx, "status.retry"),
                        )
                        .size(ControlSize::Sm)
                        .variant(ButtonVariant::Ghost)
                        .on_click(move |_, _, cx| {
                            let _ = this_retry.update(cx, |app, cx| app.retry_ui_error(cx));
                        })
                    }))
                    .into_any_element()
            });

        let overlay = self.dedup_confirm.clone().and_then(|confirm| {
            self.render_dedup_confirm(cx, this, &confirm, &groups, summary, colors)
        });

        let this_keys = this.clone();
        let groups_keys = groups.clone();
        let filter_input_keys = self.dedup_query_input.clone();

        div()
            .id("dedup-root")
            .track_focus(&self.dedup_focus)
            .key_context("KanalibDedup")
            .on_key_down(move |event, window, cx| {
                let forward = match event.keystroke.key.as_str() {
                    "j" | "n" | "down" | "pagedown" => true,
                    "k" | "p" | "up" | "pageup" => false,
                    _ => return,
                };
                if event.keystroke.modifiers.control
                    || event.keystroke.modifiers.alt
                    || event.keystroke.modifiers.platform
                {
                    return;
                }
                // 筛选输入框获得焦点时不拦截字母键，避免无法输入组名。
                if let Some(input) = &filter_input_keys
                    && window.focused(cx) == Some(input.read(cx).focus_handle(cx))
                {
                    return;
                }
                cx.stop_propagation();
                let groups = groups_keys.clone();
                let _ = this_keys.update(cx, |app, cx| {
                    if app.dedup_confirm.is_some() {
                        return;
                    }
                    app.advance_dedup(&groups, forward);
                    cx.notify();
                });
            })
            .relative()
            .w(width)
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(colors.bg)
            .child(header)
            .child(filter_bar)
            .children(threshold_bar)
            .child(sort_bar)
            .children(audit_bar)
            .children(banner)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .flex()
                    .flex_row()
                    .child(rail)
                    .child(detail),
            )
            .children(overlay)
    }

    pub(crate) fn render_dedup_detail(
        &mut self,
        cx: &App,
        this: &Entity<Self>,
        group: &DuplicateGroup,
        groups: Arc<Vec<DuplicateGroup>>,
        scanning: bool,
        detail_width: Pixels,
        protected_paths: &[PathBuf],
        colors: UiColors,
    ) -> AnyElement {
        let this_prev = this.clone();
        let this_next = this.clone();
        let this_show_all = this.clone();
        let groups_prev = groups.clone();
        let groups_next = groups.clone();
        let pending = group.state == ReviewState::Pending;
        let editable = pending && !scanning;
        let keeper = group.keeper_asset();
        let duplicate_count = group.assets.len().saturating_sub(1);
        let reclaimable = group.reclaimable();
        let group_kind = group.kind;
        let kind_label = match group.kind {
            GroupKind::Exact => t(cx, "dedup.detail.kind_exact"),
            GroupKind::Similar => t(cx, "dedup.detail.kind_similar"),
        };
        let similarity = group
            .similarity_pct()
            .map(|pct| {
                t_args(cx, "dedup.detail.similarity", &[("pct", &pct.to_string())]).to_string()
            })
            .unwrap_or_default();
        let protected_duplicate = group
            .duplicate_paths()
            .iter()
            .any(|path| is_protected(path, protected_paths));
        let has_protected = group
            .assets
            .iter()
            .any(|asset| is_protected(&asset.path, protected_paths));
        let group_remote = any_remote_asset(group.assets.iter().map(|asset| asset.path.as_path()));

        let open = keeper.map(|asset| {
            let path = asset.path.clone();
            let remote = is_remote_asset(&path);
            let label = if remote {
                t(cx, "dedup.open_remote")
            } else if asset.is_video() {
                t(cx, "dedup.open_video")
            } else {
                t(cx, "dedup.open")
            };
            let open_toaster = self.toaster.clone();
            let this_open = this.clone();
            Button::new("dedup-open", label)
                .disabled(remote)
                .on_click(move |_, _, cx| {
                    if let Err(error) = open_asset_with_system(&path) {
                        // F-073 6.2：去重页 banner 会把 shared.error 优先显示为 danger 就地说明。
                        let message = t_args(
                            cx,
                            "dedup.open_failed",
                            &[("error", &error.to_string())],
                        )
                        .to_string();
                        let retry_path = path.clone();
                        let _ = this_open.update(cx, |app, cx| {
                            app.set_ui_error_with_retry(
                                message,
                                UiRetryAction::OpenWithSystem(retry_path),
                                cx,
                            );
                        });
                        push_toast(&open_toaster, cx, error, Severity::Danger);
                    }
                })
        });
        let this_compare = this.clone();
        let group_for_compare = group.clone();
        let compare_button = (group.assets.len() >= 2).then(|| {
            let group = group_for_compare.clone();
            let this = this_compare.clone();
            Button::new(
                "dedup-compare-open",
                if group.assets.len() > 4 {
                    t(cx, "dedup.compare_first4")
                } else {
                    t(cx, "dedup.compare_group")
                },
            )
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |app, cx| app.open_compare_group(&group, cx));
            })
        });

        let compact_header = detail_width < px(720.);
        let header = div()
            .flex_none()
            .px(px(16.))
            .py(px(11.))
            .border_b_1()
            .border_color(colors.border)
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .when(compact_header, |header| header.flex_col().items_start())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .when(compact_header, |title| title.w_full())
                    .when(!compact_header, |title| title.flex_1().min_w(px(0.)))
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t_args(
                                cx,
                                "dedup.detail.header",
                                &[
                                    ("count", &group.assets.len().to_string()),
                                    ("kind", &kind_label),
                                    ("state", &group_state_label(cx, group.state)),
                                    (
                                        "protected",
                                        &if has_protected {
                                            t(cx, "dedup.detail.protected").to_string()
                                        } else {
                                            String::new()
                                        },
                                    ),
                                    ("similarity", &similarity),
                                ],
                            )),
                    )
                    .child(div().truncate().text_xs().text_color(colors.muted).child(
                        t_args(
                            cx,
                            "dedup.detail.meta",
                            &[
                                (
                                    "reason",
                                    &localize_dedup_reason(cx, &group.suggested_reason),
                                ),
                                ("bytes", &format_bytes(reclaimable)),
                                (
                                    "keeper",
                                    &keeper.map(Asset::title).unwrap_or_else(|| {
                                        t(cx, "dedup.detail.no_keeper").to_string()
                                    }),
                                ),
                            ],
                        ),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(5.))
                    .when(compact_header, |actions| actions.w_full().justify_end())
                    .when(!compact_header, |actions| actions.flex_none())
                    .child(Button::new("dedup-prev", t(cx, "dedup.prev")).on_click(
                        move |_, _, cx| {
                            let _ = this_prev.update(cx, |app, cx| {
                                app.advance_dedup(&groups_prev, false);
                                cx.notify();
                            });
                        },
                    ))
                    .child(Button::new("dedup-next", t(cx, "dedup.next")).on_click(
                        move |_, _, cx| {
                            let _ = this_next.update(cx, |app, cx| {
                                app.advance_dedup(&groups_next, true);
                                cx.notify();
                            });
                        },
                    ))
                    .children(open)
                    .children(compare_button)
                    .children((group.assets.len() > DEDUP_MAX_VISIBLE).then(|| {
                        Button::new(
                            "dedup-show-all",
                            if self.dedup_show_all {
                                t(cx, "dedup.collapse").to_string()
                            } else {
                                t_args(
                                    cx,
                                    "dedup.show_all",
                                    &[("n", &group.assets.len().to_string())],
                                )
                                .to_string()
                            },
                        )
                        .on_click(move |_, _, cx| {
                            let _ = this_show_all.update(cx, |app, cx| {
                                app.dedup_show_all = !app.dedup_show_all;
                                cx.notify();
                            });
                        })
                    })),
            );

        let shown_assets = if self.dedup_show_all {
            group.assets.iter().collect::<Vec<_>>()
        } else {
            group
                .assets
                .iter()
                .take(DEDUP_MAX_VISIBLE)
                .collect::<Vec<_>>()
        };
        let side_by_side = detail_width >= px(1050.);
        let cards = shown_assets
            .iter()
            .map(|asset| {
                dedup_asset_card(
                    cx,
                    group,
                    asset,
                    group.keeper.as_ref() == Some(&asset.path),
                    group.suggested == asset.path,
                    editable,
                    this.clone(),
                    &self.thumb_cache,
                    colors,
                    protected_paths,
                )
            })
            .collect::<Vec<_>>();

        let compare: AnyElement = if side_by_side {
            div()
                .id("dedup-compare")
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .px(px(16.))
                .py(px(12.))
                .flex()
                .items_start()
                .gap(px(10.))
                .overflow_x_scroll()
                .children(cards)
                .into_any_element()
        } else {
            div()
                .id("dedup-compare")
                .flex_none()
                .h(px(324.))
                .px(px(16.))
                .py(px(12.))
                .flex()
                .items_start()
                .gap(px(10.))
                .overflow_x_scroll()
                .children(cards)
                .child(
                    div()
                        .flex_none()
                        .w(px(140.))
                        .min_h(px(220.))
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap(px(5.))
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(t(cx, "dedup.note.click_to_keep"))
                        .child(t(cx, "dedup.note.keeper_safe"))
                        .child(t(cx, "dedup.note.legend")),
                )
                .into_any_element()
        };

        let diff = dedup_diff(cx, group, &shown_assets, colors);
        let body: AnyElement = if side_by_side {
            div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .flex_row()
                .child(compare)
                .child(
                    div()
                        .id("dedup-diff-panel")
                        .w(px(460.))
                        .flex_none()
                        .h_full()
                        .overflow_y_scroll()
                        .border_l_1()
                        .border_color(colors.border)
                        .bg(colors.surface)
                        .child(diff),
                )
                .into_any_element()
        } else {
            div()
                .id("dedup-detail-scroll")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(compare)
                .child(diff)
                .into_any_element()
        };

        let this_ignore = this.clone();
        let this_related = this.clone();
        let this_later = this.clone();
        let this_keep = this.clone();
        let this_trash = this.clone();
        let groups_ignore = groups.clone();
        let groups_related = groups.clone();
        let groups_later = groups.clone();
        let groups_keep = groups.clone();
        let key = group.key.clone();

        let actions = if pending {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .child(
                    Button::new("dedup-ignore", t(cx, "dedup.action.not_duplicate"))
                        .disabled(scanning)
                        .on_click({
                            let key = key.clone();
                            move |_, _, cx| {
                                let _ = this_ignore.update(cx, |app, cx| {
                                    let groups = groups_ignore.clone();
                                    match app.state.mark_dedup(
                                        &key,
                                        ReviewState::Ignored,
                                        group_kind,
                                    ) {
                                        Ok(()) => {
                                            app.dedup_notice =
                                                Some(t(cx, "dedup.notice.ignored").to_string());
                                            app.advance_dedup(&groups, true);
                                        }
                                        Err(error) => {
                                            app.dedup_notice = Some(
                                                t_args(
                                                    cx,
                                                    "dedup.notice.failed",
                                                    &[("error", &format!("{error:#}"))],
                                                )
                                                .to_string(),
                                            );
                                        }
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                )
                .children((group_kind == GroupKind::Similar).then(|| {
                    Button::new("dedup-related", t(cx, "dedup.state.related"))
                        .disabled(scanning)
                        .on_click({
                            let key = key.clone();
                            move |_, _, cx| {
                                let _ = this_related.update(cx, |app, cx| {
                                    let groups = groups_related.clone();
                                    match app.state.mark_dedup(
                                        &key,
                                        ReviewState::Related,
                                        GroupKind::Similar,
                                    ) {
                                        Ok(()) => {
                                            app.dedup_notice =
                                                Some(t(cx, "dedup.notice.related").to_string());
                                            app.advance_dedup(&groups, true);
                                        }
                                        Err(error) => {
                                            app.dedup_notice = Some(
                                                t_args(
                                                    cx,
                                                    "dedup.notice.failed",
                                                    &[("error", &format!("{error:#}"))],
                                                )
                                                .to_string(),
                                            );
                                        }
                                    }
                                    cx.notify();
                                });
                            }
                        })
                }))
                .child(
                    Button::new("dedup-later", t(cx, "dedup.action.later")).on_click(
                        move |_, _, cx| {
                            let _ = this_later.update(cx, |app, cx| {
                                app.advance_dedup(&groups_later, true);
                                cx.notify();
                            });
                        },
                    ),
                )
                .child(
                    Button::new("dedup-keep-all", t(cx, "dedup.action.keep_all"))
                        .disabled(scanning)
                        .on_click({
                            let key = key.clone();
                            move |_, _, cx| {
                                let _ = this_keep.update(cx, |app, cx| {
                                    let groups = groups_keep.clone();
                                    match app.state.mark_dedup(&key, ReviewState::Done, group_kind)
                                    {
                                        Ok(()) => {
                                            app.dedup_notice =
                                                Some(t(cx, "dedup.notice.kept").to_string());
                                            app.advance_dedup(&groups, true);
                                        }
                                        Err(error) => {
                                            app.dedup_notice = Some(
                                                t_args(
                                                    cx,
                                                    "dedup.notice.failed",
                                                    &[("error", &format!("{error:#}"))],
                                                )
                                                .to_string(),
                                            );
                                        }
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(
                    Button::new(
                        "dedup-trash",
                        t_args(
                            cx,
                            "dedup.action.trash",
                            &[("n", &duplicate_count.to_string())],
                        ),
                    )
                    .variant(ButtonVariant::Danger)
                    .disabled(scanning || keeper.is_none() || protected_duplicate || group_remote)
                    .on_click(move |_, _, cx| {
                        let _ = this_trash.update(cx, |app, cx| {
                            app.dedup_confirm = Some(DedupConfirm::Group(key.clone()));
                            cx.notify();
                        });
                    }),
                )
                .into_any_element()
        } else {
            let this_reopen = this.clone();
            let key = group.key.clone();
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .child(
                    Button::new("dedup-reopen", t(cx, "dedup.action.reopen"))
                        .disabled(scanning)
                        .on_click(move |_, _, cx| {
                            let _ = this_reopen.update(cx, |app, cx| {
                                match app.state.mark_dedup(&key, ReviewState::Pending, group_kind) {
                                    Ok(()) => {
                                        app.dedup_notice =
                                            Some(t(cx, "dedup.notice.reopened").to_string());
                                        app.dedup_selected = Some(key.clone());
                                    }
                                    Err(error) => {
                                        app.dedup_notice = Some(
                                            t_args(
                                                cx,
                                                "dedup.notice.failed",
                                                &[("error", &format!("{error:#}"))],
                                            )
                                            .to_string(),
                                        );
                                    }
                                }
                                cx.notify();
                            });
                        }),
                )
                .into_any_element()
        };

        let action_bar = div()
            .flex_none()
            .pt(px(10.))
            .pb(px(14.))
            .px(px(16.))
            .border_t_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
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
                            .child(t_args(
                                cx,
                                "dedup.action_bar.summary",
                                &[
                                    (
                                        "mode",
                                        &if group_kind == GroupKind::Similar {
                                            t(cx, "dedup.action_bar.mode_similar").to_string()
                                        } else {
                                            t(cx, "dedup.action_bar.mode_exact").to_string()
                                        },
                                    ),
                                    ("n", &duplicate_count.to_string()),
                                    ("bytes", &format_bytes(reclaimable)),
                                ],
                            )),
                    )
                    .child(div().truncate().text_xs().text_color(colors.subtle).child(
                        if scanning {
                            t(cx, "dedup.action_bar.scanning")
                        } else if group_remote {
                            t(cx, "dedup.action_bar.remote")
                        } else if protected_duplicate {
                            t(cx, "dedup.action_bar.protected")
                        } else {
                            t(cx, "dedup.action_bar.safe")
                        },
                    )),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .child(actions),
            );

        div()
            .relative()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .child(body)
            .child(action_bar)
            .into_any_element()
    }

    pub(crate) fn render_dedup_confirm(
        &self,
        cx: &App,
        this: &Entity<Self>,
        confirm: &DedupConfirm,
        groups: &Arc<Vec<DuplicateGroup>>,
        summary: DedupSummary,
        colors: UiColors,
    ) -> Option<AnyElement> {
        let (title, detail, rows, key, remote) = match confirm {
            DedupConfirm::Group(key) => {
                let group = groups.iter().find(|group| &group.key == key)?;
                let keeper = group.keeper_asset()?;
                let duplicates = group
                    .assets
                    .iter()
                    .filter(|asset| asset.path != keeper.path)
                    .collect::<Vec<_>>();
                if duplicates.is_empty() {
                    return None;
                }
                let remote =
                    any_remote_asset(group.assets.iter().map(|asset| asset.path.as_path()));
                let rows = duplicates
                    .iter()
                    .take(6)
                    .map(|asset| (asset.title(), format_bytes(asset.bytes)))
                    .collect::<Vec<_>>();
                (
                    t(cx, "dedup.confirm.title").to_string(),
                    t_args(
                        cx,
                        "dedup.confirm.group_detail",
                        &[
                            ("n", &duplicates.len().to_string()),
                            (
                                "kind",
                                &if group.kind == GroupKind::Similar {
                                    t(cx, "dedup.confirm.kind_similar").to_string()
                                } else {
                                    String::new()
                                },
                            ),
                            ("keeper", &keeper.title()),
                            (
                                "note",
                                &if group.kind == GroupKind::Similar {
                                    t(cx, "dedup.confirm.similar_note").to_string()
                                } else {
                                    String::new()
                                },
                            ),
                        ],
                    )
                    .to_string(),
                    rows,
                    Some(key.clone()),
                    remote,
                )
            }
            DedupConfirm::Batch => {
                let pending = groups
                    .iter()
                    .filter(|group| group.state == ReviewState::Pending)
                    .collect::<Vec<_>>();
                if pending.is_empty() {
                    return None;
                }
                let asset_count = pending
                    .iter()
                    .map(|group| group.assets.len().saturating_sub(1))
                    .sum::<usize>();
                let remote = pending.iter().any(|group| {
                    any_remote_asset(group.assets.iter().map(|asset| asset.path.as_path()))
                });
                let rows = pending
                    .iter()
                    .take(6)
                    .map(|group| {
                        (
                            group.title(),
                            t_args(
                                cx,
                                "dedup.confirm.copies",
                                &[("n", &group.assets.len().saturating_sub(1).to_string())],
                            )
                            .to_string(),
                        )
                    })
                    .collect::<Vec<_>>();
                (
                    t(cx, "dedup.confirm.batch_title").to_string(),
                    t_args(
                        cx,
                        "dedup.confirm.batch_detail",
                        &[
                            ("groups", &summary.pending.to_string()),
                            ("assets", &asset_count.to_string()),
                            ("bytes", &format_bytes(summary.pending_bytes)),
                        ],
                    )
                    .to_string(),
                    rows,
                    None,
                    remote,
                )
            }
        };
        let detail = if remote {
            t_args(cx, "dedup.confirm.remote_suffix", &[("detail", &detail)]).to_string()
        } else {
            detail
        };
        // F-047：移入前先记录预计释放量；成功后连同结果写入 settings JSON。
        let planned_bytes = planned_reclaim_bytes(groups, key.as_deref());
        let audit_scope = if key.is_some() { "group" } else { "batch" };

        let close_app = this.clone();
        let this_confirm = this.clone();
        let cancel_label = t(cx, "dedup.confirm.cancel");
        let confirm_label = t(cx, "dedup.confirm.confirm");
        let overlay = Dialog::new("dedup-confirm-dialog", title, move |_, cx| {
            let _ = close_app.update(cx, |app, cx| {
                app.dedup_confirm = None;
                cx.notify();
            });
        })
        .detail(detail)
        .child(
            div()
                .rounded(px(RADIUS_LG))
                .border_1()
                .border_color(colors.border)
                .overflow_hidden()
                .children(rows.iter().enumerate().map(|(index, (name, size))| {
                    div()
                        .px(px(8.))
                        .py(px(5.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .bg(if index % 2 == 0 {
                            colors.surface
                        } else {
                            colors.sunken
                        })
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(colors.fg)
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(colors.subtle)
                                .child(size.clone()),
                        )
                })),
        )
        .action(move |close| {
            Button::new("dedup-cancel", cancel_label)
                .on_click(move |_, window, cx| close(window, cx))
        })
        .action(move |_| {
            Button::new("dedup-confirm", confirm_label)
                .variant(ButtonVariant::Danger)
                .disabled(remote)
                .on_click(move |_, _, cx| {
                    let _ = this_confirm.update(cx, |app, cx| {
                        let result = match &key {
                            Some(key) => app.state.trash_dedup_group(key),
                            None => app.state.trash_preselected(),
                        };
                        match result {
                            Ok(done) => {
                                let audit = TrashAudit {
                                    at: now_unix_secs(),
                                    groups: done.groups,
                                    assets: done.assets,
                                    failed: done.failed,
                                    protected_skipped: done.protected_skipped,
                                    bytes: planned_bytes,
                                    scope: audit_scope.to_string(),
                                };
                                app.persist_setting("dedup.last_trash", audit.to_json(), cx);
                                app.last_trash_audit = Some(audit);
                                app.dedup_notice = Some(
                                    t_args(
                                        cx,
                                        "dedup.notice.trashed",
                                        &[
                                            ("groups", &done.groups.to_string()),
                                            ("assets", &done.assets.to_string()),
                                            (
                                                "failed",
                                                &if done.failed > 0 {
                                                    t_args(
                                                        cx,
                                                        "dedup.notice.trash_failed",
                                                        &[("n", &done.failed.to_string())],
                                                    )
                                                    .to_string()
                                                } else {
                                                    String::new()
                                                },
                                            ),
                                            (
                                                "skipped",
                                                &if done.protected_skipped > 0 {
                                                    t_args(
                                                        cx,
                                                        "dedup.notice.trash_skipped",
                                                        &[(
                                                            "n",
                                                            &done.protected_skipped.to_string(),
                                                        )],
                                                    )
                                                    .to_string()
                                                } else {
                                                    String::new()
                                                },
                                            ),
                                        ],
                                    )
                                    .to_string(),
                                );
                            }
                            Err(error) => {
                                app.dedup_notice = Some(
                                    t_args(
                                        cx,
                                        "dedup.notice.trash_error",
                                        &[("error", &format!("{error:#}"))],
                                    )
                                    .to_string(),
                                );
                            }
                        }
                        app.dedup_confirm = None;
                        app.dedup_selected = None;
                        cx.notify();
                    });
                })
        })
        .into_any_element();
        Some(overlay)
    }
}

/// 结果视图内的本地筛选：名称、路径、自动标签与手工标签，大小写不敏感。
fn similar_filter_matches(asset: &Asset, filter_lower: &str) -> bool {
    asset.title().to_lowercase().contains(filter_lower)
        || asset
            .path
            .display()
            .to_string()
            .to_lowercase()
            .contains(filter_lower)
        || asset
            .all_tags()
            .any(|tag| tag.tag.to_lowercase().contains(filter_lower))
}

/// 阈值预览的采样说明；精确计算时不写「抽样」。
fn preview_sample_note_text(cx: &App, preview: &SimilarThresholdPreview) -> String {
    if preview.sampled {
        let pct = if preview.total_assets > 0 {
            preview.sampled_assets as f64 * 100.0 / preview.total_assets as f64
        } else {
            0.0
        };
        t_args(
            cx,
            "dedup.preview.sampled",
            &[
                ("sampled", &preview.sampled_assets.to_string()),
                ("total", &preview.total_assets.to_string()),
                ("pct", &format!("{pct:.1}")),
            ],
        )
        .to_string()
    } else {
        t_args(
            cx,
            "dedup.preview.full",
            &[("n", &preview.total_assets.to_string())],
        )
        .to_string()
    }
}

/// 测试用的中文快照；界面文本走 `dedup.preview.*` 目录。
#[cfg(test)]
fn preview_sample_note(preview: &SimilarThresholdPreview) -> String {
    if preview.sampled {
        let pct = if preview.total_assets > 0 {
            preview.sampled_assets as f64 * 100.0 / preview.total_assets as f64
        } else {
            0.0
        };
        format!(
            "抽样 {} / {} 张（约 {pct:.1}%）",
            preview.sampled_assets, preview.total_assets
        )
    } else {
        format!("全量 {} 张", preview.total_assets)
    }
}

/// 结果行：缩略图 + 名称/路径 + 距离/相似度；整行可点选，右键复用图库菜单。
fn similar_hit_row(
    cx: &App,
    asset: &Asset,
    distance: u32,
    similarity_pct: u32,
    selected: bool,
    this: Entity<KanalibApp>,
    thumb_cache: Entity<ThumbCache>,
    focus: FocusHandle,
    colors: UiColors,
) -> AnyElement {
    let path = asset.path.clone();
    let title = asset.title();
    let path_text = path.display().to_string();
    let menu = crate::views::asset_context_menu(asset, &this, cx);
    let thumb = asset.tile_thumb.clone().or_else(|| asset.thumb.clone());
    let media: AnyElement = match thumb {
        Some(thumb) => img(thumb)
            .image_cache(&thumb_cache)
            .w(px(52.))
            .h(px(52.))
            .flex_none()
            .rounded(px(6.))
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        None => div()
            .w(px(52.))
            .h(px(52.))
            .flex_none()
            .rounded(px(6.))
            .bg(colors.sunken)
            .into_any_element(),
    };
    let this_click = this.clone();
    let path_click = path.clone();
    let focus_click = focus.clone();
    let this_right = this.clone();
    let path_right = path.clone();
    let focus_right = focus.clone();
    let row = div()
        .id(SharedString::from(format!(
            "similar-hit-{}",
            path.display()
        )))
        .role(Role::Button)
        .aria_label(title.clone())
        .w_full()
        .h(px(SIMILAR_ROW_HEIGHT))
        .px(px(12.))
        .rounded(px(7.))
        .cursor_pointer()
        .bg(if selected {
            colors.selection
        } else {
            colors.bg
        })
        .hover(move |style| style.bg(colors.hover))
        .on_click(move |event, window, cx| {
            window.focus(&focus_click, cx);
            let (modifiers, click_count) = match event {
                ClickEvent::Mouse(event) => (event.down.modifiers, event.down.click_count),
                ClickEvent::Keyboard(_) | ClickEvent::Touch(_) => (Modifiers::default(), 1),
            };
            let path = path_click.clone();
            let _ = this_click.update(cx, |app, cx| {
                app.handle_tile_click(path, modifiers, click_count, cx)
            });
        })
        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
            window.focus(&focus_right, cx);
            let _ = this_right.update(cx, |app, cx| {
                app.select_single_path(path_right.clone(), cx);
            });
        })
        .flex()
        .items_center()
        .gap(px(10.))
        .child(media)
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
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(path_text),
                ),
        )
        .child(small_badge(
            t_args(cx, "dedup.hit.distance", &[("n", &distance.to_string())]).to_string(),
            colors.warning,
            colors.warning_subtle,
        ))
        .child(small_badge(
            t_args(
                cx,
                "dedup.kind.similar",
                &[("pct", &similarity_pct.to_string())],
            )
            .to_string(),
            colors.accent,
            colors.selection,
        ));
    div()
        .w_full()
        .child(
            ContextMenu::new(
                SharedString::from(format!("similar-context-{}", path.display())),
                menu,
            )
            .w_full()
            .child(row),
        )
        .into_any_element()
}

impl KanalibApp {
    // ---- F-004：检查器 / 右键 / 命令面板「查找相似」 ----

    /// 以 `anchor` 为锚点打开临时结果视图；默认沿用已保存的相似阈值。
    pub(crate) fn open_similar(&mut self, anchor: PathBuf, cx: &mut Context<Self>) {
        let threshold = {
            let shared = self.state.shared.lock().unwrap();
            shared.dedup.similar_threshold
        };
        self.similar_view = Some(SimilarView {
            anchor,
            threshold,
            scanned: 0,
            truncated: false,
            hits: Vec::new(),
            error: None,
        });
        self.similar_filter.clear();
        if let Some(input) = &self.similar_filter_input {
            input.update(cx, |input, cx| input.set_text("", cx));
        }
        self.similar_visible = Arc::new(Vec::new());
        self.similar_visible_revision = u64::MAX;
        self.similar_visible_filter.clear();
        self.screen = Screen::Similar;
        self.dedup_confirm = None;
        self.clear_multi_selection();
        self.state.select(None);
        self.auto_select_pending = true;
        self.focus_grid_pending = true;
        self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.refresh_similar_query(cx);
    }

    /// 命令面板入口：以当前选中（或可见第一张）为锚点。
    pub(crate) fn open_similar_selected(&mut self, cx: &mut Context<Self>) {
        let path = self
            .state
            .shared
            .lock()
            .unwrap()
            .selected
            .clone()
            .or_else(|| self.selection.iter().next().cloned())
            .or_else(|| self.visible_paths().first().cloned());
        match path {
            Some(path) => self.open_similar(path, cx),
            None => {
                let message = t(cx, "dedup.similar.select_first");
                crate::app::push_toast(&self.toaster, cx, message, Severity::Warning)
            }
        }
    }

    /// 离开临时结果视图并丢弃结果；不写库、不留缓存。
    pub(crate) fn close_similar(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::Library;
        self.similar_view = None;
        self.similar_visible = Arc::new(Vec::new());
        self.similar_visible_revision = u64::MAX;
        self.similar_visible_filter.clear();
        self.similar_refresh_task = None;
        self.clear_multi_selection();
        self.state.select(None);
        self.auto_select_pending = true;
        self.focus_grid_pending = true;
        self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// 重新执行当前锚点查询；失败信息留在视图内，不弹全局错误。
    pub(crate) fn refresh_similar_query(&mut self, cx: &mut Context<Self>) {
        let Some((anchor, threshold)) = self
            .similar_view
            .as_ref()
            .map(|view| (view.anchor.clone(), view.threshold))
        else {
            return;
        };
        match self
            .state
            .similar_to_anchor(&anchor, threshold, SIMILAR_RESULT_LIMIT)
        {
            Ok(result) => {
                if let Some(view) = self.similar_view.as_mut() {
                    view.threshold = result.threshold;
                    view.scanned = result.scanned;
                    view.truncated = result.truncated;
                    view.hits = result.hits;
                    view.error = None;
                }
            }
            Err(error) => {
                if let Some(view) = self.similar_view.as_mut() {
                    view.hits.clear();
                    view.scanned = 0;
                    view.truncated = false;
                    view.error = Some(
                        t_args(
                            cx,
                            "dedup.similar.query_failed",
                            &[("error", &format!("{error:#}"))],
                        )
                        .to_string(),
                    );
                }
            }
        }
        self.similar_visible_revision = u64::MAX;
        self.clear_multi_selection();
        self.auto_select_pending = true;
        self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// 结果视图的阈值入口：只改这一次查询的临时阈值，不写默认值、不清空评审状态。
    pub(crate) fn set_similar_view_threshold(&mut self, threshold: u8, cx: &mut Context<Self>) {
        let threshold = threshold.min(32);
        let Some(view) = self.similar_view.as_mut() else {
            return;
        };
        if view.threshold == threshold {
            return;
        }
        view.threshold = threshold;
        let weak = cx.entity().downgrade();
        self.similar_refresh_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SIMILAR_REFRESH_DEBOUNCE_MS))
                .await;
            let _ = weak.update(cx, |app, cx| app.refresh_similar_query(cx));
        }));
        cx.notify();
    }

    /// 把当前查询命中映射成资产下标；只在查询/筛选/数据版本变化时调用。
    pub(crate) fn rebuild_similar_visible(
        &mut self,
        assets: &Arc<RwLock<Vec<Asset>>>,
        revision: u64,
    ) {
        let hits = self
            .similar_view
            .as_ref()
            .map(|view| view.hits.clone())
            .unwrap_or_default();
        let filter = self.similar_filter.trim().to_lowercase();
        let mut visible = Vec::new();
        {
            let assets = assets.read().unwrap();
            if !hits.is_empty() {
                let index_of = assets
                    .iter()
                    .enumerate()
                    .map(|(index, asset)| (asset.path.as_path(), index))
                    .collect::<HashMap<&Path, usize>>();
                for hit in &hits {
                    let Some(&index) = index_of.get(hit.path.as_path()) else {
                        continue;
                    };
                    if filter.is_empty() || similar_filter_matches(&assets[index], &filter) {
                        visible.push(index);
                    }
                }
            }
        }
        self.similar_visible = Arc::new(visible);
        self.similar_visible_revision = revision;
        self.similar_visible_filter = self.similar_filter.clone();
    }
}

impl KanalibApp {
    // ---- F-045：0–32 滑杆、临时值/默认值与后台预览 ----

    pub(crate) fn set_settings_threshold_draft(&mut self, threshold: u8, cx: &mut Context<Self>) {
        let value = threshold.min(32);
        self.settings_threshold_draft = Some(value);
        self.schedule_threshold_preview(value, cx);
        cx.notify();
    }

    pub(crate) fn set_dedup_threshold_draft(&mut self, threshold: u8, cx: &mut Context<Self>) {
        let value = threshold.min(32);
        self.dedup_threshold_draft = Some(value);
        self.schedule_threshold_preview(value, cx);
        cx.notify();
    }

    /// 放弃临时值，回到已保存的默认阈值并刷新预览。
    pub(crate) fn discard_threshold_draft(&mut self, settings: bool, cx: &mut Context<Self>) {
        if settings {
            self.settings_threshold_draft = None;
        } else {
            self.dedup_threshold_draft = None;
        }
        let default = self.state.shared.lock().unwrap().dedup.similar_threshold;
        self.schedule_threshold_preview(default, cx);
        cx.notify();
    }

    /// 把候选阈值保存为默认值；只写设置，不清空任何评审状态。
    pub(crate) fn apply_threshold_default(&mut self, threshold: u8, cx: &mut Context<Self>) {
        let saved = self.state.set_similar_threshold(threshold);
        self.settings_threshold_draft = None;
        self.dedup_threshold_draft = None;
        self.schedule_threshold_preview(saved, cx);
        let message = t_args(
            cx,
            "dedup.threshold.default_set",
            &[("n", &saved.to_string())],
        );
        crate::app::push_toast(&self.toaster, cx, message, Severity::Success);
        cx.notify();
    }

    /// 去重页：应用阈值并重新扫描相似组；`start_similar_scan` 会复用既有评审状态。
    pub(crate) fn apply_threshold_and_scan(&mut self, threshold: u8, cx: &mut Context<Self>) {
        let saved = self.state.set_similar_threshold(threshold);
        self.dedup_threshold_draft = None;
        self.state.start_similar_scan();
        self.dedup_selected = None;
        self.dedup_confirm = None;
        self.dedup_notice = Some(
            t_args(
                cx,
                "dedup.threshold.rescan_notice",
                &[("n", &saved.to_string())],
            )
            .to_string(),
        );
        self.schedule_threshold_preview(saved, cx);
        cx.notify();
    }

    /// 预览在后台线程执行（同步调用在大库可能 1.8–2.6s），并带 220ms 防抖。
    pub(crate) fn schedule_threshold_preview(&mut self, threshold: u8, cx: &mut Context<Self>) {
        let threshold = threshold.min(32);
        if self.similar_preview_running && self.similar_preview_threshold == Some(threshold) {
            return;
        }
        self.similar_preview_epoch = self.similar_preview_epoch.wrapping_add(1);
        let epoch = self.similar_preview_epoch;
        self.similar_preview_running = true;
        self.similar_preview_threshold = Some(threshold);
        let state = self.state.clone();
        let weak = cx.entity().downgrade();
        self.similar_preview_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SIMILAR_PREVIEW_DEBOUNCE_MS))
                .await;
            let task = cx
                .background_executor()
                .spawn(async move { state.preview_similar_threshold(threshold) });
            let preview = task.await;
            let _ = weak.update(cx, |app, cx| {
                if app.similar_preview_epoch != epoch {
                    return;
                }
                app.similar_preview_running = false;
                app.similar_preview_threshold = Some(preview.threshold);
                app.similar_preview = Some(preview);
                cx.notify();
            });
        }));
    }

    /// 进入设置页/去重页后按需预览一次；渲染路径只在这里触发后台任务。
    pub(crate) fn maybe_schedule_threshold_preview(&mut self, cx: &mut Context<Self>) {
        let default = self.state.shared.lock().unwrap().dedup.similar_threshold;
        let threshold = match self.screen {
            Screen::Settings => self.settings_threshold_draft.unwrap_or(default),
            Screen::Duplicates => self.dedup_threshold_draft.unwrap_or(default),
            _ => return,
        };
        if self.similar_preview_running || self.similar_preview_threshold == Some(threshold) {
            return;
        }
        self.schedule_threshold_preview(threshold, cx);
    }
}

impl KanalibApp {
    /// 设置页与去重页共用的阈值编辑器：滑杆 + 6/10/14 快捷档 + 预估 + 链式风险。
    pub(crate) fn render_threshold_editor(
        &self,
        cx: &App,
        this: &Entity<Self>,
        default: u8,
        draft: Option<u8>,
        settings: bool,
        colors: UiColors,
    ) -> AnyElement {
        let effective = draft.unwrap_or(default);
        let prefix = if settings { "settings" } else { "dedup" };
        // F-073 6.2：应用并扫描会启动耗时扫描，进行中给出 spinner 且不再重复触发。
        let scan_running = self.state.shared.lock().unwrap().scanning;
        let mut presets = div().flex().items_center().gap(px(5.));
        for (value, label) in [
            (6u8, t(cx, "dedup.threshold.preset_strict")),
            (10u8, t(cx, "dedup.threshold.preset_standard")),
            (14u8, t(cx, "dedup.threshold.preset_loose")),
        ] {
            let this_preset = this.clone();
            presets = presets.child(
                Button::new(
                    SharedString::from(format!("{prefix}-sim-preset-{value}")),
                    label,
                )
                .size(ControlSize::Sm)
                .variant(if effective == value {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Secondary
                })
                .on_click(move |_, _, cx| {
                    let _ = this_preset.update(cx, |app, cx| {
                        if settings {
                            app.set_settings_threshold_draft(value, cx);
                        } else {
                            app.set_dedup_threshold_draft(value, cx);
                        }
                    });
                }),
            );
        }
        let this_slider = this.clone();
        let slider = div().w(px(200.)).child(
            Slider::new(
                SharedString::from(format!("{prefix}-similar-threshold-slider")),
                effective as f64,
            )
            .range(0., 32.)
            .step(1.)
            .on_change(move |value, _, cx| {
                let value = value.round().clamp(0., 32.) as u8;
                let _ = this_slider.update(cx, |app, cx| {
                    if settings {
                        app.set_settings_threshold_draft(value, cx);
                    } else {
                        app.set_dedup_threshold_draft(value, cx);
                    }
                });
            }),
        );

        let draft_label: AnyElement = match draft {
            Some(value) if value != default => div()
                .text_xs()
                .text_color(colors.warning)
                .child(t_args(
                    cx,
                    "dedup.threshold.draft_changed",
                    &[
                        ("value", &value.to_string()),
                        ("default", &default.to_string()),
                    ],
                ))
                .into_any_element(),
            Some(value) => div()
                .text_xs()
                .text_color(colors.muted)
                .child(t_args(
                    cx,
                    "dedup.threshold.draft_same",
                    &[("value", &value.to_string())],
                ))
                .into_any_element(),
            None => div()
                .text_xs()
                .text_color(colors.muted)
                .child(t_args(
                    cx,
                    "dedup.threshold.using_default",
                    &[("default", &default.to_string())],
                ))
                .into_any_element(),
        };

        let preview_note: AnyElement =
            if self.similar_preview_running && self.similar_preview_threshold == Some(effective) {
                div()
                    .text_xs()
                    .text_color(colors.accent)
                    .child(t(cx, "dedup.threshold.previewing"))
                    .into_any_element()
            } else if let Some(preview) = self
                .similar_preview
                .as_ref()
                .filter(|preview| preview.threshold == effective)
            {
                let mut row = div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(5.))
                    .child(div().text_xs().text_color(colors.fg).child(t_args(
                        cx,
                        "dedup.threshold.estimate",
                        &[
                            ("groups", &preview.group_count.to_string()),
                            ("members", &preview.member_count.to_string()),
                        ],
                    )))
                    .child(small_badge(
                        preview_sample_note_text(cx, preview),
                        colors.muted,
                        colors.sunken,
                    ));
                if preview.chain_risk {
                    row = row
                        .child(small_badge(
                            t_args(
                                cx,
                                "dedup.threshold.chain_risk",
                                &[("n", &preview.chain_groups.to_string())],
                            )
                            .to_string(),
                            colors.danger,
                            colors.sunken,
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors.warning)
                                .child(t(cx, "dedup.threshold.chain_hint")),
                        );
                }
                row.into_any_element()
            } else {
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(if self.similar_preview_running {
                        t(cx, "dedup.threshold.previewing_other")
                    } else {
                        t(cx, "dedup.threshold.no_preview")
                    })
                    .into_any_element()
            };

        let this_refresh = this.clone();
        let this_apply = this.clone();
        let this_scan = this.clone();
        let this_discard = this.clone();
        let refresh = Button::new(
            SharedString::from(format!("{prefix}-sim-preview-refresh")),
            t(cx, "dedup.threshold.refresh"),
        )
        .size(ControlSize::Sm)
        .on_click(move |_, _, cx| {
            let _ = this_refresh.update(cx, |app, cx| {
                app.schedule_threshold_preview(effective, cx);
            });
        });
        let apply = Button::new(
            SharedString::from(format!("{prefix}-sim-apply-default")),
            t_args(
                cx,
                "dedup.threshold.set_default",
                &[("value", &effective.to_string())],
            ),
        )
        .size(ControlSize::Sm)
        .variant(ButtonVariant::Primary)
        .on_click(move |_, _, cx| {
            let _ = this_apply.update(cx, |app, cx| app.apply_threshold_default(effective, cx));
        });
        let scan = (!settings).then(|| {
            Button::new(
                "dedup-sim-apply-scan",
                t_args(
                    cx,
                    "dedup.threshold.apply_scan",
                    &[("value", &effective.to_string())],
                ),
            )
            .size(ControlSize::Sm)
            .variant(ButtonVariant::Primary)
            .loading(scan_running)
            .on_click(move |_, _, cx| {
                let _ = this_scan.update(cx, |app, cx| app.apply_threshold_and_scan(effective, cx));
            })
        });
        let discard = draft.map(|_| {
            Button::new(
                SharedString::from(format!("{prefix}-sim-discard-draft")),
                t(cx, "dedup.threshold.discard"),
            )
            .size(ControlSize::Sm)
            .on_click(move |_, _, cx| {
                let _ =
                    this_discard.update(cx, |app, cx| app.discard_threshold_draft(settings, cx));
            })
        });

        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t_args(
                                cx,
                                "dedup.threshold.title",
                                &[
                                    ("value", &effective.to_string()),
                                    (
                                        "pct",
                                        &similarity_pct_from_distance(effective as u32).to_string(),
                                    ),
                                ],
                            )),
                    )
                    .child(draft_label)
                    .child(slider)
                    .child(presets),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(preview_note)
                    .child(div().flex_1())
                    .child(refresh)
                    .child(apply)
                    .children(scan)
                    .children(discard),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(t(cx, "dedup.threshold.footer")),
            )
            .into_any_element()
    }
}

impl KanalibApp {
    // ---- F-004：临时结果视图 ----

    pub(crate) fn render_similar(
        &mut self,
        cx: &App,
        this: &Entity<Self>,
        assets: Arc<RwLock<Vec<Asset>>>,
        visible_indices: Arc<Vec<usize>>,
        selection: Arc<HashSet<PathBuf>>,
        filter_input: &Entity<TextInput>,
        width: Pixels,
        colors: UiColors,
    ) -> AnyElement {
        let Some(view) = self.similar_view.as_ref() else {
            return div().into_any_element();
        };
        let anchor = view.anchor.clone();
        let threshold = view.threshold;
        let scanned = view.scanned;
        let truncated = view.truncated;
        let total_hits = view.hits.len();
        let error = view.error.clone();
        let scanning = self.state.shared.lock().unwrap().scanning;
        let distances = view
            .hits
            .iter()
            .map(|hit| (hit.path.clone(), (hit.distance, hit.similarity_pct)))
            .collect::<HashMap<PathBuf, (u32, u32)>>();

        let this_back = this.clone();
        let this_refresh = this.clone();
        let this_all = this.clone();
        let this_clear = this.clone();
        let this_quick = this.clone();
        let mut preset_row = div().flex().items_center().gap(px(5.));
        for (value, label) in [
            (6u8, t(cx, "dedup.threshold.preset_strict")),
            (10u8, t(cx, "dedup.threshold.preset_standard")),
            (14u8, t(cx, "dedup.threshold.preset_loose")),
        ] {
            let this_preset = this.clone();
            preset_row = preset_row.child(
                Button::new(
                    SharedString::from(format!("similar-view-preset-{value}")),
                    label,
                )
                .size(ControlSize::Sm)
                .variant(if threshold == value {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Secondary
                })
                .on_click(move |_, _, cx| {
                    let _ = this_preset.update(cx, |app, cx| {
                        app.set_similar_view_threshold(value, cx);
                    });
                }),
            );
        }
        let this_slider = this.clone();
        let slider = div().w(px(180.)).child(
            Slider::new("similar-view-threshold-slider", threshold as f64)
                .range(0., 32.)
                .step(1.)
                .on_change(move |value, _, cx| {
                    let value = value.round().clamp(0., 32.) as u8;
                    let _ = this_slider.update(cx, |app, cx| {
                        app.set_similar_view_threshold(value, cx);
                    });
                }),
        );

        let header = div()
            .flex_none()
            .px(px(16.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .border_b_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Button::new("similar-back", t(cx, "common.back_to_library"))
                            .size(ControlSize::Sm)
                            .icon(IconName::ChevronLeft)
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_, _, cx| {
                                let _ = this_back.update(cx, |app, cx| app.close_similar(cx));
                            }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t(cx, "page.similar")),
                    )
                    .child(
                        div()
                            .min_w(px(0.))
                            .truncate()
                            .text_xs()
                            .text_color(colors.muted)
                            .child(anchor.display().to_string()),
                    )
                    .child(div().flex_1())
                    .child(small_badge(
                        t_args(cx, "dedup.similar.compared", &[("n", &scanned.to_string())])
                            .to_string(),
                        colors.muted,
                        colors.sunken,
                    ))
                    .child(small_badge(
                        t_args(
                            cx,
                            "dedup.similar.results",
                            &[("n", &total_hits.to_string())],
                        )
                        .to_string(),
                        colors.accent,
                        colors.selection,
                    ))
                    .children(truncated.then(|| {
                        small_badge(
                            t_args(
                                cx,
                                "dedup.similar.truncated",
                                &[("n", &SIMILAR_RESULT_LIMIT.to_string())],
                            )
                            .to_string(),
                            colors.warning,
                            colors.warning_subtle,
                        )
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(10.))
                    .child(div().text_xs().text_color(colors.subtle).child(t_args(
                        cx,
                        "dedup.similar.threshold",
                        &[
                            ("threshold", &threshold.to_string()),
                            (
                                "pct",
                                &similarity_pct_from_distance(threshold as u32).to_string(),
                            ),
                        ],
                    )))
                    .child(slider)
                    .child(preset_row)
                    .child(
                        Button::new("similar-refresh", t(cx, "dedup.similar.refresh"))
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ = this_refresh
                                    .update(cx, |app, cx| app.refresh_similar_query(cx));
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(if scanning {
                                t(cx, "dedup.similar.scanning_note").to_string()
                            } else {
                                t(cx, "dedup.similar.readonly_note").to_string()
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(280.))
                            .on_mouse_down(MouseButton::Left, |_, window, _| {
                                window.prevent_default()
                            })
                            .child(
                                Input::new(filter_input)
                                    .clearable()
                                    .least(t(cx, "dedup.similar.filter_least")),
                            ),
                    )
                    .child(div().text_xs().text_color(colors.muted).child(t_args(
                        cx,
                        "dedup.similar.count",
                        &[
                            ("shown", &visible_indices.len().to_string()),
                            ("total", &total_hits.to_string()),
                        ],
                    )))
                    .children((!selection.is_empty()).then(|| {
                        div().text_xs().text_color(colors.accent).child(t_args(
                            cx,
                            "dedup.similar.selected",
                            &[("n", &selection.len().to_string())],
                        ))
                    }))
                    .child(div().flex_1())
                    .child(
                        Button::new("similar-select-all", t(cx, "dedup.similar.select_all"))
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ = this_all.update(cx, |app, cx| app.select_all_visible(cx));
                            }),
                    )
                    .child(
                        Button::new(
                            "similar-clear-selection",
                            t(cx, "dedup.similar.clear_selection"),
                        )
                        .size(ControlSize::Sm)
                        .on_click(move |_, _, cx| {
                            let _ = this_clear.update(cx, |app, cx| app.clear_selection(cx));
                        }),
                    )
                    .child(
                        Button::new("similar-quick-view", t(cx, "dedup.similar.quick_view"))
                            .size(ControlSize::Sm)
                            .icon(IconName::Maximize2)
                            .on_click(move |_, _, cx| {
                                let _ = this_quick.update(cx, |app, cx| app.open_quick_view(cx));
                            }),
                    ),
            );

        let body: AnyElement = if visible_indices.is_empty() {
            div()
                .flex_1()
                .p(px(24.))
                .text_sm()
                .text_color(colors.muted)
                .child(
                    error
                        .clone()
                        .unwrap_or_else(|| t(cx, "dedup.similar.empty").to_string()),
                )
                .into_any_element()
        } else {
            let assets_for_list = assets.clone();
            let indices_for_list = visible_indices.clone();
            let selection_for_list = selection.clone();
            let this_for_list = this.clone();
            let thumb_cache = self.thumb_cache.clone();
            let focus = self.grid_focus.clone();
            let distances = Arc::new(distances);
            uniform_list(
                "similar-hits",
                indices_for_list.len(),
                move |range, _, cx| {
                    let Ok(assets) = assets_for_list.read() else {
                        return Vec::new();
                    };
                    range
                        .map(|row| {
                            let index = indices_for_list[row];
                            let Some(asset) = assets.get(index) else {
                                return div().into_any_element();
                            };
                            let (distance, pct) =
                                distances.get(&asset.path).copied().unwrap_or((0, 100));
                            similar_hit_row(
                                cx,
                                asset,
                                distance,
                                pct,
                                selection_for_list.contains(&asset.path),
                                this_for_list.clone(),
                                thumb_cache.clone(),
                                focus.clone(),
                                colors,
                            )
                        })
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&self.grid_scroll)
            .flex_1()
            .w_full()
            .min_h(px(0.))
            .into_any_element()
        };

        let banner: Option<AnyElement> =
            error
                .filter(|_| !visible_indices.is_empty())
                .map(|message| {
                    div()
                        .px(px(16.))
                        .py(px(6.))
                        .text_xs()
                        .text_color(colors.danger)
                        .child(message)
                        .into_any_element()
                });

        div()
            .w(width)
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(colors.bg)
            .child(header)
            .children(banner)
            .child(
                div()
                    .id("similar-results")
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .px(px(8.))
                    .py(px(6.))
                    .flex()
                    .flex_col()
                    .key_context(GRID_CONTEXT)
                    .track_focus(&self.grid_focus)
                    .on_mouse_down(MouseButton::Left, {
                        let focus = self.grid_focus.clone();
                        move |_, window, cx| window.focus(&focus, cx)
                    })
                    .child(body),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod similar_ui_tests {
    use super::{preview_sample_note, similar_filter_matches};

    fn hit(tag: &str) -> kanalib_core::model::TagHit {
        kanalib_core::model::TagHit {
            tag: tag.to_string(),
            category: "general".into(),
            score: 0.9,
            vocab_index: None,
        }
    }

    fn asset(path: &str) -> kanalib_core::model::Asset {
        kanalib_core::model::Asset {
            path: std::path::PathBuf::from(path),
            library_root: std::path::PathBuf::from(r"C:\pics"),
            thumb: None,
            tile_thumb: None,
            width: 1,
            height: 1,
            group: String::new(),
            rating: String::new(),
            favorite: false,
            star: 0,
            tags: vec![hit("long_hair")],
            manual_tags: Vec::new(),
            tagged: false,
            tag_attempts: 0,
            error: None,
            damage: None,
            sha1: None,
            blake3: None,
            phash: None,
            bytes: 1,
            mtime: 0,
            captured_at: None,
            metadata_json: String::new(),
        }
    }

    #[test]
    fn preview_sample_note_marks_sampled_and_exact() {
        let sampled = kanalib_core::model::SimilarThresholdPreview {
            threshold: 10,
            group_count: 3,
            member_count: 9,
            chain_risk: false,
            chain_groups: 0,
            sampled_assets: 4096,
            total_assets: 100_000,
            sampled: true,
        };
        assert!(preview_sample_note(&sampled).contains("抽样 4096 / 100000"));
        assert!(preview_sample_note(&sampled).contains("4.1%"));
        let exact = kanalib_core::model::SimilarThresholdPreview {
            sampled_assets: 12,
            total_assets: 12,
            sampled: false,
            ..sampled
        };
        assert_eq!(preview_sample_note(&exact), "全量 12 张");
    }

    #[test]
    fn similar_filter_matches_title_path_and_tags() {
        let asset = asset(r"C:\pics\Hatsune_Miku.png");
        assert!(similar_filter_matches(&asset, "miku"));
        assert!(similar_filter_matches(&asset, "long_hair"));
        assert!(similar_filter_matches(&asset, r"c:\pics"));
        assert!(!similar_filter_matches(&asset, "kagamine"));
    }
}

#[cfg(test)]
mod dedup_sort_tests {
    use super::*;

    fn asset(path: &str, bytes: u64) -> Asset {
        Asset {
            path: PathBuf::from(path),
            library_root: PathBuf::from(r"C:\pics"),
            thumb: None,
            tile_thumb: None,
            width: 1,
            height: 1,
            group: String::new(),
            rating: String::new(),
            favorite: false,
            star: 0,
            tags: Vec::new(),
            manual_tags: Vec::new(),
            tagged: false,
            tag_attempts: 0,
            error: None,
            damage: None,
            sha1: None,
            blake3: None,
            phash: None,
            bytes,
            mtime: 0,
            captured_at: None,
            metadata_json: String::new(),
        }
    }

    fn group(
        key: &str,
        keeper: &str,
        copies: &[(&str, u64)],
        state: ReviewState,
    ) -> DuplicateGroup {
        let keeper_asset = asset(keeper, 100);
        let mut assets = vec![keeper_asset];
        assets.extend(copies.iter().map(|(path, bytes)| asset(path, *bytes)));
        let total_bytes = assets.iter().map(|asset| asset.bytes).sum();
        DuplicateGroup {
            key: key.to_string(),
            kind: GroupKind::Exact,
            assets,
            state,
            keeper: Some(PathBuf::from(keeper)),
            suggested: PathBuf::from(keeper),
            suggested_reason: "测试".into(),
            member_count: copies.len() + 1,
            signature: key.to_string(),
            total_bytes,
        }
    }

    #[test]
    fn dedup_query_matches_name_path_and_size_filters() {
        let one = group(
            "a",
            r"C:\pics\Alpha.png",
            &[(r"C:\pics\Alpha-copy.png", 40)],
            ReviewState::Pending,
        );
        let two = group(
            "b",
            r"C:\pics\Beta.png",
            &[(r"C:\pics\Beta-copy.png", 400)],
            ReviewState::Pending,
        );
        assert!(dedup_group_matches_query(&one, ""));
        assert!(dedup_group_matches_query(&one, "alpha"));
        assert!(dedup_group_matches_query(&one, "path:copy"));
        assert!(!dedup_group_matches_query(&one, "beta"));
        assert!(dedup_group_matches_query(&two, "size:>300b"));
        assert!(!dedup_group_matches_query(&one, "size:>300b"));
        assert!(dedup_group_matches_query(&one, "reclaim:<50b"));
        assert!(dedup_group_matches_query(&one, "pics"));
        assert!(!dedup_group_matches_query(&one, "no-such-group"));
    }

    #[test]
    fn dedup_sorting_covers_path_name_size_and_reclaim() {
        let groups = vec![
            group(
                "b",
                r"C:\pics\zulu.png",
                &[(r"C:\pics\zulu-copy.png", 50)],
                ReviewState::Pending,
            ),
            group(
                "a",
                r"C:\pics\alpha.png",
                &[(r"C:\pics\alpha-copy.png", 500)],
                ReviewState::Pending,
            ),
            group(
                "c",
                r"C:\pics\mike.png",
                &[(r"C:\pics\mike-copy.png", 5)],
                ReviewState::Pending,
            ),
        ];
        let by_path = dedup_visible_indices(&groups, DedupFilter::All, "", DedupSort::Path, false);
        assert_eq!(by_path, vec![1, 2, 0]);
        let by_name_desc =
            dedup_visible_indices(&groups, DedupFilter::All, "", DedupSort::Name, true);
        assert_eq!(by_name_desc, vec![0, 2, 1]);
        let by_size = dedup_visible_indices(&groups, DedupFilter::All, "", DedupSort::Size, true);
        assert_eq!(by_size, vec![1, 0, 2]);
        let by_reclaim_asc =
            dedup_visible_indices(&groups, DedupFilter::All, "", DedupSort::Reclaimable, false);
        assert_eq!(by_reclaim_asc, vec![2, 0, 1]);
        let filtered = dedup_visible_indices(
            &groups,
            DedupFilter::All,
            "alpha",
            DedupSort::Default,
            false,
        );
        assert_eq!(filtered, vec![1]);
    }

    #[test]
    fn planned_bytes_and_audit_round_trip() {
        let groups = vec![
            group(
                "a",
                r"C:\pics\a.png",
                &[(r"C:\pics\a-copy.png", 40)],
                ReviewState::Pending,
            ),
            group(
                "b",
                r"C:\pics\b.png",
                &[(r"C:\pics\b-copy.png", 60)],
                ReviewState::Pending,
            ),
            group(
                "c",
                r"C:\pics\c.png",
                &[(r"C:\pics\c-copy.png", 90)],
                ReviewState::Done,
            ),
        ];
        assert_eq!(planned_reclaim_bytes(&groups, Some("")), 0);
        assert_eq!(planned_reclaim_bytes(&groups, Some("a")), 40);
        assert_eq!(planned_reclaim_bytes(&groups, None), 100);
        let audit = TrashAudit {
            at: 1_761_000_000,
            groups: 2,
            assets: 3,
            failed: 1,
            protected_skipped: 1,
            bytes: 1024 * 1024,
            scope: "batch".into(),
        };
        let parsed = TrashAudit::from_json(&audit.to_json()).expect("round trip");
        assert_eq!(parsed, audit);
        assert!(audit.describe().contains("批量 2 组"));
        assert!(audit.describe().contains("1 张失败"));
        assert!(audit.describe().contains("跳过 1 个受保护组"));
        assert_eq!(parse_human_bytes("10mb"), Some(10 * 1024 * 1024));
        assert_eq!(parse_human_bytes("1.5kb"), Some(1536));
        assert_eq!(parse_human_bytes("bad"), None);
    }
}
