//! F-030 文件监听与增量同步：按来源启动 `notify` + 防抖，事件批处理只做单文件
//! 增量索引与安全删除；事件风暴回退到该来源的 `rescan_root`。
//!
//! 安全底线：监听只更新数据库与内存图库，绝不删除磁盘文件；只有来源可访问且
//! 路径确认缺失（`NotFound`）时才移除 DB 记录。来源离线、权限错误或存在性无法
//! 判断时一律保留记录。

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};

use crate::model::{ScanCounts, ScanFilterConfig, is_image};
use crate::scan_filter::{CompiledIgnoreRules, CompiledScanFilter, IgnoreRules};
use crate::state::{AppState, IndexedFile};

/// 防抖窗口；批量事件会在此窗口后合并处理。
pub const DEBOUNCE_WINDOW: Duration = Duration::from_millis(1_800);

/// 单批事件超过该阈值时不做逐文件索引，回退到整来源重扫。
pub const WATCH_EVENT_LIMIT: usize = 500;

/// 持有单个来源的防抖监听器；drop 时监听线程干净退出。
pub struct WatchHandle {
    _debouncer: Debouncer<RecommendedWatcher>,
}

/// 文件系统探测结果；`Uncertain` 表示权限/IO 错误，绝不能据此删除记录。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchProbe {
    File(u64),
    Missing,
    Directory,
    Uncertain,
}

/// 一批监听事件的分类结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchClassification {
    /// 存在且需要新增/更新的图片路径。
    pub changed: Vec<PathBuf>,
    /// 不存在且可能已删除的图片路径。
    pub missing: Vec<PathBuf>,
    /// 被排除、非图片、目录或无法判断而忽略的数量。
    pub ignored: usize,
    /// 单批事件超过阈值，需要回退整来源重扫。
    pub oversized: bool,
}

/// 为单个来源创建防抖监听器。失败由调用方标记来源错误并提示手动重扫。
pub(crate) fn spawn(state: &Arc<AppState>, root: PathBuf) -> Result<WatchHandle> {
    let weak = Arc::downgrade(state);
    let callback_root = root.clone();
    let mut debouncer = new_debouncer(DEBOUNCE_WINDOW, move |result: DebounceEventResult| {
        let Some(state) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(events) => {
                if events.is_empty() {
                    return;
                }
                let mut paths = Vec::with_capacity(events.len());
                for event in events {
                    paths.push(event.path);
                }
                handle_watch_batch(&state, &callback_root, paths);
            }
            Err(error) => {
                log::warn!("watch error under {}: {error}", callback_root.display());
                mark_source_error(
                    &state,
                    &callback_root,
                    format!("文件监听错误：{error}；请手动重新扫描该来源"),
                );
            }
        }
    })
    .context("创建文件监听器失败")?;
    debouncer
        .watcher()
        .watch(&root, RecursiveMode::Recursive)
        .with_context(|| format!("监听来源失败：{}", root.display()))?;
    Ok(WatchHandle {
        _debouncer: debouncer,
    })
}

fn probe_path(path: &Path) -> WatchProbe {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => WatchProbe::Directory,
        Ok(metadata) => WatchProbe::File(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => WatchProbe::Missing,
        Err(_) => WatchProbe::Uncertain,
    }
}

/// 把一批原始事件分类为“新增/修改”“缺失”与忽略项。
///
/// 纯函数：存在性由 `probe` 注入，便于单元测试；非法 glob 返回错误。
pub fn classify_watch_paths(
    root: &Path,
    config: &ScanFilterConfig,
    paths: impl IntoIterator<Item = PathBuf>,
    probe: impl FnMut(&Path) -> WatchProbe,
) -> Result<WatchClassification> {
    classify_watch_paths_with_ignores(
        root,
        config,
        &IgnoreRules::default(),
        paths,
        probe,
    )
}

/// 同时应用 F-032 排除规则与 F-037 持久忽略列表的事件分类。
pub fn classify_watch_paths_with_ignores(
    root: &Path,
    config: &ScanFilterConfig,
    rules: &IgnoreRules,
    paths: impl IntoIterator<Item = PathBuf>,
    mut probe: impl FnMut(&Path) -> WatchProbe,
) -> Result<WatchClassification> {
    let ignores = CompiledIgnoreRules::new(root, rules);
    let compiled = CompiledScanFilter::new(root, &ignores.merged_config(config))?;
    let unique = paths.into_iter().collect::<BTreeSet<_>>();
    let mut classification = WatchClassification::default();
    for path in unique {
        if !path.starts_with(root) {
            classification.ignored += 1;
            continue;
        }
        if ignores.path_ignored(&path) {
            classification.ignored += 1;
            continue;
        }
        match probe(&path) {
            WatchProbe::File(bytes) => {
                if is_image(&path) && !ignores.ignores(&compiled, &path, false, Some(bytes)) {
                    classification.changed.push(path);
                } else {
                    classification.ignored += 1;
                }
            }
            WatchProbe::Missing => {
                if is_image(&path) && !ignores.ignores(&compiled, &path, false, None) {
                    classification.missing.push(path);
                } else {
                    classification.ignored += 1;
                }
            }
            WatchProbe::Directory | WatchProbe::Uncertain => classification.ignored += 1,
        }
    }
    // 只统计真正需要处理的图片事件；超过阈值时交给整来源重扫，避免逐文件哈希/缩略图。
    if classification.changed.len() + classification.missing.len() > WATCH_EVENT_LIMIT {
        classification.oversized = true;
        classification.changed.clear();
        classification.missing.clear();
    }
    Ok(classification)
}

/// 监听批处理入口（运行在防抖线程上）。
pub(crate) fn handle_watch_batch(state: &Arc<AppState>, root: &Path, paths: Vec<PathBuf>) {
    if paths.is_empty() || state.is_scanning() {
        return;
    }
    let config = state.scan_filter(root);
    let rules = state.shared.lock().unwrap().ignore_rules.clone();
    let classification = match classify_watch_paths_with_ignores(
        root,
        &config,
        &rules,
        paths,
        probe_path,
    ) {
        Ok(classification) => classification,
        Err(error) => {
            log::warn!("watch classify {}: {error:#}", root.display());
            mark_source_error(
                state,
                root,
                format!("监听事件处理失败：{error:#}；请手动重新扫描该来源"),
            );
            return;
        }
    };
    if classification.oversized {
        log::warn!(
            "watch storm under {}; falling back to rescan",
            root.display()
        );
        if let Err(error) = state.rescan_root(root.to_path_buf()) {
            log::warn!("watch storm rescan {}: {error:#}", root.display());
            mark_source_error(
                state,
                root,
                format!("文件变化过多，自动重扫失败：{error:#}"),
            );
        }
        return;
    }
    if let Err(error) = apply_watch_batch(state, root, &classification) {
        log::warn!("watch batch {}: {error:#}", root.display());
        mark_source_error(
            state,
            root,
            format!("增量同步失败：{error:#}；请手动重新扫描该来源"),
        );
    }
}

/// 执行已分类的监听批次：先处理新增/修改（含 SHA1 重命名恢复），再安全删除。
pub(crate) fn apply_watch_batch(
    state: &Arc<AppState>,
    root: &Path,
    batch: &WatchClassification,
) -> Result<()> {
    let db = crate::db::Db::open(&state.db_path())?;
    let folder = root.to_string_lossy().to_string();
    let mut imported = 0usize;
    let mut updated = 0usize;
    let mut failed = 0usize;
    let mut renamed_old = HashSet::new();

    for path in &batch.changed {
        if state.is_scanning() {
            // 全量扫描已开始，交给扫描对账，避免两边同时写。
            return Ok(());
        }
        match state.watch_index_file(&db, root, path) {
            Ok(outcome) => {
                match outcome.kind {
                    IndexedFile::Unchanged | IndexedFile::Ignored => {}
                    IndexedFile::Imported | IndexedFile::Renamed => imported += 1,
                    IndexedFile::Updated => updated += 1,
                }
                if let Some(old_path) = outcome.renamed_from {
                    renamed_old.insert(old_path);
                }
                if outcome.hash_failed || outcome.thumb_failed {
                    failed += 1;
                }
            }
            Err(error) => {
                failed += 1;
                log::warn!("watch index {}: {error:#}", path.display());
            }
        }
    }

    // 新文件按现有打标队列语义入队；不在监听线程里启动推理。
    db.enqueue_tag_jobs(&folder)?;
    let queue = db.tag_job_counts(&folder)?;

    let mut deleted = HashSet::new();
    let mut removed = 0usize;
    if !batch.missing.is_empty() {
        // 来源可访问是删除的前提；扫描期间一律不删。
        let accessible = !state.is_scanning() && root.is_dir() && std::fs::read_dir(root).is_ok();
        if !accessible {
            failed += batch.missing.len();
        } else {
            let mut confirmed = Vec::new();
            for path in &batch.missing {
                if renamed_old.contains(path)
                    || !path.starts_with(root)
                    || !definitely_missing(path)
                {
                    continue;
                }
                confirmed.push(path.clone());
            }
            if !confirmed.is_empty() {
                removed = db.delete_assets(&confirmed)?;
                deleted.extend(confirmed);
            }
        }
    }

    let mut shared = state.shared.lock().unwrap();
    if !deleted.is_empty() {
        shared.remove_assets(&deleted);
        shared.refresh_after_removal(&deleted);
    }
    shared.set_queue_counts(queue.pending, queue.running, queue.error);
    shared.status = format!(
        "监听更新 · 变更 {} · 移除 {} · 队列 {}",
        batch.changed.len(),
        removed,
        queue.pending
    );
    let status = shared.source_status_mut(root);
    status.online = true;
    status.error = None;
    status.counts = ScanCounts {
        discovered: batch.changed.len() + batch.missing.len(),
        imported,
        updated,
        skipped: 0,
        removed,
        failed,
    };
    shared.error = None;
    shared.revision = shared.revision.wrapping_add(1);
    Ok(())
}

/// 只有明确的 `NotFound` 才算缺失；权限或 IO 错误一律不删。
fn definitely_missing(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        Ok(_) => false,
    }
}

pub(crate) fn mark_source_error(state: &Arc<AppState>, root: &Path, message: String) {
    let mut shared = state.shared.lock().unwrap();
    let status = shared.source_status_mut(root);
    status.online = root.is_dir();
    status.error = Some(message.clone());
    shared.error = Some(message);
    shared.status = format!("文件监听异常 · {}", root.display());
    shared.revision = shared.revision.wrapping_add(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config() -> ScanFilterConfig {
        ScanFilterConfig::default()
    }

    #[test]
    fn classifies_changed_missing_ignored_and_directories() {
        let root = PathBuf::from("C:/library");
        let config = ScanFilterConfig {
            exclude_globs: vec!["cache/**".into()],
            include_hidden: false,
            ..ScanFilterConfig::default()
        };
        let probes = HashMap::from([
            (root.join("new.png"), WatchProbe::File(10)),
            (root.join("edited.jpg"), WatchProbe::File(20)),
            (root.join("gone.png"), WatchProbe::Missing),
            (
                root.join("gone.txt"),
                WatchProbe::Missing,
            ),
            (root.join("cache").join("x.png"), WatchProbe::File(30)),
            (root.join(".hidden.png"), WatchProbe::File(40)),
            (root.join("sub"), WatchProbe::Directory),
            (root.join("denied.png"), WatchProbe::Uncertain),
            (PathBuf::from("C:/elsewhere/out.png"), WatchProbe::File(50)),
        ]);
        let classification = classify_watch_paths(
            &root,
            &config,
            probes.keys().cloned().collect::<Vec<_>>(),
            |path| probes.get(path).copied().unwrap_or(WatchProbe::Uncertain),
        )
        .unwrap();

        assert_eq!(
            classification.changed,
            vec![
                root.join("edited.jpg"),
                root.join("new.png"),
            ]
        );
        assert_eq!(classification.missing, vec![root.join("gone.png")]);
        assert_eq!(classification.ignored, 6);
        assert!(!classification.oversized);
    }

    #[test]
    fn event_storm_requests_rescan_without_per_file_work() {
        let root = PathBuf::from("C:/library");
        let paths = (0..=WATCH_EVENT_LIMIT)
            .map(|index| root.join(format!("image-{index}.png")))
            .collect::<Vec<_>>();
        let classification =
            classify_watch_paths(&root, &config(), paths, |_| WatchProbe::File(1)).unwrap();
        assert!(classification.oversized);
        assert!(classification.changed.is_empty());
        assert!(classification.missing.is_empty());
    }

    #[test]
    fn persistent_ignore_paths_and_globs_are_skipped() {
        let root = PathBuf::from("C:/library");
        let path = root.join("ignored.png");
        let mut rules = IgnoreRules::default();
        rules.insert_path(&path);
        rules.insert_glob(&root, "cache/**").unwrap();
        let probes = HashMap::from([
            (path.clone(), WatchProbe::File(10)),
            (root.join("cache").join("x.png"), WatchProbe::File(20)),
            (root.join("keep.png"), WatchProbe::File(30)),
        ]);
        let classification = classify_watch_paths_with_ignores(
            &root,
            &config(),
            &rules,
            probes.keys().cloned().collect::<Vec<_>>(),
            |path| probes.get(path).copied().unwrap_or(WatchProbe::Uncertain),
        )
        .unwrap();
        assert_eq!(classification.changed, vec![root.join("keep.png")]);
        assert_eq!(classification.ignored, 2);
    }

    #[test]
    fn duplicate_events_are_processed_once() {
        let root = PathBuf::from("C:/library");
        let path = root.join("a.png");
        let classification = classify_watch_paths(
            &root,
            &config(),
            vec![path.clone(), path.clone()],
            |_| WatchProbe::File(1),
        )
        .unwrap();
        assert_eq!(classification.changed, vec![path]);
    }

    #[test]
    fn invalid_glob_in_stored_config_is_reported() {
        let root = PathBuf::from("C:/library");
        let config = ScanFilterConfig {
            exclude_globs: vec!["[".into()],
            ..ScanFilterConfig::default()
        };
        assert!(
            classify_watch_paths(&root, &config, vec![root.join("a.png")], |_| {
                WatchProbe::File(1)
            })
            .is_err()
        );
    }

    #[test]
    fn definitely_missing_only_accepts_not_found() {
        let dir = std::env::temp_dir().join(format!(
            "kanalib-watch-missing-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("a.png");
        std::fs::write(&existing, b"x").unwrap();
        assert!(!definitely_missing(&existing));
        assert!(definitely_missing(&dir.join("gone.png")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
