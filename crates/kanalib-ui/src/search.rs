use crate::*;
use crate::app::{KanalibApp, push_toast};
use crate::theme::*;

/// 搜索输入停止多久后才真正查询（毫秒）。
pub(crate) const SEARCH_DEBOUNCE_MS: u64 = 220;

/// 最近查询最多保留的条数；只写 settings JSON，不加 schema。
pub(crate) const SEARCH_HISTORY_LIMIT: usize = 10;

/// F-015：解析 `search.history`。JSON 数组之外的格式一律视为空历史，
/// 过滤空白项、按大小写不敏感去重并截断到上限。
pub(crate) fn parse_search_history(raw: &str) -> Vec<String> {
    let entries = serde_json::from_str::<Vec<String>>(raw).unwrap_or_default();
    let mut history = Vec::new();
    for entry in entries {
        push_history_entry(&mut history, &entry);
    }
    history
}

fn push_history_entry(history: &mut Vec<String>, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return false;
    }
    let folded = query.to_lowercase();
    let mut next = history.clone();
    next.retain(|existing| existing.to_lowercase() != folded);
    next.insert(0, query.to_string());
    next.truncate(SEARCH_HISTORY_LIMIT);
    if *history == next {
        false
    } else {
        *history = next;
        true
    }
}

/// F-015：记录一次最近查询；空查询不记，重复项移到最前。返回是否变化。
pub(crate) fn record_search_history(history: &mut Vec<String>, query: &str) -> bool {
    push_history_entry(history, query)
}

/// 把查询按空白切成 chip；只做展示/删除，不改变解析语义。
pub(crate) fn search_tokens(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_string).collect()
}

/// 删除第 `index` 个查询 chip，保留其余 token 的相对顺序。
pub(crate) fn remove_search_token(query: &str, index: usize) -> String {
    let mut tokens = search_tokens(query);
    if index < tokens.len() {
        tokens.remove(index);
    }
    tokens.join(" ")
}

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

    use super::*;

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

    #[test]
    fn search_history_parses_filters_and_truncates() {
        assert_eq!(parse_search_history("not json"), Vec::<String>::new());
        assert_eq!(parse_search_history("[]"), Vec::<String>::new());
        let raw = r#"["  sky  ","","Sky","tag:clara","sky","a","b","c","d","e","f","g","h"]"#;
        let history = parse_search_history(raw);
        assert_eq!(history.len(), SEARCH_HISTORY_LIMIT);
        // 大小写不敏感去重，保留最后一次写法；空项被丢弃。
        assert_eq!(history.iter().filter(|entry| entry.eq_ignore_ascii_case("sky")).count(), 1);
        assert!(history.contains(&"tag:clara".to_string()));
    }

    #[test]
    fn search_history_records_move_to_front_and_ignore_empty() {
        let mut history = vec!["b".to_string(), "a".to_string()];
        assert!(record_search_history(&mut history, " a "));
        assert_eq!(history, vec!["a".to_string(), "b".to_string()]);
        assert!(!record_search_history(&mut history, "  "));
        assert!(!record_search_history(&mut history, "a"));
        assert_eq!(history, vec!["a".to_string(), "b".to_string()]);
        for index in 0..20 {
            record_search_history(&mut history, &format!("q{index}"));
        }
        assert_eq!(history.len(), SEARCH_HISTORY_LIMIT);
        assert_eq!(history.first().map(String::as_str), Some("q19"));
    }

    #[test]
    fn search_chips_split_and_remove_tokens() {
        assert_eq!(search_tokens("  tag:sky  size:>1mb "), vec![
            "tag:sky".to_string(),
            "size:>1mb".to_string()
        ]);
        assert_eq!(remove_search_token("tag:sky size:>1mb", 0), "size:>1mb");
        assert_eq!(remove_search_token("tag:sky size:>1mb", 1), "tag:sky");
        assert_eq!(remove_search_token("tag:sky", 9), "tag:sky");
        assert_eq!(remove_search_token("", 0), "");
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

    // ---- F-015：搜索历史 / 语法帮助 / 查询 chips ----

    /// 把查询写回搜索框并立即重跑；用于最近查询与 chip 删除。
    pub(crate) fn apply_search_text(&mut self, text: String, cx: &mut Context<Self>) {
        if let Some(search) = &self.search_input {
            search.update(cx, |input, cx| input.set_text(text.clone(), cx));
        }
        self.screen = Screen::Library;
        self.search_pending = text.clone();
        if self.remember_search(&text) {
            cx.notify();
        }
        self.schedule_search(cx);
    }

    /// 记录一次最近查询（空文本忽略），写入 `search.history`。
    pub(crate) fn remember_search(&mut self, query: &str) -> bool {
        if !record_search_history(&mut self.search_history, query) {
            return false;
        }
        let json = serde_json::to_string(&self.search_history).unwrap_or_else(|_| "[]".into());
        self.persist_setting("search.history", json);
        true
    }

    /// 搜索框回车/失焦时提交当前文本；不改变查询本身。
    pub(crate) fn commit_search_history(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &self.search_input else {
            return;
        };
        let text = search.read(cx).text().trim().to_string();
        if self.remember_search(&text) {
            cx.notify();
        }
    }

    pub(crate) fn clear_search_history(&mut self, cx: &mut Context<Self>) {
        if self.search_history.is_empty() {
            return;
        }
        self.search_history.clear();
        self.persist_setting("search.history", "[]");
        cx.notify();
    }

    /// 查询 chips：紧贴标题栏下方展示当前生效的查询 token，点击 × 移除。
    pub(crate) fn render_search_chips(
        &self,
        this: &Entity<Self>,
        colors: UiColors,
    ) -> Option<AnyElement> {
        if self.screen != Screen::Library {
            return None;
        }
        let tokens = search_tokens(&self.last_search);
        if tokens.is_empty() {
            return None;
        }
        let mut row = div()
            .id("search-chips")
            .flex_none()
            .w_full()
            .px(px(16.))
            .py(px(4.))
            .flex()
            .items_center()
            .gap(px(4.))
            .border_b_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child("查询"),
            );
        for (index, token) in tokens.into_iter().enumerate() {
            let this = this.clone();
            let label = token.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("search-chip-{index}")))
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(colors.sunken)
                    .text_xs()
                    .text_color(colors.fg)
                    .child(label)
                    .child(
                        div()
                            .id(SharedString::from(format!("search-chip-x-{index}")))
                            .cursor_pointer()
                            .px(px(2.))
                            .text_color(colors.subtle)
                            .hover(move |style| style.text_color(colors.danger))
                            .on_click(move |_, _, cx| {
                                let _ = this.update(cx, |app, cx| {
                                    app.remove_search_token(index, cx);
                                });
                            })
                            .child("×"),
                    ),
            );
        }
        row = row.child(div().flex_1()).child(
            div()
                .flex_none()
                .text_xs()
                .text_color(colors.subtle)
                .child("回车记录到最近查询 · 语法见搜索框右侧 ?"),
        );
        Some(row.into_any_element())
    }

    pub(crate) fn remove_search_token(&mut self, index: usize, cx: &mut Context<Self>) {
        let next = remove_search_token(&self.last_search, index);
        self.apply_search_text(next, cx);
    }
}
