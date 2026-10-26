use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use ely_gpui_component::{
    buttons::{Button, ButtonVariant, IconButton, SegmentedControl},
    feedback::{Toast, ToastViewport, Toaster},
    forms::{Choice, Input, Rating, TagInput, TextInput},
    menus::{ContextMenu, DropdownMenu, Menu, MenuItem},
    navigation::{Command, CommandPalette},
    overlays::{ConfirmDialog, Lightbox, PromptDialog, Slide},
    primitives::{Icon, IconName, Severity},
    shell::drag_region,
    theme::{ActiveTheme, ControlSize, IconSize, Mode, Theme},
};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    FontWeight, Hsla, ImageCache, ImageCacheError, ImageCacheItem, IntoElement, KeyBinding,
    Modifiers, MouseButton, ObjectFit, ParentElement, Pixels, Render, RenderImage, Resource, Role,
    ScrollStrategy, SharedString, Styled, Task, UniformListScrollHandle, WeakEntity, Window,
    WindowControlArea, div, image_cache, img, px, uniform_list, white,
};
#[cfg(feature = "frame-profiler")]
use hdrhistogram::Histogram;

use kanalib_core::dedup::{
    DedupSummary, DuplicateGroup, GroupKind, ReviewState, is_protected,
    similarity_pct_from_distance,
};
use kanalib_core::model::{
    Asset, Counts, Filter, SavedSearch, SearchQuery, TagHit, normalize_tag_name,
};
use kanalib_core::export;
use kanalib_core::state::{AppState, CacheStats};

const GRID_GAP: f32 = 12.;
const GRID_PADDING: f32 = 16.;
const ROW_PADDING: f32 = 12.;
const GRID_CONTEXT: &str = "KanalibGrid";
const DEDUP_RAIL_WIDTH: f32 = 272.;
const DEDUP_GROUP_ROW_HEIGHT: f32 = 78.;
const DEDUP_CARD_WIDTH: f32 = 214.;
const DEDUP_MAX_VISIBLE: usize = 4;
const SETTINGS_MAX_WIDTH: f32 = 1080.;
const TAG_ROW_HEIGHT: f32 = 54.;
const TITLEBAR_HEIGHT: f32 = 48.;

gpui::actions!(
    kanalib,
    [
        OpenPalette,
        FocusSearch,
        ClearSelection,
        SelectAllVisible,
        SelectPrev,
        SelectNext,
        SelectPrevRow,
        SelectNextRow,
        SelectFirst,
        SelectLast,
        OpenQuickView,
        ToggleFavorite,
        Rate0,
        Rate1,
        Rate2,
        Rate3,
        Rate4,
        Rate5,
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum SortMode {
    #[default]
    Name,
    Modified,
    Size,
    Resolution,
    Star,
    Favorite,
    Rating,
}

impl SortMode {
    fn label(self) -> &'static str {
        match self {
            Self::Name => "文件名",
            Self::Modified => "修改时间",
            Self::Size => "文件大小",
            Self::Resolution => "分辨率",
            Self::Star => "星级",
            Self::Favorite => "收藏优先",
            Self::Rating => "模型评级",
        }
    }

    fn default_ascending(self) -> bool {
        matches!(self, Self::Name)
    }

    fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Modified => "modified",
            Self::Size => "size",
            Self::Resolution => "resolution",
            Self::Star => "star",
            Self::Favorite => "favorite",
            Self::Rating => "rating",
        }
    }

    fn from_key(value: &str) -> Option<Self> {
        Some(match value {
            "name" => Self::Name,
            "modified" => Self::Modified,
            "size" => Self::Size,
            "resolution" => Self::Resolution,
            "star" => Self::Star,
            "favorite" => Self::Favorite,
            "rating" => Self::Rating,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum DensityPreset {
    Small,
    #[default]
    Medium,
    Large,
}

impl DensityPreset {
    fn key(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }

    fn from_key(value: &str) -> Self {
        match value {
            "small" => Self::Small,
            "large" => Self::Large,
            _ => Self::Medium,
        }
    }

    fn min_width(self) -> f32 {
        match self {
            Self::Small => 128.,
            Self::Medium => 160.,
            Self::Large => 224.,
        }
    }

    fn image_height(self) -> f32 {
        match self {
            Self::Small => 112.,
            Self::Medium => 150.,
            Self::Large => 208.,
        }
    }

    fn row_height(self) -> f32 {
        self.image_height() + 98.
    }
}

fn grid_columns(available: f32, min_width: f32) -> usize {
    let usable = (available - GRID_PADDING * 2.0 + GRID_GAP) / (min_width + GRID_GAP);
    (usable.floor() as usize).max(1)
}

fn grid_tile_width(available: f32, columns: usize) -> f32 {
    let usable = available - GRID_PADDING * 2.0 - GRID_GAP * columns.saturating_sub(1) as f32;
    (usable / columns.max(1) as f32).max(1.0)
}

fn rating_rank(rating: &str) -> u64 {
    match rating.to_ascii_lowercase().as_str() {
        "g" => 1,
        "s" => 2,
        "q" => 3,
        "e" => 4,
        _ => 0,
    }
}

fn prompt_for(asset: &Asset) -> String {
    let mut tags = asset
        .all_tags()
        .filter(|tag| {
            matches!(
                tag.category.as_str(),
                "general" | "character" | "copyright" | "manual"
            )
        })
        .collect::<Vec<_>>();
    tags.sort_by(|a, b| b.score.total_cmp(&a.score));
    tags.iter()
        .map(|tag| tag.tag.replace('_', " "))
        .collect::<Vec<_>>()
        .join(", ")
}

fn source_label(root: &Path, aliases: &HashMap<PathBuf, String>) -> String {
    aliases
        .get(root)
        .filter(|alias| !alias.trim().is_empty())
        .cloned()
        .or_else(|| {
            root.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| root.display().to_string())
}

fn lightbox_caption(asset: &Asset) -> SharedString {
    let general = asset
        .all_tags()
        .filter(|tag| matches!(tag.category.as_str(), "general" | "manual"))
        .count();
    let mut parts = vec![asset.title(), format!("{} × {}", asset.width, asset.height)];
    if asset.favorite {
        parts.push("收藏".into());
    }
    if asset.star > 0 {
        parts.push(format!("{} 星", asset.star));
    }
    parts.push(format!("{general} 个标签"));
    SharedString::from(parts.join(" · "))
}

fn push_toast(
    toaster: &Entity<Toaster>,
    cx: &mut App,
    message: impl Into<SharedString>,
    severity: Severity,
) {
    toaster.update(cx, |toaster, cx| {
        toaster.push(Toast::new(message).severity(severity), cx);
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Library,
    Duplicates,
    Tags,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DedupFilter {
    Pending,
    Ignored,
    Related,
    Done,
    All,
}

impl DedupFilter {
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "未审",
            Self::Ignored => "已忽略",
            Self::Related => "相关变体",
            Self::Done => "已保留",
            Self::All => "全部",
        }
    }

    fn matches(self, state: ReviewState) -> bool {
        match self {
            Self::Pending => state == ReviewState::Pending,
            Self::Ignored => state == ReviewState::Ignored,
            Self::Related => state == ReviewState::Related,
            Self::Done => state == ReviewState::Done,
            Self::All => true,
        }
    }
}

#[derive(Clone, Debug)]
enum DedupConfirm {
    Group(String),
    Batch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagDialogMode {
    Add,
    Remove,
}

#[derive(Clone)]
struct SourceMenuState {
    request: u64,
    root: PathBuf,
    position: gpui::Point<Pixels>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportFormat {
    Csv,
    Json,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportTarget {
    Selected,
    View,
}

impl ExportTarget {
    fn slug(self) -> &'static str {
        match self {
            Self::Selected => "selected",
            Self::View => "view",
        }
    }
}

#[derive(Clone, Copy)]
struct UiColors {
    bg: Hsla,
    surface: Hsla,
    sunken: Hsla,
    border: Hsla,
    fg: Hsla,
    muted: Hsla,
    subtle: Hsla,
    accent: Hsla,
    selection: Hsla,
    danger: Hsla,
    hover: Hsla,
    success: Hsla,
    success_subtle: Hsla,
    warning: Hsla,
    warning_subtle: Hsla,
    backdrop: Hsla,
}

impl UiColors {
    fn from_theme(cx: &App) -> Self {
        let colors = &cx.theme().colors;
        Self {
            bg: colors.bg,
            surface: colors.surface,
            sunken: colors.sunken,
            border: colors.border,
            fg: colors.fg,
            muted: colors.fg_muted,
            subtle: colors.fg_subtle,
            accent: colors.accent,
            selection: colors.selection,
            danger: colors.danger,
            hover: colors.hover,
            success: colors.success,
            success_subtle: colors.success_subtle,
            warning: colors.warning,
            warning_subtle: colors.warning_subtle,
            backdrop: colors.backdrop,
        }
    }
}

/// 网格 tile 的解码图上限。gpui 默认的全局 asset cache 只进不出：滚过多少张缩略图就常驻
/// 多少张 BGRA（512 档约 1MB/张，10k 库跑一遍就是 GB 级），这里换成按 LRU 限量的缓存。
const IMAGE_CACHE_LIMIT: usize = 128;

fn clamp_cache_limit(limit: usize) -> usize {
    limit.max(1)
}

/// 返回需要逐出的 key：按最近使用排序取最旧的，直到长度回到上限，且不动 `keep`。
fn cache_victims<T>(
    entries: &HashMap<Resource, (T, u64)>,
    limit: usize,
    keep: &Resource,
) -> Vec<Resource> {
    if entries.len() <= limit {
        return Vec::new();
    }
    let mut ordered = entries
        .iter()
        .filter(|(key, _)| *key != keep)
        .map(|(key, (_, used))| (*used, key.clone()))
        .collect::<Vec<_>>();
    ordered.sort_by_key(|(used, _)| *used);
    let overflow = entries.len() - limit;
    ordered
        .into_iter()
        .take(overflow)
        .map(|(_, key)| key)
        .collect()
}

/// Path 资源的解码由本缓存自己负责（绕开 gpui 的 CachedLoad/ImageCacheItem）；其余资源仍走 gpui。
enum ThumbEntry {
    Gpui(ImageCacheItem),
    Loading,
    Loaded(Result<Arc<RenderImage>, ImageCacheError>),
}

/// 有界的解码图缓存：可见 tile 每帧都会 touch，所以只会被更近的挤掉，不会被自己挤掉。
pub struct ThumbCache {
    entries: HashMap<Resource, (ThumbEntry, u64)>,
    clock: u64,
    limit: usize,
    /// 取证档：按 Arc 指针登记每个交出的解码图，dump 时统计仍存活的 RenderImage 数。
    #[cfg(feature = "frame-profiler")]
    watch: HashMap<usize, std::sync::Weak<RenderImage>>,
    entity: WeakEntity<ThumbCache>,
}

impl ThumbCache {
    fn with_limit(limit: usize, cx: &mut Context<Self>) -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
            limit: clamp_cache_limit(limit),
            #[cfg(feature = "frame-profiler")]
            watch: HashMap::new(),
            entity: cx.entity().downgrade(),
        }
    }

    fn spawn_decode(&self, resource: Resource, cx: &mut App) {
        let key = resource.clone();
        let task = cx
            .background_executor()
            .spawn(async move { decode_thumb(&resource) });
        let weak = self.entity.clone();
        cx.spawn(async move |cx| {
            let result = task.await;
            let _ = weak.update(cx, |cache, cx| {
                if let Some((entry, _)) = cache.entries.get_mut(&key) {
                    *entry = ThumbEntry::Loaded(result);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 缓存里已解码位图的字节数（BGRA，逐帧累计），用来区分"缓存装太多"和"上色板涨"。
    #[cfg(feature = "frame-profiler")]
    fn cached_bytes(&self) -> u64 {
        self.entries
            .values()
            .filter_map(|(entry, _)| {
                let image: Arc<RenderImage> = match entry {
                    ThumbEntry::Loaded(Ok(image)) => image.clone(),
                    ThumbEntry::Gpui(item) => item.get()?.ok()?,
                    _ => return None,
                };
                let size = image.size(0);
                let frames = image.frame_count().max(1) as u64;
                Some(size.width.0 as u64 * size.height.0 as u64 * 4 * frames)
            })
            .sum()
    }

    #[cfg(feature = "frame-profiler")]
    fn watch_stats(&self) -> (usize, usize) {
        (
            self.watch.len(),
            self.watch
                .values()
                .filter(|weak| weak.strong_count() > 0)
                .count(),
        )
    }
}

/// 与 gpui 的 ImageAssetLoader 对齐：解码为 BGRA 单帧。
fn decode_thumb(resource: &Resource) -> Result<Arc<RenderImage>, ImageCacheError> {
    let Resource::Path(path) = resource else {
        return Err(ImageCacheError::Asset(
            "thumb cache only decodes file resources".into(),
        ));
    };
    let bytes =
        std::fs::read(path.as_ref()).map_err(|error| ImageCacheError::Io(Arc::new(error)))?;
    let image =
        image::load_from_memory(&bytes).map_err(|error| ImageCacheError::Image(Arc::new(error)))?;
    let mut buffer = image.into_rgba8();
    for pixel in buffer.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

#[cfg(feature = "frame-profiler")]
fn watch_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KANALIB_WATCH_IMAGES").is_some())
}

impl ImageCache for ThumbCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        self.clock += 1;
        let clock = self.clock;
        if let Some((entry, used)) = self.entries.get_mut(resource) {
            *used = clock;
            if let ThumbEntry::Gpui(item) = entry {
                let _ = item.use_image(window);
            }
        } else {
            match resource {
                Resource::Path(_) => {
                    self.entries
                        .insert(resource.clone(), (ThumbEntry::Loading, clock));
                    self.spawn_decode(resource.clone(), cx);
                }
                _ => {
                    let item = ImageCacheItem::new(resource, cx);
                    self.entries
                        .insert(resource.clone(), (ThumbEntry::Gpui(item), clock));
                }
            }
        }
        let result = match self.entries.get(resource) {
            Some((ThumbEntry::Loaded(result), _)) => Some(result.clone()),
            Some((ThumbEntry::Gpui(item), _)) => item.use_image(window),
            _ => None,
        };
        for victim in cache_victims(&self.entries, self.limit, resource) {
            // 解码中的条目下一轮再逐出，避免取消后台解码任务。
            let evictable = self
                .entries
                .get(&victim)
                .is_some_and(|(entry, _)| !matches!(entry, ThumbEntry::Loading));
            if !evictable {
                continue;
            }
            if let Some((entry, _)) = self.entries.remove(&victim) {
                match entry {
                    ThumbEntry::Loaded(Ok(image)) => cx.drop_image(image, Some(window)),
                    ThumbEntry::Gpui(item) => {
                        if let Some(Ok(image)) = item.get() {
                            cx.drop_image(image, Some(window));
                        }
                    }
                    _ => {}
                }
            }
        }
        #[cfg(feature = "frame-profiler")]
        if watch_enabled()
            && let Some(Ok(image)) = &result
        {
            let ptr = std::sync::Arc::as_ptr(image) as usize;
            let weak = std::sync::Arc::downgrade(image);
            match self.watch.get_mut(&ptr) {
                // 地址被新 Arc 复用时，旧 Weak 已死则替换，否则保持原对象。
                Some(existing) if existing.strong_count() == 0 => *existing = weak,
                Some(_) => {}
                None => {
                    self.watch.insert(ptr, weak);
                }
            }
        }
        result
    }
}

#[derive(Default)]
struct ViewCache {
    counts_revision: u64,
    counts: Counts,
    search_revision: u64,
    visible_revision: u64,
    visible_filter: Option<Filter>,
    visible_search: String,
    visible_search_revision: u64,
    visible_sort: SortMode,
    visible_sort_ascending: bool,
    visible_indices: Arc<Vec<usize>>,
}

/// 合成器帧计时探针。`KANALIB_FRAME_METRICS=<秒>` 开启：导入结束后每帧请求一次动画帧，
/// 到时把 gpui profiler 的直方图汇总成一行 JSON。
#[cfg(feature = "frame-profiler")]
struct FrameProbe {
    duration: Duration,
    started_at: Instant,
    /// 至少观察到一次 scanning=true，避免在 start_scan 置位前的首帧就起跑。
    armed: bool,
    baseline_presents: Option<u64>,
    renders: usize,
}

pub struct KanalibApp {
    state: Arc<AppState>,
    grid_scroll: UniformListScrollHandle,
    dedup_scroll: UniformListScrollHandle,
    tag_scroll: UniformListScrollHandle,
    tag_list: Vec<kanalib_core::model::TagSummary>,
    cache_stats: Option<CacheStats>,
    last_search: String,
    search_paths: Option<Arc<HashSet<PathBuf>>>,
    last_scanning: bool,
    view: ViewCache,
    grid_rows: usize,
    grid_columns: usize,
    screen: Screen,
    dedup_mode: GroupKind,
    dedup_filter: DedupFilter,
    dedup_selected: Option<String>,
    dedup_show_all: bool,
    dedup_confirm: Option<DedupConfirm>,
    dedup_notice: Option<String>,
    settings_notice: Option<String>,
    refresh_task: Option<Task<()>>,
    thumb_cache: Entity<ThumbCache>,
    scroll_task: Option<Task<()>>,
    similar_task: Option<Task<()>>,
    grid_focus: FocusHandle,
    focus_grid_pending: bool,
    auto_select_pending: bool,
    selection: Arc<HashSet<PathBuf>>,
    selection_anchor: Option<PathBuf>,
    sort: SortMode,
    sort_ascending: bool,
    density: DensityPreset,
    search_input: Option<Entity<TextInput>>,
    palette_open: bool,
    lightbox_slides: Vec<(PathBuf, SharedString)>,
    lightbox_at: Option<usize>,
    lightbox_viewed: HashSet<PathBuf>,
    save_search_open: bool,
    tag_dialog: Option<TagDialogMode>,
    tag_dialog_epoch: u64,
    tag_dialog_paths: Vec<PathBuf>,
    source_remove_confirm: Option<PathBuf>,
    source_rename: Option<PathBuf>,
    source_rename_epoch: u64,
    source_menu: Option<SourceMenuState>,
    source_menu_epoch: u64,
    toaster: Entity<Toaster>,
    theme_dark: bool,
    #[cfg(feature = "frame-profiler")]
    frame_probe: Option<FrameProbe>,
    #[cfg(feature = "frame-profiler")]
    probe_watch: Option<Task<()>>,
}

impl KanalibApp {
    pub fn new(folder: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let open_dedup = std::env::var("KANALIB_OPEN_DEDUP").ok();
        let open_settings = std::env::var_os("KANALIB_OPEN_SETTINGS").is_some();
        let screen = if open_settings {
            Screen::Settings
        } else if open_dedup.is_some() {
            Screen::Duplicates
        } else {
            Screen::Library
        };
        let dedup_mode = if open_dedup.as_deref() == Some("similar") {
            GroupKind::Similar
        } else {
            GroupKind::Exact
        };
        #[cfg(feature = "frame-profiler")]
        let frame_probe = std::env::var("KANALIB_FRAME_METRICS").ok().map(|value| {
            let seconds = value.trim().parse::<f64>().unwrap_or(6.).clamp(1., 600.);
            FrameProbe {
                duration: Duration::from_secs_f64(seconds),
                started_at: Instant::now(),
                armed: false,
                baseline_presents: None,
                renders: 0,
            }
        });
        #[cfg(not(feature = "frame-profiler"))]
        if std::env::var_os("KANALIB_FRAME_METRICS").is_some() {
            log::warn!("帧计时需要 --features frame-profiler 构建的二进制");
        }
        cx.bind_keys([
            KeyBinding::new("ctrl-k", OpenPalette, None),
            KeyBinding::new("ctrl-shift-p", OpenPalette, None),
            KeyBinding::new("ctrl-f", FocusSearch, None),
            KeyBinding::new("up", SelectPrevRow, Some(GRID_CONTEXT)),
            KeyBinding::new("down", SelectNextRow, Some(GRID_CONTEXT)),
            KeyBinding::new("left", SelectPrev, Some(GRID_CONTEXT)),
            KeyBinding::new("right", SelectNext, Some(GRID_CONTEXT)),
            KeyBinding::new("home", SelectFirst, Some(GRID_CONTEXT)),
            KeyBinding::new("end", SelectLast, Some(GRID_CONTEXT)),
            KeyBinding::new("enter", OpenQuickView, Some(GRID_CONTEXT)),
            KeyBinding::new("space", OpenQuickView, Some(GRID_CONTEXT)),
            KeyBinding::new("f", ToggleFavorite, Some(GRID_CONTEXT)),
            KeyBinding::new("0", Rate0, Some(GRID_CONTEXT)),
            KeyBinding::new("1", Rate1, Some(GRID_CONTEXT)),
            KeyBinding::new("2", Rate2, Some(GRID_CONTEXT)),
            KeyBinding::new("3", Rate3, Some(GRID_CONTEXT)),
            KeyBinding::new("4", Rate4, Some(GRID_CONTEXT)),
            KeyBinding::new("5", Rate5, Some(GRID_CONTEXT)),
            KeyBinding::new("ctrl-a", SelectAllVisible, Some(GRID_CONTEXT)),
            KeyBinding::new("escape", ClearSelection, Some(GRID_CONTEXT)),
        ]);
        let state = AppState::new(folder);
        let initial_selected = state.shared.lock().unwrap().selected.clone();
        let sort = state
            .setting("ui.sort")
            .and_then(|value| SortMode::from_key(&value))
            .unwrap_or_default();
        let sort_ascending = state
            .setting("ui.sort_ascending")
            .map(|value| value == "true")
            .unwrap_or_else(|| sort.default_ascending());
        let density = state
            .setting("ui.density")
            .map(|value| DensityPreset::from_key(&value))
            .unwrap_or_default();
        let theme_dark = state
            .setting("ui.theme")
            .map(|value| value == "dark")
            .unwrap_or_else(|| cx.theme().is_dark());
        if theme_dark != cx.theme().is_dark() {
            Theme::set_mode_now(
                if theme_dark { Mode::Dark } else { Mode::Light },
                cx,
            );
        }
        Self {
            state,
            grid_scroll: UniformListScrollHandle::new(),
            dedup_scroll: UniformListScrollHandle::new(),
            tag_scroll: UniformListScrollHandle::new(),
            tag_list: Vec::new(),
            cache_stats: None,
            last_search: String::new(),
            search_paths: None,
            last_scanning: false,
            view: ViewCache::default(),
            grid_rows: 0,
            grid_columns: 1,
            screen,
            dedup_mode,
            dedup_filter: DedupFilter::Pending,
            dedup_selected: None,
            dedup_show_all: false,
            dedup_confirm: None,
            dedup_notice: None,
            settings_notice: None,
            refresh_task: None,
            thumb_cache: cx.new(|cx| {
                ThumbCache::with_limit(
                    std::env::var("KANALIB_IMAGE_CACHE")
                        .ok()
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(IMAGE_CACHE_LIMIT),
                    cx,
                )
            }),
            scroll_task: None,
            similar_task: None,
            grid_focus: cx.focus_handle(),
            focus_grid_pending: true,
            auto_select_pending: false,
            selection: Arc::new(initial_selected.iter().cloned().collect()),
            selection_anchor: initial_selected,
            sort,
            sort_ascending,
            density,
            search_input: None,
            palette_open: false,
            lightbox_slides: Vec::new(),
            lightbox_at: None,
            lightbox_viewed: HashSet::new(),
            save_search_open: false,
            tag_dialog: None,
            tag_dialog_epoch: 0,
            tag_dialog_paths: Vec::new(),
            source_remove_confirm: None,
            source_rename: None,
            source_rename_epoch: 0,
            source_menu: None,
            source_menu_epoch: 0,
            toaster: cx.new(|_| Toaster::default()),
            theme_dark,
            #[cfg(feature = "frame-profiler")]
            frame_probe,
            #[cfg(feature = "frame-profiler")]
            probe_watch: None,
        }
    }

    pub fn start(&mut self, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        self.refresh_task = Some(cx.spawn(async move |_, cx| {
            let mut last_status = String::new();
            let mut last_scanning = false;
            let mut last_revision = 0u64;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let snapshot = weak.update(cx, |app, _| {
                    let shared = app.state.shared.lock().unwrap();
                    (shared.status.clone(), shared.scanning, shared.revision)
                });
                let Ok((status, scanning, revision)) = snapshot else {
                    break;
                };
                if status != last_status || scanning != last_scanning || revision != last_revision {
                    last_status = status;
                    last_scanning = scanning;
                    last_revision = revision;
                    if weak.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }
        }));
        if std::env::var_os("KANALIB_UI_SCROLL_TEST").is_some() {
            let weak = cx.entity().downgrade();
            self.scroll_task = Some(cx.spawn(async move |_, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(200))
                        .await;
                    let ready = weak.update(cx, |app, _| {
                        let scanning = app.state.shared.lock().unwrap().scanning;
                        !scanning && app.grid_rows > 0
                    });
                    match ready {
                        Ok(true) => break,
                        Ok(false) => {}
                        Err(_) => return,
                    }
                }
                let stride = std::env::var("KANALIB_UI_SCROLL_STRIDE")
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(32);
                log::info!("ui scroll test start: stride={stride}");
                let steps = std::env::var("KANALIB_UI_SCROLL_STEPS")
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(400);
                for step in 0..steps {
                    cx.background_executor()
                        .timer(Duration::from_millis(33))
                        .await;
                    let result = weak.update(cx, |app, cx| {
                        if app.grid_rows == 0 {
                            return;
                        }
                        let target = (step * stride) % app.grid_rows;
                        app.grid_scroll.scroll_to_item(target, ScrollStrategy::Top);
                        cx.notify();
                    });
                    if result.is_err() {
                        break;
                    }
                }
                log::info!("ui scroll test end");
                #[cfg(feature = "frame-profiler")]
                let _ = weak.update(cx, |app, cx| {
                    let (handed_out, alive) = {
                        let cache = app.thumb_cache.read(cx);
                        cache.watch_stats()
                    };
                    log::info!("image watch: handed_out={handed_out} alive_at_scroll_end={alive}");
                    #[cfg(feature = "frame-profiler")]
                    kanalib_core::alloc_stats::traces::dump();
                    if watch_enabled() {
                        let paths: Vec<gpui::Resource> = {
                            let shared = app.state.shared.lock().unwrap();
                            shared
                                .assets
                                .read()
                                .unwrap()
                                .iter()
                                .map(|asset| gpui::Resource::from(asset.path.clone()))
                                .collect()
                        };
                        let total = paths.len();
                        for resource in &paths {
                            cx.remove_asset::<gpui::ImgResourceLoader>(resource);
                        }
                        log::info!("global image store purge: paths={total}");
                    }
                });
            }));
        }
        #[cfg(feature = "frame-profiler")]
        if self.frame_probe.is_some() {
            let weak = cx.entity().downgrade();
            self.probe_watch = Some(cx.spawn(async move |_, cx| {
                for tick in 0..90u32 {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let renders = weak
                        .update(cx, |app, _| {
                            app.frame_probe
                                .as_ref()
                                .map_or(-1i64, |probe| probe.renders as i64)
                        })
                        .unwrap_or(-2);
                    log::info!("frame probe heartbeat: tick={tick} renders={renders}");
                }
            }));
        }
        self.state.start_scan();
        if std::env::var_os("KANALIB_SIMILAR_SCAN").is_some()
            && self.screen == Screen::Duplicates
            && self.dedup_mode == GroupKind::Similar
        {
            let weak = cx.entity().downgrade();
            self.similar_task = Some(cx.spawn(async move |_, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(200))
                        .await;
                    let ready = weak.update(cx, |app, _| {
                        let shared = app.state.shared.lock().unwrap();
                        let has_assets = shared.assets.read().unwrap().len() >= 2;
                        (
                            !shared.scanning && has_assets,
                            !shared.dedup.similar_scanned && !shared.dedup.similar_scanning,
                        )
                    });
                    match ready {
                        Ok((true, true)) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.state.start_similar_scan();
                                cx.notify();
                            });
                            break;
                        }
                        Ok((true, false)) | Err(_) => break,
                        Ok((false, _)) => {}
                    }
                }
            }));
        }
        self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        // 同步 rfd 会开嵌套消息循环，GPUI 的 RefCell 在鼠标回调里仍被借用，导致闪退。
        cx.spawn(async move |_, cx| {
            let Some(handles) = rfd::AsyncFileDialog::new()
                .set_title("添加图库文件夹")
                .pick_folders()
                .await
            else {
                return;
            };
            let folders = handles
                .into_iter()
                .map(|handle| handle.path().to_path_buf())
                .collect::<Vec<_>>();
            if folders.is_empty() {
                return;
            }
            let _ = weak.update(cx, |app, cx| {
                match app.state.add_roots(folders) {
                    Ok(0) => {
                        push_toast(
                            &app.toaster,
                            cx,
                            "所选文件夹已在图库中",
                            Severity::Warning,
                        );
                        return;
                    }
                    Ok(added) => push_toast(
                        &app.toaster,
                        cx,
                        format!("已添加 {added} 个来源"),
                        Severity::Success,
                    ),
                    Err(error) => {
                        app.state.set_error(format!("添加文件夹失败：{error:#}"));
                        push_toast(
                            &app.toaster,
                            cx,
                            format!("添加文件夹失败：{error:#}"),
                            Severity::Danger,
                        );
                        return;
                    }
                }
                if let Some(search) = &app.search_input {
                    search.update(cx, |input, cx| input.set_text("", cx));
                }
                app.state.set_filter(Filter::All);
                app.state.start_scan();
                app.last_search.clear();
                app.search_paths = None;
                app.screen = Screen::Library;
                app.dedup_selected = None;
                app.dedup_confirm = None;
                app.source_remove_confirm = None;
                app.clear_multi_selection();
                app.focus_grid_pending = true;
                app.auto_select_pending = true;
                app.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            });
        })
        .detach();
    }

    fn rescan(&mut self, cx: &mut Context<Self>) {
        self.state.start_scan();
        self.last_search.clear();
        self.search_paths = None;
        self.screen = Screen::Library;
        self.dedup_selected = None;
        self.dedup_confirm = None;
        self.clear_multi_selection();
        self.focus_grid_pending = true;
        self.auto_select_pending = true;
        self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn retry_failed(&mut self, cx: &mut Context<Self>) {
        match self.state.retry_failed() {
            Ok(count) => {
                log::info!("retry failed assets: {count}");
                self.state.set_filter(Filter::All);
                self.state.start_scan();
                self.screen = Screen::Library;
                self.clear_multi_selection();
                self.auto_select_pending = true;
                self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
            Err(error) => {
                log::warn!("retry failed: {error:#}");
                self.state.set_error(format!("重试失败: {error:#}"));
            }
        }
        cx.notify();
    }

    fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.screen = Screen::Library;
        self.state.set_filter(filter);
        self.clear_multi_selection();
        self.focus_grid_pending = true;
        self.auto_select_pending = true;
        self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn open_dedup_mode(&mut self, mode: GroupKind, cx: &mut Context<Self>) {
        self.screen = Screen::Duplicates;
        self.dedup_mode = mode;
        self.dedup_filter = DedupFilter::Pending;
        self.dedup_selected = None;
        self.dedup_confirm = None;
        self.dedup_show_all = false;
        self.dedup_notice = None;
        self.dedup_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::Settings;
        self.dedup_confirm = None;
        self.settings_notice = None;
        self.refresh_cache_stats(cx);
    }

    fn refresh_cache_stats(&mut self, cx: &mut Context<Self>) {
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

    fn cleanup_cache(&mut self, cx: &mut Context<Self>) {
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

    fn remove_source(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        match self.state.remove_root(&root) {
            Ok(removed) => {
                self.source_remove_confirm = None;
                self.clear_multi_selection();
                self.auto_select_pending = true;
                push_toast(
                    &self.toaster,
                    cx,
                    format!("已移除来源，磁盘文件未删除（{removed} 张资产移出视图）"),
                    Severity::Success,
                );
            }
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("移除来源失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    fn open_source_rename(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.source_rename = Some(root);
        self.source_rename_epoch = self.source_rename_epoch.wrapping_add(1);
        cx.notify();
    }

    fn apply_source_rename(&mut self, alias: &str, cx: &mut Context<Self>) {
        let Some(root) = self.source_rename.clone() else {
            return;
        };
        match self.state.set_source_alias(&root, alias) {
            Ok(()) => {
                self.source_rename = None;
                push_toast(
                    &self.toaster,
                    cx,
                    if alias.trim().is_empty() {
                        "已恢复来源默认名称".to_string()
                    } else {
                        format!("已将来源重命名为 {}", alias.trim())
                    },
                    Severity::Success,
                );
            }
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("重命名来源失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    fn open_tag_manager(&mut self, cx: &mut Context<Self>) {
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

    fn refresh_tag_manager(&mut self, cx: &mut Context<Self>) {
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

    fn set_tag_blocked(&mut self, name: String, blocked: bool, cx: &mut Context<Self>) {
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

    fn advance_dedup(&mut self, groups: &[DuplicateGroup], forward: bool) {
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

    fn visible_paths(&self) -> Vec<PathBuf> {
        let shared = self.state.shared.lock().unwrap();
        let assets = shared.assets.read().unwrap();
        self.view
            .visible_indices
            .iter()
            .filter_map(|&index| assets.get(index).map(|asset| asset.path.clone()))
            .collect()
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        if !self.selection.is_empty() {
            return self.selection.iter().cloned().collect();
        }
        self.state
            .shared
            .lock()
            .unwrap()
            .selected
            .iter()
            .cloned()
            .collect()
    }

    fn range_between(&self, anchor: Option<&Path>, path: &Path) -> Option<Vec<PathBuf>> {
        let shared = self.state.shared.lock().unwrap();
        let assets = shared.assets.read().unwrap();
        let anchor_at = anchor.and_then(|anchor| {
            self.view.visible_indices.iter().position(|&index| {
                assets
                    .get(index)
                    .is_some_and(|asset| asset.path.as_path() == anchor)
            })
        })?;
        let target_at = self.view.visible_indices.iter().position(|&index| {
            assets
                .get(index)
                .is_some_and(|asset| asset.path.as_path() == path)
        })?;
        let (start, end) = if anchor_at <= target_at {
            (anchor_at, target_at)
        } else {
            (target_at, anchor_at)
        };
        Some(
            self.view.visible_indices[start..=end]
                .iter()
                .filter_map(|&index| assets.get(index).map(|asset| asset.path.clone()))
                .collect(),
        )
    }

    fn handle_tile_click(
        &mut self,
        path: PathBuf,
        modifiers: Modifiers,
        click_count: usize,
        cx: &mut Context<Self>,
    ) {
        let primary = self.state.shared.lock().unwrap().selected.clone();
        if modifiers.shift {
            let anchor = self.selection_anchor.clone().or(primary.clone());
            if let Some(paths) = self.range_between(anchor.as_deref(), &path) {
                self.selection = Arc::new(paths.into_iter().collect());
            } else {
                self.selection = Arc::new(HashSet::from([path.clone()]));
            }
            self.state.select(Some(path.clone()));
        } else if modifiers.secondary() {
            let mut selection = (*self.selection).clone();
            if selection.remove(&path) {
                if primary.as_ref() == Some(&path) {
                    self.state.select(selection.iter().next().cloned());
                }
            } else {
                selection.insert(path.clone());
                self.state.select(Some(path.clone()));
            }
            self.selection = Arc::new(selection);
            self.selection_anchor = Some(path.clone());
        } else {
            self.selection = Arc::new(HashSet::from([path.clone()]));
            self.selection_anchor = Some(path.clone());
            self.state.select(Some(path.clone()));
        }
        if click_count >= 2 {
            self.open_lightbox_at(&path);
        }
        cx.notify();
    }

    fn select_position(&mut self, position: usize, cx: &mut Context<Self>) {
        let path = {
            let shared = self.state.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            let Some(&index) = self.view.visible_indices.get(position) else {
                return;
            };
            let Some(asset) = assets.get(index) else {
                return;
            };
            asset.path.clone()
        };
        self.selection = Arc::new(HashSet::from([path.clone()]));
        self.selection_anchor = Some(path.clone());
        self.state.select(Some(path));
        let row = position / self.grid_columns.max(1);
        self.grid_scroll
            .scroll_to_item(row, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn navigate(&mut self, delta: isize, cx: &mut Context<Self>) {
        let (len, current) = {
            let shared = self.state.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            let current = shared.selected.as_ref().and_then(|selected| {
                self.view.visible_indices.iter().position(|&index| {
                    assets
                        .get(index)
                        .is_some_and(|asset| &asset.path == selected)
                })
            });
            (self.view.visible_indices.len(), current)
        };
        if len == 0 {
            return;
        }
        let target = match current {
            Some(position) if delta < 0 => position.saturating_sub((-delta) as usize),
            Some(position) => (position + delta as usize).min(len - 1),
            None => 0,
        };
        self.select_position(target, cx);
    }

    fn select_edge(&mut self, last: bool, cx: &mut Context<Self>) {
        let len = self.view.visible_indices.len();
        if len > 0 {
            self.select_position(if last { len - 1 } else { 0 }, cx);
        }
    }

    fn select_all_visible(&mut self, cx: &mut Context<Self>) {
        let paths = self.visible_paths();
        if paths.is_empty() {
            return;
        }
        self.selection = Arc::new(paths.iter().cloned().collect());
        self.selection_anchor = paths.first().cloned();
        let primary = self.state.shared.lock().unwrap().selected.clone();
        let primary_visible = primary.as_ref().is_some_and(|path| paths.contains(path));
        if !primary_visible {
            self.state.select(paths.first().cloned());
        }
        cx.notify();
    }

    fn select_single_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.selection = Arc::new(HashSet::from([path.clone()]));
        self.selection_anchor = Some(path.clone());
        self.state.select(Some(path));
        cx.notify();
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selection = Arc::new(HashSet::new());
        self.selection_anchor = None;
        self.auto_select_pending = false;
        self.state.select(None);
        cx.notify();
    }

    fn clear_multi_selection(&mut self) {
        self.selection = Arc::new(HashSet::new());
        self.selection_anchor = None;
    }

    fn paths_all_favorite(&self, paths: &[PathBuf]) -> bool {
        if paths.is_empty() {
            return false;
        }
        let targets = paths.iter().map(PathBuf::as_path).collect::<HashSet<_>>();
        let shared = self.state.shared.lock().unwrap();
        let assets = shared.assets.read().unwrap();
        let mut found = 0;
        let mut favorite = 0;
        for asset in assets.iter() {
            if targets.contains(asset.path.as_path()) {
                found += 1;
                if asset.favorite {
                    favorite += 1;
                }
            }
        }
        found == targets.len() && favorite == found
    }

    fn apply_favorite(&mut self, favorite: bool, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            push_toast(&self.toaster, cx, "先选择图片", Severity::Warning);
            return;
        }
        match self.state.set_favorite(&paths, favorite) {
            Ok(updated) => push_toast(
                &self.toaster,
                cx,
                if favorite {
                    format!("已收藏 {updated} 张")
                } else {
                    format!("已取消收藏 {updated} 张")
                },
                Severity::Success,
            ),
            Err(error) => {
                self.state.set_error(format!("收藏失败：{error:#}"));
                push_toast(
                    &self.toaster,
                    cx,
                    format!("收藏失败：{error:#}"),
                    Severity::Danger,
                );
            }
        }
        let leaving_favorites =
            !favorite && self.state.shared.lock().unwrap().filter == Filter::Favorites;
        if leaving_favorites {
            self.selection = Arc::new(HashSet::new());
            self.selection_anchor = None;
            self.state.select(None);
            self.auto_select_pending = true;
            cx.notify();
        } else {
            cx.notify();
        }
    }

    fn toggle_favorite(&mut self, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        let favorite = !self.paths_all_favorite(&paths);
        self.apply_favorite(favorite, cx);
    }

    fn apply_star(&mut self, star: u8, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            push_toast(&self.toaster, cx, "先选择图片", Severity::Warning);
            return;
        }
        match self.state.set_star(&paths, star) {
            Ok(updated) => push_toast(
                &self.toaster,
                cx,
                if star == 0 {
                    format!("已清除 {updated} 张的星级")
                } else {
                    format!("已设置 {updated} 张为 {star} 星")
                },
                Severity::Success,
            ),
            Err(error) => {
                self.state.set_error(format!("设置星级失败：{error:#}"));
                push_toast(
                    &self.toaster,
                    cx,
                    format!("设置星级失败：{error:#}"),
                    Severity::Danger,
                );
            }
        }
        cx.notify();
    }

    fn open_tag_dialog(&mut self, mode: TagDialogMode, cx: &mut Context<Self>) {
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

    fn apply_tag_dialog(&mut self, name: &str, cx: &mut Context<Self>) {
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

    fn export_assets(
        &mut self,
        target: ExportTarget,
        export_format: ExportFormat,
        cx: &mut Context<Self>,
    ) {
        let mut paths = match target {
            ExportTarget::Selected => self.selected_paths(),
            ExportTarget::View => self.visible_paths(),
        };
        if target == ExportTarget::Selected {
            paths.sort();
            paths.dedup();
        }
        if paths.is_empty() {
            push_toast(&self.toaster, cx, "没有可导出的图片", Severity::Warning);
            return;
        }
        let assets = {
            let shared = self.state.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            let by_path = assets
                .iter()
                .map(|asset| (asset.path.as_path(), asset))
                .collect::<HashMap<_, _>>();
            paths
                .iter()
                .filter_map(|path| by_path.get(path.as_path()).map(|asset| (*asset).clone()))
                .collect::<Vec<_>>()
        };
        if assets.is_empty() {
            push_toast(&self.toaster, cx, "没有可导出的图片", Severity::Warning);
            return;
        }
        let content = match export_format {
            ExportFormat::Csv => export::to_csv(&assets),
            ExportFormat::Json => match export::to_json(&assets) {
                Ok(content) => content,
                Err(error) => {
                    push_toast(
                        &self.toaster,
                        cx,
                        format!("生成 JSON 失败：{error:#}"),
                        Severity::Danger,
                    );
                    return;
                }
            },
        };
        let filename = format!(
            "kanalib-{}.{}",
            target.slug(),
            export_format.extension()
        );
        let count = assets.len();
        let bytes = content.len() as u64;
        let weak = cx.entity().downgrade();
        let toaster = self.toaster.clone();
        cx.spawn(async move |_, cx| {
            let Some(handle) = rfd::AsyncFileDialog::new()
                .set_file_name(&filename)
                .save_file()
                .await
            else {
                return;
            };
            let path = handle.path().to_path_buf();
            let result = std::fs::write(&path, content);
            let _ = weak.update(cx, |_, cx| {
                match result {
                    Ok(()) => push_toast(
                        &toaster,
                        cx,
                        format!("已导出 {count} 张 · {}", format_bytes(bytes)),
                        Severity::Success,
                    ),
                    Err(error) => push_toast(
                        &toaster,
                        cx,
                        format!("导出失败：{error:#}"),
                        Severity::Danger,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn copy_selected_prompt(&mut self, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            push_toast(&self.toaster, cx, "先选择图片", Severity::Warning);
            return;
        }
        let targets = paths.iter().map(PathBuf::as_path).collect::<HashSet<_>>();
        let prompts = {
            let shared = self.state.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            let mut prompts = Vec::new();
            for asset in assets.iter() {
                if targets.contains(asset.path.as_path()) {
                    let prompt = prompt_for(asset);
                    if !prompt.is_empty() {
                        prompts.push(prompt);
                    }
                }
            }
            prompts
        };
        if prompts.is_empty() {
            push_toast(
                &self.toaster,
                cx,
                "所选图片没有可复制的标签",
                Severity::Warning,
            );
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(prompts.join("\n\n")));
        push_toast(
            &self.toaster,
            cx,
            format!("已复制 {} 张图片的 Prompt", prompts.len()),
            Severity::Success,
        );
    }

    fn copy_selected_paths(&mut self, cx: &mut Context<Self>) {
        let mut paths = self.selected_paths();
        if paths.is_empty() {
            push_toast(&self.toaster, cx, "先选择图片", Severity::Warning);
            return;
        }
        paths.sort();
        let text = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        push_toast(
            &self.toaster,
            cx,
            format!("已复制 {} 条路径", paths.len()),
            Severity::Success,
        );
    }

    fn open_lightbox_at(&mut self, path: &Path) {
        let (slides, at) = {
            let shared = self.state.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            let mut slides = Vec::new();
            let mut at = None;
            for &index in self.view.visible_indices.iter() {
                let Some(asset) = assets.get(index) else {
                    continue;
                };
                if asset.path.as_path() == path {
                    at = Some(slides.len());
                }
                slides.push((asset.path.clone(), lightbox_caption(asset)));
            }
            if at.is_none() {
                if let Some(asset) = assets.iter().find(|asset| asset.path.as_path() == path) {
                    slides = vec![(asset.path.clone(), lightbox_caption(asset))];
                    at = Some(0);
                }
            }
            (slides, at)
        };
        if let Some(at) = at {
            self.palette_open = false;
            self.lightbox_viewed.clear();
            if let Some((path, _)) = slides.get(at) {
                self.lightbox_viewed.insert(path.clone());
            }
            self.lightbox_slides = slides;
            self.lightbox_at = Some(at);
        }
    }

    fn open_quick_view(&mut self, cx: &mut Context<Self>) {
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
            Some(path) => {
                self.open_lightbox_at(&path);
                cx.notify();
            }
            None => push_toast(&self.toaster, cx, "没有可查看的图片", Severity::Warning),
        }
    }

    fn take_lightbox_cleanup(&mut self) -> Vec<PathBuf> {
        if let Some(at) = self.lightbox_at {
            if let Some((path, _)) = self.lightbox_slides.get(at) {
                self.lightbox_viewed.insert(path.clone());
            }
        }
        self.lightbox_at = None;
        self.lightbox_slides.clear();
        self.lightbox_viewed.drain().collect()
    }

    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = &self.search_input {
            let handle = search.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    fn persist_setting(&self, key: &str, value: impl AsRef<str>) {
        if let Err(error) = self.state.set_setting(key, value.as_ref()) {
            log::warn!("save setting {key}: {error:#}");
        }
    }

    fn choose_sort_mode(&mut self, mode: SortMode, cx: &mut Context<Self>) {
        self.sort = mode;
        self.sort_ascending = mode.default_ascending();
        self.persist_setting("ui.sort", mode.key());
        self.persist_setting("ui.sort_ascending", self.sort_ascending.to_string());
        self.focus_grid_pending = true;
        cx.notify();
    }

    fn set_sort_ascending(&mut self, ascending: bool, cx: &mut Context<Self>) {
        self.sort_ascending = ascending;
        self.persist_setting("ui.sort_ascending", ascending.to_string());
        self.focus_grid_pending = true;
        cx.notify();
    }

    fn set_density(&mut self, density: DensityPreset, cx: &mut Context<Self>) {
        self.density = density;
        self.persist_setting("ui.density", density.key());
        self.focus_grid_pending = true;
        cx.notify();
    }

    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.theme_dark = !self.theme_dark;
        Theme::set_mode_now(
            if self.theme_dark {
                Mode::Dark
            } else {
                Mode::Light
            },
            cx,
        );
        self.persist_setting("ui.theme", if self.theme_dark { "dark" } else { "light" });
        cx.notify();
    }

    fn apply_saved_search(
        &mut self,
        saved: &SavedSearch,
        search: &Entity<TextInput>,
        cx: &mut Context<Self>,
    ) {
        search.update(cx, |input, cx| input.set_text(saved.query.clone(), cx));
        self.state.set_filter(saved.filter.clone());
        self.screen = Screen::Library;
        self.dedup_confirm = None;
        self.clear_multi_selection();
        self.focus_grid_pending = true;
        self.auto_select_pending = true;
        self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn delete_saved_search(&mut self, id: i64, cx: &mut Context<Self>) {
        match self.state.delete_saved_search(id) {
            Ok(()) => push_toast(&self.toaster, cx, "已删除智能集合", Severity::Success),
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("删除智能集合失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    fn confirm_save_search(&mut self, name: &str, cx: &mut Context<Self>) {
        let filter = self.state.shared.lock().unwrap().filter.clone();
        let query = self.last_search.clone();
        match self.state.save_saved_search(name, &filter, &query) {
            Ok(saved) => {
                self.save_search_open = false;
                push_toast(
                    &self.toaster,
                    cx,
                    format!("已保存智能集合「{}」", saved.name),
                    Severity::Success,
                );
            }
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("保存失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    fn run_palette_command(&mut self, command: &str, cx: &mut Context<Self>) {
        self.palette_open = false;
        match command {
            "goto:all" => self.set_filter(Filter::All, cx),
            "goto:untagged" => self.set_filter(Filter::Untagged, cx),
            "goto:failed" => self.set_filter(Filter::Failed, cx),
            "goto:damaged" => self.set_filter(Filter::Damaged, cx),
            "goto:fav" => self.set_filter(Filter::Favorites, cx),
            "goto:dup" => self.open_dedup_mode(GroupKind::Exact, cx),
            "goto:similar" => self.open_dedup_mode(GroupKind::Similar, cx),
            "goto:tags" => self.open_tag_manager(cx),
            "goto:settings" => self.open_settings(cx),
            "view:quick" => self.open_quick_view(cx),
            "view:select-all" => self.select_all_visible(cx),
            "view:clear" => self.clear_selection(cx),
            "view:save" => {
                self.save_search_open = true;
                cx.notify();
            }
            "view:theme" => self.toggle_theme(cx),
            "view:small" => self.set_density(DensityPreset::Small, cx),
            "view:medium" => self.set_density(DensityPreset::Medium, cx),
            "view:large" => self.set_density(DensityPreset::Large, cx),
            "view:folder" => self.choose_folder(cx),
            "scan:rescan" => self.rescan(cx),
            "scan:retry" => self.retry_failed(cx),
            "act:fav" => self.apply_favorite(true, cx),
            "act:unfav" => self.apply_favorite(false, cx),
            "act:toggle-fav" => self.toggle_favorite(cx),
            "act:star0" => self.apply_star(0, cx),
            "act:star1" => self.apply_star(1, cx),
            "act:star2" => self.apply_star(2, cx),
            "act:star3" => self.apply_star(3, cx),
            "act:star4" => self.apply_star(4, cx),
            "act:star5" => self.apply_star(5, cx),
            "act:copy-prompt" => self.copy_selected_prompt(cx),
            "act:copy-paths" => self.copy_selected_paths(cx),
            "tag:add" => self.open_tag_dialog(TagDialogMode::Add, cx),
            "tag:remove" => self.open_tag_dialog(TagDialogMode::Remove, cx),
            "export:selected-csv" => {
                self.export_assets(ExportTarget::Selected, ExportFormat::Csv, cx)
            }
            "export:selected-json" => {
                self.export_assets(ExportTarget::Selected, ExportFormat::Json, cx)
            }
            "export:view-csv" => self.export_assets(ExportTarget::View, ExportFormat::Csv, cx),
            "export:view-json" => self.export_assets(ExportTarget::View, ExportFormat::Json, cx),
            "sort:name" => self.choose_sort_mode(SortMode::Name, cx),
            "sort:modified" => self.choose_sort_mode(SortMode::Modified, cx),
            "sort:size" => self.choose_sort_mode(SortMode::Size, cx),
            "sort:resolution" => self.choose_sort_mode(SortMode::Resolution, cx),
            "sort:star" => self.choose_sort_mode(SortMode::Star, cx),
            "sort:fav" => self.choose_sort_mode(SortMode::Favorite, cx),
            "sort:rating" => self.choose_sort_mode(SortMode::Rating, cx),
            "sort:toggle" => self.set_sort_ascending(!self.sort_ascending, cx),
            _ => {}
        }
        cx.notify();
    }
}

impl Render for KanalibApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_metrics = std::env::var_os("KANALIB_UI_METRICS").is_some();
        let render_started = Instant::now();
        let colors = UiColors::from_theme(cx);
        let this = cx.entity();
        let search = window.use_keyed_state("kanalib-search", cx, |window, cx| {
            TextInput::new(window, cx).placeholder("搜索文件名 / 标签 / is:fav / star:>=4")
        });
        if self.search_input.is_none() {
            self.search_input = Some(search.clone());
        }
        let search_text = search.read(cx).text().to_string();
        let search_query = SearchQuery::parse(&search_text);
        // gpui-pre 的 viewport_size 自 0.3.x 起就是逻辑像素，不能再按 DPI 除一次。
        let scale = window.scale_factor();
        let viewport = window.viewport_size();
        let window_width = viewport.width;
        let sidebar_width = px(245.);
        let inspector_width = px(310.);
        let mut main_width = window_width - sidebar_width - inspector_width;
        if main_width < px(360.) {
            main_width = px(360.);
        }
        log::debug!(
            "layout viewport={}x{} scale={} main={}",
            viewport.width.as_f32(),
            viewport.height.as_f32(),
            scale,
            main_width.as_f32()
        );
        let counts_started = Instant::now();
        let (
            roots,
            status,
            error,
            scanning,
            device,
            queue_pending,
            queue_running,
            queue_error,
            filter,
            mut selected,
            assets,
            revision,
            counts_view,
            dedup_exact_groups,
            dedup_exact_summary,
            dedup_similar_groups,
            dedup_similar_summary,
            similar_scanned,
            similar_scanning,
            similar_threshold,
            protected_paths,
            saved_searches,
            blocked_tags,
            source_aliases,
        ) = {
            let shared = self.state.shared.lock().unwrap();
            let revision = shared.revision;
            let counts_view = (revision != self.view.counts_revision).then(|| shared.counts.view());
            (
                shared.roots.clone(),
                shared.status.clone(),
                shared.error.clone(),
                shared.scanning,
                shared.sidecar_device.clone(),
                shared.queue_pending,
                shared.queue_running,
                shared.queue_error,
                shared.filter.clone(),
                shared.selected.clone(),
                shared.assets.clone(),
                revision,
                counts_view,
                shared.dedup.groups.clone(),
                shared.dedup.summary(GroupKind::Exact),
                shared.dedup.similar_groups.clone(),
                shared.dedup.summary(GroupKind::Similar),
                shared.dedup.similar_scanned,
                shared.dedup.similar_scanning,
                shared.dedup.similar_threshold,
                shared.dedup.protected_paths.clone(),
                shared.saved_searches.clone(),
                shared.blocked_tags.clone(),
                shared.source_aliases.clone(),
            )
        };
        if let Some(counts) = counts_view {
            let elapsed_us = counts_started.elapsed().as_secs_f64() * 1_000_000.0;
            if ui_metrics {
                log::info!(
                    "ui counts view: us={elapsed_us:.0} total={} revision={revision}",
                    counts.total
                );
            }
            self.view.counts = counts;
            self.view.counts_revision = revision;
        }
        let counts = self.view.counts.clone();
        let query_changed = search_text != self.last_search;
        let scan_changed = scanning != self.last_scanning;
        self.last_scanning = scanning;
        if query_changed {
            self.last_search = search_text.clone();
        }
        if query_changed || scan_changed {
            let free_text = search_query.free_text();
            self.search_paths = if free_text.is_empty() {
                None
            } else {
                match self.state.search_paths(&free_text) {
                    Ok(paths) => Some(Arc::new(paths)),
                    Err(error) => {
                        log::warn!("search: {error:#}");
                        None
                    }
                }
            };
            self.view.search_revision = self.view.search_revision.wrapping_add(1);
        }
        if query_changed {
            self.clear_multi_selection();
            self.auto_select_pending = true;
            self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        let needs_visible = revision != self.view.visible_revision
            || self.view.visible_filter.as_ref() != Some(&filter)
            || self.view.visible_search != search_text
            || self.view.visible_search_revision != self.view.search_revision
            || self.view.visible_sort != self.sort
            || self.view.visible_sort_ascending != self.sort_ascending;
        if needs_visible {
            let started = Instant::now();
            let duplicate_paths = dedup_exact_groups
                .iter()
                .chain(dedup_similar_groups.iter())
                .flat_map(|group| group.assets.iter().map(|asset| asset.path.clone()))
                .collect::<HashSet<PathBuf>>();
            let indices = {
                let assets = assets.read().unwrap();
                let mut indices = assets
                    .iter()
                    .enumerate()
                    .filter(|(_, asset)| {
                        filter.matches(asset)
                            && search_query.matches(
                                asset,
                                self.search_paths.as_deref(),
                                Some(&duplicate_paths),
                            )
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                match self.sort {
                    SortMode::Name => {
                        indices.sort_by_cached_key(|&index| {
                            let asset = &assets[index];
                            (asset.title().to_lowercase(), asset.path.clone())
                        });
                        if !self.sort_ascending {
                            indices.reverse();
                        }
                    }
                    mode => {
                        indices.sort_by_cached_key(|&index| {
                            let asset = &assets[index];
                            let primary = match mode {
                                SortMode::Modified => asset.mtime,
                                SortMode::Size => asset.bytes,
                                SortMode::Resolution => asset.width as u64 * asset.height as u64,
                                SortMode::Star => asset.star as u64,
                                SortMode::Favorite => asset.favorite as u64,
                                SortMode::Rating => rating_rank(&asset.rating),
                                SortMode::Name => unreachable!(),
                            };
                            let primary = if self.sort_ascending {
                                primary
                            } else {
                                u64::MAX - primary
                            };
                            (primary, asset.title().to_lowercase(), asset.path.clone())
                        });
                    }
                }
                indices
            };
            let elapsed_us = started.elapsed().as_secs_f64() * 1_000_000.0;
            self.view.visible_indices = Arc::new(indices);
            self.view.visible_revision = revision;
            self.view.visible_filter = Some(filter.clone());
            self.view.visible_search = search_text;
            self.view.visible_search_revision = self.view.search_revision;
            self.view.visible_sort = self.sort;
            self.view.visible_sort_ascending = self.sort_ascending;
            if ui_metrics {
                log::info!(
                    "ui visible rebuild: us={elapsed_us:.0} visible={} revision={revision} sort={:?}",
                    self.view.visible_indices.len(),
                    self.sort
                );
            }
        }
        let visible_indices = self.view.visible_indices.clone();
        let visible_count = visible_indices.len();
        if self.auto_select_pending {
            self.auto_select_pending = false;
            let selected_visible = selected.as_ref().is_some_and(|path| {
                let assets = assets.read().unwrap();
                visible_indices
                    .iter()
                    .any(|&index| assets.get(index).is_some_and(|asset| &asset.path == path))
            });
            if !selected_visible {
                let first = {
                    let assets = assets.read().unwrap();
                    visible_indices
                        .first()
                        .and_then(|&index| assets.get(index).map(|asset| asset.path.clone()))
                };
                self.state.select(first.clone());
                selected = first.clone();
                if let Some(path) = first {
                    if self.selection.is_empty() {
                        self.selection = Arc::new(HashSet::from([path.clone()]));
                        self.selection_anchor = Some(path);
                    }
                } else {
                    self.selection = Arc::new(HashSet::new());
                    self.selection_anchor = None;
                }
            }
        }
        #[cfg(feature = "frame-profiler")]
        {
            let mut finished = None;
            if let Some(probe) = self.frame_probe.as_mut() {
                if scanning {
                    probe.armed = true;
                } else if probe.armed && !visible_indices.is_empty() {
                    if probe.baseline_presents.is_none() {
                        let baseline = window
                            .frame_duration_snapshot()
                            .present_interval_histogram
                            .len();
                        probe.baseline_presents = Some(baseline);
                        probe.started_at = Instant::now();
                        log::info!(
                            "frame probe start: secs={} assets={} baseline_presents={baseline} viewport={}x{} scale={}",
                            probe.duration.as_secs_f64(),
                            visible_indices.len(),
                            viewport.width.as_f32() as i32,
                            viewport.height.as_f32() as i32,
                            scale
                        );
                    }
                    probe.renders += 1;
                    // 每帧重新登记 next-frame 回调：gpui 只在窗口活动且持续有动画需求时
                    // 记录 present-to-present 间隔，否则直方图不累积。
                    window.request_animation_frame();
                    if Instant::now() >= probe.started_at + probe.duration {
                        finished = Some((
                            probe.renders,
                            probe.baseline_presents.unwrap_or_default(),
                            probe.started_at.elapsed(),
                        ));
                    }
                }
            }
            if let Some((renders, baseline, elapsed)) = finished {
                let (cached_images, cached_bytes) = {
                    let cache = self.thumb_cache.read(cx);
                    (cache.entries.len(), cache.cached_bytes())
                };
                #[cfg(feature = "frame-profiler")]
                {
                    let (handed_out, alive) = {
                        let cache = self.thumb_cache.read(cx);
                        cache.watch_stats()
                    };
                    log::info!("image watch: handed_out={handed_out} alive_at_probe_end={alive}");
                }
                report_frame_probe(
                    window,
                    renders,
                    baseline,
                    elapsed,
                    cached_images,
                    cached_bytes,
                );
                self.frame_probe = None;
            }
        }
        let columns = grid_columns(main_width.as_f32(), self.density.min_width());
        self.grid_rows = visible_indices.len().div_ceil(columns);
        self.grid_columns = columns;
        let selected_asset = {
            let assets = assets.read().unwrap();
            selected.as_ref().and_then(|path| {
                visible_indices
                    .iter()
                    .find(|&&index| &assets[index].path == path)
                    .map(|&index| assets[index].clone())
            })
        };

        let filter_title = match &filter {
            Filter::Source(root) => source_label(root, &source_aliases),
            _ => filter.label(),
        };
        let sidebar = self.render_sidebar(
            &this,
            &roots,
            &source_aliases,
            &status,
            error.as_deref(),
            scanning,
            device.as_deref(),
            queue_pending,
            queue_running,
            queue_error,
            &filter,
            &counts,
            self.screen,
            self.dedup_mode,
            dedup_exact_summary.pending,
            dedup_similar_summary.pending,
            similar_scanned,
            &saved_searches,
            &blocked_tags,
            &search,
            colors,
        );
        let content: AnyElement = if self.screen == Screen::Tags {
            let tags_width = (window_width - sidebar_width).max(px(640.));
            self.render_tags(
                &this,
                window,
                cx,
                tags_width,
                &blocked_tags,
                colors,
            )
            .into_any_element()
        } else if self.screen == Screen::Settings {
            let settings_width = (window_width - sidebar_width).max(px(640.));
            self.render_settings(
                &this,
                &roots,
                &source_aliases,
                &protected_paths,
                similar_threshold,
                queue_pending,
                queue_running,
                queue_error,
                &status,
                error.as_deref(),
                settings_width,
                colors,
            )
            .into_any_element()
        } else if self.screen == Screen::Duplicates {
            let dedup_width = (window_width - sidebar_width).max(px(640.));
            self.render_dedup(
                &this,
                dedup_exact_groups,
                dedup_exact_summary,
                dedup_similar_groups,
                dedup_similar_summary,
                similar_scanned,
                similar_scanning,
                similar_threshold,
                &protected_paths,
                &status,
                error.as_deref(),
                scanning,
                dedup_width,
                colors,
            )
            .into_any_element()
        } else {
            self.render_main(
                &this,
                assets,
                visible_indices,
                self.selection.clone(),
                roots.is_empty(),
                error.as_deref(),
                scanning,
                main_width,
                colors,
            )
            .into_any_element()
        };
        let inspector = (self.screen == Screen::Library).then(|| {
            self.render_inspector(
                &this,
                selected_asset.as_ref(),
                &counts,
                &blocked_tags,
                &source_aliases,
                inspector_width,
                colors,
            )
            .into_any_element()
        });

        let palette = self.palette_open.then(|| {
            let close = this.clone();
            let run = this.clone();
            let dark = self.theme_dark;
            CommandPalette::new("kanalib-palette", move |_, cx| {
                let _ = close.update(cx, |app, cx| {
                    app.palette_open = false;
                    cx.notify();
                });
            })
            .group(
                "跳转",
                [
                    Command::new("goto:all", "全部图片").icon(IconName::Image),
                    Command::new("goto:untagged", "未打标").icon(IconName::Sparkles),
                    Command::new("goto:fav", "收藏").icon(IconName::Heart),
                    Command::new("goto:failed", "失败").icon(IconName::ImageOff),
                    Command::new("goto:damaged", "损坏的图片").icon(IconName::ImageOff),
                    Command::new("goto:dup", "重复项").icon(IconName::Copy),
                    Command::new("goto:similar", "相似图片").icon(IconName::LayoutGrid),
                    Command::new("goto:tags", "标签管理").icon(IconName::Tag),
                    Command::new("goto:settings", "设置").icon(IconName::SlidersHorizontal),
                ],
            )
            .group(
                "视图",
                [
                    Command::new("view:quick", "快速查看")
                        .icon(IconName::Maximize2)
                        .keys("enter"),
                    Command::new("view:select-all", "全选可见")
                        .icon(IconName::SquareCheck)
                        .keys("ctrl-a"),
                    Command::new("view:clear", "清除选择")
                        .icon(IconName::X)
                        .keys("escape"),
                    Command::new("view:save", "保存当前视图为智能集合").icon(IconName::Plus),
                    Command::new("view:small", "缩略图 · 小").icon(IconName::LayoutGrid),
                    Command::new("view:medium", "缩略图 · 中").icon(IconName::LayoutGrid),
                    Command::new("view:large", "缩略图 · 大").icon(IconName::LayoutGrid),
                    Command::new(
                        "view:theme",
                        if dark {
                            "切换到浅色主题"
                        } else {
                            "切换到深色主题"
                        },
                    )
                    .icon(if dark { IconName::Sun } else { IconName::Moon }),
                ],
            )
            .group(
                "图片操作",
                [
                    Command::new("act:toggle-fav", "切换收藏")
                        .icon(IconName::Heart)
                        .keys("f"),
                    Command::new("act:star1", "设为 1 星").icon(IconName::Star),
                    Command::new("act:star2", "设为 2 星").icon(IconName::Star),
                    Command::new("act:star3", "设为 3 星").icon(IconName::Star),
                    Command::new("act:star4", "设为 4 星").icon(IconName::Star),
                    Command::new("act:star5", "设为 5 星").icon(IconName::Star),
                    Command::new("act:star0", "清除星级").icon(IconName::Star),
                    Command::new("act:copy-prompt", "复制选中 Prompt")
                        .icon(IconName::ClipboardCheck),
                    Command::new("act:copy-paths", "复制选中路径").icon(IconName::Copy),
                    Command::new("tag:add", "添加标签…").icon(IconName::Tag),
                    Command::new("tag:remove", "移除标签…").icon(IconName::X),
                ],
            )
            .group(
                "导出",
                [
                    Command::new("export:selected-csv", "导出选中 · CSV")
                        .icon(IconName::Download),
                    Command::new("export:selected-json", "导出选中 · JSON")
                        .icon(IconName::Download),
                    Command::new("export:view-csv", "导出当前视图 · CSV")
                        .icon(IconName::Download),
                    Command::new("export:view-json", "导出当前视图 · JSON")
                        .icon(IconName::Download),
                ],
            )
            .group(
                "排序",
                [
                    Command::new("sort:name", "按文件名排序").icon(IconName::ArrowDownUp),
                    Command::new("sort:modified", "按修改时间排序").icon(IconName::Clock),
                    Command::new("sort:size", "按文件大小排序").icon(IconName::FileImage),
                    Command::new("sort:resolution", "按分辨率排序").icon(IconName::Maximize2),
                    Command::new("sort:star", "按星级排序").icon(IconName::Star),
                    Command::new("sort:fav", "收藏优先").icon(IconName::Heart),
                    Command::new("sort:rating", "按模型评级排序").icon(IconName::Sparkles),
                    Command::new("sort:toggle", "切换排序方向").icon(IconName::ArrowDownUp),
                ],
            )
            .group(
                "图库",
                [
                    Command::new("scan:rescan", "重新扫描").icon(IconName::RefreshCw),
                    Command::new("scan:retry", "重试失败的打标").icon(IconName::WandSparkles),
                    Command::new("view:folder", "添加文件夹").icon(IconName::FolderOpen),
                ],
            )
            .on_run(move |value, _, cx| {
                let _ = run.update(cx, |app, cx| app.run_palette_command(value, cx));
            })
            .into_any_element()
        });
        let lightbox = self.lightbox_at.and_then(|at| {
            if self.lightbox_slides.is_empty() {
                return None;
            }
            let slides = self
                .lightbox_slides
                .iter()
                .map(|(path, caption)| Slide::new(path.clone(), caption.clone()))
                .collect::<Vec<_>>();
            let close = this.downgrade();
            let step = this.downgrade();
            Some(
                Lightbox::new("kanalib-lightbox", slides, at, move |_, cx| {
                    let paths = close
                        .update(cx, |app, cx| {
                            let paths = app.take_lightbox_cleanup();
                            app.focus_grid_pending = true;
                            cx.notify();
                            paths
                        })
                        .unwrap_or_default();
                    for path in paths {
                        cx.remove_asset::<gpui::ImgResourceLoader>(&Resource::from(path));
                    }
                })
                .on_step(move |to, _, cx| {
                    let previous = step
                        .update(cx, |app, cx| {
                            let previous = app.lightbox_at.and_then(|at| {
                                app.lightbox_slides.get(at).map(|(path, _)| path.clone())
                            });
                            app.lightbox_at = Some(to);
                            if let Some((path, _)) = app.lightbox_slides.get(to) {
                                let path = path.clone();
                                app.lightbox_viewed.insert(path.clone());
                                app.selection = Arc::new(HashSet::from([path.clone()]));
                                app.selection_anchor = Some(path.clone());
                                app.state.select(Some(path));
                            }
                            cx.notify();
                            previous
                        })
                        .ok()
                        .flatten();
                    if let Some(previous) = previous {
                        cx.remove_asset::<gpui::ImgResourceLoader>(&Resource::from(previous));
                    }
                })
                .into_any_element(),
            )
        });
        let save_dialog = self.save_search_open.then(|| {
            let field = window.use_keyed_state("kanalib-save-search", cx, |window, cx| {
                TextInput::new(window, cx).placeholder("智能集合名称")
            });
            let close = this.clone();
            let submit = this.clone();
            let filter_label = filter.label();
            PromptDialog::new(
                "kanalib-save-search",
                "保存为智能集合",
                &field,
                move |_, cx| {
                    let _ = close.update(cx, |app, cx| {
                        app.save_search_open = false;
                        cx.notify();
                    });
                },
            )
            .label(format!("当前筛选：{filter_label}；搜索词会一起保存"))
            .submit("保存")
            .check(|text| {
                if text.trim().is_empty() {
                    Err("名称不能为空".into())
                } else {
                    Ok(())
                }
            })
            .on_submit(move |text, _, cx| {
                let _ = submit.update(cx, |app, cx| app.confirm_save_search(text, cx));
            })
            .into_any_element()
        });
        let tag_dialog = self.tag_dialog.map(|mode| {
            let epoch = self.tag_dialog_epoch;
            let field = window.use_keyed_state(
                SharedString::from(format!("kanalib-tag-field-{epoch}")),
                cx,
                |window, cx| TextInput::new(window, cx).placeholder("标签名"),
            );
            let close = this.clone();
            let submit = this.clone();
            let count = self.tag_dialog_paths.len();
            let title = if mode == TagDialogMode::Add {
                "添加标签"
            } else {
                "移除标签"
            };
            let label = if mode == TagDialogMode::Add {
                format!("作用于 {count} 张图片；空格会转为下划线")
            } else {
                format!("作用于 {count} 张图片；自动与手工标签都会移除")
            };
            PromptDialog::new(
                SharedString::from(format!("kanalib-tag-dialog-{epoch}")),
                title,
                &field,
                move |_, cx| {
                    let _ = close.update(cx, |app, cx| {
                        app.tag_dialog = None;
                        app.tag_dialog_paths.clear();
                        cx.notify();
                    });
                },
            )
            .label(label)
            .submit(if mode == TagDialogMode::Add {
                "添加"
            } else {
                "移除"
            })
            .check(|text| {
                if normalize_tag_name(text).is_empty() {
                    Err("标签名不能为空".into())
                } else {
                    Ok(())
                }
            })
            .on_submit(move |text, _, cx| {
                let _ = submit.update(cx, |app, cx| app.apply_tag_dialog(text, cx));
            })
            .into_any_element()
        });
        let remove_source_dialog = self.source_remove_confirm.clone().map(|root| {
            let close = this.clone();
            let confirm = this.clone();
            ConfirmDialog::new(
                "remove-source-dialog",
                "移除来源？",
                format!(
                    "只会从图库移除，磁盘文件不会删除：\n{}",
                    root.display()
                ),
                move |_, cx| {
                    let _ = close.update(cx, |app, cx| {
                        app.source_remove_confirm = None;
                        cx.notify();
                    });
                },
            )
            .confirm("移除来源")
            .destructive()
            .on_confirm(move |_, cx| {
                let _ = confirm.update(cx, |app, cx| app.remove_source(root.clone(), cx));
            })
            .into_any_element()
        });
        let source_rename_dialog = self.source_rename.clone().map(|root| {
            let epoch = self.source_rename_epoch;
            let alias = source_aliases.get(&root).cloned().unwrap_or_default();
            let placeholder = source_label(&root, &source_aliases);
            let field_holder = window.use_keyed_state(
                SharedString::from(format!("source-rename-field-{epoch}")),
                cx,
                move |window, cx| {
                    let input =
                        cx.new(|cx| TextInput::new(window, cx).placeholder(placeholder));
                    input.update(cx, |input, cx| input.set_text(alias, cx));
                    input
                },
            );
            let field = field_holder.read(cx).clone();
            let close = this.clone();
            let submit = this.clone();
            PromptDialog::new(
                SharedString::from(format!("source-rename-dialog-{epoch}")),
                "重命名来源",
                &field,
                move |_, cx| {
                    let _ = close.update(cx, |app, cx| {
                        app.source_rename = None;
                        cx.notify();
                    });
                },
            )
            .label("只改图库内显示名，不改磁盘文件夹；留空恢复文件夹名")
            .submit("保存")
            .on_submit(move |text, _, cx| {
                let _ = submit.update(cx, |app, cx| app.apply_source_rename(text, cx));
            })
            .into_any_element()
        });
        let source_context_menu = self.source_menu.as_ref().map(|state| {
            ContextMenu::new(
                "source-context-global",
                source_menu(&state.root, this.clone()),
            )
            .manual(Some((state.request, state.position)))
            .child(div().size(px(0.)))
            .into_any_element()
        });
        let titlebar = self.render_titlebar(
            &this,
            window,
            cx,
            &filter,
            &filter_title,
            visible_count,
            &search,
            scanning,
            colors,
        );
        let shell = div()
            .flex()
            .flex_col()
            .relative()
            .w_full()
            .h_full()
            .bg(colors.bg)
            .text_color(colors.fg)
            .font_family(cx.theme().font_family.clone())
            .child(titlebar)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .flex()
                    .flex_row()
                    .child(sidebar)
                    .child(content)
                    .children(inspector),
            )
            .children(palette)
            .children(lightbox)
            .children(save_dialog)
            .children(tag_dialog)
            .children(remove_source_dialog)
            .children(source_rename_dialog)
            .children(source_context_menu)
            .child(ToastViewport::new("kanalib-toasts", &self.toaster))
            .on_action(cx.listener(|this, _: &OpenPalette, _, cx| {
                let cleanup = if this.palette_open {
                    Vec::new()
                } else {
                    this.take_lightbox_cleanup()
                };
                this.palette_open = !this.palette_open;
                for path in cleanup {
                    cx.remove_asset::<gpui::ImgResourceLoader>(&Resource::from(path));
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.focus_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.navigate(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.navigate(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevRow, _, cx| {
                this.navigate(-(this.grid_columns as isize), cx);
            }))
            .on_action(cx.listener(|this, _: &SelectNextRow, _, cx| {
                this.navigate(this.grid_columns as isize, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectFirst, _, cx| this.select_edge(false, cx)))
            .on_action(cx.listener(|this, _: &SelectLast, _, cx| this.select_edge(true, cx)))
            .on_action(cx.listener(|this, _: &SelectAllVisible, _, cx| {
                this.select_all_visible(cx);
            }))
            .on_action(cx.listener(|this, _: &ClearSelection, _, cx| this.clear_selection(cx)))
            .on_action(cx.listener(|this, _: &OpenQuickView, _, cx| this.open_quick_view(cx)))
            .on_action(cx.listener(|this, _: &ToggleFavorite, _, cx| this.toggle_favorite(cx)))
            .on_action(cx.listener(|this, _: &Rate0, _, cx| this.apply_star(0, cx)))
            .on_action(cx.listener(|this, _: &Rate1, _, cx| this.apply_star(1, cx)))
            .on_action(cx.listener(|this, _: &Rate2, _, cx| this.apply_star(2, cx)))
            .on_action(cx.listener(|this, _: &Rate3, _, cx| this.apply_star(3, cx)))
            .on_action(cx.listener(|this, _: &Rate4, _, cx| this.apply_star(4, cx)))
            .on_action(cx.listener(|this, _: &Rate5, _, cx| this.apply_star(5, cx)));
        if self.focus_grid_pending
            && self.screen == Screen::Library
            && !self.palette_open
            && self.lightbox_at.is_none()
            && !self.save_search_open
            && self.tag_dialog.is_none()
            && self.source_remove_confirm.is_none()
            && self.source_rename.is_none()
        {
            window.focus(&self.grid_focus, cx);
            self.focus_grid_pending = false;
        }
        if ui_metrics {
            log::info!(
                "ui render build: ms={:.2}",
                render_started.elapsed().as_secs_f64() * 1000.0
            );
        }
        // 整棵树共用一个有界图片缓存：子树里所有 img 都走它，不再进无上限的全局 asset cache。
        image_cache(self.thumb_cache.clone())
            .size_full()
            .child(shell)
    }
}

/// present 间隔 = 上一帧提交完成到本帧提交完成的间隔，即 gpui 交给 DWM 的合成节奏。
#[cfg(feature = "frame-profiler")]
fn report_frame_probe(
    window: &Window,
    renders: usize,
    baseline_presents: u64,
    elapsed: Duration,
    cached_images: usize,
    cached_bytes: u64,
) {
    use std::io::Write as _;

    let snapshot = window.frame_duration_snapshot();
    let presents = snapshot
        .present_interval_histogram
        .len()
        .saturating_sub(baseline_presents);
    let seconds = elapsed.as_secs_f64();
    let viewport = window.viewport_size();
    let over_budget = 100.
        - snapshot
            .present_interval_histogram
            .percentile_below(16_666_666);
    let json = serde_json::json!({
        "kind": "frame_metrics",
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "viewport": [viewport.width.as_f32() as i32, viewport.height.as_f32() as i32],
        "scale": window.scale_factor(),
        "probe_secs": round(seconds, 3),
        "renders": renders,
        "presents": presents,
        "cached_images": cached_images,
        "cached_mb": round(cached_bytes as f64 / 1_048_576., 1),
        "fps": round(presents as f64 / seconds, 1),
        "present_interval_ms": histogram_ms(&snapshot.present_interval_histogram),
        "draw_ms": histogram_ms(&snapshot.draw_duration_histogram),
        "dirty_to_present_ms": histogram_ms(&snapshot.dirty_to_present_histogram),
        "present_over_16_67ms_pct": round(over_budget, 2),
    });
    log::info!("frame metrics: {json}");
    if presents == 0 {
        log::warn!("帧计时无效：窗口不在前台活动时 gpui 不记录 present 间隔");
    }
    if let Some(path) = std::env::var_os("KANALIB_METRICS_PATH") {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(mut file) => {
                let _ = file.write_all(format!("{json}\n").as_bytes());
            }
            Err(error) => {
                let display = std::path::Path::new(&path).display();
                log::warn!("metrics path {display}: {error}");
            }
        }
    }
}

#[cfg(feature = "frame-profiler")]
fn histogram_ms(hist: &Histogram<u64>) -> serde_json::Value {
    let ms = |ns: u64| round(ns as f64 / 1_000_000., 3);
    serde_json::json!({
        "samples": hist.len(),
        "p50": ms(hist.value_at_percentile(50.)),
        "p95": ms(hist.value_at_percentile(95.)),
        "p99": ms(hist.value_at_percentile(99.)),
        "max": ms(hist.max()),
    })
}

#[cfg(feature = "frame-profiler")]
fn round(value: f64, digits: u32) -> f64 {
    let factor = 10f64.powi(digits as i32);
    (value * factor).round() / factor
}

impl KanalibApp {
    fn render_sidebar(
        &self,
        this: &Entity<Self>,
        roots: &[PathBuf],
        source_aliases: &HashMap<PathBuf, String>,
        status: &str,
        error: Option<&str>,
        scanning: bool,
        device: Option<&str>,
        queue_pending: usize,
        queue_running: usize,
        queue_error: usize,
        filter: &Filter,
        counts: &Counts,
        screen: Screen,
        dedup_mode: GroupKind,
        dedup_pending: usize,
        dedup_similar_pending: usize,
        dedup_similar_scanned: bool,
        saved_searches: &[SavedSearch],
        blocked_tags: &HashSet<String>,
        search: &Entity<TextInput>,
        colors: UiColors,
    ) -> impl IntoElement + use<> {
        let state = self.state.clone();
        let this_choose = this.clone();
        let this_rescan = this.clone();
        let this_stop = this.clone();
        let this_retry = this.clone();
        let this_dedup = this.clone();
        let this_settings = this.clone();
        let library = screen == Screen::Library;
        let mut body = div()
            .id("sidebar-body")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px(px(8.))
            .pb(px(12.))
            .flex()
            .flex_col()
            .gap(px(2.));

        body = body.child(section("来源", colors));
        if roots.is_empty() {
            body = body.child(
                div()
                    .px(px(8.))
                    .pb(px(4.))
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("还没有图库文件夹"),
            );
        } else {
            for root in roots {
                let count = counts
                    .sources
                    .iter()
                    .find(|(path, _)| path == root)
                    .map(|(_, count)| *count)
                    .unwrap_or(0);
                body = body.child(source_row(
                    SharedString::from(format!("filter-source-{}", root.display())),
                    root.clone(),
                    source_label(root, source_aliases),
                    count,
                    library && filter == &Filter::Source(root.clone()),
                    this.clone(),
                    colors,
                ));
            }
        }

        body = body.child(section("筛选", colors));
        body = body.child(filter_row(
            SharedString::from("filter-all"),
            "全部图片".into(),
            counts.total,
            library && *filter == Filter::All,
            Filter::All,
            state.clone(),
            this.clone(),
            colors,
        ));
        body = body.child(filter_row(
            SharedString::from("filter-untagged"),
            "未打标".into(),
            counts.untagged,
            library && *filter == Filter::Untagged,
            Filter::Untagged,
            state.clone(),
            this.clone(),
            colors,
        ));
        body = body.child(filter_row(
            SharedString::from("filter-favorites"),
            "收藏".into(),
            counts.favorites,
            library && *filter == Filter::Favorites,
            Filter::Favorites,
            state.clone(),
            this.clone(),
            colors,
        ));
        if counts.failed > 0 {
            body = body.child(filter_row(
                SharedString::from("filter-failed"),
                "失败".into(),
                counts.failed,
                library && *filter == Filter::Failed,
                Filter::Failed,
                state.clone(),
                this.clone(),
                colors,
            ));
        }
        if counts.damaged > 0 {
            body = body.child(filter_row(
                SharedString::from("filter-damaged"),
                "损坏".into(),
                counts.damaged,
                library && *filter == Filter::Damaged,
                Filter::Damaged,
                state.clone(),
                this.clone(),
                colors,
            ));
        }

        body = body.child(section("整理", colors));
        body = body.child(dedup_row(
            SharedString::from("dedup-entry"),
            "重复项",
            dedup_pending.to_string(),
            screen == Screen::Duplicates && dedup_mode == GroupKind::Exact,
            GroupKind::Exact,
            this_dedup.clone(),
            colors,
        ));
        body = body.child(dedup_row(
            SharedString::from("dedup-similar"),
            "相似图片",
            if dedup_similar_scanned {
                dedup_similar_pending.to_string()
            } else {
                "扫描".to_string()
            },
            screen == Screen::Duplicates && dedup_mode == GroupKind::Similar,
            GroupKind::Similar,
            this_dedup,
            colors,
        ));
        let this_tags = this.clone();
        body = body.child(tag_manager_row(
            SharedString::from("tag-manager-entry"),
            "标签管理",
            screen == Screen::Tags,
            this_tags,
            colors,
        ));

        let this_save_search = this.clone();
        body = body.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(8.))
                .pt(px(10.))
                .pb(px(4.))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.subtle)
                        .child("智能集合"),
                )
                .child(
                    IconButton::new("save-saved-search", IconName::Plus)
                        .size(ControlSize::Sm)
                        .tooltip("保存当前视图")
                        .on_click(move |_, _, cx| {
                            let _ = this_save_search.update(cx, |app, cx| {
                                app.save_search_open = true;
                                cx.notify();
                            });
                        }),
                ),
        );
        if saved_searches.is_empty() {
            body = body.child(
                div()
                    .px(px(8.))
                    .pb(px(4.))
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("保存筛选与搜索，随时回来"),
            );
        } else {
            for saved in saved_searches {
                let name = saved.name.clone();
                let detail = if saved.query.is_empty() {
                    saved.filter.label()
                } else {
                    format!("{} · {}", saved.filter.label(), saved.query)
                };
                let saved_for_apply = saved.clone();
                let search_for_apply = search.clone();
                let this_apply = this.clone();
                let this_delete = this.clone();
                let id = saved.id;
                body = body.child(
                    div()
                        .id(SharedString::from(format!("saved-search-{id}")))
                        .role(Role::Button)
                        .aria_label(name.clone())
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(move |style| style.bg(colors.hover))
                        .on_click(move |_, _, cx| {
                            let _ = this_apply.update(cx, |app, cx| {
                                app.apply_saved_search(&saved_for_apply, &search_for_apply, cx)
                            });
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .child(div().truncate().text_xs().text_color(colors.fg).child(name))
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(colors.subtle)
                                        .child(detail),
                                ),
                        )
                        .child(
                            IconButton::new(
                                SharedString::from(format!("delete-saved-search-{id}")),
                                IconName::X,
                            )
                            .size(ControlSize::Sm)
                            .tooltip("删除智能集合")
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let _ = this_delete
                                    .update(cx, |app, cx| app.delete_saved_search(id, cx));
                            }),
                        ),
                );
            }
        }

        if !counts.ratings.is_empty() {
            body = body.child(section("评级", colors));
            for (rating, count) in counts.ratings.iter() {
                body = body.child(filter_row(
                    SharedString::from(format!("filter-rating-{rating}")),
                    format!("评级 {rating}"),
                    *count,
                    library && filter == &Filter::Rating(rating.clone()),
                    Filter::Rating(rating.clone()),
                    state.clone(),
                    this.clone(),
                    colors,
                ));
            }
        }

        if !counts.groups.is_empty() {
            body = body.child(section("自动归类", colors));
            for (group, count) in counts.groups.iter() {
                body = body.child(filter_row(
                    SharedString::from(format!("filter-group-{group}")),
                    group.clone(),
                    *count,
                    library && filter == &Filter::Group(group.clone()),
                    Filter::Group(group.clone()),
                    state.clone(),
                    this.clone(),
                    colors,
                ));
            }
        }

        if !counts.tags.is_empty() {
            body = body.child(section("常用标签", colors));
            for (tag, count) in counts
                .tags
                .iter()
                .filter(|(tag, _)| !blocked_tags.contains(tag))
            {
                body = body.child(filter_row(
                    SharedString::from(format!("filter-tag-{tag}")),
                    format!("#{tag}"),
                    *count,
                    library && filter == &Filter::Tag(tag.clone()),
                    Filter::Tag(tag.clone()),
                    state.clone(),
                    this.clone(),
                    colors,
                ));
            }
        }

        div()
            .w(px(245.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .child(
                div()
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .child("Kanalib"),
                    ),
            )
            .child(
                div()
                    .px(px(12.))
                    .pb(px(8.))
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(
                        Button::new("choose-folder", "添加文件夹")
                            .primary()
                            .on_click(move |_, _, cx| {
                                let _ = this_choose.update(cx, |app, cx| app.choose_folder(cx));
                            }),
                    )
                    .child(Button::new("rescan", "重新扫描").on_click(move |_, _, cx| {
                        let _ = this_rescan.update(cx, |app, cx| app.rescan(cx));
                    }))
                    .children((counts.failed > 0).then(|| {
                        Button::new("retry-failed", "重试失败").on_click(move |_, _, cx| {
                            let _ = this_retry.update(cx, |app, cx| app.retry_failed(cx));
                        })
                    }))
                    .children(scanning.then(|| {
                        Button::new("stop", "停止").on_click(move |_, _, cx| {
                            let _ = this_stop.update(cx, |app, cx| {
                                app.state.stop();
                                cx.notify();
                            });
                        })
                    })),
            )
            .child(
                div()
                    .mx(px(10.))
                    .mb(px(8.))
                    .p(px(10.))
                    .rounded(px(8.))
                    .bg(colors.sunken)
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_xs()
                            .text_color(if error.is_some() {
                                colors.danger
                            } else {
                                colors.muted
                            })
                            .child(error.unwrap_or(status).to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(format!(
                                "{} 张 · 已打标 {} · 未打标 {} · 失败 {}",
                                counts.total, counts.tagged, counts.untagged, counts.failed
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(device.unwrap_or("推理设备 · 未加载").to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if queue_error > 0 {
                                colors.warning
                            } else {
                                colors.subtle
                            })
                            .child(if queue_pending + queue_running + queue_error == 0 {
                                "打标队列 · 已清空".to_string()
                            } else {
                                format!(
                                    "打标队列 · 等待 {queue_pending} · 运行 {queue_running} · 失败 {queue_error}"
                                )
                            }),
                    ),
            )
            .child(
                div().px(px(8.)).py(px(4.)).child(settings_row(
                    SharedString::from("settings-entry"),
                    "设置",
                    screen == Screen::Settings,
                    this_settings,
                    colors,
                )),
            )
            .child(body)
    }

    fn sort_menu(&self, this: &Entity<Self>) -> Menu {
        let menu = |this: &Entity<Self>, mode: SortMode| {
            let this = this.clone();
            MenuItem::radio(mode.label(), self.sort == mode).on_click(move |_, cx| {
                let _ = this.update(cx, |app, cx| app.choose_sort_mode(mode, cx));
            })
        };
        let ascending = this.clone();
        let descending = this.clone();
        Menu::new()
            .item(menu(this, SortMode::Name))
            .item(menu(this, SortMode::Modified))
            .item(menu(this, SortMode::Size))
            .item(menu(this, SortMode::Resolution))
            .item(menu(this, SortMode::Star))
            .item(menu(this, SortMode::Favorite))
            .item(menu(this, SortMode::Rating))
            .separator()
            .item(MenuItem::radio("升序", self.sort_ascending).on_click(move |_, cx| {
                let _ = ascending.update(cx, |app, cx| app.set_sort_ascending(true, cx));
            }))
            .item(
                MenuItem::radio("降序", !self.sort_ascending).on_click(move |_, cx| {
                    let _ = descending.update(cx, |app, cx| app.set_sort_ascending(false, cx));
                }),
            )
    }

    fn export_menu(&self, this: &Entity<Self>) -> Menu {
        let selected_csv = this.clone();
        let selected_json = this.clone();
        let view_csv = this.clone();
        let view_json = this.clone();
        Menu::new()
            .item(
                MenuItem::new("选中 · CSV")
                    .icon(IconName::Download)
                    .on_click(move |_, cx| {
                        let _ = selected_csv.update(cx, |app, cx| {
                            app.export_assets(ExportTarget::Selected, ExportFormat::Csv, cx)
                        });
                    }),
            )
            .item(
                MenuItem::new("选中 · JSON")
                    .icon(IconName::Download)
                    .on_click(move |_, cx| {
                        let _ = selected_json.update(cx, |app, cx| {
                            app.export_assets(ExportTarget::Selected, ExportFormat::Json, cx)
                        });
                    }),
            )
            .separator()
            .item(
                MenuItem::new("当前视图 · CSV")
                    .icon(IconName::Download)
                    .on_click(move |_, cx| {
                        let _ = view_csv
                            .update(cx, |app, cx| {
                                app.export_assets(ExportTarget::View, ExportFormat::Csv, cx)
                            });
                    }),
            )
            .item(
                MenuItem::new("当前视图 · JSON")
                    .icon(IconName::Download)
                    .on_click(move |_, cx| {
                        let _ = view_json.update(cx, |app, cx| {
                            app.export_assets(ExportTarget::View, ExportFormat::Json, cx)
                        });
                    }),
            )
    }

    fn render_titlebar(
        &self,
        this: &Entity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
        filter: &Filter,
        filter_title: &str,
        visible: usize,
        search: &Entity<TextInput>,
        scanning: bool,
        colors: UiColors,
    ) -> AnyElement {
        let library = self.screen == Screen::Library;
        let wide = window.viewport_size().width >= px(1180.);
        let search_width = if wide { px(200.) } else { px(140.) };
        let this_clear = this.clone();
        let this_stop = this.clone();
        let this_palette = this.clone();
        let this_density = this.clone();
        let this_theme = this.clone();

        let clear = (library && *filter != Filter::All).then(|| {
            IconButton::new("clear-filter", IconName::X)
                .tooltip("清除筛选")
                .on_click(move |_, _, cx| {
                    let _ = this_clear.update(cx, |app, cx| {
                        app.set_filter(Filter::All, cx);
                    });
                })
        });
        let stop = (library && scanning).then(|| {
            Button::new("stop-main", "停止")
                .variant(ButtonVariant::Danger)
                .size(ControlSize::Sm)
                .on_click(move |_, _, cx| {
                    let _ = this_stop.update(cx, |app, cx| {
                        app.state.stop();
                        cx.notify();
                    });
                })
        });
        let palette = library.then(|| {
            IconButton::new("open-palette", IconName::Command)
                .tooltip("命令面板 (Ctrl+K)")
                .on_click(move |_, _, cx| {
                    let _ = this_palette.update(cx, |app, cx| {
                        app.palette_open = true;
                        cx.notify();
                    });
                })
        });
        let sort = library.then(|| {
            DropdownMenu::new("library-sort", "排序", self.sort_menu(this))
                .icon(IconName::ArrowDownUp)
                .variant(ButtonVariant::Ghost)
        });
        let density = library.then(|| {
            SegmentedControl::new("library-density", self.density.key())
                .size(ControlSize::Sm)
                .segment("small", "小", None)
                .segment("medium", "中", None)
                .segment("large", "大", None)
                .on_change(move |value, _, cx| {
                    let _ = this_density.update(cx, |app, cx| {
                        app.set_density(DensityPreset::from_key(value.as_ref()), cx)
                    });
                })
        });
        let theme = IconButton::new(
            "theme-toggle",
            if self.theme_dark {
                IconName::Sun
            } else {
                IconName::Moon
            },
        )
        .tooltip(if self.theme_dark {
            "切换到浅色主题"
        } else {
            "切换到深色主题"
        })
        .on_click(move |_, _, cx| {
            let _ = this_theme.update(cx, |app, cx| app.toggle_theme(cx));
        });
        let export = library.then(|| {
            DropdownMenu::new("library-export", "导出", self.export_menu(this))
                .icon(IconName::Download)
                .variant(ButtonVariant::Ghost)
        });
        let search_field = library.then(|| {
            div()
                .w(search_width)
                .child(Input::new(search).clearable().least("搜索 / is:fav"))
        });

        let mut left = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .items_center()
            .gap(px(8.));
        let show_brand = !library || wide;
        if show_brand {
            left = left.child(
                div()
                    .flex_none()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Kanalib"),
            );
            if library {
                left = left.child(div().flex_none().w(px(1.)).h(px(14.)).bg(colors.border));
            }
        }
        if library {
            left = left
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_sm()
                        .child(filter_title.to_string()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(colors.muted)
                        .child(format!("{visible} 张")),
                )
                .children(clear)
                .children(stop);
        } else {
            let label = match self.screen {
                Screen::Tags => "标签管理",
                Screen::Settings => "设置",
                Screen::Duplicates => "重复项",
                Screen::Library => "",
            };
            left = left.child(
                div()
                    .flex_none()
                    .text_sm()
                    .text_color(colors.muted)
                    .child(label),
            );
        }

        let mut right = div()
            .flex_none()
            .h_full()
            .flex()
            .items_center()
            .gap(px(4.));
        if library {
            right = right
                .children(palette)
                .children(sort)
                .children(density)
                .child(theme)
                .children(export)
                .children(search_field);
        } else {
            right = right.child(theme);
        }
        right = right.child(window_controls(window, colors));

        drag_region("kanalib-titlebar", window, cx)
            .w_full()
            .flex_none()
            .h(px(TITLEBAR_HEIGHT))
            .pl(px(12.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.))
            .bg(colors.bg)
            .border_b_1()
            .border_color(colors.border)
            .child(left)
            .child(right)
            .into_any_element()
    }

    fn render_main(
        &self,
        this: &Entity<Self>,
        assets: Arc<RwLock<Vec<Asset>>>,
        visible_indices: Arc<Vec<usize>>,
        selection: Arc<HashSet<PathBuf>>,
        roots_empty: bool,
        error: Option<&str>,
        scanning: bool,
        width: Pixels,
        colors: UiColors,
    ) -> impl IntoElement {
        let star_menu = {
            let menu_item = |this: &Entity<Self>, star: u8, label: &'static str| {
                let this = this.clone();
                MenuItem::new(label).on_click(move |_, cx| {
                    let _ = this.update(cx, |app, cx| app.apply_star(star, cx));
                })
            };
            Menu::new()
                .item(menu_item(this, 1, "1 星"))
                .item(menu_item(this, 2, "2 星"))
                .item(menu_item(this, 3, "3 星"))
                .item(menu_item(this, 4, "4 星"))
                .item(menu_item(this, 5, "5 星"))
                .separator()
                .item(menu_item(this, 0, "清除星级"))
        };

        let tag_menu = {
            let add = this.clone();
            let remove = this.clone();
            Menu::new()
                .item(
                    MenuItem::new("添加标签…")
                        .icon(IconName::Tag)
                        .on_click(move |_, cx| {
                            let _ = add.update(cx, |app, cx| {
                                app.open_tag_dialog(TagDialogMode::Add, cx)
                            });
                        }),
                )
                .item(
                    MenuItem::new("移除标签…")
                        .icon(IconName::X)
                        .on_click(move |_, cx| {
                            let _ = remove.update(cx, |app, cx| {
                                app.open_tag_dialog(TagDialogMode::Remove, cx)
                            });
                        }),
                )
        };
        let export_menu = self.export_menu(this);
        let columns = grid_columns(width.as_f32(), self.density.min_width());
        let tile_width = grid_tile_width(width.as_f32(), columns);
        let image_height = self.density.image_height();
        let row_height = self.density.row_height();
        let visible = visible_indices.len();
        let row_count = visible.div_ceil(columns);
        let assets_for_list = assets;
        let rows_for_list = visible_indices;
        let grid: AnyElement = if rows_for_list.is_empty() {
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
                        .child(if roots_empty {
                            "还没有图库文件夹。点击侧栏“添加文件夹”开始。"
                        } else if scanning {
                            "正在扫描图片..."
                        } else {
                            "没有符合筛选的图片。"
                        }),
                )
                .into_any_element()
        } else {
            let this_for_list = this.clone();
            let thumb_cache_for_list = self.thumb_cache.clone();
            let focus_for_list = self.grid_focus.clone();
            let selection_for_list = selection.clone();
            uniform_list("asset-grid", row_count, move |range, _, _| {
                let Ok(assets) = assets_for_list.read() else {
                    return Vec::new();
                };
                range
                    .map(|row| {
                        let start = row * columns;
                        let end = (start + columns).min(rows_for_list.len());
                        let mut line = div()
                            .w_full()
                            .h(px(row_height))
                            .flex()
                            .gap(px(GRID_GAP))
                            .px(px(GRID_PADDING))
                            .pt(px(ROW_PADDING))
                            .pb(px(ROW_PADDING));
                        for &asset_index in &rows_for_list[start..end] {
                            let asset = &assets[asset_index];
                            let is_selected = selection_for_list.contains(&asset.path);
                            line = line.child(tile(
                                asset.clone(),
                                is_selected,
                                this_for_list.clone(),
                                thumb_cache_for_list.clone(),
                                tile_width,
                                image_height,
                                focus_for_list.clone(),
                                colors,
                            ));
                        }
                        line
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.grid_scroll)
            .flex_1()
            .w_full()
            .min_h(px(0.))
            .into_any_element()
        };
        let grid_area = div()
            .id("library-grid")
            .role(Role::Grid)
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .flex()
            .flex_col()
            .track_focus(&self.grid_focus)
            .key_context(GRID_CONTEXT)
            .on_mouse_down(MouseButton::Left, {
                let focus = self.grid_focus.clone();
                move |_, window, cx| window.focus(&focus, cx)
            })
            .child(grid);

        let bulk = (selection.len() >= 2).then(|| {
            let this_fav = this.clone();
            let this_unfav = this.clone();
            let this_prompt = this.clone();
            let this_paths = this.clone();
            let this_bulk_clear = this.clone();
            div()
                .px(px(16.))
                .py(px(6.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .bg(colors.sunken)
                .border_b_1()
                .border_color(colors.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.fg)
                                .child(format!("已选 {} 张", selection.len())),
                        )
                        .child(
                            Button::new("bulk-favorite", "收藏")
                                .size(ControlSize::Sm)
                                .icon(IconName::Heart)
                                .on_click(move |_, _, cx| {
                                    let _ =
                                        this_fav.update(cx, |app, cx| app.apply_favorite(true, cx));
                                }),
                        )
                        .child(
                            Button::new("bulk-unfavorite", "取消收藏")
                                .size(ControlSize::Sm)
                                .on_click(move |_, _, cx| {
                                    let _ = this_unfav
                                        .update(cx, |app, cx| app.apply_favorite(false, cx));
                                }),
                        )
                        .child(
                            DropdownMenu::new("bulk-star", "星级", star_menu)
                                .icon(IconName::Star)
                                .variant(ButtonVariant::Ghost),
                        )
                        .child(
                            DropdownMenu::new("bulk-tags", "标签", tag_menu)
                                .icon(IconName::Tag)
                                .variant(ButtonVariant::Ghost),
                        )
                        .child(
                            Button::new("bulk-copy-prompt", "复制 Prompt")
                                .size(ControlSize::Sm)
                                .on_click(move |_, _, cx| {
                                    let _ = this_prompt
                                        .update(cx, |app, cx| app.copy_selected_prompt(cx));
                                }),
                        )
                        .child(
                            Button::new("bulk-copy-paths", "复制路径")
                                .size(ControlSize::Sm)
                                .on_click(move |_, _, cx| {
                                    let _ = this_paths
                                        .update(cx, |app, cx| app.copy_selected_paths(cx));
                                }),
                        )
                        .child(
                            DropdownMenu::new("bulk-export", "导出", export_menu)
                                .icon(IconName::Download)
                                .variant(ButtonVariant::Ghost),
                        ),
                )
                .child(
                    Button::new("bulk-clear", "取消选择")
                        .size(ControlSize::Sm)
                        .on_click(move |_, _, cx| {
                            let _ = this_bulk_clear.update(cx, |app, cx| app.clear_selection(cx));
                        }),
                )
        });

        div()
            .w(width)
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .children(bulk)
            .children(error.map(|message| {
                div()
                    .mx(px(16.))
                    .mt(px(10.))
                    .p(px(10.))
                    .rounded(px(6.))
                    .bg(colors.sunken)
                    .text_xs()
                    .text_color(colors.danger)
                    .child(message.to_string())
            }))
            .child(grid_area)
    }

    fn render_dedup(
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

        let this_scan = this.clone();
        let this_preselect = this.clone();
        let this_batch = this.clone();
        let this_library = this.clone();
        let this_similar_scan = this.clone();
        let preselect_mode = self.dedup_mode;
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
                if similar_scanning {
                    "相似图片 扫描中".to_string()
                } else {
                    format!("相似图片 {}", similar_summary.total)
                },
                similar_mode,
                GroupKind::Similar,
                this.clone(),
                colors,
            ));

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
                    .child(
                        Button::new("dedup-scan", "重新扫描")
                            .disabled(busy)
                            .on_click(move |_, _, cx| {
                                let _ = this_scan.update(cx, |app, cx| {
                                    app.state.start_scan();
                                    app.dedup_confirm = None;
                                    app.dedup_selected = None;
                                    app.dedup_notice = None;
                                    cx.notify();
                                });
                            }),
                    )
                    .children(similar_mode.then(|| {
                        Button::new(
                            "dedup-similar-scan",
                            if similar_scanned {
                                "重新扫描相似"
                            } else {
                                "扫描相似图片"
                            },
                        )
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
                            .disabled(busy || summary.pending == 0)
                            .on_click(move |_, _, cx| {
                                let _ = this_batch.update(cx, |app, cx| {
                                    app.dedup_confirm = Some(DedupConfirm::Batch);
                                    cx.notify();
                                });
                            })
                    }))
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
                .flex()
                .items_center()
                .gap(px(5.))
                .border_b_1()
                .border_color(colors.border)
                .child(div().text_xs().text_color(colors.subtle).child("相似阈值"))
                .child(similar_preset_button(
                    6,
                    "严格 ≤6",
                    similar_threshold == 6,
                    busy,
                    this.clone(),
                ))
                .child(similar_preset_button(
                    10,
                    "标准 ≤10",
                    similar_threshold == 10,
                    busy,
                    this.clone(),
                ))
                .child(similar_preset_button(
                    14,
                    "宽松 ≤14",
                    similar_threshold == 14,
                    busy,
                    this.clone(),
                ))
                .child(div().text_xs().text_color(colors.subtle).child(format!(
                    "当前 ≤{} · 约 {}% 相似",
                    similar_threshold,
                    similarity_pct_from_distance(similar_threshold as u32)
                )))
                .child(div().flex_1())
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(if similar_scanning {
                            "计算中...".to_string()
                        } else if similar_scanned {
                            format!("已扫 {} 组", similar_summary.total)
                        } else {
                            "尚未扫描".to_string()
                        }),
                )
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
        let banner: Option<AnyElement> = error
            .map(|message| (message.to_string(), colors.danger))
            .or_else(|| notice.map(|message| (message, colors.muted)))
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

    fn render_dedup_detail(
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

        let open = keeper.map(|asset| {
            let path = asset.path.clone();
            Button::new("dedup-open", "打开保留项").on_click(move |_, _, _| open_path(&path))
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
                        .disabled(scanning || keeper.is_none() || protected_duplicate)
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

    fn render_dedup_confirm(
        &self,
        this: &Entity<Self>,
        confirm: &DedupConfirm,
        groups: &Arc<Vec<DuplicateGroup>>,
        summary: DedupSummary,
        colors: UiColors,
    ) -> Option<AnyElement> {
        let (title, detail, rows, key) = match confirm {
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
                )
            }
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

    fn render_tags(
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
                        let this_block = this_list.clone();
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
                                Button::new(
                                    SharedString::from(format!("block-tag-{}", block_name)),
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

    fn render_settings(
        &self,
        this: &Entity<Self>,
        roots: &[PathBuf],
        source_aliases: &HashMap<PathBuf, String>,
        protected_paths: &[PathBuf],
        similar_threshold: u8,
        queue_pending: usize,
        queue_running: usize,
        queue_error: usize,
        status: &str,
        error: Option<&str>,
        width: Pixels,
        colors: UiColors,
    ) -> AnyElement {
        let page_padding = px(24.);
        let this_back = this.clone();
        let this_add = this.clone();
        let this_refresh_cache = this.clone();
        let this_cleanup_cache = this.clone();
        let cache_stats = self.cache_stats.clone();
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

        let mut threshold_row = div().flex().items_center().gap(px(6.));
        for (value, label) in [(6u8, "严格 ≤6"), (10u8, "标准 ≤10"), (14u8, "宽松 ≤14")]
        {
            let this_threshold = this.clone();
            threshold_row = threshold_row.child(
                Button::new(
                    SharedString::from(format!("settings-threshold-{value}")),
                    label,
                )
                .variant(if similar_threshold == value {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Secondary
                })
                .on_click(move |_, _, cx| {
                    let _ = this_threshold.update(cx, |app, cx| {
                        let saved = app.state.set_similar_threshold(value);
                        app.settings_notice = Some(format!("相似阈值默认值已设为 ≤{saved}"));
                        cx.notify();
                    });
                }),
            );
        }
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
                    .child("默认阈值用于相似扫描；去重页也可临时切换。当前："),
            )
            .child(threshold_row);

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
                let this_remove = this.clone();
                let this_rename = this.clone();
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
                                .gap(px(1.))
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
                                        .child(root.display().to_string()),
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
            .child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("只清理没有任何资产引用的缩略图；缓存预算淘汰暂未启用。"),
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
                            .child(sources_section)
                            .child(protected_section)
                            .child(similar_section)
                            .child(data_section)
                            .child(safety_section)
                            .child(shortcut_section)
                            .child(rules_section),
                    ),
            )
            .into_any_element()
    }

    fn render_inspector(
        &self,
        this: &Entity<Self>,
        selected: Option<&Asset>,
        counts: &Counts,
        blocked_tags: &HashSet<String>,
        source_aliases: &HashMap<PathBuf, String>,
        width: Pixels,
        colors: UiColors,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let this = this.clone();
        let content: AnyElement = if let Some(asset) = selected {
            let preview: AnyElement = match asset.thumb.as_ref().or(Some(&asset.path)) {
                Some(path) => img(path.clone())
                    .image_cache(&self.thumb_cache)
                    .w_full()
                    .h(px(230.))
                    .object_fit(ObjectFit::Contain)
                    .into_any_element(),
                None => div()
                    .h(px(230.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("无预览")
                    .into_any_element(),
            };

            let path = asset.path.clone();
            let repair_this = this.clone();
            let repair_path = asset.path.clone();
            let export_state = state.clone();
            let export_this = this.clone();
            let export_path = asset.path.clone();
            let manual_initial = asset
                .manual_tags
                .iter()
                .map(|tag| normalize_tag_name(&tag.tag))
                .collect::<HashSet<_>>();
            let manual_names = asset
                .manual_tags
                .iter()
                .map(|tag| SharedString::from(tag.tag.clone()))
                .collect::<Vec<_>>();
            let mut suggestion_names = counts
                .tags
                .iter()
                .filter(|(name, _)| !blocked_tags.contains(name))
                .take(40)
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>();
            for tag in asset.all_tags() {
                if !suggestion_names
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&tag.tag))
                {
                    suggestion_names.push(tag.tag.clone());
                }
            }
            let suggestions = suggestion_names
                .into_iter()
                .map(|name| Choice::new(name.clone(), name))
                .collect::<Vec<_>>();
            let manual_state = state.clone();
            let manual_this = this.clone();
            let manual_toaster = self.toaster.clone();
            let manual_path = path.clone();
            let manual_input = TagInput::new(
                SharedString::from(format!("manual-tags-{}", path.display())),
                manual_names,
            )
            .placeholder("添加标签，回车确认")
            .check(|value| !normalize_tag_name(value).is_empty())
            .suggestions(suggestions)
            .on_change(move |next, _, cx| {
                let next_set = next
                    .iter()
                    .map(|value| normalize_tag_name(value))
                    .filter(|value| !value.is_empty())
                    .collect::<HashSet<_>>();
                let added = next_set.difference(&manual_initial).cloned().collect::<Vec<_>>();
                let removed = manual_initial
                    .difference(&next_set)
                    .cloned()
                    .collect::<Vec<_>>();
                let mut changed = 0usize;
                let mut failure = None;
                for tag in &added {
                    match manual_state.add_tags(std::slice::from_ref(&manual_path), tag) {
                        Ok(updated) => changed += updated,
                        Err(error) => {
                            failure = Some(format!("{error:#}"));
                            break;
                        }
                    }
                }
                if failure.is_none() {
                    for tag in &removed {
                        match manual_state.remove_tags(std::slice::from_ref(&manual_path), tag) {
                            Ok(updated) => changed += updated,
                            Err(error) => {
                                failure = Some(format!("{error:#}"));
                                break;
                            }
                        }
                    }
                }
                if let Some(error) = failure {
                    manual_state.set_error(format!("标签操作失败：{error}"));
                    push_toast(
                        &manual_toaster,
                        cx,
                        format!("标签操作失败：{error}"),
                        Severity::Danger,
                    );
                } else if changed > 0 {
                    push_toast(
                        &manual_toaster,
                        cx,
                        "标签已更新",
                        Severity::Success,
                    );
                } else if !added.is_empty() || !removed.is_empty() {
                    push_toast(
                        &manual_toaster,
                        cx,
                        "标签已存在或无需变更",
                        Severity::Warning,
                    );
                }
                let _ = manual_this.update(cx, |_, cx| cx.notify());
            });

            let mut tags = div().id("inspector-tags").flex().flex_col().gap(px(2.));
            for category in [
                "general",
                "character",
                "copyright",
                "style",
                "meta",
                "rating",
            ] {
                let hits: Vec<&TagHit> = asset
                    .tags
                    .iter()
                    .filter(|hit| hit.category == category)
                    .take(if category == "general" { 12 } else { 6 })
                    .collect();
                if hits.is_empty() {
                    continue;
                }
                tags = tags.child(
                    div()
                        .mt(px(6.))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.subtle)
                        .child(category.to_string()),
                );
                for hit in hits {
                    let filter = Filter::Tag(hit.tag.clone());
                    let filter_state = state.clone();
                    let filter_this = this.clone();
                    let remove_state = state.clone();
                    let remove_this = this.clone();
                    let remove_toaster = self.toaster.clone();
                    let remove_path = path.clone();
                    let tag = hit.tag.clone();
                    let remove_tag = tag.clone();
                    let score = hit.score;
                    tags = tags.child(
                        div()
                            .id(SharedString::from(format!(
                                "inspector-tag-{}-{}",
                                category, tag
                            )))
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(6.))
                            .py(px(3.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(move |style| style.bg(colors.hover))
                            .on_click(move |_, _, cx| {
                                filter_state.set_filter(filter.clone());
                                let _ = filter_this.update(cx, |app, cx| {
                                    app.screen = Screen::Library;
                                    cx.notify();
                                });
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.fg)
                                    .child(format!("#{tag}")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(colors.subtle)
                                            .child(format!("{score:.3}")),
                                    )
                                    .child(
                                        IconButton::new(
                                            SharedString::from(format!(
                                                "remove-tag-{}-{}",
                                                category, tag
                                            )),
                                            IconName::X,
                                        )
                                        .size(ControlSize::Sm)
                                        .tooltip("移除标签")
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            match remove_state.remove_tags(
                                                std::slice::from_ref(&remove_path),
                                                &remove_tag,
                                            ) {
                                                Ok(updated) => push_toast(
                                                    &remove_toaster,
                                                    cx,
                                                    if updated > 0 {
                                                        format!("已移除标签 #{remove_tag}")
                                                    } else {
                                                        "没有可移除的标签".to_string()
                                                    },
                                                    if updated > 0 {
                                                        Severity::Success
                                                    } else {
                                                        Severity::Warning
                                                    },
                                                ),
                                                Err(error) => push_toast(
                                                    &remove_toaster,
                                                    cx,
                                                    format!("移除标签失败：{error:#}"),
                                                    Severity::Danger,
                                                ),
                                            }
                                            let _ = remove_this.update(cx, |_, cx| cx.notify());
                                        }),
                                    ),
                            ),
                    );
                }
            }

            let path_for_open = path.clone();
            let open = Button::new("open-file", "打开文件")
                .size(ControlSize::Sm)
                .on_click(move |_, _, _| {
                    open_path(&path_for_open);
                });
            let this_quick = this.clone();
            let quick = Button::new("quick-view", "快速查看")
                .size(ControlSize::Sm)
                .icon(IconName::Maximize2)
                .on_click(move |_, _, cx| {
                    let _ = this_quick.update(cx, |app, cx| app.open_quick_view(cx));
                });
            let favorite = asset.favorite;
            let favorite_state = state.clone();
            let favorite_this = this.clone();
            let favorite_toaster = self.toaster.clone();
            let favorite_path = path.clone();
            let mut favorite_button = Button::new(
                SharedString::from(format!("inspector-favorite-{}", path.display())),
                if favorite { "已收藏" } else { "收藏" },
            )
            .size(ControlSize::Sm)
            .icon(IconName::Heart)
            .on_click(move |_, _, cx| {
                match favorite_state.set_favorite(std::slice::from_ref(&favorite_path), !favorite) {
                    Ok(updated) => push_toast(
                        &favorite_toaster,
                        cx,
                        if favorite {
                            format!("已取消收藏 {updated} 张")
                        } else {
                            format!("已收藏 {updated} 张")
                        },
                        Severity::Success,
                    ),
                    Err(error) => {
                        favorite_state.set_error(format!("收藏失败：{error:#}"));
                        push_toast(
                            &favorite_toaster,
                            cx,
                            format!("收藏失败：{error:#}"),
                            Severity::Danger,
                        );
                    }
                }
                let _ = favorite_this.update(cx, |_, cx| cx.notify());
            });
            if favorite {
                favorite_button = favorite_button.primary();
            }
            let star_state = state.clone();
            let star_this = this.clone();
            let star_toaster = self.toaster.clone();
            let star_path = path.clone();
            let rating_control = Rating::new(
                SharedString::from(format!("inspector-star-{}", path.display())),
                asset.star,
            )
            .on_change(move |value, _, cx| {
                match star_state.set_star(std::slice::from_ref(&star_path), value) {
                    Ok(updated) => push_toast(
                        &star_toaster,
                        cx,
                        if value == 0 {
                            format!("已清除 {updated} 张的星级")
                        } else {
                            format!("已设置 {updated} 张为 {value} 星")
                        },
                        Severity::Success,
                    ),
                    Err(error) => {
                        star_state.set_error(format!("设置星级失败：{error:#}"));
                        push_toast(
                            &star_toaster,
                            cx,
                            format!("设置星级失败：{error:#}"),
                            Severity::Danger,
                        );
                    }
                }
                let _ = star_this.update(cx, |_, cx| cx.notify());
            });
            let prompt = prompt_for(asset);
            let prompt_empty = prompt.is_empty();
            let prompt_count = asset
                .tags
                .iter()
                .filter(|tag| {
                    matches!(tag.category.as_str(), "general" | "character" | "copyright")
                })
                .count();
            let prompt_toaster = self.toaster.clone();
            let copy_prompt = Button::new("copy-prompt", "复制 Prompt")
                .size(ControlSize::Sm)
                .disabled(prompt_empty)
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(prompt.clone()));
                    push_toast(
                        &prompt_toaster,
                        cx,
                        format!("已复制 {prompt_count} 个标签"),
                        Severity::Success,
                    );
                });
            let copy_path_text = path.display().to_string();
            let copy_path_toaster = self.toaster.clone();
            let copy_path = Button::new("copy-path", "复制路径")
                .size(ControlSize::Sm)
                .icon(IconName::Copy)
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_path_text.clone()));
                    push_toast(&copy_path_toaster, cx, "已复制文件路径", Severity::Success);
                });

            let has_issue = asset.error.is_some() || asset.damage.is_some();
            let issue = has_issue.then(|| {
                let repair = Button::new("repair-asset", "容错重解")
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        let path = repair_path.clone();
                        let _ = repair_this.update(cx, |app, cx| {
                            if let Err(error) = app.state.repair_asset(path) {
                                app.state.set_error(format!("修复失败：{error:#}"));
                            }
                            cx.notify();
                        });
                    });
                let export = Button::new("export-repair", "导出修复副本…")
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        let state = export_state.clone();
                        let path = export_path.clone();
                        let weak = export_this.downgrade();
                        cx.spawn(async move |cx| {
                            let stem = path
                                .file_stem()
                                .map(|value| value.to_string_lossy().to_string())
                                .unwrap_or_else(|| "image".to_string());
                            let mut dialog = rfd::AsyncFileDialog::new()
                                .set_title("导出修复副本")
                                .set_file_name(format!("{stem}-repaired.png"))
                                .add_filter("PNG 图片", &["png"])
                                .add_filter("JPEG 图片", &["jpg", "jpeg"]);
                            if let Some(parent) = path.parent() {
                                dialog = dialog.set_directory(parent);
                            }
                            let Some(handle) = dialog.save_file().await else {
                                return;
                            };
                            let output = handle.path().to_path_buf();
                            let result = state.export_repaired_copy(path.clone(), output);
                            let _ = weak.update(cx, |app, cx| {
                                if let Err(error) = result {
                                    app.state
                                        .set_error(format!("导出修复副本失败：{error:#}"));
                                }
                                cx.notify();
                            });
                        })
                        .detach();
                    });
                let mut panel = div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(8.))
                    .rounded(px(6.))
                    .bg(colors.sunken)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.subtle)
                            .child("文件异常"),
                    );
                if let Some(damage) = &asset.damage {
                    panel = panel.child(
                        div()
                            .text_xs()
                            .text_color(colors.warning)
                            .child(damage.clone()),
                    );
                }
                if let Some(error) = &asset.error {
                    panel = panel.child(
                        div()
                            .text_xs()
                            .text_color(colors.danger)
                            .child(format!("打标失败：{}", friendly_media_error(error))),
                    );
                }
                panel
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child("修复只重建预览并重新打标，不改动原文件；导出会另存为新文件。"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .child(repair)
                            .child(export),
                    )
            });

            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(preview)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(asset.title()),
                )
                .child(div().text_xs().text_color(colors.muted).child(format!(
                    "{} × {} · {} · {}",
                    asset.width,
                    asset.height,
                    if asset.rating.is_empty() {
                        "未评级"
                    } else {
                        asset.rating.as_str()
                    },
                    asset.group
                )))
                .child(div().text_xs().text_color(colors.subtle).child(format!(
                    "{} · {}",
                    format_bytes(asset.bytes),
                    format_date(asset.mtime)
                )))
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(format!(
                            "来源 · {}",
                            source_label(&asset.library_root, source_aliases)
                        )),
                )
                .child(div().text_xs().text_color(colors.subtle).child(
                    match (&asset.sha1, &asset.blake3) {
                        (Some(sha1), Some(blake3)) => format!(
                            "SHA1 {} · B3 {}{}",
                            &sha1[..12.min(sha1.len())],
                            &blake3[..8.min(blake3.len())],
                            match asset.phash {
                                Some(phash) => format!(" · pHash {phash:016x}"),
                                None => " · pHash 未计算（解码失败）".to_string(),
                            }
                        ),
                        _ => "内容哈希未计算".to_string(),
                    },
                ))
                .children(issue)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(favorite_button)
                        .child(quick)
                        .child(open),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_xs().text_color(colors.subtle).child("个人星级"))
                        .child(rating_control),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(copy_prompt)
                        .child(copy_path),
                )
                .child(
                    div()
                        .mt(px(4.))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.subtle)
                        .child("手工标签（回车添加，自动保存）"),
                )
                .child(manual_input)
                .child(
                    div()
                        .mt(px(4.))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.subtle)
                        .child("自动标签（点击筛选，× 移除）"),
                )
                .child(tags)
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .text_sm()
                .text_color(colors.muted)
                .child("选择一张图片")
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child("自动标签与分类来自本地 PixAI Tagger。"),
                )
                .into_any_element()
        };

        div()
            .w(width)
            .flex_none()
            .h_full()
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .id("inspector-scroll")
            .overflow_y_scroll()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child("检查器"),
            )
            .child(content)
    }
}

fn section(title: &str, colors: UiColors) -> impl IntoElement {
    div()
        .px(px(8.))
        .pt(px(10.))
        .pb(px(4.))
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.subtle)
        .child(title.to_string())
}

fn source_row(
    id: SharedString,
    root: PathBuf,
    label: String,
    count: usize,
    active: bool,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> impl IntoElement {
    let filter = Filter::Source(root.clone());
    let this_click = this.clone();
    let this_right = this.clone();
    let root_right = root.clone();
    let mut row = div()
        .id(id)
        .role(Role::Button)
        .aria_label(root.display().to_string())
        .flex()
        .items_center()
        .justify_between()
        .px(px(8.))
        .py(px(5.))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover));
    if active {
        row = row.bg(colors.selection);
    }
    row.on_click(move |_, _, cx| {
        let _ = this_click.update(cx, |app, cx| {
            app.set_filter(filter.clone(), cx);
            app.dedup_confirm = None;
        });
    })
    .on_mouse_down(MouseButton::Right, move |event, window, cx| {
        window.prevent_default();
        let _ = this_right.update(cx, |app, cx| {
            app.source_menu_epoch = app.source_menu_epoch.wrapping_add(1);
            app.source_menu = Some(SourceMenuState {
                request: app.source_menu_epoch,
                root: root_right.clone(),
                position: event.position,
            });
            cx.notify();
        });
    })
    .child(
        div()
            .flex_1()
            .min_w(px(0.))
            .truncate()
            .text_xs()
            .text_color(colors.fg)
            .child(label),
    )
    .child(
        div()
            .flex_none()
            .text_xs()
            .text_color(colors.subtle)
            .child(count.to_string()),
    )
}

fn source_menu(root: &Path, this: Entity<KanalibApp>) -> Menu {
    let rename_this = this.clone();
    let rename_root = root.to_path_buf();
    let remove_this = this.clone();
    let remove_root = root.to_path_buf();
    let reveal_root = root.to_path_buf();
    Menu::new()
        .item(
            MenuItem::new("重命名…")
                .icon(IconName::Pencil)
                .on_click(move |_, cx| {
                    let _ = rename_this.update(cx, |app, cx| {
                        app.open_source_rename(rename_root.clone(), cx);
                    });
                }),
        )
        .item(
            MenuItem::new("在资源管理器中打开")
                .icon(IconName::FolderOpen)
                .on_click(move |_, _| reveal_path(&reveal_root)),
        )
        .separator()
        .item(
            MenuItem::new("移除来源…")
                .icon(IconName::Trash2)
                .on_click(move |_, cx| {
                    let _ = remove_this.update(cx, |app, cx| {
                        app.source_remove_confirm = Some(remove_root.clone());
                        cx.notify();
                    });
                }),
        )
}

fn filter_row(
    id: SharedString,
    label: String,
    count: usize,
    active: bool,
    filter: Filter,
    _state: Arc<AppState>,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> impl IntoElement {
    let mut row = div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .flex()
        .items_center()
        .justify_between()
        .px(px(8.))
        .py(px(5.))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover));
    if active {
        row = row.bg(colors.selection);
    }
    row.on_click(move |_, _, cx| {
        let _ = this.update(cx, |app, cx| {
            app.set_filter(filter.clone(), cx);
            app.dedup_confirm = None;
        });
    })
    .child(
        div()
            .flex_1()
            .min_w(px(0.))
            .truncate()
            .text_xs()
            .text_color(colors.fg)
            .child(label),
    )
    .child(
        div()
            .flex_none()
            .text_xs()
            .text_color(colors.subtle)
            .child(count.to_string()),
    )
}

fn dedup_row(
    id: SharedString,
    label: &'static str,
    count: String,
    active: bool,
    mode: GroupKind,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> impl IntoElement {
    let mut row = div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .flex()
        .items_center()
        .justify_between()
        .px(px(8.))
        .py(px(5.))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover));
    if active {
        row = row.bg(colors.selection);
    }
    row.on_click(move |_, _, cx| {
        let _ = this.update(cx, |app, cx| app.open_dedup_mode(mode, cx));
    })
    .child(div().text_xs().text_color(colors.fg).child(label))
    .child(
        div()
            .text_xs()
            .text_color(if active { colors.accent } else { colors.subtle })
            .child(count),
    )
}

fn tag_manager_row(
    id: SharedString,
    label: &'static str,
    active: bool,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> impl IntoElement {
    let mut row = div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .flex()
        .items_center()
        .justify_between()
        .px(px(8.))
        .py(px(5.))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover));
    if active {
        row = row.bg(colors.selection);
    }
    row.on_click(move |_, _, cx| {
        let _ = this.update(cx, |app, cx| app.open_tag_manager(cx));
    })
    .child(div().text_xs().text_color(colors.fg).child(label))
    .child(
        div()
            .text_xs()
            .text_color(if active { colors.accent } else { colors.subtle })
            .child("›"),
    )
}

fn settings_row(
    id: SharedString,
    label: &'static str,
    active: bool,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> impl IntoElement {
    let mut row = div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .flex()
        .items_center()
        .justify_between()
        .px(px(8.))
        .py(px(5.))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover));
    if active {
        row = row.bg(colors.selection);
    }
    row.on_click(move |_, _, cx| {
        let _ = this.update(cx, |app, cx| app.open_settings(cx));
    })
    .child(div().text_xs().text_color(colors.fg).child(label))
    .child(div().text_xs().text_color(colors.subtle).child("›"))
}

fn dedup_filter_chip(
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

fn dedup_tab(
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

fn similar_preset_button(
    threshold: u8,
    label: &'static str,
    active: bool,
    disabled: bool,
    this: Entity<KanalibApp>,
) -> AnyElement {
    Button::new(SharedString::from(format!("sim-preset-{threshold}")), label)
        .variant(if active {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Secondary
        })
        .disabled(disabled)
        .on_click(move |_, _, cx| {
            let _ = this.update(cx, |app, cx| {
                app.state.set_similar_threshold(threshold);
                app.state.start_similar_scan();
                app.dedup_selected = None;
                app.dedup_confirm = None;
                app.dedup_notice = None;
                cx.notify();
            });
        })
        .into_any_element()
}

fn small_badge(label: String, fg: Hsla, bg: Hsla) -> AnyElement {
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

fn group_state_label(state: ReviewState) -> &'static str {
    match state {
        ReviewState::Pending => "未审",
        ReviewState::Ignored => "已忽略",
        ReviewState::Related => "相关变体",
        ReviewState::Done => "已保留",
    }
}

fn dedup_group_row(
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

fn dedup_asset_card(
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

struct DiffRow {
    label: &'static str,
    values: Vec<String>,
    best: Option<usize>,
}

fn best_max<T: Ord + Copy>(assets: &[&Asset], value: impl Fn(&Asset) -> T) -> Option<usize> {
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

fn best_min<T: Ord + Copy>(assets: &[&Asset], value: impl Fn(&Asset) -> T) -> Option<usize> {
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

fn push_diff_row(
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

fn dedup_diff(group: &DuplicateGroup, assets: &[&Asset], colors: UiColors) -> AnyElement {
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

fn window_controls(window: &Window, colors: UiColors) -> impl IntoElement {
    let maximized = window.is_maximized();
    div()
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .child(window_control_button(
            "window-min",
            IconName::Minus,
            WindowControlArea::Min,
            false,
            colors,
        ))
        .child(window_control_button(
            "window-max",
            if maximized {
                IconName::Copy
            } else {
                IconName::Square
            },
            WindowControlArea::Max,
            false,
            colors,
        ))
        .child(window_control_button(
            "window-close",
            IconName::X,
            WindowControlArea::Close,
            true,
            colors,
        ))
}

fn window_control_button(
    id: &'static str,
    icon: IconName,
    area: WindowControlArea,
    close: bool,
    colors: UiColors,
) -> impl IntoElement {
    let group = SharedString::from(format!("{id}-hover"));
    div()
        .id(id)
        .group(group.clone())
        .flex_none()
        .h_full()
        .w(px(46.))
        .flex()
        .items_center()
        .justify_center()
        .window_control_area(area)
        .hover(move |style| style.bg(if close { colors.danger } else { colors.hover }))
        .on_mouse_down(MouseButton::Left, |_, window, _| {
            window.prevent_default()
        })
        .on_click(move |_, window, _| match area {
            WindowControlArea::Min => window.minimize_window(),
            WindowControlArea::Max => window.zoom_window(),
            WindowControlArea::Close => window.remove_window(),
            WindowControlArea::Drag => {}
        })
        .child(
            Icon::new(icon)
                .size(IconSize::Sm)
                .color(colors.muted)
                .group_hover_color(group, if close { white() } else { colors.fg }),
        )
}

fn friendly_media_error(message: &str) -> String {
    let text = message.to_lowercase();
    if text.contains("truncated") || text.contains("unexpected end of file") {
        "文件不完整（图片数据被截断）".to_string()
    } else if text.contains("cannot identify image file")
        || text.contains("unidentifiedimageerror")
    {
        "无法识别的图片，文件可能已损坏".to_string()
    } else if text.contains("decompression bomb") {
        "图片尺寸超出安全上限".to_string()
    } else {
        message.to_string()
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.;
    const MB: f64 = KB * 1024.;
    const GB: f64 = MB * 1024.;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes / KB)
    } else {
        format!("{} B", bytes as u64)
    }
}

fn format_date(seconds: u64) -> String {
    if seconds == 0 {
        return "未知".to_string();
    }
    let days = (seconds / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    format!("{year:04}-{month:02}-{day:02}")
}

fn tile(
    asset: Asset,
    selected: bool,
    this: Entity<KanalibApp>,
    thumb_cache: Entity<ThumbCache>,
    width: f32,
    image_height: f32,
    focus: FocusHandle,
    colors: UiColors,
) -> impl IntoElement {
    let path = asset.path.clone();
    let thumb = asset.tile_thumb.clone().or_else(|| asset.thumb.clone());
    let media: AnyElement = if let Some(thumb) = thumb {
        img(thumb)
            .image_cache(&thumb_cache)
            .size_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element()
    } else {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(colors.subtle)
            .child("…")
            .into_any_element()
    };
    let border = if selected {
        colors.accent
    } else {
        colors.border
    };
    let error = asset
        .error
        .clone()
        .map(|message| friendly_media_error(&message));
    let has_error = error.is_some();
    let damage = asset.damage.clone();
    let has_damage = damage.is_some();
    let summary = error.unwrap_or_else(|| asset.summary());
    let title = asset.title();
    let tag_count = asset
        .all_tags()
        .filter(|tag| matches!(tag.category.as_str(), "general" | "manual"))
        .count();
    let rating = asset.rating.to_uppercase();
    let favorite = asset.favorite;
    let star = asset.star;

    let star_menu = {
        let mut menu = Menu::new();
        for (value, label) in [
            (1u8, "1 星"),
            (2, "2 星"),
            (3, "3 星"),
            (4, "4 星"),
            (5, "5 星"),
        ] {
            let this = this.clone();
            let path = path.clone();
            menu = menu.item(MenuItem::new(label).on_click(move |_, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.select_single_path(path.clone(), cx);
                    app.apply_star(value, cx);
                });
            }));
        }
        let this = this.clone();
        let path = path.clone();
        menu.separator()
            .item(MenuItem::new("清除星级").on_click(move |_, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.select_single_path(path.clone(), cx);
                    app.apply_star(0, cx);
                });
            }))
    };
    let menu = {
        let quick_this = this.clone();
        let quick_path = path.clone();
        let favorite_this = this.clone();
        let favorite_path = path.clone();
        let prompt_this = this.clone();
        let prompt_path = path.clone();
        let paths_this = this.clone();
        let paths_path = path.clone();
        let open_file = path.clone();
        let reveal_file = path.clone();
        Menu::new()
            .item(
                MenuItem::new("快速查看")
                    .icon(IconName::Maximize2)
                    .on_click(move |_, cx| {
                        let _ = quick_this.update(cx, |app, cx| {
                            app.select_single_path(quick_path.clone(), cx);
                            app.open_quick_view(cx);
                        });
                    }),
            )
            .item(
                MenuItem::new(if favorite { "取消收藏" } else { "收藏" })
                    .icon(IconName::Heart)
                    .on_click(move |_, cx| {
                        let _ = favorite_this.update(cx, |app, cx| {
                            app.select_single_path(favorite_path.clone(), cx);
                            app.toggle_favorite(cx);
                        });
                    }),
            )
            .item(MenuItem::submenu("星级", star_menu))
            .separator()
            .item(
                MenuItem::new("复制 Prompt")
                    .icon(IconName::ClipboardCheck)
                    .on_click(move |_, cx| {
                        let _ = prompt_this.update(cx, |app, cx| {
                            app.select_single_path(prompt_path.clone(), cx);
                            app.copy_selected_prompt(cx);
                        });
                    }),
            )
            .item(
                MenuItem::new("复制路径")
                    .icon(IconName::Copy)
                    .on_click(move |_, cx| {
                        let _ = paths_this.update(cx, |app, cx| {
                            app.select_single_path(paths_path.clone(), cx);
                            app.copy_selected_paths(cx);
                        });
                    }),
            )
            .separator()
            .item(
                MenuItem::new("打开文件")
                    .icon(IconName::Image)
                    .on_click(move |_, _| open_path(&open_file)),
            )
            .item(
                MenuItem::new("在资源管理器中显示")
                    .icon(IconName::FolderOpen)
                    .on_click(move |_, _| reveal_path(&reveal_file)),
            )
    };
    let this_click = this.clone();
    let path_click = path.clone();
    let focus_click = focus.clone();
    let this_right = this.clone();
    let path_right = path.clone();
    let focus_right = focus.clone();

    let tile = div()
        .id(SharedString::from(format!("tile-{}", asset.path.display())))
        .role(Role::Button)
        .aria_label(title.clone())
        .w(px(width))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(5.))
        .cursor_pointer()
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
        .child(
            div()
                .relative()
                .w_full()
                .h(px(image_height))
                .rounded(px(8.))
                .overflow_hidden()
                .border_1()
                .border_color(border)
                .bg(colors.sunken)
                .child(media)
                .children(selected.then(|| {
                    div()
                        .absolute()
                        .top(px(6.))
                        .left(px(6.))
                        .size(px(20.))
                        .rounded_full()
                        .bg(colors.accent)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Check)
                                .size(IconSize::Xs)
                                .color(colors.bg),
                        )
                }))
                .children(favorite.then(|| {
                    div()
                        .absolute()
                        .top(px(6.))
                        .right(px(6.))
                        .size(px(22.))
                        .rounded_full()
                        .bg(colors.backdrop)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Heart)
                                .size(IconSize::Sm)
                                .color(colors.danger),
                        )
                }))
                .children(has_damage.then(|| {
                    div()
                        .absolute()
                        .left(px(6.))
                        .bottom(px(6.))
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(colors.backdrop)
                        .text_xs()
                        .text_color(colors.warning)
                        .child("损坏")
                })),
        )
        .child(
            div()
                .truncate()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.fg)
                .child(title),
        )
        .child(
            div()
                .truncate()
                .text_xs()
                .text_color(if has_error {
                    colors.danger
                } else if has_damage {
                    colors.warning
                } else {
                    colors.muted
                })
                .child(summary),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(if rating.is_empty() {
                            if star > 0 {
                                format!("★ {star}")
                            } else {
                                "未评级".to_string()
                            }
                        } else if star > 0 {
                            format!("评级 {rating} · ★ {star}")
                        } else {
                            format!("评级 {rating}")
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(format!("{tag_count} 标签")),
                ),
        );

    div()
        .flex_none()
        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
            window.focus(&focus_right, cx);
            let _ = this_right.update(cx, |app, cx| {
                app.select_single_path(path_right.clone(), cx);
            });
        })
        .child(
            ContextMenu::new(
                SharedString::from(format!("tile-context-{}", asset.path.display())),
                menu,
            )
            .child(tile),
        )
}

#[cfg(target_os = "windows")]
fn open_path(path: &PathBuf) {
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}

#[cfg(not(target_os = "windows"))]
fn open_path(path: &PathBuf) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

#[cfg(target_os = "windows")]
fn reveal_path(path: &PathBuf) {
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
}

#[cfg(not(target_os = "windows"))]
fn reveal_path(path: &PathBuf) {
    if let Some(parent) = path.parent() {
        open_path(&parent.to_path_buf());
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "frame-profiler")]
    use super::decode_thumb;
    use super::{Resource, cache_victims, clamp_cache_limit};
    use std::collections::HashMap;
    use std::path::Path;

    fn resource(name: &str) -> Resource {
        Resource::Path(Path::new(name).into())
    }

    #[test]
    fn image_cache_victims_are_least_recently_used_first() {
        let current = resource("a");
        let entries: HashMap<Resource, ((), u64)> = HashMap::from([
            (resource("a"), ((), 3)),
            (resource("b"), ((), 1)),
            (resource("c"), ((), 2)),
        ]);

        assert_eq!(cache_victims(&entries, 3, &current), Vec::new());
        assert_eq!(cache_victims(&entries, 2, &current), vec![resource("b")]);

        let drained = cache_victims(&entries, 1, &current);
        assert_eq!(drained, vec![resource("b"), resource("c")]);
    }

    #[test]
    fn image_cache_limit_is_at_least_one() {
        assert_eq!(clamp_cache_limit(0), 1);
        assert_eq!(clamp_cache_limit(128), 128);
    }

    /// 手动取证：解码 300 张 t256 后立即丢弃，观察 256K 档是否有泄漏。
    #[cfg(feature = "frame-profiler")]
    #[test]
    #[ignore]
    fn decode_does_not_leak_256k_bucket() {
        let library = std::path::Path::new(".kanalib/thumbs");
        let mut files: Vec<std::path::PathBuf> = walkdir::WalkDir::new(library)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.file_type().is_file() && entry.file_name().to_string_lossy().contains("-t256")
            })
            .map(|entry| entry.into_path())
            .take(300)
            .collect();
        files.sort();
        assert!(!files.is_empty(), "stress library with t256 files required");
        let bucket = 18 - 10;
        let before = kanalib_core::alloc_stats::live_bucket_bytes(bucket);
        for path in &files {
            let result = decode_thumb(&Resource::from(path.clone()));
            assert!(result.is_ok(), "decode failed for {}", path.display());
        }
        let after = kanalib_core::alloc_stats::live_bucket_bytes(bucket);
        eprintln!(
            "decode bucket delta: files={} before={before} after={after} delta_kb={}",
            files.len(),
            (after as i64 - before as i64) / 1024,
        );
    }
}
