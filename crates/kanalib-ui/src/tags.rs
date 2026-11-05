use crate::app::{KanalibApp, push_toast};
use crate::components::*;
use crate::localization::{t, t_args};
use crate::settings::TAG_ROW_HEIGHT;
use crate::theme::*;
use crate::*;

/// 低置信度审核最多加载的候选条数。
pub(crate) const TAG_REVIEW_LIMIT: usize = 80;

/// F-025：低频标签阈值（含）；0 使用单独统计，绝不自动删除。
pub(crate) const TAG_LOW_FREQ_MAX: usize = 2;

/// 标签管理的筛选口径；全部只影响展示，不修改标签数据。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum TagManagerFilter {
    #[default]
    All,
    Unused,
    LowFrequency,
    AutoOnly,
}

impl TagManagerFilter {
    pub(crate) const ALL: [Self; 4] = [Self::All, Self::Unused, Self::LowFrequency, Self::AutoOnly];

    pub(crate) fn label(self, cx: &App) -> SharedString {
        t(
            cx,
            match self {
                Self::All => "tags.filter.all",
                Self::Unused => "tags.filter.unused",
                Self::LowFrequency => "tags.filter.low",
                Self::AutoOnly => "tags.filter.auto",
            },
        )
    }

    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Unused => "unused",
            Self::LowFrequency => "low",
            Self::AutoOnly => "auto",
        }
    }

    pub(crate) fn matches(self, tag: &TagSummary) -> bool {
        match self {
            Self::All => true,
            Self::Unused => tag.count == 0,
            Self::LowFrequency => tag.count > 0 && tag.count <= TAG_LOW_FREQ_MAX,
            // “仅自动”= 有使用但一次手工添加都没有；手工标签不算。
            Self::AutoOnly => tag.count > 0 && tag.manual_count == 0,
        }
    }
}

/// 标签管理排序；默认保持词表的用量降序。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum TagManagerSort {
    #[default]
    Default,
    Name,
    UsageAsc,
    UsageDesc,
    ManualDesc,
}

impl TagManagerSort {
    pub(crate) const ALL: [Self; 5] = [
        Self::Default,
        Self::Name,
        Self::UsageAsc,
        Self::UsageDesc,
        Self::ManualDesc,
    ];

    pub(crate) fn label(self, cx: &App) -> SharedString {
        t(
            cx,
            match self {
                Self::Default => "tags.sort.default",
                Self::Name => "tags.sort.name",
                Self::UsageAsc => "tags.sort.usage_asc",
                Self::UsageDesc => "tags.sort.usage_desc",
                Self::ManualDesc => "tags.sort.manual_desc",
            },
        )
    }
}

/// F-025：按筛选口径 + 名称搜索过滤，再按排序规则排列；不删除任何标签。
pub(crate) fn filter_tag_summaries(
    tags: &[TagSummary],
    query: &str,
    filter: TagManagerFilter,
) -> Vec<TagSummary> {
    let needle = query.trim().to_lowercase();
    tags.iter()
        .filter(|tag| {
            filter.matches(tag)
                && (needle.is_empty()
                    || tag.name.to_lowercase().contains(&needle)
                    || tag.category.to_lowercase().contains(&needle))
        })
        .cloned()
        .collect()
}

pub(crate) fn sort_tag_summaries(tags: &mut [TagSummary], sort: TagManagerSort) {
    match sort {
        TagManagerSort::Default => {}
        TagManagerSort::Name => tags.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name))
        }),
        TagManagerSort::UsageAsc => tags.sort_by(|a, b| {
            a.count
                .cmp(&b.count)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
        TagManagerSort::UsageDesc => tags.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
        TagManagerSort::ManualDesc => tags.sort_by(|a, b| {
            b.manual_count
                .cmp(&a.manual_count)
                .then_with(|| b.count.cmp(&a.count))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
    }
}

/// 0 使用 / 低频 / 仅自动的计数，只用于标签管理页标题。
pub(crate) fn tag_manager_counts(tags: &[TagSummary]) -> (usize, usize, usize) {
    let mut unused = 0;
    let mut low = 0;
    let mut auto_only = 0;
    for tag in tags {
        if TagManagerFilter::Unused.matches(tag) {
            unused += 1;
        }
        if TagManagerFilter::LowFrequency.matches(tag) {
            low += 1;
        }
        if TagManagerFilter::AutoOnly.matches(tag) {
            auto_only += 1;
        }
    }
    (unused, low, auto_only)
}

/// 待审候选按资产分组；core 已按路径、分数排序，这里保持同样的顺序。
pub(crate) fn group_reviews_by_asset(
    reviews: &[TagCandidateReview],
) -> Vec<(PathBuf, Vec<TagCandidateReview>)> {
    let mut groups: Vec<(PathBuf, Vec<TagCandidateReview>)> = Vec::new();
    for review in reviews {
        match groups.last_mut() {
            Some((path, candidates)) if *path == review.path => {
                candidates.push(review.clone());
            }
            _ => groups.push((review.path.clone(), vec![review.clone()])),
        }
    }
    groups
}

impl KanalibApp {
    /// 读取待审的低置信度候选：分数低于有效阈值、尚未处理过的候选。
    pub(crate) fn refresh_tag_reviews(&mut self, cx: &mut Context<Self>) {
        if self.tag_review_running {
            return;
        }
        self.tag_review_running = true;
        let state = self.state.clone();
        let task = cx
            .background_executor()
            .spawn(async move { state.pending_tag_reviews(TAG_REVIEW_LIMIT) });
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = task.await;
            let _ = weak.update(cx, |app, cx| {
                app.tag_review_running = false;
                match result {
                    Ok(reviews) => {
                        app.tag_reviews = reviews;
                        app.tag_review_notice = None;
                    }
                    Err(error) => {
                        let message = t_args(
                            cx,
                            "tags.review.load_failed",
                            &[("error", &format!("{error:#}"))],
                        )
                        .to_string();
                        log::warn!("pending tag reviews: {error:#}");
                        app.tag_review_notice = Some(message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// 接受或忽略单条候选。
    pub(crate) fn apply_tag_review(
        &mut self,
        path: PathBuf,
        tag: String,
        state: TagReviewState,
        cx: &mut Context<Self>,
    ) {
        self.apply_tag_review_async(path, Some(tag), state, cx);
    }

    /// 按资产批量处理该资产的全部待审候选。
    pub(crate) fn apply_asset_review(
        &mut self,
        path: PathBuf,
        state: TagReviewState,
        cx: &mut Context<Self>,
    ) {
        self.apply_tag_review_async(path, None, state, cx);
    }

    pub(crate) fn apply_tag_review_async(
        &mut self,
        path: PathBuf,
        tag: Option<String>,
        state: TagReviewState,
        cx: &mut Context<Self>,
    ) {
        let label = match (&tag, state) {
            (Some(tag), TagReviewState::Accepted) => {
                t_args(cx, "tags.review.accept", &[("tag", tag)])
            }
            (Some(tag), TagReviewState::Ignored) => {
                t_args(cx, "tags.review.ignore", &[("tag", tag)])
            }
            (Some(tag), TagReviewState::Pending) => {
                t_args(cx, "tags.review.restore", &[("tag", tag)])
            }
            (None, TagReviewState::Accepted) => t(cx, "tags.review.accept_asset"),
            (None, TagReviewState::Ignored) => t(cx, "tags.review.ignore_asset"),
            (None, TagReviewState::Pending) => t(cx, "tags.review.restore_asset"),
        }
        .to_string();
        let app_state = self.state.clone();
        let toaster = self.toaster.clone();
        let review_path = path.clone();
        let task = cx.background_executor().spawn(async move {
            match &tag {
                Some(tag) => app_state
                    .review_tag_candidate(&review_path, tag, state)
                    .map(|()| 1usize),
                None => app_state.review_asset_candidates(&review_path, state),
            }
        });
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = task.await;
            let _ = weak.update(cx, |app, cx| {
                match result {
                    Ok(changed) => {
                        let message = t_args(
                            cx,
                            "tags.review.done",
                            &[("label", &label), ("count", &changed.to_string())],
                        );
                        app.tag_review_notice = Some(message.to_string());
                        push_toast(&toaster, cx, message, Severity::Success);
                        app.refresh_tag_reviews(cx);
                    }
                    Err(error) => {
                        let message = t_args(
                            cx,
                            "tags.review.failed",
                            &[("label", &label), ("error", &format!("{error:#}"))],
                        );
                        app.tag_review_notice = Some(message.to_string());
                        push_toast(&toaster, cx, message, Severity::Danger);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn open_tag_manager(&mut self, cx: &mut Context<Self>) {
        // F-025：使用全词表（含 0 使用标签），否则「0 使用」筛选永远为空。
        match self.state.tag_vocabulary() {
            Ok(tags) => self.tag_list = tags,
            Err(error) => {
                log::warn!("list tags: {error:#}");
                self.tag_list.clear();
                let message = t_args(cx, "tags.load_failed", &[("error", &format!("{error:#}"))]);
                push_toast(&self.toaster, cx, message, Severity::Danger);
            }
        }
        self.screen = Screen::Tags;
        self.dedup_confirm = None;
        self.palette_open = false;
        self.tag_return_pending = false;
        self.tag_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// F-070：从标签页跳到图库，保留标签页的筛选/搜索/排序与滚动状态。
    pub(crate) fn open_tag_assets(&mut self, name: String, cx: &mut Context<Self>) {
        self.set_filter(Filter::Tag(name), cx);
        self.tag_return_pending = true;
        cx.notify();
    }

    /// 返回标签页时不重置滚动与筛选；只刷新计数。
    pub(crate) fn return_to_tag_manager(&mut self, cx: &mut Context<Self>) {
        if self.viewer_at.is_some() {
            self.close_viewer(cx);
        }
        self.focus_grid_pending = false;
        self.tag_return_pending = false;
        self.screen = Screen::Tags;
        self.dedup_confirm = None;
        self.palette_open = false;
        self.refresh_tag_manager(cx);
    }

    pub(crate) fn refresh_tag_manager(&mut self, cx: &mut Context<Self>) {
        match self.state.tag_vocabulary() {
            Ok(tags) => self.tag_list = tags,
            Err(error) => {
                let message = t_args(cx, "tags.load_failed", &[("error", &format!("{error:#}"))]);
                push_toast(&self.toaster, cx, message, Severity::Danger);
            }
        }
        cx.notify();
    }

    pub(crate) fn set_tag_blocked(&mut self, name: String, blocked: bool, cx: &mut Context<Self>) {
        log::info!("tag manager: set blocked name={name} blocked={blocked}");
        match self.state.set_tag_blocked(&name, blocked) {
            Ok(()) => {
                let message = if blocked {
                    t_args(cx, "tags.block.done", &[("name", &name)])
                } else {
                    t_args(cx, "tags.unblock.done", &[("name", &name)])
                };
                push_toast(&self.toaster, cx, message, Severity::Success);
            }
            Err(error) => {
                let message = t_args(cx, "tags.block.failed", &[("error", &format!("{error:#}"))]);
                push_toast(&self.toaster, cx, message, Severity::Danger);
            }
        }
        cx.notify();
    }

    /// 全词表补全：前缀命中优先，其次使用次数；输入法/中文直接按子串匹配。
    pub(crate) fn update_tag_suggestions(&mut self, query: &str) {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            self.tag_suggestions.clear();
            return;
        }
        let mut matches = self
            .tag_vocab
            .iter()
            .filter(|tag| tag.name.to_lowercase().contains(&needle))
            .map(|tag| (tag.name.clone(), tag.count))
            .collect::<Vec<_>>();
        matches.sort_by(|(a_name, a_count), (b_name, b_count)| {
            let a_prefix = a_name.to_lowercase().starts_with(&needle);
            let b_prefix = b_name.to_lowercase().starts_with(&needle);
            b_prefix
                .cmp(&a_prefix)
                .then_with(|| b_count.cmp(a_count))
                .then_with(|| a_name.cmp(b_name))
        });
        matches.truncate(50);
        self.tag_suggestions = matches;
    }

    pub(crate) fn open_tag_rename(&mut self, name: String, cx: &mut Context<Self>) {
        self.tag_rename = Some(name);
        self.tag_rename_epoch = self.tag_rename_epoch.wrapping_add(1);
        cx.notify();
    }

    pub(crate) fn open_tag_merge(&mut self, name: String, cx: &mut Context<Self>) {
        self.tag_merge = Some(name);
        self.tag_merge_epoch = self.tag_merge_epoch.wrapping_add(1);
        cx.notify();
    }

    pub(crate) fn apply_tag_rename(&mut self, new_name: &str, cx: &mut Context<Self>) {
        let Some(old) = self.tag_rename.clone() else {
            return;
        };
        match self.state.rename_tag(&old, new_name) {
            Ok(affected) => {
                self.tag_rename = None;
                let message = t_args(cx, "tags.rename.done", &[("count", &affected.to_string())]);
                push_toast(&self.toaster, cx, message, Severity::Success);
                self.refresh_tag_manager(cx);
            }
            Err(error) => {
                let message = t_args(
                    cx,
                    "tags.rename.failed",
                    &[("error", &format!("{error:#}"))],
                );
                push_toast(&self.toaster, cx, message, Severity::Danger);
            }
        }
        cx.notify();
    }

    pub(crate) fn apply_tag_merge(&mut self, target: &str, cx: &mut Context<Self>) {
        let Some(source) = self.tag_merge.clone() else {
            return;
        };
        match self.state.merge_tags(&source, target) {
            Ok(affected) => {
                self.tag_merge = None;
                let message = t_args(cx, "tags.merge.done", &[("count", &affected.to_string())]);
                push_toast(&self.toaster, cx, message, Severity::Success);
                self.refresh_tag_manager(cx);
            }
            Err(error) => {
                let message = t_args(cx, "tags.merge.failed", &[("error", &format!("{error:#}"))]);
                push_toast(&self.toaster, cx, message, Severity::Danger);
            }
        }
        cx.notify();
    }

    pub(crate) fn open_tag_dialog(&mut self, mode: TagDialogMode, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            let message = t(cx, "tags.select_images");
            push_toast(&self.toaster, cx, message, Severity::Warning);
            return;
        }
        self.palette_open = false;
        self.tag_dialog = Some(mode);
        self.tag_dialog_paths = paths;
        self.tag_dialog_epoch = self.tag_dialog_epoch.wrapping_add(1);
        cx.notify();
    }

    pub(crate) fn apply_tag_dialog(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(mode) = self.tag_dialog else {
            return;
        };
        let paths = self.tag_dialog_paths.clone();
        if paths.is_empty() {
            let message = t(cx, "tags.select_images");
            push_toast(&self.toaster, cx, message, Severity::Warning);
            return;
        }
        let result = match mode {
            TagDialogMode::Add => self.state.add_tags(&paths, name),
            TagDialogMode::Remove => self.state.remove_tags(&paths, name),
        };
        match result {
            Ok(0) => {
                let message = if mode == TagDialogMode::Add {
                    t(cx, "tags.exists_or_invalid")
                } else {
                    t(cx, "tags.not_tagged")
                };
                push_toast(&self.toaster, cx, message, Severity::Warning);
            }
            Ok(updated) => {
                self.tag_dialog = None;
                self.tag_dialog_paths.clear();
                let message = if mode == TagDialogMode::Add {
                    t_args(
                        cx,
                        "tags.added",
                        &[("count", &updated.to_string()), ("name", name)],
                    )
                } else {
                    t_args(
                        cx,
                        "tags.removed",
                        &[("count", &updated.to_string()), ("name", name)],
                    )
                };
                push_toast(&self.toaster, cx, message, Severity::Success);
            }
            Err(error) => {
                let message = t_args(
                    cx,
                    "tags.action_failed",
                    &[("error", &format!("{error:#}"))],
                );
                self.state.set_error(message.to_string());
                push_toast(&self.toaster, cx, message, Severity::Danger);
            }
        }
        cx.notify();
    }

    pub(crate) fn render_tags(
        &mut self,
        this: &Entity<Self>,
        cx: &mut Context<Self>,
        width: Pixels,
        search: Entity<TextInput>,
        blocked_tags: &HashSet<String>,
        colors: UiColors,
    ) -> impl IntoElement + use<> {
        let query = search.read(cx).text().trim().to_lowercase();
        let tags = filter_tag_summaries(&self.tag_list, &query, self.tag_manager_filter);
        let filtered = {
            let mut tags = tags;
            sort_tag_summaries(&mut tags, self.tag_manager_sort);
            tags
        };
        let total = self.tag_list.len();
        let (unused, low_frequency, auto_only) = tag_manager_counts(&self.tag_list);
        let blocked_count = self
            .tag_list
            .iter()
            .filter(|tag| blocked_tags.contains(&tag.name))
            .count();
        let this_back = this.clone();
        let this_refresh = this.clone();
        let body: AnyElement = if filtered.is_empty() {
            let this_reset = this.clone();
            let search_reset = search.clone();
            let reset_label = t(cx, "tags.reset_filter");
            let action = (total > 0).then(|| {
                Button::new("tag-empty-reset", reset_label)
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        let _ = this_reset.update(cx, |app, cx| {
                            app.tag_manager_filter = TagManagerFilter::All;
                            search_reset.update(cx, |input, cx| input.set_text("", cx));
                            cx.notify();
                        });
                    })
                    .into_any_element()
            });
            let (icon, title, description) = if total == 0 {
                (
                    IconName::Tag,
                    t(cx, "tags.empty.title"),
                    t(cx, "tags.empty.description"),
                )
            } else {
                (
                    IconName::SearchX,
                    t(cx, "tags.no_match.title"),
                    t(cx, "tags.no_match.description"),
                )
            };
            empty_state(icon, title, description, action, colors)
        } else {
            let this_list = this.clone();
            let blocked = blocked_tags.clone();
            let row_count = filtered.len();
            uniform_list("tag-manager-list", row_count, move |range, _, cx| {
                range
                    .map(|row| {
                        let tag = &filtered[row];
                        let blocked_row = blocked.contains(&tag.name);
                        let name = tag.name.clone();
                        let block_name = tag.name.clone();
                        let rename_name = tag.name.clone();
                        let merge_name = tag.name.clone();
                        let view_name = tag.name.clone();
                        let this_block = this_list.clone();
                        let this_rename = this_list.clone();
                        let this_merge = this_list.clone();
                        let this_view = this_list.clone();
                        let has_assets = tag.count > 0;
                        div()
                            .w_full()
                            .h(px(TAG_ROW_HEIGHT))
                            .px(px(16.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.))
                            .border_b_1()
                            .border_color(colors.border)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(if blocked_row {
                                                FontWeight::NORMAL
                                            } else {
                                                FontWeight::SEMIBOLD
                                            })
                                            .text_color(if blocked_row {
                                                colors.muted
                                            } else {
                                                colors.fg
                                            })
                                            .child(name),
                                    )
                                    .child(div().text_xs().text_color(colors.subtle).child(
                                        t_args(
                                            cx,
                                            if blocked_row {
                                                "tags.row.meta_blocked"
                                            } else {
                                                "tags.row.meta"
                                            },
                                            &[
                                                ("category", &tag.category),
                                                ("count", &tag.count.to_string()),
                                                ("manual", &tag.manual_count.to_string()),
                                            ],
                                        ),
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(
                                        Button::new(
                                            SharedString::from(format!("view-tag-{view_name}")),
                                            t(cx, "tags.view_assets"),
                                        )
                                        .size(ControlSize::Sm)
                                        .disabled(!has_assets)
                                        .on_click(
                                            move |_, _, cx| {
                                                let _ = this_view.update(cx, |app, cx| {
                                                    app.open_tag_assets(view_name.clone(), cx)
                                                });
                                            },
                                        ),
                                    )
                                    .child(
                                        IconButton::new(
                                            SharedString::from(format!("rename-tag-{rename_name}")),
                                            IconName::Pencil,
                                        )
                                        .size(ControlSize::Sm)
                                        .tooltip(t(cx, "tags.rename"))
                                        .on_click(
                                            move |_, _, cx| {
                                                let _ = this_rename.update(cx, |app, cx| {
                                                    app.open_tag_rename(rename_name.clone(), cx)
                                                });
                                            },
                                        ),
                                    )
                                    .child(
                                        IconButton::new(
                                            SharedString::from(format!("merge-tag-{merge_name}")),
                                            IconName::Merge,
                                        )
                                        .size(ControlSize::Sm)
                                        .tooltip(t(cx, "tags.merge"))
                                        .on_click(
                                            move |_, _, cx| {
                                                let _ = this_merge.update(cx, |app, cx| {
                                                    app.open_tag_merge(merge_name.clone(), cx)
                                                });
                                            },
                                        ),
                                    )
                                    .child(
                                        Button::new(
                                            SharedString::from(format!("block-tag-{}", block_name)),
                                            if blocked_row {
                                                t(cx, "tags.unblock")
                                            } else {
                                                t(cx, "tags.block")
                                            },
                                        )
                                        .size(ControlSize::Sm)
                                        .on_click(
                                            move |_, _, cx| {
                                                let _ = this_block.update(cx, |app, cx| {
                                                    app.set_tag_blocked(
                                                        block_name.clone(),
                                                        !blocked_row,
                                                        cx,
                                                    )
                                                });
                                            },
                                        ),
                                    ),
                            )
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.tag_scroll)
            .flex_1()
            .w_full()
            .min_h(px(0.))
            .into_any_element()
        };

        div()
            .w(width)
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(PAGE_HEADER_HEIGHT))
                    .px(px(SPACE_4))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(SPACE_2))
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w(px(0.))
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(t(cx, "tags.title")),
                            )
                            .child(div().text_xs().text_color(colors.muted).truncate().child(
                                t_args(
                                    cx,
                                    "tags.summary",
                                    &[
                                        ("total", &total.to_string()),
                                        ("unused", &unused.to_string()),
                                        ("low_frequency", &low_frequency.to_string()),
                                        ("auto_only", &auto_only.to_string()),
                                        ("blocked", &blocked_count.to_string()),
                                    ],
                                ),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(SPACE_1_5))
                            .child(
                                Button::new("tags-refresh", t(cx, "tags.refresh"))
                                    .icon(IconName::RefreshCw)
                                    .on_click(move |_, _, cx| {
                                        let _ = this_refresh
                                            .update(cx, |app, cx| app.refresh_tag_manager(cx));
                                    }),
                            )
                            .child(
                                Button::new("tags-back", t(cx, "common.back_to_library")).on_click(
                                    move |_, _, cx| {
                                        let _ = this_back.update(cx, |app, cx| {
                                            app.screen = Screen::Library;
                                            app.tag_return_pending = false;
                                            app.focus_grid_pending = true;
                                            cx.notify();
                                        });
                                    },
                                ),
                            ),
                    ),
            )
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str, count: usize, manual_count: usize) -> TagSummary {
        TagSummary {
            name: name.to_string(),
            category: "style".to_string(),
            count,
            manual_count,
        }
    }

    #[test]
    fn tag_filters_separate_unused_low_and_auto_only() {
        let tags = vec![
            tag("unused", 0, 0),
            tag("rare", 1, 0),
            tag("low", 2, 1),
            tag("popular", 8, 3),
            tag("auto", 9, 0),
        ];
        assert!(TagManagerFilter::All.matches(&tags[0]));
        assert!(TagManagerFilter::Unused.matches(&tags[0]));
        assert!(!TagManagerFilter::Unused.matches(&tags[1]));
        assert!(TagManagerFilter::LowFrequency.matches(&tags[1]));
        assert!(TagManagerFilter::LowFrequency.matches(&tags[2]));
        assert!(!TagManagerFilter::LowFrequency.matches(&tags[3]));
        assert!(TagManagerFilter::AutoOnly.matches(&tags[1]));
        assert!(TagManagerFilter::AutoOnly.matches(&tags[4]));
        assert!(!TagManagerFilter::AutoOnly.matches(&tags[2]));
        assert!(!TagManagerFilter::AutoOnly.matches(&tags[0]));
        assert_eq!(tag_manager_counts(&tags), (1, 2, 2));
    }

    #[test]
    fn tag_filter_respects_search_and_sorting_is_stable() {
        let tags = vec![
            tag("Beta", 3, 0),
            tag("alpha", 3, 2),
            tag("gamma", 1, 0),
            tag("unused", 0, 0),
        ];
        let filtered = filter_tag_summaries(&tags, "a", TagManagerFilter::All);
        assert_eq!(filtered.len(), 3);

        let mut sorted = filtered;
        sort_tag_summaries(&mut sorted, TagManagerSort::UsageAsc);
        assert_eq!(
            sorted
                .iter()
                .map(|tag| tag.name.as_str())
                .collect::<Vec<_>>(),
            vec!["gamma", "alpha", "Beta"]
        );
        sort_tag_summaries(&mut sorted, TagManagerSort::UsageDesc);
        assert_eq!(
            sorted
                .iter()
                .map(|tag| tag.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "Beta", "gamma"]
        );
        sort_tag_summaries(&mut sorted, TagManagerSort::ManualDesc);
        assert_eq!(sorted.first().map(|tag| tag.name.as_str()), Some("alpha"));
    }

    #[test]
    fn tag_filters_never_drop_data() {
        // 筛选只返回副本；原词表保持完整（不自动删除）。
        let tags = vec![tag("unused", 0, 0), tag("used", 4, 1)];
        let filtered = filter_tag_summaries(&tags, "", TagManagerFilter::Unused);
        assert_eq!(filtered.len(), 1);
        assert_eq!(tags.len(), 2);
    }
}
