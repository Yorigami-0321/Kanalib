//! F-005 分组浏览：分组模型、吸顶组头与按行虚拟化的分组列表元素。
//!
//! 分组只重排「可见资产下标」，格内布局、行高与缩略图档位完全沿用 6 种视图的固定规格；
//! 列表元素仍只为可见行建元素（行级虚拟化），组头是独立的固定高度行，滚动时吸顶。

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Element, ElementId, GlobalElementId,
    Hitbox, InspectorElementId, InteractiveElement, Interactivity, IntoElement, ItemSize, LayoutId,
    Overflow, Pixels, ScrollStrategy, StatefulInteractiveElement, StyleRefinement, Styled,
    UniformListScrollHandle, Window, point, px, size,
};

use super::*;
use crate::app::KanalibApp;
use crate::format::format_date;
use crate::sidebar::source_label;

/// 组头高度：Windows Explorer 风格的单行标题（所有视图一致）。
pub(crate) const GROUP_HEADER_HEIGHT: f32 = 34.;

/// 一个分组：标题、成员数量与在显示行/像素空间中的起点。
#[derive(Clone, Debug)]
pub(crate) struct GroupEntry {
    /// 折叠状态使用的稳定键（日期字符串 / 来源路径 / 评级代码）。
    pub(crate) key: String,
    pub(crate) title: String,
    pub(crate) count: usize,
    /// 该组第一个资产在 `ordered` 中的位置。
    pub(crate) start_asset: usize,
    /// 组头在显示行中的下标。
    pub(crate) start_row: usize,
    /// 组头在内容坐标系中的 y 偏移。
    pub(crate) start_px: f32,
    pub(crate) collapsed: bool,
}

/// 分组后的显示布局；`ordered` 是分组重排后的可见资产下标。
#[derive(Clone, Debug)]
pub(crate) struct GroupLayout {
    pub(crate) columns: usize,
    pub(crate) row_height: f32,
    pub(crate) groups: Vec<GroupEntry>,
    pub(crate) ordered: Arc<Vec<usize>>,
    /// 含组头的显示行数。
    pub(crate) total_rows: usize,
    pub(crate) total_height: f32,
}

impl Default for GroupLayout {
    fn default() -> Self {
        Self {
            columns: 1,
            row_height: 32.,
            groups: Vec::new(),
            ordered: Arc::new(Vec::new()),
            total_rows: 0,
            total_height: 0.,
        }
    }
}

impl GroupLayout {
    pub(crate) fn is_header_row(&self, row: usize) -> bool {
        if self.groups.is_empty() {
            return false;
        }
        row == self.groups[self.group_of_row(row)].start_row
    }

    pub(crate) fn group_of_row(&self, row: usize) -> usize {
        match self.groups.binary_search_by(|group| group.start_row.cmp(&row)) {
            Ok(index) => index,
            Err(0) => 0,
            Err(index) => index - 1,
        }
    }

    fn group_of_px(&self, y: f32) -> usize {
        match self
            .groups
            .binary_search_by(|group| {
                group
                    .start_px
                    .partial_cmp(&y)
                    .unwrap_or(Ordering::Equal)
            }) {
            Ok(index) => index,
            Err(0) => 0,
            Err(index) => index - 1,
        }
    }

    pub(crate) fn row_height_at(&self, row: usize) -> f32 {
        if self.is_header_row(row) {
            GROUP_HEADER_HEIGHT
        } else {
            self.row_height
        }
    }

    /// 显示行的 y 偏移（内容坐标系）。
    pub(crate) fn row_y(&self, row: usize) -> f32 {
        if row >= self.total_rows {
            return self.total_height;
        }
        let group = &self.groups[self.group_of_row(row)];
        let local = row - group.start_row;
        if local == 0 {
            group.start_px
        } else {
            group.start_px
                + GROUP_HEADER_HEIGHT
                + (local - 1) as f32 * self.row_height
        }
    }

    /// 给定内容坐标 y 命中的显示行。
    pub(crate) fn row_at(&self, y: f32) -> usize {
        if self.groups.is_empty() {
            return 0;
        }
        let group = &self.groups[self.group_of_px(y.max(0.))];
        if y < group.start_px + GROUP_HEADER_HEIGHT {
            group.start_row
        } else {
            let local = 1
                + ((y - group.start_px - GROUP_HEADER_HEIGHT) / self.row_height.max(1.)).floor()
                    as usize;
            (group.start_row + local).min(self.total_rows.saturating_sub(1))
        }
    }

    /// 当前应吸顶的组与吸顶 y（0 表示贴住列表顶部，负值表示被下一组头顶出）。
    pub(crate) fn sticky(&self, scroll_top: f32, header_height: f32) -> Option<(usize, f32)> {
        if self.groups.is_empty() {
            return None;
        }
        let index = self.group_of_px(scroll_top);
        let group = &self.groups[index];
        if scroll_top <= group.start_px {
            // 组头本身还在列表顶部（自然位置可见），不需要吸顶副本。
            return None;
        }
        let mut y = 0.;
        if let Some(next) = self.groups.get(index + 1) {
            let next_y = next.start_px - scroll_top;
            if next_y < header_height {
                y = next_y - header_height;
            }
        }
        Some((index, y))
    }

    /// 把「内容行号」映射成应该滚到的显示行：组首行滚到组头，折叠组滚到组头。
    pub(crate) fn scroll_target_row(&self, content_row: usize) -> Option<usize> {
        if self.groups.is_empty() {
            return None;
        }
        let index = match self
            .groups
            .binary_search_by(|group| group.start_asset.cmp(&content_row))
        {
            Ok(index) => index,
            Err(0) => 0,
            Err(index) => index - 1,
        };
        let group = &self.groups[index];
        let local = content_row.saturating_sub(group.start_asset);
        if group.collapsed {
            return Some(group.start_row);
        }
        let local_row = local / self.columns.max(1);
        if local_row == 0 {
            Some(group.start_row)
        } else {
            Some((group.start_row + local_row).min(self.total_rows.saturating_sub(1)))
        }
    }

    /// 内容行 → (组下标, 组内内容行)；组头返回 `None`。
    pub(crate) fn content_row_of_display(&self, display_row: usize) -> Option<(usize, usize)> {
        if self.groups.is_empty() {
            return None;
        }
        let index = self.group_of_row(display_row);
        let group = &self.groups[index];
        if display_row == group.start_row || group.collapsed {
            None
        } else {
            Some((index, display_row - group.start_row - 1))
        }
    }
}

/// 分组键；只在重建布局时构造，避免为每张图生成字符串。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum BucketKey {
    Day(u64),
    Source(usize),
    Rating(u8),
}

struct Bucket {
    key: String,
    title: String,
    /// 日期为天数（降序），评级为优先级（升序）；来源不使用。
    order: u64,
    indices: Vec<usize>,
}

fn rating_code(rating: &str) -> u8 {
    match rating.to_ascii_lowercase().as_str() {
        "g" => 1,
        "s" => 2,
        "q" => 3,
        "e" => 4,
        _ => 5,
    }
}

fn rating_title(code: u8) -> String {
    match code {
        1 => "评级 G".to_string(),
        2 => "评级 S".to_string(),
        3 => "评级 Q".to_string(),
        4 => "评级 E".to_string(),
        _ => "未评级".to_string(),
    }
}

/// 按当前排序顺序分桶、排序组、并把组内资产顺序展开为 `ordered`。
///
/// 组内顺序保持当前排序；组间顺序：日期从新到旧、来源按名称、评级 G/S/Q/E/未评级。
pub(crate) fn build_group_layout(
    source: &[usize],
    assets: &[Asset],
    mode: GroupMode,
    columns: usize,
    row_height: f32,
    collapsed: &HashSet<String>,
    aliases: &HashMap<PathBuf, String>,
) -> GroupLayout {
    let columns = columns.max(1);
    let mut buckets: Vec<Bucket> = Vec::new();
    let mut lookup: HashMap<BucketKey, usize> = HashMap::new();
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut root_ids: HashMap<&Path, usize> = HashMap::new();

    for &index in source {
        let Some(asset) = assets.get(index) else {
            continue;
        };
        let (bucket_key, order) = match mode {
            GroupMode::None => continue,
            GroupMode::Modified => {
                let day = asset.mtime / 86_400;
                (BucketKey::Day(day), day)
            }
            GroupMode::Captured => {
                let day = asset.taken_or_modified() / 86_400;
                (BucketKey::Day(day), day)
            }
            GroupMode::Source => {
                let root = asset.library_root.as_path();
                let id = *root_ids.entry(root).or_insert_with(|| {
                    roots.push(asset.library_root.clone());
                    roots.len() - 1
                });
                (BucketKey::Source(id), 0)
            }
            GroupMode::Rating => {
                let code = rating_code(&asset.rating);
                (BucketKey::Rating(code), u64::from(code))
            }
        };
        let bucket = *lookup.entry(bucket_key).or_insert_with(|| {
            let (key, title) = match bucket_key {
                BucketKey::Day(day) => {
                    let date = format_date(day * 86_400);
                    (date.clone(), date)
                }
                BucketKey::Source(id) => {
                    let root = &roots[id];
                    (root.display().to_string(), source_label(root, aliases))
                }
                BucketKey::Rating(code) => {
                    let key = match code {
                        1 => "g".to_string(),
                        2 => "s".to_string(),
                        3 => "q".to_string(),
                        4 => "e".to_string(),
                        _ => String::new(),
                    };
                    (key, rating_title(code))
                }
            };
            buckets.push(Bucket {
                key,
                title,
                order,
                indices: Vec::new(),
            });
            buckets.len() - 1
        });
        buckets[bucket].indices.push(index);
    }

    match mode {
        GroupMode::Modified | GroupMode::Captured => {
            buckets.sort_by(|left, right| right.order.cmp(&left.order));
        }
        GroupMode::Source => buckets.sort_by(|left, right| {
            left.title
                .to_lowercase()
                .cmp(&right.title.to_lowercase())
                .then_with(|| left.key.cmp(&right.key))
        }),
        GroupMode::Rating => buckets.sort_by_key(|bucket| bucket.order),
        GroupMode::None => {}
    }

    let mut ordered = Vec::with_capacity(source.len());
    let mut groups = Vec::with_capacity(buckets.len());
    let mut start_row = 0usize;
    let mut start_px = 0f32;
    for bucket in buckets {
        let count = bucket.indices.len();
        let rows = count.div_ceil(columns);
        let is_collapsed = collapsed.contains(&bucket.key);
        groups.push(GroupEntry {
            key: bucket.key,
            title: bucket.title,
            count,
            start_asset: ordered.len(),
            start_row,
            start_px,
            collapsed: is_collapsed,
        });
        ordered.extend_from_slice(&bucket.indices);
        start_row += 1 + if is_collapsed { 0 } else { rows };
        start_px += GROUP_HEADER_HEIGHT
            + if is_collapsed {
                0.
            } else {
                rows as f32 * row_height
            };
    }
    let total_rows = start_row;
    let total_height = start_px;
    GroupLayout {
        columns,
        row_height,
        groups,
        ordered: Arc::new(ordered),
        total_rows,
        total_height,
    }
}

/// 分组视图缓存：只在数据版本（`source` 指针）、维度、列数或折叠状态变化时重建。
#[derive(Default)]
pub(crate) struct GroupCache {
    source: Arc<Vec<usize>>,
    grouped: Option<Arc<Vec<usize>>>,
    layout: Option<Arc<GroupLayout>>,
    mode: GroupMode,
    columns: usize,
    row_height: f32,
}

impl GroupCache {
    /// 折叠状态变化后强制下次渲染重建（折叠不重排可见集合，只改行数）。
    pub(crate) fn invalidate_collapse(&mut self) {
        self.layout = None;
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// 设置里的 `ui.group_by`（默认关闭）；`KANALIB_GROUP_BY` 仅用于自动化截图/压测。
pub(crate) fn initial_group_mode(state: &AppState) -> GroupMode {
    std::env::var("KANALIB_GROUP_BY")
        .ok()
        .and_then(|value| GroupMode::from_key(value.trim()))
        .or_else(|| {
            state
                .setting("ui.group_by")
                .and_then(|value| GroupMode::from_key(&value))
        })
        .unwrap_or_default()
}

impl KanalibApp {
    /// 按当前维度重建分组布局；返回本次要用于渲染/导航的可见资产顺序。
    ///
    /// 分组只做重排，`assets` 与排序结果本身不变；关闭分组时恢复分组前的顺序。
    pub(crate) fn refresh_group_view(
        &mut self,
        passed: Arc<Vec<usize>>,
        columns: usize,
        row_height: f32,
        assets: &Arc<RwLock<Vec<Asset>>>,
    ) -> Arc<Vec<usize>> {
        let passed_is_grouped = self
            .group_cache
            .grouped
            .as_ref()
            .is_some_and(|grouped| Arc::ptr_eq(grouped, &passed));
        if self.group_by == GroupMode::None {
            if passed_is_grouped {
                let source = self.group_cache.source.clone();
                self.view.visible_indices = source.clone();
                self.group_cache.reset();
                return source;
            }
            if self.group_cache.grouped.is_some() {
                self.group_cache.reset();
            }
            return passed;
        }

        let layout_stale = self.group_cache.layout.is_none()
            || self.group_cache.mode != self.group_by
            || self.group_cache.columns != columns
            || (self.group_cache.row_height - row_height).abs() > f32::EPSILON;
        if passed_is_grouped && !layout_stale {
            return passed;
        }

        let source = if passed_is_grouped {
            self.group_cache.source.clone()
        } else {
            passed
        };
        let aliases = self.state.shared.lock().unwrap().source_aliases.clone();
        let layout = {
            let assets = assets.read().unwrap();
            build_group_layout(
                &source,
                &assets,
                self.group_by,
                columns,
                row_height,
                &self.collapsed_groups,
                &aliases,
            )
        };
        let layout = Arc::new(layout);
        let ordered = layout.ordered.clone();
        self.view.visible_indices = ordered.clone();
        self.group_cache = GroupCache {
            source,
            grouped: Some(ordered.clone()),
            layout: Some(layout),
            mode: self.group_by,
            columns,
            row_height,
        };
        ordered
    }

    pub(crate) fn grouped_layout(&self) -> Option<Arc<GroupLayout>> {
        if self.group_by == GroupMode::None {
            return None;
        }
        self.group_cache.layout.clone()
    }

    pub(crate) fn set_group_mode(&mut self, mode: GroupMode, cx: &mut Context<Self>) {
        if self.screen != Screen::Library || self.group_by == mode {
            return;
        }
        self.group_by = mode;
        self.persist_setting("ui.group_by", mode.key());
        self.collapsed_groups.clear();
        self.group_cache.reset();
        self.pending_reveal_group = None;
        // 重排后原像素位置不再对应同一批图片，回到顶部最可预期。
        self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.focus_grid_pending = true;
        cx.notify();
    }

    pub(crate) fn toggle_group_collapse(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.collapsed_groups.remove(&key) {
            self.collapsed_groups.insert(key.clone());
        }
        self.group_cache.invalidate_collapse();
        self.pending_reveal_group = Some(key);
        cx.notify();
    }

    pub(crate) fn group_menu(&self, this: &Entity<Self>) -> Menu {
        let mut menu = Menu::new();
        for (index, mode) in GroupMode::ALL.into_iter().enumerate() {
            if index == 1 {
                menu = menu.separator();
            }
            let this = this.clone();
            menu = menu.item(
                MenuItem::radio(mode.label(), self.group_by == mode).on_click(move |_, cx| {
                    let _ = this.update(cx, |app, cx| app.set_group_mode(mode, cx));
                }),
            );
        }
        menu
    }
}

pub(crate) struct GroupedListFrameState {
    rows: Vec<AnyElement>,
    sticky: Option<AnyElement>,
}

pub(crate) type RowRender = Rc<dyn Fn(usize, &mut Window, &mut App) -> AnyElement>;
pub(crate) type HeaderRender = Rc<dyn Fn(usize, bool, &mut Window, &mut App) -> AnyElement>;/// 变高行（固定内容行高 + 固定组头高）的虚拟列表；滚动与 `UniformListScrollHandle` 兼容。
pub(crate) struct GroupedList {
    id: ElementId,
    layout: Arc<GroupLayout>,
    row_height: f32,
    render_row: RowRender,
    render_header: HeaderRender,
    scroll_handle: Option<UniformListScrollHandle>,
    reveal_row: Option<usize>,
    interactivity: Interactivity,
}

impl GroupedList {
    pub(crate) fn new(
        id: impl Into<ElementId>,
        layout: Arc<GroupLayout>,
        row_height: f32,
        reveal_row: Option<usize>,
        render_row: RowRender,
        render_header: HeaderRender,
    ) -> Self {
        let id = id.into();
        let mut interactivity = Interactivity::new();
        interactivity.element_id = Some(id.clone());
        interactivity.base_style.overflow.y = Some(Overflow::Scroll);
        Self {
            id,
            layout,
            row_height,
            render_row,
            render_header,
            scroll_handle: None,
            reveal_row,
            interactivity,
        }
    }

    /// 复用 `grid_scroll` 句柄：滚轮/键盘定位都通过同一份偏移与 `scroll_to_item` 请求。
    pub(crate) fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self {
        let base = handle.0.borrow().base_handle.clone();
        self = <Self as StatefulInteractiveElement>::track_scroll(self, &base);
        self.scroll_handle = Some(handle.clone());
        self
    }
}

impl Styled for GroupedList {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.interactivity.base_style
    }
}

impl InteractiveElement for GroupedList {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl StatefulInteractiveElement for GroupedList {}

impl IntoElement for GroupedList {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for GroupedList {
    type RequestLayoutState = GroupedListFrameState;
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.interactivity.source_location()
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let total_height = self.layout.total_height;
        let layout_id = self.interactivity.request_layout(
            global_id,
            inspector_id,
            window,
            cx,
            |style, window, _cx| {
                window.with_text_style(style.text_style().cloned(), |window| {
                    window.request_measured_layout(
                        style,
                        move |known_dimensions, available_space, _window, _cx| {
                            let width = known_dimensions.width.unwrap_or(match available_space.width
                            {
                                AvailableSpace::Definite(width) => width,
                                _ => px(0.),
                            });
                            let height = match available_space.height {
                                AvailableSpace::Definite(height) => height.min(px(total_height)),
                                _ => px(total_height),
                            };
                            size(width, height)
                        },
                    )
                })
            },
        );
        (
            layout_id,
            GroupedListFrameState {
                rows: Vec::new(),
                sticky: None,
            },
        )
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        frame_state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Hitbox> {
        let content_size = size(bounds.size.width, px(self.layout.total_height));
        let layout = self.layout.clone();
        let row_height = self.row_height;
        let render_row = self.render_row.clone();
        let render_header = self.render_header.clone();
        let scroll_handle = self.scroll_handle.clone();
        let reveal_row = self.reveal_row.take();
        self.interactivity.prepaint(
            global_id,
            inspector_id,
            bounds,
            content_size,
            window,
            cx,
            move |_style, scroll_offset, hitbox, window, cx| {
                let viewport_height = bounds.size.height.as_f32();
                let max_scroll = (layout.total_height - viewport_height).max(0.);
                let mut scroll_top = (-scroll_offset.y).as_f32().clamp(0., max_scroll);

                if let Some(handle) = &scroll_handle {
                    let mut state = handle.0.borrow_mut();
                    state.last_item_size = Some(ItemSize {
                        item: bounds.size,
                        contents: size(bounds.size.width, px(layout.total_height)),
                    });
                }

                let mut requested = None;
                if let Some(handle) = &scroll_handle {
                    if let Some(deferred) = handle.0.borrow_mut().deferred_scroll_to_item.take() {
                        if let Some(row) = layout.scroll_target_row(deferred.item_index) {
                            requested = Some((row, deferred.strategy));
                        }
                    }
                }
                if let Some(row) = reveal_row {
                    requested = Some((row, ScrollStrategy::Top));
                }
                if let Some((row, strategy)) = requested {
                    let row_top = layout.row_y(row);
                    let row_bottom = row_top + layout.row_height_at(row);
                    let target = match strategy {
                        ScrollStrategy::Top => row_top,
                        ScrollStrategy::Center => row_top - (viewport_height - row_height) / 2.,
                        ScrollStrategy::Bottom => row_bottom - viewport_height,
                        ScrollStrategy::Nearest => {
                            if row_top < scroll_top {
                                row_top
                            } else if row_bottom > scroll_top + viewport_height {
                                row_bottom - viewport_height
                            } else {
                                scroll_top
                            }
                        }
                    };
                    scroll_top = target.clamp(0., max_scroll);
                    if let Some(handle) = &scroll_handle {
                        handle
                            .0
                            .borrow()
                            .base_handle
                            .set_offset(point(scroll_offset.x, -px(scroll_top)));
                    }
                }

                frame_state.rows.clear();
                frame_state.sticky = None;
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let mut row = layout.row_at(scroll_top);
                    let mut y = layout.row_y(row) - scroll_top;
                    while row < layout.total_rows && y < viewport_height {
                        let height = layout.row_height_at(row);
                        let mut element = if layout.is_header_row(row) {
                            render_header(layout.group_of_row(row), false, window, cx)
                        } else {
                            render_row(row, window, cx)
                        };
                        element.layout_as_root(
                            size(
                                AvailableSpace::Definite(bounds.size.width),
                                AvailableSpace::Definite(px(height)),
                            ),
                            window,
                            cx,
                        );
                        element.prepaint_at(point(bounds.left(), bounds.top() + px(y)), window, cx);
                        frame_state.rows.push(element);
                        y += height;
                        row += 1;
                    }
                    if let Some((group, sticky_y)) =
                        layout.sticky(scroll_top, GROUP_HEADER_HEIGHT)
                    {
                        let mut element = render_header(group, true, window, cx);
                        element.layout_as_root(
                            size(
                                AvailableSpace::Definite(bounds.size.width),
                                AvailableSpace::Definite(px(GROUP_HEADER_HEIGHT)),
                            ),
                            window,
                            cx,
                        );
                        element.prepaint_at(
                            point(bounds.left(), bounds.top() + px(sticky_y)),
                            window,
                            cx,
                        );
                        frame_state.sticky = Some(element);
                    }
                });
                hitbox
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        frame_state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.interactivity.paint(
            global_id,
            inspector_id,
            bounds,
            prepaint.as_ref(),
            window,
            cx,
            |_style, window, cx| {
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    for row in &mut frame_state.rows {
                        row.paint(window, cx);
                    }
                    if let Some(sticky) = &mut frame_state.sticky {
                        sticky.paint(window, cx);
                    }
                });
            },
        );
    }
}

/// 组头：标题 + 数量 + 折叠箭头；点击整行切换折叠。
pub(crate) fn group_header(
    group: &GroupEntry,
    pinned: bool,
    this: &Entity<KanalibApp>,
    colors: UiColors,
) -> AnyElement {
    let key = group.key.clone();
    let this = this.clone();
    let collapsed = group.collapsed;
    let aria = format!("{}，{} 张", group.title, group.count);
    div()
        .id(SharedString::from(format!(
            "group-header-{}-{}",
            group.key,
            if pinned { "pinned" } else { "inline" }
        )))
        .role(Role::Button)
        .aria_label(aria)
        .w_full()
        .h(px(GROUP_HEADER_HEIGHT))
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(GRID_PADDING))
        .bg(colors.bg)
        .border_b_1()
        .border_color(colors.border)
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover))
        .on_click(move |_, _window, cx| {
            let key = key.clone();
            let _ = this.update(cx, |app, cx| {
                app.toggle_group_collapse(key, cx);
            });
        })
        .child(
            Icon::new(if collapsed {
                IconName::ChevronRight
            } else {
                IconName::ChevronDown
            })
            .size(IconSize::Xs)
            .color(colors.muted),
        )
        .child(
            div()
                .truncate()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.fg)
                .child(group.title.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(colors.muted)
                .child(format!("{} 张", group.count)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn asset(path: &str, mtime: u64, rating: &str, root: &str) -> Asset {
        Asset {
            path: PathBuf::from(path),
            library_root: PathBuf::from(root),
            thumb: None,
            tile_thumb: None,
            width: 100,
            height: 100,
            group: String::new(),
            rating: rating.to_string(),
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
            bytes: 1,
            mtime,
            captured_at: None,
            metadata_json: String::new(),
        }
    }

    fn day(date: &str) -> u64 {
        // 2026-10-01 00:00:00 UTC
        let base = 1_790_812_800u64;
        match date {
            "2026-10-01" => base,
            "2026-10-02" => base + 86_400,
            "2026-10-03" => base + 2 * 86_400,
            other => panic!("unknown date {other}"),
        }
    }

    fn visible(assets: &[Asset]) -> Vec<usize> {
        (0..assets.len()).collect()
    }

    #[test]
    fn groups_by_modified_date_newest_first() {
        let assets = vec![
            asset("a.png", day("2026-10-01") + 10, "", "/lib"),
            asset("b.png", day("2026-10-03") + 20, "", "/lib"),
            asset("c.png", day("2026-10-01") + 30, "", "/lib"),
            asset("d.png", day("2026-10-02") + 40, "", "/lib"),
        ];
        let layout = build_group_layout(
            &visible(&assets),
            &assets,
            GroupMode::Modified,
            2,
            150.,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(layout.groups.len(), 3);
        assert_eq!(layout.groups[0].title, "2026-10-03");
        assert_eq!(layout.groups[1].title, "2026-10-02");
        assert_eq!(layout.groups[2].title, "2026-10-01");
        assert_eq!(layout.groups[2].count, 2);
        // 组内保持排序顺序（a 在 c 前）。
        assert_eq!(&*layout.ordered, &[1, 3, 0, 2]);
        assert_eq!(layout.total_rows, 3 + 3);
        assert_eq!(layout.total_height, 3. * GROUP_HEADER_HEIGHT + 3. * 150.);
    }

    #[test]
    fn captured_falls_back_to_mtime() {
        let assets = vec![
            asset("a.png", day("2026-10-01"), "", "/lib"),
            asset("b.png", day("2026-10-03"), "", "/lib"),
        ];
        let layout = build_group_layout(
            &visible(&assets),
            &assets,
            GroupMode::Captured,
            1,
            32.,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(layout.groups.len(), 2);
        assert_eq!(layout.groups[0].title, "2026-10-03");
    }

    #[test]
    fn collapsing_a_group_removes_only_its_content_rows() {
        let assets = vec![
            asset("a.png", day("2026-10-01"), "", "/lib"),
            asset("b.png", day("2026-10-03"), "", "/lib"),
            asset("c.png", day("2026-10-01"), "", "/lib"),
        ];
        let expanded = build_group_layout(
            &visible(&assets),
            &assets,
            GroupMode::Modified,
            2,
            150.,
            &HashSet::new(),
            &HashMap::new(),
        );
        let mut collapsed_keys = HashSet::new();
        collapsed_keys.insert("2026-10-01".to_string());
        let collapsed = build_group_layout(
            &visible(&assets),
            &assets,
            GroupMode::Modified,
            2,
            150.,
            &collapsed_keys,
            &HashMap::new(),
        );
        assert_eq!(expanded.total_rows, 2 + 2);
        assert_eq!(collapsed.total_rows, 2 + 1);
        assert!(collapsed.groups[1].collapsed);
        assert_eq!(
            collapsed.total_height,
            expanded.total_height - 150.
        );
        // 折叠组的内容行应落到组头。
        let start = collapsed.groups[1].start_asset;
        assert_eq!(
            collapsed.scroll_target_row(start),
            Some(collapsed.groups[1].start_row)
        );
    }

    #[test]
    fn row_mapping_and_sticky_push_up() {
        let assets = vec![
            asset("a.png", day("2026-10-03") + 1, "", "/lib"),
            asset("b.png", day("2026-10-03") + 2, "", "/lib"),
            asset("c.png", day("2026-10-02") + 3, "", "/lib"),
            asset("d.png", day("2026-10-02") + 4, "", "/lib"),
        ];
        let layout = build_group_layout(
            &visible(&assets),
            &assets,
            GroupMode::Modified,
            2,
            150.,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(layout.groups[0].start_row, 0);
        assert_eq!(layout.groups[1].start_row, 2);
        assert_eq!(layout.row_y(0), 0.);
        assert_eq!(layout.row_y(1), GROUP_HEADER_HEIGHT);
        assert_eq!(
            layout.row_y(2),
            GROUP_HEADER_HEIGHT + 150.
        );
        assert_eq!(layout.row_at(0.), 0);
        assert_eq!(layout.row_at(GROUP_HEADER_HEIGHT + 1.), 1);
        assert_eq!(layout.row_at(GROUP_HEADER_HEIGHT + 150.), 2);

        // 第一组的组头滚出后吸顶。
        let sticky = layout
            .sticky(GROUP_HEADER_HEIGHT + 50., GROUP_HEADER_HEIGHT)
            .expect("sticky");
        assert_eq!(sticky.0, 0);
        assert_eq!(sticky.1, 0.);
        // 第二组组头进入顶部时把第一组推出。
        let near_next = GROUP_HEADER_HEIGHT + 150. - 10.;
        let sticky = layout
            .sticky(near_next, GROUP_HEADER_HEIGHT)
            .expect("sticky");
        assert_eq!(sticky.0, 0);
        assert!(sticky.1 < 0. && sticky.1 > -GROUP_HEADER_HEIGHT);
        // 组头还在自然位置时不画吸顶副本。
        assert!(
            layout
                .sticky(GROUP_HEADER_HEIGHT + 150., GROUP_HEADER_HEIGHT)
                .is_none()
        );
        // 第二组组头吸顶后，第一组不再吸顶。
        let sticky = layout
            .sticky(GROUP_HEADER_HEIGHT + 150. + 40., GROUP_HEADER_HEIGHT)
            .expect("sticky");
        assert_eq!(sticky.0, 1);
        assert_eq!(sticky.1, 0.);
    }

    #[test]
    fn groups_by_source_and_rating() {
        let assets = vec![
            asset("a.png", 10, "e", "/lib/alpha"),
            asset("b.png", 20, "", "/lib/beta"),
            asset("c.png", 30, "g", "/lib/alpha"),
        ];
        let mut aliases = HashMap::new();
        aliases.insert(PathBuf::from("/lib/alpha"), "主图库".to_string());
        let layout = build_group_layout(
            &visible(&assets),
            &assets,
            GroupMode::Source,
            1,
            32.,
            &HashSet::new(),
            &aliases,
        );
        assert_eq!(layout.groups.len(), 2);
        assert_eq!(layout.groups[0].title, "beta");
        assert_eq!(layout.groups[0].count, 1);
        assert_eq!(layout.groups[1].title, "主图库");
        assert_eq!(layout.groups[1].count, 2);

        let layout = build_group_layout(
            &visible(&assets),
            &assets,
            GroupMode::Rating,
            1,
            32.,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(
            layout
                .groups
                .iter()
                .map(|group| group.title.as_str())
                .collect::<Vec<_>>(),
            vec!["评级 G", "评级 E", "未评级"]
        );
    }
}
