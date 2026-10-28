use crate::*;
use crate::app::{KanalibApp, push_toast};
use crate::format::*;
use crate::theme::*;
use gpui::{Bounds, Point};

/// F-003：对比视图的键上下文；Esc 关闭对比，+/-/0 缩放，方向键平移。
pub(crate) const COMPARE_CONTEXT: &str = "KanalibCompare";
pub(crate) const COMPARE_MAX_ITEMS: usize = 4;
const COMPARE_MIN_ITEMS: usize = 2;
const ZOOM_MIN: f32 = 1.;
const ZOOM_MAX: f32 = 16.;
const HEADER_HEIGHT: f32 = 48.;
const FOOTER_HEIGHT: f32 = 98.;
const DIFF_HEIGHT: f32 = 268.;
const PANE_GAP: f32 = 10.;
const PANE_PADDING: f32 = 12.;
const DIFF_LABEL_WIDTH: f32 = 76.;

/// 每个对比栏的实际图像区；canvas 在 prepaint 写入，用于缩放中心与平移换算。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CompareStage {
    pub(crate) bounds: Bounds<Pixels>,
}

impl CompareStage {
    fn size(&self) -> (f32, f32) {
        (
            f32::from(self.bounds.size.width),
            f32::from(self.bounds.size.height),
        )
    }
}

/// 每栏的拖拽起点；只记录本栏上一次指针位置。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CompareDrag {
    pub(crate) from: Option<Point<Pixels>>,
}

/// 适配比例：整图放进图像区，小图不放大（与查看器的 fit 一致）。
pub(crate) fn fit_scale(image: (f32, f32), stage: (f32, f32)) -> f32 {
    if image.0 <= 0. || image.1 <= 0. || stage.0 <= 0. || stage.1 <= 0. {
        return 1.;
    }
    (stage.0 / image.0).min(stage.1 / image.1).min(1.0)
}

/// 当前缩放下的显示尺寸（适配比例 × 同步缩放倍数）。
pub(crate) fn zoomed_size(image: (f32, f32), stage: (f32, f32), zoom: f32) -> (f32, f32) {
    let scale = fit_scale(image, stage) * zoom.max(ZOOM_MIN);
    (image.0 * scale, image.1 * scale)
}

/// 每栏独立钳制平移：图片比图像区小的一侧保持居中，不露出空白。
pub(crate) fn limited_offset(pan: (f32, f32), shown: (f32, f32), stage: (f32, f32)) -> (f32, f32) {
    let limit = |shown: f32, stage: f32| ((shown - stage) / 2.0).max(0.0);
    (
        pan.0.clamp(-limit(shown.0, stage.0), limit(shown.0, stage.0)),
        pan.1.clamp(-limit(shown.1, stage.1), limit(shown.1, stage.1)),
    )
}

/// 以图像区的同一个点为中心缩放；所有栏共享平移量，因此缩放中心同步。
pub(crate) fn zoom_about(
    pan: (f32, f32),
    at: (f32, f32),
    zoom: f32,
    factor: f32,
) -> (f32, (f32, f32)) {
    let next = (zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
    if (next - zoom).abs() < 1e-4 {
        return (zoom, pan);
    }
    if next <= ZOOM_MIN {
        return (next, (0., 0.));
    }
    let ratio = next / zoom;
    (
        next,
        (
            at.0 - (at.0 - pan.0) * ratio,
            at.1 - (at.1 - pan.1) * ratio,
        ),
    )
}

#[derive(Clone, Debug)]
pub(crate) struct CompareRow {
    pub(crate) label: &'static str,
    pub(crate) values: Vec<String>,
    pub(crate) same: bool,
}

fn push_row(rows: &mut Vec<CompareRow>, label: &'static str, values: Vec<String>) {
    let same = values
        .first()
        .is_some_and(|first| values.iter().all(|value| value == first));
    rows.push(CompareRow {
        label,
        values,
        same,
    });
}

/// 对比栏与差异表共用的元数据行；`same` 为 true 的行在表里弱化。
pub(crate) fn compare_metadata_rows(assets: &[Asset]) -> Vec<CompareRow> {
    let mut rows = Vec::new();
    push_row(
        &mut rows,
        "尺寸",
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
    );
    push_row(
        &mut rows,
        "文件大小",
        assets
            .iter()
            .map(|asset| format_bytes(asset.bytes))
            .collect(),
    );
    push_row(
        &mut rows,
        "格式",
        assets
            .iter()
            .map(|asset| {
                asset
                    .path
                    .extension()
                    .map(|ext| ext.to_string_lossy().to_uppercase())
                    .unwrap_or_else(|| "—".to_string())
            })
            .collect(),
    );
    push_row(
        &mut rows,
        "拍摄时间",
        assets
            .iter()
            .map(|asset| {
                asset
                    .captured_at
                    .map(format_date)
                    .unwrap_or_else(|| "未记录".to_string())
            })
            .collect(),
    );
    push_row(
        &mut rows,
        "修改时间",
        assets
            .iter()
            .map(|asset| format_date(asset.mtime))
            .collect(),
    );
    push_row(
        &mut rows,
        "星级",
        assets
            .iter()
            .map(|asset| {
                if asset.star > 0 {
                    format!("★ {}", asset.star)
                } else {
                    "未设".to_string()
                }
            })
            .collect(),
    );
    push_row(
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
    );
    push_row(
        &mut rows,
        "标签",
        assets
            .iter()
            .map(|asset| {
                let top = asset
                    .top_tags(2)
                    .into_iter()
                    .map(|hit| hit.tag.clone())
                    .collect::<Vec<_>>()
                    .join(" / ");
                let count = asset.all_tags().count();
                if top.is_empty() {
                    format!("{count} 个")
                } else {
                    format!("{count} 个 · {top}")
                }
            })
            .collect(),
    );
    push_row(
        &mut rows,
        "路径",
        assets
            .iter()
            .map(|asset| asset.path.display().to_string())
            .collect(),
    );
    rows
}

impl KanalibApp {
    // ---- F-003：图片对比（并排 2–4 张、同步缩放/平移、差异表、保留标记） ----

    /// 命令面板 / 检查器 / 右键菜单入口：从当前选中里取 2–4 张。
    pub(crate) fn open_compare_selected(&mut self, cx: &mut Context<Self>) {
        let mut selected = if self.selection.is_empty() {
            self.state
                .shared
                .lock()
                .unwrap()
                .selected
                .iter()
                .cloned()
                .collect::<HashSet<_>>()
        } else {
            (*self.selection).clone()
        };
        let requested = selected.len();
        if requested < COMPARE_MIN_ITEMS {
            push_toast(
                &self.toaster,
                cx,
                format!("先选中 {COMPARE_MIN_ITEMS}–{COMPARE_MAX_ITEMS} 张图片再对比"),
                Severity::Warning,
            );
            return;
        }
        let order = if self.screen == Screen::Similar {
            self.similar_visible.clone()
        } else {
            self.view.visible_indices.clone()
        };
        let mut items = Vec::new();
        {
            let shared = self.state.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            for &index in order.iter() {
                let Some(asset) = assets.get(index) else {
                    continue;
                };
                if selected.remove(&asset.path) {
                    items.push(asset.clone());
                }
                if items.len() >= COMPARE_MAX_ITEMS {
                    break;
                }
            }
            // 选中项不在当前可见列表里时（相似结果等），按库内顺序补齐。
            if items.len() < COMPARE_MAX_ITEMS && !selected.is_empty() {
                for asset in assets.iter() {
                    if selected.remove(&asset.path) {
                        items.push(asset.clone());
                    }
                    if items.len() >= COMPARE_MAX_ITEMS {
                        break;
                    }
                }
            }
        }
        if items.len() < COMPARE_MIN_ITEMS {
            push_toast(
                &self.toaster,
                cx,
                "选中的图片已不在图库中，无法对比",
                Severity::Warning,
            );
            return;
        }
        let truncated = requested > COMPARE_MAX_ITEMS;
        let origin = self.screen;
        self.start_compare(items, None, origin, cx);
        if truncated {
            push_toast(
                &self.toaster,
                cx,
                format!("最多同时对比 {COMPARE_MAX_ITEMS} 张，已取前 {COMPARE_MAX_ITEMS} 张"),
                Severity::Info,
            );
        }
    }

    /// 去重详情入口：组内 2–4 张；更多时取前 4 张。
    pub(crate) fn open_compare_group(&mut self, group: &DuplicateGroup, cx: &mut Context<Self>) {
        if group.assets.len() < COMPARE_MIN_ITEMS {
            push_toast(
                &self.toaster,
                cx,
                "组内至少要有 2 张才能对比",
                Severity::Warning,
            );
            return;
        }
        let items = group
            .assets
            .iter()
            .take(COMPARE_MAX_ITEMS)
            .cloned()
            .collect::<Vec<_>>();
        let keeper = group
            .keeper
            .clone()
            .filter(|path| items.iter().any(|asset| &asset.path == path));
        let truncated = group.assets.len() > COMPARE_MAX_ITEMS;
        let total = group.assets.len();
        self.start_compare(items, keeper, Screen::Duplicates, cx);
        if truncated {
            push_toast(
                &self.toaster,
                cx,
                format!("组内 {total} 张，仅对比前 {COMPARE_MAX_ITEMS} 张"),
                Severity::Info,
            );
        }
    }

    fn start_compare(
        &mut self,
        items: Vec<Asset>,
        keeper: Option<PathBuf>,
        origin: Screen,
        cx: &mut Context<Self>,
    ) {
        if self.viewer_at.is_some() {
            self.close_viewer(cx);
        }
        let handle = if origin == Screen::Duplicates {
            &self.dedup_scroll
        } else {
            &self.grid_scroll
        };
        self.compare_origin_scroll = Some(handle.0.borrow().base_handle.offset());
        self.compare_origin = origin;
        self.compare_items = items;
        self.compare_zoom = ZOOM_MIN;
        self.compare_pan = (0., 0.);
        self.compare_keeper = keeper;
        self.compare_original = false;
        self.screen = Screen::Compare;
        self.compare_focus_pending = true;
        self.focus_grid_pending = false;
        self.palette_open = false;
        cx.notify();
    }

    /// Esc / 返回按钮：回到原视图并恢复进入前的滚动位置；不删除、不移动任何文件。
    pub(crate) fn close_compare(&mut self, cx: &mut Context<Self>) {
        let origin = self.compare_origin;
        let saved = self.compare_origin_scroll.take();
        let handle = if origin == Screen::Duplicates {
            &self.dedup_scroll
        } else {
            &self.grid_scroll
        };
        if let Some(offset) = saved {
            let mut state = handle.0.borrow_mut();
            state.base_handle.set_offset(offset);
            state.deferred_scroll_to_item = None;
        }
        self.screen = origin;
        self.compare_items.clear();
        self.compare_keeper = None;
        self.compare_zoom = ZOOM_MIN;
        self.compare_pan = (0., 0.);
        self.focus_grid_pending = true;
        cx.notify();
    }

    pub(crate) fn compare_zoom_by(
        &mut self,
        at: (f32, f32),
        factor: f32,
        cx: &mut Context<Self>,
    ) {
        if self.screen != Screen::Compare {
            return;
        }
        let (zoom, pan) = zoom_about(self.compare_pan, at, self.compare_zoom, factor);
        if zoom == self.compare_zoom {
            return;
        }
        self.compare_zoom = zoom;
        self.compare_pan = pan;
        cx.notify();
    }

    pub(crate) fn compare_pan_by(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        if self.screen != Screen::Compare || self.compare_zoom <= ZOOM_MIN + 1e-3 {
            return;
        }
        self.compare_pan.0 = (self.compare_pan.0 + dx).clamp(-20_000., 20_000.);
        self.compare_pan.1 = (self.compare_pan.1 + dy).clamp(-20_000., 20_000.);
        cx.notify();
    }

    pub(crate) fn compare_fit(&mut self, cx: &mut Context<Self>) {
        self.compare_zoom = ZOOM_MIN;
        self.compare_pan = (0., 0.);
        cx.notify();
    }

    pub(crate) fn compare_toggle_original(&mut self, cx: &mut Context<Self>) {
        self.compare_original = !self.compare_original;
        cx.notify();
    }

    /// 保留标记只存在内存里；再次点击同一张即取消。
    pub(crate) fn compare_mark_keeper(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.compare_keeper.as_ref() == Some(&path) {
            self.compare_keeper = None;
        } else {
            self.compare_keeper = Some(path);
        }
        cx.notify();
    }

    /// 右键时若目标已在多选集合里，保留多选，便于从右键菜单直接进入对比。
    pub(crate) fn handle_tile_right_click(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.selection.len() > 1 && self.selection.contains(&path) {
            self.state.select(Some(path));
            cx.notify();
        } else {
            self.select_single_path(path, cx);
        }
    }

    pub(crate) fn render_compare(
        &mut self,
        this: &Entity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
        width: Pixels,
        colors: UiColors,
    ) -> AnyElement {
        if self.compare_focus_pending {
            window.focus(&self.compare_focus, cx);
            self.compare_focus_pending = false;
        }
        if self.compare_items.is_empty() {
            return div()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(colors.muted)
                .child("没有可对比的图片")
                .into_any_element();
        }
        let count = self.compare_items.len();
        let viewport_height = f32::from(window.viewport_size().height);
        let content_height = (viewport_height - TITLEBAR_HEIGHT - STATUS_BAR_HEIGHT).max(240.);
        let estimated_stage = (
            ((f32::from(width) - PANE_PADDING * 2.0 - PANE_GAP * (count as f32 - 1.0))
                / count as f32)
                .max(80.0),
            (content_height - HEADER_HEIGHT - DIFF_HEIGHT - PANE_PADDING * 2.0 - FOOTER_HEIGHT)
                .max(120.0),
        );
        let mut panes = Vec::new();
        for (index, asset) in self.compare_items.iter().enumerate() {
            let stage = window.use_keyed_state(
                SharedString::from(format!("kanalib-compare-stage-{index}")),
                cx,
                |_, _| CompareStage::default(),
            );
            let drag = window.use_keyed_state(
                SharedString::from(format!("kanalib-compare-drag-{index}")),
                cx,
                |_, _| CompareDrag::default(),
            );
            let measured = stage.read(cx).size();
            let stage_size = if measured.0 > 1. && measured.1 > 1. {
                measured
            } else {
                estimated_stage
            };
            panes.push(self.render_compare_pane(
                this,
                asset,
                index,
                count,
                &stage,
                &drag,
                stage_size,
                colors,
            ));
        }
        let keeper = self.compare_keeper.clone();
        let rows = compare_metadata_rows(&self.compare_items);
        let diff_rows = rows.iter().filter(|row| !row.same).count();

        let close_button = this.clone();
        let zoom_out = this.clone();
        let zoom_in = this.clone();
        let fit = this.clone();
        let original = this.clone();
        let close_action = this.clone();
        let zoom_in_action = this.clone();
        let zoom_out_action = this.clone();
        let fit_action = this.clone();
        let pan_left = this.clone();
        let pan_right = this.clone();
        let pan_up = this.clone();
        let pan_down = this.clone();
        let back_label = match self.compare_origin {
            Screen::Similar => "返回结果",
            Screen::Duplicates => "返回重复项",
            Screen::Tags => "返回标签",
            Screen::Settings => "返回设置",
            _ => "返回图库",
        };
        let zoom_label = if self.compare_zoom <= ZOOM_MIN + 1e-3 {
            "适配".to_string()
        } else {
            format!("×{:.2}", self.compare_zoom)
        };

        let header = div()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .px(px(PANE_PADDING))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(10.))
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .min_w(px(0.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Button::new("compare-back", back_label)
                            .icon(IconName::ChevronLeft)
                            .size(ControlSize::Sm)
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_, _, cx| {
                                let _ = close_button
                                    .update(cx, |app, cx| app.close_compare(cx));
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("图片对比 · {count} 张")),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child("同步缩放/平移 · 绿色边框 = 保留项"),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        Button::new("compare-fit", "适应")
                            .size(ControlSize::Sm)
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_, _, cx| {
                                let _ = fit.update(cx, |app, cx| app.compare_fit(cx));
                            }),
                    )
                    .child(
                        IconButton::new("compare-zoom-out", IconName::ZoomOut)
                            .size(ControlSize::Sm)
                            .tooltip("缩小 (-)")
                            .on_click(move |_, _, cx| {
                                let _ = zoom_out.update(cx, |app, cx| {
                                    app.compare_zoom_by((0., 0.), 0.8, cx)
                                });
                            }),
                    )
                    .child(
                        div()
                            .w(px(54.))
                            .flex_none()
                            .text_xs()
                            .text_color(colors.muted)
                            .flex()
                            .justify_center()
                            .child(zoom_label),
                    )
                    .child(
                        IconButton::new("compare-zoom-in", IconName::ZoomIn)
                            .size(ControlSize::Sm)
                            .tooltip("放大 (+)")
                            .on_click(move |_, _, cx| {
                                let _ = zoom_in.update(cx, |app, cx| {
                                    app.compare_zoom_by((0., 0.), 1.25, cx)
                                });
                            }),
                    )
                    .child(
                        Button::new(
                            "compare-source",
                            if self.compare_original { "预览图" } else { "原图" },
                        )
                        .size(ControlSize::Sm)
                        .variant(ButtonVariant::Ghost)
                        .on_click(move |_, _, cx| {
                            let _ = original
                                .update(cx, |app, cx| app.compare_toggle_original(cx));
                        }),
                    ),
            );

        let panes_row = div()
            .id("compare-panes")
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .flex()
            .gap(px(PANE_GAP))
            .px(px(PANE_PADDING))
            .py(px(PANE_PADDING))
            .children(panes);

        let diff = self.render_compare_diff(&self.compare_items, &rows, keeper.as_deref(), colors);

        div()
            .id("kanalib-compare")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.fg)
            .key_context(COMPARE_CONTEXT)
            .track_focus(&self.compare_focus)
            .on_action(move |_: &CompareClose, _, cx| {
                let _ = close_action.update(cx, |app, cx| app.close_compare(cx));
            })
            .on_action(move |_: &CompareZoomIn, _, cx| {
                let _ = zoom_in_action.update(cx, |app, cx| {
                    app.compare_zoom_by((0., 0.), 1.25, cx)
                });
            })
            .on_action(move |_: &CompareZoomOut, _, cx| {
                let _ = zoom_out_action.update(cx, |app, cx| {
                    app.compare_zoom_by((0., 0.), 0.8, cx)
                });
            })
            .on_action(move |_: &CompareFit, _, cx| {
                let _ = fit_action.update(cx, |app, cx| app.compare_fit(cx));
            })
            .on_action(move |_: &ComparePanLeft, _, cx| {
                let _ = pan_left.update(cx, |app, cx| app.compare_pan_by(40., 0., cx));
            })
            .on_action(move |_: &ComparePanRight, _, cx| {
                let _ = pan_right.update(cx, |app, cx| app.compare_pan_by(-40., 0., cx));
            })
            .on_action(move |_: &ComparePanUp, _, cx| {
                let _ = pan_up.update(cx, |app, cx| app.compare_pan_by(0., 40., cx));
            })
            .on_action(move |_: &ComparePanDown, _, cx| {
                let _ = pan_down.update(cx, |app, cx| app.compare_pan_by(0., -40., cx));
            })
            .child(header)
            .child(panes_row)
            .child(
                div()
                    .truncate()
                    .px(px(PANE_PADDING))
                    .pb(px(4.))
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(format!(
                        "差异 {diff_rows} 项 · 相同 {} 项 · 保留标记只影响本次对比，不删除或移动文件；Esc 返回",
                        rows.len().saturating_sub(diff_rows)
                    )),
            )
            .child(diff)
            .into_any_element()
    }

    fn render_compare_pane(
        &self,
        this: &Entity<Self>,
        asset: &Asset,
        index: usize,
        count: usize,
        stage: &Entity<CompareStage>,
        drag: &Entity<CompareDrag>,
        stage_size: (f32, f32),
        colors: UiColors,
    ) -> AnyElement {
        let source: PathBuf = if self.compare_original {
            asset.path.clone()
        } else {
            asset
                .thumb
                .clone()
                .or_else(|| asset.tile_thumb.clone())
                .unwrap_or_else(|| asset.path.clone())
        };
        let image_size = (asset.width as f32, asset.height as f32);
        let shown = zoomed_size(image_size, stage_size, self.compare_zoom);
        let offset = limited_offset(self.compare_pan, shown, stage_size);
        let left = (stage_size.0 - shown.0) / 2.0 + offset.0;
        let top = (stage_size.1 - shown.1) / 2.0 + offset.1;
        let is_keeper = self.compare_keeper.as_ref() == Some(&asset.path);
        let border = if is_keeper {
            colors.success
        } else {
            colors.border
        };

        let media: AnyElement = if asset.width == 0 || asset.height == 0 {
            img(source)
                .image_cache(&self.thumb_cache)
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        } else {
            img(source)
                .image_cache(&self.thumb_cache)
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(shown.0.max(1.0)))
                .h(px(shown.1.max(1.0)))
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        };

        let this_wheel = this.clone();
        let stage_wheel = stage.clone();
        let this_press = this.clone();
        let drag_press = drag.clone();
        let this_move = this.clone();
        let drag_move = drag.clone();
        let drag_up = drag.clone();
        let drag_out = drag.clone();
        let stage_canvas = stage.clone();
        let stage_box = div()
            .id(SharedString::from(format!("compare-stage-{index}")))
            .relative()
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .overflow_hidden()
            .bg(colors.sunken)
            .cursor_grab()
            .on_scroll_wheel(move |event, window, cx| {
                let delta = event.delta.pixel_delta(window.line_height());
                let zooming = event.modifiers.platform || event.modifiers.control;
                let bounds = stage_wheel.read(cx).bounds;
                let center = bounds.center();
                let at = event.position.relative_to(&center);
                if zooming {
                    let factor = (-f32::from(delta.y) * 0.01).exp();
                    let _ = this_wheel.update(cx, |app, cx| {
                        app.compare_zoom_by((f32::from(at.x), f32::from(at.y)), factor, cx)
                    });
                } else {
                    let _ = this_wheel.update(cx, |app, cx| {
                        app.compare_pan_by(f32::from(delta.x), f32::from(delta.y), cx)
                    });
                }
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                let _ = this_press.update(cx, |_, cx| {
                    drag_press.update(cx, |drag, cx| {
                        drag.from = Some(event.position);
                        cx.notify();
                    });
                });
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                drag_up.update(cx, |drag, cx| {
                    if drag.from.take().is_some() {
                        cx.notify();
                    }
                });
            })
            .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
                drag_out.update(cx, |drag, cx| {
                    if drag.from.take().is_some() {
                        cx.notify();
                    }
                });
            })
            .on_mouse_move(move |event, _, cx| {
                let from = drag_move.read(cx).from;
                let Some(from) = from else {
                    return;
                };
                let delta = event.position.relative_to(&from);
                if delta.x == px(0.) && delta.y == px(0.) {
                    return;
                }
                drag_move.update(cx, |drag, _| drag.from = Some(event.position));
                let _ = this_move.update(cx, |app, cx| {
                    app.compare_pan_by(f32::from(delta.x), f32::from(delta.y), cx)
                });
            })
            .child(
                canvas(
                    move |bounds, window, cx| {
                        if stage_canvas.read(cx).bounds != bounds {
                            stage_canvas.update(cx, |stage, cx| {
                                stage.bounds = bounds;
                                cx.notify();
                            });
                            window.request_animation_frame();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(media)
            .children(is_keeper.then(|| {
                div()
                    .absolute()
                    .top(px(7.))
                    .left(px(7.))
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(colors.success_subtle)
                    .text_xs()
                    .text_color(colors.success)
                    .child("保留")
            }))
            .child(
                div()
                    .absolute()
                    .top(px(7.))
                    .right(px(7.))
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(colors.backdrop)
                    .text_xs()
                    .text_color(colors.muted)
                    .child(format!("{} / {count}", index + 1)),
            );

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
        let star = if asset.star > 0 {
            format!("★ {}", asset.star)
        } else {
            "未设星级".to_string()
        };
        let tags = asset.all_tags().count();
        let keeper_button = Button::new(
            SharedString::from(format!("compare-keeper-{index}")),
            if is_keeper { "取消保留" } else { "标记保留" },
        )
        .size(ControlSize::Sm)
        .variant(ButtonVariant::Ghost)
        .icon(if is_keeper { IconName::Check } else { IconName::Star })
        .on_click({
            let this = this.clone();
            let path = asset.path.clone();
            move |_, _, cx| {
                let _ = this.update(cx, |app, cx| app.compare_mark_keeper(path.clone(), cx));
            }
        });

        div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_col()
            .rounded(px(9.))
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .bg(colors.surface)
            .child(stage_box)
            .child(
                div()
                    .flex_none()
                    .h(px(FOOTER_HEIGHT))
                    .px(px(9.))
                    .py(px(6.))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .border_t_1()
                    .border_color(colors.border)
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
                            .child(format!("{star} · {rating} · {tags} 标签")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.))
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(if is_keeper {
                                        colors.success
                                    } else {
                                        colors.subtle
                                    })
                                    .child(if is_keeper {
                                        "当前保留项（仅标记）"
                                    } else {
                                        "仅标记，不改文件"
                                    }),
                            )
                            .child(keeper_button),
                    ),
            )
            .into_any_element()
    }

    fn render_compare_diff(
        &self,
        assets: &[Asset],
        rows: &[CompareRow],
        keeper: Option<&Path>,
        colors: UiColors,
    ) -> AnyElement {
        let header = div()
            .flex()
            .items_center()
            .bg(colors.sunken)
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .w(px(DIFF_LABEL_WIDTH))
                    .flex_none()
                    .px(px(7.))
                    .py(px(4.))
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("字段"),
            )
            .children(assets.iter().map(|asset| {
                let is_keeper = keeper == Some(asset.path.as_path());
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .px(px(7.))
                    .py(px(4.))
                    .truncate()
                    .text_xs()
                    .text_color(if is_keeper {
                        colors.success
                    } else {
                        colors.subtle
                    })
                    .child(if is_keeper {
                        format!("{} · 保留", asset.title())
                    } else {
                        asset.title()
                    })
            }));
        let table = div()
            .rounded(px(7.))
            .border_1()
            .border_color(colors.border)
            .overflow_hidden()
            .child(header)
            .children(rows.iter().map(|row| {
                let label_color = if row.same {
                    colors.subtle
                } else {
                    colors.warning
                };
                div()
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .w(px(DIFF_LABEL_WIDTH))
                            .flex_none()
                            .px(px(7.))
                            .py(px(4.))
                            .text_xs()
                            .text_color(label_color)
                            .child(row.label),
                    )
                    .children(row.values.iter().map(|value| {
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .px(px(7.))
                            .py(px(4.))
                            .truncate()
                            .text_xs()
                            .text_color(if row.same {
                                colors.subtle
                            } else {
                                colors.muted
                            })
                            .child(value.clone())
                    }))
            }));
        div()
            .id("compare-diff")
            .flex_none()
            .h(px(DIFF_HEIGHT))
            .px(px(PANE_PADDING))
            .pb(px(PANE_PADDING))
            .overflow_y_scroll()
            .child(table)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        compare_metadata_rows, fit_scale, limited_offset, zoom_about, zoomed_size,
    };
    use kanalib_core::model::Asset;

    fn asset(
        path: &str,
        width: u32,
        height: u32,
        bytes: u64,
        star: u8,
        rating: &str,
        captured_at: Option<u64>,
        tags: &[&str],
    ) -> Asset {
        Asset {
            path: std::path::PathBuf::from(path),
            library_root: std::path::PathBuf::from(r"C:\pics"),
            thumb: None,
            tile_thumb: None,
            width,
            height,
            group: String::new(),
            rating: rating.to_string(),
            favorite: false,
            star,
            tags: tags
                .iter()
                .map(|tag| kanalib_core::model::TagHit {
                    tag: tag.to_string(),
                    category: "general".into(),
                    score: 0.9,
                    vocab_index: None,
                })
                .collect(),
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
            captured_at,
            metadata_json: String::new(),
        }
    }

    #[test]
    fn fit_scale_keeps_small_images_and_fits_large_ones() {
        assert_eq!(fit_scale((4000., 3000.), (400., 300.)), 0.1);
        assert_eq!(fit_scale((3000., 4000.), (400., 300.)), 0.075);
        assert_eq!(fit_scale((100., 50.), (400., 300.)), 1.0);
        assert_eq!(fit_scale((0., 50.), (400., 300.)), 1.0);
    }

    #[test]
    fn zoom_about_keeps_the_shared_center_and_clamps() {
        let (zoom, pan) = zoom_about((0., 0.), (40., -20.), 1.0, 2.0);
        assert_eq!(zoom, 2.0);
        assert_eq!(pan, (-40., 20.));
        let (zoom, pan) = zoom_about(pan, (40., -20.), zoom, 0.5);
        assert_eq!(zoom, 1.0);
        assert_eq!(pan, (0., 0.));
        let (zoom, _) = zoom_about((0., 0.), (0., 0.), 10.0, 4.0);
        assert_eq!(zoom, 16.0);
    }

    #[test]
    fn limited_offset_keeps_small_sides_centered() {
        let shown = zoomed_size((4000., 2000.), (400., 300.), 2.0);
        assert_eq!(shown, (800., 400.));
        assert_eq!(limited_offset((900., 40.), shown, (400., 300.)), (200., 40.));
        assert_eq!(
            limited_offset((0., 0.), (100., 50.), (400., 300.)),
            (0., 0.)
        );
    }

    #[test]
    fn metadata_rows_mark_same_and_different_fields() {
        let first = asset(
            r"C:\pics\a.png",
            4000,
            3000,
            10 * 1024 * 1024,
            4,
            "G",
            Some(1_700_000_000),
            &["long_hair", "smile"],
        );
        let mut second = asset(
            r"C:\pics\b.png",
            1920,
            1080,
            2 * 1024 * 1024,
            4,
            "G",
            None,
            &["long_hair"],
        );
        second.mtime = 1_700_000_000;
        let rows = compare_metadata_rows(&[first, second]);
        let row = |label: &str| rows.iter().find(|row| row.label == label).unwrap();
        assert!(!row("尺寸").same);
        assert!(!row("文件大小").same);
        assert!(!row("拍摄时间").same);
        assert!(!row("标签").same);
        assert!(row("星级").same);
        assert!(row("评级").same);
        assert!(row("格式").same);
        assert!(row("路径").same == false);
    }
}
