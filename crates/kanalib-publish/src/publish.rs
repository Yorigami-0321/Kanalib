//! 导出主流程：扫描本地库 -> 写分片/缩略图/锐利档/原图 -> 写 index.json 与账本。
//!
//! 语义要点（对照 docs/backlog/photo-wall-contract.md）：
//! - 排序完全交给 core 的 Db::search_page（Modified 降序 + path 兜底），这里只按游标顺序消费；
//! - 每个 item 的 t512 必定存在（缩略图源缺失就跳过该资产并 warn），t256 只是锦上添花；
//! - t1024/t2048 从原图解码一次、按 ICC/EXIF 转 sRGB 与方向、Lanczos 缩放后以 q90 重编码，
//!   源缺失/不可解码/视频宁可缺档也不放大，前端逐档回退；
//! - 只读：除账本与产物外不改库；打开库时的补列/建索引由 core 负责（stderr 已提示）；
//! - --dry-run 不写任何文件；日志一律走 stderr。

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use kanalib_core::color;
use kanalib_core::db::{Db, SearchCursor, SearchPageRequest, SearchSort};
use kanalib_core::model::{Asset, Filter, SearchQuery, TagHit};
use kanalib_core::sources;

use crate::fsutil;
use crate::ledger::{LEDGER_FILE, Ledger, LedgerEntry};
use crate::manifest::{self, Facets, Index, Item, Paths, SCHEMA_VERSION, Shard, ShardRef};
use crate::timefmt;

/// 分页抓取步长（core 允许到 10000，1000 足够减少查询次数又不过度占用内存）。
const PAGE_LIMIT: usize = 1000;

/// 锐利档：长边像素与目录名。只从原图生成，绝不把小图放大。
const TIERS: [(&str, u32); 2] = [("t1024", 1024), ("t2048", 2048)];
/// 锐利档 JPEG 质量（photo-wall W3：高质量、正确 sRGB）。
const TIER_JPEG_QUALITY: u8 = 90;

/// 发布参数，由 CLI 或库调用方构造。
#[derive(Clone, Debug)]
pub struct Options {
    pub db_path: PathBuf,
    pub out_dir: PathBuf,
    /// 最多导出多少条（None = 全量）。
    pub limit: Option<usize>,
    /// 是否导出原图到 o/<id>.<ext>。
    pub with_originals: bool,
    /// 是否从原图生成 t1024/t2048 锐利档。
    pub tiers: bool,
    /// 只打印统计，不写盘。
    pub dry_run: bool,
    pub shard_size: usize,
    pub max_facet_tags: usize,
    pub max_facet_groups: usize,
    /// 显式指定的本地根目录（空 = 读 settings.library.local_roots 再回退 sources）。
    pub roots: Vec<PathBuf>,
    /// 是否删除未被本次 index.json 引用的陈旧分片（默认只警告）。
    pub prune_stale: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            db_path: PathBuf::new(),
            out_dir: PathBuf::new(),
            limit: None,
            with_originals: false,
            tiers: true,
            dry_run: false,
            shard_size: manifest::DEFAULT_SHARD_SIZE,
            max_facet_tags: manifest::MAX_FACET_TAGS,
            max_facet_groups: manifest::MAX_FACET_GROUPS,
            roots: Vec::new(),
            prune_stale: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct FileStats {
    pub written: usize,
    pub skipped: usize,
    pub missing: usize,
    pub failed: usize,
}

/// 锐利档统计。written_* 按文件计；missing/failed 按资产计（一个资产解码失败不会重复计数）。
#[derive(Clone, Debug, Default)]
pub struct TierStats {
    pub written_t1024: usize,
    pub written_t2048: usize,
    pub skipped: usize,
    pub not_needed: usize,
    pub missing: usize,
    pub failed: usize,
}

impl TierStats {
    pub fn written(&self) -> usize {
        self.written_t1024 + self.written_t2048
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub scanned: usize,
    pub exported: usize,
    pub skipped_no_thumb: usize,
    pub skipped_invalid: usize,
    pub duplicates: usize,
    /// 从同内容重复副本补回标签的条目数。
    pub tags_from_duplicates: usize,
    /// 裁掉源图纯白底带的条目数（反馈 §5.6）。
    pub trimmed: usize,
    pub shards: usize,
    pub shard_size: usize,
    pub tags: usize,
    pub groups: usize,
    pub thumbs: FileStats,
    pub tiers: TierStats,
    pub originals: FileStats,
    pub stale_shards: usize,
    pub pruned_shards: usize,
}

impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "扫描 {} 条；导出 {} 条（重复 id {}、缺可用缩略图 {}、字段不合法 {}）",
            self.scanned,
            self.exported,
            self.duplicates,
            self.skipped_no_thumb,
            self.skipped_invalid
        )?;
        if self.tags_from_duplicates > 0 {
            writeln!(
                f,
                "其中 {} 条的标签来自同内容副本（被保留的那份没打标签）",
                self.tags_from_duplicates
            )?;
        }
        if self.trimmed > 0 {
            writeln!(f, "其中 {} 条裁掉了源图纯白底带（全档重编码）", self.trimmed)?;
        }
        writeln!(
            f,
            "分片 {} 片（每片上限 {}）；facets 标签 {} 个 / 分组 {} 个",
            self.shards, self.shard_size, self.tags, self.groups
        )?;
        writeln!(
            f,
            "缩略图 t512/t256：新写 {}、跳过（账本命中）{}、源缺失 {}、失败 {}",
            self.thumbs.written, self.thumbs.skipped, self.thumbs.missing, self.thumbs.failed
        )?;
        writeln!(
            f,
            "锐利档 t1024/t2048：新写 {}/{}、跳过（账本命中）{}、无需生成 {}、源缺失 {}、失败 {}",
            self.tiers.written_t1024,
            self.tiers.written_t2048,
            self.tiers.skipped,
            self.tiers.not_needed,
            self.tiers.missing,
            self.tiers.failed
        )?;
        write!(
            f,
            "原图：新写 {}、跳过（账本命中）{}、源缺失 {}、失败 {}",
            self.originals.written,
            self.originals.skipped,
            self.originals.missing,
            self.originals.failed
        )
    }
}

/// 源指纹：判定账本里的条目是否仍然有效。
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileMeta {
    sha1: Option<String>,
    mtime: u64,
    bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Thumb,
    Original,
}

/// 执行一次导出。返回统计；出错时返回带上下文的错误。
pub fn run(options: &Options) -> Result<Stats> {
    if options.shard_size == 0 {
        bail!("--shard-size 必须大于 0");
    }
    if options.out_dir.as_os_str().is_empty() {
        bail!("--out 不能为空");
    }
    if !options.db_path.is_file() {
        bail!("数据库不存在：{}", options.db_path.display());
    }
    eprintln!("[kanalib-publish] 注意：打开库时会执行 core 迁移（补列/建索引），与桌面行为一致");
    let db = Db::open(&options.db_path)
        .with_context(|| format!("打开数据库失败：{}", options.db_path.display()))?;
    let folders = resolve_folders(&db, &options.roots)?;
    if folders.is_empty() {
        bail!(
            "没有可扫描的本地根目录：settings.library.local_roots 与 sources 表均为空，请用 --root 指定"
        );
    }
    eprintln!(
        "[kanalib-publish] 本地根目录 {} 个：{}",
        folders.len(),
        folders.join("；")
    );

    let mut publisher = Publisher::new(options)?;
    publisher.scan(&db, &folders)?;
    let stats = publisher.finish()?;
    Ok(stats)
}

/// 把设置/来源表里的本地根目录整理成 assets.folder 值列表。
fn resolve_folders(db: &Db, explicit: &[PathBuf]) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut folders = Vec::new();
    if !explicit.is_empty() {
        for root in explicit {
            if !root.is_dir() {
                eprintln!(
                    "[kanalib-publish] 警告：--root 指定的目录不存在或不是目录：{}",
                    root.display()
                );
            }
            let text = root.to_string_lossy().to_string();
            if seen.insert(folder_key(&text)) {
                folders.push(text);
            }
        }
        return Ok(folders);
    }

    if let Some(raw) = db
        .setting("library.local_roots")
        .context("读取 settings.library.local_roots 失败")?
    {
        match serde_json::from_str::<Vec<String>>(&raw) {
            Ok(list) => {
                for item in list {
                    if seen.insert(folder_key(&item)) {
                        folders.push(item);
                    }
                }
            }
            Err(error) => eprintln!(
                "[kanalib-publish] 警告：settings.library.local_roots 不是字符串数组（{error}），改用 sources 表"
            ),
        }
    }

    if folders.is_empty() {
        let records = db.list_sources().context("读取 sources 表失败")?;
        for record in records {
            if record.kind != "local" {
                continue;
            }
            if seen.insert(folder_key(&record.locator)) {
                folders.push(record.locator);
            }
        }
        eprintln!(
            "[kanalib-publish] settings.library.local_roots 为空，回退到 sources 表中的 {} 个本地来源",
            folders.len()
        );
    }
    Ok(folders)
}

/// 目录比较键：统一分隔符、去尾斜杠，Windows 下忽略大小写。
fn folder_key(folder: &str) -> String {
    let trimmed = folder.replace('\\', "/");
    let trimmed = trimmed.trim_end_matches('/');
    if cfg!(windows) {
        trimmed.to_lowercase()
    } else {
        trimmed.to_string()
    }
}

/// 从缩略图路径派生 t256 网格档路径（core 的实现不公开，规则是 {stem}-t256.jpg）。
fn tile_thumb_path(thumb: &Path) -> PathBuf {
    let stem = thumb
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    thumb.with_file_name(format!("{stem}-t256.jpg"))
}

/// 底部连续纯白 (255,255,255) 行带：源图上传侧垫白/损坏的痕迹（反馈 §5.6）。
/// 源图确认用 4%/8 行；t256 候选检测用 3%/3 行——带边界的 JPEG ringing
/// 会让小档上的候选带高略低于真值，阈值放宽以免漏检。
fn white_bottom_band(rgb: &image::RgbImage, min_frac: f64, min_rows: u32) -> Option<u32> {
    let (width, height) = rgb.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let mut band = 0u32;
    for y in (0..height).rev() {
        if !(0..width).all(|x| rgb.get_pixel(x, y).0 == [255, 255, 255]) {
            break;
        }
        band += 1;
    }
    if band >= min_rows && band < height && f64::from(band) >= f64::from(height) * min_frac {
        Some(band)
    } else {
        None
    }
}

/// 原图扩展名：小写、无点、只允许字母数字，最长 10。
fn original_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    if ext.is_empty() || ext.len() > 10 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(ext)
}

struct Publisher<'a> {
    options: &'a Options,
    /// 上次发布的账本（用于增量跳过）。
    previous: Ledger,
    /// 本次发布写出的账本。
    next: Ledger,
    seen: HashSet<String>,
    /// 同内容重复副本的标签池（键是小写 sha1 / blake3）：保留的那份没标签时用来补位。
    duplicate_tags: HashMap<String, Vec<TagHit>>,
    tag_counts: HashMap<String, manifest::TagCount>,
    group_counts: HashMap<String, usize>,
    pending: Vec<Item>,
    shards: Vec<ShardRef>,
    stats: Stats,
}

impl<'a> Publisher<'a> {
    fn new(options: &'a Options) -> Result<Self> {
        let stats = Stats {
            shard_size: options.shard_size,
            ..Stats::default()
        };
        Ok(Self {
            options,
            previous: Ledger::load(&options.out_dir),
            next: Ledger::default(),
            seen: HashSet::new(),
            duplicate_tags: HashMap::new(),
            tag_counts: HashMap::new(),
            group_counts: HashMap::new(),
            pending: Vec::new(),
            shards: Vec::new(),
            stats,
        })
    }

    fn limit_reached(&self) -> bool {
        self.options
            .limit
            .is_some_and(|limit| self.stats.exported >= limit)
    }

    fn scan(&mut self, db: &Db, folders: &[String]) -> Result<()> {
        // 同一内容的重复副本只导出第一条；若第一条没标签、另一条有，用这张表补位。
        // 数据侧真出现过：样例图在 stress 库里有一份未打标签的拷贝，且 mtime 更新，
        // 于是被保留下来、标签全丢（不是 asset_tags 没 join）。
        self.duplicate_tags = db
            .tags_for_duplicated_assets()
            .context("读取重复资产的标签失败")?;
        let filter = Filter::All;
        let query = SearchQuery::default();
        let mut cursor: Option<SearchCursor> = None;
        'scan: loop {
            let request = SearchPageRequest {
                filter: &filter,
                query: &query,
                sort: SearchSort::Modified,
                ascending: false,
                limit: PAGE_LIMIT,
                cursor: cursor.as_ref(),
                duplicate_paths: None,
                include_total: false,
            };
            let page = db
                .search_page(folders, request)
                .context("分页读取资产列表失败")?;
            if page.paths.is_empty() {
                break;
            }
            for path in &page.paths {
                if self.limit_reached() {
                    break 'scan;
                }
                self.stats.scanned += 1;
                match db.load_asset(path) {
                    Ok(mut asset) => {
                        self.fill_tags_from_duplicates(&mut asset);
                        self.push_asset(&asset)?
                    }
                    Err(error) => {
                        self.stats.skipped_invalid += 1;
                        eprintln!(
                            "[kanalib-publish] 警告：读取资产失败，跳过 {}（{error:#}）",
                            path.display()
                        );
                    }
                }
            }
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(())
    }

    /// 保留的那份没有任何标签时，用同内容副本的标签补位。
    /// 只补"空"的情况：两份都有标签时以被保留那条为准，避免把不同批次的打标结果混在一起。
    fn fill_tags_from_duplicates(&mut self, asset: &mut Asset) {
        if !asset.tags.is_empty() || !asset.manual_tags.is_empty() || self.duplicate_tags.is_empty()
        {
            return;
        }
        let lookup = |hash: Option<&str>| {
            hash.map(|value| value.to_ascii_lowercase())
                .and_then(|value| self.duplicate_tags.get(&value).cloned())
        };
        if let Some(tags) =
            lookup(asset.sha1.as_deref()).or_else(|| lookup(asset.blake3.as_deref()))
        {
            asset.tags = tags;
            self.stats.tags_from_duplicates += 1;
        }
    }

    /// 处理单条资产：校验 -> 复制文件 -> 入账本 -> 攒进当前分片。
    fn push_asset(&mut self, asset: &Asset) -> Result<()> {
        let id = manifest::asset_id(asset);
        if !self.seen.insert(id.clone()) {
            self.stats.duplicates += 1;
            return Ok(());
        }
        if asset.width == 0 || asset.height == 0 {
            self.stats.skipped_invalid += 1;
            eprintln!(
                "[kanalib-publish] 警告：宽高为 0，跳过 {}",
                asset.path.display()
            );
            return Ok(());
        }
        if sources::is_remote_path(&asset.path) {
            self.stats.skipped_invalid += 1;
            eprintln!(
                "[kanalib-publish] 警告：远程只读来源不发布，跳过 {}",
                asset.path.display()
            );
            return Ok(());
        }
        let Some(thumb) = asset.thumb.clone() else {
            self.stats.skipped_no_thumb += 1;
            eprintln!(
                "[kanalib-publish] 警告：没有缩略图记录，跳过 {}",
                asset.path.display()
            );
            return Ok(());
        };

        let meta = FileMeta {
            sha1: asset
                .sha1
                .as_deref()
                .map(|value| value.to_ascii_lowercase())
                .or_else(|| {
                    asset
                        .blake3
                        .as_deref()
                        .map(|value| value.to_ascii_lowercase())
                }),
            mtime: asset.mtime,
            bytes: asset.bytes,
        };
        self.next.insert(
            &id,
            LedgerEntry {
                sha1: meta.sha1.clone(),
                mtime: meta.mtime,
                bytes: meta.bytes,
                files: Vec::new(),
                trim_band: None,
            },
        );

        let tile = tile_thumb_path(&thumb);
        // 纯白底带裁剪（反馈 §5.6）：源图上传侧垫白/损坏留下全宽纯白底带，
        // 各档缩略图都会继承，与解码瞬态无关。在导出 t256 上候选检测（源指纹
        // 变化时用缓存 t256），精确边界以源图为准；裁掉后全档重编码。账本记
        // trim_band，增量跑据此保持 manifest 高度不回退。
        let prior = self.previous.get(&id);
        let source_current =
            prior.is_some_and(|entry| entry.is_current(meta.sha1.as_deref(), meta.mtime, meta.bytes));
        let mut band = if source_current {
            prior.and_then(|entry| entry.trim_band)
        } else {
            None
        };
        if band.is_none() {
            let out_tile = self.options.out_dir.join(format!("t256/{id}.jpg"));
            let probe_path = if source_current || !tile.is_file() {
                out_tile
            } else {
                tile.clone()
            };
            if let Ok(probe) = image::open(&probe_path).map(|value| value.to_rgb8()) {
                if white_bottom_band(&probe, 0.03, 3).is_some() {
                    // t256 带边界的 JPEG ringing 会低估带高，精确边界以源图为准。
                    if let Ok(decoded) = color::decode(&asset.path) {
                        let mut source = color::convert_to_srgb(decoded.image, &decoded.profile);
                        source.apply_orientation(decoded.orientation);
                        // 裁剪行数以方向套用后的源图为准；与库记高度不一致
                        // （旋转图）时不裁，避免 manifest 高度错配。
                        band = white_bottom_band(&source.to_rgb8(), 0.04, 8)
                            .filter(|_| source.height() == asset.height);
                    }
                }
            }
        }

        let mut item_asset = asset.clone();
        let sharp = match band.filter(|value| *value < asset.height) {
            Some(band) => {
                let height = asset.height - band;
                let long_edge = asset.width.max(height);
                let mut rels = vec![format!("t256/{id}.jpg"), format!("t512/{id}.jpg")];
                rels.extend(
                    TIERS
                        .iter()
                        .filter(|(_, edge)| long_edge > *edge)
                        .map(|(tier, _)| format!("{tier}/{id}.jpg")),
                );
                // 账本命中且裁剪产物都在：跳过重编码，只登记（契约 §6 增量语义）。
                // 必须确认账本记过 trim_band：升级首跑时旧产物还带白底带，
                // 不能当作已裁剪缓存跳过。
                let cached = source_current
                    && prior.is_some_and(|entry| {
                        entry.trim_band.is_some()
                            && rels.iter().all(|rel| {
                                entry.files.iter().any(|file| file == rel)
                                    && self.options.out_dir.join(rel).is_file()
                            })
                    });
                if cached {
                    for rel in &rels {
                        self.record_file(&id, rel);
                    }
                    self.stats.thumbs.skipped += 2;
                    self.stats.tiers.skipped += rels.len() - 2;
                    item_asset.height = height;
                    if let Some(entry) = self.next.assets.get_mut(&id) {
                        entry.trim_band = Some(band);
                    }
                    Some(
                        TIERS
                            .iter()
                            .filter(|(_, edge)| long_edge > *edge)
                            .map(|(_, edge)| *edge as u16)
                            .max()
                            .unwrap_or(0),
                    )
                } else {
                    match self.write_trimmed_tiers(&id, asset, band)? {
                        Some(sharp) => {
                            item_asset.height = height;
                            if let Some(entry) = self.next.assets.get_mut(&id) {
                                entry.trim_band = Some(band);
                            }
                            self.stats.trimmed += 1;
                            Some(sharp)
                        }
                        None => self.normal_tiers(&id, asset, &thumb, &tile, &meta)?,
                    }
                }
            }
            None => self.normal_tiers(&id, asset, &thumb, &tile, &meta)?,
        };
        let Some(sharp) = sharp else {
            return Ok(());
        };

        let mut orig = None;
        if self.options.with_originals {
            match original_extension(&asset.path) {
                Some(ext) => {
                    if asset.path.is_file() {
                        let rel = format!("o/{id}.{ext}");
                        if self.sync_file(&id, &rel, &asset.path, &meta, Slot::Original)? {
                            orig = Some(ext);
                        }
                    } else {
                        self.stats.originals.missing += 1;
                        eprintln!(
                            "[kanalib-publish] 警告：原图文件缺失，不导出 o/ {}",
                            asset.path.display()
                        );
                    }
                }
                None => eprintln!(
                    "[kanalib-publish] 警告：无法识别原图扩展名，不导出 o/ {}",
                    asset.path.display()
                ),
            }
        }

        let mut item = manifest::build_item(&item_asset);
        item.orig = orig;
        item.sharp = Some(sharp);
        manifest::count_tags(&mut self.tag_counts, &item);
        manifest::count_group(&mut self.group_counts, &item);
        self.pending.push(item);
        self.stats.exported += 1;
        if self.pending.len() >= self.options.shard_size {
            self.flush_shard()?;
        }
        Ok(())
    }

    /// 常规路径：t512/t256 增量复制缩略图缓存，锐利档从源解码。
    /// Ok(None) = t512 硬不变量不满足，整条跳过。
    fn normal_tiers(
        &mut self,
        id: &str,
        asset: &Asset,
        thumb: &Path,
        tile: &Path,
        meta: &FileMeta,
    ) -> Result<Option<u16>> {
        // t512 是硬不变量：源缩略图不可用就整条跳过。
        let t512 = format!("t512/{id}.jpg");
        if !self.sync_file(id, &t512, thumb, meta, Slot::Thumb)? {
            self.stats.skipped_no_thumb += 1;
            self.next.remove(id);
            return Ok(None);
        }
        // t256 网格档：源缺失不报错，只写 t512 并让前端回退。
        if tile.is_file() {
            let t256 = format!("t256/{id}.jpg");
            let _ = self.sync_file(id, &t256, tile, meta, Slot::Thumb)?;
        } else {
            self.stats.thumbs.missing += 1;
        }
        Ok(Some(self.sync_sharp_tiers(id, asset, meta)?))
    }

    /// 白底带裁剪路径：源解码一次、裁掉底带，从裁剪图写 t256/t512/t1024/t2048。
    /// Ok(None) = 源不可解码或裁完为空，调用方回退常规路径。
    fn write_trimmed_tiers(
        &mut self,
        id: &str,
        asset: &Asset,
        band: u32,
    ) -> Result<Option<u16>> {
        let decoded = match color::decode(&asset.path) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("[kanalib-publish] 警告：白底带裁剪解码失败，回退常规路径 {id}（{error}）");
                return Ok(None);
            }
        };
        let mut source = color::convert_to_srgb(decoded.image, &decoded.profile);
        source.apply_orientation(decoded.orientation);
        if source.height() != asset.height {
            return Ok(None);
        }
        let height = source.height().saturating_sub(band);
        if height == 0 {
            return Ok(None);
        }
        let cropped = source.crop_imm(0, 0, source.width(), height);
        let long_edge = cropped.width().max(cropped.height());
        let mut available = 0u16;
        for (tier, edge) in [("t256", 256u32), ("t512", 512), ("t1024", 1024), ("t2048", 2048)] {
            let sharp = edge >= 1024;
            if sharp && long_edge <= edge {
                self.stats.tiers.not_needed += 1;
                continue;
            }
            let rel = format!("{tier}/{id}.jpg");
            if self.options.dry_run {
                if sharp {
                    self.tier_written(tier);
                    available = available.max(edge as u16);
                } else {
                    self.stats.thumbs.written += 1;
                }
                continue;
            }
            let scaled = if long_edge > edge {
                cropped.resize(edge, edge, FilterType::Lanczos3)
            } else {
                cropped.clone()
            };
            let rgb = scaled.to_rgb8();
            let mut bytes = Vec::new();
            let encoded =
                JpegEncoder::new_with_quality(&mut bytes, TIER_JPEG_QUALITY).encode_image(&rgb);
            let written = match encoded {
                Ok(()) => {
                    let target = self.options.out_dir.join(&rel);
                    if let Some(parent) = target.parent() {
                        fsutil::ensure_dir(parent)?;
                    }
                    fsutil::write_atomic(&target, &bytes).is_ok()
                }
                Err(error) => {
                    eprintln!("[kanalib-publish] 警告：白底带裁剪编码 {rel} 失败（{error}）");
                    false
                }
            };
            if written {
                self.record_file(id, &rel);
                if sharp {
                    self.tier_written(tier);
                    available = available.max(edge as u16);
                } else {
                    self.stats.thumbs.written += 1;
                }
            } else if sharp {
                self.stats.tiers.failed += 1;
            } else {
                self.stats.thumbs.failed += 1;
            }
        }
        Ok(Some(available))
    }

    /// 生成/跳过 t1024/t2048：解码原图一次，转 sRGB、套 EXIF 方向，再按档缩放重编码。
    /// 源缺失、视频、不可解码都只降级（前端逐档回退），不中止发布。
    /// 返回本条目可用的最高档长边（0 = 无锐利档），写进 item.sharp 让前端不必 404 试探。
    fn sync_sharp_tiers(&mut self, id: &str, asset: &Asset, meta: &FileMeta) -> Result<u16> {
        if !self.options.tiers {
            return Ok(0);
        }
        // 视频的缩略图是首帧；原图是视频容器，不能当静图解码，也不该把首帧放大成"锐利档"。
        if kanalib_core::model::is_video(&asset.path) {
            self.stats.tiers.not_needed += TIERS.len();
            return Ok(0);
        }
        if !asset.path.is_file() {
            // 源库目录可能已被移除而缩略图缓存还在；这里按缺失计数，不逐条刷警告。
            self.stats.tiers.missing += 1;
            return Ok(0);
        }

        let long_edge = asset.width.max(asset.height);
        let mut available = 0u16;
        let mut jobs = Vec::new();
        for (tier, edge) in TIERS {
            if long_edge <= edge {
                self.stats.tiers.not_needed += 1;
                continue;
            }
            let rel = format!("{tier}/{id}.jpg");
            if self.file_is_current(id, &rel, meta) {
                self.stats.tiers.skipped += 1;
                self.record_file(id, &rel);
                available = available.max(edge as u16);
                continue;
            }
            if self.options.dry_run {
                self.tier_written(tier);
                available = available.max(edge as u16);
                continue;
            }
            jobs.push((tier, edge, rel));
        }
        if jobs.is_empty() {
            return Ok(available);
        }

        let decoded = match color::decode(&asset.path) {
            Ok(value) => value,
            Err(error) => {
                self.stats.tiers.failed += 1;
                eprintln!("[kanalib-publish] 警告：锐利档解码失败，跳过 {id}（{error}）");
                return Ok(available);
            }
        };
        let mut image = color::convert_to_srgb(decoded.image, &decoded.profile);
        image.apply_orientation(decoded.orientation);

        let mut failed = false;
        for (tier, edge, rel) in jobs {
            let scaled = if image.width().max(image.height()) > edge {
                image.resize(edge, edge, FilterType::Lanczos3)
            } else {
                image.clone()
            };
            let rgb = scaled.to_rgb8();
            let mut bytes = Vec::new();
            match JpegEncoder::new_with_quality(&mut bytes, TIER_JPEG_QUALITY).encode_image(&rgb) {
                Ok(()) => {
                    let target = self.options.out_dir.join(&rel);
                    if let Some(parent) = target.parent() {
                        fsutil::ensure_dir(parent)?;
                    }
                    match fsutil::write_atomic(&target, &bytes) {
                        Ok(()) => {
                            self.tier_written(tier);
                            self.record_file(id, &rel);
                            available = available.max(edge as u16);
                        }
                        Err(error) => {
                            failed = true;
                            eprintln!("[kanalib-publish] 警告：写锐利档 {rel} 失败（{error:#}）");
                        }
                    }
                }
                Err(error) => {
                    failed = true;
                    eprintln!("[kanalib-publish] 警告：编码锐利档 {rel} 失败（{error}）");
                }
            }
        }
        if failed {
            self.stats.tiers.failed += 1;
        }
        Ok(available)
    }

    fn tier_written(&mut self, tier: &str) {
        if tier == "t1024" {
            self.stats.tiers.written_t1024 += 1;
        } else {
            self.stats.tiers.written_t2048 += 1;
        }
    }

    /// 账本命中且源指纹未变、目标仍在。
    fn file_is_current(&self, id: &str, rel: &str, meta: &FileMeta) -> bool {
        self.previous.get(id).is_some_and(|entry| {
            entry.is_current(meta.sha1.as_deref(), meta.mtime, meta.bytes)
                && entry.files.iter().any(|file| file == rel)
        }) && self.options.out_dir.join(rel).is_file()
    }

    /// 增量复制：账本命中且源指纹未变、目标仍在 -> 跳过；返回目标是否可用。
    fn sync_file(
        &mut self,
        id: &str,
        rel: &str,
        source: &Path,
        meta: &FileMeta,
        slot: Slot,
    ) -> Result<bool> {
        if !source.is_file() {
            self.bucket_mut(slot).missing += 1;
            eprintln!(
                "[kanalib-publish] 警告：源文件缺失，跳过 {rel}（{}）",
                source.display()
            );
            return Ok(false);
        }
        let target = self.options.out_dir.join(rel);
        let unchanged = self.file_is_current(id, rel, meta);
        if unchanged {
            self.bucket_mut(slot).skipped += 1;
            self.record_file(id, rel);
            return Ok(true);
        }
        if self.options.dry_run {
            self.bucket_mut(slot).written += 1;
            return Ok(true);
        }
        if let Some(parent) = target.parent() {
            fsutil::ensure_dir(parent)?;
        }
        match fs::copy(source, &target) {
            Ok(_) => {
                self.bucket_mut(slot).written += 1;
                self.record_file(id, rel);
                Ok(true)
            }
            Err(error) => {
                self.bucket_mut(slot).failed += 1;
                eprintln!("[kanalib-publish] 警告：复制 {rel} 失败（{error}）");
                Ok(false)
            }
        }
    }

    fn bucket_mut(&mut self, slot: Slot) -> &mut FileStats {
        match slot {
            Slot::Thumb => &mut self.stats.thumbs,
            Slot::Original => &mut self.stats.originals,
        }
    }

    fn record_file(&mut self, id: &str, rel: &str) {
        if let Some(entry) = self.next.assets.get_mut(id) {
            if !entry.files.iter().any(|file| file == rel) {
                entry.files.push(rel.to_string());
            }
        }
    }

    fn flush_shard(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let index = self.shards.len();
        let items = std::mem::take(&mut self.pending);
        let count = items.len();
        let path = format!("m/{index:04}.json");
        if !self.options.dry_run {
            let shard = Shard {
                schema: SCHEMA_VERSION,
                index,
                items,
            };
            let bytes = serde_json::to_vec(&shard).context("序列化分片失败")?;
            fsutil::write_atomic(&self.options.out_dir.join(&path), &bytes)?;
        }
        self.shards.push(ShardRef {
            i: index,
            path,
            count,
        });
        Ok(())
    }

    fn finish(mut self) -> Result<Stats> {
        self.flush_shard()?;
        let tags = manifest::build_tag_facets(&self.tag_counts, self.options.max_facet_tags);
        let groups =
            manifest::build_group_facets(&self.group_counts, self.options.max_facet_groups);
        self.stats.shards = self.shards.len();
        self.stats.tags = tags.len();
        self.stats.groups = groups.len();
        if self.options.dry_run {
            return Ok(self.stats);
        }

        fsutil::ensure_dir(&self.options.out_dir)?;
        fsutil::ensure_dir(&self.options.out_dir.join("m"))?;
        fsutil::ensure_dir(&self.options.out_dir.join("t256"))?;
        fsutil::ensure_dir(&self.options.out_dir.join("t512"))?;
        if self.options.tiers {
            fsutil::ensure_dir(&self.options.out_dir.join("t1024"))?;
            fsutil::ensure_dir(&self.options.out_dir.join("t2048"))?;
        }
        if self.options.with_originals {
            fsutil::ensure_dir(&self.options.out_dir.join("o"))?;
        }

        let index = Index {
            schema: SCHEMA_VERSION,
            generated_at: timefmt::rfc3339_utc_now(),
            count: self.stats.exported,
            shard_size: self.options.shard_size,
            shards: self.shards.clone(),
            facets: Facets { tags, groups },
            paths: Paths::default(),
        };
        let bytes = serde_json::to_vec(&index).context("序列化 index.json 失败")?;
        fsutil::write_atomic(&self.options.out_dir.join("index.json"), &bytes)?;
        self.next.save(&self.options.out_dir)?;
        self.prune_stale()?;
        eprintln!(
            "[kanalib-publish] 已写出 {}（index.json + {} 分片 + {} / {}）",
            self.options.out_dir.display(),
            self.shards.len(),
            LEDGER_FILE,
            "账本条目"
        );
        Ok(self.stats)
    }

    /// 处理 m/ 下未被本次 index.json 引用的陈旧分片。
    fn prune_stale(&mut self) -> Result<()> {
        let dir = self.options.out_dir.join("m");
        let Ok(entries) = fs::read_dir(&dir) else {
            return Ok(());
        };
        let keep = self.shards.len();
        let mut stale = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let Some(stem) = name.strip_suffix(".json") else {
                continue;
            };
            if stem.is_empty() || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            let Ok(index) = stem.parse::<usize>() else {
                continue;
            };
            if index >= keep {
                stale.push((index, path));
            }
        }
        stale.sort_by_key(|(index, _)| *index);
        self.stats.stale_shards = stale.len();
        if stale.is_empty() {
            return Ok(());
        }
        let names = stale
            .iter()
            .map(|(_, path)| {
                path.file_name()
                    .map(|value| value.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join("、");
        if !self.options.prune_stale {
            eprintln!(
                "[kanalib-publish] 警告：m/ 下有 {} 个陈旧分片未被 index.json 引用：{names}；确认后可用 --prune-stale 清理",
                stale.len()
            );
            return Ok(());
        }
        let mut removed = 0;
        for (index, path) in stale {
            match fs::remove_file(&path) {
                Ok(()) => {
                    removed += 1;
                    eprintln!("[kanalib-publish] 已删除陈旧分片 m/{index:04}.json");
                }
                Err(error) => eprintln!(
                    "[kanalib-publish] 警告：删除陈旧分片 m/{index:04}.json 失败（{error}）"
                ),
            }
        }
        self.stats.pruned_shards = removed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::white_bottom_band;
    use image::RgbImage;

    fn image_with_band(width: u32, height: u32, band: u32) -> RgbImage {
        let mut img = RgbImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let pixel = if y >= height - band {
                    [255, 255, 255]
                } else {
                    [10, 120, 200]
                };
                img.put_pixel(x, y, image::Rgb(pixel));
            }
        }
        img
    }

    #[test]
    fn detects_trailing_white_band() {
        assert_eq!(white_bottom_band(&image_with_band(40, 200, 40), 0.04, 8), Some(40));
    }

    #[test]
    fn ignores_clean_image() {
        assert_eq!(white_bottom_band(&image_with_band(40, 200, 0), 0.04, 8), None);
    }

    #[test]
    fn ignores_band_below_threshold() {
        assert_eq!(white_bottom_band(&image_with_band(40, 200, 4), 0.04, 8), None);
    }

    #[test]
    fn ignores_all_white_image() {
        assert_eq!(white_bottom_band(&image_with_band(40, 200, 200), 0.04, 8), None);
    }

    #[test]
    fn candidate_threshold_is_looser() {
        assert_eq!(white_bottom_band(&image_with_band(40, 200, 7), 0.03, 3), Some(7));
        assert_eq!(white_bottom_band(&image_with_band(40, 200, 7), 0.04, 8), None);
    }

    #[test]
    fn band_must_reach_bottom_row() {
        // 白在中间、底行有色：不算底带。
        let mut img = image_with_band(40, 200, 0);
        for y in 100..140 {
            for x in 0..40 {
                img.put_pixel(x, y, image::Rgb([255, 255, 255]));
            }
        }
        assert_eq!(white_bottom_band(&img, 0.04, 8), None);
    }
}
