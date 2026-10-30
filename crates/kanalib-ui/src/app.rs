use crate::*;
use crate::format::*;
use crate::search::{SearchBundle, query_wants_duplicates, search_timing_enabled};
use crate::settings::parse_optional_bytes;
use crate::sidebar::{
    SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, SIDEBAR_SECTIONS, source_label,
    source_menu,
};
use crate::thumb_cache::IMAGE_CACHE_LIMIT;
#[cfg(feature = "frame-profiler")]
use crate::thumb_cache::watch_enabled;
use crate::theme::*;
use crate::viewer::VIEWER_CONTEXT;
use crate::views::{GRID_CONTEXT, grid_columns};
use gpui::ExternalPaths;
use kanalib_core::model::{ExcludedAssetAction, SavedSearchView, ScanFilterImpact, is_image};

pub(crate) fn rating_rank(rating: &str) -> u64 {
    match rating.to_ascii_lowercase().as_str() {
        "g" => 1,
        "s" => 2,
        "q" => 3,
        "e" => 4,
        _ => 0,
    }
}

pub(crate) fn prompt_for(asset: &Asset) -> String {
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

pub(crate) fn push_toast(
    toaster: &Entity<Toaster>,
    cx: &mut App,
    message: impl Into<SharedString>,
    severity: Severity,
) {
    toaster.update(cx, |toaster, cx| {
        toaster.push(Toast::new(message).severity(severity), cx);
    });
}

#[derive(Default)]
pub(crate) struct ViewCache {
    pub(crate) counts_revision: u64,
    pub(crate) counts: Counts,
    pub(crate) search_revision: u64,
    pub(crate) visible_revision: u64,
    pub(crate) visible_filter: Option<Filter>,
    pub(crate) visible_search: String,
    pub(crate) visible_search_revision: u64,
    pub(crate) visible_sort: SortMode,
    pub(crate) visible_sort_ascending: bool,
    pub(crate) visible_indices: Arc<Vec<usize>>,
}

/// 一次搜索的端到端计时探针；只在 `KANALIB_SEARCH_TIMING` 下创建。
pub(crate) struct SearchFrameProbe {
    input_at: Instant,
    visible_ms: f64,
    run_ms: f64,
    hits: usize,
}

/// 合成器帧计时探针。`KANALIB_FRAME_METRICS=<秒>` 开启：导入结束后每帧请求一次动画帧，
/// 到时把 gpui profiler 的直方图汇总成一行 JSON。
#[cfg(feature = "frame-profiler")]
pub(crate) struct FrameProbe {
    duration: Duration,
    started_at: Instant,
    /// 至少观察到一次 scanning=true，避免在 start_scan 置位前的首帧就起跑。
    armed: bool,
    baseline_presents: Option<u64>,
    renders: usize,
}

pub struct KanalibApp {
    pub(crate) state: Arc<AppState>,
    pub(crate) grid_scroll: UniformListScrollHandle,
    pub(crate) dedup_scroll: UniformListScrollHandle,
    pub(crate) tag_scroll: UniformListScrollHandle,
    pub(crate) tag_list: Vec<kanalib_core::model::TagSummary>,
    pub(crate) tag_vocab: Vec<kanalib_core::model::TagSummary>,
    pub(crate) tag_vocab_revision: u64,
    pub(crate) tag_pane_category: String,
    pub(crate) tag_pane_expanded: bool,
    pub(crate) tag_pane_sort_name: bool,
    pub(crate) tag_suggestions: Vec<(String, usize)>,
    pub(crate) cache_stats: Option<CacheStats>,
    pub(crate) thumb_budget_gb: u64,
    pub(crate) cache_last_cleanup: Option<String>,
    pub(crate) cache_budget_checked: bool,
    pub(crate) last_search: String,
    /// 搜索框当前文本；与 `last_search` 不同说明还在防抖窗口内。
    pub(crate) search_pending: String,
    pub(crate) search_bundle: Option<Arc<SearchBundle>>,
    pub(crate) search_debounce_task: Option<Task<()>>,
    /// 输入文本变化后第一次排入防抖的时刻；端到端计时用。
    pub(crate) search_queued_at: Option<Instant>,
    /// 搜索可见索引重建后的下一帧探针；只读计时，不参与渲染结果。
    pub(crate) search_frame_probe: Option<SearchFrameProbe>,
    /// 已排入一次“数据变了要重查”的刷新；避免每次渲染都重置防抖计时。
    pub(crate) search_refresh_queued: bool,
    pub(crate) search_running: bool,
    pub(crate) last_scanning: bool,
    pub(crate) view: ViewCache,
    pub(crate) grid_rows: usize,
    pub(crate) grid_columns: usize,
    pub(crate) screen: Screen,
    pub(crate) dedup_mode: GroupKind,
    pub(crate) dedup_filter: DedupFilter,
    pub(crate) dedup_selected: Option<String>,
    pub(crate) dedup_show_all: bool,
    pub(crate) dedup_confirm: Option<DedupConfirm>,
    pub(crate) dedup_notice: Option<String>,
    pub(crate) rescan_confirm: Option<RescanOrigin>,
    pub(crate) settings_notice: Option<String>,
    pub(crate) tag_thresholds: Option<TagThresholds>,
    pub(crate) tag_queue_status: Option<TagQueueStatus>,
    pub(crate) tag_queue_signature: Option<String>,
    pub(crate) tag_job_errors: Vec<TagJobError>,
    pub(crate) tag_cpu_allowed: bool,
    pub(crate) tag_settings_notice: Option<String>,
    pub(crate) tag_recompute_running: bool,
    pub(crate) tag_retag_running: bool,
    pub(crate) tag_retag_confirm: Option<TagRetagConfirm>,
    pub(crate) tag_reviews: Vec<TagCandidateReview>,
    pub(crate) tag_review_notice: Option<String>,
    pub(crate) tag_review_running: bool,
    pub(crate) source_statuses: Vec<SourceScanStatus>,
    pub(crate) last_backup: Option<String>,
    pub(crate) backup_running: bool,
    /// 已暂存、等待重启应用的恢复请求（启动/恢复后同步一次，避免底栏每帧读库）。
    pub(crate) pending_restore_hint: Option<String>,
    pub(crate) diagnostic_snapshot: Option<DiagnosticSnapshot>,
    /// F-038：窗口状态（位置/大小/最大化/屏幕）的落盘根目录与节流保存状态。
    pub(crate) window_state_root: PathBuf,
    pub(crate) window_state_pending: Option<crate::platform::WindowState>,
    pub(crate) window_state_task: Option<Task<()>>,
    pub(crate) window_state_subscription: Option<gpui::Subscription>,
    pub(crate) window_state_quit: Option<gpui::Subscription>,
    pub(crate) refresh_task: Option<Task<()>>,
    pub(crate) thumb_cache: Entity<ThumbCache>,
    pub(crate) cache_task: Option<Task<()>>,
    pub(crate) scroll_task: Option<Task<()>>,
    pub(crate) similar_task: Option<Task<()>>,
    pub(crate) grid_focus: FocusHandle,
    pub(crate) viewer_focus: FocusHandle,
    pub(crate) focus_grid_pending: bool,
    pub(crate) focus_viewer_pending: bool,
    pub(crate) viewer_original: bool,
    pub(crate) auto_select_pending: bool,
    pub(crate) selection: Arc<HashSet<PathBuf>>,
    pub(crate) selection_anchor: Option<PathBuf>,
    pub(crate) sort: SortMode,
    pub(crate) sort_ascending: bool,
    pub(crate) view_mode: ViewMode,
    pub(crate) pane_mode: PaneMode,
    pub(crate) show_sidebar: bool,
    pub(crate) sidebar_width: f32,
    pub(crate) sidebar_collapsed: HashSet<String>,
    pub(crate) sidebar_resizing: bool,
    pub(crate) sidebar_persist_task: Option<Task<()>>,
    pub(crate) show_extensions: bool,
    pub(crate) pending_scroll_position: Option<usize>,
    pub(crate) search_input: Option<Entity<TextInput>>,
    pub(crate) palette_open: bool,
    pub(crate) viewer_slides: Vec<(PathBuf, SharedString)>,
    pub(crate) viewer_at: Option<usize>,
    pub(crate) viewer_viewed: HashSet<PathBuf>,
    pub(crate) viewer_origin_filter: Option<Filter>,
    pub(crate) viewer_origin_search: String,
    pub(crate) save_search_open: bool,
    pub(crate) tag_dialog: Option<TagDialogMode>,
    pub(crate) tag_dialog_epoch: u64,
    pub(crate) tag_dialog_paths: Vec<PathBuf>,
    pub(crate) tag_rename: Option<String>,
    pub(crate) tag_rename_epoch: u64,
    pub(crate) tag_merge: Option<String>,
    pub(crate) tag_merge_epoch: u64,
    pub(crate) source_remove_confirm: Option<PathBuf>,
    pub(crate) source_rename: Option<PathBuf>,
    pub(crate) source_rename_epoch: u64,
    pub(crate) source_menu: Option<SourceMenuState>,
    pub(crate) source_menu_epoch: u64,
    pub(crate) toaster: Entity<Toaster>,
    pub(crate) theme_dark: bool,
    #[cfg(feature = "frame-profiler")]
    pub(crate) frame_probe: Option<FrameProbe>,
    #[cfg(feature = "frame-profiler")]
    pub(crate) probe_watch: Option<Task<()>>,
    // ---- F-032 / F-036 / F-037（第三轮 G3）：全部加在结构体末尾，便于并行合并。 ----
    /// 设置页正在编辑的来源排除规则草稿；保存前不影响扫描。
    pub(crate) scan_filter_draft: Option<ScanFilterDraft>,
    /// 「从图库移除」的二次确认；确认后只删 DB 记录。
    pub(crate) remove_assets_confirm: Option<RemoveAssetsRequest>,
    /// 拖拽导入待确认的路径（文件夹/来源外的图片父目录）。
    pub(crate) drop_prompt: Option<DropPrompt>,
    /// 外部文件拖拽进入窗口时的高亮遮罩；离开/释放后立即清除。
    pub(crate) drop_highlight: bool,
    /// 组织对象（智能集合重命名 / 相册增删改）弹窗状态；见 `sidebar::render_org_dialog`。
    pub(crate) org_dialog: Option<crate::sidebar::OrgDialog>,
    pub(crate) org_dialog_epoch: u64,
    /// 相册列表与归属缓存；侧栏/检查器只读，按 `shared.revision` 失效。
    pub(crate) album_cache: std::cell::RefCell<crate::sidebar::AlbumCache>,
    // ---- F-005（第三轮 G2b1）：分组浏览状态，全部追加在末尾。 ----
    /// 当前分组维度（`ui.group_by`，默认关闭）。
    pub(crate) group_by: GroupMode,
    /// 已折叠的分组键；不持久化。
    pub(crate) collapsed_groups: HashSet<String>,
    /// 分组布局缓存；只在数据版本/维度/列数/折叠变化时重建。
    pub(crate) group_cache: crate::views::GroupCache,
    /// 折叠后待定位的组；布局重建后把该组组头滚到顶部。
    pub(crate) pending_reveal_group: Option<String>,
    // ---- F-004 / F-045（第三轮 G2）：相似结果与阈值预览，字段统一加在末尾。 ----
    /// 当前单锚点相似查询；进入新查询时整体替换，离开视图即丢弃。
    pub(crate) similar_view: Option<crate::dedup::SimilarView>,
    /// 结果视图内的本地筛选输入（名称/标签/路径）。
    pub(crate) similar_filter_input: Option<Entity<TextInput>>,
    pub(crate) similar_filter: String,
    /// 当前查询结果映射到资产下标；只在查询/筛选/数据版本变化时重建。
    pub(crate) similar_visible: Arc<Vec<usize>>,
    pub(crate) similar_visible_revision: u64,
    pub(crate) similar_visible_filter: String,
    pub(crate) similar_refresh_task: Option<Task<()>>,
    /// 设置页与去重页的临时阈值；None 表示沿用已保存的默认值。
    pub(crate) settings_threshold_draft: Option<u8>,
    pub(crate) dedup_threshold_draft: Option<u8>,
    /// 阈值预览结果；`similar_preview_threshold` 标记它对应的候选值。
    pub(crate) similar_preview: Option<kanalib_core::model::SimilarThresholdPreview>,
    pub(crate) similar_preview_threshold: Option<u8>,
    pub(crate) similar_preview_running: bool,
    pub(crate) similar_preview_epoch: u64,
    pub(crate) similar_preview_task: Option<Task<()>>,
    // ---- F-003（第三轮 G2b2）：图片对比状态，字段全部追加在末尾。 ----
    /// 当前对比的 2–4 张；进入时克隆一次，离开即清空，不新开缓存。
    pub(crate) compare_items: Vec<Asset>,
    /// 进入对比前的视图与滚动偏移；Esc 返回时恢复。
    pub(crate) compare_origin: Screen,
    pub(crate) compare_origin_scroll: Option<gpui::Point<gpui::Pixels>>,
    /// 所有栏共享的缩放倍数（1 = 适配）与平移像素。
    pub(crate) compare_zoom: f32,
    pub(crate) compare_pan: (f32, f32),
    /// 仅 UI 的保留标记；不写库、不动文件。
    pub(crate) compare_keeper: Option<PathBuf>,
    pub(crate) compare_original: bool,
    pub(crate) compare_focus: FocusHandle,
    pub(crate) compare_focus_pending: bool,
    /// 自动化截图用：启动后预选可见列表前 N 张（PostMessage 无法合成 Ctrl+点击）。
    pub(crate) demo_select_first: Option<usize>,
}

impl Render for KanalibApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_metrics = std::env::var_os("KANALIB_UI_METRICS").is_some();
        let render_started = Instant::now();
        let colors = UiColors::from_theme(cx);
        let this = cx.entity();
        let search = window.use_keyed_state("kanalib-search", cx, |window, cx| {
            TextInput::new(window, cx).placeholder("搜索 / tag: / character: / size:>10mb")
        });
        if self.search_input.is_none() {
            self.search_input = Some(search.clone());
        }
        let input_text = search.read(cx).text().to_string();
        if input_text != self.search_pending {
            self.search_pending = input_text;
            self.schedule_search(cx);
        }
        // F-004：结果视图内的本地筛选输入；变化时只重建结果下标，不触发图库查询。
        let similar_filter = window.use_keyed_state("kanalib-similar-filter", cx, |window, cx| {
            TextInput::new(window, cx).placeholder("筛选相似结果（名称/标签/路径）")
        });
        if self.similar_filter_input.is_none() {
            self.similar_filter_input = Some(similar_filter.clone());
        }
        let similar_filter_text = similar_filter.read(cx).text().to_string();
        if similar_filter_text != self.similar_filter {
            self.similar_filter = similar_filter_text;
            self.similar_visible_revision = u64::MAX;
            // 筛选变化后隐藏的选中项不应继续计数；下一帧自动选中首个可见结果。
            self.clear_multi_selection();
            self.auto_select_pending = self.screen == Screen::Similar;
        }
        // 结果由“已应用”的查询驱动；输入框还在防抖窗口内时先显示上一次的结果。
        let search_text = self.last_search.clone();
        let mut search_query = SearchQuery::parse(&search_text);
        let tag_aliases = self.state.shared.lock().unwrap().tag_aliases.clone();
        search_query.resolve_tag_aliases(&tag_aliases);
        let search_warnings = search_query.warnings().to_vec();
        // gpui-pre 的 viewport_size 自 0.3.x 起就是逻辑像素，不能再按 DPI 除一次。
        let scale = window.scale_factor();
        let viewport = window.viewport_size();
        let window_width = viewport.width;
        let inspector_width = px(self.pane_mode.width());
        let panel_width = if self.show_sidebar {
            let available = (window_width - px(ACTIVITY_BAR_WIDTH) - inspector_width - px(340.))
                .max(px(SIDEBAR_MIN_WIDTH));
            px(self
                .sidebar_width
                .min(available.as_f32())
                .max(SIDEBAR_MIN_WIDTH))
        } else {
            px(0.)
        };
        let sidebar_width = if self.show_sidebar {
            px(ACTIVITY_BAR_WIDTH) + panel_width
        } else {
            px(0.)
        };
        let mut main_width = window_width - sidebar_width - inspector_width;
        if main_width < px(340.) {
            main_width = px(340.);
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
            pending_cleanup,
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
                shared.pending_cleanup.clone(),
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
        if !scanning && self.tag_vocab_revision != revision {
            match self.state.tag_vocabulary() {
                Ok(vocabulary) => {
                    self.tag_vocab = vocabulary;
                    self.tag_vocab_revision = revision;
                }
                Err(error) => log::warn!("tag vocabulary: {error:#}"),
            }
        }
        let query_changed = search_text != self.view.visible_search;
        let scan_changed = scanning != self.last_scanning;
        self.last_scanning = scanning;
        if scan_changed && !scanning && !self.selection.is_empty() {
            let selected = self.selection.clone();
            let retained = assets
                .read()
                .unwrap()
                .iter()
                .filter(|asset| selected.contains(&asset.path))
                .map(|asset| asset.path.clone())
                .collect::<HashSet<_>>();
            if retained.len() != selected.len() {
                self.selection = Arc::new(retained);
                if self
                    .selection_anchor
                    .as_ref()
                    .is_some_and(|path| !self.selection.contains(path))
                {
                    self.selection_anchor = None;
                }
            }
        }
        self.thumb_cache
            .update(cx, |cache, cx| cache.flush_access_if_due(cx));
        if !scanning && (scan_changed || !self.cache_budget_checked) {
            self.cache_budget_checked = true;
            self.run_cache_budget_cleanup(false, cx);
        }
        if query_changed {
            self.clear_multi_selection();
            self.auto_select_pending = true;
            self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        // 资产集合或筛选变了就重跑一次查询；扫描中不重跑，避免和扫描抢数据库。
        let search_sort = SearchSort::from_key(self.sort.key()).unwrap_or(SearchSort::Name);
        let bundle_stale = self.search_bundle.as_ref().map_or(true, |bundle| {
            !bundle.matches(&search_text, revision, &filter, search_sort, self.sort_ascending)
        });
        if !scanning && bundle_stale && !self.search_refresh_queued {
            self.search_refresh_queued = true;
            self.schedule_search(cx);
        }
        let needs_visible = revision != self.view.visible_revision
            || self.view.visible_filter.as_ref() != Some(&filter)
            || self.view.visible_search != search_text
            || self.view.visible_search_revision != self.view.search_revision
            || self.view.visible_sort != self.sort
            || self.view.visible_sort_ascending != self.sort_ascending;
        if needs_visible {
            let started = Instant::now();
            // 分页结果与当前查询、筛选、排序和数据版本一致时直接用，否则回退到内存过滤。
            let current_bundle = self.search_bundle.clone();
            let bundle = current_bundle.clone().filter(|bundle| {
                bundle.matches(
                    &search_text,
                    revision,
                    &filter,
                    search_sort,
                    self.sort_ascending,
                )
            });
            let page_paths = bundle.as_ref().and_then(|bundle| bundle.page_paths.clone());
            // 快照过期时至少还能复用同一段文本的 FTS 命中集，避免退化成逐条子串匹配。
            let fts_paths = bundle
                .as_ref()
                .and_then(|bundle| bundle.fts_paths.clone())
                .or_else(|| {
                    current_bundle
                        .as_ref()
                        .filter(|bundle| bundle.text == search_text)
                        .and_then(|bundle| bundle.fts_paths.clone())
                });
            let indices = match page_paths {
                Some(paths) => {
                    // search_page 已经完成筛选与排序，按返回顺序映射到资产下标。
                    let assets = assets.read().unwrap();
                    let index_of = assets
                        .iter()
                        .enumerate()
                        .map(|(index, asset)| (asset.path.as_path(), index))
                        .collect::<HashMap<&Path, usize>>();
                    paths
                        .iter()
                        .filter_map(|path| index_of.get(path.as_path()).copied())
                        .collect::<Vec<_>>()
                }
                None => {
                    // 去重路径集合只在 dup:true 时构造：其它查询根本不读它。
                    let duplicate_paths = query_wants_duplicates(&search_text).then(|| {
                        dedup_exact_groups
                            .iter()
                            .chain(dedup_similar_groups.iter())
                            .flat_map(|group| group.assets.iter().map(|asset| asset.path.clone()))
                            .collect::<HashSet<PathBuf>>()
                    });
                    let assets = assets.read().unwrap();
                    let mut indices = assets
                        .iter()
                        .enumerate()
                        .filter(|(_, asset)| {
                            filter.matches(asset)
                                && search_query.matches(
                                    asset,
                                    fts_paths.as_deref(),
                                    duplicate_paths.as_ref(),
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
                                    SortMode::Captured => asset.taken_or_modified(),
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
                }
            };
            let elapsed_us = started.elapsed().as_secs_f64() * 1_000_000.0;
            self.view.visible_indices = Arc::new(indices);
            self.view.visible_revision = revision;
            self.view.visible_filter = Some(filter.clone());
            self.view.visible_search = search_text.clone();
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
            // F-012 端到端：输入排入防抖 → 查询完成并应用到可见索引（本帧）。
            // 最后一段“实际呈现”由下一帧回调记录；不参与渲染逻辑。
            if search_timing_enabled()
                && let Some(bundle) = bundle.as_ref()
            {
                let visible_ms = bundle.input_at.elapsed().as_secs_f64() * 1000.0;
                log::info!(
                    "search timing: ui_input_to_visible_ms={visible_ms:.1} \
                     run_search_ms={:.1} hits={} sort={:?}",
                    bundle.run_ms,
                    self.view.visible_indices.len(),
                    self.sort
                );
                self.search_frame_probe = Some(SearchFrameProbe {
                    input_at: bundle.input_at,
                    visible_ms,
                    run_ms: bundle.run_ms,
                    hits: self.view.visible_indices.len(),
                });
            }
        }
        let visible_indices = self.view.visible_indices.clone();
        // F-004：相似结果视图使用自己的有序下标；查询/筛选/数据版本变化时才重建。
        // 不能回写 `view.visible_indices`：它是图库的可见缓存，覆盖后返回图库会变成空列表。
        let visible_indices = if self.screen == Screen::Similar {
            if self.similar_visible_revision != revision
                || self.similar_visible_filter != self.similar_filter
            {
                self.rebuild_similar_visible(&assets, revision);
            }
            self.similar_visible.clone()
        } else {
            visible_indices
        };
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
        // 自动化截图用：PostMessage 合成的点击带不上 Ctrl，允许启动时预选前 N 张；
        // 正常启动不设置该环境变量，不影响手动多选。
        if let Some(count) = self.demo_select_first {
            let ready = visible_indices.len() >= count || (!scanning && !visible_indices.is_empty());
            if ready {
                let paths = {
                    let assets = assets.read().unwrap();
                    visible_indices
                        .iter()
                        .take(count)
                        .filter_map(|&index| assets.get(index).map(|asset| asset.path.clone()))
                        .collect::<Vec<_>>()
                };
                if !paths.is_empty() {
                    self.selection = Arc::new(paths.iter().cloned().collect());
                    self.selection_anchor = paths.last().cloned();
                    self.state.select(paths.last().cloned());
                    selected = paths.last().cloned();
                }
                self.demo_select_first = None;
            }
        }
        if let Some(origin_filter) = self.viewer_origin_filter.clone()
            && self.viewer_at.is_some()
            && (origin_filter != filter || self.viewer_origin_search != search_text)
        {
            let current_path = self
                .viewer_at
                .and_then(|at| self.viewer_slides.get(at))
                .map(|(path, _)| path.clone());
            let still_visible = current_path.is_some_and(|path| {
                let assets = assets.read().unwrap();
                visible_indices
                    .iter()
                    .any(|&index| assets.get(index).is_some_and(|asset| asset.path == path))
            });
            if still_visible {
                self.viewer_origin_filter = Some(filter.clone());
                self.viewer_origin_search = search_text.clone();
            } else {
                self.close_viewer(cx);
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
        let columns = if self.view_mode.is_grid() {
            grid_columns(main_width.as_f32(), self.view_mode.min_width())
        } else {
            1
        };
        let row_count = if self.view_mode.is_grid() {
            visible_indices.len().div_ceil(columns)
        } else {
            visible_indices.len()
        };
        self.grid_rows = row_count;
        self.grid_columns = columns;
        if let Some(position) = self.pending_scroll_position.take() {
            if row_count > 0 {
                let row = if self.view_mode.is_grid() {
                    (position / columns.max(1)).min(row_count - 1)
                } else {
                    position.min(row_count - 1)
                };
                self.grid_scroll
                    .scroll_to_item(row, ScrollStrategy::Nearest);
            } else {
                self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
        }
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
        let dedup_pending_total = dedup_exact_summary.pending
            + if similar_scanned {
                dedup_similar_summary.pending
            } else {
                0
            };
        let dedup_similar_count = dedup_similar_groups.len();
        let tag_search = window.use_keyed_state("kanalib-sidebar-tag-search", cx, |window, cx| {
            TextInput::new(window, cx).placeholder("搜索标签")
        });
        let tag_query = tag_search.read(cx).text().to_string();
        let sidebar: AnyElement = if self.show_sidebar {
            self.render_sidebar(
                &this,
                &roots,
                &source_aliases,
                scanning,
                &filter,
                &counts,
                self.screen,
                dedup_pending_total,
                &saved_searches,
                &blocked_tags,
                &self.tag_vocab,
                &tag_search,
                &tag_query,
                &search,
                panel_width.as_f32(),
                colors,
            )
            .into_any_element()
        } else {
            div().flex_none().into_any_element()
        };
        let (cache_hits, cache_misses) = self.thumb_cache.read(cx).access_stats();
        let viewer_content = (self.screen == Screen::Library || self.screen == Screen::Similar)
            .then(|| {
                self.viewer_at
                    .and_then(|at| self.render_viewer(&this, at, colors))
            })
            .flatten();
        // F-045：设置页/去重页进入或切换阈值时，后台计算一次预览（同步调用可能 1.8–2.6s）。
        if matches!(self.screen, Screen::Settings | Screen::Duplicates) {
            self.maybe_schedule_threshold_preview(cx);
        }
        let content: AnyElement = if let Some(viewer) = viewer_content {
            viewer
        } else if self.screen == Screen::Tags {
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
                window,
                cx,
                &roots,
                &source_aliases,
                &protected_paths,
                similar_threshold,
                queue_pending,
                queue_running,
                queue_error,
                &status,
                error.as_deref(),
                cache_hits,
                cache_misses,
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
        } else if self.screen == Screen::Similar {
            // F-004：临时结果视图，宽度与其它整页视图一致；不渲染检查器。
            let similar_width = (window_width - sidebar_width).max(px(640.));
            self.render_similar(
                &this,
                assets,
                visible_indices,
                self.selection.clone(),
                &similar_filter,
                similar_width,
                colors,
            )
            .into_any_element()
        } else if self.screen == Screen::Compare {
            // F-003：只渲染当前 2–4 张；与其它整页视图同宽，不加载全库缩略图。
            let compare_width = (window_width - sidebar_width).max(px(640.));
            self.render_compare(&this, window, cx, compare_width, colors)
                .into_any_element()
        } else {
            self.render_main(
                &this,
                assets,
                visible_indices,
                self.selection.clone(),
                roots.is_empty(),
                error.as_deref(),
                &search_warnings,
                scanning,
                main_width,
                colors,
            )
            .into_any_element()
        };
        let inspector = (self.screen == Screen::Library && self.pane_mode != PaneMode::Hidden)
            .then(|| {
                match self.pane_mode {
                    PaneMode::Preview => self
                        .render_preview_inspector(
                            &this,
                            selected_asset.as_ref(),
                            &source_aliases,
                            inspector_width,
                            colors,
                        )
                        .into_any_element(),
                    PaneMode::Details | PaneMode::Hidden => self
                        .render_inspector(
                            &this,
                            selected_asset.as_ref(),
                            &blocked_tags,
                            &source_aliases,
                            inspector_width,
                            colors,
                        )
                        .into_any_element(),
                }
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
                    Command::new("goto:dup", "重复项 · 完全重复").icon(IconName::Copy),
                    Command::new("goto:similar", "重复项 · 相似图片").icon(IconName::LayoutGrid),
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
                    Command::new("view:large", "查看 · 大图标")
                        .icon(IconName::Image)
                        .keys("ctrl-shift-1"),
                    Command::new("view:medium", "查看 · 中图标")
                        .icon(IconName::LayoutGrid)
                        .keys("ctrl-shift-3"),
                    Command::new("view:small", "查看 · 小图标")
                        .icon(IconName::Grid3x3)
                        .keys("ctrl-shift-4"),
                    Command::new("view:list", "查看 · 列表")
                        .icon(IconName::List)
                        .keys("ctrl-shift-5"),
                    Command::new("view:details", "查看 · 详细信息")
                        .icon(IconName::Table)
                        .keys("ctrl-shift-6"),
                    Command::new("view:content", "查看 · 内容")
                        .icon(IconName::AlignJustify)
                        .keys("ctrl-shift-8"),
                    Command::new("pane:details", "窗格 · 详细信息").icon(IconName::PanelRight),
                    Command::new("pane:preview", "窗格 · 预览").icon(IconName::Image),
                    Command::new("pane:hidden", "窗格 · 隐藏").icon(IconName::EyeOff),
                    Command::new("view:sidebar", "显示/隐藏侧栏").icon(IconName::PanelLeft),
                    Command::new("view:extensions", "显示/隐藏文件扩展名").icon(IconName::FileType),
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
                    Command::new("similar:find", "查找相似").icon(IconName::Search),
                    Command::new("compare:selected", "图片对比（选中 2–4 张）")
                        .icon(IconName::Columns2),
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
                    Command::new("sort:captured", "按拍摄时间排序").icon(IconName::Camera),
                    Command::new("sort:size", "按文件大小排序").icon(IconName::FileImage),
                    Command::new("sort:resolution", "按分辨率排序").icon(IconName::Maximize2),
                    Command::new("sort:star", "按星级排序").icon(IconName::Star),
                    Command::new("sort:fav", "收藏优先").icon(IconName::Heart),
                    Command::new("sort:rating", "按模型评级排序").icon(IconName::Sparkles),
                    Command::new("sort:toggle", "切换排序方向").icon(IconName::ArrowDownUp),
                ],
            )
            .group(
                "打标",
                [
                    Command::new("tag:retag-selected", "重打标选中")
                        .icon(IconName::WandSparkles),
                    Command::new("tag:prioritize", "提升打标优先级").icon(IconName::ArrowUp),
                    Command::new("tag:queue-toggle", "暂停 / 继续打标").icon(IconName::Pause),
                ],
            )
            .group(
                "图库",
                [
                    Command::new("scan:rescan", "重新扫描图库").icon(IconName::RefreshCw),
                    Command::new("scan:retry", "重试失败的打标").icon(IconName::WandSparkles),
                    Command::new("view:folder", "添加文件夹").icon(IconName::FolderOpen),
                ],
            )
            .group(
                "图库运维",
                [
                    Command::new("library:remove-selected", "从图库移除选中（不删磁盘）")
                        .icon(IconName::Trash2),
                    Command::new(
                        "library:remove-selected-hash",
                        "从图库移除选中并忽略内容哈希",
                    )
                    .icon(IconName::Ban),
                    Command::new("library:open-ignores", "查看忽略列表").icon(IconName::EyeOff),
                ],
            )
            .group(
                "备份与恢复",
                [
                    Command::new("backup:now", "立即备份数据库").icon(IconName::Database),
                    Command::new("backup:restore", "恢复数据库备份…").icon(IconName::History),
                    Command::new("backup:cancel-pending", "取消待恢复备份")
                        .icon(IconName::RotateCcw),
                    Command::new("backup:settings", "打开设置 · 数据库备份")
                        .icon(IconName::SlidersHorizontal),
                ],
            )
            .group(
                "组织",
                [
                    Command::new("org:new-album", "新建相册").icon(IconName::FolderPlus),
                    Command::new("org:save-collection", "保存当前视图为智能集合")
                        .icon(IconName::Plus),
                ],
            )
            .group(
                "分组",
                [
                    Command::new("group:none", "分组 · 关闭").icon(IconName::LayoutGrid),
                    Command::new("group:modified", "分组 · 修改时间").icon(IconName::Clock),
                    Command::new("group:captured", "分组 · 拍摄时间").icon(IconName::Camera),
                    Command::new("group:source", "分组 · 来源").icon(IconName::FolderOpen),
                    Command::new("group:rating", "分组 · 评级").icon(IconName::Star),
                ],
            )
            .on_run(move |value, _, cx| {
                let _ = run.update(cx, |app, cx| app.run_palette_command(value, cx));
            })
            .into_any_element()
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
        let tag_rename_dialog = self.tag_rename.clone().map(|name| {
            let epoch = self.tag_rename_epoch;
            let initial = name.clone();
            let field_holder = window.use_keyed_state(
                SharedString::from(format!("kanalib-tag-rename-{epoch}")),
                cx,
                move |window, cx| {
                    let input = cx.new(|cx| TextInput::new(window, cx).placeholder("新标签名"));
                    let value = initial.clone();
                    input.update(cx, |input, cx| input.set_text(value, cx));
                    input
                },
            );
            let field = field_holder.read(cx).clone();
            let close = this.clone();
            let submit = this.clone();
            let count = self
                .tag_vocab
                .iter()
                .find(|tag| tag.name == name)
                .map(|tag| tag.count)
                .unwrap_or(0);
            PromptDialog::new(
                SharedString::from(format!("kanalib-tag-rename-dialog-{epoch}")),
                "重命名标签",
                &field,
                move |_, cx| {
                    let _ = close.update(cx, |app, cx| {
                        app.tag_rename = None;
                        cx.notify();
                    });
                },
            )
            .label(format!(
                "#{name} 影响 {count} 张图片；旧名会保留为别名，仍可搜索。"
            ))
            .submit("重命名")
            .check(|text| {
                if normalize_tag_name(text).is_empty() {
                    Err("标签名不能为空".into())
                } else {
                    Ok(())
                }
            })
            .on_submit(move |text, _, cx| {
                let _ = submit.update(cx, |app, cx| app.apply_tag_rename(text, cx));
            })
            .into_any_element()
        });
        let tag_merge_dialog = self.tag_merge.clone().map(|source| {
            let epoch = self.tag_merge_epoch;
            let field = window.use_keyed_state(
                SharedString::from(format!("kanalib-tag-merge-{epoch}")),
                cx,
                |window, cx| TextInput::new(window, cx).placeholder("目标标签名（已有或新建）"),
            );
            let close = this.clone();
            let submit = this.clone();
            let count = self
                .tag_vocab
                .iter()
                .find(|tag| tag.name == source)
                .map(|tag| tag.count)
                .unwrap_or(0);
            let check_source = source.clone();
            PromptDialog::new(
                SharedString::from(format!("kanalib-tag-merge-dialog-{epoch}")),
                "合并标签",
                &field,
                move |_, cx| {
                    let _ = close.update(cx, |app, cx| {
                        app.tag_merge = None;
                        cx.notify();
                    });
                },
            )
            .label(format!(
                "将 #{source}（{count} 张）合并到目标标签；来源名会保留为别名，手工/自动标记一并合并。"
            ))
            .submit("合并")
            .check(move |text| {
                let name = normalize_tag_name(text);
                if name.is_empty() {
                    Err("目标标签名不能为空".into())
                } else if name == normalize_tag_name(&check_source) {
                    Err("目标和来源相同".into())
                } else {
                    Ok(())
                }
            })
            .on_submit(move |text, _, cx| {
                let _ = submit.update(cx, |app, cx| app.apply_tag_merge(text, cx));
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
        let rescan_dialog = self.rescan_confirm.map(|origin| {
            let close = this.clone();
            let confirm = this.clone();
            ConfirmDialog::new(
                "rescan-dialog",
                "重新扫描图库？",
                format!(
                    "重新扫描会重新索引图库，并清空当前相似扫描结果。\n当前有 {} 组相似图片结果。",
                    dedup_similar_count
                ),
                move |_, cx| {
                    let _ = close.update(cx, |app, cx| {
                        app.rescan_confirm = None;
                        cx.notify();
                    });
                },
            )
            .confirm("重新扫描图库")
            .on_confirm(move |_, cx| {
                let _ = confirm.update(cx, |app, cx| app.perform_rescan(origin, cx));
            })
            .into_any_element()
        });
        let tag_retag_dialog = self.tag_retag_confirm.clone().map(|pending| {
            let close = this.clone();
            let confirm = this.clone();
            let count = pending.paths.len();
            let scope = pending.scope;
            ConfirmDialog::new(
                "tag-retag-dialog",
                "重打标？",
                format!(
                    "将对{scope} {count} 张图片重打标：已有候选的按当前阈值重算，缺候选的入队；手工标签会保留。"
                ),
                move |_, cx| {
                    let _ = close.update(cx, |app, cx| {
                        app.tag_retag_confirm = None;
                        cx.notify();
                    });
                },
            )
            .confirm("开始重打标")
            .on_confirm(move |_, cx| {
                let _ = confirm.update(cx, |app, cx| {
                    if let Some(pending) = app.tag_retag_confirm.take() {
                        app.start_retag(pending.paths, pending.scope, cx);
                    }
                    cx.notify();
                });
            })
            .into_any_element()
        });
        let cleanup_dialog = pending_cleanup.as_ref().map(|pending| {
            let close = this.clone();
            let confirm = this.clone();
            let roots = pending
                .roots
                .iter()
                .map(|root| source_label(root, &source_aliases))
                .collect::<Vec<_>>()
                .join("、");
            let count = pending.paths.len();
            ConfirmDialog::new(
                "cleanup-missing-dialog",
                "移除缺失图片记录？",
                format!(
                    "最近一次完整扫描未在磁盘上找到 {count} 张图片。\n来源：{roots}\n\n确认后只会从图库移除这些记录，不会删除磁盘文件。\n如果来源是移动硬盘或网络盘，请先取消并恢复连接。"
                ),
                move |_, cx| {
                    let _ = close.update(cx, |app, cx| {
                        app.state.cancel_pending_cleanup();
                        cx.notify();
                    });
                },
            )
            .confirm("移除记录")
            .destructive()
            .on_confirm(move |_, cx| {
                let _ = confirm.update(cx, |app, cx| {
                    match app.state.confirm_pending_cleanup() {
                        Ok(_) => app.clear_multi_selection(),
                        Err(error) => {
                            app.state.set_error(format!("清理缺失记录失败：{error:#}"));
                        }
                    }
                    cx.notify();
                });
            })
            .into_any_element()
        });
        let remove_assets_dialog = self.remove_assets_confirm.clone().map(|request| {
            let close = this.clone();
            let confirm = this.clone();
            let count = request.paths.len();
            let hash_note = if request.remember_hash {
                "\n\n同时忽略这些图片的内容哈希：同内容副本重扫时也不会重新导入。"
            } else {
                "\n\n同时忽略这些路径：重扫不会重新导入；可在设置页的忽略列表恢复。"
            };
            ConfirmDialog::new(
                "remove-assets-dialog",
                "从图库移除？",
                format!(
                    "将从图库移除 {count} 张图片。{hash_note}\n只删除图库记录，磁盘文件原样保留。"
                ),
                move |_, cx| {
                    let _ = close.update(cx, |app, cx| {
                        app.remove_assets_confirm = None;
                        cx.notify();
                    });
                },
            )
            .confirm("从图库移除")
            .destructive()
            .on_confirm(move |_, cx| {
                let _ = confirm.update(cx, |app, cx| app.confirm_remove_from_library(cx));
            })
            .into_any_element()
        });
        let drop_dialog = self.drop_prompt.clone().map(|prompt| {
            let DropPrompt::AddSources { dirs, parents } = prompt;
            let mut lines = Vec::new();
            if !dirs.is_empty() {
                lines.push(format!("添加 {} 个文件夹为来源：", dirs.len()));
                for dir in dirs.iter().take(4) {
                    lines.push(format!("· {}", dir.display()));
                }
                if dirs.len() > 4 {
                    lines.push(format!("· …另有 {} 个", dirs.len() - 4));
                }
            }
            if !parents.is_empty() {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.push(format!(
                    "为来源外的 {} 张图片添加所在文件夹：",
                    parents.len()
                ));
                for path in parents.iter().take(4) {
                    lines.push(format!("· {}", path.display()));
                }
                if parents.len() > 4 {
                    lines.push(format!("· …另有 {} 个", parents.len() - 4));
                }
            }
            lines.push(String::new());
            lines.push("只建立索引，不复制、不移动磁盘文件。".to_string());
            let close = this.clone();
            let confirm = this.clone();
            ConfirmDialog::new(
                "drop-import-dialog",
                "拖拽导入？",
                lines.join("\n"),
                move |_, cx| {
                    let _ = close.update(cx, |app, cx| {
                        app.drop_prompt = None;
                        cx.notify();
                    });
                },
            )
            .confirm("添加为来源")
            .on_confirm(move |_, cx| {
                let _ = confirm.update(cx, |app, cx| app.confirm_drop_prompt(cx));
            })
            .into_any_element()
        });
        let drop_overlay = self.drop_highlight.then(|| {
            div()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size_full()
                .bg(colors.accent.opacity(0.10))
                .border_2()
                .border_color(colors.accent)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .child(
                    Icon::new(IconName::FolderPlus)
                        .size(IconSize::Lg)
                        .color(colors.accent),
                )
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.fg)
                        .child("松开以导入到图库"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.muted)
                        .child("文件夹 → 添加为来源 · 来源内的图片 → 定位选中 · 其他图片 → 询问父目录"),
                )
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
        // 组织弹窗（智能集合重命名 / 相册增删改）：与其它弹窗一样挂在根节点。
        let org_dialog = self.render_org_dialog(window, cx, &this);
        let titlebar = self.render_titlebar(
            &this,
            window,
            cx,
            &filter_title,
            visible_count,
            &search,
            colors,
        );
        let status_bar = self.render_status_bar(
            &this,
            &status,
            error.as_deref(),
            scanning,
            device.as_deref(),
            queue_pending,
            queue_running,
            queue_error,
            &counts,
            &filter,
            window_width,
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
            .child(status_bar)
            .children(palette)
            .children(save_dialog)
            .children(tag_dialog)
            .children(tag_rename_dialog)
            .children(tag_merge_dialog)
            .children(remove_source_dialog)
            .children(rescan_dialog)
            .children(tag_retag_dialog)
            .children(source_rename_dialog)
            .children(source_context_menu)
            .children(cleanup_dialog)
            .children(remove_assets_dialog)
            .children(drop_dialog)
            .children(drop_overlay)
            .children(org_dialog)
            .child(ToastViewport::new("kanalib-toasts", &self.toaster))
            .on_action(cx.listener(|this, _: &OpenPalette, _, cx| {
                if !this.palette_open && this.viewer_at.is_some() {
                    this.close_viewer(cx);
                }
                this.palette_open = !this.palette_open;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.set_show_sidebar(!this.show_sidebar, cx);
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
            // F-004：结果视图里 Esc 先关快速查看，再回到图库；图库里仍是清除选择。
            .on_action(cx.listener(|this, _: &CloseSimilar, _, cx| {
                if this.screen == Screen::Similar && this.viewer_at.is_none() {
                    this.close_similar(cx);
                } else {
                    this.clear_selection(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenQuickView, _, cx| this.open_quick_view(cx)))
            .on_action(cx.listener(|this, _: &ToggleFavorite, _, cx| this.toggle_favorite(cx)))
            .on_action(cx.listener(|this, _: &Rate0, _, cx| this.apply_star(0, cx)))
            .on_action(cx.listener(|this, _: &Rate1, _, cx| this.apply_star(1, cx)))
            .on_action(cx.listener(|this, _: &Rate2, _, cx| this.apply_star(2, cx)))
            .on_action(cx.listener(|this, _: &Rate3, _, cx| this.apply_star(3, cx)))
            .on_action(cx.listener(|this, _: &Rate4, _, cx| this.apply_star(4, cx)))
            .on_action(cx.listener(|this, _: &Rate5, _, cx| this.apply_star(5, cx)))
            .on_action(cx.listener(|this, _: &ViewIconsLarge, _, cx| {
                this.set_view_mode(ViewMode::IconsLarge, cx)
            }))
            .on_action(cx.listener(|this, _: &ViewIconsMedium, _, cx| {
                this.set_view_mode(ViewMode::IconsMedium, cx)
            }))
            .on_action(cx.listener(|this, _: &ViewIconsSmall, _, cx| {
                this.set_view_mode(ViewMode::IconsSmall, cx)
            }))
            .on_action(cx.listener(|this, _: &ViewList, _, cx| {
                this.set_view_mode(ViewMode::List, cx)
            }))
            .on_action(cx.listener(|this, _: &ViewDetails, _, cx| {
                this.set_view_mode(ViewMode::Details, cx)
            }))
            .on_action(cx.listener(|this, _: &ViewContent, _, cx| {
                this.set_view_mode(ViewMode::Content, cx)
            }))
            // F-036：外部文件拖拽。进入时显示遮罩，释放时按路径类型分流；
            // 取消（Esc/离开窗口）只清除遮罩，不产生任何副作用。
            .on_drop::<ExternalPaths>({
                let this_drop = this.clone();
                move |paths: &ExternalPaths, _, cx| {
                    let paths = paths.paths().to_vec();
                    let _ = this_drop.update(cx, |app, cx| app.handle_external_drop(paths, cx));
                }
            })
            .on_drag_move::<ExternalPaths>({
                let this_enter = this.clone();
                move |_event, _, cx| {
                    let _ = this_enter.update(cx, |app, cx| {
                        if !app.drop_highlight {
                            app.drop_highlight = true;
                            cx.notify();
                        }
                    });
                }
            })
            .on_file_drop_exit({
                let this_exit = this.clone();
                move |_event, _, cx| {
                    let _ = this_exit.update(cx, |app, cx| {
                        if app.drop_highlight {
                            app.drop_highlight = false;
                            cx.notify();
                        }
                    });
                }
            });
        if self.focus_viewer_pending && self.viewer_at.is_some() {
            window.focus(&self.viewer_focus, cx);
            self.focus_viewer_pending = false;
        }
        if self.focus_grid_pending
            && (self.screen == Screen::Library || self.screen == Screen::Similar)
            && !self.palette_open
            && self.viewer_at.is_none()
            && !self.save_search_open
            && self.tag_dialog.is_none()
            && self.tag_rename.is_none()
            && self.tag_merge.is_none()
            && self.source_remove_confirm.is_none()
            && self.source_rename.is_none()
            && self.rescan_confirm.is_none()
            && self.tag_retag_confirm.is_none()
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
        if let Some(probe) = self.search_frame_probe.take() {
            window.on_next_frame(move |_, _| {
                log::info!(
                    "search timing: ui_input_to_frame_ms={:.1} visible_ms={:.1} \
                     run_search_ms={:.1} hits={}",
                    probe.input_at.elapsed().as_secs_f64() * 1000.0,
                    probe.visible_ms,
                    probe.run_ms,
                    probe.hits
                );
            });
        }
        // 整棵树共用一个有界图片缓存：子树里所有 img 都走它，不再进无上限的全局 asset cache。
        image_cache(self.thumb_cache.clone())
            .size_full()
            .child(shell)
    }
}

impl KanalibApp {
    /// 当前视图状态：保存/更新智能集合时一起落库。
    pub(crate) fn current_saved_search_view(&self) -> SavedSearchView {
        SavedSearchView {
            sort_key: self.sort.key().to_string(),
            sort_ascending: self.sort_ascending,
            view_mode: self.view_mode.key().to_string(),
            pane_mode: self.pane_mode.key().to_string(),
            show_sidebar: self.show_sidebar,
            show_extensions: self.show_extensions,
        }
    }

    /// 应用智能集合：恢复筛选、搜索、排序、视图模式与窗格/侧栏开关。
    ///
    /// 旧集合（迁移前的行）`sort_key`/`view_mode`/`pane_mode` 为空字符串，语义是
    /// 「沿用当前」，这里逐项判断，不做静默覆盖。
    pub(crate) fn apply_saved_search_with_view(
        &mut self,
        saved: &SavedSearch,
        search: &Entity<TextInput>,
        cx: &mut Context<Self>,
    ) {
        search.update(cx, |input, cx| input.set_text(saved.query.clone(), cx));
        self.state.set_filter(saved.filter.clone());
        if let Some(mode) = SortMode::from_key(&saved.view.sort_key) {
            self.sort = mode;
            self.persist_setting("ui.sort", mode.key());
        }
        self.sort_ascending = saved.view.sort_ascending;
        self.persist_setting("ui.sort_ascending", self.sort_ascending.to_string());
        if let Some(mode) = ViewMode::from_key(&saved.view.view_mode) {
            self.view_mode = mode;
            self.persist_setting("ui.view_mode", mode.key());
        }
        if !saved.view.pane_mode.is_empty() {
            let mode = PaneMode::from_key(&saved.view.pane_mode);
            self.pane_mode = mode;
            self.persist_setting("ui.pane_mode", mode.key());
        }
        self.show_sidebar = saved.view.show_sidebar;
        self.persist_setting("ui.show_sidebar", self.show_sidebar.to_string());
        self.show_extensions = saved.view.show_extensions;
        self.persist_setting("ui.show_extensions", self.show_extensions.to_string());
        self.screen = Screen::Library;
        self.dedup_confirm = None;
        self.clear_multi_selection();
        self.focus_grid_pending = true;
        self.auto_select_pending = true;
        self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.schedule_search(cx);
        cx.notify();
    }

    /// 用当前筛选/搜索/排序/视图覆盖集合内容；集合 id 与名称不变。
    pub(crate) fn update_saved_search_from_current(&mut self, id: i64, cx: &mut Context<Self>) {
        let filter = self.state.shared.lock().unwrap().filter.clone();
        let query = self.last_search.clone();
        let view = self.current_saved_search_view();
        match self.state.update_saved_search(id, &filter, &query, &view) {
            Ok(saved) => push_toast(
                &self.toaster,
                cx,
                format!(
                    "已用当前视图更新「{}」（筛选/搜索/排序/视图/窗格）",
                    saved.name
                ),
                Severity::Success,
            ),
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("更新智能集合失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    pub(crate) fn toggle_saved_search_pin(&mut self, id: i64, cx: &mut Context<Self>) {
        let pinned = self
            .state
            .shared
            .lock()
            .unwrap()
            .saved_searches
            .iter()
            .find(|saved| saved.id == id)
            .map(|saved| saved.pinned)
            .unwrap_or(false);
        match self.state.set_saved_search_pinned(id, !pinned) {
            Ok(saved) => push_toast(
                &self.toaster,
                cx,
                if saved.pinned {
                    format!("已置顶「{}」，重启后保持", saved.name)
                } else {
                    format!("已取消置顶「{}」", saved.name)
                },
                Severity::Success,
            ),
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("置顶失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    /// 上下移动智能集合；置顶组与普通组不互相跨越（先取消置顶再移动）。
    pub(crate) fn move_saved_search(&mut self, id: i64, delta: isize, cx: &mut Context<Self>) {
        let searches = self.state.shared.lock().unwrap().saved_searches.clone();
        let Some(index) = searches.iter().position(|saved| saved.id == id) else {
            return;
        };
        let target = index as isize + delta;
        if target < 0 || target as usize >= searches.len() {
            return;
        }
        let target = target as usize;
        if searches[index].pinned != searches[target].pinned {
            push_toast(
                &self.toaster,
                cx,
                "置顶集合与普通集合不能互相跨越；先取消置顶再移动",
                Severity::Warning,
            );
            return;
        }
        let mut ordered = searches.iter().map(|saved| saved.id).collect::<Vec<_>>();
        ordered.swap(index, target);
        match self.state.reorder_saved_searches(&ordered) {
            Ok(()) => push_toast(&self.toaster, cx, "已调整智能集合顺序", Severity::Success),
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("调整顺序失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    /// 保存智能集合时同时记录排序/视图/窗格；同名冲突用核心的 `DuplicateNameError` 判定，
    /// 弹窗保持打开，绝不静默覆盖。
    pub(crate) fn confirm_save_search_with_view(&mut self, name: &str, cx: &mut Context<Self>) {
        let filter = self.state.shared.lock().unwrap().filter.clone();
        let query = self.last_search.clone();
        let view = self.current_saved_search_view();
        match self
            .state
            .save_saved_search_with_view(name, &filter, &query, &view)
        {
            Ok(saved) => {
                self.save_search_open = false;
                push_toast(
                    &self.toaster,
                    cx,
                    format!("已保存智能集合「{}」", saved.name),
                    Severity::Success,
                );
            }
            Err(error) => {
                if error
                    .downcast_ref::<kanalib_core::model::DuplicateNameError>()
                    .is_some()
                {
                    push_toast(
                        &self.toaster,
                        cx,
                        crate::sidebar::duplicate_name_message(name),
                        Severity::Warning,
                    );
                } else {
                    push_toast(
                        &self.toaster,
                        cx,
                        format!("保存失败：{error:#}"),
                        Severity::Danger,
                    );
                }
            }
        }
        cx.notify();
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
            KeyBinding::new("ctrl-b", ToggleSidebar, None),
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
            KeyBinding::new("escape", CloseSimilar, Some(GRID_CONTEXT)),
            KeyBinding::new("ctrl-shift-1", ViewIconsLarge, None),
            KeyBinding::new("ctrl-shift-3", ViewIconsMedium, None),
            KeyBinding::new("ctrl-shift-4", ViewIconsSmall, None),
            KeyBinding::new("ctrl-shift-5", ViewList, None),
            KeyBinding::new("ctrl-shift-6", ViewDetails, None),
            KeyBinding::new("ctrl-shift-8", ViewContent, None),
            KeyBinding::new("escape", ViewerClose, Some(VIEWER_CONTEXT)),
            KeyBinding::new("left", ViewerPrev, Some(VIEWER_CONTEXT)),
            KeyBinding::new("right", ViewerNext, Some(VIEWER_CONTEXT)),
            KeyBinding::new("f", ViewerFullscreen, Some(VIEWER_CONTEXT)),
            KeyBinding::new(
                "escape",
                CompareClose,
                Some(crate::compare::COMPARE_CONTEXT),
            ),
            KeyBinding::new(
                "+",
                CompareZoomIn,
                Some(crate::compare::COMPARE_CONTEXT),
            ),
            KeyBinding::new(
                "=",
                CompareZoomIn,
                Some(crate::compare::COMPARE_CONTEXT),
            ),
            KeyBinding::new(
                "-",
                CompareZoomOut,
                Some(crate::compare::COMPARE_CONTEXT),
            ),
            KeyBinding::new("0", CompareFit, Some(crate::compare::COMPARE_CONTEXT)),
            KeyBinding::new(
                "left",
                ComparePanLeft,
                Some(crate::compare::COMPARE_CONTEXT),
            ),
            KeyBinding::new(
                "right",
                ComparePanRight,
                Some(crate::compare::COMPARE_CONTEXT),
            ),
            KeyBinding::new("up", ComparePanUp, Some(crate::compare::COMPARE_CONTEXT)),
            KeyBinding::new(
                "down",
                ComparePanDown,
                Some(crate::compare::COMPARE_CONTEXT),
            ),
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
        let view_mode = resolve_view_mode(
            state.setting("ui.view_mode").as_deref(),
            state.setting("ui.density").as_deref(),
        );
        let pane_mode = state
            .setting("ui.pane_mode")
            .map(|value| PaneMode::from_key(&value))
            .unwrap_or_default();
        let show_sidebar = state
            .setting("ui.show_sidebar")
            .map(|value| value != "false")
            .unwrap_or(true);
        let show_extensions = state
            .setting("ui.show_extensions")
            .map(|value| value != "false")
            .unwrap_or(true);
        let sidebar_width = state
            .setting("ui.sidebar_width")
            .and_then(|value| value.parse::<f32>().ok())
            .map(|value| value.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH))
            .unwrap_or(SIDEBAR_DEFAULT_WIDTH);
        let sidebar_collapsed = state
            .setting("ui.sidebar_collapsed")
            .map(|value| {
                value
                    .split(',')
                    .filter(|key| SIDEBAR_SECTIONS.contains(key))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let thumb_budget_gb = state
            .setting("cache.thumb_budget_gb")
            .and_then(|value| value.parse::<u64>().ok())
            .map(|value| value.clamp(1, 1000))
            .unwrap_or(10);
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
        let cache_dir = state.cache_dir();
        let db_path = state.db_path();
        let source_statuses = state.source_statuses();
        let group_by = crate::views::initial_group_mode(&state);
        let access_path = db_path
            .parent()
            .map(|dir| dir.join("thumb_access.json"))
            .unwrap_or_else(|| cache_dir.join("thumb_access.json"));
        Self {
            state,
            grid_scroll: UniformListScrollHandle::new(),
            dedup_scroll: UniformListScrollHandle::new(),
            tag_scroll: UniformListScrollHandle::new(),
            tag_list: Vec::new(),
            tag_vocab: Vec::new(),
            tag_vocab_revision: 0,
            tag_pane_category: "general".into(),
            tag_pane_expanded: false,
            tag_pane_sort_name: false,
            tag_suggestions: Vec::new(),
            cache_stats: None,
            thumb_budget_gb,
            cache_last_cleanup: None,
            cache_budget_checked: false,
            last_search: String::new(),
            search_pending: String::new(),
            search_bundle: None,
            search_debounce_task: None,
            search_queued_at: None,
            search_frame_probe: None,
            search_refresh_queued: false,
            search_running: false,
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
            rescan_confirm: None,
            settings_notice: None,
            tag_thresholds: None,
            tag_queue_status: None,
            tag_queue_signature: None,
            tag_job_errors: Vec::new(),
            tag_cpu_allowed: false,
            tag_settings_notice: None,
            tag_recompute_running: false,
            tag_retag_running: false,
            tag_retag_confirm: None,
            tag_reviews: Vec::new(),
            tag_review_notice: None,
            tag_review_running: false,
            source_statuses,
            last_backup: None,
            backup_running: false,
            pending_restore_hint: None,
            diagnostic_snapshot: None,
            refresh_task: None,
            cache_task: None,
            thumb_cache: cx.new(|cx| {
                ThumbCache::with_limit(
                    std::env::var("KANALIB_IMAGE_CACHE")
                        .ok()
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(IMAGE_CACHE_LIMIT),
                    cache_dir,
                    db_path,
                    access_path,
                    cx,
                )
            }),
            scroll_task: None,
            similar_task: None,
            grid_focus: cx.focus_handle(),
            viewer_focus: cx.focus_handle(),
            focus_grid_pending: true,
            focus_viewer_pending: false,
            viewer_original: false,
            auto_select_pending: false,
            selection: Arc::new(initial_selected.iter().cloned().collect()),
            selection_anchor: initial_selected,
            sort,
            sort_ascending,
            view_mode,
            pane_mode,
            show_sidebar,
            sidebar_width,
            sidebar_collapsed,
            sidebar_resizing: false,
            sidebar_persist_task: None,
            show_extensions,
            pending_scroll_position: None,
            search_input: None,
            palette_open: false,
            viewer_slides: Vec::new(),
            viewer_at: None,
            viewer_viewed: HashSet::new(),
            viewer_origin_filter: None,
            viewer_origin_search: String::new(),
            save_search_open: false,
            tag_dialog: None,
            tag_dialog_epoch: 0,
            tag_dialog_paths: Vec::new(),
            tag_rename: None,
            tag_rename_epoch: 0,
            tag_merge: None,
            tag_merge_epoch: 0,
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
            scan_filter_draft: None,
            remove_assets_confirm: None,
            drop_prompt: None,
            // 仅供自动化截取拖拽高亮遮罩；正常启动保持关闭，真实拖拽仍走 on_drop。
            drop_highlight: std::env::var_os("KANALIB_DROP_PREVIEW").is_some(),
            org_dialog: None,
            org_dialog_epoch: 0,
            album_cache: std::cell::RefCell::new(crate::sidebar::AlbumCache::default()),
            group_by,
            collapsed_groups: HashSet::new(),
            group_cache: crate::views::GroupCache::default(),
            pending_reveal_group: None,
            similar_view: None,
            similar_filter_input: None,
            similar_filter: String::new(),
            similar_visible: Arc::new(Vec::new()),
            similar_visible_revision: u64::MAX,
            similar_visible_filter: String::new(),
            similar_refresh_task: None,
            settings_threshold_draft: None,
            dedup_threshold_draft: None,
            similar_preview: None,
            similar_preview_threshold: None,
            similar_preview_running: false,
            similar_preview_epoch: 0,
            similar_preview_task: None,
            compare_items: Vec::new(),
            compare_origin: Screen::Library,
            compare_origin_scroll: None,
            compare_zoom: 1.0,
            compare_pan: (0., 0.),
            compare_keeper: None,
            compare_original: false,
            compare_focus: cx.focus_handle(),
            compare_focus_pending: false,
            demo_select_first: std::env::var("KANALIB_SELECT_FIRST")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|count| *count > 0),
            window_state_root: kanalib_core::sidecar::project_root(),
            window_state_pending: None,
            window_state_task: None,
            window_state_subscription: None,
            window_state_quit: None,
        }
    }

    /// F-038：监听窗口移动/缩放/最大化并节流保存；关闭与退出时各补写一次。
    fn init_window_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.window_state_root.clone();
        window.on_window_should_close(cx, move |window, cx| {
            let state = crate::platform::WindowState::capture(window, cx);
            if let Err(error) = state.save(&root) {
                log::warn!("save window state on close: {error}");
            }
            true
        });
        self.window_state_subscription = Some(cx.observe_window_bounds(window, |app, window, cx| {
            app.queue_window_state_save(window, cx);
        }));
        self.window_state_quit = Some(cx.on_app_quit(|app, _cx| {
            if let Some(state) = app.window_state_pending.take() {
                if let Err(error) = state.save(&app.window_state_root) {
                    log::warn!("save window state on quit: {error}");
                }
            }
            async {}
        }));
        self.queue_window_state_save(window, cx);
    }

    /// 500ms 静默窗口后写盘；拖动过程中只更新 pending，不逐帧碰磁盘。
    fn queue_window_state_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = crate::platform::WindowState::capture(window, cx);
        if self.window_state_pending == Some(state) {
            return;
        }
        self.window_state_pending = Some(state);
        let running = self
            .window_state_task
            .as_ref()
            .is_some_and(|task| !task.is_ready());
        if running {
            return;
        }
        let root = self.window_state_root.clone();
        self.window_state_task = Some(cx.spawn(async move |app, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                match app.update(cx, |app, _| app.window_state_pending.take()) {
                    Ok(Some(state)) => {
                        if let Err(error) = state.save(&root) {
                            log::warn!("save window state: {error}");
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        }));
    }

    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.init_window_state(window, cx);
        self.sync_source_statuses();
        // 启动时若有上次暂存但尚未应用的恢复请求（通常会在打开数据库前应用），
        // 给出持久提示与设置页取消入口；正常启动这里为 None。
        self.refresh_pending_restore_hint(cx);
        if self.screen == Screen::Settings {
            self.refresh_diagnostics();
            self.refresh_tag_settings(cx);
        }
        // 先跑一次空查询，让首屏用上分页下推的结果。
        self.schedule_search(cx);
        let weak = cx.entity().downgrade();
        self.refresh_task = Some(cx.spawn(async move |_, cx| {
            let mut last_status = String::new();
            let mut last_scanning = false;
            let mut last_revision = 0u64;
            let mut ticks = 0u64;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                // 队列状态只在设置页轮询：约 1.2s 一次队列快照，每 ~6s 连阈值与错误一起刷新。
                ticks = ticks.wrapping_add(1);
                if ticks % 10 == 0
                    && weak
                        .update(cx, |app, cx| {
                            if app.screen == Screen::Settings {
                                if ticks % 50 == 0 {
                                    app.refresh_tag_settings(cx);
                                } else {
                                    app.refresh_tag_queue_status(cx);
                                }
                            }
                        })
                        .is_err()
                {
                    break;
                }
                let snapshot = weak.update(cx, |app, _| {
                    let shared = app.state.shared.lock().unwrap();
                    (shared.status.clone(), shared.scanning, shared.revision)
                });
                let Ok((status, scanning, revision)) = snapshot else {
                    break;
                };
                if status != last_status || scanning != last_scanning || revision != last_revision {
                    let scan_finished = last_scanning && !scanning;
                    last_status = status;
                    last_scanning = scanning;
                    last_revision = revision;
                    if weak
                        .update(cx, |app, cx| {
                            app.sync_source_statuses();
                            if scan_finished && app.screen == Screen::Settings {
                                app.refresh_diagnostics();
                                app.refresh_tag_settings(cx);
                            }
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }));
        let weak = cx.entity().downgrade();
        self.cache_task = Some(cx.spawn(async move |_, cx| loop {
            cx.background_executor()
                .timer(Duration::from_secs(600))
                .await;
            if weak
                .update(cx, |app, cx| {
                    app.run_cache_budget_cleanup(false, cx);
                })
                .is_err()
            {
                break;
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

    pub(crate) fn choose_folder(&mut self, cx: &mut Context<Self>) {
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
                app.reset_search();
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

    pub(crate) fn rescan(&mut self, cx: &mut Context<Self>) {
        self.request_rescan(RescanOrigin::Library, cx);
    }

    /// 图库重扫会清空相似扫描结果。已有相似组时先确认，避免看起来像“重置”。
    pub(crate) fn request_rescan(&mut self, origin: RescanOrigin, cx: &mut Context<Self>) {
        let similar_groups = {
            let shared = self.state.shared.lock().unwrap();
            if shared.dedup.similar_scanned {
                shared.dedup.similar_groups.len()
            } else {
                0
            }
        };
        if similar_groups > 0 {
            self.rescan_confirm = Some(origin);
        } else {
            self.perform_rescan(origin, cx);
        }
        cx.notify();
    }

    pub(crate) fn perform_rescan(&mut self, origin: RescanOrigin, cx: &mut Context<Self>) {
        self.rescan_confirm = None;
        self.state.start_scan();
        match origin {
            RescanOrigin::Library => {
                // 保留当前查询：资产集合变了会自动重跑一次分页查询。
                self.screen = Screen::Library;
                self.dedup_selected = None;
                self.dedup_confirm = None;
                self.clear_multi_selection();
                self.focus_grid_pending = true;
                self.auto_select_pending = true;
                self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
            RescanOrigin::Dedup => {
                self.screen = Screen::Duplicates;
                self.dedup_selected = None;
                self.dedup_confirm = None;
                self.dedup_notice = None;
            }
        }
        cx.notify();
    }

    pub(crate) fn retry_failed(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::Settings;
        self.dedup_confirm = None;
        self.tag_retag_confirm = None;
        self.tag_settings_notice = None;
        self.settings_notice = None;
        self.refresh_cache_stats(cx);
        self.sync_source_statuses();
        self.refresh_diagnostics();
        self.refresh_tag_settings(cx);
        self.refresh_tag_reviews(cx);
    }

    /// 同步“已暂存、等待重启”的恢复请求到内存提示；只在启动/恢复/取消时调用，
    /// 不放进每帧渲染（`backup_status` 会打开一次数据库）。
    pub(crate) fn refresh_pending_restore_hint(&mut self, cx: &mut Context<Self>) {
        match self.state.backup_status().pending_restore {
            Some(pending) => {
                let message = format!(
                    "待恢复备份：{} · 重启 Kanalib 后生效；可在设置页「数据库备份」取消",
                    pending.candidate.display()
                );
                let changed = self.pending_restore_hint.as_deref() != Some(message.as_str());
                self.pending_restore_hint = Some(message.clone());
                if changed {
                    self.settings_notice = Some(message.clone());
                    push_toast(
                        &self.toaster,
                        cx,
                        "检测到待恢复备份：重启后生效，可在设置页取消",
                        Severity::Warning,
                    );
                }
            }
            None => self.pending_restore_hint = None,
        }
    }

    /// 数据目录（数据库所在目录）。
    pub(crate) fn data_dir(&self) -> PathBuf {
        let db_path = self.state.db_path();
        db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or(db_path)
    }

    /// 从核心状态同步来源扫描快照；不额外遍历资产，供每帧布局读取。
    pub(crate) fn sync_source_statuses(&mut self) {
        let mut statuses = {
            let shared = self.state.shared.lock().unwrap();
            shared
                .source_statuses
                .values()
                .cloned()
                .collect::<Vec<_>>()
        };
        statuses.sort_by(|left, right| left.root.cmp(&right.root));
        self.source_statuses = statuses;
    }

    pub(crate) fn visible_paths(&self) -> Vec<PathBuf> {
        let shared = self.state.shared.lock().unwrap();
        let assets = shared.assets.read().unwrap();
        self.view
            .visible_indices
            .iter()
            .filter_map(|&index| assets.get(index).map(|asset| asset.path.clone()))
            .collect()
    }

    pub(crate) fn selected_paths(&self) -> Vec<PathBuf> {
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

    pub(crate) fn range_between(&self, anchor: Option<&Path>, path: &Path) -> Option<Vec<PathBuf>> {
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

    pub(crate) fn handle_tile_click(
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
            self.open_viewer_at(&path);
        }
        cx.notify();
    }

    pub(crate) fn select_position(&mut self, position: usize, cx: &mut Context<Self>) {
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

    pub(crate) fn navigate(&mut self, delta: isize, cx: &mut Context<Self>) {
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

    pub(crate) fn select_edge(&mut self, last: bool, cx: &mut Context<Self>) {
        let len = self.view.visible_indices.len();
        if len > 0 {
            self.select_position(if last { len - 1 } else { 0 }, cx);
        }
    }

    pub(crate) fn select_all_visible(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn select_single_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.selection = Arc::new(HashSet::from([path.clone()]));
        self.selection_anchor = Some(path.clone());
        self.state.select(Some(path));
        cx.notify();
    }

    pub(crate) fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selection = Arc::new(HashSet::new());
        self.selection_anchor = None;
        self.auto_select_pending = false;
        self.state.select(None);
        cx.notify();
    }

    pub(crate) fn clear_multi_selection(&mut self) {
        self.selection = Arc::new(HashSet::new());
        self.selection_anchor = None;
    }

    pub(crate) fn paths_all_favorite(&self, paths: &[PathBuf]) -> bool {
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

    pub(crate) fn apply_favorite(&mut self, favorite: bool, cx: &mut Context<Self>) {
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

    pub(crate) fn toggle_favorite(&mut self, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        let favorite = !self.paths_all_favorite(&paths);
        self.apply_favorite(favorite, cx);
    }

    pub(crate) fn apply_star(&mut self, star: u8, cx: &mut Context<Self>) {
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

    pub(crate) fn export_assets(
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

    pub(crate) fn copy_selected_prompt(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn copy_selected_paths(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = &self.search_input {
            let handle = search.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    pub(crate) fn persist_setting(&self, key: &str, value: impl AsRef<str>) {
        if let Err(error) = self.state.set_setting(key, value.as_ref()) {
            log::warn!("save setting {key}: {error:#}");
        }
    }

    pub(crate) fn choose_sort_mode(&mut self, mode: SortMode, cx: &mut Context<Self>) {
        self.sort = mode;
        self.sort_ascending = mode.default_ascending();
        self.persist_setting("ui.sort", mode.key());
        self.persist_setting("ui.sort_ascending", self.sort_ascending.to_string());
        self.focus_grid_pending = true;
        // 排序是查询的一部分，分页结果要按新排序重跑。
        self.schedule_search(cx);
        cx.notify();
    }

    pub(crate) fn set_sort_ascending(&mut self, ascending: bool, cx: &mut Context<Self>) {
        self.sort_ascending = ascending;
        self.persist_setting("ui.sort_ascending", ascending.to_string());
        self.focus_grid_pending = true;
        self.schedule_search(cx);
        cx.notify();
    }

    pub(crate) fn selected_visible_position(&self) -> usize {
        let shared = self.state.shared.lock().unwrap();
        let selected = shared.selected.as_ref();
        let assets = shared.assets.read().unwrap();
        selected
            .and_then(|path| {
                self.view
                    .visible_indices
                    .iter()
                    .position(|&index| assets.get(index).is_some_and(|asset| &asset.path == path))
            })
            .unwrap_or(0)
    }

    /// 切视图/窗格/侧栏后行索引会变，记下选中项在可见列表中的位置，等新布局算出行号再滚动。
    pub(crate) fn anchor_scroll(&mut self) {
        self.pending_scroll_position = Some(self.selected_visible_position());
    }

    pub(crate) fn set_view_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        if self.screen != Screen::Library || self.view_mode == mode {
            return;
        }
        self.anchor_scroll();
        self.view_mode = mode;
        self.persist_setting("ui.view_mode", mode.key());
        self.focus_grid_pending = true;
        cx.notify();
    }

    pub(crate) fn set_pane_mode(&mut self, mode: PaneMode, cx: &mut Context<Self>) {
        if self.pane_mode == mode {
            return;
        }
        self.anchor_scroll();
        self.pane_mode = mode;
        self.persist_setting("ui.pane_mode", mode.key());
        self.focus_grid_pending = true;
        cx.notify();
    }

    pub(crate) fn toggle_pane_mode(&mut self, mode: PaneMode, cx: &mut Context<Self>) {
        let next = if self.pane_mode == mode && mode != PaneMode::Hidden {
            PaneMode::Hidden
        } else {
            mode
        };
        self.set_pane_mode(next, cx);
    }

    pub(crate) fn set_show_extensions(&mut self, show: bool, cx: &mut Context<Self>) {
        self.show_extensions = show;
        self.persist_setting("ui.show_extensions", show.to_string());
        cx.notify();
    }

    pub(crate) fn toggle_theme(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn run_palette_command(&mut self, command: &str, cx: &mut Context<Self>) {
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
            "view:large" => self.set_view_mode(ViewMode::IconsLarge, cx),
            "view:medium" => self.set_view_mode(ViewMode::IconsMedium, cx),
            "view:small" => self.set_view_mode(ViewMode::IconsSmall, cx),
            "view:list" => self.set_view_mode(ViewMode::List, cx),
            "view:details" => self.set_view_mode(ViewMode::Details, cx),
            "view:content" => self.set_view_mode(ViewMode::Content, cx),
            "pane:details" => self.toggle_pane_mode(PaneMode::Details, cx),
            "pane:preview" => self.toggle_pane_mode(PaneMode::Preview, cx),
            "pane:hidden" => self.set_pane_mode(PaneMode::Hidden, cx),
            "view:sidebar" => self.set_show_sidebar(!self.show_sidebar, cx),
            "view:extensions" => self.set_show_extensions(!self.show_extensions, cx),
            "view:folder" => self.choose_folder(cx),
            "scan:rescan" => self.rescan(cx),
            "scan:retry" => self.retry_failed(cx),
            "tag:retag-selected" => self.request_retag(true, cx),
            "tag:prioritize" => self.prioritize_selected(cx),
            "tag:queue-toggle" => self.toggle_tag_queue(cx),
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
            "sort:captured" => self.choose_sort_mode(SortMode::Captured, cx),
            "sort:size" => self.choose_sort_mode(SortMode::Size, cx),
            "sort:resolution" => self.choose_sort_mode(SortMode::Resolution, cx),
            "sort:star" => self.choose_sort_mode(SortMode::Star, cx),
            "sort:fav" => self.choose_sort_mode(SortMode::Favorite, cx),
            "sort:rating" => self.choose_sort_mode(SortMode::Rating, cx),
            "sort:toggle" => self.set_sort_ascending(!self.sort_ascending, cx),
            // F-037：从图库移除（只删 DB）与忽略列表入口。
            "library:remove-selected" => self.request_remove_from_library(false, cx),
            "library:remove-selected-hash" => self.request_remove_from_library(true, cx),
            "library:open-ignores" => {
                self.open_settings(cx);
                self.settings_notice = Some("忽略列表位于设置页底部「忽略列表」".to_string());
                cx.notify();
            }
            // F-061：命令面板接入备份/恢复；具体实现与设置页共用同一套方法。
            "backup:now" => self.start_backup(cx),
            "backup:restore" => self.pick_restore_backup(cx),
            "backup:cancel-pending" => self.cancel_pending_restore(cx),
            "backup:settings" => {
                self.open_settings(cx);
                self.settings_notice =
                    Some("数据库备份、自动备份计划与恢复入口位于「数据库备份」".to_string());
                cx.notify();
            }
            "org:new-album" => {
                self.open_org_dialog(crate::sidebar::OrgDialog::NewAlbum, cx);
            }
            "org:save-collection" => {
                self.save_search_open = true;
                self.focus_grid_pending = false;
                cx.notify();
            }
            // F-005：分组维度切换（菜单/命令面板同一入口）。
            "group:none" => self.set_group_mode(GroupMode::None, cx),
            "group:modified" => self.set_group_mode(GroupMode::Modified, cx),
            "group:captured" => self.set_group_mode(GroupMode::Captured, cx),
            "group:source" => self.set_group_mode(GroupMode::Source, cx),
            "group:rating" => self.set_group_mode(GroupMode::Rating, cx),
            // F-004：以当前选中图为锚点查找相似。
            "similar:find" => self.open_similar_selected(cx),
            // F-003：从选中 2–4 张进入图片对比。
            "compare:selected" => self.open_compare_selected(cx),
            _ => {}
        }
        cx.notify();
    }

    pub(crate) fn sort_menu(&self, this: &Entity<Self>) -> Menu {
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
            .item(menu(this, SortMode::Captured))
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

    pub(crate) fn display_menu(&self, this: &Entity<Self>) -> Menu {
        let sidebar = this.clone();
        let extensions = this.clone();
        let group = self.group_menu(this);
        Menu::new()
            .item(
                MenuItem::check("侧栏", self.show_sidebar)
                    .icon(IconName::PanelLeft)
                    .on_click(move |_, cx| {
                        let _ = sidebar.update(cx, |app, cx| {
                            app.set_show_sidebar(!app.show_sidebar, cx)
                        });
                    }),
            )
            .item(
                MenuItem::check("显示文件扩展名", self.show_extensions)
                    .icon(IconName::FileType)
                    .on_click(move |_, cx| {
                        let _ = extensions.update(cx, |app, cx| {
                            app.set_show_extensions(!app.show_extensions, cx)
                        });
                    }),
            )
            .separator()
            .item(MenuItem::submenu("分组", group))
    }

    pub(crate) fn view_menu(&self, this: &Entity<Self>) -> Menu {
        let mut menu = Menu::new();
        for mode in ViewMode::ALL {
            let this = this.clone();
            menu = menu.item(
                MenuItem::radio(mode.label(), self.view_mode == mode)
                    .icon(mode.icon())
                    .keys(mode.shortcut())
                    .on_click(move |_, cx| {
                        let _ = this.update(cx, |app, cx| app.set_view_mode(mode, cx));
                    }),
            );
        }
        let details = this.clone();
        let preview = this.clone();
        let display = self.display_menu(this);
        menu.separator()
            .item(
                MenuItem::radio("详细信息窗格", self.pane_mode == PaneMode::Details)
                    .icon(IconName::PanelRight)
                    .on_click(move |_, cx| {
                        let _ = details.update(cx, |app, cx| {
                            app.toggle_pane_mode(PaneMode::Details, cx)
                        });
                    }),
            )
            .item(
                MenuItem::radio("预览窗格", self.pane_mode == PaneMode::Preview)
                    .icon(IconName::Image)
                    .on_click(move |_, cx| {
                        let _ = preview.update(cx, |app, cx| {
                            app.toggle_pane_mode(PaneMode::Preview, cx)
                        });
                    }),
            )
            .separator()
            .item(MenuItem::submenu("分组", self.group_menu(this)))
            .item(MenuItem::submenu("显示", display))
    }

    pub(crate) fn export_menu(&self, this: &Entity<Self>) -> Menu {
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

    pub(crate) fn render_titlebar(
        &self,
        this: &Entity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
        filter_title: &str,
        visible: usize,
        search: &Entity<TextInput>,
        colors: UiColors,
    ) -> AnyElement {
        let library = self.screen == Screen::Library;
        let width = window.viewport_size().width;
        let centered_search = library && width >= px(1250.);
        let compact = width < px(1100.);
        let search_width = if centered_search {
            if width >= px(1900.) {
                px(420.)
            } else if width >= px(1600.) {
                px(360.)
            } else if width >= px(1400.) {
                px(300.)
            } else {
                px(220.)
            }
        } else if compact {
            px(180.)
        } else {
            px(220.)
        };
        let this_theme = this.clone();

        let sort = library.then(|| {
            DropdownMenu::new("library-sort", "排序", self.sort_menu(this))
                .icon(IconName::ArrowDownUp)
                .variant(ButtonVariant::Ghost)
        });
        let view = library.then(|| {
            DropdownMenu::new("library-view", "查看", self.view_menu(this))
                .icon(IconName::LayoutGrid)
                .variant(ButtonVariant::Ghost)
        });
        let theme = IconButton::new(
            "theme-toggle",
            if self.theme_dark {
                IconName::Sun
            } else {
                IconName::Moon
            },
        )
        .size(ControlSize::Md)
        .variant(ButtonVariant::Ghost)
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
                .on_mouse_down(MouseButton::Left, |_, window, _| {
                    window.prevent_default()
                })
                .child(
                    Input::new(search)
                        .clearable()
                        .least("搜索 / tag: / character: / size:>10mb"),
                )
        });

        let show_brand = !library || !compact;
        let mut left = div()
            .flex_none()
            .min_w(px(0.))
            .flex()
            .items_center()
            .gap(px(8.));
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
            left = left.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .min_w(px(0.))
                    .child(
                        div()
                            .max_w(px(if compact { 140. } else { 260. }))
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(filter_title.to_string()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(colors.muted)
                            .child(format!("{visible} 张")),
                    ),
            );
        } else {
            let label = match self.screen {
                Screen::Tags => "标签管理",
                Screen::Settings => "设置",
                Screen::Duplicates => "重复项",
                Screen::Similar => "查找相似",
                Screen::Compare => "图片对比",
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

        let mut controls = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.));
        if library {
            controls = controls
                .children(sort)
                .children(view)
                .children(export)
                .child(theme);
        } else {
            controls = controls.child(theme);
        }
        let right = div()
            .flex_none()
            .h_full()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(controls)
            .child(window_controls(window, colors));

        let drag_pressed = window.use_keyed_state("kanalib-titlebar-drag", cx, |_, _| false);
        let drag_down = drag_pressed.clone();
        let drag_move = drag_pressed.clone();
        let drag_up = drag_pressed.clone();
        let drag_out = drag_pressed;

        let mut bar = div()
            .id("kanalib-titlebar")
            .relative()
            .w_full()
            .flex_none()
            .h(px(TITLEBAR_HEIGHT))
            .pl(px(12.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(4.))
            .bg(colors.bg)
            .border_b_1()
            .border_color(colors.border)
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                if window.default_prevented() {
                    return;
                }
                if event.click_count == 2 {
                    window.zoom_window();
                    drag_down.update(cx, |pressed, _| *pressed = false);
                    return;
                }
                drag_down.update(cx, |pressed, _| *pressed = true);
            })
            .on_mouse_move(move |_, window, cx| {
                if *drag_move.read(cx) {
                    drag_move.update(cx, |pressed, _| *pressed = false);
                    window.start_window_move();
                }
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                drag_up.update(cx, |pressed, _| *pressed = false);
            })
            .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
                drag_out.update(cx, |pressed, _| *pressed = false);
            });

        if centered_search {
            let search_left = px((width.as_f32() - search_width.as_f32()) / 2.0);
            bar = bar.child(left).child(right);
            if let Some(search) = search_field {
                bar = bar.child(
                    div()
                        .absolute()
                        .top(px(0.))
                        .left(search_left)
                        .h_full()
                        .flex()
                        .items_center()
                        .on_mouse_down(MouseButton::Left, |_, window, _| {
                            window.prevent_default()
                        })
                        .child(search),
                );
            }
        } else {
            bar = bar.child(left).child(div().flex_1().min_w(px(8.)));
            if let Some(search) = search_field {
                bar = bar
                    .child(search)
                    .child(div().flex_1().min_w(px(8.)));
            }
            bar = bar.child(right);
        }

        bar.into_any_element()
    }

    // ---- F-036 拖拽导入 ----

    /// 处理一次外部文件拖放：文件夹提示「添加为来源」，来源内图片定位选中，
    /// 来源外图片询问是否添加父目录；非图片与重复来源只提示，不产生副作用。
    pub(crate) fn handle_external_drop(
        &mut self,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.drop_highlight = false;
        if paths.is_empty() {
            cx.notify();
            return;
        }
        let roots = self.state.shared.lock().unwrap().roots.clone();
        let mut dirs = Vec::new();
        let mut locate = Vec::new();
        let mut parents = Vec::new();
        let mut existing = 0usize;
        let mut unsupported = 0usize;
        let mut seen = HashSet::new();
        for path in paths {
            let key = path_key(&path);
            if !seen.insert(key) {
                continue;
            }
            let is_dir = path.is_dir();
            let image = is_image(&path);
            match classify_drop_path(&path, is_dir, image, &roots) {
                DropPathKind::NewSource => dirs.push(path),
                DropPathKind::ExistingSource => existing += 1,
                DropPathKind::Locate => locate.push(path),
                DropPathKind::AskParent => parents.push(path),
                DropPathKind::Unsupported => unsupported += 1,
            }
        }
        if existing > 0 {
            push_toast(
                &self.toaster,
                cx,
                format!("{existing} 个路径已在图库来源中"),
                Severity::Warning,
            );
        }
        if unsupported > 0 {
            push_toast(
                &self.toaster,
                cx,
                format!("{unsupported} 个路径不是支持的图片格式"),
                Severity::Warning,
            );
        }
        if !dirs.is_empty() || !parents.is_empty() {
            self.drop_prompt = Some(DropPrompt::AddSources { dirs, parents });
        }
        if !locate.is_empty() {
            self.locate_dropped_paths(locate, cx);
        }
        cx.notify();
    }

    /// 拖拽询问的确认：添加文件夹/父目录为来源并开始扫描；取消时没有任何改动。
    pub(crate) fn confirm_drop_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.drop_prompt.take() else {
            return;
        };
        let DropPrompt::AddSources { dirs, parents } = prompt;
        let mut targets = dirs;
        for path in parents {
            if let Some(parent) = path.parent() {
                targets.push(parent.to_path_buf());
            }
        }
        targets.sort();
        targets.dedup();
        if targets.is_empty() {
            cx.notify();
            return;
        }
        match self.state.add_roots(targets) {
            Ok(0) => push_toast(
                &self.toaster,
                cx,
                "所选文件夹已在图库中",
                Severity::Warning,
            ),
            Ok(added) => {
                push_toast(
                    &self.toaster,
                    cx,
                    format!("已添加 {added} 个来源并开始扫描"),
                    Severity::Success,
                );
                if let Some(search) = &self.search_input {
                    search.update(cx, |input, cx| input.set_text("", cx));
                }
                self.state.set_filter(Filter::All);
                self.state.start_scan();
                self.reset_search();
                self.screen = Screen::Library;
                self.dedup_selected = None;
                self.dedup_confirm = None;
                self.source_remove_confirm = None;
                self.clear_multi_selection();
                self.focus_grid_pending = true;
                self.auto_select_pending = true;
                self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
            Err(error) => {
                self.state.set_error(format!("添加来源失败：{error:#}"));
                push_toast(
                    &self.toaster,
                    cx,
                    format!("添加来源失败：{error:#}"),
                    Severity::Danger,
                );
            }
        }
        cx.notify();
    }

    /// 拖入来源内图片：清空搜索/筛选并选中这些图片，下一帧滚动到选中项。
    pub(crate) fn locate_dropped_paths(
        &mut self,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        self.screen = Screen::Library;
        if let Some(search) = &self.search_input {
            search.update(cx, |input, cx| input.set_text("", cx));
        }
        self.reset_search();
        self.state.set_filter(Filter::All);
        self.schedule_search(cx);
        let found = {
            let shared = self.state.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            paths
                .iter()
                .filter(|path| assets.iter().any(|asset| &asset.path == *path))
                .cloned()
                .collect::<Vec<_>>()
        };
        if found.is_empty() {
            push_toast(
                &self.toaster,
                cx,
                "图片尚未入库（可能已被忽略），请先重扫来源",
                Severity::Warning,
            );
            cx.notify();
            return;
        }
        let missing = paths.len().saturating_sub(found.len());
        self.selection = Arc::new(found.iter().cloned().collect());
        self.selection_anchor = found.first().cloned();
        self.state.select(found.first().cloned());
        self.auto_select_pending = false;
        self.focus_grid_pending = true;
        // 查询是防抖异步的：等结果刷新后再按可见位置滚动，避免用到旧索引。
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(
                    crate::search::SEARCH_DEBOUNCE_MS + 120,
                ))
                .await;
            let _ = weak.update(cx, |app, cx| {
                app.pending_scroll_position = Some(app.selected_visible_position());
                cx.notify();
            });
        })
        .detach();
        push_toast(
            &self.toaster,
            cx,
            format!("已定位 {} 张图片", found.len()),
            Severity::Success,
        );
        if missing > 0 {
            push_toast(
                &self.toaster,
                cx,
                format!("另有 {missing} 张未入库（可能已被忽略）"),
                Severity::Warning,
            );
        }
        cx.notify();
    }

    // ---- F-037 从图库移除与忽略列表 ----

    pub(crate) fn request_remove_from_library(
        &mut self,
        remember_hash: bool,
        cx: &mut Context<Self>,
    ) {
        let mut paths = self.selected_paths();
        if paths.is_empty() {
            push_toast(&self.toaster, cx, "先选择图片", Severity::Warning);
            return;
        }
        paths.sort();
        self.remove_assets_confirm = Some(RemoveAssetsRequest {
            paths,
            remember_hash,
        });
        cx.notify();
    }

    pub(crate) fn confirm_remove_from_library(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.remove_assets_confirm.take() else {
            return;
        };
        let result = if request.remember_hash {
            self.state.remove_from_library_with_hash(&request.paths)
        } else {
            self.state.remove_from_library(&request.paths)
        };
        match result {
            Ok(removed) => {
                let current = self
                    .viewer_at
                    .and_then(|at| self.viewer_slides.get(at))
                    .map(|(path, _)| path.clone());
                if current.is_some_and(|path| request.paths.contains(&path)) {
                    self.close_viewer(cx);
                }
                self.clear_multi_selection();
                self.state.select(None);
                self.auto_select_pending = true;
                push_toast(
                    &self.toaster,
                    cx,
                    format!("已从图库移除 {removed} 张 · 磁盘文件未删除"),
                    Severity::Success,
                );
            }
            Err(error) => {
                self.state
                    .set_error(format!("从图库移除失败：{error:#}"));
                push_toast(
                    &self.toaster,
                    cx,
                    format!("从图库移除失败：{error:#}"),
                    Severity::Danger,
                );
            }
        }
        cx.notify();
    }

    // ---- F-032 排除/忽略规则编辑器 ----

    pub(crate) fn open_scan_filter_editor(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let same = self
            .scan_filter_draft
            .as_ref()
            .is_some_and(|draft| draft.root == root);
        self.scan_filter_draft = if same {
            None
        } else {
            Some(ScanFilterDraft {
                config: self.state.scan_filter(&root),
                root,
                impact: None,
                notice: None,
                confirm_remove: false,
            })
        };
        cx.notify();
    }

    pub(crate) fn close_scan_filter_editor(&mut self, cx: &mut Context<Self>) {
        self.scan_filter_draft = None;
        cx.notify();
    }

    pub(crate) fn add_scan_filter_glob(&mut self, glob: &str, cx: &mut Context<Self>) {
        let Some(draft) = self.scan_filter_draft.as_mut() else {
            return;
        };
        let glob = glob.trim();
        if glob.is_empty() {
            draft.notice = Some("排除规则不能为空".to_string());
        } else if draft.config.exclude_globs.iter().any(|item| item == glob) {
            draft.notice = Some(format!("规则已存在：{glob}"));
        } else {
            draft.config.exclude_globs.push(glob.to_string());
            draft.impact = None;
            draft.confirm_remove = false;
            draft.notice = Some("已加入草稿；点击「预览命中」由核心校验并统计。".to_string());
        }
        cx.notify();
    }

    pub(crate) fn remove_scan_filter_glob(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(draft) = self.scan_filter_draft.as_mut() else {
            return;
        };
        if index < draft.config.exclude_globs.len() {
            draft.config.exclude_globs.remove(index);
            draft.impact = None;
            draft.confirm_remove = false;
            draft.notice = Some("已从草稿移除该规则（尚未保存）".to_string());
        }
        cx.notify();
    }

    pub(crate) fn toggle_scan_filter_hidden(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.scan_filter_draft.as_mut() else {
            return;
        };
        draft.config.include_hidden = !draft.config.include_hidden;
        draft.impact = None;
        draft.confirm_remove = false;
        cx.notify();
    }

    /// 用核心的 `scan_filter_indexed_impact` 计算「已入库但会被排除」的资产。
    pub(crate) fn preview_scan_filter_draft(
        &mut self,
        min_text: &str,
        max_text: &str,
        cx: &mut Context<Self>,
    ) {
        let Some((root, mut config)) = self.scan_filter_draft.as_ref().map(|draft| {
            (draft.root.clone(), draft.config.clone())
        }) else {
            return;
        };
        match filter_limits(min_text, max_text) {
            Ok((min, max)) => {
                config.min_bytes = min;
                config.max_bytes = max;
            }
            Err(message) => {
                self.draft_notice(message);
                cx.notify();
                return;
            }
        }
        let result = self.state.scan_filter_indexed_impact(&root, &config);
        if let Some(draft) = self.scan_filter_draft.as_mut() {
            draft.config = config;
            draft.confirm_remove = false;
            match result {
                Ok(impact) => {
                    draft.notice = Some(format!(
                        "预览：已入库 {} 张 · 命中排除 {} 张 · 保留 {} 张（保存前不会生效）",
                        impact.indexed,
                        impact.excluded.len(),
                        impact.kept
                    ));
                    draft.impact = Some(impact);
                }
                Err(error) => {
                    draft.notice = Some(format!("规则无效：{error:#}"));
                    draft.impact = None;
                }
            }
        }
        cx.notify();
    }

    /// 保存草稿。`remove` 首次点击只进入二次确认；再次点击才移出命中的 DB 记录。
    pub(crate) fn apply_scan_filter_draft(
        &mut self,
        remove: bool,
        min_text: &str,
        max_text: &str,
        cx: &mut Context<Self>,
    ) {
        let Some((root, mut config, confirmed)) = self.scan_filter_draft.as_ref().map(|draft| {
            (draft.root.clone(), draft.config.clone(), draft.confirm_remove)
        }) else {
            return;
        };
        match filter_limits(min_text, max_text) {
            Ok((min, max)) => {
                config.min_bytes = min;
                config.max_bytes = max;
            }
            Err(message) => {
                self.draft_notice(message);
                cx.notify();
                return;
            }
        }
        if remove && !confirmed {
            // 先算影响面并展示确认文案，避免误点直接移出。
            let impact = self.state.scan_filter_indexed_impact(&root, &config);
            if let Some(draft) = self.scan_filter_draft.as_mut() {
                draft.config = config;
                match impact {
                    Ok(impact) => {
                        draft.notice = Some(format!(
                            "确认从图库移出 {} 张记录？只删 DB，磁盘文件保留；再次点击「确认移出」执行。",
                            impact.excluded.len()
                        ));
                        draft.impact = Some(impact);
                        draft.confirm_remove = true;
                    }
                    Err(error) => {
                        draft.notice = Some(format!("规则无效：{error:#}"));
                        draft.impact = None;
                        draft.confirm_remove = false;
                    }
                }
            }
            cx.notify();
            return;
        }
        let action = if remove {
            ExcludedAssetAction::RemoveFromLibrary
        } else {
            ExcludedAssetAction::Keep
        };
        match self.state.apply_scan_filter(&root, config.clone(), action) {
            Ok(apply) => {
                if let Some(draft) = self.scan_filter_draft.as_mut() {
                    draft.config = config;
                    draft.impact = Some(apply.impact.clone());
                    draft.confirm_remove = false;
                    draft.notice = Some(if remove {
                        format!(
                            "已保存规则并从图库移出 {} 张（磁盘文件未删除）；重扫后这些文件仍会被规则排除。",
                            apply.removed
                        )
                    } else {
                        format!(
                            "已保存规则；已入库命中 {} 张保留记录，后续扫描会跳过。",
                            apply.impact.excluded.len()
                        )
                    });
                }
                push_toast(
                    &self.toaster,
                    cx,
                    if remove {
                        format!("排除规则已保存 · 移出 {} 张记录", apply.removed)
                    } else {
                        "排除规则已保存 · 保留命中记录".to_string()
                    },
                    Severity::Success,
                );
            }
            Err(error) => {
                if let Some(draft) = self.scan_filter_draft.as_mut() {
                    draft.notice = Some(format!("保存失败：{error:#}"));
                    draft.confirm_remove = false;
                }
                push_toast(
                    &self.toaster,
                    cx,
                    format!("保存排除规则失败：{error:#}"),
                    Severity::Danger,
                );
            }
        }
        cx.notify();
    }

    pub(crate) fn cancel_scan_filter_remove(&mut self, cx: &mut Context<Self>) {
        if let Some(draft) = self.scan_filter_draft.as_mut() {
            draft.confirm_remove = false;
            draft.notice = Some("已取消移出；规则仍可继续编辑".to_string());
        }
        cx.notify();
    }

    fn draft_notice(&mut self, message: String) {
        if let Some(draft) = self.scan_filter_draft.as_mut() {
            draft.notice = Some(message);
        }
    }

    // ---- F-037 忽略列表 ----

    pub(crate) fn add_ignored_glob(&mut self, root: PathBuf, pattern: String, cx: &mut Context<Self>) {
        match self.state.add_ignored_glob(&root, &pattern) {
            Ok(true) => {
                let message = format!("已添加忽略规则 · {pattern}（只影响后续扫描，不删除文件）");
                self.draft_notice(message.clone());
                self.settings_notice = Some(message);
                push_toast(&self.toaster, cx, "已添加忽略规则", Severity::Success);
            }
            Ok(false) => {
                let message = format!("忽略规则已存在：{pattern}");
                self.draft_notice(message.clone());
                self.settings_notice = Some(message);
            }
            Err(error) => {
                let message = format!("忽略规则无效：{error:#}");
                self.draft_notice(message.clone());
                self.settings_notice = Some(message);
            }
        }
        cx.notify();
    }

    pub(crate) fn restore_ignored_paths(
        &mut self,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        match self.state.restore_ignored_paths(&paths) {
            Ok(restored) => {
                let message = format!("已恢复 {restored} 条路径忽略项 · 重扫后重新导入");
                self.settings_notice = Some(message.clone());
                push_toast(&self.toaster, cx, message, Severity::Success);
            }
            Err(error) => {
                self.settings_notice = Some(format!("恢复忽略路径失败：{error:#}"));
            }
        }
        cx.notify();
    }

    pub(crate) fn restore_ignored_hashes(
        &mut self,
        hashes: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        match self.state.restore_ignored_hashes(&hashes) {
            Ok(restored) => {
                let message = format!("已恢复 {restored} 条哈希忽略项 · 重扫后重新导入");
                self.settings_notice = Some(message.clone());
                push_toast(&self.toaster, cx, message, Severity::Success);
            }
            Err(error) => {
                self.settings_notice = Some(format!("恢复哈希忽略项失败：{error:#}"));
            }
        }
        cx.notify();
    }

    pub(crate) fn restore_ignored_glob(
        &mut self,
        root: PathBuf,
        pattern: String,
        cx: &mut Context<Self>,
    ) {
        match self.state.restore_ignored_glob(&root, &pattern) {
            Ok(true) => {
                let message = format!("已恢复忽略规则 · {pattern} · 重扫后重新导入");
                self.settings_notice = Some(message.clone());
                push_toast(&self.toaster, cx, message, Severity::Success);
            }
            Ok(false) => {
                self.settings_notice = Some(format!("忽略规则不存在：{pattern}"));
            }
            Err(error) => {
                self.settings_notice = Some(format!("恢复忽略规则失败：{error:#}"));
            }
        }
        cx.notify();
    }

    pub(crate) fn restore_all_ignores(&mut self, cx: &mut Context<Self>) {
        let rules = self.state.ignore_rules();
        let mut restored = 0usize;
        if !rules.paths.is_empty() {
            let paths = rules
                .paths
                .iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            restored += self.state.restore_ignored_paths(&paths).unwrap_or(0);
        }
        if !rules.sha1.is_empty() {
            restored += self.state.restore_ignored_hashes(&rules.sha1).unwrap_or(0);
        }
        for (root, globs) in &rules.globs {
            for glob in globs {
                if self
                    .state
                    .restore_ignored_glob(Path::new(root), glob)
                    .unwrap_or(false)
                {
                    restored += 1;
                }
            }
        }
        let message = if restored > 0 {
            format!("已恢复 {restored} 条忽略项 · 重扫后重新导入")
        } else {
            "忽略列表已经是空的".to_string()
        };
        self.settings_notice = Some(message.clone());
        push_toast(&self.toaster, cx, message, Severity::Success);
        cx.notify();
    }
}

/// 设置页正在编辑的来源排除规则；保存前只影响草稿。
#[derive(Clone)]
pub(crate) struct ScanFilterDraft {
    pub(crate) root: PathBuf,
    pub(crate) config: ScanFilterConfig,
    pub(crate) impact: Option<ScanFilterImpact>,
    pub(crate) notice: Option<String>,
    /// 第一次点击「移出命中」后置位；二次确认才真正执行。
    pub(crate) confirm_remove: bool,
}

/// 「从图库移除」的待确认请求；只删 DB，不删磁盘。
#[derive(Clone)]
pub(crate) struct RemoveAssetsRequest {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) remember_hash: bool,
}

/// 拖拽导入待确认的内容；取消即丢弃，没有副作用。
#[derive(Clone)]
pub(crate) enum DropPrompt {
    AddSources {
        dirs: Vec<PathBuf>,
        parents: Vec<PathBuf>,
    },
}

/// 单个拖入路径的处理方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DropPathKind {
    /// 新文件夹：询问是否添加为来源。
    NewSource,
    /// 已是来源或位于某个来源内：只提示，不重复添加。
    ExistingSource,
    /// 来源内的图片：定位选中。
    Locate,
    /// 来源外的图片：询问是否添加父目录为来源。
    AskParent,
    /// 非图片文件：忽略并提示。
    Unsupported,
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

/// `path` 是否位于 `root` 之下（大小写不敏感，按路径分隔符边界判断）。
pub(crate) fn path_within(root: &Path, path: &Path) -> bool {
    let root = path_key(root);
    let path = path_key(path);
    if root.is_empty() || path == root {
        return false;
    }
    path.strip_prefix(&root)
        .is_some_and(|rest| rest.starts_with('\\'))
}

fn classify_drop_path(
    path: &Path,
    is_dir: bool,
    is_image: bool,
    roots: &[PathBuf],
) -> DropPathKind {
    if is_dir {
        let known = roots
            .iter()
            .any(|root| path_key(root) == path_key(path) || path_within(root, path));
        if known {
            DropPathKind::ExistingSource
        } else {
            DropPathKind::NewSource
        }
    } else if !is_image {
        DropPathKind::Unsupported
    } else if roots.iter().any(|root| path_within(root, path)) {
        DropPathKind::Locate
    } else {
        DropPathKind::AskParent
    }
}

/// 解析规则编辑器的最小/最大字节输入；空串表示不限制。
fn filter_limits(min_text: &str, max_text: &str) -> Result<(Option<u64>, Option<u64>), String> {
    let min = parse_optional_bytes(min_text, "最小大小")?;
    let max = parse_optional_bytes(max_text, "最大大小")?;
    if let (Some(min), Some(max)) = (min, max)
        && min > max
    {
        return Err("最小大小不能大于最大大小".to_string());
    }
    Ok((min, max))
}

#[cfg(test)]
mod drop_tests {
    use super::{DropPathKind, classify_drop_path, path_within};
    use std::path::{Path, PathBuf};

    fn roots() -> Vec<PathBuf> {
        vec![PathBuf::from(r"C:\Library")]
    }

    #[test]
    fn path_within_respects_separator_and_case() {
        let root = Path::new(r"C:\Library");
        assert!(path_within(root, Path::new(r"c:\library\2026\a.png")));
        assert!(!path_within(root, Path::new(r"C:\Library2\a.png")));
        assert!(!path_within(root, Path::new(r"C:\Library")));
        assert!(!path_within(root, Path::new(r"D:\Library\a.png")));
    }

    #[test]
    fn folders_are_sources_or_already_known() {
        assert_eq!(
            classify_drop_path(Path::new(r"C:\New Pics"), true, false, &roots()),
            DropPathKind::NewSource
        );
        assert_eq!(
            classify_drop_path(Path::new(r"c:\library\sub"), true, false, &roots()),
            DropPathKind::ExistingSource
        );
        assert_eq!(
            classify_drop_path(Path::new(r"C:\Library"), true, false, &roots()),
            DropPathKind::ExistingSource
        );
    }

    #[test]
    fn images_locate_inside_and_ask_parent_outside() {
        assert_eq!(
            classify_drop_path(Path::new(r"C:\Library\a.png"), false, true, &roots()),
            DropPathKind::Locate
        );
        assert_eq!(
            classify_drop_path(Path::new(r"D:\Elsewhere\a.png"), false, true, &roots()),
            DropPathKind::AskParent
        );
        assert_eq!(
            classify_drop_path(Path::new(r"C:\Library\notes.txt"), false, false, &roots()),
            DropPathKind::Unsupported
        );
    }
}
