use crate::*;
use crate::app::{KanalibApp, push_toast};

/// 搜索输入停止多久后才真正查询（毫秒）。
pub(crate) const SEARCH_DEBOUNCE_MS: u64 = 220;

/// UI 单次 SQL 探测上限。超过此数直接回退内存过滤，避免把 core 的
/// `MAX_SEARCH_PAGE`（10000）当成探测窗口：全命中时 extent 会扫到 8 万条，
/// 并让可见索引持有上万路径，叠加缩略图缓存抖动后 UI 端到端可达数秒。
/// 结果仍由内存过滤补齐，语义不变。
pub(crate) const UI_SEARCH_PAGE_LIMIT: usize = 512;

/// 只读计时探针：`KANALIB_SEARCH_TIMING=1` 时把防抖与查询耗时写进日志，
/// 不改变任何查询/渲染行为。
pub(crate) fn search_timing_enabled() -> bool {
    std::env::var_os("KANALIB_SEARCH_TIMING").is_some()
}

/// 查询里是否带了 `dup:true`。与 core 的解析规则保持一致：整词、带 `-` 前缀的取反写法不算。
pub(crate) fn query_wants_duplicates(text: &str) -> bool {
    text.split_whitespace().any(|raw| {
        if raw.starts_with('-') {
            return false;
        }
        raw.to_ascii_lowercase()
            .strip_prefix("dup:")
            .is_some_and(|value| value.trim() == "true")
    })
}

/// 一次搜索的结果快照。防抖结束后整体替换，避免新旧查询的结果混用。
pub(crate) struct SearchBundle {
    /// 已应用的查询文本。
    pub(crate) text: String,
    /// 生成快照时的数据版本；资产集合变了就不能再用分页结果。
    pub(crate) revision: u64,
    pub(crate) filter: Filter,
    pub(crate) sort: SearchSort,
    pub(crate) ascending: bool,
    /// FTS 命中的路径；自由文本为空时为 None。
    pub(crate) fts_paths: Option<Arc<HashSet<PathBuf>>>,
    /// `search_page` 返回的有序路径。结果超过分页上限时为 None，回退到内存过滤。
    pub(crate) page_paths: Option<Arc<Vec<PathBuf>>>,
    /// 输入触发防抖的时刻；只用于 `KANALIB_SEARCH_TIMING` 的端到端计时。
    pub(crate) input_at: Instant,
    /// 后台查询耗时（含 FTS 命中集合）；只用于计时日志。
    pub(crate) run_ms: f64,
}

impl SearchBundle {
    /// 快照是否还对应当前的查询、筛选、排序与数据版本。
    pub(crate) fn matches(
        &self,
        text: &str,
        revision: u64,
        filter: &Filter,
        sort: SearchSort,
        ascending: bool,
    ) -> bool {
        self.text == text
            && self.revision == revision
            && &self.filter == filter
            && self.sort == sort
            && self.ascending == ascending
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn query_wants_duplicates_only_for_dup_true() {
        assert!(super::query_wants_duplicates("dup:true"));
        assert!(super::query_wants_duplicates("sky dup:true"));
        assert!(super::query_wants_duplicates("DUP:TRUE"));
        assert!(!super::query_wants_duplicates("dup:false"));
        assert!(!super::query_wants_duplicates("-dup:true"));
        assert!(!super::query_wants_duplicates("sky"));
        assert!(!super::query_wants_duplicates(""));
    }

}

impl KanalibApp {
    /// 清空搜索状态；切换来源或重扫后调用。
    pub(crate) fn reset_search(&mut self) {
        self.last_search.clear();
        self.search_pending.clear();
        self.search_bundle = None;
        self.view.search_revision = self.view.search_revision.wrapping_add(1);
    }

    /// 输入防抖：停止输入 SEARCH_DEBOUNCE_MS 后才发起查询。
    /// 替换旧任务会取消它，所以连续输入只会跑最后一次查询。
    pub(crate) fn schedule_search(&mut self, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let queued_at = Instant::now();
        // 端到端计时起点：输入文本变化后第一次排入防抖的时刻。
        self.search_queued_at = Some(queued_at);
        self.search_debounce_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SEARCH_DEBOUNCE_MS))
                .await;
            if search_timing_enabled() {
                log::info!(
                    "search timing: debounce waited {:.1}ms",
                    queued_at.elapsed().as_secs_f64() * 1000.0
                );
            }
            let _ = weak.update(cx, |app, cx| {
                app.search_refresh_queued = false;
                app.run_search(cx);
            });
        }));
    }

    /// 执行一次查询。FTS 与分页下推都放到后台线程，避免阻塞渲染。
    pub(crate) fn run_search(&mut self, cx: &mut Context<Self>) {
        let text = self.search_pending.clone();
        let input_at = self.search_queued_at.take().unwrap_or_else(Instant::now);
        let wants_duplicates = query_wants_duplicates(&text);
        let sort = SearchSort::from_key(self.sort.key()).unwrap_or(SearchSort::Name);
        let ascending = self.sort_ascending;
        let (revision, filter, tag_aliases, duplicate_paths) = {
            let shared = self.state.shared.lock().unwrap();
            // 去重路径集合只在 dup:true 时构造：其它查询根本不读它。
            let duplicates = if wants_duplicates {
                Some(
                    shared
                        .dedup
                        .groups
                        .iter()
                        .chain(shared.dedup.similar_groups.iter())
                        .flat_map(|group| group.assets.iter().map(|asset| asset.path.clone()))
                        .collect::<HashSet<PathBuf>>(),
                )
            } else {
                None
            };
            (
                shared.revision,
                shared.filter.clone(),
                shared.tag_aliases.clone(),
                duplicates,
            )
        };
        if self
            .search_bundle
            .as_ref()
            .is_some_and(|bundle| bundle.matches(text.as_str(), revision, &filter, sort, ascending))
        {
            return;
        }
        self.search_running = true;
        let started = Instant::now();
        let mut query = SearchQuery::parse(&text);
        // SQL 按标签名精确匹配，不会自己解析别名；这里先换成规范名，和内存路径保持一致。
        query.resolve_tag_aliases(&tag_aliases);
        let state = self.state.clone();
        let request_filter = filter.clone();
        let task = cx.background_executor().spawn(async move {
            let page_started = Instant::now();
            let page = state
                .search_page(SearchPageRequest {
                    filter: &request_filter,
                    query: &query,
                    sort,
                    ascending,
                    limit: UI_SEARCH_PAGE_LIMIT,
                    cursor: None,
                    duplicate_paths: duplicate_paths.as_ref(),
                    include_total: false,
                })
                .ok();
            let page_ms = page_started.elapsed().as_secs_f64() * 1000.0;
            // 结果超过分页上限时只能回退到内存过滤，保留 FTS 命中集合。
            let truncated = page.as_ref().is_some_and(|page| page.next.is_some());
            let fts_paths = {
                let free = query.free_text();
                if free.is_empty() {
                    None
                } else {
                    state.search_paths(&free).ok().map(Arc::new)
                }
            };
            (page, truncated, fts_paths, page_ms)
        });
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let (page, truncated, fts_paths, page_ms) = task.await;
            let task_ms = started.elapsed().as_secs_f64() * 1000.0;
            let _ = weak.update(cx, |app, cx| {
                app.search_running = false;
                let page_paths = match page {
                    Some(page) if !truncated => Some(Arc::new(page.paths)),
                    _ => None,
                };
                if search_timing_enabled() {
                    log::info!(
                        "search timing: text={text:?} page_ms={page_ms:.1} \
                         run_search_ms={task_ms:.1} hits={} truncated={truncated}",
                        page_paths.as_ref().map_or(0, |paths| paths.len()),
                    );
                }
                app.search_bundle = Some(Arc::new(SearchBundle {
                    text: text.clone(),
                    revision,
                    filter: filter.clone(),
                    sort,
                    ascending,
                    fts_paths,
                    page_paths,
                    input_at,
                    run_ms: task_ms,
                }));
                app.last_search = text;
                app.view.search_revision = app.view.search_revision.wrapping_add(1);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.screen = Screen::Library;
        self.state.set_filter(filter);
        self.clear_multi_selection();
        self.focus_grid_pending = true;
        self.auto_select_pending = true;
        self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        // 筛选是查询的一部分，分页结果要按新筛选重跑。
        self.schedule_search(cx);
        cx.notify();
    }

    /// 兼容入口：应用集合时同时恢复排序/视图/窗格（实现见 `app.rs`）。
    pub(crate) fn apply_saved_search(
        &mut self,
        saved: &SavedSearch,
        search: &Entity<TextInput>,
        cx: &mut Context<Self>,
    ) {
        self.apply_saved_search_with_view(saved, search, cx);
    }

    pub(crate) fn delete_saved_search(&mut self, id: i64, cx: &mut Context<Self>) {
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

    pub(crate) fn confirm_save_search(&mut self, name: &str, cx: &mut Context<Self>) {
        self.confirm_save_search_with_view(name, cx);
    }

}
