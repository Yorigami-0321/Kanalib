use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use image::{ImageFormat, Rgb, RgbImage};
use serde::Deserialize;

use crate::db::{Db, JobCounts, StoredMeta};
use crate::dedup::{
    DEFAULT_SIMILAR_THRESHOLD, DuplicateReview, GroupKind, ReviewState, is_protected,
};
use crate::ffmpeg::{self, ThumbJob};
use crate::media;
use crate::model::{
    Asset, CountAccumulator, Filter, PendingCleanup, SavedSearch, ScanCounts, ScanFilterConfig,
    Shared, SourceScanStatus, TagHit, TagSummary, normalize_tag_name,
};
use crate::scan_filter::{self, ScanFilterPreview};
use crate::sidecar::{Sidecar, project_root, repair_file};
use crate::watch;

#[derive(Clone, Copy, Debug, Default)]
pub struct TrashSummary {
    pub groups: usize,
    pub assets: usize,
    pub failed: usize,
    pub protected_skipped: usize,
}

#[derive(Clone, Debug, Default)]
pub struct CacheStats {
    pub files: usize,
    pub bytes: u64,
    pub orphan_files: usize,
    pub orphan_bytes: u64,
}

pub struct AppState {
    pub shared: Mutex<Shared>,
    cancel: Mutex<Option<Arc<AtomicBool>>>,
    similar_scan_id: Mutex<u64>,
    /// 每个来源一个防抖监听器；清空即停止监听。
    watchers: Mutex<HashMap<PathBuf, watch::WatchHandle>>,
    /// 自身弱引用；`add_roots`/`remove_root` 只有 `&self`，启动监听时需要升级为 Arc。
    self_ref: Mutex<Weak<AppState>>,
    /// 是否允许启动文件监听（headless / KANALIB_DISABLE_WATCH 或测试时为 false）。
    watch_allowed: bool,
    db_path: PathBuf,
    cache_dir: PathBuf,
}

impl AppState {
    pub fn new(folder: Option<PathBuf>) -> Arc<Self> {
        let watch_allowed = std::env::var_os("KANALIB_HEADLESS").is_none()
            && std::env::var_os("KANALIB_DISABLE_WATCH").is_none();
        Self::open(project_root(), folder, watch_allowed)
    }

    /// 打开指定数据根目录；测试与 headless 复用同一套初始化逻辑。
    fn open(data_root: PathBuf, folder: Option<PathBuf>, watch_allowed: bool) -> Arc<Self> {
        let data_dir = data_root.join(".kanalib");
        let db_path = data_dir.join("kanalib.db");
        let cache_dir = data_dir.join("thumbs");
        let _ = std::fs::create_dir_all(&cache_dir);
        let initial_folder = folder.map(absolute);
        let (
            roots,
            assets,
            reviews,
            similar_threshold,
            protected_paths,
            queue_counts,
            saved_searches,
            blocked_tags,
        ) = match Db::open(&db_path) {
                Ok(db) => {
                    if let Err(error) = import_vocabulary(&db) {
                        log::warn!("vocab import: {error:#}");
                    }
                    if let Err(error) = backfill_vocab(&db) {
                        log::warn!("vocab backfill: {error:#}");
                    }
                    let saved_roots = db.setting("library.local_roots").ok().flatten();
                    let mut roots = match saved_roots.as_deref() {
                        Some(value) => serde_json::from_str::<Vec<String>>(value)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|path| absolute(PathBuf::from(path)))
                            .collect::<Vec<_>>(),
                        None => initial_folder.clone().into_iter().collect(),
                    };
                    let mut seen = HashSet::new();
                    roots.retain(|root| seen.insert(root.clone()));
                    if saved_roots.is_none() {
                        let values = roots
                            .iter()
                            .map(|root| root.to_string_lossy().to_string())
                            .collect::<Vec<_>>();
                        if let Err(error) = db.set_setting(
                            "library.local_roots",
                            &serde_json::to_string(&values).unwrap_or_default(),
                        ) {
                            log::warn!("save local roots: {error:#}");
                        }
                    }
                    let root_strings = roots
                        .iter()
                        .map(|root| root.to_string_lossy().to_string())
                        .collect::<Vec<_>>();
                    let assets = load_tile_thumbs(match db.load_library(&root_strings) {
                        Ok(assets) => assets,
                        Err(error) => {
                            log::error!("load assets: {error:#}");
                            Vec::new()
                        }
                    });
                    let reviews = match db.load_duplicate_reviews() {
                        Ok(reviews) => reviews,
                        Err(error) => {
                            log::warn!("load duplicate reviews: {error:#}");
                            Default::default()
                        }
                    };
                    let threshold = db
                        .setting("dedup.similar_threshold")
                        .ok()
                        .flatten()
                        .and_then(|value| value.parse::<u8>().ok())
                        .map(|value| value.clamp(2, 24))
                        .unwrap_or(DEFAULT_SIMILAR_THRESHOLD);
                    let protected_paths = db
                        .setting("dedup.protected_paths")
                        .ok()
                        .flatten()
                        .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
                        .unwrap_or_default()
                        .into_iter()
                        .map(PathBuf::from)
                        .collect::<Vec<_>>();
                    if let Err(error) = db.recover_tag_jobs() {
                        log::warn!("recover tag jobs: {error:#}");
                    }
                    let mut queue_counts = JobCounts::default();
                    for root in &root_strings {
                        if let Err(error) = db.enqueue_tag_jobs(root) {
                            log::warn!("enqueue tag jobs for {root}: {error:#}");
                        }
                        if let Ok(counts) = db.tag_job_counts(root) {
                            queue_counts.pending += counts.pending;
                            queue_counts.running += counts.running;
                            queue_counts.error += counts.error;
                        }
                    }
                    let saved_searches = db.load_saved_searches().unwrap_or_else(|error| {
                        log::warn!("load saved searches: {error:#}");
                        Vec::new()
                    });
                    let blocked_tags = db
                        .setting("tags.blocked")
                        .ok()
                        .flatten()
                        .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
                        .unwrap_or_default();
                    (
                        roots,
                        assets,
                        reviews,
                        threshold,
                        protected_paths,
                        queue_counts,
                        saved_searches,
                        blocked_tags,
                    )
                }
                Err(error) => {
                    log::error!("open database: {error:#}");
                    (
                        initial_folder.into_iter().collect(),
                        Vec::new(),
                        Default::default(),
                        DEFAULT_SIMILAR_THRESHOLD,
                        Vec::new(),
                        JobCounts::default(),
                        Vec::new(),
                        Vec::new(),
                    )
                }
            };
        let mut shared = Shared::new(roots, assets, reviews, similar_threshold, protected_paths);
        shared.saved_searches = saved_searches;
        shared.blocked_tags = blocked_tags.into_iter().collect();
        shared.tag_aliases = Db::open(&db_path)
            .ok()
            .and_then(|db| db.load_tag_aliases().ok())
            .unwrap_or_default();
        shared.source_aliases = Db::open(&db_path)
            .ok()
            .and_then(|db| db.setting("library.source_aliases").ok().flatten())
            .and_then(|value| serde_json::from_str::<HashMap<String, String>>(&value).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|(path, alias)| (absolute(PathBuf::from(path)), alias))
            .collect();
        shared.scan_filters = Db::open(&db_path)
            .ok()
            .and_then(|db| scan_filter::load_filters(&db).ok())
            .unwrap_or_default();
        shared.set_queue_counts(
            queue_counts.pending,
            queue_counts.running,
            queue_counts.error,
        );
        let state = Arc::new(Self {
            shared: Mutex::new(shared),
            cancel: Mutex::new(None),
            similar_scan_id: Mutex::new(0),
            watchers: Mutex::new(HashMap::new()),
            self_ref: Mutex::new(Weak::new()),
            watch_allowed,
            db_path,
            cache_dir,
        });
        *state.self_ref.lock().unwrap() = Arc::downgrade(&state);
        if watch_allowed {
            state.start_watching();
        }
        state
    }

    /// 测试用：隔离数据根目录，不启动文件监听。
    #[cfg(test)]
    pub(crate) fn open_for_test(data_root: PathBuf, folder: Option<PathBuf>) -> Arc<Self> {
        Self::open(data_root, folder, false)
    }

    pub fn add_roots(&self, paths: Vec<PathBuf>) -> Result<usize> {
        let mut added = Vec::new();
        {
            let shared = self.shared.lock().unwrap();
            for path in paths {
                let path = absolute(path);
                if !path.is_dir() {
                    bail!("不是文件夹: {}", path.display());
                }
                if !shared.roots.contains(&path) && !added.contains(&path) {
                    added.push(path);
                }
            }
        }
        if added.is_empty() {
            return Ok(0);
        }
        let roots = {
            let shared = self.shared.lock().unwrap();
            let mut roots = shared.roots.clone();
            roots.extend(added.iter().cloned());
            roots
        };
        self.save_roots(&roots)?;
        {
            let mut shared = self.shared.lock().unwrap();
            shared.roots = roots;
            for root in &added {
                let online = root.is_dir();
                shared
                    .source_statuses
                    .entry(root.clone())
                    .or_insert_with(|| SourceScanStatus {
                        root: root.clone(),
                        online,
                        ..SourceScanStatus::default()
                    })
                    .online = online;
            }
            shared.revision = shared.revision.wrapping_add(1);
            shared.status = format!("已添加 {} 个来源", added.len());
            shared.error = None;
        }
        self.start_watching();
        Ok(added.len())
    }

    pub fn remove_root(&self, root: &Path) -> Result<usize> {
        let root = absolute(root.to_path_buf());
        let roots = {
            let shared = self.shared.lock().unwrap();
            let mut roots = shared.roots.clone();
            let before = roots.len();
            roots.retain(|item| item != &root);
            if roots.len() == before {
                return Ok(0);
            }
            roots
        };
        self.save_roots(&roots)?;
        let removed_paths = {
            let shared = self.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            assets
                .iter()
                .filter(|asset| asset.library_root == root)
                .map(|asset| asset.path.clone())
                .collect::<HashSet<_>>()
        };
        let mut queue_counts = JobCounts::default();
        if let Ok(db) = Db::open(&self.db_path) {
            for folder in &roots {
                if let Ok(counts) = db.tag_job_counts(&folder.to_string_lossy()) {
                    queue_counts.pending += counts.pending;
                    queue_counts.running += counts.running;
                    queue_counts.error += counts.error;
                }
            }
        }
        let filters = {
            let shared = self.shared.lock().unwrap();
            let mut filters = shared.scan_filters.clone();
            filters.remove(&root);
            filters
        };
        if let Err(error) = scan_filter::save_filters(&Db::open(&self.db_path)?, &filters) {
            log::warn!("save scan filters after removing root: {error:#}");
        }
        let removed = {
            let mut shared = self.shared.lock().unwrap();
            shared.roots = roots;
            if matches!(&shared.filter, Filter::Source(source) if source == &root) {
                shared.filter = Filter::All;
            }
            let removed = shared.remove_assets(&removed_paths);
            shared.refresh_after_removal(&removed_paths);
            shared.source_statuses.remove(&root);
            shared.scan_filters = filters;
            shared.set_queue_counts(
                queue_counts.pending,
                queue_counts.running,
                queue_counts.error,
            );
            shared.status = if shared.roots.is_empty() {
                "请添加文件夹".to_string()
            } else {
                format!("已移除来源 · 磁盘文件未删除 · 剩余 {} 个来源", shared.roots.len())
            };
            shared.error = None;
            removed
        };
        self.bump_similar_scan_id();
        self.start_watching();
        Ok(removed)
    }

    fn save_roots(&self, roots: &[PathBuf]) -> Result<()> {
        let values = roots
            .iter()
            .map(|root| root.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        Db::open(&self.db_path)?
            .set_setting("library.local_roots", &serde_json::to_string(&values)?)
    }

    fn bump_similar_scan_id(&self) -> u64 {
        let mut id = self.similar_scan_id.lock().unwrap();
        *id = id.wrapping_add(1);
        *id
    }

    fn invalidate_similar_scan(&self) {
        self.bump_similar_scan_id();
        let mut shared = self.shared.lock().unwrap();
        shared.dedup.clear_similar();
        shared.revision = shared.revision.wrapping_add(1);
    }

    pub fn start_scan(self: &Arc<Self>) {
        self.invalidate_similar_scan();
        if let Some(flag) = self.cancel.lock().unwrap().take() {
            flag.store(true, Ordering::SeqCst);
        }
        let flag = Arc::new(AtomicBool::new(false));
        *self.cancel.lock().unwrap() = Some(flag.clone());
        let roots = self.shared.lock().unwrap().roots.clone();
        if roots.is_empty() {
            let mut shared = self.shared.lock().unwrap();
            shared.scanning = false;
            shared.status = "请添加文件夹".into();
            shared.revision = shared.revision.wrapping_add(1);
            return;
        }
        {
            let mut shared = self.shared.lock().unwrap();
            shared.scanning = true;
            shared.error = None;
            shared.pending_cleanup = None;
            shared.status = "扫描中...".into();
        }
        let state = self.clone();
        let skip_tag = std::env::var_os("KANALIB_SKIP_TAG").is_some();
        let spawned = std::thread::Builder::new()
            .name("kanalib-indexer".into())
            .spawn(move || {
                let result = scan_roots(&state, &roots, &flag, skip_tag);
                let root_strings = roots
                    .iter()
                    .map(|root| root.to_string_lossy().to_string())
                    .collect::<Vec<_>>();
                let mut queue = JobCounts::default();
                if let Ok(db) = Db::open(&state.db_path) {
                    for root in &root_strings {
                        if let Ok(counts) = db.tag_job_counts(root) {
                            queue.pending += counts.pending;
                            queue.running += counts.running;
                            queue.error += counts.error;
                        }
                    }
                }
                let mut shared = state.shared.lock().unwrap();
                shared.refresh_dedup();
                shared.set_queue_counts(queue.pending, queue.running, queue.error);
                shared.scanning = false;
                if let Err(error) = result {
                    shared.error = Some(format!("{error:#}"));
                    shared.status = "索引失败".into();
                } else if flag.load(Ordering::SeqCst) {
                    shared.status = format!("已停止 · {} 张图片", shared.counts.total());
                } else if queue.pending > 0 {
                    shared.status = format!(
                        "完成 · {} 张图片 · 队列 {} 张待处理 · {} 个来源",
                        shared.counts.total(),
                        queue.pending,
                        roots.len()
                    );
                } else if queue.error > 0 {
                    shared.status = format!(
                        "完成 · {} 张图片 · {} 个失败待处理 · {} 个来源",
                        shared.counts.total(),
                        queue.error,
                        roots.len()
                    );
                } else {
                    shared.status = format!(
                        "完成 · {} 张图片 · {} 个来源",
                        shared.counts.total(),
                        roots.len()
                    );
                }
                shared.revision = shared.revision.wrapping_add(1);
            });
        if spawned.is_err() {
            let mut shared = self.shared.lock().unwrap();
            shared.error = Some("无法创建索引线程".into());
            shared.scanning = false;
        }
    }

    pub fn scan_blocking(self: &Arc<Self>, skip_tag: bool) -> Result<()> {
        self.invalidate_similar_scan();
        let roots = self.shared.lock().unwrap().roots.clone();
        if roots.is_empty() {
            let mut shared = self.shared.lock().unwrap();
            shared.scanning = false;
            shared.status = "请添加文件夹".into();
            return Ok(());
        }
        {
            let mut shared = self.shared.lock().unwrap();
            shared.scanning = true;
            shared.error = None;
            shared.pending_cleanup = None;
            shared.status = "扫描中...".into();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let result = scan_roots(self, &roots, &cancel, skip_tag);
        {
            let mut shared = self.shared.lock().unwrap();
            shared.refresh_dedup();
            shared.scanning = false;
        }
        if let Err(error) = &result {
            let mut shared = self.shared.lock().unwrap();
            shared.error = Some(format!("{error:#}"));
            shared.status = "索引失败".into();
        }
        result
    }

    /// 各来源最近扫描状态快照（按根目录排序，在线状态实时刷新）。
    pub fn source_statuses(&self) -> Vec<SourceScanStatus> {
        self.shared.lock().unwrap().refresh_source_statuses()
    }

    /// 读取来源排除规则；未配置时返回默认（包含隐藏文件、无排除 glob）。
    pub fn scan_filter(&self, root: &Path) -> ScanFilterConfig {
        let root = absolute(root.to_path_buf());
        self.shared
            .lock()
            .unwrap()
            .scan_filters
            .get(&root)
            .cloned()
            .unwrap_or_default()
    }

    /// 保存来源排除规则；非法 glob 直接报错，不写入 settings。
    pub fn set_scan_filter(&self, root: &Path, config: ScanFilterConfig) -> Result<()> {
        let root = absolute(root.to_path_buf());
        scan_filter::CompiledScanFilter::new(&root, &config)?;
        let mut filters = {
            let shared = self.shared.lock().unwrap();
            if !shared.roots.contains(&root) {
                bail!("来源不存在: {}", root.display());
            }
            shared.scan_filters.clone()
        };
        if config.is_active() {
            filters.insert(root.clone(), config);
        } else {
            filters.remove(&root);
        }
        scan_filter::save_filters(&Db::open(&self.db_path)?, &filters)?;
        let mut shared = self.shared.lock().unwrap();
        shared.scan_filters = filters;
        shared.revision = shared.revision.wrapping_add(1);
        shared.status = format!("已更新来源排除规则 · {}", root.display());
        shared.error = None;
        Ok(())
    }

    /// 预览规则会命中的图片数与被排除样例；供来源设置 UI 使用。
    pub fn preview_scan_filter(
        &self,
        root: &Path,
        config: &ScanFilterConfig,
    ) -> Result<ScanFilterPreview> {
        let root = absolute(root.to_path_buf());
        {
            let shared = self.shared.lock().unwrap();
            if !shared.roots.contains(&root) {
                bail!("来源不存在: {}", root.display());
            }
        }
        scan_filter::preview_scan_filter(&root, config)
    }

    /// 重新扫描单个来源；扫描进行中拒绝。来源离线时只标记错误，不触发清理。
    ///
    /// 同一入口供来源右键菜单与监听事件风暴回退使用。
    pub fn rescan_root(self: &Arc<Self>, root: PathBuf) -> Result<()> {
        let skip_tag = std::env::var_os("KANALIB_SKIP_TAG").is_some();
        self.start_rescan(root, skip_tag)
    }

    fn start_rescan(self: &Arc<Self>, root: PathBuf, skip_tag: bool) -> Result<()> {
        let root = absolute(root);
        self.prepare_rescan(&root)?;
        // 与全量重扫保持一致：单来源扫描后旧相似组可能已失效。
        self.invalidate_similar_scan();
        let state = self.clone();
        let thread_root = root.clone();
        let spawned = std::thread::Builder::new()
            .name("kanalib-rescan".into())
            .spawn(move || {
                if let Err(error) = state.run_rescan(&thread_root, skip_tag) {
                    log::warn!("rescan {}: {error:#}", thread_root.display());
                }
            });
        if spawned.is_err() {
            let mut shared = self.shared.lock().unwrap();
            shared.scanning = false;
            shared.error = Some("无法创建重扫线程".into());
            shared.status = "重扫失败".into();
            shared.revision = shared.revision.wrapping_add(1);
            bail!("无法创建重扫线程");
        }
        Ok(())
    }

    fn prepare_rescan(&self, root: &Path) -> Result<()> {
        {
            let shared = self.shared.lock().unwrap();
            if !shared.roots.iter().any(|item| item == root) {
                bail!("来源不存在: {}", root.display());
            }
            if shared.scanning {
                bail!("扫描进行中，请完成后再重新扫描来源");
            }
        }
        if !root.is_dir() {
            let mut shared = self.shared.lock().unwrap();
            let status = shared.source_status_mut(root);
            status.online = false;
            status.error = Some(format!("来源不可访问：{}", root.display()));
            shared.error = Some(format!("来源离线，已保留图库记录：{}", root.display()));
            shared.status = format!("来源离线 · {}", root.display());
            shared.revision = shared.revision.wrapping_add(1);
            bail!("来源不可访问: {}", root.display());
        }
        let mut shared = self.shared.lock().unwrap();
        if shared.scanning {
            bail!("扫描进行中，请完成后再重新扫描来源");
        }
        shared.scanning = true;
        shared.error = None;
        shared.status = format!(
            "重新扫描来源 · {}",
            root.file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| root.display().to_string())
        );
        let status = shared.source_status_mut(root);
        status.online = true;
        status.error = None;
        shared.revision = shared.revision.wrapping_add(1);
        Ok(())
    }

    fn run_rescan(self: &Arc<Self>, root: &Path, skip_tag: bool) -> Result<()> {
        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancel.lock().unwrap() = Some(cancel.clone());
        let result = scan_one_root(self, root, &cancel, skip_tag, 1, 1);
        let mut queue = JobCounts::default();
        if let Ok(db) = Db::open(&self.db_path)
            && let Ok(counts) = db.tag_job_counts(&root.to_string_lossy())
        {
            queue = counts;
        }
        let mut shared = self.shared.lock().unwrap();
        shared.refresh_dedup();
        shared.set_queue_counts(queue.pending, queue.running, queue.error);
        shared.scanning = false;
        match &result {
            Ok(_) => {
                shared.status = format!(
                    "来源扫描完成 · {} · {} 张图片",
                    root.file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| root.display().to_string()),
                    shared.counts.total()
                );
            }
            Err(error) => {
                shared.error = Some(format!("{error:#}"));
                shared.status = "来源扫描失败".into();
            }
        }
        shared.revision = shared.revision.wrapping_add(1);
        let succeeded = result.is_ok();
        drop(shared);
        // 手动重扫成功后恢复该来源的监听；离线/失败时保留原状态，等下次重扫。
        if succeeded && root.is_dir() {
            self.start_watching();
        }
        result.map(|_| ())
    }

    /// 启动（或重启）所有来源的文件监听；headless/禁用时为空操作。
    pub fn start_watching(&self) {
        if !self.watch_allowed {
            return;
        }
        let Some(state) = self.self_ref.lock().unwrap().clone().upgrade() else {
            return;
        };
        let roots = self.shared.lock().unwrap().roots.clone();
        // 先把旧监听器移到临时表：重建成功后统一替换，单个来源失败时保留其旧监听器。
        let mut previous = std::mem::take(&mut *self.watchers.lock().unwrap());
        let mut handles = HashMap::new();
        for root in roots {
            match watch::spawn(&state, root.clone()) {
                Ok(handle) => {
                    handles.insert(root, handle);
                }
                Err(error) => {
                    log::warn!("watch {}: {error:#}", root.display());
                    if let Some(handle) = previous.remove(&root) {
                        handles.insert(root, handle);
                        continue;
                    }
                    let mut shared = self.shared.lock().unwrap();
                    let status = shared.source_status_mut(&root);
                    status.online = root.is_dir();
                    status.error =
                        Some(format!("无法监听来源：{error:#}；请手动重新扫描"));
                    shared.status = format!("文件监听失败 · {}", root.display());
                    shared.revision = shared.revision.wrapping_add(1);
                }
            }
        }
        let mut watchers = self.watchers.lock().unwrap();
        *watchers = handles;
    }

    /// 停止所有来源监听；监听线程由 `Debouncer` drop 干净退出。
    pub fn stop_watching(&self) {
        self.watchers.lock().unwrap().clear();
    }

    pub(crate) fn is_scanning(&self) -> bool {
        self.shared.lock().unwrap().scanning
    }

    /// 监听线程的增量单文件索引入口；复用批处理已打开的数据库连接。
    pub(crate) fn watch_index_file(
        self: &Arc<Self>,
        db: &Db,
        root: &Path,
        path: &Path,
    ) -> Result<FileIndexOutcome> {
        let folder = root.to_string_lossy();
        index_file(
            self,
            db,
            root,
            &folder,
            path,
            RenamePolicy::MissingOnDisk,
            None,
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn rescan_root_blocking(
        self: &Arc<Self>,
        root: PathBuf,
        skip_tag: bool,
    ) -> Result<()> {
        let root = absolute(root);
        self.prepare_rescan(&root)?;
        self.run_rescan(&root, skip_tag)
    }

    pub fn set_filter(&self, filter: Filter) {
        let mut shared = self.shared.lock().unwrap();
        let resolved = match &filter {
            Filter::Tag(name) => shared.tag_aliases.get(name).cloned().map(Filter::Tag),
            _ => None,
        };
        shared.filter = resolved.unwrap_or(filter);
    }

    pub fn select(&self, path: Option<PathBuf>) {
        self.shared.lock().unwrap().selected = path;
    }

    pub fn set_favorite(&self, paths: &[PathBuf], favorite: bool) -> Result<usize> {
        if paths.is_empty() {
            return Ok(0);
        }
        let updated = Db::open(&self.db_path)?.set_favorite(paths, favorite)?;
        let targets = paths.iter().map(PathBuf::as_path).collect::<HashSet<_>>();
        let mut shared = self.shared.lock().unwrap();
        let Shared {
            assets,
            counts,
            revision,
            status,
            error,
            ..
        } = &mut *shared;
        {
            let mut assets = assets.write().unwrap();
            for asset in assets.iter_mut() {
                if targets.contains(asset.path.as_path()) && asset.favorite != favorite {
                    counts.remove(asset);
                    asset.favorite = favorite;
                    counts.add(asset);
                }
            }
        }
        *revision = revision.wrapping_add(1);
        *status = if favorite {
            format!("已收藏 {updated} 张")
        } else {
            format!("已取消收藏 {updated} 张")
        };
        *error = None;
        Ok(updated)
    }

    pub fn set_star(&self, paths: &[PathBuf], star: u8) -> Result<usize> {
        if paths.is_empty() {
            return Ok(0);
        }
        let star = star.min(5);
        let updated = Db::open(&self.db_path)?.set_star(paths, star)?;
        let targets = paths.iter().map(PathBuf::as_path).collect::<HashSet<_>>();
        let mut shared = self.shared.lock().unwrap();
        let Shared {
            assets,
            revision,
            status,
            error,
            ..
        } = &mut *shared;
        {
            let mut assets = assets.write().unwrap();
            for asset in assets.iter_mut() {
                if targets.contains(asset.path.as_path()) {
                    asset.star = star;
                }
            }
        }
        *revision = revision.wrapping_add(1);
        *status = if star == 0 {
            format!("已清除 {updated} 张的星级")
        } else {
            format!("已设置 {updated} 张为 {star} 星")
        };
        *error = None;
        Ok(updated)
    }

    pub fn add_tags(&self, paths: &[PathBuf], name: &str) -> Result<usize> {
        let name = normalize_tag_name(name);
        if name.is_empty() {
            bail!("标签名不能为空");
        }
        if paths.is_empty() {
            return Ok(0);
        }
        let targets = paths.iter().map(PathBuf::as_path).collect::<HashSet<_>>();
        let to_add = {
            let shared = self.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            assets
                .iter()
                .filter(|asset| {
                    targets.contains(asset.path.as_path())
                        && !asset
                            .all_tags()
                            .any(|tag| tag.tag.eq_ignore_ascii_case(&name))
                })
                .map(|asset| asset.path.clone())
                .collect::<Vec<_>>()
        };
        if to_add.is_empty() {
            return Ok(0);
        }
        let updated = Db::open(&self.db_path)?.add_manual_tags(&to_add, &name, "manual")?;
        let added = to_add.iter().map(PathBuf::as_path).collect::<HashSet<_>>();
        let mut shared = self.shared.lock().unwrap();
        let Shared {
            assets,
            counts,
            revision,
            status,
            error,
            ..
        } = &mut *shared;
        {
            let mut assets = assets.write().unwrap();
            for asset in assets.iter_mut() {
                if added.contains(asset.path.as_path())
                    && !asset
                        .all_tags()
                        .any(|tag| tag.tag.eq_ignore_ascii_case(&name))
                {
                    counts.remove(asset);
                    asset.manual_tags.push(TagHit {
                        tag: name.clone(),
                        category: "manual".into(),
                        score: 1.0,
                        vocab_index: None,
                    });
                    counts.add(asset);
                }
            }
        }
        *revision = revision.wrapping_add(1);
        *status = format!("已为 {updated} 张添加标签 #{name}");
        *error = None;
        Ok(updated)
    }

    pub fn remove_tags(&self, paths: &[PathBuf], name: &str) -> Result<usize> {
        let name = normalize_tag_name(name);
        if name.is_empty() || paths.is_empty() {
            return Ok(0);
        }
        let updated = Db::open(&self.db_path)?.remove_tags(paths, &name)?;
        let targets = paths.iter().map(PathBuf::as_path).collect::<HashSet<_>>();
        let mut shared = self.shared.lock().unwrap();
        let Shared {
            assets,
            counts,
            revision,
            status,
            error,
            ..
        } = &mut *shared;
        {
            let mut assets = assets.write().unwrap();
            for asset in assets.iter_mut() {
                if !targets.contains(asset.path.as_path()) {
                    continue;
                }
                let has_tag = asset
                    .all_tags()
                    .any(|tag| tag.tag.eq_ignore_ascii_case(&name));
                if !has_tag {
                    continue;
                }
                counts.remove(asset);
                asset
                    .tags
                    .retain(|tag| !tag.tag.eq_ignore_ascii_case(&name));
                asset
                    .manual_tags
                    .retain(|tag| !tag.tag.eq_ignore_ascii_case(&name));
                counts.add(asset);
            }
        }
        *revision = revision.wrapping_add(1);
        *status = format!("已从 {updated} 张移除标签 #{name}");
        *error = None;
        Ok(updated)
    }

    pub fn setting(&self, key: &str) -> Option<String> {
        Db::open(&self.db_path)
            .ok()
            .and_then(|db| db.setting(key).ok().flatten())
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        Db::open(&self.db_path)?.set_setting(key, value)
    }

    pub fn list_tags(&self) -> Result<Vec<TagSummary>> {
        let roots = self
            .shared
            .lock()
            .unwrap()
            .roots
            .iter()
            .map(|root| root.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        Db::open(&self.db_path)?.list_tags_in(&roots)
    }

    /// 全词表 + 当前图库用量；未使用的标签保留 0 次，供补全使用。
    pub fn tag_vocabulary(&self) -> Result<Vec<TagSummary>> {
        let roots = self
            .shared
            .lock()
            .unwrap()
            .roots
            .iter()
            .map(|root| root.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let db = Db::open(&self.db_path)?;
        let used = db.list_tags_in(&roots)?;
        let names = db.list_tag_names()?;
        let mut used = used
            .into_iter()
            .map(|tag| (tag.name.clone(), tag))
            .collect::<HashMap<_, _>>();
        let mut vocabulary = names
            .into_iter()
            .map(|(name, category)| {
                used.remove(&name).unwrap_or(TagSummary {
                    name,
                    category,
                    count: 0,
                    manual_count: 0,
                })
            })
            .collect::<Vec<_>>();
        vocabulary.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(vocabulary)
    }

    pub fn set_source_alias(&self, root: &Path, alias: &str) -> Result<()> {
        let root = absolute(root.to_path_buf());
        let alias = alias.trim();
        let mut aliases = {
            let shared = self.shared.lock().unwrap();
            if !shared.roots.contains(&root) {
                bail!("来源不存在: {}", root.display());
            }
            shared.source_aliases.clone()
        };
        if alias.is_empty() {
            aliases.remove(&root);
        } else {
            aliases.insert(root.clone(), alias.to_string());
        }
        let values = aliases
            .iter()
            .map(|(path, alias)| {
                (
                    path.to_string_lossy().to_string(),
                    alias.to_string(),
                )
            })
            .collect::<HashMap<_, _>>();
        Db::open(&self.db_path)?
            .set_setting("library.source_aliases", &serde_json::to_string(&values)?)?;
        let mut shared = self.shared.lock().unwrap();
        shared.source_aliases = aliases;
        shared.revision = shared.revision.wrapping_add(1);
        shared.status = if alias.is_empty() {
            format!("已恢复来源默认名称 · {}", root.display())
        } else {
            format!("已重命名来源为 {alias}")
        };
        shared.error = None;
        Ok(())
    }

    pub fn set_tag_blocked(&self, name: &str, blocked: bool) -> Result<()> {
        let name = normalize_tag_name(name);
        if name.is_empty() {
            bail!("标签名不能为空");
        }
        let mut values = {
            let shared = self.shared.lock().unwrap();
            shared.blocked_tags.iter().cloned().collect::<HashSet<_>>()
        };
        let changed = if blocked {
            values.insert(name.clone())
        } else {
            values.remove(&name)
        };
        if !changed {
            return Ok(());
        }
        let mut list = values.into_iter().collect::<Vec<_>>();
        list.sort();
        Db::open(&self.db_path)?.set_setting("tags.blocked", &serde_json::to_string(&list)?)?;
        let mut shared = self.shared.lock().unwrap();
        shared.blocked_tags = list.into_iter().collect();
        shared.revision = shared.revision.wrapping_add(1);
        shared.status = if blocked {
            format!("已屏蔽标签 #{name}")
        } else {
            format!("已取消屏蔽标签 #{name}")
        };
        shared.error = None;
        Ok(())
    }

    fn reload_after_tag_change(&self, status: String) -> Result<()> {
        let roots = self
            .shared
            .lock()
            .unwrap()
            .roots
            .iter()
            .map(|root| root.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let db = Db::open(&self.db_path)?;
        let assets = load_tile_thumbs(db.load_library(&roots)?);
        let aliases = db.load_tag_aliases()?;
        let mut shared = self.shared.lock().unwrap();
        let target = shared.assets.clone();
        *target.write().unwrap() = assets;
        {
            let assets = target.read().unwrap();
            shared.counts = CountAccumulator::from_assets(&assets);
            shared.dedup.rebuild(&assets);
        }
        shared.tag_aliases = aliases;
        shared.revision = shared.revision.wrapping_add(1);
        shared.status = status;
        shared.error = None;
        Ok(())
    }

    pub fn rename_tag(&self, old: &str, new: &str) -> Result<usize> {
        let affected = Db::open(&self.db_path)?.rename_tag(old, new)?;
        self.reload_after_tag_change(format!(
            "已重命名标签 #{old} → #{new} · 影响 {affected} 张"
        ))?;
        Ok(affected)
    }

    pub fn merge_tags(&self, source: &str, target: &str) -> Result<usize> {
        let affected = Db::open(&self.db_path)?.merge_tags(source, target)?;
        self.reload_after_tag_change(format!(
            "已合并标签 #{source} → #{target} · 影响 {affected} 张"
        ))?;
        Ok(affected)
    }

    pub fn save_saved_search(
        &self,
        name: &str,
        filter: &Filter,
        query: &str,
    ) -> Result<SavedSearch> {
        let name = name.trim();
        if name.is_empty() {
            bail!("智能集合名称不能为空");
        }
        if self
            .shared
            .lock()
            .unwrap()
            .saved_searches
            .iter()
            .any(|search| search.name == name)
        {
            bail!("已存在同名智能集合");
        }
        let id = Db::open(&self.db_path)?.save_saved_search(name, filter, query)?;
        let saved = SavedSearch {
            id,
            name: name.to_string(),
            filter: filter.clone(),
            query: query.to_string(),
        };
        let mut shared = self.shared.lock().unwrap();
        shared.saved_searches.push(saved.clone());
        shared.revision = shared.revision.wrapping_add(1);
        shared.status = format!("已保存智能集合「{}」", saved.name);
        shared.error = None;
        Ok(saved)
    }

    pub fn delete_saved_search(&self, id: i64) -> Result<()> {
        Db::open(&self.db_path)?.delete_saved_search(id)?;
        let mut shared = self.shared.lock().unwrap();
        shared.saved_searches.retain(|search| search.id != id);
        shared.revision = shared.revision.wrapping_add(1);
        shared.status = "已删除智能集合".into();
        shared.error = None;
        Ok(())
    }

    pub fn retry_failed(&self) -> Result<usize> {
        let roots = self
            .shared
            .lock()
            .unwrap()
            .roots
            .iter()
            .map(|root| root.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let db = Db::open(&self.db_path)?;
        let mut total = 0;
        for root in &roots {
            total += db.retry_failed(root)?;
        }
        Ok(total)
    }

    pub fn cancel_pending_cleanup(&self) {
        let mut shared = self.shared.lock().unwrap();
        let Some(pending) = shared.pending_cleanup.take() else {
            return;
        };
        shared.error = None;
        shared.status = format!("已保留 {} 张缺失图片记录", pending.paths.len());
        shared.revision = shared.revision.wrapping_add(1);
    }

    /// 用户在弹窗中确认后，移除完整扫描确认缺失的资产记录；不触碰磁盘文件。
    pub fn confirm_pending_cleanup(&self) -> Result<usize> {
        let pending = self.shared.lock().unwrap().pending_cleanup.clone();
        let Some(pending) = pending else {
            return Ok(0);
        };
        let db = Db::open(&self.db_path)?;
        let paths = pending.paths.iter().cloned().collect::<Vec<_>>();
        let removed = db.delete_assets(&paths)?;
        let mut queue = JobCounts::default();
        for root in &pending.roots {
            let root = root.to_string_lossy();
            if let Ok(counts) = db.tag_job_counts(&root) {
                queue.pending += counts.pending;
                queue.running += counts.running;
                queue.error += counts.error;
            }
        }
        let mut shared = self.shared.lock().unwrap();
        shared.pending_cleanup = None;
        shared.remove_assets(&pending.paths);
        shared.refresh_after_removal(&pending.paths);
        shared.set_queue_counts(queue.pending, queue.running, queue.error);
        shared.error = None;
        shared.status = format!("已从图库移除 {} 张缺失图片记录", removed);
        shared.revision = shared.revision.wrapping_add(1);
        Ok(removed)
    }

    /// 容错重解单张图片：重建预览缓存并用容错模式重新打标，不修改原文件。
    pub fn repair_asset(self: &Arc<Self>, path: PathBuf) -> Result<()> {
        {
            let shared = self.shared.lock().unwrap();
            if shared.scanning {
                bail!("图库扫描进行中，完成后再修复");
            }
        }
        let state = self.clone();
        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        {
            let mut shared = self.shared.lock().unwrap();
            shared.status = format!("修复中 · {label}");
            shared.error = None;
            shared.revision = shared.revision.wrapping_add(1);
        }
        let thread_path = path.clone();
        let spawned = std::thread::Builder::new()
            .name("kanalib-repair".into())
            .spawn(move || {
                if let Err(error) = state.repair_asset_blocking(&thread_path) {
                    log::warn!("repair {}: {error:#}", thread_path.display());
                    let mut shared = state.shared.lock().unwrap();
                    shared.error = Some(format!("修复失败：{error:#}"));
                    shared.status = format!("修复失败 · {label}");
                    shared.revision = shared.revision.wrapping_add(1);
                }
            });
        if spawned.is_err() {
            let mut shared = self.shared.lock().unwrap();
            shared.error = Some("无法创建修复线程".into());
            shared.status = "修复失败".into();
            shared.revision = shared.revision.wrapping_add(1);
        }
        Ok(())
    }

    fn repair_asset_blocking(&self, path: &Path) -> Result<()> {
        let (mtime, bytes) = {
            let shared = self.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            let asset = assets
                .iter()
                .find(|asset| asset.path == path)
                .with_context(|| format!("资产不存在：{}", path.display()))?;
            (asset.mtime, asset.bytes)
        };
        let db = Db::open(&self.db_path)?;
        std::fs::create_dir_all(&self.cache_dir)?;
        let target = thumb_target(path, &self.cache_dir, mtime, bytes, 1);
        let outcome = repair_file(path, &target, Some(512))
            .with_context(|| format!("容错解码 {}", path.display()))?;
        let tile = ensure_tile_thumb(&target);
        let mut asset = db.update_media(
            path,
            Some(&target),
            outcome.width,
            outcome.height,
            outcome.damage.as_deref(),
        )?;
        db.record_thumb_source(&target, path, mtime, bytes, 1)?;
        asset.tile_thumb = tile;
        {
            let mut shared = self.shared.lock().unwrap();
            shared.upsert(asset);
            shared.status = "预览缓存已重建 · 加载打标模型...".into();
            shared.revision = shared.revision.wrapping_add(1);
        }

        let (mut sidecar, ready) = match Sidecar::spawn() {
            Ok(value) => value,
            Err(error) => {
                let mut shared = self.shared.lock().unwrap();
                shared.error = Some(format!("预览已重建，但打标模型加载失败：{error:#}"));
                shared.status = "预览已重建，打标未完成".into();
                shared.revision = shared.revision.wrapping_add(1);
                return Ok(());
            }
        };
        {
            let mut shared = self.shared.lock().unwrap();
            shared.sidecar_device = Some(if ready.half {
                format!("{} · FP16", ready.device)
            } else {
                ready.device
            });
            shared.status = "预览已重建 · 重新打标...".into();
            shared.revision = shared.revision.wrapping_add(1);
        }
        let item = sidecar
            .tag(&[path.to_path_buf()])?
            .into_iter()
            .next()
            .context("sidecar 未返回打标结果")?;
        if item.ok {
            let updated = {
                let shared = self.shared.lock().unwrap();
                let existing = shared
                    .assets
                    .read()
                    .unwrap()
                    .iter()
                    .find(|asset| asset.path == path)
                    .cloned();
                existing.map(|mut asset| {
                    asset.width = item.width.unwrap_or(asset.width);
                    asset.height = item.height.unwrap_or(asset.height);
                    asset.rating = item.rating.clone().unwrap_or_default();
                    asset.group = item.group.clone().unwrap_or_else(|| "未分类".to_string());
                    asset.tags = item.tags.clone().unwrap_or_default();
                    asset.tagged = true;
                    asset.tag_attempts = 0;
                    asset.error = None;
                    asset.damage = item.damage.clone();
                    asset
                })
            };
            let Some(asset) = updated else {
                bail!("资产在修复期间被移除：{}", path.display());
            };
            db.update_tags(&asset)?;
            let damage = asset.damage.clone();
            let name = asset.title();
            {
                let mut shared = self.shared.lock().unwrap();
                shared.upsert(asset);
                shared.status = if let Some(damage) = damage {
                    format!("修复完成（{damage}） · {name}")
                } else {
                    format!("修复完成，文件可正常读取 · {name}")
                };
                shared.revision = shared.revision.wrapping_add(1);
            }
        } else {
            let error = item
                .error
                .clone()
                .unwrap_or_else(|| "图片解码失败".to_string());
            if let Some(detail) = &item.error_detail {
                log::warn!("sidecar detail {}: {detail}", item.path);
            }
            let attempts = db.mark_error(path, &error, item.damage.as_deref())?;
            let updated = {
                let shared = self.shared.lock().unwrap();
                let mut asset = shared
                    .assets
                    .read()
                    .unwrap()
                    .iter()
                    .find(|asset| asset.path == path)
                    .cloned();
                if let Some(asset) = asset.as_mut() {
                    asset.error = Some(error.clone());
                    asset.damage = item.damage.clone();
                    asset.tag_attempts = attempts;
                }
                asset
            };
            if let Some(asset) = updated {
                let mut shared = self.shared.lock().unwrap();
                shared.upsert(asset);
                shared.error = Some(format!("预览已重建，但仍无法打标：{error}"));
                shared.status = "修复未完成 · 文件仍不可完整读取".into();
                shared.revision = shared.revision.wrapping_add(1);
            }
        }
        Ok(())
    }

    /// 把容错重编码的结果另存为新文件；原文件不会被改写。
    pub fn export_repaired_copy(self: &Arc<Self>, path: PathBuf, output: PathBuf) -> Result<()> {
        let state = self.clone();
        let label = output
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| output.display().to_string());
        {
            let mut shared = self.shared.lock().unwrap();
            shared.status = format!("正在导出修复副本 · {label}");
            shared.error = None;
            shared.revision = shared.revision.wrapping_add(1);
        }
        let thread_output = output.clone();
        let spawned = std::thread::Builder::new()
            .name("kanalib-repair-export".into())
            .spawn(move || {
                match repair_file(&path, &thread_output, None) {
                    Ok(outcome) => {
                        let mut shared = state.shared.lock().unwrap();
                        shared.status = if let Some(damage) = outcome.damage {
                            format!("已导出修复副本（{damage}） · {}", thread_output.display())
                        } else {
                            format!("已导出修复副本 · {}", thread_output.display())
                        };
                        shared.error = None;
                        shared.revision = shared.revision.wrapping_add(1);
                    }
                    Err(error) => {
                        log::warn!("export repair {}: {error:#}", path.display());
                        let mut shared = state.shared.lock().unwrap();
                        shared.error = Some(format!("导出修复副本失败：{error:#}"));
                        shared.status = "导出修复副本失败".into();
                        shared.revision = shared.revision.wrapping_add(1);
                    }
                }
            });
        if spawned.is_err() {
            let mut shared = self.shared.lock().unwrap();
            shared.error = Some("无法创建导出线程".into());
            shared.status = "导出修复副本失败".into();
            shared.revision = shared.revision.wrapping_add(1);
        }
        Ok(())
    }

    pub fn set_dedup_keeper(&self, key: &str, path: PathBuf, kind: GroupKind) -> bool {
        let review = {
            let mut shared = self.shared.lock().unwrap();
            let review = shared.dedup.set_keeper(key, &path, kind);
            if review.is_some() {
                shared.revision = shared.revision.wrapping_add(1);
            }
            review
        };
        let Some(review) = review else {
            return false;
        };
        match Db::open(&self.db_path).and_then(|db| db.save_duplicate_review(&review)) {
            Ok(()) => true,
            Err(error) => {
                log::warn!("save duplicate keeper: {error:#}");
                true
            }
        }
    }

    pub fn preselect_dedup(&self, kind: GroupKind) -> usize {
        let mut shared = self.shared.lock().unwrap();
        let selected = shared.dedup.preselect(kind);
        if selected > 0 {
            shared.status = format!("已按规则预选 {selected} 组");
            shared.revision = shared.revision.wrapping_add(1);
        }
        selected
    }

    pub fn mark_dedup(&self, key: &str, state: ReviewState, kind: GroupKind) -> Result<()> {
        let review = {
            let mut shared = self.shared.lock().unwrap();
            shared.dedup.set_state(key, state, kind)
        };
        let Some(review) = review else {
            bail!("duplicate group not found: {key}");
        };
        Db::open(&self.db_path)?.save_duplicate_review(&review)?;
        let mut shared = self.shared.lock().unwrap();
        shared.error = None;
        shared.revision = shared.revision.wrapping_add(1);
        shared.status = match state {
            ReviewState::Ignored => "已标记为不是重复".to_string(),
            ReviewState::Related => "已标记为相关变体".to_string(),
            ReviewState::Done => "已保留整组".to_string(),
            ReviewState::Pending => "已重新打开此组".to_string(),
        };
        Ok(())
    }

    pub fn set_similar_threshold(&self, threshold: u8) -> u8 {
        let threshold = threshold.clamp(2, 24);
        {
            let mut shared = self.shared.lock().unwrap();
            shared.dedup.similar_threshold = threshold;
            shared.revision = shared.revision.wrapping_add(1);
        }
        if let Err(error) = Db::open(&self.db_path)
            .and_then(|db| db.set_setting("dedup.similar_threshold", &threshold.to_string()))
        {
            log::warn!("save similar threshold: {error:#}");
        }
        threshold
    }

    pub fn add_protected_path(&self, path: PathBuf) -> Result<()> {
        let path = absolute(path);
        if !path.is_dir() {
            bail!("保护路径必须是文件夹: {}", path.display());
        }
        let paths = {
            let shared = self.shared.lock().unwrap();
            if shared
                .dedup
                .protected_paths
                .iter()
                .any(|item| item == &path)
            {
                return Ok(());
            }
            let mut paths = shared.dedup.protected_paths.clone();
            paths.push(path.clone());
            paths
        };
        self.save_protected_paths(&paths)?;
        let mut shared = self.shared.lock().unwrap();
        shared.dedup.set_protected_paths(paths);
        shared.refresh_dedup();
        shared.error = None;
        shared.status = format!("已添加保护路径：{}", path.display());
        Ok(())
    }

    pub fn remove_protected_path(&self, path: &Path) -> Result<()> {
        let path = absolute(path.to_path_buf());
        let paths = {
            let shared = self.shared.lock().unwrap();
            let mut paths = shared.dedup.protected_paths.clone();
            let before = paths.len();
            paths.retain(|item| item != &path);
            if paths.len() == before {
                return Ok(());
            }
            paths
        };
        self.save_protected_paths(&paths)?;
        let mut shared = self.shared.lock().unwrap();
        shared.dedup.set_protected_paths(paths);
        shared.refresh_dedup();
        shared.error = None;
        shared.status = format!("已移除保护路径：{}", path.display());
        Ok(())
    }

    fn save_protected_paths(&self, paths: &[PathBuf]) -> Result<()> {
        let values = paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let json = serde_json::to_string(&values)?;
        Db::open(&self.db_path)?.set_setting("dedup.protected_paths", &json)
    }

    pub fn db_path(&self) -> PathBuf {
        self.db_path.clone()
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.cache_dir.clone()
    }

    pub fn cache_stats(&self) -> Result<CacheStats> {
        Ok(self.scan_cache()?.0)
    }

    pub fn cleanup_orphan_thumbs(&self) -> Result<(usize, u64)> {
        let (_, orphans) = self.scan_cache()?;
        let mut removed = 0;
        let mut bytes = 0;
        for (path, size) in orphans {
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    removed += 1;
                    bytes += size;
                }
                Err(error) => log::warn!("remove thumb {}: {error}", path.display()),
            }
        }
        Ok((removed, bytes))
    }

    fn scan_cache(&self) -> Result<(CacheStats, Vec<(PathBuf, u64)>)> {
        let referenced = Db::open(&self.db_path)?.all_thumb_paths()?;
        let names = referenced
            .iter()
            .filter_map(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .collect::<HashSet<_>>();
        let mut stats = CacheStats::default();
        let mut orphans = Vec::new();
        let entries = match std::fs::read_dir(&self.cache_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((stats, orphans));
            }
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    log::warn!("read cache entry: {error}");
                    continue;
                }
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            stats.files += 1;
            stats.bytes += size;
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            let derived = name
                .strip_suffix("-t256.jpg")
                .map(|stem| format!("{stem}.jpg"));
            let used = names.contains(&name) || derived.is_some_and(|base| names.contains(&base));
            if !used {
                stats.orphan_files += 1;
                stats.orphan_bytes += size;
                orphans.push((path, size));
            }
        }
        Ok((stats, orphans))
    }

    pub fn start_similar_scan(self: &Arc<Self>) {
        let scan_id = self.bump_similar_scan_id();
        let (assets, threshold, revision, reviews, old_groups, protected) = {
            let mut shared = self.shared.lock().unwrap();
            if shared.scanning {
                shared.error = Some("图库扫描进行中，完成后再扫描相似图片".into());
                shared.revision = shared.revision.wrapping_add(1);
                return;
            }
            let assets = shared.assets.read().unwrap().clone();
            if assets.len() < 2 {
                shared.dedup.similar_scanning = false;
                shared.status = "图片不足 2 张，无法扫描相似图片".into();
                shared.revision = shared.revision.wrapping_add(1);
                return;
            }
            let threshold = shared.dedup.similar_threshold;
            let reviews = shared.dedup.review_snapshot();
            let old_groups = shared.dedup.similar_groups.clone();
            let protected = shared.dedup.protected_paths.clone();
            shared.dedup.similar_scanning = true;
            shared.dedup.similar_scanned = false;
            shared.dedup.similar_groups = Arc::new(Vec::new());
            shared.status = format!("相似扫描中 · 汉明距离 ≤{threshold} · {} 张", assets.len());
            shared.error = None;
            shared.revision = shared.revision.wrapping_add(1);
            (
                assets,
                threshold,
                shared.revision,
                reviews,
                old_groups,
                protected,
            )
        };

        let state = self.clone();
        let spawned = std::thread::Builder::new()
            .name("kanalib-similar".into())
            .spawn(move || {
                let groups = crate::dedup::compute_similar_groups(
                    &assets,
                    threshold,
                    &reviews,
                    &old_groups,
                    &protected,
                );
                let current_id = *state.similar_scan_id.lock().unwrap();
                if current_id != scan_id {
                    return;
                }
                let mut shared = state.shared.lock().unwrap();
                if shared.revision != revision {
                    shared.dedup.similar_scanning = false;
                    shared.status = "图库已变化，请重新扫描相似图片".into();
                    shared.revision = shared.revision.wrapping_add(1);
                    return;
                }
                let count = groups.len();
                let pending = groups
                    .iter()
                    .filter(|group| group.state == ReviewState::Pending)
                    .count();
                shared.dedup.set_similar_groups(groups, threshold);
                shared.status = format!("相似扫描完成 · {count} 组 · 未审 {pending} 组");
                shared.revision = shared.revision.wrapping_add(1);
            });
        if spawned.is_err() {
            let mut shared = self.shared.lock().unwrap();
            shared.dedup.similar_scanning = false;
            shared.error = Some("无法创建相似扫描线程".into());
            shared.revision = shared.revision.wrapping_add(1);
        }
    }

    pub fn trash_dedup_group(&self, key: &str) -> Result<TrashSummary> {
        let (roots, group, protected) = {
            let shared = self.shared.lock().unwrap();
            let group = shared
                .dedup
                .group(key)
                .cloned()
                .with_context(|| format!("duplicate group not found: {key}"))?;
            (
                shared.roots.clone(),
                group,
                shared.dedup.protected_paths.clone(),
            )
        };
        let keeper = group
            .keeper
            .clone()
            .context("duplicate group has no keeper")?;
        if !roots.iter().any(|root| keeper.starts_with(root)) {
            bail!("保留项不在当前图库内: {}", keeper.display());
        }
        let paths = group.duplicate_paths();
        if paths.is_empty() {
            bail!("这一组没有待移入回收站的副本");
        }
        if let Some(blocked) = paths.iter().find(|path| is_protected(path, &protected)) {
            bail!("受保护路径不能移入回收站: {}", blocked.display());
        }
        validate_under_roots(&roots, &paths)?;

        let report = crate::trash::move_to_recycle_bin(&paths)?;
        let moved = report.moved.iter().cloned().collect::<HashSet<_>>();
        Db::open(&self.db_path)?.delete_assets(&report.moved)?;
        let all_moved = report.failed.is_empty();
        if all_moved {
            Db::open(&self.db_path)?.save_duplicate_review(&DuplicateReview {
                group_key: group.key.clone(),
                state: ReviewState::Done,
                keeper_path: Some(keeper),
                member_count: group.member_count,
                signature: group.signature.clone(),
            })?;
        }

        let mut shared = self.shared.lock().unwrap();
        shared.remove_assets(&moved);
        shared.refresh_after_removal(&moved);
        shared.status = if all_moved {
            format!("已移入回收站 {} 张", report.moved.len())
        } else {
            format!(
                "已移入回收站 {} 张；{} 张仍待处理",
                report.moved.len(),
                report.failed.len()
            )
        };
        shared.error = report.error.clone();
        Ok(TrashSummary {
            groups: usize::from(all_moved),
            assets: report.moved.len(),
            failed: report.failed.len(),
            protected_skipped: 0,
        })
    }

    pub fn trash_preselected(&self) -> Result<TrashSummary> {
        let (roots, groups, protected) = {
            let shared = self.shared.lock().unwrap();
            let groups = shared
                .dedup
                .groups
                .iter()
                .filter(|group| group.state == ReviewState::Pending)
                .cloned()
                .collect::<Vec<_>>();
            (
                shared.roots.clone(),
                groups,
                shared.dedup.protected_paths.clone(),
            )
        };
        if groups.is_empty() {
            bail!("没有待处理的重复组");
        }

        let mut paths = Vec::new();
        let mut protected_skipped = 0usize;
        for group in &groups {
            let keeper = group
                .keeper
                .as_ref()
                .with_context(|| format!("duplicate group has no keeper: {}", group.key))?;
            if !roots.iter().any(|root| keeper.starts_with(root)) {
                bail!("保留项不在当前图库内: {}", keeper.display());
            }
            let copies = group.duplicate_paths();
            if copies.iter().any(|path| is_protected(path, &protected)) {
                protected_skipped += 1;
                continue;
            }
            validate_under_roots(&roots, &copies)?;
            paths.extend(copies);
        }
        if paths.is_empty() {
            bail!("待处理的组都包含受保护路径，未移入回收站");
        }

        let report = crate::trash::move_to_recycle_bin(&paths)?;
        let moved = report.moved.iter().cloned().collect::<HashSet<_>>();
        let db = Db::open(&self.db_path)?;
        db.delete_assets(&report.moved)?;

        let mut completed = 0usize;
        for group in &groups {
            let copies = group.duplicate_paths();
            if !copies.is_empty() && copies.iter().all(|path| moved.contains(path)) {
                db.save_duplicate_review(&DuplicateReview {
                    group_key: group.key.clone(),
                    state: ReviewState::Done,
                    keeper_path: group.keeper.clone(),
                    member_count: group.member_count,
                    signature: group.signature.clone(),
                })?;
                completed += 1;
            }
        }

        let mut shared = self.shared.lock().unwrap();
        shared.remove_assets(&moved);
        shared.refresh_after_removal(&moved);
        let mut status = format!(
            "批量处理完成：{} 组 · 移入回收站 {} 张",
            completed,
            report.moved.len()
        );
        if protected_skipped > 0 {
            status.push_str(&format!("；跳过 {protected_skipped} 个受保护组"));
        }
        shared.status = status;
        shared.error = report.error.clone();
        Ok(TrashSummary {
            groups: completed,
            assets: report.moved.len(),
            failed: report.failed.len(),
            protected_skipped,
        })
    }

    pub fn set_error(&self, message: String) {
        let mut shared = self.shared.lock().unwrap();
        shared.error = Some(message);
        shared.status = "重试失败".into();
    }

    pub fn search_paths(&self, text: &str) -> Result<HashSet<PathBuf>> {
        let roots = self
            .shared
            .lock()
            .unwrap()
            .roots
            .iter()
            .map(|root| root.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let tokens = text
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        Db::open(&self.db_path)?.search_paths_in(&roots, &tokens)
    }

    pub fn stop(&self) {
        if let Some(flag) = self.cancel.lock().unwrap().take() {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

#[derive(Default, Debug)]
struct ScanStats {
    files: usize,
    imported: usize,
    unchanged: usize,
    updated: usize,
    removed: usize,
    metadata_errors: usize,
    hashes_reused: usize,
    hashes_computed: usize,
    hash_errors: usize,
    thumb_errors: usize,
    untagged: usize,
    tagged: usize,
    tag_errors: usize,
    skipped_tagging: bool,
    walk_complete: bool,
    root_accessible: bool,
    cancelled: bool,
    walk_secs: f64,
    import_secs: f64,
    hash_secs: f64,
    thumb_prefetch_jobs: usize,
    thumb_prefetch_generated: usize,
    thumb_prefetch_failed: usize,
    thumb_prefetch_secs: f64,
    thumb_secs: f64,
    tag_load_secs: f64,
    tag_secs: f64,
    total_secs: f64,
}

impl ScanStats {
    fn failed(&self) -> usize {
        self.metadata_errors + self.hash_errors + self.thumb_errors + self.tag_errors
    }

    fn scan_counts(&self) -> ScanCounts {
        ScanCounts {
            discovered: self.files,
            // `imported` 同时统计未变化文件，扣除后才是真正新增/迁入数。
            imported: self.imported.saturating_sub(self.unchanged),
            updated: self.updated,
            removed: self.removed,
            failed: self.failed(),
        }
    }
}

fn log_scan_metrics(stats: &ScanStats) {
    let json = serde_json::json!({
        "files": stats.files,
        "imported": stats.imported,
        "unchanged": stats.unchanged,
        "updated": stats.updated,
        "removed": stats.removed,
        "metadata_errors": stats.metadata_errors,
        "hashes_reused": stats.hashes_reused,
        "hashes_computed": stats.hashes_computed,
        "hash_errors": stats.hash_errors,
        "thumb_errors": stats.thumb_errors,
        "untagged": stats.untagged,
        "tagged": stats.tagged,
        "tag_errors": stats.tag_errors,
        "skipped_tagging": stats.skipped_tagging,
        "walk_complete": stats.walk_complete,
        "root_accessible": stats.root_accessible,
        "cancelled": stats.cancelled,
        "walk_secs": stats.walk_secs,
        "import_secs": stats.import_secs,
        "hash_secs": stats.hash_secs,
        "thumb_prefetch_jobs": stats.thumb_prefetch_jobs,
        "thumb_prefetch_generated": stats.thumb_prefetch_generated,
        "thumb_prefetch_failed": stats.thumb_prefetch_failed,
        "thumb_prefetch_secs": stats.thumb_prefetch_secs,
        "thumb_secs": stats.thumb_secs,
        "tag_load_secs": stats.tag_load_secs,
        "tag_secs": stats.tag_secs,
        "total_secs": stats.total_secs,
    });
    log::info!("scan metrics: {json}");
    if let Some(path) = std::env::var_os("KANALIB_METRICS_PATH") {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(mut file) => {
                let _ = writeln!(file, "{json}");
            }
            Err(error) => log::warn!("metrics path {}: {error}", Path::new(&path).display()),
        }
    }
}

fn scan_roots(
    state: &Arc<AppState>,
    roots: &[PathBuf],
    cancel: &Arc<AtomicBool>,
    skip_tag: bool,
) -> Result<()> {
    for (position, root) in roots.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        scan_one_root(state, root, cancel, skip_tag, position + 1, roots.len())?;
    }
    Ok(())
}

/// 扫描单个来源，并在结束后写回该来源的运行时状态。
fn scan_one_root(
    state: &Arc<AppState>,
    root: &Path,
    cancel: &Arc<AtomicBool>,
    skip_tag: bool,
    position: usize,
    total: usize,
) -> Result<ScanStats> {
    {
        let mut shared = state.shared.lock().unwrap();
        shared.scanning = true;
        shared.status = if total > 1 {
            format!(
                "扫描来源 {position}/{total} · {}",
                root.file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| root.display().to_string())
            )
        } else {
            format!(
                "扫描来源 · {}",
                root.file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| root.display().to_string())
            )
        };
        let online = root.is_dir();
        let status = shared.source_status_mut(root);
        status.root = root.to_path_buf();
        status.online = online;
        if online {
            status.error = None;
        } else {
            status.error = Some(format!("来源不可访问：{}", root.display()));
        }
        shared.revision = shared.revision.wrapping_add(1);
    }
    let result = index(state, root, cancel, skip_tag);
    match &result {
        Ok(stats) => record_source_scan(state, root, stats, cancel),
        Err(error) => record_source_error(state, root, format!("{error:#}")),
    }
    result
}

fn record_source_scan(
    state: &Arc<AppState>,
    root: &Path,
    stats: &ScanStats,
    cancel: &AtomicBool,
) {
    let mut shared = state.shared.lock().unwrap();
    let asset_count = {
        let assets = shared.assets.read().unwrap();
        assets
            .iter()
            .filter(|asset| asset.library_root == root)
            .count()
    };
    let status = shared.source_status_mut(root);
    status.root = root.to_path_buf();
    status.asset_count = asset_count;
    status.online = stats.root_accessible && root.is_dir();
    if !stats.root_accessible {
        status.online = false;
        if status.error.is_none() {
            status.error = Some(format!("来源不可访问，已保留图库记录：{}", root.display()));
        }
    } else if !stats.walk_complete {
        status.error = Some(format!(
            "来源未能完整遍历（可能离线或权限不足），已保留图库记录：{}",
            root.display()
        ));
    } else {
        status.error = None;
    }
    if stats.root_accessible {
        status.counts = stats.scan_counts();
        if stats.walk_complete && !stats.cancelled && !cancel.load(Ordering::SeqCst) {
            status.last_scan = Some(unix_now());
        }
    }
    shared.revision = shared.revision.wrapping_add(1);
}

fn record_source_error(state: &Arc<AppState>, root: &Path, message: String) {
    let mut shared = state.shared.lock().unwrap();
    let online = root.is_dir();
    let status = shared.source_status_mut(root);
    status.root = root.to_path_buf();
    status.online = online;
    status.error = Some(message.clone());
    shared.error = Some(message);
    shared.revision = shared.revision.wrapping_add(1);
}

fn index(
    state: &Arc<AppState>,
    folder: &Path,
    cancel: &Arc<AtomicBool>,
    skip_tag: bool,
) -> Result<ScanStats> {
    let start = Instant::now();
    let mut stats = ScanStats::default();
    let result = index_inner(state, folder, cancel, skip_tag, &mut stats);
    stats.total_secs = start.elapsed().as_secs_f64();
    log_scan_metrics(&stats);
    result.map(|()| stats)
}

fn rename_candidate(
    db: &Db,
    folder: &str,
    sha1: &str,
    new_path: &Path,
    files: &[PathBuf],
) -> Result<Option<PathBuf>> {
    for candidate in db.asset_paths_by_sha1(folder, sha1)? {
        // 排除规则会让仍在磁盘上的旧文件缺席本次 files；必须再确认它确实消失，
        // 否则会把它误当成重命名来源，搬走标签。
        if candidate != new_path
            && files.binary_search(&candidate).is_err()
            && definitely_missing(&candidate)
        {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

/// 监听增量用：同 SHA1 且磁盘上已消失的旧资产视为重命名来源。
fn rename_candidate_missing(
    db: &Db,
    folder: &str,
    sha1: &str,
    new_path: &Path,
) -> Result<Option<PathBuf>> {
    for candidate in db.asset_paths_by_sha1(folder, sha1)? {
        if candidate != new_path && definitely_missing(&candidate) {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn definitely_missing(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        Ok(_) => false,
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn stored_orientation(meta: &StoredMeta) -> u8 {
    serde_json::from_str::<media::ExifData>(&meta.metadata_json)
        .map(|exif| exif.orientation)
        .unwrap_or(1)
        .clamp(1, 8)
}

fn thumb_cache_current(
    meta: &StoredMeta,
    path: &Path,
    cache_dir: &Path,
    mtime: u64,
    bytes: u64,
) -> bool {
    let expected = thumb_target(path, cache_dir, mtime, bytes, stored_orientation(meta));
    let stored = Path::new(&meta.thumb);
    stored == expected && stored.exists()
}

fn prefetch_thumbnails(
    state: &Arc<AppState>,
    db: &Db,
    files: &[PathBuf],
    cancel: &AtomicBool,
    stats: &mut ScanStats,
) -> HashMap<PathBuf, (media::ExifData, Option<u64>)> {
    let mut cached_exif = HashMap::new();
    if !ffmpeg::available() {
        return cached_exif;
    }
    let started = Instant::now();
    let mut jobs = Vec::new();
    for (index, path) in files.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        if index % 1000 == 0 {
            let mut shared = state.shared.lock().unwrap();
            shared.status = format!("检查缩略图 {}/{}", index + 1, files.len());
        }
        let Ok(metadata) = std::fs::metadata(path) else {
            continue;
        };
        let bytes = metadata.len();
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let stored = db.stored_meta(path).ok().flatten();
        if stored.as_ref().is_some_and(|meta| {
            meta.is_unchanged(mtime, bytes)
                && meta.exif_state == "done"
                && thumb_cache_current(meta, path, &state.cache_dir, mtime, bytes)
        }) {
            continue;
        }
        let (exif, captured_at) = media::read_exif(path);
        let target = thumb_target(path, &state.cache_dir, mtime, bytes, exif.orientation);
        if !target.exists() {
            jobs.push(ThumbJob {
                source: path.clone(),
                target,
                alpha: ffmpeg::alpha_source(path),
            });
        }
        cached_exif.insert(path.clone(), (exif, captured_at));
    }
    stats.thumb_prefetch_jobs = jobs.len();
    if jobs.is_empty() {
        stats.thumb_prefetch_secs = started.elapsed().as_secs_f64();
        return cached_exif;
    }
    let total = jobs.len();
    let progress = |done: usize, total: usize| {
        let mut shared = state.shared.lock().unwrap();
        shared.status = format!("预生成缩略图 {done}/{total}");
    };
    let outcome = ffmpeg::generate_thumbnails(&jobs, cancel, &progress);
    stats.thumb_prefetch_generated = outcome.generated;
    stats.thumb_prefetch_failed = outcome.failed;
    stats.thumb_prefetch_secs = started.elapsed().as_secs_f64();
    log::info!(
        "thumb prefetch: jobs={} generated={} failed={} batches={} secs={:.2}",
        total,
        outcome.generated,
        outcome.failed,
        outcome.batches,
        stats.thumb_prefetch_secs
    );
    cached_exif
}

fn append_scan_warning(shared: &mut Shared, message: String) {
    shared.error = Some(match shared.error.take() {
        Some(previous) if !previous.is_empty() => format!("{previous}\n{message}"),
        _ => message,
    });
}

fn index_inner(
    state: &Arc<AppState>,
    folder: &Path,
    cancel: &Arc<AtomicBool>,
    skip_tag: bool,
    stats: &mut ScanStats,
) -> Result<()> {
    let db = Db::open(&state.db_path)?;
    let folder_string = folder.to_string_lossy().to_string();
    // 来源离线或权限不足时直接返回：不改图库、不触发缺失清理。
    let accessible = folder.is_dir() && std::fs::read_dir(folder).is_ok();
    stats.root_accessible = accessible;
    if !accessible {
        stats.walk_complete = false;
        let mut shared = state.shared.lock().unwrap();
        let status = shared.source_status_mut(folder);
        status.root = folder.to_path_buf();
        status.online = false;
        status.error = Some(format!("来源不可访问：{}", folder.display()));
        shared.revision = shared.revision.wrapping_add(1);
        log::warn!("source inaccessible: {}", folder.display());
        return Ok(());
    }

    let walk_start = Instant::now();
    let config = state.scan_filter(folder);
    let walk = scan_filter::collect_images(folder, &config)?;
    let files = walk.files;
    let mut walk_complete = walk.walk_complete;
    stats.files = files.len();
    stats.walk_secs = walk_start.elapsed().as_secs_f64();
    stats.walk_complete = walk_complete;
    {
        let mut shared = state.shared.lock().unwrap();
        shared.status = format!("发现 {} 张图片，生成缩略图...", files.len());
    }
    let mut prefetched_exif = prefetch_thumbnails(state, &db, &files, cancel, stats);

    let rename_policy = if walk_complete {
        RenamePolicy::Scan(&files)
    } else {
        // 遍历不完整时不做重命名对账，避免把仍存在的旧记录误判为已迁移。
        RenamePolicy::Disabled
    };
    let import_start = Instant::now();
    for (index, path) in files.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            stats.cancelled = true;
            stats.import_secs = import_start.elapsed().as_secs_f64();
            let mut shared = state.shared.lock().unwrap();
            shared.status = "已停止".into();
            shared.scanning = false;
            return Ok(());
        }
        let outcome = match index_file(
            state,
            &db,
            folder,
            &folder_string,
            path,
            rename_policy,
            Some((index + 1, files.len())),
            prefetched_exif.remove(path),
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                stats.metadata_errors += 1;
                log::warn!("index {}: {error:#}", path.display());
                continue;
            }
        };
        stats.hashes_reused += outcome.hash_reused as usize;
        stats.hashes_computed += outcome.hash_computed as usize;
        stats.hash_errors += outcome.hash_failed as usize;
        stats.thumb_errors += outcome.thumb_failed as usize;
        stats.hash_secs += outcome.hash_secs;
        stats.thumb_secs += outcome.thumb_secs;
        match outcome.kind {
            IndexedFile::Unchanged => {
                stats.unchanged += 1;
                stats.imported += 1;
                if stats.unchanged % 500 == 0 {
                    let mut shared = state.shared.lock().unwrap();
                    shared.status = format!("校验 {}/{}", index + 1, files.len());
                }
            }
            IndexedFile::Imported | IndexedFile::Renamed => stats.imported += 1,
            IndexedFile::Updated => {
                stats.imported += 1;
                stats.updated += 1;
            }
        }
        if stats.imported % 1000 == 0 {
            log::info!(
                "index progress: imported={} unchanged={} elapsed={:.2}s hash={:.2}s thumb={:.2}s",
                stats.imported,
                stats.unchanged,
                import_start.elapsed().as_secs_f64(),
                stats.hash_secs,
                stats.thumb_secs
            );
        }
    }
    stats.import_secs = import_start.elapsed().as_secs_f64();

    let folder_label = folder
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| folder.display().to_string());
    // 扫描结束时再次确认来源可访问；网络盘/移动盘在遍历后掉线时不得清理。
    let root_accessible = folder.is_dir() && std::fs::read_dir(folder).is_ok();
    if !root_accessible {
        walk_complete = false;
    }
    stats.walk_complete = walk_complete;
    if !cancel.load(Ordering::SeqCst) {
        if !walk_complete {
            let mut shared = state.shared.lock().unwrap();
            let status = shared.source_status_mut(folder);
            status.online = root_accessible;
            status.error = Some(format!(
                "来源未能完整遍历（可能离线或权限不足），已保留图库记录：{}",
                folder.display()
            ));
            append_scan_warning(
                &mut shared,
                format!(
                    "来源「{folder_label}」未能完整遍历（可能离线或权限不足），已保留原图库记录，未自动移除任何图片。"
                ),
            );
            shared.revision = shared.revision.wrapping_add(1);
            log::warn!("incomplete walk for {}", folder.display());
        } else {
            let db_paths = db.asset_paths(&folder_string)?;
            // 只清理“确实不在磁盘上”的记录；被排除规则过滤但仍存在的文件保留记录。
            let missing = db_paths
                .iter()
                .filter(|path| files.binary_search(path).is_err() && definitely_missing(path))
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                let total = db_paths.len();
                let missing_count = missing.len();
                let full_purge = total > 0 && missing_count == total;
                let large_purge = missing_count >= 10 && missing_count * 2 >= total;
                if full_purge || large_purge {
                    let missing_set = missing.into_iter().collect::<HashSet<_>>();
                    let mut shared = state.shared.lock().unwrap();
                    let pending = shared.pending_cleanup.get_or_insert_with(|| PendingCleanup {
                        roots: Vec::new(),
                        paths: HashSet::new(),
                    });
                    if !pending.roots.iter().any(|root| root == folder) {
                        pending.roots.push(folder.to_path_buf());
                    }
                    pending.paths.extend(missing_set);
                    append_scan_warning(
                        &mut shared,
                        format!(
                            "来源「{folder_label}」有 {missing_count} 张图片不在磁盘上。为避免来源离线或目录不完整导致误删，已保留记录，请确认是否从图库移除。"
                        ),
                    );
                    shared.revision = shared.revision.wrapping_add(1);
                    log::warn!(
                        "cleanup deferred for {}: {} missing of {} assets",
                        folder.display(),
                        missing_count,
                        total
                    );
                } else {
                    let missing_set = missing.iter().cloned().collect::<HashSet<_>>();
                    let removed = db.delete_assets(&missing)?;
                    stats.removed += removed;
                    let mut shared = state.shared.lock().unwrap();
                    shared.remove_assets(&missing_set);
                    shared.status = format!("已移除 {} 张磁盘上已不存在的图片", removed);
                    shared.revision = shared.revision.wrapping_add(1);
                    log::info!(
                        "cleanup: removed {} missing assets under {}",
                        removed,
                        folder.display()
                    );
                }
            }
        }
    }

    {
        let mut shared = state.shared.lock().unwrap();
        shared.sort_assets();
    }
    backfill_tile_thumbs(state, cancel);
    if let Err(error) = db.recover_tag_jobs() {
        log::warn!("recover tag jobs: {error:#}");
    }
    db.enqueue_tag_jobs(&folder_string)?;
    let queue = db.tag_job_counts(&folder_string)?;
    stats.untagged = queue.pending;
    {
        let mut shared = state.shared.lock().unwrap();
        shared.set_queue_counts(queue.pending, queue.running, queue.error);
        shared.status = if queue.pending > 0 {
            format!("待打标 {} 张 · 队列已恢复", queue.pending)
        } else if queue.error > 0 {
            format!(
                "完成 · {} 张图片 · {} 个失败待处理",
                files.len(),
                queue.error
            )
        } else {
            format!("完成 · {} 张图片，全部已打标", files.len())
        };
    }
    if queue.pending == 0 {
        let mut shared = state.shared.lock().unwrap();
        shared.scanning = false;
        return Ok(());
    }
    if skip_tag {
        stats.skipped_tagging = true;
        let mut shared = state.shared.lock().unwrap();
        shared.set_queue_counts(queue.pending, 0, queue.error);
        shared.status = format!(
            "完成 · {} 张图片（跳过打标，队列 {} 张待处理）",
            files.len(),
            queue.pending
        );
        shared.scanning = false;
        return Ok(());
    }

    {
        let mut shared = state.shared.lock().unwrap();
        shared.status = format!("加载 PixAI 模型...（队列 {} 张）", queue.pending);
    }
    let load_start = Instant::now();
    let (mut sidecar, ready) = Sidecar::spawn()?;
    stats.tag_load_secs = load_start.elapsed().as_secs_f64();
    {
        let mut shared = state.shared.lock().unwrap();
        shared.sidecar_device = Some(if ready.half {
            format!("{} · FP16", ready.device)
        } else {
            ready.device.clone()
        });
        shared.status = format!("打标中 0/{} · {} 类标签", queue.pending, ready.classes);
    }
    process_tag_queue(
        state,
        &db,
        &folder_string,
        &mut sidecar,
        cancel,
        stats,
        queue.pending,
    )?;
    Ok(())
}

/// 单文件索引结果；全量扫描与监听增量共用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum IndexedFile {
    #[default]
    Unchanged,
    Imported,
    Updated,
    Renamed,
}

#[derive(Clone, Copy, Debug)]
enum RenamePolicy<'a> {
    /// 遍历不完整或调用方不允许重命名对账。
    Disabled,
    /// 全量扫描：候选旧路径不在本次遍历结果中。
    Scan(&'a [PathBuf]),
    /// 监听增量：候选旧路径在磁盘上已消失。
    MissingOnDisk,
}

#[derive(Debug, Default)]
pub(crate) struct FileIndexOutcome {
    pub(crate) kind: IndexedFile,
    pub(crate) renamed_from: Option<PathBuf>,
    pub(crate) hash_failed: bool,
    pub(crate) thumb_failed: bool,
    hash_reused: bool,
    hash_computed: bool,
    hash_secs: f64,
    thumb_secs: f64,
}

/// 从 `index_inner` 抽取的单文件索引：hash/EXIF/缩略图/upsert，可选重命名恢复。
fn index_file(
    state: &Arc<AppState>,
    db: &Db,
    _folder: &Path,
    folder_string: &str,
    path: &Path,
    rename: RenamePolicy<'_>,
    progress: Option<(usize, usize)>,
    prefetched_exif: Option<(media::ExifData, Option<u64>)>,
) -> Result<FileIndexOutcome> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let metadata =
        std::fs::metadata(path).with_context(|| format!("读取文件信息 {}", path.display()))?;
    let bytes = metadata.len();
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let stored = db.stored_meta(path)?;
    let existed_before = stored.is_some();
    let stored_media = stored
        .as_ref()
        .filter(|meta| meta.mtime == mtime && meta.bytes == bytes)
        .and_then(|meta| {
            let thumb = (!meta.thumb.is_empty())
                .then(|| PathBuf::from(&meta.thumb))
                .filter(|thumb| thumb.exists())?;
            Some((meta.width, meta.height, thumb))
        });
    let reset_tag_attempts = stored
        .as_ref()
        .is_none_or(|meta| meta.mtime != mtime || meta.bytes != bytes);
    let exif_done = stored
        .as_ref()
        .is_some_and(|meta| meta.exif_state == "done");
    let cache_current = stored.as_ref().is_some_and(|meta| {
        thumb_cache_current(meta, path, &state.cache_dir, mtime, bytes)
    });
    if stored
        .as_ref()
        .is_some_and(|meta| meta.is_unchanged(mtime, bytes))
        && exif_done
        && cache_current
    {
        return Ok(FileIndexOutcome::default());
    }

    let mut outcome = FileIndexOutcome::default();
    let hashes = stored
        .as_ref()
        .and_then(|meta| meta.content_if_current(mtime, bytes));
    let hashes = match hashes {
        Some(hashes) => {
            outcome.hash_reused = true;
            hashes
        }
        None => {
            {
                let mut shared = state.shared.lock().unwrap();
                shared.status = match progress {
                    Some((done, total)) => format!("哈希 {done}/{total} · {name}"),
                    None => format!("监听 · 索引 {name}"),
                };
            }
            let started = Instant::now();
            let hashes = match media::hash_file(path) {
                Ok(hashes) => {
                    outcome.hash_computed = true;
                    hashes
                }
                Err(error) => {
                    outcome.hash_failed = true;
                    log::warn!("hash {}: {error:#}", path.display());
                    crate::media::ContentHashes {
                        decode_error: Some("无法读取图片内容".into()),
                        ..Default::default()
                    }
                }
            };
            outcome.hash_secs = started.elapsed().as_secs_f64();
            hashes
        }
    };
    let (exif, captured_at) = prefetched_exif.unwrap_or_else(|| media::read_exif(path));
    let metadata_json = serde_json::to_string(&exif).unwrap_or_else(|_| "{}".to_string());
    let thumb_started = Instant::now();
    let thumbnail = ensure_thumb(path, &state.cache_dir, mtime, bytes, exif.orientation);
    outcome.thumb_secs = thumb_started.elapsed().as_secs_f64();
    let (thumb, width, height) = match thumbnail {
        Ok(value) => value,
        Err(error) => {
            outcome.thumb_failed = true;
            log::warn!("thumbnail {}: {error:#}", path.display());
            match stored_media {
                // 容错修复过的缩略图对 Rust 解码器仍不可读，同文件沿用已有缓存与尺寸。
                Some((width, height, thumb)) => (Some(thumb), width, height),
                None => (None, 0, 0),
            }
        }
    };
    if !existed_before && !hashes.sha1.is_empty() {
        let candidate = match rename {
            RenamePolicy::Disabled => None,
            RenamePolicy::Scan(files) => {
                match rename_candidate(db, &folder_string, &hashes.sha1, path, files) {
                    Ok(candidate) => candidate,
                    Err(error) => {
                        log::warn!("rename lookup {}: {error:#}", path.display());
                        None
                    }
                }
            }
            RenamePolicy::MissingOnDisk => {
                match rename_candidate_missing(db, &folder_string, &hashes.sha1, path) {
                    Ok(candidate) => candidate,
                    Err(error) => {
                        log::warn!("rename lookup {}: {error:#}", path.display());
                        None
                    }
                }
            }
        };
        if let Some(old_path) = candidate {
            match db.move_asset_media(
                &old_path,
                path,
                &folder_string,
                thumb.as_deref(),
                width,
                height,
                bytes,
                mtime,
                &hashes,
            ) {
                Ok(mut asset) => {
                    db.set_exif(path, captured_at, &metadata_json)?;
                    if let Some(thumb) = thumb.as_deref() {
                        db.record_thumb_source(thumb, path, mtime, bytes, exif.orientation)?;
                    }
                    asset.captured_at = captured_at;
                    asset.metadata_json = metadata_json;
                    {
                        let mut shared = state.shared.lock().unwrap();
                        shared.move_asset(&old_path, asset);
                        shared.status = match progress {
                            Some((done, total)) => format!("已识别重命名 {done}/{total} · {name}"),
                            None => format!("监听 · 已识别重命名 {name}"),
                        };
                    }
                    log::info!(
                        "renamed asset: {} -> {}",
                        old_path.display(),
                        path.display()
                    );
                    outcome.kind = IndexedFile::Renamed;
                    outcome.renamed_from = Some(old_path);
                    return Ok(outcome);
                }
                Err(error) => {
                    log::warn!(
                        "rename adopt {} -> {} failed: {error:#}; falling back to upsert",
                        old_path.display(),
                        path.display()
                    );
                }
            }
        }
    }
    let mut asset = db.upsert_meta(
        &folder_string,
        path,
        thumb.as_deref(),
        width,
        height,
        bytes,
        mtime,
        &hashes,
        reset_tag_attempts,
    )?;
    db.set_exif(path, captured_at, &metadata_json)?;
    if let Some(thumb) = thumb.as_deref() {
        db.record_thumb_source(thumb, path, mtime, bytes, exif.orientation)?;
    }
    asset.captured_at = captured_at;
    asset.metadata_json = metadata_json;
    {
        let mut shared = state.shared.lock().unwrap();
        shared.upsert(asset);
        shared.status = match progress {
            Some((done, total)) => format!(
                "缩略图 {done}/{total} · 已打标 {}/{}",
                shared.counts.tagged_count(),
                shared.counts.total()
            ),
            None => format!("监听 · 已索引 {name}"),
        };
    }
    outcome.kind = if existed_before {
        IndexedFile::Updated
    } else {
        IndexedFile::Imported
    };
    Ok(outcome)
}

fn process_tag_queue(
    state: &Arc<AppState>,
    db: &Db,
    folder: &str,
    sidecar: &mut Sidecar,
    cancel: &Arc<AtomicBool>,
    stats: &mut ScanStats,
    total: usize,
) -> Result<()> {
    let tag_start = Instant::now();
    let mut done = 0usize;
    loop {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let jobs = db.claim_tag_jobs(folder, 4)?;
        if jobs.is_empty() {
            break;
        }
        let job_ids = jobs.iter().map(|job| job.id).collect::<Vec<_>>();
        {
            let counts = db.tag_job_counts(folder)?;
            let mut shared = state.shared.lock().unwrap();
            shared.set_queue_counts(counts.pending, counts.running, counts.error);
            shared.status = format!(
                "打标中 {done}/{total} · 队列 {} · 失败 {}",
                counts.pending, counts.error
            );
        }
        let paths = jobs.iter().map(|job| job.path.clone()).collect::<Vec<_>>();
        let items = match sidecar.tag(&paths) {
            Ok(items) => items,
            Err(error) => {
                db.release_tag_jobs(&job_ids)?;
                let counts = db.tag_job_counts(folder)?;
                let mut shared = state.shared.lock().unwrap();
                shared.set_queue_counts(counts.pending, counts.running, counts.error);
                return Err(error).context("sidecar tag batch");
            }
        };
        let by_path = jobs
            .iter()
            .map(|job| (job.path.clone(), job.id))
            .collect::<HashMap<_, _>>();
        let mut handled = HashSet::new();
        for item in items {
            let path = PathBuf::from(&item.path);
            let job_id = by_path.get(&path).copied();
            if let Some(job_id) = job_id {
                handled.insert(job_id);
            }
            if item.ok {
                if let Some(detail) = &item.error_detail {
                    log::warn!("sidecar detail {}: {detail}", item.path);
                }
                let asset = {
                    let shared = state.shared.lock().unwrap();
                    let existing = shared
                        .assets
                        .read()
                        .unwrap()
                        .iter()
                        .find(|asset| asset.path == path)
                        .cloned();
                    existing.map(|mut asset| {
                        if asset.width == 0 {
                            asset.width = item.width.unwrap_or(0);
                            asset.height = item.height.unwrap_or(0);
                        }
                        asset.rating = item.rating.clone().unwrap_or_default();
                        asset.group = item.group.clone().unwrap_or_else(|| "未分类".to_string());
                        asset.tags = item.tags.clone().unwrap_or_default();
                        asset.tagged = true;
                        asset.tag_attempts = 0;
                        asset.error = None;
                        asset.damage = item.damage.clone();
                        asset
                    })
                };
                let Some(asset) = asset else {
                    if let Some(job_id) = job_id {
                        db.fail_tag_job(job_id, "asset not found")?;
                    }
                    stats.tag_errors += 1;
                    done += 1;
                    continue;
                };
                db.update_tags(&asset)?;
                if let Some(job_id) = job_id {
                    db.complete_tag_job(job_id)?;
                }
                state.shared.lock().unwrap().upsert(asset);
                stats.tagged += 1;
            } else {
                let error = item
                    .error
                    .clone()
                    .unwrap_or_else(|| "sidecar image error".to_string());
                if let Some(detail) = &item.error_detail {
                    log::warn!("sidecar detail {}: {detail}", item.path);
                }
                let attempts = db.mark_error(&path, &error, item.damage.as_deref())?;
                if let Some(job_id) = job_id {
                    db.fail_tag_job(job_id, &error)?;
                }
                stats.tag_errors += 1;
                let updated = {
                    let shared = state.shared.lock().unwrap();
                    let mut asset = shared
                        .assets
                        .read()
                        .unwrap()
                        .iter()
                        .find(|asset| asset.path == path)
                        .cloned();
                    if let Some(asset) = asset.as_mut() {
                        asset.error = Some(error);
                        asset.damage = item.damage.clone();
                        asset.tag_attempts = attempts;
                    }
                    asset
                };
                if let Some(asset) = updated {
                    state.shared.lock().unwrap().upsert(asset);
                }
            }
            done += 1;
        }
        let unhandled = jobs
            .iter()
            .filter(|job| !handled.contains(&job.id))
            .map(|job| job.id)
            .collect::<Vec<_>>();
        if !unhandled.is_empty() {
            log::warn!(
                "{} tag jobs returned no result; requeueing",
                unhandled.len()
            );
            db.release_tag_jobs(&unhandled)?;
        }
        let counts = db.tag_job_counts(folder)?;
        let mut shared = state.shared.lock().unwrap();
        shared.set_queue_counts(counts.pending, counts.running, counts.error);
        shared.status = format!(
            "打标中 {done}/{total} · 队列 {} · 失败 {}",
            counts.pending, counts.error
        );
    }
    stats.tag_secs = tag_start.elapsed().as_secs_f64();

    let counts = db.tag_job_counts(folder)?;
    let mut shared = state.shared.lock().unwrap();
    shared.set_queue_counts(counts.pending, counts.running, counts.error);
    shared.sort_assets();
    shared.status = if cancel.load(Ordering::SeqCst) {
        format!(
            "已停止 · 已打标 {done}/{total} · 队列 {} 张待处理",
            counts.pending
        )
    } else if counts.pending > 0 {
        format!(
            "打标暂停 · 队列 {} 张待处理 · {} 个失败",
            counts.pending, counts.error
        )
    } else if counts.error > 0 {
        format!(
            "完成 · {} 张图片 · {} 个失败待处理",
            shared.counts.total(),
            counts.error
        )
    } else {
        format!("完成 · {} 张图片已打标", shared.counts.total())
    };
    shared.scanning = false;
    Ok(())
}

#[derive(Deserialize)]
struct ModelConfig {
    tags: Vec<String>,
    #[serde(default)]
    tags_split: Vec<(String, usize)>,
}

/// 把模型词表灌进 tags 表，补全与类别筛选才能在未打标的标签上工作。
fn import_vocabulary(db: &Db) -> Result<()> {
    let config_path = crate::sidecar::model_dir().join("config.json");
    let bytes =
        std::fs::read(&config_path).with_context(|| format!("read {}", config_path.display()))?;
    let config: ModelConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {}", config_path.display()))?;
    let marker = format!(
        "{}:{}",
        config.tags.len(),
        config
            .tags_split
            .iter()
            .map(|(category, count)| format!("{category}-{count}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    if db.setting("tags.vocab_marker").ok().flatten().as_deref() == Some(marker.as_str()) {
        return Ok(());
    }
    let mut categories = vec![String::new(); config.tags.len()];
    let mut start = 0;
    for (category, count) in &config.tags_split {
        let end = (start + count).min(categories.len());
        for slot in &mut categories[start..end] {
            *slot = category.clone();
        }
        start = end;
    }
    let entries = config
        .tags
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), categories[index].clone(), index as u32))
        .collect::<Vec<_>>();
    let inserted = db.import_vocabulary(&entries)?;
    db.set_setting("tags.vocab_marker", &marker)?;
    log::info!(
        "imported vocabulary: {} tags, {} new",
        entries.len(),
        inserted
    );
    Ok(())
}

fn backfill_vocab(db: &Db) -> Result<()> {
    let missing = db.tags_missing_vocab()?;
    if missing.is_empty() {
        return Ok(());
    }
    let config_path = crate::sidecar::model_dir().join("config.json");
    let bytes =
        std::fs::read(&config_path).with_context(|| format!("read {}", config_path.display()))?;
    let config: ModelConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {}", config_path.display()))?;
    let index = config
        .tags
        .iter()
        .enumerate()
        .map(|(index, tag)| (tag.as_str(), index as u32))
        .collect::<HashMap<_, _>>();
    let mappings = missing
        .iter()
        .filter_map(|name| index.get(name.as_str()).map(|index| (name.clone(), *index)))
        .collect::<Vec<_>>();
    if mappings.is_empty() {
        return Ok(());
    }
    db.set_vocab_indices(&mappings)?;
    log::info!("backfilled vocab_index for {} tags", mappings.len());
    Ok(())
}

fn absolute(path: PathBuf) -> PathBuf {
    std::path::absolute(&path).unwrap_or(path)
}

fn validate_under_roots(roots: &[PathBuf], paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        if !path.is_absolute() {
            bail!("拒绝处理相对路径: {}", path.display());
        }
        if !roots.iter().any(|root| path.starts_with(root)) {
            bail!("拒绝处理当前图库之外的文件: {}", path.display());
        }
    }
    Ok(())
}

/// 启动时把已存在的小档直接挂到资产上，省掉一次生成与等待。
fn load_tile_thumbs(mut assets: Vec<Asset>) -> Vec<Asset> {
    for asset in &mut assets {
        if let Some(thumb) = &asset.thumb {
            let tile = tile_thumb_path(thumb);
            if tile.exists() {
                asset.tile_thumb = Some(tile);
            }
        }
    }
    assets
}

/// 网格用小档缩略图路径：与 512 档同目录同前缀，只加 `-t256`。
fn tile_thumb_path(thumb: &Path) -> PathBuf {
    let stem = thumb
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    thumb.with_file_name(format!("{stem}-t256.jpg"))
}

/// 从已有的 512 档再缩到 256；失败时返回 None，网格回退 512 档。
/// 缓存预算淘汰删掉缩略图后，下一次滚动到该图时按需重建。
pub fn rebuild_missing_thumb(db_path: &Path, cache_dir: &Path, thumb_path: &Path) -> bool {
    if thumb_path.exists() {
        return true;
    }
    let Some(name) = thumb_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
    else {
        return false;
    };
    if let Some(stem) = name.strip_suffix("-t256.jpg") {
        let base = thumb_path.with_file_name(format!("{stem}.jpg"));
        if base.exists() {
            return ensure_tile_thumb(&base).is_some();
        }
        let Ok(db) = Db::open(db_path) else {
            return false;
        };
        let Ok(Some(source)) = db.thumb_source(&base) else {
            return false;
        };
        let Ok((Some(generated), _, _)) = ensure_thumb(
            &source.path,
            cache_dir,
            source.mtime,
            source.bytes,
            source.orientation,
        ) else {
            return false;
        };
        if generated != base {
            let _ = std::fs::copy(&generated, &base);
        }
        return ensure_tile_thumb(&base).is_some();
    }
    let Ok(db) = Db::open(db_path) else {
        return false;
    };
    let Ok(Some(source)) = db.thumb_source(thumb_path) else {
        return false;
    };
    let Ok((Some(generated), _, _)) = ensure_thumb(
        &source.path,
        cache_dir,
        source.mtime,
        source.bytes,
        source.orientation,
    ) else {
        return false;
    };
    if generated != thumb_path {
        let _ = std::fs::copy(&generated, thumb_path);
    }
    let _ = ensure_tile_thumb(thumb_path);
    true
}

fn ensure_tile_thumb(thumb: &Path) -> Option<PathBuf> {
    let target = tile_thumb_path(thumb);
    if target.exists() {
        return Some(target);
    }
    let image = image::ImageReader::open(thumb)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    image.thumbnail(256, 256).save(&target).ok()?;
    Some(target)
}

/// 补齐网格小档。图片 IO 全部在锁外，每批写回一次并 bump revision，
/// 这样首屏能在几百毫秒内切到小档，其余边滚边补。
fn backfill_tile_thumbs(state: &Arc<AppState>, cancel: &AtomicBool) {
    let started = Instant::now();
    let pending = {
        let shared = state.shared.lock().unwrap();
        let assets = shared.assets.read().unwrap();
        assets
            .iter()
            .enumerate()
            .filter_map(|(index, asset)| match (&asset.thumb, &asset.tile_thumb) {
                (Some(thumb), None) => Some((index, thumb.clone())),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    if pending.is_empty() {
        return;
    }
    let mut filled = 0usize;
    let mut failed = 0usize;
    for batch in pending.chunks(256) {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let done = batch
            .iter()
            .filter_map(|(index, thumb)| ensure_tile_thumb(thumb).map(|tile| (*index, tile)))
            .collect::<Vec<_>>();
        failed += batch.len() - done.len();
        filled += done.len();
        let mut shared = state.shared.lock().unwrap();
        {
            let mut assets = shared.assets.write().unwrap();
            for (index, tile) in done {
                if let Some(asset) = assets.get_mut(index) {
                    asset.tile_thumb = Some(tile);
                }
            }
        }
        shared.revision = shared.revision.wrapping_add(1);
    }
    log::info!(
        "tile tier: pending={} filled={} failed={} secs={:.2}",
        pending.len(),
        filled,
        failed,
        started.elapsed().as_secs_f64()
    );
}

fn thumb_target(
    path: &Path,
    cache_dir: &Path,
    mtime: u64,
    bytes: u64,
    orientation: u8,
) -> PathBuf {
    let mut key = format!(
        "v3-{}-{}-{}-{}",
        path.to_string_lossy(),
        mtime,
        bytes,
        path.extension()
            .map(|ext| ext.to_string_lossy())
            .unwrap_or_default()
    );
    // 方向 1 沿用旧键，升级后无需为绝大多数照片重做缩略图。
    if orientation != 1 {
        key.push_str(&format!("-o{orientation}"));
    }
    let hash = blake3::hash(key.as_bytes()).to_hex();
    cache_dir.join(format!("{}.jpg", &hash.as_str()[..24]))
}

fn ensure_thumb(
    path: &Path,
    cache_dir: &Path,
    mtime: u64,
    bytes: u64,
    orientation: u8,
) -> Result<(Option<PathBuf>, u32, u32)> {
    std::fs::create_dir_all(cache_dir)?;
    let target = thumb_target(path, cache_dir, mtime, bytes, orientation);

    if target.exists() {
        let cached = image::ImageReader::open(&target)
            .ok()
            .and_then(|reader| reader.with_guessed_format().ok())
            .and_then(|reader| reader.into_dimensions().ok())
            .filter(|(width, height)| *width > 0 && *height > 0);
        if let Some((thumb_width, thumb_height)) = cached {
            match image::ImageReader::open(path)
                .ok()
                .and_then(|reader| reader.with_guessed_format().ok())
                .and_then(|reader| reader.into_dimensions().ok())
            {
                Some((mut width, mut height)) => {
                    if (5..=8).contains(&orientation) {
                        std::mem::swap(&mut width, &mut height);
                    }
                    return Ok((Some(target), width, height));
                }
                // FFmpeg can decode formats the image crate only writes; the
                // generated cache is already rotated and sized.
                None => return Ok((Some(target), thumb_width, thumb_height)),
            }
        }
        let _ = std::fs::remove_file(&target);
    }

    let mut image = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()
        .with_context(|| format!("decode {}", path.display()))?;
    if let Some(orientation) = image::metadata::Orientation::from_exif(orientation) {
        image.apply_orientation(orientation);
    }
    let (width, height) = (image.width(), image.height());
    let rgba = image.thumbnail(512, 512).to_rgba8();
    let (thumb_width, thumb_height) = rgba.dimensions();
    let mut rgb = RgbImage::new(thumb_width, thumb_height);
    for (source, target_pixel) in rgba.pixels().zip(rgb.pixels_mut()) {
        let alpha = source[3] as u32;
        let inverse = 255 - alpha;
        *target_pixel = Rgb([
            ((source[0] as u32 * alpha + 255 * inverse) / 255) as u8,
            ((source[1] as u32 * alpha + 255 * inverse) / 255) as u8,
            ((source[2] as u32 * alpha + 255 * inverse) / 255) as u8,
        ]);
    }
    image::DynamicImage::ImageRgb8(rgb).save_with_format(&target, ImageFormat::Jpeg)?;
    Ok((Some(target), width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kanalib-state-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn evicted_thumbs_rebuild_from_source_mapping() {
        let dir = temp_dir("thumb-rebuild");
        let cache = dir.join("thumbs");
        let source = dir.join("a.png");
        image::RgbImage::from_pixel(64, 32, image::Rgb([20, 40, 80]))
            .save(&source)
            .unwrap();
        let db_path = dir.join("kanalib.db");
        let db = Db::open(&db_path).unwrap();
        let (thumb, width, height) = ensure_thumb(&source, &cache, 1, 123, 1).unwrap();
        assert_eq!((width, height), (64, 32));
        let thumb = thumb.unwrap();
        db.record_thumb_source(&thumb, &source, 1, 123, 1)
            .unwrap();
        let tile = ensure_tile_thumb(&thumb).unwrap();

        std::fs::remove_file(&tile).unwrap();
        assert!(rebuild_missing_thumb(&db_path, &cache, &tile));
        assert!(tile.exists());

        std::fs::remove_file(&thumb).unwrap();
        assert!(rebuild_missing_thumb(&db_path, &cache, &thumb));
        assert!(thumb.exists());

        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn exif_orientation_jpeg(path: &Path, orientation: u8) {
        image::RgbImage::from_pixel(20, 10, image::Rgb([200, 100, 50]))
            .save(path)
            .unwrap();
        let jpeg = std::fs::read(path).unwrap();
        let mut tiff = Vec::new();
        tiff.extend(b"II*\0");
        tiff.extend(8u32.to_le_bytes());
        tiff.extend(1u16.to_le_bytes());
        tiff.extend(0x0112u16.to_le_bytes());
        tiff.extend(3u16.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        tiff.extend((orientation as u16).to_le_bytes());
        tiff.extend([0u8, 0u8]);
        tiff.extend(0u32.to_le_bytes());
        let payload = [b"Exif\0\0".as_slice(), tiff.as_slice()].concat();
        let mut app1 = vec![0xFF, 0xE1];
        app1.extend(((payload.len() + 2) as u16).to_be_bytes());
        app1.extend(payload);
        let mut output = Vec::with_capacity(jpeg.len() + app1.len());
        output.extend(&jpeg[..2]);
        output.extend(app1);
        output.extend(&jpeg[2..]);
        std::fs::write(path, output).unwrap();
    }

    #[test]
    fn exif_orientation_rotates_thumbnail_and_swaps_dimensions() {
        let dir = temp_dir("exif-orientation");
        let cache = dir.join("thumbs");
        let source = dir.join("rotated.jpg");
        exif_orientation_jpeg(&source, 6);
        let bytes = std::fs::metadata(&source).unwrap().len();
        let (exif, captured_at) = media::read_exif(&source);
        assert_eq!(exif.orientation, 6);
        assert!(captured_at.is_none());
        let (_thumb, width, height) = ensure_thumb(&source, &cache, 1, bytes, exif.orientation)
            .unwrap();
        assert_eq!((width, height), (10, 20));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn write_test_image(path: &Path, color: [u8; 3]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        image::RgbImage::from_pixel(4, 4, image::Rgb(color))
            .save(path)
            .unwrap();
    }

    fn test_state(tag: &str) -> (PathBuf, PathBuf, Arc<AppState>) {
        let base = temp_dir(tag);
        let lib = base.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let state = AppState::open_for_test(base.join("data"), Some(lib.clone()));
        (base, lib, state)
    }

    #[test]
    fn rescan_root_tracks_status_and_removes_confirmed_missing() {
        let (base, lib, state) = test_state("rescan-status");
        for index in 0..3u8 {
            write_test_image(&lib.join(format!("{index}.png")), [index, 0, 0]);
        }
        state.rescan_root_blocking(lib.clone(), true).unwrap();

        let statuses = state.source_statuses();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].root, lib);
        assert_eq!(statuses[0].asset_count, 3);
        assert_eq!(statuses[0].counts.discovered, 3);
        assert_eq!(statuses[0].counts.imported, 3);
        assert_eq!(statuses[0].counts.updated, 0);
        assert!(statuses[0].online);
        assert!(statuses[0].last_scan.is_some());
        assert!(statuses[0].error.is_none());
        assert_eq!(state.shared.lock().unwrap().counts.total(), 3);

        std::fs::remove_file(lib.join("1.png")).unwrap();
        state.rescan_root_blocking(lib.clone(), true).unwrap();

        let statuses = state.source_statuses();
        assert_eq!(statuses[0].asset_count, 2);
        assert_eq!(statuses[0].counts.discovered, 2);
        assert_eq!(statuses[0].counts.removed, 1);
        assert_eq!(state.shared.lock().unwrap().counts.total(), 2);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn rescan_root_preserves_tags_across_rename() {
        let (base, lib, state) = test_state("rescan-rename");
        let old = lib.join("old.png");
        write_test_image(&old, [10, 20, 30]);
        state.rescan_root_blocking(lib.clone(), true).unwrap();
        state.add_tags(std::slice::from_ref(&old), "keepme").unwrap();

        let new = lib.join("new.png");
        std::fs::rename(&old, &new).unwrap();
        state.rescan_root_blocking(lib.clone(), true).unwrap();

        let shared = state.shared.lock().unwrap();
        let assets = shared.assets.read().unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].path, new);
        assert!(
            assets[0]
                .all_tags()
                .any(|tag| tag.tag == "keepme" && tag.category == "manual")
        );
        drop(assets);
        drop(shared);
        assert_eq!(state.source_statuses()[0].asset_count, 1);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn offline_source_rescan_keeps_records_and_marks_error() {
        let (base, lib, state) = test_state("offline");
        write_test_image(&lib.join("a.png"), [1, 2, 3]);
        state.rescan_root_blocking(lib.clone(), true).unwrap();

        std::fs::rename(&lib, base.join("offline")).unwrap();
        let error = state.rescan_root(lib.clone()).unwrap_err();
        assert!(format!("{error:#}").contains("不可访问"));

        let statuses = state.source_statuses();
        assert_eq!(statuses.len(), 1);
        assert!(!statuses[0].online);
        assert!(statuses[0].error.is_some());
        assert_eq!(statuses[0].asset_count, 1);
        assert_eq!(state.shared.lock().unwrap().counts.total(), 1);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn scan_filter_excludes_previews_persists_and_rescans() {
        let (base, lib, state) = test_state("scan-filter");
        write_test_image(&lib.join("keep.png"), [1, 2, 3]);
        write_test_image(&lib.join("cache").join("skip.png"), [4, 5, 6]);

        let config = ScanFilterConfig {
            exclude_globs: vec!["cache/**".into()],
            ..ScanFilterConfig::default()
        };
        let preview = state.preview_scan_filter(&lib, &config).unwrap();
        assert_eq!((preview.matched, preview.excluded), (1, 1));
        assert_eq!(preview.sample_excluded.len(), 1);

        state.set_scan_filter(&lib, config.clone()).unwrap();
        assert_eq!(state.scan_filter(&lib), config);
        let saved = state.setting("scan.filters").unwrap();
        assert!(saved.contains("cache/**"));

        state.scan_blocking(true).unwrap();
        assert_eq!(state.source_statuses()[0].asset_count, 1);
        assert_eq!(state.shared.lock().unwrap().counts.total(), 1);

        state
            .set_scan_filter(&lib, ScanFilterConfig::default())
            .unwrap();
        assert_eq!(state.scan_filter(&lib), ScanFilterConfig::default());
        state.scan_blocking(true).unwrap();
        assert_eq!(state.source_statuses()[0].asset_count, 2);
        assert_eq!(state.shared.lock().unwrap().counts.total(), 2);

        let invalid = ScanFilterConfig {
            exclude_globs: vec!["[".into()],
            ..ScanFilterConfig::default()
        };
        assert!(state.set_scan_filter(&lib, invalid).is_err());
        assert_eq!(state.scan_filter(&lib), ScanFilterConfig::default());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn watch_batch_adds_removes_and_restores_rename_tags() {
        let (base, lib, state) = test_state("watch-batch");
        let old = lib.join("a.png");
        write_test_image(&old, [1, 1, 1]);
        state.rescan_root_blocking(lib.clone(), true).unwrap();
        state.add_tags(std::slice::from_ref(&old), "keepme").unwrap();

        // 重命名：只把新路径交给监听批处理，靠 SHA1 + 磁盘缺失恢复标签。
        let renamed = lib.join("b.png");
        std::fs::rename(&old, &renamed).unwrap();
        crate::watch::handle_watch_batch(&state, &lib, vec![renamed.clone()]);
        {
            let shared = state.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            assert_eq!(assets.len(), 1);
            assert_eq!(assets[0].path, renamed);
            assert!(
                assets[0]
                    .all_tags()
                    .any(|tag| tag.tag == "keepme" && tag.category == "manual")
            );
        }

        // 新增
        let added = lib.join("c.png");
        write_test_image(&added, [9, 9, 9]);
        crate::watch::handle_watch_batch(&state, &lib, vec![added.clone()]);
        assert!(
            state
                .shared
                .lock()
                .unwrap()
                .assets
                .read()
                .unwrap()
                .iter()
                .any(|asset| asset.path == added)
        );

        // 删除：确认缺失后只移除 DB/内存记录。
        std::fs::remove_file(&added).unwrap();
        crate::watch::handle_watch_batch(&state, &lib, vec![added.clone()]);
        assert!(
            !state
                .shared
                .lock()
                .unwrap()
                .assets
                .read()
                .unwrap()
                .iter()
                .any(|asset| asset.path == added)
        );
        assert_eq!(state.source_statuses()[0].asset_count, 1);
        assert!(base.join("lib").join("b.png").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn excluded_existing_file_is_not_treated_as_rename_source() {
        let (base, lib, state) = test_state("filter-rename");
        let old = lib.join("old.png");
        write_test_image(&old, [7, 8, 9]);
        state.rescan_root_blocking(lib.clone(), true).unwrap();
        state.add_tags(std::slice::from_ref(&old), "keepme").unwrap();

        let filter = ScanFilterConfig {
            exclude_globs: vec!["old.png".into()],
            ..ScanFilterConfig::default()
        };
        state.set_scan_filter(&lib, filter).unwrap();

        // 与 old.png 内容相同的 new.png：old.png 只是被排除，仍在磁盘上，
        // 不能被当成“已迁移”的旧路径而搬走标签。
        let new = lib.join("new.png");
        std::fs::copy(&old, &new).unwrap();
        state.rescan_root_blocking(lib.clone(), true).unwrap();

        let shared = state.shared.lock().unwrap();
        let assets = shared.assets.read().unwrap();
        assert_eq!(assets.len(), 2);
        let old_asset = assets.iter().find(|asset| asset.path == old).unwrap();
        assert!(
            old_asset
                .all_tags()
                .any(|tag| tag.tag == "keepme" && tag.category == "manual")
        );
        let new_asset = assets.iter().find(|asset| asset.path == new).unwrap();
        assert!(!new_asset.all_tags().any(|tag| tag.tag == "keepme"));
        drop(assets);
        drop(shared);
        std::fs::remove_dir_all(&base).unwrap();
    }
}
