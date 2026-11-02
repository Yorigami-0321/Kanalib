use crate::*;

/// 网格 tile 的解码图上限。gpui 默认的全局 asset cache 只进不出：滚过多少张缩略图就常驻
/// 多少张 BGRA（512 档约 1MB/张，10k 库跑一遍就是 GB 级），这里换成按 LRU 限量的缓存。
pub(crate) const IMAGE_CACHE_LIMIT: usize = 128;

/// 解码失败后隔多久允许同一资源重试；避免坏缩略图整会话空白，也避免每帧重试。
const THUMB_RETRY_MS: u64 = 30_000;

fn clamp_cache_limit(limit: usize) -> usize {
    limit.max(1)
}

fn wall_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
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
pub(crate) enum ThumbEntry {
    Gpui(ImageCacheItem),
    Loading,
    Loaded(Result<Arc<RenderImage>, ImageCacheError>),
    Failed { at: u64, error: ImageCacheError },
}

/// 有界的解码图缓存：可见 tile 每帧都会 touch，所以只会被更近的挤掉，不会被自己挤掉。
pub struct ThumbCache {
    pub(crate) entries: HashMap<Resource, (ThumbEntry, u64)>,
    pub(crate) clock: u64,
    pub(crate) limit: usize,
    pub(crate) cache_dir: PathBuf,
    pub(crate) db_path: PathBuf,
    pub(crate) access_path: PathBuf,
    pub(crate) access: HashMap<PathBuf, u64>,
    pub(crate) access_dirty: usize,
    pub(crate) last_access_flush: Instant,
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    /// 取证档：按 Arc 指针登记每个交出的解码图，dump 时统计仍存活的 RenderImage 数。
    #[cfg(feature = "frame-profiler")]
    pub(crate) watch: HashMap<usize, std::sync::Weak<RenderImage>>,
    pub(crate) entity: WeakEntity<ThumbCache>,
}

#[derive(Clone, Debug, Default)]
pub struct CacheCleanup {
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub budget_bytes: u64,
    pub removed_files: usize,
    pub removed_bytes: u64,
    pub removed_groups: usize,
    pub protected_groups: usize,
    pub failed_files: usize,
    pub removed_paths: Vec<PathBuf>,
}

impl ThumbCache {
    pub(crate) fn with_limit(
        limit: usize,
        cache_dir: PathBuf,
        db_path: PathBuf,
        access_path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Self {
        let access = std::fs::read(&access_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HashMap<String, u64>>(&bytes).ok())
            .map(|map| {
                map.into_iter()
                    .map(|(path, used)| (PathBuf::from(path), used))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            entries: HashMap::new(),
            clock: 0,
            limit: clamp_cache_limit(limit),
            cache_dir,
            db_path,
            access_path,
            access,
            access_dirty: 0,
            last_access_flush: Instant::now(),
            hits: 0,
            misses: 0,
            #[cfg(feature = "frame-profiler")]
            watch: HashMap::new(),
            entity: cx.entity().downgrade(),
        }
    }

    pub(crate) fn flush_access_if_due(&mut self, cx: &mut Context<Self>) {
        if self.access_dirty == 0 {
            return;
        }
        if self.access_dirty < 512 && self.last_access_flush.elapsed() < Duration::from_secs(30) {
            return;
        }
        self.flush_access(cx);
    }

    fn flush_access(&mut self, cx: &mut Context<Self>) {
        if self.access_dirty == 0 {
            return;
        }
        self.access_dirty = 0;
        self.last_access_flush = Instant::now();
        let snapshot = self
            .access
            .iter()
            .filter_map(|(path, used)| path.to_str().map(|path| (path.to_string(), *used)))
            .collect::<HashMap<_, _>>();
        let Ok(json) = serde_json::to_vec(&snapshot) else {
            return;
        };
        let path = self.access_path.clone();
        if let Some(parent) = path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            log::warn!("create thumb access dir {}: {error}", parent.display());
            return;
        }
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = std::fs::write(&path, json) {
                    log::warn!("write thumb access {}: {error}", path.display());
                }
            })
            .detach();
    }

    pub(crate) fn access_stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    pub(crate) fn cleanup_snapshot(&self) -> (PathBuf, HashMap<PathBuf, u64>, HashSet<PathBuf>) {
        let protected = self
            .entries
            .keys()
            .filter_map(|resource| match resource {
                Resource::Path(path) => Some(PathBuf::from(path.as_ref())),
                _ => None,
            })
            .collect();
        (self.cache_dir.clone(), self.access.clone(), protected)
    }

    pub(crate) fn apply_cleanup(&mut self, report: &CacheCleanup, cx: &mut Context<Self>) {
        for path in &report.removed_paths {
            self.access.remove(path);
        }
        self.access_dirty += report.removed_paths.len();
        self.flush_access(cx);
    }

    /// 磁盘扫描与删除在后台线程执行；这里只接收只读快照，不碰 UI 实体。
    pub(crate) fn cleanup_budget(
        cache_dir: &Path,
        access: &HashMap<PathBuf, u64>,
        protected: &HashSet<PathBuf>,
        budget_bytes: u64,
    ) -> CacheCleanup {
        let mut report = CacheCleanup {
            budget_bytes,
            ..Default::default()
        };
        let entries = match std::fs::read_dir(cache_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return report,
            Err(error) => {
                log::warn!("read thumb cache {}: {error}", cache_dir.display());
                return report;
            }
        };
        struct Group {
            files: Vec<(PathBuf, u64)>,
            bytes: u64,
            last_used: u64,
        }
        let mut groups: HashMap<String, Group> = HashMap::new();
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            // 512 档与 `-t256` 派生档必须归入同一组，否则保护主图时会删掉小档。
            let base = name.strip_suffix(".jpg").unwrap_or(name.as_str());
            let key = base.strip_suffix("-t256").unwrap_or(base).to_string();
            let modified = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0);
            let last_used = modified.max(access.get(&path).copied().unwrap_or(0));
            let group = groups.entry(key).or_insert(Group {
                files: Vec::new(),
                bytes: 0,
                last_used,
            });
            group.files.push((path, size));
            group.bytes += size;
            group.last_used = group.last_used.max(last_used);
        }
        report.before_bytes = groups.values().map(|group| group.bytes).sum();
        report.after_bytes = report.before_bytes;
        if report.before_bytes <= budget_bytes {
            return report;
        }
        let recently_used = wall_millis().saturating_sub(5 * 60 * 1000);
        let target = budget_bytes - budget_bytes / 20;
        let mut ordered = groups.into_iter().collect::<Vec<_>>();
        ordered.sort_by_key(|(_, group)| group.last_used);
        for (_, group) in ordered {
            if report.after_bytes <= target {
                break;
            }
            let recently_accessed = group
                .files
                .iter()
                .any(|(path, _)| access.get(path).is_some_and(|used| *used >= recently_used));
            let is_protected =
                recently_accessed || group.files.iter().any(|(path, _)| protected.contains(path));
            if is_protected {
                report.protected_groups += 1;
                continue;
            }
            let mut group_removed = 0usize;
            for (path, size) in &group.files {
                match std::fs::remove_file(path) {
                    Ok(()) => {
                        report.after_bytes = report.after_bytes.saturating_sub(*size);
                        report.removed_bytes += size;
                        report.removed_files += 1;
                        report.removed_paths.push(path.clone());
                        group_removed += 1;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        report.after_bytes = report.after_bytes.saturating_sub(*size);
                        report.removed_bytes += size;
                        report.removed_files += 1;
                        report.removed_paths.push(path.clone());
                        group_removed += 1;
                    }
                    Err(error) => {
                        report.failed_files += 1;
                        log::warn!("remove thumb {}: {error}", path.display());
                    }
                }
            }
            if group_removed > 0 {
                report.removed_groups += 1;
            }
        }
        report
    }

    fn spawn_decode(&self, resource: Resource, cx: &mut App) {
        let key = resource.clone();
        let db_path = self.db_path.clone();
        let cache_dir = self.cache_dir.clone();
        let task = cx
            .background_executor()
            .spawn(async move { decode_thumb(&resource, &db_path, &cache_dir) });
        let weak = self.entity.clone();
        cx.spawn(async move |cx| {
            let result = task.await;
            let _ = weak.update(cx, |cache, cx| {
                if let Some((entry, _)) = cache.entries.get_mut(&key) {
                    *entry = match result {
                        Ok(image) => ThumbEntry::Loaded(Ok(image)),
                        Err(error) => ThumbEntry::Failed {
                            at: wall_millis(),
                            error,
                        },
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 缓存里已解码位图的字节数（BGRA，逐帧累计），用来区分"缓存装太多"和"上色板涨"。
    #[cfg(feature = "frame-profiler")]
    pub(crate) fn cached_bytes(&self) -> u64 {
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
    pub(crate) fn watch_stats(&self) -> (usize, usize) {
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
fn decode_thumb(
    resource: &Resource,
    db_path: &Path,
    cache_dir: &Path,
) -> Result<Arc<RenderImage>, ImageCacheError> {
    let Resource::Path(path) = resource else {
        return Err(ImageCacheError::Asset(
            "thumb cache only decodes file resources".into(),
        ));
    };
    if !path.exists()
        && !kanalib_core::state::rebuild_missing_thumb(db_path, cache_dir, path.as_ref())
    {
        log::warn!("thumb missing and could not rebuild: {}", path.display());
    }
    let bytes =
        std::fs::read(path.as_ref()).map_err(|error| ImageCacheError::Io(Arc::new(error)))?;
    let image =
        image::load_from_memory(&bytes).map_err(|error| ImageCacheError::Image(Arc::new(error)))?;
    let mut buffer = image.into_rgba8();
    for pixel in buffer.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

#[cfg(feature = "frame-profiler")]
pub(crate) fn watch_enabled() -> bool {
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
        if let Resource::Path(path) = resource {
            self.access
                .insert(PathBuf::from(path.as_ref()), wall_millis());
            self.access_dirty += 1;
        }
        let mut retry_failed = false;
        if let Some((entry, used)) = self.entries.get_mut(resource) {
            self.hits += 1;
            *used = clock;
            match entry {
                ThumbEntry::Gpui(item) => {
                    let _ = item.use_image(window);
                }
                ThumbEntry::Failed { at, .. }
                    if wall_millis().saturating_sub(*at) >= THUMB_RETRY_MS =>
                {
                    // 失败过期：重新排队解码，而不是把错误永久钉在缓存里。
                    *entry = ThumbEntry::Loading;
                    retry_failed = true;
                }
                _ => {}
            }
        } else {
            self.misses += 1;
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
        if retry_failed {
            self.misses += 1;
            self.spawn_decode(resource.clone(), cx);
        }
        let result = match self.entries.get(resource) {
            Some((ThumbEntry::Loaded(result), _)) => Some(result.clone()),
            Some((ThumbEntry::Failed { error, .. }, _)) => Some(Err(error.clone())),
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

#[cfg(test)]
mod tests {
    use super::{ThumbCache, cache_victims, clamp_cache_limit};
    use gpui::Resource;
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

    #[test]
    fn budget_cleanup_removes_unprotected_groups_only() {
        let dir = std::env::temp_dir().join(format!(
            "kanalib-cleanup-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.jpg");
        let a_tile = dir.join("a-t256.jpg");
        let b = dir.join("b.jpg");
        for path in [&a, &a_tile, &b] {
            std::fs::write(path, vec![0u8; 100]).unwrap();
        }
        let protected = std::collections::HashSet::from([a.clone()]);
        let report = ThumbCache::cleanup_budget(&dir, &HashMap::new(), &protected, 0);
        assert!(a.exists(), "protected group must survive");
        assert!(a_tile.exists());
        assert!(!b.exists());
        assert_eq!(report.removed_files, 1);
        assert_eq!(report.removed_paths, vec![b]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
