use crate::*;
use crate::settings::TAG_ROW_HEIGHT;
use crate::app::{KanalibApp, push_toast};
use crate::theme::*;

/// 低置信度审核最多加载的候选条数。
pub(crate) const TAG_REVIEW_LIMIT: usize = 80;

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
                        let message = format!("读取待审候选失败：{error:#}");
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
    pub(crate) fn apply_asset_review(&mut self, path: PathBuf, state: TagReviewState, cx: &mut Context<Self>) {
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
            (Some(tag), TagReviewState::Accepted) => format!("接受候选 #{tag}"),
            (Some(tag), TagReviewState::Ignored) => format!("忽略候选 #{tag}"),
            (Some(tag), TagReviewState::Pending) => format!("恢复候选 #{tag}"),
            (None, TagReviewState::Accepted) => "整张接受".to_string(),
            (None, TagReviewState::Ignored) => "整张忽略".to_string(),
            (None, TagReviewState::Pending) => "整张恢复待审".to_string(),
        };
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
                        let message = format!("{label} · 处理 {changed} 条");
                        app.tag_review_notice = Some(message.clone());
                        push_toast(&toaster, cx, message, Severity::Success);
                        app.refresh_tag_reviews(cx);
                    }
                    Err(error) => {
                        let message = format!("{label}失败：{error:#}");
                        app.tag_review_notice = Some(message.clone());
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
        match self.state.list_tags() {
            Ok(tags) => self.tag_list = tags,
            Err(error) => {
                log::warn!("list tags: {error:#}");
                self.tag_list.clear();
                push_toast(
                    &self.toaster,
                    cx,
                    format!("读取标签失败：{error:#}"),
                    Severity::Danger,
                );
            }
        }
        self.screen = Screen::Tags;
        self.dedup_confirm = None;
        self.palette_open = false;
        self.tag_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    pub(crate) fn refresh_tag_manager(&mut self, cx: &mut Context<Self>) {
        match self.state.list_tags() {
            Ok(tags) => self.tag_list = tags,
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("读取标签失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    pub(crate) fn set_tag_blocked(&mut self, name: String, blocked: bool, cx: &mut Context<Self>) {
        log::info!("tag manager: set blocked name={name} blocked={blocked}");
        match self.state.set_tag_blocked(&name, blocked) {
            Ok(()) => push_toast(
                &self.toaster,
                cx,
                if blocked {
                    format!("已屏蔽标签 #{name}")
                } else {
                    format!("已取消屏蔽标签 #{name}")
                },
                Severity::Success,
            ),
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("更新屏蔽失败：{error:#}"),
                Severity::Danger,
            ),
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
                push_toast(
                    &self.toaster,
                    cx,
                    format!("已重命名标签 · 影响 {affected} 张"),
                    Severity::Success,
                );
                self.refresh_tag_manager(cx);
            }
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("重命名标签失败：{error:#}"),
                Severity::Danger,
            ),
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
                push_toast(
                    &self.toaster,
                    cx,
                    format!("已合并标签 · 影响 {affected} 张"),
                    Severity::Success,
                );
                self.refresh_tag_manager(cx);
            }
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("合并标签失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    pub(crate) fn open_tag_dialog(&mut self, mode: TagDialogMode, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            push_toast(&self.toaster, cx, "先选择图片", Severity::Warning);
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
            push_toast(&self.toaster, cx, "先选择图片", Severity::Warning);
            return;
        }
        let result = match mode {
            TagDialogMode::Add => self.state.add_tags(&paths, name),
            TagDialogMode::Remove => self.state.remove_tags(&paths, name),
        };
        match result {
            Ok(0) => push_toast(
                &self.toaster,
                cx,
                if mode == TagDialogMode::Add {
                    "标签已存在或名称无效"
                } else {
                    "所选图片没有这个标签"
                },
                Severity::Warning,
            ),
            Ok(updated) => {
                self.tag_dialog = None;
                self.tag_dialog_paths.clear();
                push_toast(
                    &self.toaster,
                    cx,
                    if mode == TagDialogMode::Add {
                        format!("已为 {updated} 张添加标签 #{name}")
                    } else {
                        format!("已从 {updated} 张移除标签 #{name}")
                    },
                    Severity::Success,
                );
            }
            Err(error) => {
                self.state.set_error(format!("标签操作失败：{error:#}"));
                push_toast(
                    &self.toaster,
                    cx,
                    format!("标签操作失败：{error:#}"),
                    Severity::Danger,
                );
            }
        }
        cx.notify();
    }

    pub(crate) fn render_tags(
        &mut self,
        this: &Entity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
        width: Pixels,
        blocked_tags: &HashSet<String>,
        colors: UiColors,
    ) -> impl IntoElement + use<> {
        let search = window.use_keyed_state("tag-manager-search", cx, |window, cx| {
            TextInput::new(window, cx).placeholder("筛选标签名 / 类别")
        });
        let query = search.read(cx).text().trim().to_lowercase();
        let filtered = self
            .tag_list
            .iter()
            .filter(|tag| {
                query.is_empty()
                    || tag.name.to_lowercase().contains(&query)
                    || tag.category.to_lowercase().contains(&query)
            })
            .cloned()
            .collect::<Vec<_>>();
        let total = self.tag_list.len();
        let blocked_count = self
            .tag_list
            .iter()
            .filter(|tag| blocked_tags.contains(&tag.name))
            .count();
        let this_back = this.clone();
        let this_refresh = this.clone();
        let body: AnyElement = if filtered.is_empty() {
            div()
                .flex_1()
                .p(px(24.))
                .child(
                    div()
                        .p(px(24.))
                        .rounded(px(8.))
                        .bg(colors.sunken)
                        .text_sm()
                        .text_color(colors.muted)
                        .child(if total == 0 {
                            "还没有标签，先导入图片并完成自动打标。"
                        } else {
                            "没有匹配的标签。"
                        }),
                )
                .into_any_element()
        } else {
            let this_list = this.clone();
            let blocked = blocked_tags.clone();
            let row_count = filtered.len();
            uniform_list("tag-manager-list", row_count, move |range, _, _| {
                range
                    .map(|row| {
                        let tag = &filtered[row];
                        let blocked_row = blocked.contains(&tag.name);
                        let name = tag.name.clone();
                        let block_name = tag.name.clone();
                        let rename_name = tag.name.clone();
                        let merge_name = tag.name.clone();
                        let this_block = this_list.clone();
                        let this_rename = this_list.clone();
                        let this_merge = this_list.clone();
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
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(colors.subtle)
                                            .child(format!(
                                                "#{} · {} 张 · 手工 {} 次{}",
                                                tag.category,
                                                tag.count,
                                                tag.manual_count,
                                                if blocked_row { " · 已屏蔽" } else { "" }
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(
                                        IconButton::new(
                                            SharedString::from(format!(
                                                "rename-tag-{rename_name}"
                                            )),
                                            IconName::Pencil,
                                        )
                                        .size(ControlSize::Sm)
                                        .tooltip("重命名")
                                        .on_click(move |_, _, cx| {
                                            let _ = this_rename.update(cx, |app, cx| {
                                                app.open_tag_rename(rename_name.clone(), cx)
                                            });
                                        }),
                                    )
                                    .child(
                                        IconButton::new(
                                            SharedString::from(format!(
                                                "merge-tag-{merge_name}"
                                            )),
                                            IconName::Merge,
                                        )
                                        .size(ControlSize::Sm)
                                        .tooltip("合并到其他标签")
                                        .on_click(move |_, _, cx| {
                                            let _ = this_merge.update(cx, |app, cx| {
                                                app.open_tag_merge(merge_name.clone(), cx)
                                            });
                                        }),
                                    )
                                    .child(
                                        Button::new(
                                            SharedString::from(format!(
                                                "block-tag-{}",
                                                block_name
                                            )),
                                            if blocked_row { "取消屏蔽" } else { "屏蔽" },
                                        )
                                        .size(ControlSize::Sm)
                                        .on_click(move |_, _, cx| {
                                            let _ = this_block.update(cx, |app, cx| {
                                                app.set_tag_blocked(
                                                    block_name.clone(),
                                                    !blocked_row,
                                                    cx,
                                                )
                                            });
                                        }),
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
                    .h(px(58.))
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .border_1()
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
                                    .child("标签管理"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.muted)
                                    .child(format!(
                                        "共 {total} 个标签 · 已屏蔽 {blocked_count} 个 · 屏蔽只影响侧栏标签面"
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .w(px(260.))
                                    .child(Input::new(&search).clearable().least("筛选标签")),
                            )
                            .child(
                                Button::new("tags-refresh", "刷新")
                                    .size(ControlSize::Sm)
                                    .icon(IconName::RefreshCw)
                                    .on_click(move |_, _, cx| {
                                        let _ = this_refresh.update(cx, |app, cx| {
                                            app.refresh_tag_manager(cx)
                                        });
                                    }),
                            )
                            .child(
                                Button::new("tags-back", "返回图库")
                                    .size(ControlSize::Sm)
                                    .on_click(move |_, _, cx| {
                                        let _ = this_back.update(cx, |app, cx| {
                                            app.screen = Screen::Library;
                                            app.focus_grid_pending = true;
                                            cx.notify();
                                        });
                                    }),
                            ),
                    ),
            )
            .child(body)
    }

}
