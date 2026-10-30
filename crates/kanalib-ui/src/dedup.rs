use crate::*;
use crate::app::{KanalibApp, push_toast};
use crate::format::*;
use crate::theme::*;
use crate::views::GRID_CONTEXT;
use kanalib_core::model::{SimilarHit, SimilarThresholdPreview};

pub(crate) const DEDUP_RAIL_WIDTH: f32 = 272.;
pub(crate) const DEDUP_GROUP_ROW_HEIGHT: f32 = 78.;
pub(crate) const DEDUP_CARD_WIDTH: f32 = 214.;
pub(crate) const DEDUP_MAX_VISIBLE: usize = 4;

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

pub(crate) fn dedup_filter_chip(
    filter: DedupFilter,
    count: usize,
    active: bool,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> AnyElement {
    div()
        .id(SharedString::from(format!(
            "dedup-filter-{}",
            filter.label()
        )))
        .px(px(9.))
        .py(px(4.))
        .rounded(px(5.))
        .cursor_pointer()
        .bg(if active {
            colors.selection
        } else {
            colors.sunken
        })
        .text_xs()
        .text_color(if active { colors.fg } else { colors.muted })
        .hover(move |style| style.bg(colors.hover))
        .on_click(move |_, _, cx| {
            let _ = this.update(cx, |app, cx| {
                app.dedup_filter = filter;
                app.dedup_selected = None;
                app.dedup_show_all = false;
                app.dedup_scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            });
        })
        .child(format!("{} {}", filter.label(), count))
        .into_any_element()
}

pub(crate) fn dedup_tab(
    label: String,
    active: bool,
    mode: GroupKind,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("dedup-tab-{label}")))
        .px(px(9.))
        .py(px(4.))
        .rounded(px(5.))
        .cursor_pointer()
        .bg(if active {
            colors.surface
        } else {
            colors.sunken
        })
        .text_xs()
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(if active { colors.fg } else { colors.subtle })
        .hover(move |style| style.bg(colors.hover))
        .on_click(move |_, _, cx| {
            let _ = this.update(cx, |app, cx| app.open_dedup_mode(mode, cx));
        })
        .child(label)
        .into_any_element()
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

pub(crate) fn group_state_label(state: ReviewState) -> &'static str {
    match state {
        ReviewState::Pending => "未审",
        ReviewState::Ignored => "已忽略",
        ReviewState::Related => "相关变体",
        ReviewState::Done => "已保留",
    }
}

pub(crate) fn dedup_group_row(
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
            "完全相同".to_string(),
            colors.success,
            colors.success_subtle,
        ),
        GroupKind::Similar => (
            format!("相似 {}%", group.similarity_pct().unwrap_or(0)),
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
                                    format!("{count} 张"),
                                    colors.muted,
                                    colors.sunken,
                                ))
                                .child(small_badge(kind_label, kind_fg, kind_bg))
                                .children(has_protected.then(|| {
                                    small_badge("保护".into(), colors.accent, colors.selection)
                                }))
                                .children(has_remote.then(|| {
                                    small_badge("远端只读".into(), colors.accent, colors.selection)
                                }))
                                .child(small_badge(
                                    group_state_label(group.state).into(),
                                    state_fg,
                                    state_bg,
                                )),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(colors.subtle)
                                .child(format!(
                                    "可释放 {reclaimable} · {}",
                                    group.suggested_reason
                                )),
                        ),
                ),
        )
        .into_any_element()
}

pub(crate) fn dedup_asset_card(
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
            .child("无缩略图")
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
        "尺寸未知".to_string()
    } else {
        format!("{}×{}", asset.width, asset.height)
    };
    let rating = if asset.rating.is_empty() {
        "未评级".to_string()
    } else {
        format!("评级 {}", asset.rating.to_uppercase())
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
            "保留".into(),
            colors.success,
            colors.success_subtle,
        ));
    }
    if is_suggested && !is_keeper {
        badge = badge.child(small_badge(
            "推荐".into(),
            colors.warning,
            colors.warning_subtle,
        ));
    }
    if is_protected_path {
        badge = badge.child(small_badge("保护".into(), colors.accent, colors.selection));
    }
    if is_remote {
        badge = badge.child(small_badge("远端只读".into(), colors.accent, colors.selection));
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
            .child(format!("相似 {pct}%"))
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
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(format!(
                            "修改 {} · {} 标签 · {rating}",
                            format_date(asset.mtime),
                            asset.all_tags().count()
                        )),
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
                            "当前保留项".to_string()
                        } else if is_suggested {
                            format!("推荐：{}", group.suggested_reason)
                        } else if editable {
                            "点击设为保留".to_string()
                        } else if pending {
                            "扫描中，暂不可操作".to_string()
                        } else {
                            "此组已保留".to_string()
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
    label: &'static str,
    values: Vec<String>,
    best: Option<usize>,
}

pub(crate) fn best_max<T: Ord + Copy>(assets: &[&Asset], value: impl Fn(&Asset) -> T) -> Option<usize> {
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

pub(crate) fn best_min<T: Ord + Copy>(assets: &[&Asset], value: impl Fn(&Asset) -> T) -> Option<usize> {
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
    label: &'static str,
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

pub(crate) fn dedup_diff(group: &DuplicateGroup, assets: &[&Asset], colors: UiColors) -> AnyElement {
    if assets.is_empty() {
        return div().into_any_element();
    }
    let mut rows = Vec::new();
    push_diff_row(
        &mut rows,
        "分辨率",
        assets
            .iter()
            .map(|asset| {
                if asset.width == 0 || asset.height == 0 {
                    "未知".to_string()
                } else {
                    format!("{}×{}", asset.width, asset.height)
                }
            })
            .collect(),
        best_max(assets, |asset| asset.width as u64 * asset.height as u64),
    );
    push_diff_row(
        &mut rows,
        "文件大小",
        assets
            .iter()
            .map(|asset| format_bytes(asset.bytes))
            .collect(),
        best_max(assets, |asset| asset.bytes),
    );
    push_diff_row(
        &mut rows,
        "格式",
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
        "修改日期",
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
        "标签",
        assets
            .iter()
            .map(|asset| {
                let top = asset
                    .top_tags(2)
                    .into_iter()
                    .map(|hit| hit.tag.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ");
                if top.is_empty() {
                    format!("{} 个", asset.all_tags().count())
                } else {
                    format!("{} 个 · {top}", asset.all_tags().count())
                }
            })
            .collect(),
        best_max(assets, |asset| asset.all_tags().count()),
    );
    push_diff_row(
        &mut rows,
        "评级",
        assets
            .iter()
            .map(|asset| {
                if asset.rating.is_empty() {
                    "未评级".to_string()
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
            "相似度",
            assets
                .iter()
                .map(|asset| {
                    group
                        .distance_to_keeper(asset)
                        .map(|distance| {
                            format!(
                                "{}%（距离 {distance}）",
                                similarity_pct_from_distance(distance)
                            )
                        })
                        .unwrap_or_else(|| "未知".to_string())
                })
                .collect(),
            best_min(assets, |asset| {
                group.distance_to_keeper(asset).unwrap_or(u32::MAX)
            }),
        );
    }
    push_diff_row(
        &mut rows,
        "路径",
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
                .child("字段"),
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
        .child(format!(
            "建议保留：{}",
            group
                .suggested_asset()
                .map(Asset::title)
                .unwrap_or_default()
        ))
        .child(
            div()
                .text_color(colors.warning)
                .child(group.suggested_reason.clone()),
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
                        .child("差异"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child("只显示不同字段"),
                ),
        )
        .child(recommendation)
        .child(table)
        .child(div().text_xs().text_color(colors.subtle).child(
            if group.kind == GroupKind::Similar {
                "相似度按 pHash 汉明距离换算；组内可能链式合并，请逐组确认。"
            } else {
                "字节相同：重点看路径、日期和标签；分辨率/大小通常相同。"
            },
        ))
        .into_any_element()
}

impl KanalibApp {
    pub(crate) fn open_dedup_mode(&mut self, mode: GroupKind, cx: &mut Context<Self>) {
        self.screen = Screen::Duplicates;
        self.dedup_mode = mode;
        self.dedup_filter = DedupFilter::Pending;
        self.dedup_selected = None;
        self.dedup_confirm = None;
        self.dedup_show_all = false;
        self.dedup_notice = None;
        self.dedup_threshold_draft = None;
        self.dedup_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    pub(crate) fn advance_dedup(&mut self, groups: &[DuplicateGroup], forward: bool) {
        let visible = groups
            .iter()
            .filter(|group| self.dedup_filter.matches(group.state))
            .map(|group| group.key.clone())
            .collect::<Vec<_>>();
        if visible.is_empty() {
            self.dedup_selected = None;
            return;
        }
        let index = self
            .dedup_selected
            .as_ref()
            .and_then(|selected| visible.iter().position(|key| key == selected));
        let next = match (index, forward) {
            (Some(index), true) => (index + 1) % visible.len(),
            (Some(index), false) => (index + visible.len() - 1) % visible.len(),
            (None, _) => 0,
        };
        self.dedup_selected = Some(visible[next].clone());
        self.dedup_show_all = false;
    }

    pub(crate) fn render_dedup(
        &mut self,
        this: &Entity<Self>,
        exact_groups: Arc<Vec<DuplicateGroup>>,
        exact_summary: DedupSummary,
        similar_groups: Arc<Vec<DuplicateGroup>>,
        similar_summary: DedupSummary,
        similar_scanned: bool,
        similar_scanning: bool,
        similar_threshold: u8,
        protected_paths: &[PathBuf],
        status: &str,
        error: Option<&str>,
        library_scanning: bool,
        width: Pixels,
        colors: UiColors,
    ) -> impl IntoElement {
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

        let visible_indices = groups
            .iter()
            .enumerate()
            .filter(|(_, group)| self.dedup_filter.matches(group.state))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
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
        let similar_tab_label = if similar_scanning {
            "相似图片 · 扫描中".to_string()
        } else if similar_scanned {
            format!("相似图片 {}", similar_summary.total)
        } else {
            "相似图片 · 未扫描".to_string()
        };
        let tabs = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(2.))
            .rounded(px(7.))
            .bg(colors.sunken)
            .child(dedup_tab(
                format!("完全重复 {}", exact_summary.total),
                !similar_mode,
                GroupKind::Exact,
                this.clone(),
                colors,
            ))
            .child(dedup_tab(
                similar_tab_label,
                similar_mode,
                GroupKind::Similar,
                this.clone(),
                colors,
            ));

        let more_menu = Menu::new().item(
            MenuItem::new("重新扫描图库")
                .disabled(busy)
                .on_click(move |_, cx| {
                    let _ = this_rescan.update(cx, |app, cx| {
                        app.request_rescan(RescanOrigin::Dedup, cx);
                    });
                }),
        );
        let header = div()
            .px(px(16.))
            .py(px(11.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
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
                            .gap(px(8.))
                            .min_w(px(0.))
                            .child(
                                div()
                                    .flex_none()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("重复项"),
                            )
                            .child(tabs),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(colors.muted)
                            .child(format!(
                                "未审 {} 组 · {} 张 · 可释放 {} · {}",
                                summary.pending,
                                summary.pending_assets,
                                format_bytes(summary.pending_bytes),
                                if library_scanning {
                                    "图库扫描中..."
                                } else if similar_mode && similar_scanning {
                                    "相似扫描中..."
                                } else {
                                    status
                                }
                            )),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .children(similar_mode.then(|| {
                        Button::new(
                            "dedup-similar-scan",
                            if similar_scanning {
                                "相似扫描中…"
                            } else if similar_scanned {
                                "重新扫描相似图片"
                            } else {
                                "扫描相似图片"
                            },
                        )
                        .primary()
                        .disabled(busy)
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
                        Button::new("dedup-preselect", "按规则预选")
                            .disabled(busy || summary.pending == 0)
                            .on_click(move |_, _, cx| {
                                let _ = this_preselect.update(cx, |app, cx| {
                                    let selected = app.state.preselect_dedup(preselect_mode);
                                    app.dedup_notice = Some(if selected == 0 {
                                        "没有待处理的组".to_string()
                                    } else {
                                        format!(
                                            "已按规则预选 {selected} 组；移入回收站前仍会汇总确认"
                                        )
                                    });
                                    cx.notify();
                                });
                            }),
                    )
                    .children((!similar_mode).then(|| {
                        Button::new("dedup-batch", "批量移入回收站")
                            .disabled(busy || summary.pending == 0 || has_remote_pending)
                            .on_click(move |_, _, cx| {
                                let _ = this_batch.update(cx, |app, cx| {
                                    app.dedup_confirm = Some(DedupConfirm::Batch);
                                    cx.notify();
                                });
                            })
                    }))
                    .child(
                        DropdownMenu::new("dedup-more", "更多", more_menu),
                    )
                    .child(
                        Button::new("dedup-library", "返回图库").on_click(move |_, _, cx| {
                            let _ = this_library.update(cx, |app, cx| {
                                app.screen = Screen::Library;
                                app.dedup_confirm = None;
                                cx.notify();
                            });
                        }),
                    ),
            );

        let threshold_bar: Option<AnyElement> = similar_mode.then(|| {
            div()
                .px(px(16.))
                .py(px(6.))
                .border_b_1()
                .border_color(colors.border)
                .child(self.render_threshold_editor(
                    this,
                    similar_threshold,
                    self.dedup_threshold_draft,
                    false,
                    colors,
                ))
                .into_any_element()
        });

        let filter_bar = div()
            .px(px(16.))
            .py(px(7.))
            .flex()
            .items_center()
            .gap(px(5.))
            .border_b_1()
            .border_color(colors.border)
            .child(dedup_filter_chip(
                DedupFilter::Pending,
                summary.pending,
                self.dedup_filter == DedupFilter::Pending,
                this.clone(),
                colors,
            ))
            .child(dedup_filter_chip(
                DedupFilter::Ignored,
                summary.ignored,
                self.dedup_filter == DedupFilter::Ignored,
                this.clone(),
                colors,
            ))
            .children(similar_mode.then(|| {
                dedup_filter_chip(
                    DedupFilter::Related,
                    summary.related,
                    self.dedup_filter == DedupFilter::Related,
                    this.clone(),
                    colors,
                )
            }))
            .child(dedup_filter_chip(
                DedupFilter::Done,
                summary.done,
                self.dedup_filter == DedupFilter::Done,
                this.clone(),
                colors,
            ))
            .child(dedup_filter_chip(
                DedupFilter::All,
                summary.total,
                self.dedup_filter == DedupFilter::All,
                this.clone(),
                colors,
            ))
            .child(div().flex_1())
            .child(div().text_xs().text_color(colors.subtle).child(if busy {
                "扫描中，完成后更新分组"
            } else if similar_mode {
                "相似组只逐组处理，不提供批量删除"
            } else {
                "先预选，再逐组确认"
            }));

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
                    .child("组"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(format!("{} 组", visible_indices.len())),
            );

        let rail_list: AnyElement = if visible_indices.is_empty() {
            div()
                .flex_1()
                .p(px(16.))
                .text_xs()
                .text_color(colors.subtle)
                .child(if similar_mode && !similar_scanned && !similar_scanning {
                    "尚未扫描相似图片。"
                } else if similar_scanning {
                    "正在计算相似图片..."
                } else if library_scanning {
                    "图库扫描完成后会显示重复组。"
                } else {
                    "此筛选下没有组。"
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
                move |range, _, _| {
                    range
                        .map(|row| {
                            let index = indices_for_list[row];
                            let group = &groups_for_list[index];
                            dedup_group_row(
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
                this,
                &group,
                groups.clone(),
                busy,
                detail_width,
                protected_paths,
                colors,
            )
        } else {
            let message = if similar_mode && !similar_scanned && !similar_scanning {
                "尚未扫描相似图片。选一个阈值开始扫描。"
            } else if similar_scanning {
                "正在计算相似图片..."
            } else if library_scanning {
                "正在扫描图库..."
            } else if similar_mode {
                "没有符合筛选的相似组"
            } else {
                "没有符合筛选的重复组"
            };
            div()
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(colors.muted)
                .child(message)
                .into_any_element()
        };

        let notice = self.dedup_notice.clone();
        let remote_note = has_remote_pending.then(|| {
            (
                "含远端只读资产的组不能移入回收站；批量回收站已禁用，请逐组处理本地组。"
                    .to_string(),
                colors.warning,
            )
        });
        let banner: Option<AnyElement> = error
            .map(|message| (message.to_string(), colors.danger))
            .or_else(|| notice.map(|message| (message, colors.muted)))
            .or(remote_note)
            .map(|(message, color)| {
                div()
                    .px(px(16.))
                    .py(px(7.))
                    .border_b_1()
                    .border_color(colors.border)
                    .bg(colors.sunken)
                    .text_xs()
                    .text_color(color)
                    .child(message)
                    .into_any_element()
            });

        let overlay = self.dedup_confirm.clone().and_then(|confirm| {
            self.render_dedup_confirm(this, &confirm, &groups, summary, colors)
        });

        div()
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
            GroupKind::Exact => "字节完全相同",
            GroupKind::Similar => "相似候选组",
        };
        let similarity = group
            .similarity_pct()
            .map(|pct| format!(" · 相似度 ≥{pct}%"))
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
                "打开保留项（远端只读）"
            } else if asset.is_video() {
                "用系统播放器打开"
            } else {
                "打开保留项"
            };
            let open_toaster = self.toaster.clone();
            Button::new("dedup-open", label)
                .disabled(remote)
                .on_click(move |_, _, cx| {
                    if let Err(error) = open_asset_with_system(&path) {
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
                    "对比前 4 张"
                } else {
                    "对比此组"
                },
            )
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |app, cx| app.open_compare_group(&group, cx));
            })
        });

        let header = div()
            .flex_none()
            .px(px(16.))
            .py(px(11.))
            .border_b_1()
            .border_color(colors.border)
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!(
                                "{} 张 · {kind_label} · {}{similarity}{}",
                                group.assets.len(),
                                group_state_label(group.state),
                                if has_protected {
                                    " · 含保护路径"
                                } else {
                                    ""
                                }
                            )),
                    )
                    .child(div().text_xs().text_color(colors.muted).child(format!(
                        "{} · 可释放 {} · 当前保留：{}",
                        group.suggested_reason,
                        format_bytes(reclaimable),
                        keeper.map(Asset::title).unwrap_or_else(|| "未选择".into())
                    ))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .child(
                        Button::new("dedup-prev", "上一组").on_click(move |_, _, cx| {
                            let _ = this_prev.update(cx, |app, cx| {
                                app.advance_dedup(&groups_prev, false);
                                cx.notify();
                            });
                        }),
                    )
                    .child(
                        Button::new("dedup-next", "下一组").on_click(move |_, _, cx| {
                            let _ = this_next.update(cx, |app, cx| {
                                app.advance_dedup(&groups_next, true);
                                cx.notify();
                            });
                        }),
                    )
                    .children(open)
                    .children(compare_button)
                    .children((group.assets.len() > DEDUP_MAX_VISIBLE).then(|| {
                        Button::new(
                            "dedup-show-all",
                            if self.dedup_show_all {
                                "收起".to_string()
                            } else {
                                format!("全部 {} 张", group.assets.len())
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
                        .child("点击任意卡片设为保留项")
                        .child("保留项不会被移入回收站")
                        .child("绿色 = 保留 · 黄色 = 推荐"),
                )
                .into_any_element()
        };

        let diff = dedup_diff(group, &shown_assets, colors);
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
                    Button::new("dedup-ignore", "不是重复")
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
                                            app.dedup_notice = Some("已忽略此组".into());
                                            app.advance_dedup(&groups, true);
                                        }
                                        Err(error) => {
                                            app.dedup_notice = Some(format!("操作失败: {error:#}"));
                                        }
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                )
                .children((group_kind == GroupKind::Similar).then(|| {
                    Button::new("dedup-related", "相关变体")
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
                                            app.dedup_notice = Some("已标记为相关变体".into());
                                            app.advance_dedup(&groups, true);
                                        }
                                        Err(error) => {
                                            app.dedup_notice = Some(format!("操作失败: {error:#}"));
                                        }
                                    }
                                    cx.notify();
                                });
                            }
                        })
                }))
                .child(
                    Button::new("dedup-later", "稍后").on_click(move |_, _, cx| {
                        let _ = this_later.update(cx, |app, cx| {
                            app.advance_dedup(&groups_later, true);
                            cx.notify();
                        });
                    }),
                )
                .child(
                    Button::new("dedup-keep-all", "全部保留")
                        .disabled(scanning)
                        .on_click({
                            let key = key.clone();
                            move |_, _, cx| {
                                let _ = this_keep.update(cx, |app, cx| {
                                    let groups = groups_keep.clone();
                                    match app.state.mark_dedup(&key, ReviewState::Done, group_kind)
                                    {
                                        Ok(()) => {
                                            app.dedup_notice = Some("已保留整组".into());
                                            app.advance_dedup(&groups, true);
                                        }
                                        Err(error) => {
                                            app.dedup_notice = Some(format!("操作失败: {error:#}"));
                                        }
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(
                    Button::new("dedup-trash", format!("移入回收站 ({duplicate_count})"))
                        .variant(ButtonVariant::Danger)
                        .disabled(
                            scanning || keeper.is_none() || protected_duplicate || group_remote,
                        )
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
                    Button::new("dedup-reopen", "重新打开此组")
                        .disabled(scanning)
                        .on_click(move |_, _, cx| {
                            let _ = this_reopen.update(cx, |app, cx| {
                                match app.state.mark_dedup(&key, ReviewState::Pending, group_kind) {
                                    Ok(()) => {
                                        app.dedup_notice = Some("已重新打开此组".into());
                                        app.dedup_selected = Some(key.clone());
                                    }
                                    Err(error) => {
                                        app.dedup_notice = Some(format!("操作失败: {error:#}"));
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
                            .child(format!(
                                "保留 1 张 · {} {duplicate_count} 张 · 预计释放 {}",
                                if group_kind == GroupKind::Similar {
                                    "待确认"
                                } else {
                                    "待处理"
                                },
                                format_bytes(reclaimable)
                            )),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(if scanning {
                                "扫描中，完成后可操作"
                            } else if group_remote {
                                "包含远端只读资产，移入回收站已禁用；可标记忽略或全部保留"
                            } else if protected_duplicate {
                                "包含受保护路径，移入回收站已禁用；请把受保护文件设为保留项或调整保护路径"
                            } else {
                                "只移入系统回收站，不永久删除；可从回收站恢复"
                            }),
                    ),
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
                    "移入回收站？".to_string(),
                    format!(
                        "将把 {} 张{}副本移入系统回收站；保留 {}，不修改原图。{}",
                        duplicates.len(),
                        if group.kind == GroupKind::Similar {
                            "相似"
                        } else {
                            ""
                        },
                        keeper.title(),
                        if group.kind == GroupKind::Similar {
                            " 相似组只是候选，请确认它们确实是同一张图的不同版本。"
                        } else {
                            ""
                        }
                    ),
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
                            format!("{} 张副本", group.assets.len().saturating_sub(1)),
                        )
                    })
                    .collect::<Vec<_>>();
                (
                    "批量移入回收站？".to_string(),
                    format!(
                        "将处理 {} 个未审组，把 {} 张副本移入系统回收站；每组保留当前保留项。含保护路径的组会跳过。预计释放 {}。",
                        summary.pending,
                        asset_count,
                        format_bytes(summary.pending_bytes)
                    ),
                    rows,
                    None,
                    remote,
                )
            }
        };
        let detail = if remote {
            format!("{detail} 其中含远端只读资产，不能移入回收站。")
        } else {
            detail
        };

        let this_cancel = this.clone();
        let this_confirm = this.clone();
        let overlay = div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .occlude()
            .bg(colors.backdrop)
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(480.))
                    .p(px(18.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.surface)
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(div().text_sm().font_weight(FontWeight::BOLD).child(title))
                    .child(div().text_xs().text_color(colors.muted).child(detail))
                    .child(
                        div()
                            .rounded(px(7.))
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
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(6.))
                            .child(
                                Button::new("dedup-cancel", "取消").on_click(move |_, _, cx| {
                                    let _ = this_cancel.update(cx, |app, cx| {
                                        app.dedup_confirm = None;
                                        cx.notify();
                                    });
                                }),
                            )
                            .child(
                                Button::new("dedup-confirm", "确认移入回收站")
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
                                                    app.dedup_notice = Some(format!(
                                                        "已处理 {} 组 · 移入回收站 {} 张{}{}",
                                                        done.groups,
                                                        done.assets,
                                                        if done.failed > 0 {
                                                            format!("；{} 张未移动", done.failed)
                                                        } else {
                                                            String::new()
                                                        },
                                                        if done.protected_skipped > 0 {
                                                            format!(
                                                                "；跳过 {} 个受保护组",
                                                                done.protected_skipped
                                                            )
                                                        } else {
                                                            String::new()
                                                        }
                                                    ));
                                                }
                                                Err(error) => {
                                                    app.dedup_notice =
                                                        Some(format!("移入回收站失败: {error:#}"));
                                                }
                                            }
                                            app.dedup_confirm = None;
                                            app.dedup_selected = None;
                                            cx.notify();
                                        });
                                    }),
                            ),
                    ),
            )
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
    let menu = crate::views::asset_context_menu(asset, &this);
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
        .bg(if selected { colors.selection } else { colors.bg })
        .hover(move |style| style.bg(colors.hover))
        .on_click(move |event, window, cx| {
            window.focus(&focus_click, cx);
            let (modifiers, click_count) = match event {
                ClickEvent::Mouse(event) => (event.down.modifiers, event.down.click_count),
                ClickEvent::Keyboard(_) | ClickEvent::Touch(_) => (Modifiers::default(), 1),
            };
            let path = path_click.clone();
            let _ =
                this_click.update(cx, |app, cx| app.handle_tile_click(path, modifiers, click_count, cx));
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
            format!("距离 {distance}"),
            colors.warning,
            colors.warning_subtle,
        ))
        .child(small_badge(
            format!("相似 {similarity_pct}%"),
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
            None => crate::app::push_toast(
                &self.toaster,
                cx,
                "先选择一张图片再查找相似",
                Severity::Warning,
            ),
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
                    view.error = Some(format!("查找相似失败：{error:#}"));
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
        crate::app::push_toast(
            &self.toaster,
            cx,
            format!("相似阈值默认值已设为 ≤{saved}；已有审核状态保留"),
            Severity::Success,
        );
        cx.notify();
    }

    /// 去重页：应用阈值并重新扫描相似组；`start_similar_scan` 会复用既有评审状态。
    pub(crate) fn apply_threshold_and_scan(&mut self, threshold: u8, cx: &mut Context<Self>) {
        let saved = self.state.set_similar_threshold(threshold);
        self.dedup_threshold_draft = None;
        self.state.start_similar_scan();
        self.dedup_selected = None;
        self.dedup_confirm = None;
        self.dedup_notice = Some(format!(
            "已用 ≤{saved} 重新扫描相似图片；已审核状态保留，扫描期间仍可查看旧分组"
        ));
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
        this: &Entity<Self>,
        default: u8,
        draft: Option<u8>,
        settings: bool,
        colors: UiColors,
    ) -> AnyElement {
        let effective = draft.unwrap_or(default);
        let prefix = if settings { "settings" } else { "dedup" };
        let mut presets = div().flex().items_center().gap(px(5.));
        for (value, label) in [(6u8, "严格 ≤6"), (10u8, "标准 ≤10"), (14u8, "宽松 ≤14")] {
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
                .child(format!("临时 ≤{value}（未保存；默认 ≤{default}）"))
                .into_any_element(),
            Some(value) => div()
                .text_xs()
                .text_color(colors.muted)
                .child(format!("临时 ≤{value}（与默认相同）"))
                .into_any_element(),
            None => div()
                .text_xs()
                .text_color(colors.muted)
                .child(format!("使用默认 ≤{default}"))
                .into_any_element(),
        };

        let preview_note: AnyElement = if self.similar_preview_running
            && self.similar_preview_threshold == Some(effective)
        {
            div()
                .text_xs()
                .text_color(colors.accent)
                .child("预览中…（大库可能 1.8–2.6 秒）")
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
                .child(div().text_xs().text_color(colors.fg).child(format!(
                    "预估 {} 组 · {} 张入组",
                    preview.group_count, preview.member_count
                )))
                .child(small_badge(
                    preview_sample_note(preview),
                    colors.muted,
                    colors.sunken,
                ));
            if preview.chain_risk {
                row = row
                    .child(small_badge(
                        format!("链式合并风险 · {} 组", preview.chain_groups),
                        colors.danger,
                        colors.sunken,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.warning)
                            .child("低阈值会把间接相似的图串成一组，建议逐组确认或提高阈值。"),
                    );
            }
            row.into_any_element()
        } else {
            div()
                .text_xs()
                .text_color(colors.subtle)
                .child(if self.similar_preview_running {
                    "正在预览其它阈值…"
                } else {
                    "尚无该阈值的预览；拖动滑杆或点「刷新预览」。"
                })
                .into_any_element()
        };

        let this_refresh = this.clone();
        let this_apply = this.clone();
        let this_scan = this.clone();
        let this_discard = this.clone();
        let refresh = Button::new(
            SharedString::from(format!("{prefix}-sim-preview-refresh")),
            "刷新预览",
        )
        .size(ControlSize::Sm)
        .on_click(move |_, _, cx| {
            let _ = this_refresh.update(cx, |app, cx| {
                app.schedule_threshold_preview(effective, cx);
            });
        });
        let apply = Button::new(
            SharedString::from(format!("{prefix}-sim-apply-default")),
            format!("设为默认 ≤{effective}"),
        )
        .size(ControlSize::Sm)
        .variant(ButtonVariant::Primary)
        .on_click(move |_, _, cx| {
            let _ = this_apply.update(cx, |app, cx| app.apply_threshold_default(effective, cx));
        });
        let scan = (!settings).then(|| {
            Button::new("dedup-sim-apply-scan", format!("应用并扫描 ≤{effective}"))
                .size(ControlSize::Sm)
                .variant(ButtonVariant::Primary)
                .on_click(move |_, _, cx| {
                    let _ = this_scan
                        .update(cx, |app, cx| app.apply_threshold_and_scan(effective, cx));
                })
        });
        let discard = draft.map(|_| {
            Button::new(
                SharedString::from(format!("{prefix}-sim-discard-draft")),
                "放弃临时值",
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
                            .child(format!(
                                "相似阈值 ≤{effective} · 约 {}% 相似",
                                similarity_pct_from_distance(effective as u32)
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
                    .child("改动阈值只影响预览与扫描，不清空已审核状态；所有操作都不删除磁盘文件。"),
            )
            .into_any_element()
    }
}

impl KanalibApp {
    // ---- F-004：临时结果视图 ----

    pub(crate) fn render_similar(
        &mut self,
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
        for (value, label) in [(6u8, "严格 ≤6"), (10u8, "标准 ≤10"), (14u8, "宽松 ≤14")] {
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
                        Button::new("similar-back", "返回图库")
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
                            .child("查找相似"),
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
                        format!("已比较 {scanned} 张 pHash"),
                        colors.muted,
                        colors.sunken,
                    ))
                    .child(small_badge(
                        format!("{total_hits} 个结果 · 距离升序"),
                        colors.accent,
                        colors.selection,
                    ))
                    .children(truncated.then(|| {
                        small_badge(
                            format!("仅显示前 {SIMILAR_RESULT_LIMIT} 条"),
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
                    .child(div().text_xs().text_color(colors.subtle).child(format!(
                        "阈值 ≤{threshold} · 相似度 ≥{}%",
                        similarity_pct_from_distance(threshold as u32)
                    )))
                    .child(slider)
                    .child(preset_row)
                    .child(
                        Button::new("similar-refresh", "重新查找")
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ = this_refresh
                                    .update(cx, |app, cx| app.refresh_similar_query(cx));
                            }),
                    )
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(colors.subtle).child(if scanning {
                        "图库扫描中，结果可能滞后".to_string()
                    } else {
                        "只读查询 · 不删除/移动文件 · 调整阈值不清空审核状态".to_string()
                    })),
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
                                    .least("筛选相似结果（名称/标签/路径）"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted)
                            .child(format!("{} / {total_hits} 条", visible_indices.len())),
                    )
                    .children((!selection.is_empty()).then(|| {
                        div()
                            .text_xs()
                            .text_color(colors.accent)
                            .child(format!("已选 {} 张", selection.len()))
                    }))
                    .child(div().flex_1())
                    .child(
                        Button::new("similar-select-all", "全选可见")
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ = this_all.update(cx, |app, cx| app.select_all_visible(cx));
                            }),
                    )
                    .child(
                        Button::new("similar-clear-selection", "清除选择")
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ = this_clear.update(cx, |app, cx| app.clear_selection(cx));
                            }),
                    )
                    .child(
                        Button::new("similar-quick-view", "快速查看")
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
                        .unwrap_or_else(|| "没有相似图片；可以放宽阈值或清除筛选。".to_string()),
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
            uniform_list("similar-hits", indices_for_list.len(), move |range, _, _| {
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
            })
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
