use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use ely_gpui_component::{
    buttons::{Button, ButtonVariant},
    forms::{Input, TextInput},
    theme::ActiveTheme,
};
use gpui::{
    div, image_cache, img, px, uniform_list, AnyElement, App, Context, Entity, ImageCache,
    ImageCacheError, ImageCacheItem, FontWeight, Hsla, IntoElement, ObjectFit, ParentElement,
    Pixels, Render, RenderImage, Resource, ScrollStrategy, SharedString, Styled, Task,
    UniformListScrollHandle, WeakEntity, Window,
};
use gpui::prelude::*;
#[cfg(feature = "frame-profiler")]
use hdrhistogram::Histogram;

use kanalib_core::dedup::{
    DedupSummary, DuplicateGroup, GroupKind, ReviewState, is_protected,
    similarity_pct_from_distance,
};
use kanalib_core::model::{Asset, Counts, Filter, SearchQuery, TagHit};
use kanalib_core::state::AppState;

const TILE_MIN_WIDTH: f32 = 160.;
const TILE_IMAGE_HEIGHT: f32 = 150.;
const GRID_GAP: f32 = 12.;
const GRID_PADDING: f32 = 16.;
const ROW_PADDING: f32 = 12.;
const ROW_HEIGHT: f32 = 248.;
const DEDUP_RAIL_WIDTH: f32 = 272.;
const DEDUP_GROUP_ROW_HEIGHT: f32 = 78.;
const DEDUP_CARD_WIDTH: f32 = 214.;
const DEDUP_MAX_VISIBLE: usize = 4;
const SETTINGS_MAX_WIDTH: f32 = 1080.;

fn grid_columns(available: f32) -> usize {
    let usable = (available - GRID_PADDING * 2.0 + GRID_GAP) / (TILE_MIN_WIDTH + GRID_GAP);
    (usable.floor() as usize).max(1)
}

fn grid_tile_width(available: f32, columns: usize) -> f32 {
    let usable = available - GRID_PADDING * 2.0 - GRID_GAP * columns.saturating_sub(1) as f32;
    (usable / columns.max(1) as f32).max(1.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Library,
    Duplicates,
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
fn cache_victims<T>(    entries: &HashMap<Resource, (T, u64)>,
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
    let bytes = std::fs::read(path.as_ref()).map_err(|error| ImageCacheError::Io(Arc::new(error)))?;
    let image = image::load_from_memory(&bytes).map_err(|error| ImageCacheError::Image(Arc::new(error)))?;
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
    last_search: String,
    search_paths: Option<Arc<HashSet<PathBuf>>>,
    last_scanning: bool,
    view: ViewCache,
    grid_rows: usize,
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
    #[cfg(feature = "frame-profiler")]
    frame_probe: Option<FrameProbe>,
    #[cfg(feature = "frame-profiler")]
    probe_watch: Option<Task<()>>,
}

impl KanalibApp {
    pub fn new(folder: PathBuf, cx: &mut Context<Self>) -> Self {
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
        Self {
            state: AppState::new(folder),
            grid_scroll: UniformListScrollHandle::new(),
            dedup_scroll: UniformListScrollHandle::new(),
            last_search: String::new(),
            search_paths: None,
            last_scanning: false,
            view: ViewCache::default(),
            grid_rows: 0,
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
                    (
                        shared.status.clone(),
                        shared.scanning,
                        shared.revision,
                    )
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
                            app.frame_probe.as_ref().map_or(-1i64, |probe| probe.renders as i64)
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
            let Some(handle) = rfd::AsyncFileDialog::new().pick_folder().await else {
                return;
            };
            let folder = handle.path().to_path_buf();
            let _ = weak.update(cx, |app, cx| {
                app.state.set_folder(folder);
                app.state.start_scan();
                app.last_search.clear();
                app.search_paths = None;
                app.screen = Screen::Library;
                app.dedup_selected = None;
                app.dedup_confirm = None;
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
}

impl Render for KanalibApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_metrics = std::env::var_os("KANALIB_UI_METRICS").is_some();
        let render_started = Instant::now();
        let colors = UiColors::from_theme(cx);
        let this = cx.entity();
        let search = window.use_keyed_state("kanalib-search", cx, |window, cx| {
            TextInput::new(window, cx).placeholder("搜索文件名 / 标签")
        });
        let search_text = search.read(cx).text().to_string();
        let search_query = SearchQuery::parse(&search_text);
        // gpui-pre 的 viewport_size 自 0.3.x 起就是逻辑像素，不能再按 DPI 除一次。
        let scale = window.scale_factor();
        let viewport = window.viewport_size();
        let window_width = viewport.width;
        let window_height = viewport.height;
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
            folder,
            status,
            error,
            scanning,
            device,
            queue_pending,
            queue_running,
            queue_error,
            filter,
            selected,
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
        ) = {
            let shared = self.state.shared.lock().unwrap();
            let revision = shared.revision;
            let counts_view = (revision != self.view.counts_revision).then(|| shared.counts.view());
            (
                shared.folder.clone(),
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
            self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        let needs_visible = revision != self.view.visible_revision
            || self.view.visible_filter.as_ref() != Some(&filter)
            || self.view.visible_search != search_text
            || self.view.visible_search_revision != self.view.search_revision;
        if needs_visible {
            let started = Instant::now();
            let duplicate_paths = dedup_exact_groups
                .iter()
                .chain(dedup_similar_groups.iter())
                .flat_map(|group| group.assets.iter().map(|asset| asset.path.clone()))
                .collect::<HashSet<PathBuf>>();
            let indices = {
                let assets = assets.read().unwrap();
                assets
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
                    .collect::<Vec<_>>()
            };
            let elapsed_us = started.elapsed().as_secs_f64() * 1_000_000.0;
            self.view.visible_indices = Arc::new(indices);
            self.view.visible_revision = revision;
            self.view.visible_filter = Some(filter.clone());
            self.view.visible_search = search_text;
            self.view.visible_search_revision = self.view.search_revision;
            if ui_metrics {
                log::info!(
                    "ui visible rebuild: us={elapsed_us:.0} visible={} revision={revision}",
                    self.view.visible_indices.len()
                );
            }
        }
        let visible_indices = self.view.visible_indices.clone();
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
        let columns = grid_columns(main_width.as_f32());
        self.grid_rows = visible_indices.len().div_ceil(columns);
        let selected_asset = {
            let assets = assets.read().unwrap();
            selected.as_ref().and_then(|path| {
                visible_indices
                    .iter()
                    .find(|&&index| &assets[index].path == path)
                    .map(|&index| assets[index].clone())
            })
        };

        let sidebar = self.render_sidebar(
            &this,
            &folder,
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
            colors,
        );
        let content: AnyElement = if self.screen == Screen::Settings {
            let settings_width = (window_width - sidebar_width).max(px(640.));
            self.render_settings(
                &this,
                &folder,
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
                window_height,
                colors,
            )
            .into_any_element()
        } else {
            self.render_main(
                &this,
                &filter,
                &search,
                assets,
                visible_indices,
                selected,
                error.as_deref(),
                scanning,
                main_width,
                colors,
            )
            .into_any_element()
        };
        let inspector = (self.screen == Screen::Library).then(|| {
            self.render_inspector(&this, selected_asset.as_ref(), inspector_width, colors)
                .into_any_element()
        });

        let shell = div()
            .flex()
            .flex_row()
            .w_full()
            .h_full()
            .bg(colors.bg)
            .text_color(colors.fg)
            .font_family(cx.theme().font_family.clone())
            .child(sidebar)
            .child(content)
            .children(inspector);
        if ui_metrics {
            log::info!(
                "ui render build: ms={:.2}",
                render_started.elapsed().as_secs_f64() * 1000.0
            );
        }
        // 整棵树共用一个有界图片缓存：子树里所有 img 都走它，不再进无上限的全局 asset cache。
        image_cache(self.thumb_cache.clone()).size_full().child(shell)
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
    let presents = snapshot.present_interval_histogram.len().saturating_sub(baseline_presents);
    let seconds = elapsed.as_secs_f64();
    let viewport = window.viewport_size();
    let over_budget = 100. - snapshot.present_interval_histogram.percentile_below(16_666_666);
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
        match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
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
        folder: &PathBuf,
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
            for (tag, count) in counts.tags.iter() {
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
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted)
                            .child(folder.display().to_string()),
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
                        Button::new("choose-folder", "选择文件夹")
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
                    .child(if scanning {
                        Button::new("stop", "停止").on_click(move |_, _, cx| {
                            let _ = this_stop.update(cx, |app, cx| {
                                app.state.stop();
                                cx.notify();
                            });
                        })
                    } else {
                        Button::new("stop", "停止")
                    }),
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

    fn render_main(
        &self,
        this: &Entity<Self>,
        filter: &Filter,
        search: &Entity<TextInput>,
        assets: Arc<RwLock<Vec<Asset>>>,
        visible_indices: Arc<Vec<usize>>,
        selected: Option<PathBuf>,
        error: Option<&str>,
        scanning: bool,
        width: Pixels,
        colors: UiColors,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let this_clear = this.clone();
        let this_stop = this.clone();
        let clear = (*filter != Filter::All).then(|| {
            Button::new("clear-filter", "清除筛选").on_click(move |_, _, cx| {
                let _ = this_clear.update(cx, |app, cx| {
                    app.set_filter(Filter::All, cx);
                });
            })
        });
        let stop = scanning.then(|| {
            Button::new("stop-main", "停止").on_click(move |_, _, cx| {
                let _ = this_stop.update(cx, |app, cx| {
                    app.state.stop();
                    cx.notify();
                });
            })
        });

        let columns = grid_columns(width.as_f32());
        let tile_width = grid_tile_width(width.as_f32(), columns);
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
                        .child(if scanning {
                            "正在扫描图片..."
                        } else {
                            "没有符合筛选的图片。"
                        }),
                )
                .into_any_element()
        } else {
            let state_for_list = state.clone();
            let this_for_list = this.clone();
            let thumb_cache_for_list = self.thumb_cache.clone();
            let selected_for_list = selected.clone();
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
                            .h(px(ROW_HEIGHT))
                            .flex()
                            .gap(px(GRID_GAP))
                            .px(px(GRID_PADDING))
                            .pt(px(ROW_PADDING))
                            .pb(px(ROW_PADDING));
                        for &asset_index in &rows_for_list[start..end] {
                            let asset = &assets[asset_index];
                            let is_selected = selected_for_list
                                .as_ref()
                                .is_some_and(|path| path == &asset.path);
                            line = line.child(tile(
                                asset.clone(),
                                is_selected,
                                state_for_list.clone(),
                                this_for_list.clone(),
                                thumb_cache_for_list.clone(),
                                tile_width,
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
                                    .child(filter.label()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.muted)
                                    .child(format!("{} 张可见", visible)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(px(320.))
                                    .child(Input::new(search).clearable().least("搜索文件名 / 标签")),
                            )
                            .children(clear)
                            .children(stop),
                    ),
            )
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
            .child(grid)
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
        height: Pixels,
        colors: UiColors,
    ) -> impl IntoElement {
        let similar_mode = self.dedup_mode == GroupKind::Similar;
        let has_banner = error.is_some() || self.dedup_notice.is_some();
        let chrome = 104.
            + if similar_mode { 48. } else { 0. }
            + if has_banner { 30. } else { 0. };
        let body_height = (height - px(chrome)).max(px(480.));
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
        if self
            .dedup_selected
            .as_ref()
            .is_none_or(|key| !visible_indices.iter().any(|&index| &groups[index].key == key))
        {
            self.dedup_selected = visible_indices.first().map(|&index| groups[index].key.clone());
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
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("重复项"))
                            .child(tabs),
                    )
                    .child(
                        div()
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
                                        format!("已按规则预选 {selected} 组；移入回收站前仍会汇总确认")
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
                    .child(Button::new("dedup-library", "返回图库").on_click(move |_, _, cx| {
                        let _ = this_library.update(cx, |app, cx| {
                            app.screen = Screen::Library;
                            app.dedup_confirm = None;
                            cx.notify();
                        });
                    })),
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
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(format!(
                            "当前 ≤{} · 约 {}% 相似",
                            similar_threshold,
                            similarity_pct_from_distance(similar_threshold as u32)
                        )),
                )
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
            .child(
                div()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(if busy {
                        "扫描中，完成后更新分组"
                    } else if similar_mode {
                        "相似组只逐组处理，不提供批量删除"
                    } else {
                        "先预选，再逐组确认"
                    }),
            );

        let rail_header = div()
            .px(px(12.))
            .py(px(9.))
            .border_b_1()
            .border_color(colors.border)
            .flex()
            .items_center()
            .justify_between()
            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).child("组"))
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
                    .flex_none()
                    .h(body_height)
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
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted)
                            .child(format!(
                                "{} · 可释放 {} · 当前保留：{}",
                                group.suggested_reason,
                                format_bytes(reclaimable),
                                keeper.map(Asset::title).unwrap_or_else(|| "未选择".into())
                            )),
                    ),
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
            group.assets.iter().take(DEDUP_MAX_VISIBLE).collect::<Vec<_>>()
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
                .child(Button::new("dedup-ignore", "不是重复").disabled(scanning).on_click({
                    let key = key.clone();
                    move |_, _, cx| {
                        let _ = this_ignore.update(cx, |app, cx| {
                            let groups = groups_ignore.clone();
                            match app.state.mark_dedup(&key, ReviewState::Ignored, group_kind) {
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
                }))
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
                                            app.dedup_notice =
                                                Some("已标记为相关变体".into());
                                            app.advance_dedup(&groups, true);
                                        }
                                        Err(error) => {
                                            app.dedup_notice =
                                                Some(format!("操作失败: {error:#}"));
                                        }
                                    }
                                    cx.notify();
                                });
                            }
                        })
                }))
                .child(Button::new("dedup-later", "稍后").on_click(move |_, _, cx| {
                    let _ = this_later.update(cx, |app, cx| {
                        app.advance_dedup(&groups_later, true);
                        cx.notify();
                    });
                }))
                .child(Button::new("dedup-keep-all", "全部保留").disabled(scanning).on_click({
                    let key = key.clone();
                    move |_, _, cx| {
                        let _ = this_keep.update(cx, |app, cx| {
                            let groups = groups_keep.clone();
                            match app.state.mark_dedup(&key, ReviewState::Done, group_kind) {
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
                }))
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
                            match app.state.mark_dedup(
                                &key,
                                ReviewState::Pending,
                                group_kind,
                            ) {
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
            .h(px(62.))
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
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
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
            .child(actions);

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
                                    .child(div().truncate().text_xs().text_color(colors.fg).child(name.clone()))
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
                            .child(Button::new("dedup-cancel", "取消").on_click(
                                move |_, _, cx| {
                                    let _ = this_cancel.update(cx, |app, cx| {
                                        app.dedup_confirm = None;
                                        cx.notify();
                                    });
                                },
                            ))
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

    fn render_settings(
        &self,
        this: &Entity<Self>,
        folder: &PathBuf,
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
        let this_back = this.clone();
        let this_add = this.clone();
        let db_path = self.state.db_path();
        let cache_dir = self.state.cache_dir();
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
                    .px(px(24.))
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
                                SharedString::from(format!(
                                    "remove-protected-{}",
                                    path.display()
                                )),
                                "移除",
                            )
                            .on_click(move |_, _, cx| {
                                let path = path.clone();
                                let _ = this_remove.update(cx, |app, cx| {
                                    match app.state.remove_protected_path(&path) {
                                        Ok(()) => {
                                            app.settings_notice = Some(format!(
                                                "已移除保护路径：{}",
                                                path.display()
                                            ));
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
                                let Some(handle) =
                                    rfd::AsyncFileDialog::new().pick_folder().await
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
        for (value, label) in [(6u8, "严格 ≤6"), (10u8, "标准 ≤10"), (14u8, "宽松 ≤14")] {
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
                        app.settings_notice =
                            Some(format!("相似阈值默认值已设为 ≤{saved}"));
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
                    .child("数据位置"),
            )
            .child(path_row("图库", folder.clone(), colors))
            .child(path_row("数据库", db_path, colors))
            .child(path_row("缩略图缓存", cache_dir, colors))
            .child(
                div().flex().child(
                    Button::new("open-data-dir", "打开数据目录").on_click(move |_, _, _| {
                        open_path(&data_dir_for_open);
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
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child(format!(
                        "等待 {queue_pending} · 运行 {queue_running} · 失败 {queue_error}"
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child("打标任务持久化在 jobs 表；重启后自动恢复未完成任务，失败项可用侧栏“重试失败”。"),
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

        let column_width = (width - px(48.)).min(px(SETTINGS_MAX_WIDTH));
        let side_padding = px(((width.as_f32() - column_width.as_f32()) / 2.0).max(24.));

        div()
            .w(width)
            .h_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .child(
                div()
                    .h(px(58.))
                    .px(side_padding)
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
                    .child(Button::new("settings-back", "返回图库").on_click(
                        move |_, _, cx| {
                            let _ = this_back.update(cx, |app, cx| {
                                app.screen = Screen::Library;
                                cx.notify();
                            });
                        },
                    )),
            )
            .children(banner)
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .px(side_padding)
                    .py(px(24.))
                    .child(
                        div()
                            .w(column_width)
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .child(protected_section)
                            .child(similar_section)
                            .child(data_section)
                            .child(safety_section)
                            .child(rules_section),
                    ),
            )
            .into_any_element()
    }

    fn render_inspector(
        &self,
        this: &Entity<Self>,
        selected: Option<&Asset>,
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

            let mut tags = div()
                .id("inspector-tags")
                .flex()
                .flex_col()
                .gap(px(2.));
            for category in ["general", "character", "copyright", "style", "meta", "rating"] {
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
                    let state = state.clone();
                    let this = this.clone();
                    let tag = hit.tag.clone();
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
                                state.set_filter(filter.clone());
                                let _ = this.update(cx, |app, cx| {
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
                                    .text_xs()
                                    .text_color(colors.subtle)
                                    .child(format!("{score:.3}")),
                            ),
                    );
                }
            }

            let path = asset.path.clone();
            let open = Button::new("open-file", "打开文件").on_click(move |_, _, _| {
                open_path(&path);
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
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.muted)
                        .child(format!(
                            "{} × {} · {} · {}",
                            asset.width,
                            asset.height,
                            if asset.rating.is_empty() {
                                "未评级"
                            } else {
                                asset.rating.as_str()
                            },
                            asset.group
                        )),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(match (&asset.sha1, &asset.blake3, asset.phash) {
                            (Some(sha1), Some(blake3), Some(phash)) => format!(
                                "SHA1 {} · B3 {} · pHash {phash:016x}",
                                &sha1[..12.min(sha1.len())],
                                &blake3[..8.min(blake3.len())]
                            ),
                            _ => "内容哈希未计算".to_string(),
                        }),
                )
                .children(asset.error.as_ref().map(|message| {
                    div()
                        .text_xs()
                        .text_color(colors.danger)
                        .child(message.clone())
                }))
                .child(open)
                .child(
                    div()
                        .mt(px(4.))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.subtle)
                        .child("标签（点击筛选）"),
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

fn filter_row(
    id: SharedString,
    label: String,
    count: usize,
    active: bool,
    filter: Filter,
    state: Arc<AppState>,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> impl IntoElement {
    let mut row = div()
        .id(id)
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
        state.set_filter(filter.clone());
        let _ = this.update(cx, |app, cx| {
            app.screen = Screen::Library;
            app.dedup_confirm = None;
            cx.notify();
        });
    })
    .child(
        div()
            .text_xs()
            .text_color(colors.fg)
            .child(label),
    )
    .child(
        div()
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
) -> impl IntoElement {    let mut row = div()
        .id(id)
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

fn settings_row(
    id: SharedString,
    label: &'static str,
    active: bool,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> impl IntoElement {
    let mut row = div()
        .id(id)
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
        .id(SharedString::from(format!("dedup-filter-{}", filter.label())))
        .px(px(9.))
        .py(px(4.))
        .rounded(px(5.))
        .cursor_pointer()
        .bg(if active { colors.selection } else { colors.sunken })
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
        .bg(if active { colors.surface } else { colors.sunken })
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
        GroupKind::Exact => ("完全相同".to_string(), colors.success, colors.success_subtle),
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
                                .child(format!("可释放 {reclaimable} · {}", group.suggested_reason)),
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
        badge = badge.child(small_badge("保留".into(), colors.success, colors.success_subtle));
    }
    if is_suggested && !is_keeper {
        badge = badge.child(small_badge("推荐".into(), colors.warning, colors.warning_subtle));
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
                    app.state.set_dedup_keeper(&group_key, path.clone(), group_kind);
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

    card = card.child(
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
                        asset.tags.len()
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
                    format!("{} 个", asset.tags.len())
                } else {
                    format!("{} 个 · {top}", asset.tags.len())
                }
            })
            .collect(),
        best_max(assets, |asset| asset.tags.len()),
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
                group
                    .distance_to_keeper(asset)
                    .unwrap_or(u32::MAX)
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
        .child(format!("建议保留：{}", group.suggested_asset().map(Asset::title).unwrap_or_default()))
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
                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).child("差异"))
                .child(div().text_xs().text_color(colors.subtle).child("只显示不同字段")),
        )
        .child(recommendation)
        .child(table)
        .child(
            div()
                .text_xs()
                .text_color(colors.subtle)
                .child(if group.kind == GroupKind::Similar {
                    "相似度按 pHash 汉明距离换算；组内可能链式合并，请逐组确认。"
                } else {
                    "字节相同：重点看路径、日期和标签；分辨率/大小通常相同。"
                }),
        )
        .into_any_element()
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
    state: Arc<AppState>,
    this: Entity<KanalibApp>,
    thumb_cache: Entity<ThumbCache>,
    width: f32,
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
    let border = if selected { colors.accent } else { colors.border };
    let error = asset.error.clone();
    let has_error = error.is_some();
    let summary = error.unwrap_or_else(|| asset.summary());
    let title = asset.title();
    let tag_count = asset
        .tags
        .iter()
        .filter(|tag| tag.category == "general")
        .count();
    let rating = asset.rating.to_uppercase();

    div()
        .id(SharedString::from(format!(
            "tile-{}",
            asset.path.display()
        )))
        .w(px(width))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(5.))
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            state.select(Some(path.clone()));
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .child(
            div()
                .w_full()
                .h(px(TILE_IMAGE_HEIGHT))
                .rounded(px(8.))
                .overflow_hidden()
                .border_1()
                .border_color(border)
                .bg(colors.sunken)
                .child(media),
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
                .text_color(if has_error { colors.danger } else { colors.muted })
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
                            "未评级".to_string()
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

#[cfg(test)]
mod tests {
    use super::{Resource, cache_victims, clamp_cache_limit};
    #[cfg(feature = "frame-profiler")]
    use super::decode_thumb;
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
                entry.file_type().is_file()
                    && entry
                        .file_name()
                        .to_string_lossy()
                        .contains("-t256")
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
