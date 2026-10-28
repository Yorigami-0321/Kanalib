use crate::*;
use crate::app::{KanalibApp, push_toast};

/// 搜索输入停止多久后才真正查询（毫秒）。
pub(crate) const SEARCH_DEBOUNCE_MS: u64 = 220;

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
        self.search_debounce_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SEARCH_DEBOUNCE_MS))
                .await;
            let _ = weak.update(cx, |app, cx| {
                app.search_refresh_queued = false;
                app.run_search(cx);
            });
        }));
    }

    /// 执行一次查询。FTS 与分页下推都放到后台线程，避免阻塞渲染。
    pub(crate) fn run_search(&mut self, cx: &mut Context<Self>) {
        let text = self.search_pending.clone();
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
        let mut query = SearchQuery::parse(&text);
        // SQL 按标签名精确匹配，不会自己解析别名；这里先换成规范名，和内存路径保持一致。
        query.resolve_tag_aliases(&tag_aliases);
        let state = self.state.clone();
        let request_filter = filter.clone();
        let task = cx.background_executor().spawn(async move {
            let page = state
                .search_page(SearchPageRequest {
                    filter: &request_filter,
                    query: &query,
                    sort,
                    ascending,
                    limit: MAX_SEARCH_PAGE,
                    cursor: None,
                    duplicate_paths: duplicate_paths.as_ref(),
                    include_total: false,
                })
                .ok();
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
            (page, truncated, fts_paths)
        });
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let (page, truncated, fts_paths) = task.await;
            let _ = weak.update(cx, |app, cx| {
                app.search_running = false;
                let page_paths = match page {
                    Some(page) if !truncated => Some(Arc::new(page.paths)),
                    _ => None,
                };
                app.search_bundle = Some(Arc::new(SearchBundle {
                    text: text.clone(),
                    revision,
                    filter: filter.clone(),
                    sort,
                    ascending,
                    fts_paths,
                    page_paths,
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

    pub(crate) fn apply_saved_search(
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

}
