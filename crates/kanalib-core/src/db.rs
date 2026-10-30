use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::backup::{Backup, StepResult};
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags, OptionalExtension, ToSql, params};
use serde::{Deserialize, Serialize};

use crate::dedup::{DuplicateReview, ReviewState};
use crate::media::ContentHashes;
use crate::model::{
    Asset, BackupReport, Collection, DateFilter, DuplicateNameError, Filter, MediaKind, NumField,
    NumOp, NumericFilter, OrganizationKind, RetagSummary, SavedSearch, SavedSearchView,
    SearchQuery, SearchQueryParts, TAG_CANDIDATE_MIN_SCORE, TAG_CATEGORIES, TableCount,
    TagCandidateReview, TagHit, TagJobError, TagQueueTotals, TagRecomputeSummary, TagReviewState,
    TagSummary, TagThresholds, candidate_limit, day_number, default_category_thresholds,
    media_kind, normalize_tag_name,
};

/// v10：`assets.media_kind`（F-040 视频）。v9 及更早的库没有该列，必须在
/// `migrate` 里用 `ALTER TABLE ... DEFAULT 'image'` 补齐，否则任何读取
/// `media_kind` 的查询都会在旧库上报 "no such column"。
const SCHEMA_VERSION: i64 = 10;
const TAG_THRESHOLDS_KEY: &str = "tags.category_thresholds";
/// 库内是否存在内置 lower() 与 Rust Unicode 小写不同的名称；避免每次打开全表探测。
const NAME_FOLD_KEY: &str = "search.name_fold_unicode";
const ALLOW_CPU_TAGGING_KEY: &str = "tags.allow_cpu";

/// 备份时比对行数的关键表；只统计实际存在的表（以 `sqlite_master` 为准）。
const BACKUP_KEY_TABLES: &[&str] = &[
    "settings",
    "sources",
    "assets",
    "tags",
    "asset_tags",
    "jobs",
    "saved_searches",
    "collections",
    "collection_assets",
    "duplicate_reviews",
    "thumb_sources",
];

/// 只读检查数据库/备份文件的结果；恢复前用它决定是否接受候选。
#[derive(Clone, Debug, Default)]
pub struct BackupInspection {
    pub path: PathBuf,
    /// `PRAGMA integrity_check` 的原始结果，正常为 `ok`。
    pub integrity: String,
    /// `PRAGMA user_version`，即库的 schema 版本。
    pub user_version: i64,
    /// 实际存在的关键表及行数；缺失的表不会出现在这里。
    pub tables: Vec<TableCount>,
}

impl BackupInspection {
    pub fn integrity_ok(&self) -> bool {
        self.integrity == "ok"
    }

    /// 关键表行数；表不存在时返回 None。
    pub fn table_rows(&self, table: &str) -> Option<i64> {
        self.tables
            .iter()
            .find(|entry| entry.table == table)
            .map(|entry| entry.rows)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct JobCounts {
    pub pending: usize,
    pub running: usize,
    pub error: usize,
}

#[derive(Clone, Debug)]
pub struct TagJob {
    pub id: i64,
    pub path: PathBuf,
}

/// 搜索结果排序；语义与 UI 的 `SortMode` 一一对应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchSort {
    Name,
    Modified,
    Captured,
    Size,
    Resolution,
    Star,
    Favorite,
    Rating,
}

impl SearchSort {
    pub fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Modified => "modified",
            Self::Captured => "captured",
            Self::Size => "size",
            Self::Resolution => "resolution",
            Self::Star => "star",
            Self::Favorite => "favorite",
            Self::Rating => "rating",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Some(match value {
            "name" => Self::Name,
            "modified" => Self::Modified,
            "captured" => Self::Captured,
            "size" => Self::Size,
            "resolution" => Self::Resolution,
            "star" => Self::Star,
            "favorite" => Self::Favorite,
            "rating" => Self::Rating,
            _ => return None,
        })
    }
}

/// 名称排序使用的折叠方式。SQLite 内置 lower() 只折叠 ASCII；当库内存在
/// 非 ASCII 大写名称时改用 Rust 的 Unicode 小写函数，保证与旧内存排序一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NameFold {
    Ascii,
    Unicode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CursorKey {
    Name {
        fold: NameFold,
        name: String,
        path: String,
        id: i64,
    },
    Primary {
        fold: NameFold,
        primary: i64,
        name: String,
        path: String,
        id: i64,
    },
}

/// 稳定的 keyset 游标：包含排序键、path 与 id 作为 tie-breaker。
/// 不透明；只应原样回传给同一个排序方式的下一页请求。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchCursor {
    sort: SearchSort,
    ascending: bool,
    key: CursorKey,
}

impl SearchCursor {
    pub fn sort(&self) -> SearchSort {
        self.sort
    }

    pub fn ascending(&self) -> bool {
        self.ascending
    }
}

#[derive(Clone, Debug, Default)]
pub struct SearchPage {
    pub paths: Vec<PathBuf>,
    pub next: Option<SearchCursor>,
    /// 仅在请求 `include_total` 时返回；需要额外 COUNT 扫描。
    pub total: Option<usize>,
}

/// 分页搜索请求。`filter` 与 `query` 的语义和当前 UI 内存过滤完全一致。
#[derive(Clone, Debug)]
pub struct SearchPageRequest<'a> {
    pub filter: &'a Filter,
    pub query: &'a SearchQuery,
    pub sort: SearchSort,
    pub ascending: bool,
    pub limit: usize,
    pub cursor: Option<&'a SearchCursor>,
    /// `dup:true` 需要内存去重集合；不传时该查询按“无结果”处理（与 `matches` 一致）。
    pub duplicate_paths: Option<&'a HashSet<PathBuf>>,
    pub include_total: bool,
}

impl<'a> SearchPageRequest<'a> {
    pub fn new(
        filter: &'a Filter,
        query: &'a SearchQuery,
        sort: SearchSort,
        ascending: bool,
        limit: usize,
    ) -> Self {
        Self {
            filter,
            query,
            sort,
            ascending,
            limit,
            cursor: None,
            duplicate_paths: None,
            include_total: false,
        }
    }
}

pub const MAX_SEARCH_PAGE: usize = 10_000;

#[derive(Debug)]
struct SearchRow {
    path: String,
    primary: Option<i64>,
    name_key: String,
    path_key: String,
    id: i64,
}

/// FTS 命中条件的放置方式。分页查询把命中 rowid 作为 `IN (SELECT ...)` 内联，
/// 让 SQLite 复用同一个物化结果；范围探测把 FTS 表放在外层驱动，命中够多时
/// 直接停止，避免把十万条 rowid 整体物化成临时索引。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FtsPlacement {
    Inline,
    External,
}

/// 宽结果集下的排序计划。产品路径固定用覆盖索引；`TempSort` 只给
/// 基准测试复现「没有覆盖索引的旧 SQL」做同库对照，不对外暴露。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortPlan {
    CoverIndex,
    /// 仅供基准测试复现旧计划。
    #[cfg(test)]
    TempSort,
}

#[derive(Debug)]
struct SearchSql {
    where_sql: String,
    where_params: Vec<Value>,
    keyset_sql: Option<String>,
    keyset_params: Vec<Value>,
    order_sql: String,
    select_sql: String,
    fold: Option<NameFold>,
    force_index: Option<&'static str>,
    /// 自由文本走 FTS 时的 MATCH 查询；`FtsPlacement::External` 下由调用方
    /// 用 `assets_fts MATCH ?` 驱动连接。
    fts_query: Option<String>,
}

impl SearchSql {
    fn page_sql(&self) -> String {
        let from = match self.force_index {
            Some(index) => format!("assets INDEXED BY {index}"),
            None => "assets".to_string(),
        };
        let mut sql = format!(
            "SELECT {} FROM {from} WHERE {}",
            self.select_sql, self.where_sql
        );
        if let Some(keyset) = &self.keyset_sql {
            sql.push_str(" AND ");
            sql.push_str(keyset);
        }
        sql.push_str(" ORDER BY ");
        sql.push_str(&self.order_sql);
        sql.push_str(" LIMIT ?");
        sql
    }
}

#[derive(Debug, Default)]
struct SqlArgs {
    clauses: Vec<String>,
    params: Vec<Value>,
}

impl SqlArgs {
    fn new() -> Self {
        Self::default()
    }

    fn clause(&mut self, clause: impl Into<String>) {
        self.clauses.push(clause.into());
    }

    fn text(&mut self, value: impl Into<String>) {
        self.params.push(Value::Text(value.into()));
    }

    fn integer(&mut self, value: i64) {
        self.params.push(Value::Integer(value));
    }

    fn real(&mut self, value: f64) {
        self.params.push(Value::Real(value));
    }

    fn sql(&self) -> String {
        self.clauses.join(" AND ")
    }
}

fn placeholder_list(count: usize) -> String {
    vec!["?"; count].join(", ")
}

/// Windows 上 Rust `Path` 按组件比较，而 SQLite 文本按字节比较；把分隔符
/// 映射成小于所有文件名字符的哨兵字节，使两者在正常路径上顺序一致。
fn path_sort_expr() -> String {
    format!(
        "replace(assets.path, char({}), char(1))",
        std::path::MAIN_SEPARATOR as u32
    )
}

fn name_sort_expr(fold: NameFold) -> String {
    match fold {
        NameFold::Ascii => "lower(assets.name)".to_string(),
        NameFold::Unicode => "kanalib_lower(assets.name)".to_string(),
    }
}

fn sort_primary_expr(sort: SearchSort) -> Option<String> {
    Some(match sort {
        SearchSort::Name => return None,
        SearchSort::Modified => "assets.mtime".to_string(),
        SearchSort::Captured => "COALESCE(assets.captured_at, assets.mtime)".to_string(),
        SearchSort::Size => "assets.bytes".to_string(),
        SearchSort::Resolution => "assets.width * assets.height".to_string(),
        SearchSort::Star => "assets.star".to_string(),
        SearchSort::Favorite => {
            "CASE WHEN assets.favorite <> 0 THEN 1 ELSE 0 END".to_string()
        }
        SearchSort::Rating => "CASE lower(assets.rating) \
             WHEN 'g' THEN 1 WHEN 's' THEN 2 WHEN 'q' THEN 3 WHEN 'e' THEN 4 ELSE 0 END"
            .to_string(),
    })
}

/// 非名称排序的覆盖索引（只覆盖默认的降序方向）。索引与 ORDER BY 的
/// `primary DESC, lower(name), path, id` 完全一致，命中很多时可以按索引
/// 顺序扫描并在取满一页后停止。
///
/// 只为同库实测有正收益的排序建索引：Modified/Captured/Size/Rating。
/// Star/Favorite 基数低，覆盖索引反而比临时排序慢；Resolution 收益不足
/// 以抵消索引体积。名称折叠为 Unicode 时 ORDER BY 用 `kanalib_lower`，
/// 与索引里的 `lower(name)` 不匹配，只能回退临时排序。
fn sort_cover_index(sort: SearchSort) -> Option<&'static str> {
    Some(match sort {
        SearchSort::Modified => "idx_assets_sort_modified_desc",
        SearchSort::Captured => "idx_assets_sort_captured_desc",
        SearchSort::Size => "idx_assets_sort_size_desc",
        SearchSort::Rating => "idx_assets_sort_rating_desc",
        SearchSort::Name
        | SearchSort::Resolution
        | SearchSort::Star
        | SearchSort::Favorite => return None,
    })
}

fn register_sql_functions(conn: &Connection) -> Result<()> {
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function(
        "kanalib_lower",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |context| {
            let value = context.get::<Option<String>>(0)?;
            Ok(value.map(|text| text.to_lowercase()))
        },
    )?;
    Ok(())
}


const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS sources (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    locator TEXT NOT NULL DEFAULT '',
    config_json TEXT NOT NULL DEFAULT '{}',
    read_only INTEGER NOT NULL DEFAULT 1,
    last_sync_at INTEGER,
    sync_cursor TEXT,
    UNIQUE(kind, locator)
);
CREATE TABLE IF NOT EXISTS assets (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    folder TEXT NOT NULL,
    name TEXT NOT NULL,
    -- v10（F-040）：新库直接建列；v9 及更早的库由 `ensure_media_kind_column` 补列。
    media_kind TEXT NOT NULL DEFAULT 'image',
    mime TEXT NOT NULL DEFAULT '',
    width INTEGER NOT NULL DEFAULT 0,
    height INTEGER NOT NULL DEFAULT 0,
    bytes INTEGER NOT NULL DEFAULT 0,
    mtime INTEGER NOT NULL DEFAULT 0,
    captured_at INTEGER,
    sha1 TEXT,
    blake3 TEXT,
    phash INTEGER,
    thumb TEXT NOT NULL DEFAULT '',
    group_name TEXT NOT NULL DEFAULT '',
    rating TEXT NOT NULL DEFAULT '',
    tags_json TEXT NOT NULL DEFAULT '[]',
    tags_text TEXT NOT NULL DEFAULT '',
    tagged INTEGER NOT NULL DEFAULT 0,
    favorite INTEGER NOT NULL DEFAULT 0,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    exif_state TEXT NOT NULL DEFAULT '',
    thumb_state TEXT NOT NULL DEFAULT '',
    tag_state TEXT NOT NULL DEFAULT '',
    tag_attempts INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    damage TEXT,
    deleted_at INTEGER,
    UNIQUE(source_id, path)
);
CREATE INDEX IF NOT EXISTS idx_assets_folder ON assets(folder);
CREATE INDEX IF NOT EXISTS idx_assets_path ON assets(path);
CREATE INDEX IF NOT EXISTS idx_assets_sha1 ON assets(sha1);
CREATE INDEX IF NOT EXISTS idx_assets_phash ON assets(phash);
CREATE TABLE IF NOT EXISTS tags (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    category TEXT NOT NULL DEFAULT '',
    vocab_index INTEGER,
    threshold_override REAL,
    parent_id INTEGER REFERENCES tags(id) ON DELETE SET NULL,
    color TEXT,
    alias_of INTEGER
);
CREATE TABLE IF NOT EXISTS asset_tags (
    asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    confidence REAL NOT NULL DEFAULT 0,
    origin TEXT NOT NULL DEFAULT 'auto',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY(asset_id, tag_id, origin)
);
CREATE INDEX IF NOT EXISTS idx_asset_tags_tag ON asset_tags(tag_id);
CREATE TABLE IF NOT EXISTS collections (
    id INTEGER PRIMARY KEY,
    source_id INTEGER REFERENCES sources(id) ON DELETE CASCADE,
    locator TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS collection_assets (
    collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    PRIMARY KEY(collection_id, asset_id)
);
CREATE TABLE IF NOT EXISTS jobs (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    asset_id INTEGER REFERENCES assets(id) ON DELETE CASCADE,
    priority INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    run_after INTEGER
);
CREATE TABLE IF NOT EXISTS saved_searches (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    query_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS assets_fts USING fts5(
    name,
    path,
    tags_text,
    content='assets',
    content_rowid='id',
    tokenize='trigram'
);
CREATE TRIGGER IF NOT EXISTS assets_ai AFTER INSERT ON assets BEGIN
    INSERT INTO assets_fts(rowid, name, path, tags_text)
    VALUES (new.id, new.name, new.path, new.tags_text);
END;
CREATE TRIGGER IF NOT EXISTS assets_ad AFTER DELETE ON assets BEGIN
    INSERT INTO assets_fts(assets_fts, rowid, name, path, tags_text)
    VALUES ('delete', old.id, old.name, old.path, old.tags_text);
END;
CREATE TRIGGER IF NOT EXISTS assets_au AFTER UPDATE OF name, path, tags_text ON assets BEGIN
    INSERT INTO assets_fts(assets_fts, rowid, name, path, tags_text)
    VALUES ('delete', old.id, old.name, old.path, old.tags_text);
    INSERT INTO assets_fts(rowid, name, path, tags_text)
    VALUES (new.id, new.name, new.path, new.tags_text);
END;
"#;

const SCHEMA_V4: &str = r#"
CREATE TABLE IF NOT EXISTS duplicate_reviews (
    group_key TEXT PRIMARY KEY,
    state TEXT NOT NULL DEFAULT 'pending',
    keeper_path TEXT,
    member_count INTEGER NOT NULL DEFAULT 0,
    signature TEXT NOT NULL DEFAULT '',
    reviewed_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_duplicate_reviews_state ON duplicate_reviews(state);
"#;

const SCHEMA_V5: &str = r#"
DELETE FROM jobs
WHERE kind = 'tag'
  AND id NOT IN (SELECT MIN(id) FROM jobs WHERE kind = 'tag' GROUP BY asset_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_jobs_kind_asset ON jobs(kind, asset_id);
CREATE INDEX IF NOT EXISTS idx_jobs_state_kind ON jobs(kind, state, priority);
"#;

/// v8：每个资产每类只保留有限个模型候选（含阈值下分数），供阈值重算与低置信度审核。
/// 主键 (asset_id, tag) 同时承担按资产的查询索引；只有 top-K 候选落库，行数有上限。
const SCHEMA_V8: &str = r#"
CREATE TABLE IF NOT EXISTS asset_scores (
    asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT '',
    score REAL NOT NULL DEFAULT 0,
    vocab_index INTEGER,
    review_state TEXT NOT NULL DEFAULT 'pending',
    reviewed_at INTEGER,
    PRIMARY KEY(asset_id, tag)
);
CREATE INDEX IF NOT EXISTS idx_asset_scores_review
    ON asset_scores(review_state, category);
CREATE INDEX IF NOT EXISTS idx_tags_threshold_override
    ON tags(threshold_override) WHERE threshold_override IS NOT NULL;
"#;

/// v9：智能集合补齐排序/视图状态、置顶与手动顺序；手动相册补反向索引。
///
/// 新增列一律带默认值，v8 及更早写入的 `saved_searches` 行不需要改写：
/// `query_json` 仍只保存 filter/query，旧行读取后视图字段就是默认语义，
/// `updated_at = 0` 表示「迁移前的集合」。
///
/// `ALTER TABLE ADD COLUMN` 不是幂等语句，因此列添加放在 `ensure_saved_search_columns`
/// 里按 `PRAGMA table_info` 守卫；本常量只保留可重复执行的索引与顺序回填。
const SCHEMA_V9: &str = r#"
CREATE INDEX IF NOT EXISTS idx_saved_searches_order
    ON saved_searches(pinned DESC, position, id);
CREATE INDEX IF NOT EXISTS idx_collection_assets_asset
    ON collection_assets(asset_id);
"#;

/// v9 新增的集合列；顺序即写入顺序。
const SAVED_SEARCH_V9_COLUMNS: &[(&str, &str)] = &[
    ("sort_key", "TEXT NOT NULL DEFAULT ''"),
    ("sort_ascending", "INTEGER NOT NULL DEFAULT 1"),
    ("view_mode", "TEXT NOT NULL DEFAULT ''"),
    ("pane_mode", "TEXT NOT NULL DEFAULT ''"),
    ("show_sidebar", "INTEGER NOT NULL DEFAULT 1"),
    ("show_extensions", "INTEGER NOT NULL DEFAULT 1"),
    ("pinned", "INTEGER NOT NULL DEFAULT 0"),
    ("position", "INTEGER NOT NULL DEFAULT 0"),
    ("updated_at", "INTEGER NOT NULL DEFAULT 0"),
];

#[derive(Clone, Debug)]
pub struct ThumbSource {
    pub path: PathBuf,
    pub mtime: u64,
    pub bytes: u64,
    pub orientation: u8,
}

#[derive(Clone, Debug)]
pub struct StoredMeta {
    pub mtime: u64,
    pub bytes: u64,
    pub sha1: Option<String>,
    pub blake3: Option<String>,
    pub phash: Option<u64>,
    pub width: u32,
    pub height: u32,
    pub thumb: String,
    pub captured_at: Option<u64>,
    pub metadata_json: String,
    pub exif_state: String,
    /// F-040：落库的媒体类型；视频没有 pHash，缓存判定要区分。
    pub media_kind: MediaKind,
}

impl StoredMeta {
    pub fn is_unchanged(&self, mtime: u64, bytes: u64) -> bool {
        if self.mtime != mtime || self.bytes != bytes {
            return false;
        }
        if self.width == 0 || self.height == 0 || self.thumb.is_empty() {
            return false;
        }
        if !Path::new(&self.thumb).exists() {
            return false;
        }
        let phash_ok = self.media_kind == MediaKind::Video || self.phash.is_some();
        self.sha1.as_deref().is_some_and(|value| !value.is_empty())
            && self
                .blake3
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            && phash_ok
    }

    pub fn content_if_current(&self, mtime: u64, bytes: u64) -> Option<ContentHashes> {
        if self.mtime != mtime || self.bytes != bytes {
            return None;
        }
        let sha1 = self.sha1.clone().filter(|value| !value.is_empty())?;
        let blake3 = self.blake3.clone().filter(|value| !value.is_empty())?;
        // 视频不参与 pHash：复用哈希时返回 0/未解码，绝不因 phash 缺失而重算全库视频。
        if self.media_kind == MediaKind::Video {
            return Some(ContentHashes {
                sha1,
                blake3,
                phash: 0,
                decode_ok: false,
                decode_error: None,
            });
        }
        Some(ContentHashes {
            sha1,
            blake3,
            phash: self.phash?,
            decode_ok: true,
            decode_error: None,
        })
    }
}

struct OldAsset {
    path: String,
    folder: String,
    width: i64,
    height: i64,
    bytes: i64,
    mtime: i64,
    thumb: String,
    group_name: String,
    rating: String,
    tags_json: String,
    tagged: i64,
    error: Option<String>,
}

pub struct Db {
    conn: Connection,
    /// `PRAGMA data_version` + 名称折叠方式缓存；避免每次搜索都全表探测非 ASCII 大写。
    name_fold: Mutex<Option<(i64, NameFold)>>,
}

/// 备份临时文件守卫：只清理本次创建的临时文件；成功 rename 后不再清理。
struct BackupTemp {
    path: PathBuf,
    kept: bool,
}

impl BackupTemp {
    fn new(path: PathBuf) -> Self {
        Self { path, kept: false }
    }

    fn keep(&mut self) {
        self.kept = true;
    }
}

impl Drop for BackupTemp {
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        let _ = fs::remove_file(&self.path);
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut side = self.path.as_os_str().to_owned();
            side.push(suffix);
            let _ = fs::remove_file(PathBuf::from(side));
        }
    }
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Some(version) = migration_version(path)? {
            backup_database(path, version)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let _ = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0));
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        register_sql_functions(&conn)?;
        migrate(&conn)?;
        ensure_asset_columns(&conn)?;
        ensure_tag_columns(&conn)?;
        ensure_thumb_sources(&conn)?;
        ensure_search_indexes(&conn)?;
        conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_assets_path ON assets(path);")?;
        Ok(Self {
            conn,
            name_fold: Mutex::new(None),
        })
    }

    /// 把当前数据库备份到 `destination`，并校验备份文件。
    ///
    /// 步骤：先 `wal_checkpoint(TRUNCATE)` 把 WAL 中未落盘页写回主库（失败只记
    /// 警告，SQLite Backup API 仍会复制当前事务视图），再用 `rusqlite` 的 backup
    /// 把数据库复制到目标目录下的临时文件；校验 `integrity_check` 与关键表行数
    /// 后原子 rename 到目标路径。已存在的目标文件默认拒绝覆盖；失败时只删除本次
    /// 创建的临时产物，源库不受影响。
    pub fn backup_to(&self, destination: &Path) -> Result<BackupReport> {
        let started = Instant::now();
        if destination.as_os_str().is_empty() {
            bail!("备份目标路径为空");
        }
        if destination.exists() {
            bail!("备份文件已存在，拒绝覆盖：{}", destination.display());
        }
        let parent = destination
            .parent()
            .filter(|value| !value.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if !parent.is_dir() {
            bail!("备份目录不存在：{}", parent.display());
        }
        let file_name = destination
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "kanalib.db".to_string());
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let pid = std::process::id();
        let mut attempt = 0u32;
        let mut temp_path = parent.join(format!(".{file_name}.{pid}-{nonce}.tmp"));
        // 只使用本进程新建的临时文件，避免误改同目录已有文件。
        while temp_path.exists() {
            attempt += 1;
            temp_path = parent.join(format!(".{file_name}.{pid}-{nonce}-{attempt}.tmp"));
        }
        let mut temp = BackupTemp::new(temp_path.clone());
        let mut warnings = Vec::new();

        match self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        }) {
            Ok((busy, log, checkpointed)) => {
                if busy != 0 || checkpointed != log {
                    warnings.push(format!(
                        "WAL checkpoint 未完全完成（忙={busy}，日志 {log} 页，已写入 {checkpointed} 页）；备份仍通过 SQLite Backup API 复制当前视图"
                    ));
                }
            }
            Err(err) => warnings.push(format!(
                "WAL checkpoint 失败：{err}；备份仍通过 SQLite Backup API 复制当前视图"
            )),
        }

        {
            let mut target = Connection::open(&temp_path)
                .with_context(|| format!("创建备份临时文件 {}", temp_path.display()))?;
            target.busy_timeout(Duration::from_secs(5))?;
            {
                let backup = Backup::new(&self.conn, &mut target)
                    .with_context(|| format!("启动数据库备份到 {}", temp_path.display()))?;
                let mut busy_retries = 0u32;
                loop {
                    match backup.step(1024)? {
                        StepResult::Done => break,
                        StepResult::More => busy_retries = 0,
                        // Busy / Locked（以及未来新增状态）都按可重试处理。
                        _ => {
                            busy_retries += 1;
                            if busy_retries > 200 {
                                bail!("源数据库持续被占用，备份在 10 秒内未能完成");
                            }
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    }
                }
            }
            // 源库是 WAL 模式，复制出的文件头也带 WAL 标记；改成 DELETE 模式，
            // 让备份是单个自包含文件（仍包含全部已 checkpoint/未落盘内容）。
            let journal_mode: String = target
                .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
                .unwrap_or_else(|_| "wal".to_string());
            if !journal_mode.eq_ignore_ascii_case("delete") {
                warnings.push(format!(
                    "备份文件日志模式仍为 {journal_mode}；建议恢复前保持同目录文件完整"
                ));
            }
        }

        let (tables, integrity) = {
            let conn = Connection::open_with_flags(&temp_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .with_context(|| format!("打开备份文件校验 {}", temp_path.display()))?;
            let integrity: String =
                conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
            let mut tables = Vec::new();
            for name in BACKUP_KEY_TABLES {
                if !table_exists(&self.conn, name)? {
                    continue;
                }
                let source_rows = table_row_count(&self.conn, name)?;
                let backup_rows = if table_exists(&conn, name)? {
                    table_row_count(&conn, name)?
                } else {
                    warnings.push(format!("备份库缺少表 {name}"));
                    0
                };
                if source_rows != backup_rows {
                    warnings.push(format!(
                        "表 {name} 行数不一致：源 {source_rows}，备份 {backup_rows}"
                    ));
                }
                tables.push(TableCount {
                    table: (*name).to_string(),
                    rows: backup_rows,
                });
            }
            (tables, integrity)
        };
        if integrity != "ok" {
            bail!("备份文件完整性校验失败：{integrity}");
        }

        let bytes = fs::metadata(&temp_path)
            .with_context(|| format!("读取备份临时文件 {}", temp_path.display()))?
            .len();
        fs::rename(&temp_path, destination)
            .with_context(|| format!("保存备份文件到 {}", destination.display()))?;
        temp.keep();
        let source = self.database_path();
        log::info!(
            "database backup {} -> {} ({} bytes, {} ms)",
            source.display(),
            destination.display(),
            bytes,
            started.elapsed().as_millis()
        );
        Ok(BackupReport {
            source,
            destination: destination.to_path_buf(),
            bytes,
            elapsed_ms: started.elapsed().as_millis() as u64,
            tables,
            integrity_ok: true,
            warnings,
        })
    }

    /// 当前 schema 版本；恢复校验用于拒绝更新版本、提示旧版本迁移。
    pub fn schema_version() -> i64 {
        SCHEMA_VERSION
    }

    /// 只读检查一个数据库文件（备份或候选恢复库）。
    ///
    /// 只执行 `PRAGMA integrity_check`、读取 `user_version`，并统计关键表行数；
    /// 不打开写事务、不运行迁移，也不修改文件。调用方负责判断版本是否可接受。
    pub fn inspect_database(path: &Path) -> Result<BackupInspection> {
        if !path.is_file() {
            bail!("数据库文件不存在：{}", path.display());
        }
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("打开数据库 {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .with_context(|| format!("检查数据库完整性 {}", path.display()))?;
        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);
        let mut tables = Vec::new();
        for name in BACKUP_KEY_TABLES {
            if table_exists(&conn, name)? {
                tables.push(TableCount {
                    table: (*name).to_string(),
                    rows: table_row_count(&conn, name)?,
                });
            }
        }
        Ok(BackupInspection {
            path: path.to_path_buf(),
            integrity,
            user_version,
            tables,
        })
    }

    /// 当前连接主库的文件路径；内存库返回空路径。
    fn database_path(&self) -> PathBuf {
        let mut statement = match self.conn.prepare("PRAGMA database_list") {
            Ok(statement) => statement,
            Err(_) => return PathBuf::new(),
        };
        let mut rows = match statement.query([]) {
            Ok(rows) => rows,
            Err(_) => return PathBuf::new(),
        };
        while let Ok(Some(row)) = rows.next() {
            let name: String = row.get(1).unwrap_or_default();
            let file: String = row.get(2).unwrap_or_default();
            if name == "main" {
                return PathBuf::from(file);
            }
        }
        PathBuf::new()
    }

    pub fn load_folder(&self, folder: &str) -> Result<Vec<Asset>> {
        self.load_library(&[folder.to_string()])
    }

    pub fn asset_paths(&self, folder: &str) -> Result<Vec<PathBuf>> {
        let mut statement = self
            .conn
            .prepare("SELECT path FROM assets WHERE folder = ?1")?;
        let rows = statement.query_map([folder], |row| row.get::<_, String>(0))?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    pub fn asset_paths_by_sha1(&self, folder: &str, sha1: &str) -> Result<Vec<PathBuf>> {
        let mut statement = self
            .conn
            .prepare("SELECT path FROM assets WHERE folder = ?1 AND sha1 = ?2")?;
        let rows = statement.query_map(params![folder, sha1], |row| row.get::<_, String>(0))?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    pub fn all_thumb_paths(&self) -> Result<HashSet<PathBuf>> {
        let mut statement = self
            .conn
            .prepare("SELECT thumb FROM assets WHERE thumb <> ''")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    pub fn load_library(&self, folders: &[String]) -> Result<Vec<Asset>> {
        if folders.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; folders.len()].join(", ");
        let sql = format!(
            "SELECT path, thumb, width, height, group_name, rating, tags_json, tagged, error,
                    sha1, blake3, phash, tag_attempts, bytes, mtime, favorite, star, folder,
                    damage, captured_at, metadata_json
             FROM assets WHERE folder IN ({placeholders})"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(folders.iter()), row_to_asset)?;
        let mut assets = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut manual = self.manual_tags_for_folders(folders)?;
        for asset in &mut assets {
            if let Some(tags) = manual.remove(&asset.path) {
                asset.manual_tags = tags;
            }
        }
        assets.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(assets)
    }

    pub fn stored_meta(&self, path: &Path) -> Result<Option<StoredMeta>> {
        let path = path.to_string_lossy().to_string();
        self.conn
            .query_row(
                "SELECT mtime, bytes, sha1, blake3, phash, width, height, thumb,
                        captured_at, metadata_json, exif_state, media_kind
                 FROM assets WHERE path = ?1",
                [&path],
                |row| {
                    let phash: Option<i64> = row.get(4)?;
                    Ok(StoredMeta {
                        mtime: row.get::<_, i64>(0)?.max(0) as u64,
                        bytes: row.get::<_, i64>(1)?.max(0) as u64,
                        sha1: row.get(2)?,
                        blake3: row.get(3)?,
                        phash: phash.map(|value| value as u64),
                        width: row.get::<_, i64>(5)?.max(0) as u32,
                        height: row.get::<_, i64>(6)?.max(0) as u32,
                        thumb: row.get(7)?,
                        captured_at: row
                            .get::<_, Option<i64>>(8)?
                            .map(|value| value.max(0) as u64),
                        metadata_json: row.get(9)?,
                        exif_state: row.get(10)?,
                        media_kind: MediaKind::from_stored(&row.get::<_, String>(11)?),
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn upsert_meta(
        &self,
        folder: &str,
        path: &Path,
        thumb: Option<&Path>,
        width: u32,
        height: u32,
        bytes: u64,
        mtime: u64,
        hashes: &ContentHashes,
        reset_tag_attempts: bool,
    ) -> Result<Asset> {
        self.invalidate_name_fold();
        let path_string = path.to_string_lossy().to_string();
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| path_string.clone());
        self.remember_name_fold(&name);
        let thumb_string = thumb
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        let source_id = self.source_id(folder)?;
        let valid_hashes = !hashes.sha1.is_empty() && !hashes.blake3.is_empty();
        // F-040：media_kind/mime 由扩展名决定，图片路径的落库值与旧库一致（默认 'image'）。
        self.conn.execute(
            "INSERT INTO assets(
                source_id, path, folder, name, media_kind, mime, width, height, bytes, mtime,
                thumb, sha1, blake3, phash, damage
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(source_id, path) DO UPDATE SET
                folder = excluded.folder,
                name = excluded.name,
                media_kind = excluded.media_kind,
                mime = excluded.mime,
                width = excluded.width,
                height = excluded.height,
                bytes = excluded.bytes,
                mtime = excluded.mtime,
                thumb = excluded.thumb,
                sha1 = excluded.sha1,
                blake3 = excluded.blake3,
                phash = excluded.phash,
                damage = excluded.damage,
                tag_attempts = CASE WHEN ?16 THEN 0 ELSE tag_attempts END",
            params![
                source_id,
                path_string,
                folder,
                name,
                media_kind(path).as_str(),
                mime_for(path),
                width,
                height,
                bytes as i64,
                mtime as i64,
                thumb_string,
                valid_hashes.then_some(hashes.sha1.as_str()),
                valid_hashes.then_some(hashes.blake3.as_str()),
                (valid_hashes && hashes.decode_ok).then_some(hashes.phash as i64),
                hashes.decode_error.as_deref(),
                reset_tag_attempts,
            ],
        )?;
        self.load_asset(path)
    }

    pub fn update_tags(&self, asset: &Asset) -> Result<()> {
        let path = asset.path.to_string_lossy().to_string();
        let tags_json = serde_json::to_string(&asset.tags)?;
        let tx = self.conn.unchecked_transaction()?;
        let asset_id: Option<i64> = tx
            .query_row("SELECT id FROM assets WHERE path = ?1", [&path], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(asset_id) = asset_id else {
            bail!("asset not found: {path}");
        };
        tx.execute(
            "UPDATE assets SET group_name = ?1, rating = ?2, tags_json = ?3,
                tagged = 1, tag_state = 'done', tag_attempts = 0, error = NULL,
                damage = ?5 WHERE id = ?4",
            params![asset.group, asset.rating, tags_json, asset_id, asset.damage],
        )?;
        tx.execute(
            "DELETE FROM asset_tags WHERE asset_id = ?1 AND origin = 'auto'",
            [asset_id],
        )?;
        for tag in &asset.tags {
            let tag_id = upsert_tag(&tx, tag)?;
            tx.execute(
                "INSERT OR IGNORE INTO asset_tags(asset_id, tag_id, confidence, origin)
                 VALUES(?1, ?2, ?3, 'auto')",
                params![asset_id, tag_id, tag.score as f64],
            )?;
        }
        refresh_tags_text(&tx, asset_id)?;
        tx.commit()?;
        Ok(())
    }

    /// 保存 sidecar 结果：候选落 `asset_scores`，按当前阈值重算自动标签。
    /// `candidates == None` 表示旧 sidecar，退回原有 `tags` 行为并清空候选。
    pub fn store_tag_result(&self, asset: &Asset, candidates: Option<&[TagHit]>) -> Result<Asset> {
        let Some(candidates) = candidates else {
            self.update_tags(asset)?;
            let path = asset.path.to_string_lossy().to_string();
            self.conn.execute(
                "DELETE FROM asset_scores WHERE asset_id = (SELECT id FROM assets WHERE path = ?1)",
                [&path],
            )?;
            return self.load_asset(&asset.path);
        };
        let path = asset.path.to_string_lossy().to_string();
        let thresholds = self.tag_thresholds()?;
        let tx = self.conn.unchecked_transaction()?;
        let asset_id: Option<i64> = tx
            .query_row("SELECT id FROM assets WHERE path = ?1", [&path], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(asset_id) = asset_id else {
            bail!("asset not found: {path}");
        };
        let states = candidate_states(&tx, asset_id)?;
        let stored = save_candidates(&tx, asset_id, candidates, &states)?;
        let auto = auto_candidates(&stored, &states, &thresholds);
        let (rating, group) = derive_rating_group(&auto, &thresholds);
        write_auto_tags(
            &tx,
            asset_id,
            &auto,
            &rating,
            &group,
            true,
            asset.damage.as_deref(),
        )?;
        tx.commit()?;
        self.load_asset(&asset.path)
    }

    pub fn tag_thresholds(&self) -> Result<TagThresholds> {
        let mut categories = default_category_thresholds();
        if let Some(raw) = self.setting(TAG_THRESHOLDS_KEY)? {
            if let Ok(stored) = serde_json::from_str::<HashMap<String, f32>>(&raw) {
                for (category, value) in stored {
                    let category = category.to_ascii_lowercase();
                    if let Some(slot) = categories.get_mut(&category)
                        && value.is_finite()
                    {
                        *slot = value.clamp(0.0, 1.0);
                    }
                }
            }
        }
        let mut overrides = HashMap::new();
        {
            let mut statement = self.conn.prepare(
                "SELECT name, threshold_override FROM tags
                 WHERE threshold_override IS NOT NULL AND alias_of IS NULL",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })?;
            for row in rows {
                let (name, value) = row?;
                if value.is_finite() {
                    overrides.insert(name, value.clamp(0.0, 1.0) as f32);
                }
            }
        }
        {
            let mut statement = self.conn.prepare(
                "SELECT alias.name, canonical.threshold_override
                 FROM tags alias
                 JOIN tags canonical ON canonical.id = alias.alias_of
                 WHERE canonical.threshold_override IS NOT NULL",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })?;
            for row in rows {
                let (name, value) = row?;
                if value.is_finite() {
                    overrides.insert(name, value.clamp(0.0, 1.0) as f32);
                }
            }
        }
        Ok(TagThresholds {
            categories,
            overrides,
        })
    }

    /// 首次收到 sidecar ready 行时，把模型配置里的类别默认阈值落库；已有设置不覆盖。
    pub fn seed_category_thresholds(&self, values: &HashMap<String, f32>) -> Result<()> {
        if values.is_empty() || self.setting(TAG_THRESHOLDS_KEY)?.is_some() {
            return Ok(());
        }
        let mut categories = default_category_thresholds();
        for (category, value) in values {
            let category = category.to_ascii_lowercase();
            if let Some(slot) = categories.get_mut(&category)
                && value.is_finite()
            {
                *slot = value.clamp(0.0, 1.0);
            }
        }
        self.set_setting(TAG_THRESHOLDS_KEY, &serde_json::to_string(&categories)?)
    }

    /// 类别阈值越界会 clamp 到 [0,1]；非有限值报错。
    pub fn set_category_threshold(&self, category: &str, value: f32) -> Result<f32> {
        let category = category.trim().to_ascii_lowercase();
        if !TAG_CATEGORIES.contains(&category.as_str()) {
            bail!("未知标签类别: {category}");
        }
        let value = threshold_value(value)?;
        let mut categories = self.tag_thresholds()?.categories;
        categories.insert(category, value);
        self.set_setting(TAG_THRESHOLDS_KEY, &serde_json::to_string(&categories)?)?;
        Ok(value)
    }

    /// 单标签覆盖；`None` 清除。别名写到规范标签上，未使用的标签也可持久化。
    pub fn set_tag_threshold(&self, tag: &str, value: Option<f32>) -> Result<Option<f32>> {
        let tag = normalize_tag_name(tag);
        if tag.is_empty() {
            bail!("标签名不能为空");
        }
        let value = value.map(threshold_value).transpose()?;
        let existing: Option<(i64, Option<i64>)> = self
            .conn
            .query_row(
                "SELECT id, alias_of FROM tags WHERE name = ?1",
                [&tag],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let tag_id = match existing {
            Some((id, alias_of)) => alias_of.unwrap_or(id),
            None => {
                if value.is_none() {
                    return Ok(None);
                }
                self.conn.execute(
                    "INSERT INTO tags(name, category) VALUES(?1, '') ON CONFLICT(name) DO NOTHING",
                    [&tag],
                )?;
                self.conn.query_row(
                    "SELECT COALESCE(alias_of, id) FROM tags WHERE name = ?1",
                    [&tag],
                    |row| row.get(0),
                )?
            }
        };
        self.conn.execute(
            "UPDATE tags SET threshold_override = ?1 WHERE id = ?2",
            params![value.map(|value| value as f64), tag_id],
        )?;
        Ok(value)
    }

    pub fn allow_cpu_tagging(&self) -> bool {
        self.setting(ALLOW_CPU_TAGGING_KEY)
            .ok()
            .flatten()
            .as_deref()
            == Some("true")
    }

    pub fn set_allow_cpu_tagging(&self, allow: bool) -> Result<()> {
        self.set_setting(ALLOW_CPU_TAGGING_KEY, if allow { "true" } else { "false" })
    }

    /// 只用已存候选重算自动标签；不调用模型，manual 标签与候选行保持不变。
    pub fn recompute_thresholds(&self, paths: &[PathBuf]) -> Result<TagRecomputeSummary> {
        let thresholds = self.tag_thresholds()?;
        let tx = self.conn.unchecked_transaction()?;
        let mut summary = TagRecomputeSummary::default();
        let mut seen = HashSet::new();
        for path in paths {
            let path_string = path.to_string_lossy().to_string();
            if !seen.insert(path_string.clone()) {
                continue;
            }
            let asset_id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM assets WHERE path = ?1",
                    [&path_string],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(asset_id) = asset_id else {
                summary.skipped += 1;
                continue;
            };
            let (stored, states) = read_candidates(&tx, asset_id)?;
            if stored.is_empty() {
                summary.skipped += 1;
                continue;
            }
            let auto = auto_candidates(&stored, &states, &thresholds);
            let (rating, group) = derive_rating_group(&auto, &thresholds);
            write_auto_tags(&tx, asset_id, &auto, &rating, &group, false, None)?;
            summary.updated += 1;
        }
        tx.commit()?;
        Ok(summary)
    }

    /// 低置信度审核：列出阈值下的待审候选。
    pub fn pending_tag_candidates(
        &self,
        folders: &[String],
        limit: usize,
    ) -> Result<Vec<TagCandidateReview>> {
        if folders.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let thresholds = self.tag_thresholds()?;
        let placeholders = vec!["?"; folders.len()].join(", ");
        let base = folders.len();
        let cases = TAG_CATEGORIES
            .iter()
            .enumerate()
            .map(|(index, category)| format!("WHEN '{category}' THEN ?{}", base + index + 1))
            .collect::<Vec<_>>()
            .join(" ");
        let limit_slot = base + TAG_CATEGORIES.len() + 1;
        // 在 SQL 里应用有效阈值（单标签覆盖优先，其次类别默认），LIMIT 才能直接命中待审行。
        let sql = format!(
            "SELECT a.path, s.tag, s.category, s.score, s.vocab_index, s.review_state,
                    s.reviewed_at
             FROM asset_scores s
             JOIN assets a ON a.id = s.asset_id
             WHERE a.folder IN ({placeholders}) AND s.review_state = 'pending'
               AND s.score < COALESCE(
                    (SELECT COALESCE(t.threshold_override, c.threshold_override)
                     FROM tags t
                     LEFT JOIN tags c ON c.id = t.alias_of
                     WHERE t.name = s.tag
                       AND COALESCE(t.threshold_override, c.threshold_override) IS NOT NULL
                     LIMIT 1),
                    CASE s.category {cases} ELSE 0.2 END)
             ORDER BY a.path, s.score DESC
             LIMIT ?{limit_slot}"
        );
        let threshold_values = TAG_CATEGORIES
            .iter()
            .map(|category| thresholds.threshold(category, "") as f64)
            .collect::<Vec<_>>();
        let limit_value = limit as i64;
        let mut values: Vec<&dyn ToSql> = Vec::with_capacity(folders.len() + 7);
        for folder in folders {
            values.push(folder);
        }
        for value in &threshold_values {
            values.push(value);
        }
        values.push(&limit_value);
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (path, tag, category, score, vocab_index, state, reviewed_at) = row?;
            let score = score.clamp(0.0, 1.0) as f32;
            let threshold = thresholds.threshold(&category, &tag);
            if score >= threshold {
                continue;
            }
            out.push(TagCandidateReview {
                path: PathBuf::from(path),
                tag,
                category,
                score,
                vocab_index: vocab_index.map(|value| value.max(0) as u32),
                threshold,
                state: TagReviewState::from_db(&state),
                reviewed_at: reviewed_at.map(|value| value.max(0) as u64),
            });
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// 检查器用：某个资产的全部候选（含已接受/已忽略）。
    pub fn asset_candidates(&self, path: &Path) -> Result<Vec<TagCandidateReview>> {
        let thresholds = self.tag_thresholds()?;
        let mut statement = self.conn.prepare(
            "SELECT a.path, s.tag, s.category, s.score, s.vocab_index, s.review_state,
                    s.reviewed_at
             FROM asset_scores s
             JOIN assets a ON a.id = s.asset_id
             WHERE a.path = ?1
             ORDER BY s.score DESC, s.tag",
        )?;
        let rows = statement.query_map([path.to_string_lossy().to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (path, tag, category, score, vocab_index, state, reviewed_at) = row?;
            let score = score.clamp(0.0, 1.0) as f32;
            out.push(TagCandidateReview {
                path: PathBuf::from(path),
                threshold: thresholds.threshold(&category, &tag),
                tag,
                category,
                score,
                vocab_index: vocab_index.map(|value| value.max(0) as u32),
                state: TagReviewState::from_db(&state),
                reviewed_at: reviewed_at.map(|value| value.max(0) as u64),
            });
        }
        Ok(out)
    }

    /// 接受/忽略单条候选；接受会把低于阈值的候选加入自动标签。
    pub fn review_tag_candidate(
        &self,
        path: &Path,
        tag: &str,
        state: TagReviewState,
    ) -> Result<()> {
        let thresholds = self.tag_thresholds()?;
        let tx = self.conn.unchecked_transaction()?;
        let asset_id: Option<i64> = tx
            .query_row(
                "SELECT id FROM assets WHERE path = ?1",
                [path.to_string_lossy().to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(asset_id) = asset_id else {
            bail!("asset not found: {}", path.display());
        };
        let changed = tx.execute(
            "UPDATE asset_scores
             SET review_state = ?1, reviewed_at = unixepoch()
             WHERE asset_id = ?2 AND tag = ?3",
            params![state.as_str(), asset_id, tag],
        )?;
        if changed == 0 {
            bail!("候选不存在: {tag}");
        }
        recompute_asset_tags(&tx, asset_id, &thresholds)?;
        tx.commit()?;
        Ok(())
    }

    /// 批量接受/忽略某个资产的全部候选；返回更新行数。
    pub fn review_asset_candidates(&self, path: &Path, state: TagReviewState) -> Result<usize> {
        let thresholds = self.tag_thresholds()?;
        let tx = self.conn.unchecked_transaction()?;
        let asset_id: Option<i64> = tx
            .query_row(
                "SELECT id FROM assets WHERE path = ?1",
                [path.to_string_lossy().to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(asset_id) = asset_id else {
            bail!("asset not found: {}", path.display());
        };
        let changed = tx.execute(
            "UPDATE asset_scores
             SET review_state = ?1, reviewed_at = unixepoch()
             WHERE asset_id = ?2",
            params![state.as_str(), asset_id],
        )?;
        recompute_asset_tags(&tx, asset_id, &thresholds)?;
        tx.commit()?;
        Ok(changed)
    }

    /// 选择性重打标：有候选时只按阈值重算；缺候选才复用 jobs 队列调用模型。
    pub fn retag_paths(&self, paths: &[PathBuf]) -> Result<RetagSummary> {
        let thresholds = self.tag_thresholds()?;
        let tx = self.conn.unchecked_transaction()?;
        let mut summary = RetagSummary::default();
        let mut seen = HashSet::new();
        for path in paths {
            let path_string = path.to_string_lossy().to_string();
            if !seen.insert(path_string.clone()) {
                continue;
            }
            let asset_id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM assets WHERE path = ?1",
                    [&path_string],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(asset_id) = asset_id else {
                summary.missing += 1;
                continue;
            };
            let (stored, states) = read_candidates(&tx, asset_id)?;
            if !stored.is_empty() {
                let auto = auto_candidates(&stored, &states, &thresholds);
                let (rating, group) = derive_rating_group(&auto, &thresholds);
                write_auto_tags(&tx, asset_id, &auto, &rating, &group, false, None)?;
                summary.recomputed += 1;
            } else {
                tx.execute(
                    "UPDATE assets
                     SET tagged = 0, tag_state = 'pending', tag_attempts = 0, error = NULL
                     WHERE id = ?1",
                    [asset_id],
                )?;
                tx.execute(
                    "INSERT INTO jobs(kind, asset_id, priority, state, attempts, last_error, run_after)
                     VALUES('tag', ?1, 0, 'pending', 0, NULL, 0)
                     ON CONFLICT(kind, asset_id) DO UPDATE SET
                        state = 'pending', attempts = 0, last_error = NULL, run_after = 0",
                    [asset_id],
                )?;
                summary.queued += 1;
            }
        }
        tx.commit()?;
        Ok(summary)
    }

    /// 按选中/当前视图提升待处理（含失败待重试）任务的优先级。
    pub fn prioritize_tag_jobs(&self, paths: &[PathBuf], priority: i64) -> Result<usize> {
        let priority = priority.clamp(0, 1000);
        let tx = self.conn.unchecked_transaction()?;
        let mut updated = 0;
        for path in paths {
            updated += tx.execute(
                "UPDATE jobs
                 SET priority = MAX(priority, ?1)
                 WHERE kind = 'tag' AND state IN ('pending', 'error')
                   AND asset_id = (SELECT id FROM assets WHERE path = ?2)",
                params![priority, path.to_string_lossy().to_string()],
            )?;
        }
        tx.commit()?;
        Ok(updated)
    }

    pub fn tag_job_errors(&self, folders: &[String], limit: usize) -> Result<Vec<TagJobError>> {
        if folders.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; folders.len()].join(", ");
        let sql = format!(
            "SELECT j.id, a.path, j.attempts, j.last_error, j.priority
             FROM jobs j
             JOIN assets a ON a.id = j.asset_id
             WHERE j.kind = 'tag' AND j.state = 'error' AND a.folder IN ({placeholders})
             ORDER BY j.priority DESC, j.id
             LIMIT ?"
        );
        let limit = limit.to_string();
        let mut values = folders.iter().map(String::as_str).collect::<Vec<_>>();
        values.push(limit.as_str());
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
            Ok(TagJobError {
                job_id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                attempts: row.get::<_, i64>(2)?.max(0) as u32,
                last_error: row.get(3)?,
                priority: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn tag_queue_totals(&self, folders: &[String]) -> Result<TagQueueTotals> {
        if folders.is_empty() {
            return Ok(TagQueueTotals::default());
        }
        let placeholders = vec!["?"; folders.len()].join(", ");
        let sql = format!(
            "SELECT j.state, COUNT(*)
             FROM jobs j
             JOIN assets a ON a.id = j.asset_id
             WHERE j.kind = 'tag' AND a.folder IN ({placeholders})
             GROUP BY j.state"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(folders.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as usize,
            ))
        })?;
        let mut totals = TagQueueTotals::default();
        for row in rows {
            let (state, count) = row?;
            totals.total += count;
            match state.as_str() {
                "pending" => totals.pending = count,
                "running" => totals.running = count,
                "error" => totals.error = count,
                "done" => totals.done = count,
                _ => {}
            }
        }
        Ok(totals)
    }

    pub fn set_favorite(&self, paths: &[PathBuf], favorite: bool) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let mut statement = tx.prepare("UPDATE assets SET favorite = ?1 WHERE path = ?2")?;
        let mut updated = 0;
        for path in paths {
            updated += statement.execute(params![favorite as i64, path.to_string_lossy()])?;
        }
        drop(statement);
        tx.commit()?;
        Ok(updated)
    }

    pub fn set_star(&self, paths: &[PathBuf], star: u8) -> Result<usize> {
        let star = star.min(5) as i64;
        let tx = self.conn.unchecked_transaction()?;
        let mut statement = tx.prepare("UPDATE assets SET star = ?1 WHERE path = ?2")?;
        let mut updated = 0;
        for path in paths {
            updated += statement.execute(params![star, path.to_string_lossy()])?;
        }
        drop(statement);
        tx.commit()?;
        Ok(updated)
    }

    pub fn add_manual_tags(&self, paths: &[PathBuf], name: &str, category: &str) -> Result<usize> {
        let name = normalize_tag_name(name);
        if name.is_empty() {
            bail!("标签名不能为空");
        }
        let category = if category.trim().is_empty() {
            "manual"
        } else {
            category.trim()
        };
        let tx = self.conn.unchecked_transaction()?;
        let mut updated = 0;
        for path in paths {
            let asset_id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM assets WHERE path = ?1",
                    [path.to_string_lossy().to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(asset_id) = asset_id else {
                continue;
            };
            tx.execute(
                "INSERT INTO tags(name, category) VALUES(?1, ?2)
                 ON CONFLICT(name) DO NOTHING",
                params![name, category],
            )?;
            let tag_id: i64 = tx.query_row(
                "SELECT COALESCE(alias_of, id) FROM tags WHERE name = ?1",
                [name.as_str()],
                |row| row.get(0),
            )?;
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO asset_tags(asset_id, tag_id, confidence, origin)
                 VALUES(?1, ?2, 1.0, 'manual')",
                params![asset_id, tag_id],
            )?;
            if inserted > 0 {
                refresh_tags_text(&tx, asset_id)?;
                updated += 1;
            }
        }
        tx.commit()?;
        Ok(updated)
    }

    pub fn remove_tags(&self, paths: &[PathBuf], name: &str) -> Result<usize> {
        let name = normalize_tag_name(name);
        if name.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut updated = 0;
        for path in paths {
            let asset_id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM assets WHERE path = ?1",
                    [path.to_string_lossy().to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(asset_id) = asset_id else {
                continue;
            };
            let tag_id: Option<i64> = tx
                .query_row(
                    "SELECT COALESCE(alias_of, id) FROM tags WHERE name = ?1",
                    [name.as_str()],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(tag_id) = tag_id else {
                continue;
            };
            let removed = tx.execute(
                "DELETE FROM asset_tags WHERE asset_id = ?1 AND tag_id = ?2",
                params![asset_id, tag_id],
            )?;
            if removed > 0 {
                refresh_auto_tags_json(&tx, asset_id)?;
                refresh_tags_text(&tx, asset_id)?;
                updated += 1;
            }
        }
        tx.commit()?;
        Ok(updated)
    }

    pub fn list_tags(&self, folder: &str) -> Result<Vec<TagSummary>> {
        self.list_tags_in(&[folder.to_string()])
    }

    /// 全词表（含尚未使用的标签）；类别来自模型词表或首次使用时的打标结果。
    pub fn list_tag_names(&self) -> Result<Vec<(String, String)>> {
        let mut statement = self
            .conn
            .prepare("SELECT name, category FROM tags WHERE alias_of IS NULL ORDER BY name")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn import_vocabulary(&self, entries: &[(String, String, u32)]) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut inserted = 0;
        {
            let mut statement = tx.prepare(
                "INSERT OR IGNORE INTO tags(name, category, vocab_index) VALUES(?1, ?2, ?3)",
            )?;
            for (name, category, index) in entries {
                inserted += statement.execute(params![name, category, *index as i64])?;
            }
        }
        tx.commit()?;
        Ok(inserted)
    }

    pub fn list_tags_in(&self, folders: &[String]) -> Result<Vec<TagSummary>> {
        if folders.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; folders.len()].join(", ");
        let sql = format!(
            "SELECT t.name, t.category, COUNT(DISTINCT a.id) AS uses,
                    COUNT(DISTINCT CASE WHEN at.origin = 'manual' THEN a.id END) AS manual
             FROM tags t
             JOIN asset_tags at ON at.tag_id = t.id
             JOIN assets a ON a.id = at.asset_id
             WHERE a.folder IN ({placeholders})
             GROUP BY t.id
             ORDER BY uses DESC, t.name"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(folders.iter()), |row| {
            Ok(TagSummary {
                name: row.get(0)?,
                category: row.get(1)?,
                count: row.get::<_, i64>(2)?.max(0) as usize,
                manual_count: row.get::<_, i64>(3)?.max(0) as usize,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn load_tag_aliases(&self) -> Result<HashMap<String, String>> {
        let mut statement = self.conn.prepare(
            "SELECT alias.name, canonical.name
             FROM tags alias
             JOIN tags canonical ON canonical.id = alias.alias_of
             ORDER BY alias.name",
        )?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }

    /// 重命名标签；旧名保留为别名，已打标资产与 FTS 一起更新。
    pub fn rename_tag(&self, old: &str, new: &str) -> Result<usize> {
        let old = normalize_tag_name(old);
        let new = normalize_tag_name(new);
        if old.is_empty() || new.is_empty() {
            bail!("标签名不能为空");
        }
        if old == new {
            return Ok(0);
        }
        let source: Option<(i64, Option<i64>, String)> = self
            .conn
            .query_row(
                "SELECT id, alias_of, category FROM tags WHERE name = ?1",
                [&old],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((source_id, alias_of, category)) = source else {
            bail!("标签不存在: {old}");
        };
        if alias_of.is_some() {
            bail!("「{old}」是别名，请对原标签操作");
        }
        let target: Option<i64> = self
            .conn
            .query_row(
                "SELECT COALESCE(alias_of, id) FROM tags WHERE name = ?1",
                [&new],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(target_id) = target
            && target_id != source_id
        {
            return self.merge_tags(&old, &new);
        }
        let affected = tag_asset_ids(&self.conn, source_id)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM tags WHERE name = ?1 AND alias_of = ?2 AND id <> ?2",
            params![new, source_id],
        )?;
        tx.execute(
            "UPDATE tags SET name = ?1, alias_of = NULL WHERE id = ?2",
            params![new, source_id],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO tags(name, category, alias_of) VALUES(?1, ?2, ?3)",
            params![old, category, source_id],
        )?;
        for asset_id in &affected {
            refresh_auto_tags_json(&tx, *asset_id)?;
            refresh_tags_text(&tx, *asset_id)?;
        }
        tx.commit()?;
        Ok(affected.len())
    }

    /// 合并两个标签；来源名保留为别名，来源的手工/自动标记并入目标。
    pub fn merge_tags(&self, source: &str, target: &str) -> Result<usize> {
        let source = normalize_tag_name(source);
        let target = normalize_tag_name(target);
        if source.is_empty() || target.is_empty() {
            bail!("标签名不能为空");
        }
        if source == target {
            return Ok(0);
        }
        let source_row: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT COALESCE(alias_of, id), category FROM tags WHERE name = ?1",
                [&source],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((source_id, source_category)) = source_row else {
            bail!("标签不存在: {source}");
        };
        let target_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT COALESCE(alias_of, id) FROM tags WHERE name = ?1",
                [&target],
                |row| row.get(0),
            )
            .optional()?;
        if target_id == Some(source_id) {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        let target_id = match target_id {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO tags(name, category) VALUES(?1, ?2)",
                    params![target, source_category],
                )?;
                tx.last_insert_rowid()
            }
        };
        let affected = tag_asset_ids(&tx, source_id)?;
        tx.execute(
            "INSERT INTO asset_tags(asset_id, tag_id, confidence, origin)
             SELECT asset_id, ?1, confidence, origin FROM asset_tags WHERE tag_id = ?2
             ON CONFLICT(asset_id, tag_id, origin) DO UPDATE SET
                confidence = MAX(confidence, excluded.confidence)",
            params![target_id, source_id],
        )?;
        tx.execute("DELETE FROM asset_tags WHERE tag_id = ?1", [source_id])?;
        tx.execute("UPDATE tags SET alias_of = NULL WHERE id = ?1", [target_id])?;
        // 别名保持一层：之前指向来源的旧名直接改指目标，避免链式查找。
        tx.execute(
            "UPDATE tags SET alias_of = ?1 WHERE alias_of = ?2",
            [target_id, source_id],
        )?;
        tx.execute(
            "UPDATE tags SET alias_of = ?1 WHERE id = ?2",
            [target_id, source_id],
        )?;
        for asset_id in &affected {
            refresh_auto_tags_json(&tx, *asset_id)?;
            refresh_tags_text(&tx, *asset_id)?;
        }
        tx.commit()?;
        Ok(affected.len())
    }

    /// 智能集合列表：置顶优先，其次手动顺序，最后 id（旧行在迁移时按 id 回填顺序）。
    pub fn load_saved_searches(&self) -> Result<Vec<SavedSearch>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {SAVED_SEARCH_COLUMNS} FROM saved_searches
             ORDER BY pinned DESC, position ASC, id ASC"
        ))?;
        let rows = statement.query_map([], row_to_saved_search)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn load_saved_search(&self, id: i64) -> Result<SavedSearch> {
        let saved = self
            .conn
            .query_row(
                &format!("SELECT {SAVED_SEARCH_COLUMNS} FROM saved_searches WHERE id = ?1"),
                [id],
                row_to_saved_search,
            )
            .optional()?;
        match saved {
            Some(saved) => Ok(saved),
            None => bail!("智能集合不存在: {id}"),
        }
    }

    /// 兼容旧调用：不记录视图状态，按默认视图创建；返回新集合 id。
    pub fn save_saved_search(&self, name: &str, filter: &Filter, query: &str) -> Result<i64> {
        Ok(self
            .create_saved_search(name, filter, query, &SavedSearchView::default())?
            .id)
    }

    /// 创建集合并记录当前视图；新集合排在手动顺序末尾（置顶仍优先）。
    /// 同名返回 `DuplicateNameError`，不覆盖已有集合。
    pub fn create_saved_search(
        &self,
        name: &str,
        filter: &Filter,
        query: &str,
        view: &SavedSearchView,
    ) -> Result<SavedSearch> {
        let name = name.trim();
        if name.is_empty() {
            bail!("智能集合名称不能为空");
        }
        self.ensure_saved_search_name_available(name, None)?;
        let payload = SavedSearchPayload {
            filter: filter.clone(),
            query: query.to_string(),
        };
        let position: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM saved_searches",
            [],
            |row| row.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO saved_searches(
                name, query_json, sort_key, sort_ascending, view_mode, pane_mode,
                show_sidebar, show_extensions, pinned, position, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, unixepoch())",
            params![
                name,
                serde_json::to_string(&payload)?,
                view.sort_key,
                view.sort_ascending as i64,
                view.view_mode,
                view.pane_mode,
                view.show_sidebar as i64,
                view.show_extensions as i64,
                position
            ],
        )?;
        self.load_saved_search(self.conn.last_insert_rowid())
    }

    /// 重命名不改 id、不改手动顺序；同名（大小写敏感）返回可判定错误。
    pub fn rename_saved_search(&self, id: i64, name: &str) -> Result<SavedSearch> {
        let name = name.trim();
        if name.is_empty() {
            bail!("智能集合名称不能为空");
        }
        self.load_saved_search(id)?;
        self.ensure_saved_search_name_available(name, Some(id))?;
        self.conn.execute(
            "UPDATE saved_searches SET name = ?1, updated_at = unixepoch() WHERE id = ?2",
            params![name, id],
        )?;
        self.load_saved_search(id)
    }

    /// 用当前筛选/搜索/视图覆盖集合内容；id 与名称保持不变。
    pub fn update_saved_search(
        &self,
        id: i64,
        filter: &Filter,
        query: &str,
        view: &SavedSearchView,
    ) -> Result<SavedSearch> {
        self.load_saved_search(id)?;
        let payload = SavedSearchPayload {
            filter: filter.clone(),
            query: query.to_string(),
        };
        self.conn.execute(
            "UPDATE saved_searches
             SET query_json = ?1, sort_key = ?2, sort_ascending = ?3, view_mode = ?4,
                 pane_mode = ?5, show_sidebar = ?6, show_extensions = ?7,
                 updated_at = unixepoch()
             WHERE id = ?8",
            params![
                serde_json::to_string(&payload)?,
                view.sort_key,
                view.sort_ascending as i64,
                view.view_mode,
                view.pane_mode,
                view.show_sidebar as i64,
                view.show_extensions as i64,
                id
            ],
        )?;
        self.load_saved_search(id)
    }

    /// 置顶只改变分组（置顶在前），不改变同组内的手动顺序。
    pub fn set_saved_search_pinned(&self, id: i64, pinned: bool) -> Result<SavedSearch> {
        let updated = self.conn.execute(
            "UPDATE saved_searches SET pinned = ?1 WHERE id = ?2",
            params![pinned as i64, id],
        )?;
        if updated == 0 {
            bail!("智能集合不存在: {id}");
        }
        self.load_saved_search(id)
    }

    /// 按给定 id 顺序重写手动顺序；必须覆盖全部集合，避免留下未定义的位置。
    pub fn reorder_saved_searches(&self, ordered_ids: &[i64]) -> Result<usize> {
        let existing = self.load_saved_searches()?;
        let known = existing
            .iter()
            .map(|search| search.id)
            .collect::<HashSet<_>>();
        let requested = ordered_ids.iter().copied().collect::<HashSet<_>>();
        if existing.len() != ordered_ids.len()
            || requested.len() != ordered_ids.len()
            || known != requested
        {
            bail!("排序列表必须包含全部智能集合");
        }
        let tx = self.conn.unchecked_transaction()?;
        for (index, id) in ordered_ids.iter().enumerate() {
            // 1 起始，与迁移回填的 `position = id` 以及新建集合的 `MAX(position)+1` 一致；
            // 0 只表示「迁移前的旧集合」，不会再被写入。
            tx.execute(
                "UPDATE saved_searches SET position = ?1 WHERE id = ?2",
                params![index as i64 + 1, id],
            )?;
        }
        tx.commit()?;
        Ok(ordered_ids.len())
    }

    fn ensure_saved_search_name_available(
        &self,
        name: &str,
        exclude_id: Option<i64>,
    ) -> Result<()> {
        let taken: bool = self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM saved_searches
                WHERE name = ?1 AND (?2 IS NULL OR id <> ?2)
             )",
            params![name, exclude_id],
            |row| row.get(0),
        )?;
        if taken {
            return Err(anyhow::Error::new(DuplicateNameError::new(
                OrganizationKind::SavedSearch,
                name,
            )));
        }
        Ok(())
    }

    pub fn delete_saved_search(&self, id: i64) -> Result<usize> {
        Ok(self
            .conn
            .execute("DELETE FROM saved_searches WHERE id = ?1", [id])?)
    }

    /// 手动相册列表（含资产数），按名称排序；计数来自同一条 SQL，增删即时可见。
    pub fn load_collections(&self) -> Result<Vec<Collection>> {
        let mut statement = self.conn.prepare(
            "SELECT c.id, c.name, c.kind, COUNT(ca.asset_id)
             FROM collections c
             LEFT JOIN collection_assets ca ON ca.collection_id = c.id
             GROUP BY c.id, c.name, c.kind
             ORDER BY c.name COLLATE NOCASE, c.id",
        )?;
        let rows = statement.query_map([], |row| {
            let count: i64 = row.get(3)?;
            Ok(Collection {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                asset_count: count.max(0) as usize,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn load_collection(&self, id: i64) -> Result<Collection> {
        let collection = self
            .conn
            .query_row(
                "SELECT c.id, c.name, c.kind,
                        (SELECT COUNT(*) FROM collection_assets ca WHERE ca.collection_id = c.id)
                 FROM collections c WHERE c.id = ?1",
                [id],
                |row| {
                    let count: i64 = row.get(3)?;
                    Ok(Collection {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        kind: row.get(2)?,
                        asset_count: count.max(0) as usize,
                    })
                },
            )
            .optional()?;
        match collection {
            Some(collection) => Ok(collection),
            None => bail!("相册不存在: {id}"),
        }
    }

    /// 新建手动相册；同名返回 `DuplicateNameError`，不覆盖已有相册。
    pub fn create_collection(&self, name: &str) -> Result<Collection> {
        let name = name.trim();
        if name.is_empty() {
            bail!("相册名称不能为空");
        }
        self.ensure_collection_name_available(name, None)?;
        self.conn.execute(
            "INSERT INTO collections(name, kind) VALUES(?1, 'manual')",
            [name],
        )?;
        self.load_collection(self.conn.last_insert_rowid())
    }

    /// 重命名不改 id，也不改关系行。
    pub fn rename_collection(&self, id: i64, name: &str) -> Result<Collection> {
        let name = name.trim();
        if name.is_empty() {
            bail!("相册名称不能为空");
        }
        self.load_collection(id)?;
        self.ensure_collection_name_available(name, Some(id))?;
        self.conn.execute(
            "UPDATE collections SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
        self.load_collection(id)
    }

    /// 删除相册只删关系行，资产行与磁盘文件都不动；返回删除的相册数（0/1）。
    pub fn delete_collection(&self, id: i64) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM collection_assets WHERE collection_id = ?1",
            [id],
        )?;
        let removed = tx.execute("DELETE FROM collections WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(removed)
    }

    fn ensure_collection_name_available(&self, name: &str, exclude_id: Option<i64>) -> Result<()> {
        let taken: bool = self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM collections WHERE name = ?1 AND (?2 IS NULL OR id <> ?2)
             )",
            params![name, exclude_id],
            |row| row.get(0),
        )?;
        if taken {
            return Err(anyhow::Error::new(DuplicateNameError::new(
                OrganizationKind::Collection,
                name,
            )));
        }
        Ok(())
    }

    /// 批量加入相册；同一资产可属于多个相册，重复加入幂等（不计入返回值）。
    /// 路径不在图库中的项被静默跳过，返回值是实际新增的关系行数。
    pub fn add_assets_to_collection(&self, id: i64, paths: &[PathBuf]) -> Result<usize> {
        if paths.is_empty() {
            return Ok(0);
        }
        self.load_collection(id)?;
        let tx = self.conn.unchecked_transaction()?;
        let mut added = 0;
        for path in paths {
            added += tx.execute(
                "INSERT OR IGNORE INTO collection_assets(collection_id, asset_id)
                 SELECT ?1, id FROM assets WHERE path = ?2",
                params![id, path.to_string_lossy().to_string()],
            )?;
        }
        tx.commit()?;
        Ok(added)
    }

    /// 批量移出相册；不存在的资产或本就不在相册里的路径不计入返回值。
    pub fn remove_assets_from_collection(&self, id: i64, paths: &[PathBuf]) -> Result<usize> {
        if paths.is_empty() {
            return Ok(0);
        }
        self.load_collection(id)?;
        let tx = self.conn.unchecked_transaction()?;
        let mut removed = 0;
        for path in paths {
            removed += tx.execute(
                "DELETE FROM collection_assets
                 WHERE collection_id = ?1
                   AND asset_id IN (SELECT id FROM assets WHERE path = ?2)",
                params![id, path.to_string_lossy().to_string()],
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }

    /// 按相册计数；每次现查，加入/移出后立即反映。
    pub fn collection_asset_count(&self, id: i64) -> Result<usize> {
        Ok(self.load_collection(id)?.asset_count)
    }

    /// 按相册列举资产路径，按路径排序。
    pub fn collection_paths(&self, id: i64) -> Result<Vec<PathBuf>> {
        self.load_collection(id)?;
        let mut statement = self.conn.prepare(
            "SELECT a.path
             FROM collection_assets ca
             JOIN assets a ON a.id = ca.asset_id
             WHERE ca.collection_id = ?1
             ORDER BY a.path",
        )?;
        let rows = statement.query_map([id], |row| row.get::<_, String>(0))?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    /// 按相册列举资产（含手动标签），供检查器/临时视图使用。
    pub fn load_collection_assets(&self, id: i64) -> Result<Vec<Asset>> {
        self.load_collection(id)?;
        let mut statement = self.conn.prepare(
            "SELECT a.path, a.thumb, a.width, a.height, a.group_name, a.rating, a.tags_json,
                    a.tagged, a.error, a.sha1, a.blake3, a.phash, a.tag_attempts, a.bytes,
                    a.mtime, a.favorite, a.star, a.folder, a.damage, a.captured_at,
                    a.metadata_json
             FROM collection_assets ca
             JOIN assets a ON a.id = ca.asset_id
             WHERE ca.collection_id = ?1
             ORDER BY a.path",
        )?;
        let rows = statement.query_map([id], row_to_asset)?;
        let mut assets = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let folders = assets
            .iter()
            .map(|asset| asset.library_root.to_string_lossy().to_string())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut manual = self.manual_tags_for_folders(&folders)?;
        for asset in &mut assets {
            if let Some(tags) = manual.remove(&asset.path) {
                asset.manual_tags = tags;
            }
        }
        Ok(assets)
    }

    /// 某资产所属的相册（含各相册计数）；同一资产可属于多个相册。
    pub fn asset_collections(&self, path: &Path) -> Result<Vec<Collection>> {
        let mut statement = self.conn.prepare(
            "SELECT c.id, c.name, c.kind,
                    (SELECT COUNT(*) FROM collection_assets ca2 WHERE ca2.collection_id = c.id)
             FROM collections c
             JOIN collection_assets ca ON ca.collection_id = c.id
             JOIN assets a ON a.id = ca.asset_id
             WHERE a.path = ?1
             ORDER BY c.name COLLATE NOCASE, c.id",
        )?;
        let rows = statement.query_map([path.to_string_lossy().to_string()], |row| {
            let count: i64 = row.get(3)?;
            Ok(Collection {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                asset_count: count.max(0) as usize,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 把相册内的路径写成每行一条的 UTF-8 清单；只写路径，不复制任何文件。
    /// 目标已存在时拒绝覆盖（`create_new`），返回写入行数。
    pub fn export_collection_paths(&self, id: i64, destination: &Path) -> Result<usize> {
        let paths = self.collection_paths(id)?;
        let mut content = String::new();
        for path in &paths {
            content.push_str(&path.to_string_lossy());
            content.push('\n');
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .with_context(|| format!("创建相册清单 {}", destination.display()))?;
        file.write_all(content.as_bytes())?;
        Ok(paths.len())
    }

    /// 来源移除时清理该来源资产的相册关系行；资产行与磁盘文件都不动。
    /// 关系只跟随「当前图库成员」，重新加入来源后需要重新加入相册。
    pub fn forget_source_collections(&self, folder: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM collection_assets
             WHERE asset_id IN (SELECT id FROM assets WHERE folder = ?1)",
            [folder],
        )?)
    }

    pub fn retry_failed(&self, folder: &str) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let retried = tx.execute(
            "UPDATE assets
             SET tag_attempts = 0, tag_state = 'pending', error = NULL
             WHERE folder = ?1 AND tagged = 0 AND tag_state = 'error'",
            [folder],
        )?;
        tx.execute(
            "UPDATE jobs
             SET state = 'pending', attempts = 0, last_error = NULL, run_after = 0
             WHERE kind = 'tag' AND asset_id IN (
                SELECT id FROM assets WHERE folder = ?1 AND tagged = 0 AND tag_attempts = 0
             )",
            [folder],
        )?;
        tx.commit()?;
        Ok(retried)
    }

    pub fn recover_tag_jobs(&self) -> Result<usize> {
        self.conn
            .execute(
                "UPDATE jobs
                 SET state = 'pending',
                     last_error = COALESCE(last_error, 'interrupted')
                 WHERE kind = 'tag' AND state = 'running'",
                [],
            )
            .map_err(Into::into)
    }

    pub fn enqueue_tag_jobs(&self, folder: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE jobs SET state = 'done', last_error = NULL
             WHERE kind = 'tag' AND asset_id IN (
                SELECT id FROM assets WHERE folder = ?1 AND tagged = 1
             )",
            [folder],
        )?;
        tx.execute(
            "UPDATE jobs SET state = 'error',
                last_error = COALESCE(last_error, 'tag attempt limit')
             WHERE kind = 'tag' AND asset_id IN (
                SELECT id FROM assets
                WHERE folder = ?1 AND tagged = 0 AND tag_attempts >= 3
             )",
            [folder],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO jobs(kind, asset_id, priority, state, attempts, last_error, run_after)
             SELECT 'tag', id, 0, 'pending', 0, NULL, 0
             FROM assets
             WHERE folder = ?1 AND tagged = 0 AND tag_attempts < 3 AND deleted_at IS NULL
               AND media_kind = 'image'",
            [folder],
        )?;
        tx.execute(
            "UPDATE jobs
             SET state = 'pending', attempts = 0, last_error = NULL, run_after = 0
             WHERE kind = 'tag' AND asset_id IN (
                SELECT id FROM assets
                WHERE folder = ?1 AND tagged = 0 AND tag_attempts < 3 AND deleted_at IS NULL
                  AND media_kind = 'image'
             )",
            [folder],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn claim_tag_jobs(&self, folder: &str, limit: usize) -> Result<Vec<TagJob>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let tx = self.conn.unchecked_transaction()?;
        let jobs = {
            let mut statement = tx.prepare(
                "SELECT j.id, a.path
                 FROM jobs j
                 JOIN assets a ON a.id = j.asset_id
                 WHERE j.kind = 'tag' AND j.state = 'pending'
                   AND a.folder = ?1 AND a.tagged = 0 AND a.tag_attempts < 3
                 ORDER BY j.priority DESC, j.id
                 LIMIT ?2",
            )?;
            let rows = statement.query_map(params![folder, limit as i64], |row| {
                Ok(TagJob {
                    id: row.get(0)?,
                    path: PathBuf::from(row.get::<_, String>(1)?),
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for job in &jobs {
            tx.execute("UPDATE jobs SET state = 'running' WHERE id = ?1", [job.id])?;
        }
        tx.commit()?;
        Ok(jobs)
    }

    pub fn complete_tag_job(&self, job_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE jobs SET state = 'done', last_error = NULL WHERE id = ?1",
            [job_id],
        )?;
        Ok(())
    }

    pub fn fail_tag_job(&self, job_id: i64, error: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE jobs
             SET state = 'error', attempts = attempts + 1, last_error = ?1
             WHERE id = ?2",
            params![error, job_id],
        )?;
        Ok(())
    }

    pub fn release_tag_jobs(&self, job_ids: &[i64]) -> Result<()> {
        if job_ids.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        for job_id in job_ids {
            tx.execute(
                "UPDATE jobs SET state = 'pending' WHERE id = ?1 AND state = 'running'",
                [job_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn tag_job_counts(&self, folder: &str) -> Result<JobCounts> {
        let mut statement = self.conn.prepare(
            "SELECT j.state, COUNT(*)
             FROM jobs j
             JOIN assets a ON a.id = j.asset_id
             WHERE j.kind = 'tag' AND a.folder = ?1
             GROUP BY j.state",
        )?;
        let rows = statement.query_map([folder], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as usize,
            ))
        })?;
        let mut counts = JobCounts::default();
        for row in rows {
            let (state, count) = row?;
            match state.as_str() {
                "pending" => counts.pending = count,
                "running" => counts.running = count,
                "error" => counts.error = count,
                _ => {}
            }
        }
        Ok(counts)
    }

    pub fn load_duplicate_reviews(&self) -> Result<HashMap<String, DuplicateReview>> {
        let mut statement = self.conn.prepare(
            "SELECT group_key, state, keeper_path, member_count, signature
             FROM duplicate_reviews",
        )?;
        let rows = statement.query_map([], |row| {
            let keeper: Option<String> = row.get(2)?;
            Ok(DuplicateReview {
                group_key: row.get(0)?,
                state: ReviewState::from_db(&row.get::<_, String>(1)?),
                keeper_path: keeper.map(PathBuf::from),
                member_count: row.get::<_, i64>(3)?.max(0) as usize,
                signature: row.get(4)?,
            })
        })?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|review| (review.group_key.clone(), review))
            .collect())
    }

    pub fn save_duplicate_review(&self, review: &DuplicateReview) -> Result<()> {
        self.conn.execute(
            "INSERT INTO duplicate_reviews(group_key, state, keeper_path, member_count, signature)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(group_key) DO UPDATE SET
                state = excluded.state,
                keeper_path = excluded.keeper_path,
                member_count = excluded.member_count,
                signature = excluded.signature,
                reviewed_at = unixepoch()",
            params![
                review.group_key,
                review.state.as_str(),
                review
                    .keeper_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string()),
                review.member_count as i64,
                review.signature,
            ],
        )?;
        Ok(())
    }

    pub fn delete_assets(&self, paths: &[PathBuf]) -> Result<usize> {
        if paths.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut deleted = 0;
        for path in paths {
            let path = path.to_string_lossy().to_string();
            // 外键已有 ON DELETE CASCADE；这里再显式清一遍关系行，
            // 保证即使 pragma foreign_keys 被关闭也不会留下孤儿相册关系。
            tx.execute(
                "DELETE FROM collection_assets
                 WHERE asset_id IN (SELECT id FROM assets WHERE path = ?1)",
                [&path],
            )?;
            deleted += tx.execute("DELETE FROM assets WHERE path = ?1", [&path])?;
        }
        tx.commit()?;
        Ok(deleted)
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Into::into)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn mark_error(&self, path: &Path, error: &str, damage: Option<&str>) -> Result<u32> {
        let attempts: Option<i64> = self
            .conn
            .query_row(
                "UPDATE assets
                 SET tagged = 0, tag_state = 'error', error = ?1,
                     damage = COALESCE(?2, damage),
                     tag_attempts = tag_attempts + 1
                 WHERE path = ?3
                 RETURNING tag_attempts",
                params![error, damage, path.to_string_lossy().to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(attempts.unwrap_or(0).max(0) as u32)
    }

    pub fn update_media(
        &self,
        path: &Path,
        thumb: Option<&Path>,
        width: u32,
        height: u32,
        damage: Option<&str>,
    ) -> Result<Asset> {
        let thumb = thumb
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        self.conn.execute(
            "UPDATE assets
             SET thumb = ?1, width = ?2, height = ?3, damage = ?4,
                 thumb_state = 'done'
             WHERE path = ?5",
            params![
                thumb,
                width,
                height,
                damage,
                path.to_string_lossy().to_string()
            ],
        )?;
        self.load_asset(path)
    }

    pub fn set_exif(
        &self,
        path: &Path,
        captured_at: Option<u64>,
        metadata_json: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE assets SET captured_at = ?1, metadata_json = ?2, exif_state = 'done'
             WHERE path = ?3",
            params![
                captured_at.map(|value| value as i64),
                metadata_json,
                path.to_string_lossy().to_string()
            ],
        )?;
        Ok(())
    }

    pub fn record_thumb_source(
        &self,
        thumb: &Path,
        source: &Path,
        mtime: u64,
        bytes: u64,
        orientation: u8,
    ) -> Result<()> {
        if thumb.as_os_str().is_empty() {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO thumb_sources(thumb, source, mtime, bytes, orientation)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(thumb) DO UPDATE SET
                source = excluded.source,
                mtime = excluded.mtime,
                bytes = excluded.bytes,
                orientation = excluded.orientation",
            params![
                thumb.to_string_lossy().to_string(),
                source.to_string_lossy().to_string(),
                mtime as i64,
                bytes as i64,
                orientation as i64
            ],
        )?;
        Ok(())
    }

    pub fn thumb_source(&self, thumb: &Path) -> Result<Option<ThumbSource>> {
        self.conn
            .query_row(
                "SELECT source, mtime, bytes, orientation FROM thumb_sources WHERE thumb = ?1",
                [thumb.to_string_lossy().to_string()],
                |row| {
                    Ok(ThumbSource {
                        path: PathBuf::from(row.get::<_, String>(0)?),
                        mtime: row.get::<_, i64>(1)?.max(0) as u64,
                        bytes: row.get::<_, i64>(2)?.max(0) as u64,
                        orientation: row.get::<_, i64>(3)?.clamp(1, 8) as u8,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// 文件重命名/移动时把原有资产行迁到新路径，保留标签、评分、收藏与人工标签。
    pub fn move_asset_media(
        &self,
        old_path: &Path,
        new_path: &Path,
        folder: &str,
        thumb: Option<&Path>,
        width: u32,
        height: u32,
        bytes: u64,
        mtime: u64,
        hashes: &ContentHashes,
    ) -> Result<Asset> {
        self.invalidate_name_fold();
        let old_string = old_path.to_string_lossy().to_string();
        let new_string = new_path.to_string_lossy().to_string();
        if let Some(name) = new_path.file_name() {
            self.remember_name_fold(&name.to_string_lossy());
        }
        if old_string == new_string {
            return self.load_asset(new_path);
        }
        let name = new_path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| new_string.clone());
        let thumb_string = thumb
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        let valid_hashes = !hashes.sha1.is_empty() && !hashes.blake3.is_empty();
        let source_id = self.source_id(folder)?;
        let tx = self.conn.unchecked_transaction()?;
        let conflict: Option<i64> = tx
            .query_row(
                "SELECT id FROM assets WHERE path = ?1",
                [&new_string],
                |row| row.get(0),
            )
            .optional()?;
        if conflict.is_some() {
            bail!("asset path already exists: {new_string}");
        }
        let old_id: Option<i64> = tx
            .query_row(
                "SELECT id FROM assets WHERE path = ?1",
                [&old_string],
                |row| row.get(0),
            )
            .optional()?;
        let Some(old_id) = old_id else {
            bail!("asset not found: {old_string}");
        };
        tx.execute(
            "UPDATE assets SET
                source_id = ?1, path = ?2, folder = ?3, name = ?4, media_kind = ?5,
                mime = ?6, width = ?7, height = ?8, bytes = ?9, mtime = ?10, thumb = ?11,
                sha1 = ?12, blake3 = ?13, phash = ?14, damage = ?15,
                thumb_state = 'done'
             WHERE id = ?16",
            params![
                source_id,
                new_string,
                folder,
                name,
                media_kind(new_path).as_str(),
                mime_for(new_path),
                width,
                height,
                bytes as i64,
                mtime as i64,
                thumb_string,
                valid_hashes.then_some(hashes.sha1.as_str()),
                valid_hashes.then_some(hashes.blake3.as_str()),
                (valid_hashes && hashes.decode_ok).then_some(hashes.phash as i64),
                hashes.decode_error.as_deref(),
                old_id,
            ],
        )?;
        tx.commit()?;
        self.load_asset(new_path)
    }

    pub fn search_paths(&self, folder: &str, tokens: &[String]) -> Result<HashSet<PathBuf>> {
        self.search_paths_in(&[folder.to_string()], tokens)
    }

    pub fn search_paths_in(
        &self,
        folders: &[String],
        tokens: &[String],
    ) -> Result<HashSet<PathBuf>> {
        if tokens.is_empty() || folders.is_empty() {
            return Ok(HashSet::new());
        }
        let mut args = SqlArgs::new();
        args.clause(format!(
            "assets.folder IN ({})",
            placeholder_list(folders.len())
        ));
        for folder in folders {
            args.text(folder.clone());
        }
        self.add_free_text_clause(
            folders,
            tokens,
            FtsPlacement::Inline,
            None,
            &mut None,
            &mut args,
        )?;
        let sql = format!("SELECT assets.path FROM assets WHERE {}", args.sql());
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(args.params.iter()), |row| {
            row.get::<_, String>(0)
        })?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    /// 下推筛选/排序 + keyset 分页；语义与 UI 的 `Filter::matches` +
    /// `SearchQuery::matches` + 内存排序逐条一致。`search_paths_in` 保留为兼容入口。
    pub fn search_page(
        &self,
        folders: &[String],
        request: SearchPageRequest<'_>,
    ) -> Result<SearchPage> {
        self.search_page_with_plan(folders, request, SortPlan::CoverIndex)
    }

    fn search_page_with_plan(
        &self,
        folders: &[String],
        request: SearchPageRequest<'_>,
        plan: SortPlan,
    ) -> Result<SearchPage> {
        if request.limit == 0 {
            bail!("搜索分页 limit 必须大于 0");
        }
        let limit = request.limit.min(MAX_SEARCH_PAGE);
        if request.limit > MAX_SEARCH_PAGE {
            log::warn!(
                "search page limit {} clamped to {MAX_SEARCH_PAGE}",
                request.limit
            );
        }
        if folders.is_empty() {
            return Ok(empty_search_page(request.include_total));
        }
        let parts = request.query.parts();
        if parts.impossible
            || (parts.duplicates_only && request.duplicate_paths.is_none())
        {
            return Ok(empty_search_page(request.include_total));
        }
        if let Some(cursor) = request.cursor {
            if cursor.sort != request.sort || cursor.ascending != request.ascending {
                bail!("搜索游标与排序方式不匹配，请从第一页重新开始");
            }
            match (&cursor.key, request.sort) {
                (CursorKey::Name { .. }, SearchSort::Name)
                | (CursorKey::Primary { .. }, SearchSort::Modified)
                | (CursorKey::Primary { .. }, SearchSort::Captured)
                | (CursorKey::Primary { .. }, SearchSort::Size)
                | (CursorKey::Primary { .. }, SearchSort::Resolution)
                | (CursorKey::Primary { .. }, SearchSort::Star)
                | (CursorKey::Primary { .. }, SearchSort::Favorite)
                | (CursorKey::Primary { .. }, SearchSort::Rating) => {}
                _ => bail!("搜索游标类型与排序方式不匹配"),
            }
        }
        let mut built = self.build_search_sql(folders, &request, &parts)?;
        let threshold = limit.saturating_mul(8).max(256);
        let preferred = preferred_probe_index(request.filter, &parts);
        let total = if request.include_total {
            let from = match preferred {
                Some(index) => format!("assets INDEXED BY {index}"),
                None => "assets".to_string(),
            };
            let sql = format!("SELECT COUNT(*) FROM {from} WHERE {}", built.where_sql);
            let count: i64 = self.conn.query_row(
                &sql,
                rusqlite::params_from_iter(built.where_params.iter()),
                |row| row.get(0),
            )?;
            Some(count.max(0) as usize)
        } else {
            None
        };
        let (empty, broad) = match total {
            Some(total) => (total == 0, total > threshold),
            None => {
                let (count, broad) = self.search_extent(
                    folders,
                    &request,
                    &parts,
                    &built,
                    threshold,
                    preferred,
                )?;
                (count == 0, broad)
            }
        };
        if empty {
            return Ok(empty_search_page(request.include_total));
        }
        // 结果很多时按排序索引顺序扫描并在到达 limit 后停止，避免对全部命中
        // 做临时排序；结果较少时优先用筛选索引，再对小结果排序。非名称排序
        // 只有降序覆盖索引，且仅在名称折叠为 ASCII（索引里的 lower(name)
        // 与 ORDER BY 一致）时可用。
        if broad {
            if built.fold == Some(NameFold::Ascii) {
                if request.sort == SearchSort::Name {
                    built.force_index = Some("idx_assets_name_lower");
                } else if plan == SortPlan::CoverIndex {
                    built.force_index = sort_cover_index(request.sort);
                }
            }
        } else {
            built.force_index = preferred;
        }
        let page_sql = built.page_sql();
        let mut params = built.where_params.clone();
        params.extend(built.keyset_params.iter().cloned());
        params.push(Value::Integer(limit as i64 + 1));
        let mut statement = self.conn.prepare(&page_sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(params.iter()), |row| {
            Ok(SearchRow {
                path: row.get(0)?,
                primary: row.get(1)?,
                name_key: row.get(2)?,
                path_key: row.get(3)?,
                id: row.get(4)?,
            })
        })?;
        let mut rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        let next = if has_more {
            rows.last()
                .map(|row| cursor_for_row(row, request.sort, request.ascending, built.fold))
                .transpose()?
        } else {
            None
        };
        Ok(SearchPage {
            paths: rows.into_iter().map(|row| PathBuf::from(row.path)).collect(),
            next,
            total,
        })
    }

    fn build_search_sql(
        &self,
        folders: &[String],
        request: &SearchPageRequest<'_>,
        parts: &SearchQueryParts<'_>,
    ) -> Result<SearchSql> {
        self.build_search_sql_with(folders, request, parts, FtsPlacement::Inline, None)
    }

    fn build_search_sql_with(
        &self,
        folders: &[String],
        request: &SearchPageRequest<'_>,
        parts: &SearchQueryParts<'_>,
        fts: FtsPlacement,
        known_fts: Option<&str>,
    ) -> Result<SearchSql> {
        let mut args = SqlArgs::new();
        args.clause(format!(
            "assets.folder IN ({})",
            placeholder_list(folders.len())
        ));
        for folder in folders {
            args.text(folder.clone());
        }
        add_filter_clause(request.filter, &mut args)?;
        let mut fts_query = None;
        self.add_query_clauses(
            folders,
            parts,
            request.duplicate_paths,
            fts,
            known_fts,
            &mut fts_query,
            &mut args,
        )?;
        let where_sql = args.sql();
        let where_params = args.params;

        // 名称折叠只取决于库内数据，名称排序和非名称排序的 tie-breaker 都
        // 需要同一个折叠方式；游标里的折叠方式优先，保证翻页期间一致。
        let fold = match request.cursor {
            Some(SearchCursor {
                key: CursorKey::Name { fold, .. },
                ..
            }) if request.sort == SearchSort::Name => Some(*fold),
            Some(SearchCursor {
                key: CursorKey::Primary { fold, .. },
                ..
            }) if request.sort != SearchSort::Name => Some(*fold),
            Some(_) => bail!("搜索游标类型与排序方式不匹配"),
            None => Some(self.detect_name_fold()?),
        };
        let name_expr = name_sort_expr(fold.unwrap());
        let path_expr = path_sort_expr();
        let primary_expr = sort_primary_expr(request.sort);
        let direction = if request.ascending { "ASC" } else { "DESC" };
        let order_sql = match &primary_expr {
            None => format!("{name_expr} {direction}, {path_expr} {direction}, assets.id {direction}"),
            Some(primary) => format!(
                "{primary} {direction}, {name_expr} ASC, {path_expr} ASC, assets.id ASC"
            ),
        };
        let select_sql = match &primary_expr {
            None => format!("assets.path, NULL, {name_expr}, {path_expr}, assets.id"),
            Some(primary) => {
                format!("assets.path, {primary}, {name_expr}, {path_expr}, assets.id")
            }
        };
        let (keyset_sql, keyset_params) = match request.cursor {
            None => (None, Vec::new()),
            Some(cursor) => build_keyset_sql(
                cursor,
                request.sort,
                request.ascending,
                &name_expr,
                &path_expr,
                primary_expr.as_deref(),
            )?,
        };
        Ok(SearchSql {
            where_sql,
            where_params,
            keyset_sql,
            keyset_params,
            order_sql,
            select_sql,
            fold,
            force_index: None,
            fts_query,
        })
    }

    /// 构造 FTS extent 探测 SQL，返回未追加 LIMIT 参数的语句与参数。
    ///
    /// 必须固定 FTS 为外表（CROSS JOIN）。普通 JOIN 在窄 FTS 结果（如 10
    /// 命中）时，SQLite 3.46 可能选择扫 folder 索引并逐行探 FTS，10 万库会
    /// 退化到十几秒；CROSS JOIN 与 `fts_has_rows` 一样强制 FTS 驱动。
    fn fts_extent_query(
        &self,
        folders: &[String],
        request: &SearchPageRequest<'_>,
        parts: &SearchQueryParts<'_>,
        fts_query: &str,
    ) -> Result<(String, Vec<Value>)> {
        let external = self.build_search_sql_with(
            folders,
            request,
            parts,
            FtsPlacement::External,
            Some(fts_query),
        )?;
        let sql = format!(
            "SELECT 1 FROM assets_fts CROSS JOIN assets ON assets.id = assets_fts.rowid
             WHERE assets_fts MATCH ? AND {} LIMIT ?",
            external.where_sql
        );
        let mut params = vec![Value::Text(fts_query.to_string())];
        params.extend(external.where_params.iter().cloned());
        Ok((sql, params))
    }

    /// 只探测“是否为空”和“是否超过阈值”，不排序、不取全量。
    ///
    /// 有 FTS 命中条件时让 FTS 表驱动：命中很多时取满阈值就停，不必像
    /// `IN (SELECT rowid ...)` 那样先把全部命中 rowid 物化（10 万全命中约
    /// 45–80ms）。
    fn search_extent(
        &self,
        folders: &[String],
        request: &SearchPageRequest<'_>,
        parts: &SearchQueryParts<'_>,
        built: &SearchSql,
        threshold: usize,
        hint: Option<&'static str>,
    ) -> Result<(usize, bool)> {
        if let Some(fts_query) = built.fts_query.clone() {
            let (sql, mut params) =
                self.fts_extent_query(folders, request, parts, &fts_query)?;
            params.push(Value::Integer(threshold as i64 + 1));
            let mut statement = self.conn.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(params.iter()), |row| {
                row.get::<_, i64>(0)
            })?;
            let mut count = 0usize;
            for row in rows {
                let _ = row?;
                count += 1;
                if count > threshold {
                    return Ok((count, true));
                }
            }
            return Ok((count, false));
        }
        let from = match hint {
            Some(index) => format!("assets INDEXED BY {index}"),
            None => "assets".to_string(),
        };
        let sql = format!("SELECT 1 FROM {from} WHERE {} LIMIT ?", built.where_sql);
        let mut params = built.where_params.clone();
        params.push(Value::Integer(threshold as i64 + 1));
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(params.iter()), |row| {
            row.get::<_, i64>(0)
        })?;
        let mut count = 0usize;
        for row in rows {
            let _ = row?;
            count += 1;
            if count > threshold {
                return Ok((count, true));
            }
        }
        Ok((count, false))
    }

    fn detect_name_fold(&self) -> Result<NameFold> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA data_version", [], |row| row.get(0))?;
        if let Some((cached_version, fold)) = *self.name_fold.lock().unwrap()
            && cached_version == version
        {
            return Ok(fold);
        }
        // 库级属性：只要存在内置 lower() 与 Rust Unicode 小写不同的名称，
        // 名称排序就改用精确函数；否则使用可索引的 lower()。
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [NAME_FOLD_KEY],
                |row| row.get(0),
            )
            .optional()?;
        let fold = match stored.as_deref() {
            Some("1") => NameFold::Unicode,
            Some("0") => NameFold::Ascii,
            _ => {
                let found: Option<i64> = self
                    .conn
                    .query_row(
                        "SELECT 1 FROM assets
                         WHERE lower(assets.name) <> kanalib_lower(assets.name) LIMIT 1",
                        [],
                        |row| row.get(0),
                    )
                    .optional()?;
                let fold = if found.is_some() {
                    NameFold::Unicode
                } else {
                    NameFold::Ascii
                };
                let value = if fold == NameFold::Unicode { "1" } else { "0" };
                let _ = self.conn.execute(
                    "INSERT INTO settings(key, value) VALUES(?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![NAME_FOLD_KEY, value],
                );
                fold
            }
        };
        *self.name_fold.lock().unwrap() = Some((version, fold));
        Ok(fold)
    }

    fn invalidate_name_fold(&self) {
        *self.name_fold.lock().unwrap() = None;
    }

    fn remember_name_fold(&self, name: &str) {
        if name.to_lowercase() != name.to_ascii_lowercase() {
            let _ = self.conn.execute(
                "INSERT INTO settings(key, value) VALUES(?1, '1')
                 ON CONFLICT(key) DO UPDATE SET value = '1'",
                [NAME_FOLD_KEY],
            );
        }
    }

    /// 自由文本：>=3 字优先 FTS5 trigram，短词或 FTS 无结果时回退 LIKE。
    /// 混合词只在所有词均为 ASCII 时把长词交给 FTS、短词留给 LIKE，避免全表扫描。
    ///
    /// `fts_query` 记录实际使用的 MATCH 查询：外置模式下不内联 IN 子句，
    /// 由调用方用 `assets_fts MATCH ?` 驱动连接。
    ///
    /// `known_fts` 用于第二次构造同一条件（`search_extent` 的外置探测）：
    /// 直接复用已判定过的 MATCH 查询，跳过重复的 `fts_has_rows`（10 万命中
    /// 的 trigram 交集探测约 50–60ms，重复一次会白付一整页的时间）。
    fn add_free_text_clause(
        &self,
        folders: &[String],
        tokens: &[String],
        fts: FtsPlacement,
        known_fts: Option<&str>,
        fts_query: &mut Option<String>,
        args: &mut SqlArgs,
    ) -> Result<()> {
        if tokens.is_empty() {
            return Ok(());
        }
        let all_long = tokens.iter().all(|token| token.chars().count() >= 3);
        let long = tokens
            .iter()
            .filter(|token| token.chars().count() >= 3)
            .cloned()
            .collect::<Vec<_>>();
        let short = tokens
            .iter()
            .filter(|token| token.chars().count() < 3)
            .cloned()
            .collect::<Vec<_>>();
        if all_long {
            let query = match known_fts {
                Some(query) => query.to_string(),
                None => fts_match_query(&long),
            };
            let has_rows = if known_fts.is_some() {
                Ok(true)
            } else {
                self.fts_has_rows(folders, &query)
            };
            match has_rows {
                Ok(true) => {
                    if fts == FtsPlacement::Inline {
                        args.clause(
                            "assets.id IN (SELECT rowid FROM assets_fts WHERE assets_fts MATCH ?)",
                        );
                        args.text(query.clone());
                    }
                    *fts_query = Some(query);
                    return Ok(());
                }
                Ok(false) if simple_ascii_tokens(tokens) => {
                    // trigram 对纯 ASCII 字母数字 >=3 字是完整子串索引；空结果
                    // 不必再做一遍全表 LIKE。
                    args.clause("0");
                    return Ok(());
                }
                Ok(false) => {}
                Err(error) => log::warn!("fts probe failed, falling back to LIKE: {error:#}"),
            }
            add_like_clause(args, tokens);
            return Ok(());
        }
        if !long.is_empty() && simple_ascii_tokens(tokens) {
            let query = match known_fts {
                Some(query) => query.to_string(),
                None => fts_match_query(&long),
            };
            let has_rows = if known_fts.is_some() {
                Ok(true)
            } else {
                self.fts_has_rows(folders, &query)
            };
            match has_rows {
                Ok(true) => {
                    if fts == FtsPlacement::Inline {
                        args.clause(
                            "assets.id IN (SELECT rowid FROM assets_fts WHERE assets_fts MATCH ?)",
                        );
                        args.text(query.clone());
                    }
                    *fts_query = Some(query);
                    add_like_clause(args, &short);
                    return Ok(());
                }
                Ok(false) => {
                    args.clause("0");
                    return Ok(());
                }
                Err(error) => log::warn!("fts probe failed, falling back to LIKE: {error:#}"),
            }
        }
        add_like_clause(args, tokens);
        Ok(())
    }

    fn fts_has_rows(&self, folders: &[String], query: &str) -> Result<bool> {
        // CROSS JOIN 固定 FTS 为外表；否则 SQLite 3.46 可能选择 folder 索引
        // 逐行探测 FTS，空结果时退化成全表扫描。
        let sql = format!(
            "SELECT 1 FROM assets_fts CROSS JOIN assets ON assets.id = assets_fts.rowid
             WHERE assets_fts MATCH ? AND assets.folder IN ({}) LIMIT 1",
            placeholder_list(folders.len())
        );
        let mut params = vec![Value::Text(query.to_string())];
        params.extend(folders.iter().cloned().map(Value::Text));
        let found: Option<i64> = self
            .conn
            .query_row(&sql, rusqlite::params_from_iter(params.iter()), |row| {
                row.get(0)
            })
            .optional()?;
        Ok(found.is_some())
    }

    fn add_query_clauses(
        &self,
        folders: &[String],
        parts: &SearchQueryParts<'_>,
        duplicate_paths: Option<&HashSet<PathBuf>>,
        fts: FtsPlacement,
        known_fts: Option<&str>,
        fts_query: &mut Option<String>,
        args: &mut SqlArgs,
    ) -> Result<()> {
        if let Some(rating) = parts.rating {
            args.clause("assets.rating = ? COLLATE NOCASE");
            args.text(rating);
        }
        if let Some(rating) = parts.exclude_rating {
            args.clause("assets.rating <> ? COLLATE NOCASE");
            args.text(rating);
        }
        if let Some(tagged) = parts.tagged {
            args.clause("assets.tagged = ?");
            args.integer(i64::from(tagged));
        }
        if let Some(favorite) = parts.favorite {
            args.clause("assets.favorite = ?");
            args.integer(i64::from(favorite));
        }
        if let Some(failed) = parts.failed {
            args.clause(if failed {
                "assets.error IS NOT NULL"
            } else {
                "assets.error IS NULL"
            });
        }
        if let Some(damaged) = parts.damaged {
            args.clause(if damaged {
                "assets.damage IS NOT NULL"
            } else {
                "assets.damage IS NULL"
            });
        }
        if let Some(star) = parts.star {
            args.clause("assets.star = ?");
            args.integer(i64::from(star));
        }
        if let Some(star) = parts.star_min {
            args.clause("assets.star >= ?");
            args.integer(i64::from(star));
        }
        if let Some(star) = parts.star_max {
            args.clause("assets.star <= ?");
            args.integer(i64::from(star));
        }
        if let Some(star) = parts.exclude_star {
            args.clause("assets.star <> ?");
            args.integer(i64::from(star));
        }
        for tag in parts.tags {
            add_tag_exists(args, tag, None, true, true);
        }
        for tag in parts.exclude_tags {
            add_tag_exists(args, tag, None, false, true);
        }
        for scoped in parts.scoped_tags {
            add_tag_exists(args, &scoped.value, Some(&scoped.category), true, true);
        }
        for scoped in parts.exclude_scoped_tags {
            add_tag_exists(args, &scoped.value, Some(&scoped.category), false, true);
        }
        for group in parts.groups {
            args.clause("instr(kanalib_lower(assets.group_name), ?) > 0");
            args.text(group.to_lowercase());
        }
        for group in parts.exclude_groups {
            args.clause("instr(kanalib_lower(assets.group_name), ?) = 0");
            args.text(group.to_lowercase());
        }
        for name in parts.names {
            args.clause("instr(kanalib_lower(assets.name), ?) > 0");
            args.text(name.to_lowercase());
        }
        for name in parts.exclude_names {
            args.clause("instr(kanalib_lower(assets.name), ?) = 0");
            args.text(name.to_lowercase());
        }
        for path in parts.paths {
            args.clause("instr(kanalib_lower(assets.path), ?) > 0");
            args.text(path.to_lowercase());
        }
        for path in parts.exclude_paths {
            args.clause("instr(kanalib_lower(assets.path), ?) = 0");
            args.text(path.to_lowercase());
        }
        for filter in parts.numeric {
            add_numeric_clause(args, filter, true);
        }
        for filter in parts.exclude_numeric {
            add_numeric_clause(args, filter, false);
        }
        for filter in parts.dates {
            add_date_clause(args, filter, true);
        }
        for filter in parts.exclude_dates {
            add_date_clause(args, filter, false);
        }
        self.add_free_text_clause(folders, parts.free, fts, known_fts, fts_query, args)?;
        for token in parts.exclude_free {
            let needle = token.to_lowercase();
            args.clause(
                "NOT (instr(kanalib_lower(assets.name), ?) > 0
                    OR instr(kanalib_lower(assets.path), ?) > 0
                    OR EXISTS (
                        SELECT 1 FROM asset_tags at
                        JOIN tags t ON t.id = at.tag_id
                        WHERE at.asset_id = assets.id
                          AND instr(kanalib_lower(t.name), ?) > 0
                    ))",
            );
            args.text(needle.clone());
            args.text(needle.clone());
            args.text(needle);
        }
        if parts.duplicates_only {
            let Some(paths) = duplicate_paths else {
                args.clause("0");
                return Ok(());
            };
            if paths.is_empty() {
                args.clause("0");
                return Ok(());
            }
            let mut chunks = Vec::new();
            let mut values = Vec::new();
            let paths = paths.iter().map(|path| path.to_string_lossy().to_string());
            let paths = paths.collect::<Vec<_>>();
            for chunk in paths.chunks(500) {
                chunks.push(format!(
                    "assets.path IN ({})",
                    placeholder_list(chunk.len())
                ));
                values.extend(chunk.iter().cloned());
            }
            args.clause(format!("({})", chunks.join(" OR ")));
            for value in values {
                args.text(value);
            }
        }
        Ok(())
    }

    pub fn tags_missing_vocab(&self) -> Result<Vec<String>> {
        let mut statement = self.conn.prepare(
            "SELECT name FROM tags WHERE vocab_index IS NULL AND alias_of IS NULL ORDER BY name",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_vocab_indices(&self, mappings: &[(String, u32)]) -> Result<()> {
        if mappings.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        for (name, index) in mappings {
            tx.execute(
                "UPDATE tags SET vocab_index = ?1 WHERE name = ?2 AND vocab_index IS NULL",
                params![*index as i64, name],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_asset(&self, path: &Path) -> Result<Asset> {
        let path_string = path.to_string_lossy().to_string();
        let mut asset = self.conn.query_row(
            "SELECT path, thumb, width, height, group_name, rating, tags_json, tagged, error,
                    sha1, blake3, phash, tag_attempts, bytes, mtime, favorite, star, folder,
                    damage, captured_at, metadata_json
             FROM assets WHERE path = ?1",
            [&path_string],
            row_to_asset,
        )?;
        asset.manual_tags = self.manual_tags_for_path(path)?;
        Ok(asset)
    }

    fn manual_tags_for_folders(&self, folders: &[String]) -> Result<HashMap<PathBuf, Vec<TagHit>>> {
        if folders.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = vec!["?"; folders.len()].join(", ");
        let sql = format!(
            "SELECT a.path, t.name, t.category, at.confidence, t.vocab_index
             FROM asset_tags at
             JOIN assets a ON a.id = at.asset_id
             JOIN tags t ON t.id = at.tag_id
             WHERE a.folder IN ({placeholders}) AND at.origin = 'manual'
             ORDER BY a.path, t.name"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows =
            statement.query_map(rusqlite::params_from_iter(folders.iter()), manual_tag_row)?;
        let mut tags: HashMap<PathBuf, Vec<TagHit>> = HashMap::new();
        for row in rows {
            let (path, tag) = row?;
            tags.entry(path).or_default().push(tag);
        }
        Ok(tags)
    }

    fn manual_tags_for_path(&self, path: &Path) -> Result<Vec<TagHit>> {
        let mut statement = self.conn.prepare(
            "SELECT a.path, t.name, t.category, at.confidence, t.vocab_index
             FROM asset_tags at
             JOIN assets a ON a.id = at.asset_id
             JOIN tags t ON t.id = at.tag_id
             WHERE a.path = ?1 AND at.origin = 'manual'
             ORDER BY t.name",
        )?;
        let rows = statement.query_map([path.to_string_lossy().to_string()], manual_tag_row)?;
        let mut tags = Vec::new();
        for row in rows {
            tags.push(row?.1);
        }
        Ok(tags)
    }

    fn source_id(&self, folder: &str) -> Result<i64> {
        let config = serde_json::json!({ "path": folder }).to_string();
        self.conn.execute(
            "INSERT OR IGNORE INTO sources(kind, name, locator, config_json, read_only)
             VALUES('local', ?1, ?1, ?2, 1)",
            params![folder, config],
        )?;
        self.conn
            .query_row(
                "SELECT id FROM sources WHERE kind = 'local' AND locator = ?1",
                [folder],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }
}

fn empty_search_page(include_total: bool) -> SearchPage {
    SearchPage {
        paths: Vec::new(),
        next: None,
        total: include_total.then_some(0),
    }
}

fn fts_match_query(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn simple_ascii_tokens(tokens: &[String]) -> bool {
    !tokens.is_empty()
        && tokens
            .iter()
            .all(|token| token.chars().all(|ch| ch.is_ascii_alphanumeric()))
}

fn add_like_clause(args: &mut SqlArgs, tokens: &[String]) {
    for token in tokens {
        args.clause(
            r"(assets.name LIKE ? ESCAPE '\' OR assets.path LIKE ? ESCAPE '\' OR assets.tags_text LIKE ? ESCAPE '\')",
        );
        let needle = format!("%{}%", escape_like(token));
        args.text(needle.clone());
        args.text(needle.clone());
        args.text(needle);
    }
}

fn preferred_probe_index(
    filter: &Filter,
    parts: &SearchQueryParts<'_>,
) -> Option<&'static str> {
    if !parts.free.is_empty() {
        return None;
    }
    if matches!(filter, Filter::Favorites) || parts.favorite == Some(true) {
        return Some("idx_assets_favorite");
    }
    if matches!(filter, Filter::Untagged) || parts.tagged == Some(false) {
        return Some("idx_assets_tagged");
    }
    if matches!(filter, Filter::Failed) || parts.failed == Some(true) {
        return Some("idx_assets_error_not_null");
    }
    if matches!(filter, Filter::Damaged) || parts.damaged == Some(true) {
        return Some("idx_assets_damage_not_null");
    }
    if matches!(filter, Filter::Rating(_)) || parts.rating.is_some() {
        return Some("idx_assets_rating_nocase");
    }
    if matches!(filter, Filter::Group(_)) {
        return Some("idx_assets_group_name");
    }
    if parts.star.is_some() || parts.star_min.is_some() || parts.star_max.is_some() {
        return Some("idx_assets_star");
    }
    if parts.exclude_numeric.is_empty() && parts.numeric.len() == 1 {
        return match parts.numeric[0].field {
            NumField::Size => Some("idx_assets_bytes"),
            NumField::Width => Some("idx_assets_width"),
            NumField::Height => Some("idx_assets_height"),
            NumField::Ratio => None,
        };
    }
    if matches!(filter, Filter::Source(_)) {
        return Some("idx_assets_folder_path");
    }
    None
}

fn add_filter_clause(filter: &Filter, args: &mut SqlArgs) -> Result<()> {
    match filter {
        Filter::All => {}
        Filter::Untagged => args.clause("assets.tagged = 0"),
        Filter::Failed => args.clause("assets.error IS NOT NULL"),
        Filter::Damaged => args.clause("assets.damage IS NOT NULL"),
        Filter::Favorites => args.clause("assets.favorite <> 0"),
        Filter::Rating(rating) => {
            args.clause("assets.rating = ? COLLATE NOCASE");
            args.text(rating.clone());
        }
        Filter::Group(group) => {
            args.clause("assets.group_name = ?");
            args.text(group.clone());
        }
        Filter::Tag(tag) => add_tag_exists(args, tag, None, true, false),
        Filter::Source(root) => {
            args.clause("assets.folder = ?");
            args.text(root.to_string_lossy().to_string());
        }
    }
    Ok(())
}

fn add_tag_exists(
    args: &mut SqlArgs,
    value: &str,
    category: Option<&str>,
    positive: bool,
    case_insensitive: bool,
) {
    let mut clause = String::from(
        "EXISTS (SELECT 1 FROM asset_tags at JOIN tags t ON t.id = at.tag_id
         WHERE at.asset_id = assets.id",
    );
    let mut values = Vec::new();
    if let Some(category) = category {
        clause.push_str(
            " AND instr(kanalib_lower(t.name), ?) > 0 AND t.category = ? COLLATE NOCASE",
        );
        values.push(value.to_lowercase());
        values.push(category.to_string());
    } else if case_insensitive {
        clause.push_str(" AND t.name = ? COLLATE NOCASE");
        values.push(value.to_string());
    } else {
        // 与 `Filter::matches` 的精确比较一致（标签筛选来自标签列表，大小写已规范）。
        clause.push_str(" AND t.name = ?");
        values.push(value.to_string());
    }
    clause.push(')');
    if !positive {
        clause = format!("NOT ({clause})");
    }
    args.clause(clause);
    for value in values {
        args.text(value);
    }
}

fn numeric_expr(field: NumField) -> String {
    match field {
        NumField::Size => "CAST(assets.bytes AS REAL)".to_string(),
        NumField::Width => "CAST(assets.width AS REAL)".to_string(),
        NumField::Height => "CAST(assets.height AS REAL)".to_string(),
        NumField::Ratio => {
            "CASE WHEN assets.height > 0
                THEN CAST(assets.width AS REAL) / CAST(assets.height AS REAL)
                ELSE NULL END"
                .to_string()
        }
    }
}

fn comparison_sql(op: NumOp, expr: &str) -> String {
    match op {
        NumOp::Eq => format!("abs(({expr}) - ?) < 2.220446049250313e-16"),
        NumOp::Gt => format!("({expr}) > ?"),
        NumOp::Ge => format!("({expr}) >= ?"),
        NumOp::Lt => format!("({expr}) < ?"),
        NumOp::Le => format!("({expr}) <= ?"),
    }
}

fn add_numeric_clause(args: &mut SqlArgs, filter: &NumericFilter, positive: bool) {
    let expr = numeric_expr(filter.field);
    let comparison = comparison_sql(filter.op, &expr);
    if positive {
        args.clause(format!("({comparison})"));
    } else {
        args.clause(format!("NOT COALESCE(({comparison}), 0)"));
    }
    args.real(filter.value);
}

fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn add_date_clause(args: &mut SqlArgs, filter: &DateFilter, positive: bool) {
    let taken = "COALESCE(assets.captured_at, assets.mtime)";
    let day = format!("CAST(({taken}) / 86400 AS INTEGER)");
    let mut values = Vec::new();
    let last_day = days_in_month(filter.year, filter.month);
    let requested_day = filter.day.unwrap_or(1);
    let (op, target_day, invalid_future) = if requested_day > last_day {
        // Rust 端比较的是 (year, month, day) 元组；2 月 31 日这类不存在的
        // 日期在 Eq 下永不命中，其他比较等价于与下月 1 日比较。
        let next = if filter.month == 12 {
            (filter.year.saturating_add(1), 1, 1)
        } else {
            (filter.year, filter.month + 1, 1)
        };
        let next_day = day_number(next.0, next.1, next.2);
        match filter.op {
            NumOp::Eq => (NumOp::Eq, 0, true),
            NumOp::Gt | NumOp::Ge => (NumOp::Ge, next_day, false),
            NumOp::Lt | NumOp::Le => (NumOp::Lt, next_day, false),
        }
    } else if filter.op == NumOp::Eq && filter.day.is_none() {
        let start = day_number(filter.year, filter.month, 1);
        let next = if filter.month == 12 {
            (filter.year.saturating_add(1), 1, 1)
        } else {
            (filter.year, filter.month + 1, 1)
        };
        values.push(start);
        values.push(day_number(next.0, next.1, next.2));
        let clause = format!("({day} >= ? AND {day} < ?)");
        args.clause(if positive {
            clause
        } else {
            format!("NOT ({clause})")
        });
        for value in values {
            args.integer(value);
        }
        return;
    } else {
        (
            filter.op,
            day_number(filter.year, filter.month, requested_day),
            false,
        )
    };
    let clause = if invalid_future {
        "0".to_string()
    } else {
        let op = match op {
            NumOp::Eq => "=",
            NumOp::Gt => ">",
            NumOp::Ge => ">=",
            NumOp::Lt => "<",
            NumOp::Le => "<=",
        };
        values.push(target_day);
        format!("{day} {op} ?")
    };
    args.clause(if positive {
        clause
    } else {
        format!("NOT ({clause})")
    });
    for value in values {
        args.integer(value);
    }
}

fn build_keyset_sql(
    cursor: &SearchCursor,
    sort: SearchSort,
    ascending: bool,
    name_expr: &str,
    path_expr: &str,
    primary_expr: Option<&str>,
) -> Result<(Option<String>, Vec<Value>)> {
    if cursor.sort != sort || cursor.ascending != ascending {
        bail!("搜索游标与排序方式不匹配，请从第一页重新开始");
    }
    match (&cursor.key, primary_expr) {
        (
            CursorKey::Name {
                name, path, id, ..
            },
            None,
        ) => {
            let op = if ascending { ">" } else { "<" };
            let sql = format!("({name_expr}, {path_expr}, assets.id) {op} (?, ?, ?)");
            Ok((
                Some(sql),
                vec![
                    Value::Text(name.clone()),
                    Value::Text(path.clone()),
                    Value::Integer(*id),
                ],
            ))
        }
        (
            CursorKey::Primary {
                primary,
                name,
                path,
                id,
                ..
            },
            Some(primary_expr),
        ) => {
            let op = if ascending { ">" } else { "<" };
            let sql = format!(
                "({primary_expr} {op} ? OR ({primary_expr} = ? AND \
                 ({name_expr}, {path_expr}, assets.id) > (?, ?, ?)))"
            );
            Ok((
                Some(sql),
                vec![
                    Value::Integer(*primary),
                    Value::Integer(*primary),
                    Value::Text(name.clone()),
                    Value::Text(path.clone()),
                    Value::Integer(*id),
                ],
            ))
        }
        _ => bail!("搜索游标类型与排序方式不匹配"),
    }
}

fn cursor_for_row(
    row: &SearchRow,
    sort: SearchSort,
    ascending: bool,
    fold: Option<NameFold>,
) -> Result<SearchCursor> {
    let key = match sort {
        SearchSort::Name => {
            let Some(fold) = fold else {
                bail!("缺少名称折叠方式，无法生成游标");
            };
            CursorKey::Name {
                fold,
                name: row.name_key.clone(),
                path: row.path_key.clone(),
                id: row.id,
            }
        }
        _ => {
            let Some(primary) = row.primary else {
                bail!("缺少排序主键，无法生成游标");
            };
            let Some(fold) = fold else {
                bail!("缺少名称折叠方式，无法生成游标");
            };
            CursorKey::Primary {
                fold,
                primary,
                name: row.name_key.clone(),
                path: row.path_key.clone(),
                id: row.id,
            }
        }
    };
    Ok(SearchCursor {
        sort,
        ascending,
        key,
    })
}

fn row_to_asset(row: &rusqlite::Row<'_>) -> rusqlite::Result<Asset> {
    let path: String = row.get(0)?;
    let thumb: String = row.get(1)?;
    let tags_json: String = row.get(6)?;
    let tagged: i64 = row.get(7)?;
    let phash: Option<i64> = row.get(11)?;
    Ok(Asset {
        path: PathBuf::from(path),
        library_root: PathBuf::from(row.get::<_, String>(17)?),
        thumb: (!thumb.is_empty()).then(|| PathBuf::from(thumb)),
        tile_thumb: None,
        width: row.get::<_, i64>(2)?.max(0) as u32,
        height: row.get::<_, i64>(3)?.max(0) as u32,
        group: row.get(4)?,
        rating: row.get(5)?,
        favorite: row.get::<_, i64>(15)? != 0,
        star: row.get::<_, i64>(16)?.clamp(0, 5) as u8,
        tags: serde_json::from_str::<Vec<TagHit>>(&tags_json).unwrap_or_default(),
        manual_tags: Vec::new(),
        tagged: tagged != 0,
        tag_attempts: row.get::<_, i64>(12)?.max(0) as u32,
        error: row.get(8)?,
        damage: row.get(18)?,
        sha1: row.get(9)?,
        blake3: row.get(10)?,
        phash: phash.map(|value| value as u64),
        bytes: row.get::<_, i64>(13)?.max(0) as u64,
        mtime: row.get::<_, i64>(14)?.max(0) as u64,
        captured_at: row
            .get::<_, Option<i64>>(19)?
            .map(|value| value.max(0) as u64),
        metadata_json: row.get(20)?,
    })
}

fn manual_tag_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(PathBuf, TagHit)> {
    Ok((
        PathBuf::from(row.get::<_, String>(0)?),
        TagHit {
            tag: row.get(1)?,
            category: row.get(2)?,
            score: row.get::<_, f64>(3)? as f32,
            vocab_index: row.get::<_, Option<i64>>(4)?.map(|value| value as u32),
        },
    ))
}

fn threshold_value(value: f32) -> Result<f32> {
    if !value.is_finite() {
        bail!("阈值必须是有限数值");
    }
    Ok(value.clamp(0.0, 1.0))
}

fn category_rank(category: &str) -> usize {
    TAG_CATEGORIES
        .iter()
        .position(|known| *known == category)
        .unwrap_or(TAG_CATEGORIES.len())
}

/// 每类按分数取 top-K，并压掉重复标签；未知类别直接丢弃。
fn cap_candidates(candidates: &[TagHit]) -> Vec<TagHit> {
    let mut buckets: HashMap<String, Vec<TagHit>> = HashMap::new();
    for candidate in candidates {
        let category = candidate.category.trim().to_ascii_lowercase();
        if candidate.tag.trim().is_empty() || !candidate.score.is_finite() {
            continue;
        }
        if candidate.score < TAG_CANDIDATE_MIN_SCORE || candidate_limit(&category) == 0 {
            continue;
        }
        buckets.entry(category).or_default().push(TagHit {
            tag: candidate.tag.trim().to_string(),
            category: candidate.category.trim().to_ascii_lowercase(),
            score: candidate.score.clamp(0.0, 1.0),
            vocab_index: candidate.vocab_index,
        });
    }
    let mut capped = Vec::new();
    for (category, mut items) in buckets {
        items.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.tag.cmp(&b.tag))
        });
        let mut seen = HashSet::new();
        items.retain(|item| seen.insert(item.tag.to_lowercase()));
        items.truncate(candidate_limit(&category));
        capped.extend(items);
    }
    capped.sort_by(|a, b| {
        category_rank(&a.category)
            .cmp(&category_rank(&b.category))
            .then_with(|| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.tag.cmp(&b.tag))
    });
    capped
}

fn candidate_states(conn: &Connection, asset_id: i64) -> Result<HashMap<String, TagReviewState>> {
    let mut statement =
        conn.prepare("SELECT tag, review_state FROM asset_scores WHERE asset_id = ?1")?;
    let rows = statement.query_map([asset_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            TagReviewState::from_db(&row.get::<_, String>(1)?),
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

fn save_candidates(
    conn: &Connection,
    asset_id: i64,
    candidates: &[TagHit],
    states: &HashMap<String, TagReviewState>,
) -> Result<Vec<TagHit>> {
    let capped = cap_candidates(candidates);
    conn.execute("DELETE FROM asset_scores WHERE asset_id = ?1", [asset_id])?;
    {
        let mut statement = conn.prepare(
            "INSERT INTO asset_scores(
                asset_id, tag, category, score, vocab_index, review_state, reviewed_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, NULL)",
        )?;
        for candidate in &capped {
            let state = states
                .get(&candidate.tag)
                .copied()
                .unwrap_or(TagReviewState::Pending);
            statement.execute(params![
                asset_id,
                candidate.tag,
                candidate.category,
                candidate.score as f64,
                candidate.vocab_index.map(|value| value as i64),
                state.as_str(),
            ])?;
        }
    }
    Ok(capped)
}

fn read_candidates(
    conn: &Connection,
    asset_id: i64,
) -> Result<(Vec<TagHit>, HashMap<String, TagReviewState>)> {
    let mut statement = conn.prepare(
        "SELECT tag, category, score, vocab_index, review_state
         FROM asset_scores WHERE asset_id = ?1
         ORDER BY score DESC, tag",
    )?;
    let rows = statement.query_map([asset_id], |row| {
        Ok((
            TagHit {
                tag: row.get(0)?,
                category: row.get(1)?,
                score: row.get::<_, f64>(2)? as f32,
                vocab_index: row
                    .get::<_, Option<i64>>(3)?
                    .map(|value| value.max(0) as u32),
            },
            TagReviewState::from_db(&row.get::<_, String>(4)?),
        ))
    })?;
    let mut candidates = Vec::new();
    let mut states = HashMap::new();
    for row in rows {
        let (candidate, state) = row?;
        states.insert(candidate.tag.clone(), state);
        candidates.push(candidate);
    }
    Ok((candidates, states))
}

fn auto_candidates(
    candidates: &[TagHit],
    states: &HashMap<String, TagReviewState>,
    thresholds: &TagThresholds,
) -> Vec<TagHit> {
    let mut seen = HashSet::new();
    let mut auto = candidates
        .iter()
        .filter(|candidate| {
            let accepted = states
                .get(&candidate.tag)
                .is_some_and(|state| *state == TagReviewState::Accepted);
            accepted || candidate.score >= thresholds.threshold(&candidate.category, &candidate.tag)
        })
        .filter(|candidate| seen.insert(candidate.tag.to_lowercase()))
        .cloned()
        .collect::<Vec<_>>();
    auto.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.tag.cmp(&b.tag))
    });
    auto
}

fn choose_copyright(tags: &[&str]) -> Option<String> {
    let mut fallback = None;
    for tag in tags {
        if *tag == "original" {
            continue;
        }
        if fallback.is_none() {
            fallback = Some((*tag).to_string());
        }
        if !tag.ends_with("_(series)") {
            return Some((*tag).to_string());
        }
        let root = tag.split(['_', ':']).next().unwrap_or("");
        for other in tags {
            if other == tag || other.ends_with("_(series)") {
                continue;
            }
            if other.split(['_', ':']).next().unwrap_or("") == root {
                return Some((*other).to_string());
            }
        }
    }
    fallback
}

/// 与 sidecar 的 rating / group 语义对齐；角色换装归并不在 Rust 侧重做。
fn derive_rating_group(auto: &[TagHit], thresholds: &TagThresholds) -> (String, String) {
    let mut passing = auto
        .iter()
        .filter(|candidate| {
            candidate.score >= thresholds.threshold(&candidate.category, &candidate.tag)
        })
        .collect::<Vec<_>>();
    passing.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.tag.cmp(&b.tag))
    });
    let rating = passing
        .iter()
        .find(|candidate| candidate.category == "rating")
        .map(|candidate| {
            candidate
                .tag
                .strip_prefix("rating:")
                .unwrap_or(&candidate.tag)
                .to_string()
        })
        .unwrap_or_default();
    let general = passing
        .iter()
        .filter(|candidate| candidate.category == "general")
        .map(|candidate| candidate.tag.as_str())
        .collect::<HashSet<_>>();
    let meta = passing
        .iter()
        .filter(|candidate| candidate.category == "meta")
        .map(|candidate| candidate.tag.as_str())
        .collect::<HashSet<_>>();
    let copyright = passing
        .iter()
        .filter(|candidate| candidate.category == "copyright")
        .map(|candidate| candidate.tag.as_str())
        .collect::<Vec<_>>();
    let group = if general
        .iter()
        .any(|tag| matches!(*tag, "text_focus" | "text-only_page" | "fake_screenshot"))
    {
        "截图/文本".to_string()
    } else if meta
        .iter()
        .any(|tag| matches!(*tag, "photo_(medium)" | "off-topic"))
    {
        "非动漫/实景".to_string()
    } else if let Some(copyright) = choose_copyright(&copyright) {
        format!("作品: {copyright}")
    } else if let Some(character) = passing
        .iter()
        .find(|candidate| candidate.category == "character")
    {
        format!("角色: {}", character.tag)
    } else if !copyright.is_empty() {
        "原创".to_string()
    } else {
        "未分类".to_string()
    };
    (rating, group)
}

fn write_auto_tags(
    conn: &Connection,
    asset_id: i64,
    tags: &[TagHit],
    rating: &str,
    group: &str,
    clear_error: bool,
    damage: Option<&str>,
) -> Result<()> {
    conn.execute(
        "DELETE FROM asset_tags WHERE asset_id = ?1 AND origin = 'auto'",
        [asset_id],
    )?;
    for tag in tags {
        let tag_id = upsert_tag(conn, tag)?;
        conn.execute(
            "INSERT INTO asset_tags(asset_id, tag_id, confidence, origin)
             VALUES(?1, ?2, ?3, 'auto')
             ON CONFLICT(asset_id, tag_id, origin) DO UPDATE SET
                confidence = MAX(confidence, excluded.confidence)",
            params![asset_id, tag_id, tag.score as f64],
        )?;
    }
    refresh_auto_tags_json(conn, asset_id)?;
    refresh_tags_text(conn, asset_id)?;
    if clear_error {
        conn.execute(
            "UPDATE assets
             SET group_name = ?1, rating = ?2, tagged = 1, tag_state = 'done',
                 tag_attempts = 0, error = NULL, damage = ?3
             WHERE id = ?4",
            params![group, rating, damage, asset_id],
        )?;
    } else {
        conn.execute(
            "UPDATE assets SET group_name = ?1, rating = ?2 WHERE id = ?3",
            params![group, rating, asset_id],
        )?;
    }
    Ok(())
}

fn recompute_asset_tags(
    conn: &Connection,
    asset_id: i64,
    thresholds: &TagThresholds,
) -> Result<bool> {
    let (candidates, states) = read_candidates(conn, asset_id)?;
    if candidates.is_empty() {
        return Ok(false);
    }
    let auto = auto_candidates(&candidates, &states, thresholds);
    let (rating, group) = derive_rating_group(&auto, thresholds);
    write_auto_tags(conn, asset_id, &auto, &rating, &group, false, None)?;
    Ok(true)
}

fn refresh_tags_text(conn: &Connection, asset_id: i64) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT name FROM (
             SELECT DISTINCT t.name
             FROM asset_tags at JOIN tags t ON t.id = at.tag_id
             WHERE at.asset_id = ?1
             UNION
             SELECT DISTINCT alias.name
             FROM asset_tags at
             JOIN tags t ON t.id = at.tag_id
             JOIN tags alias ON alias.alias_of = t.id
             WHERE at.asset_id = ?1
         ) ORDER BY name",
    )?;
    let rows = statement.query_map([asset_id], |row| row.get::<_, String>(0))?;
    let tags_text = rows.collect::<rusqlite::Result<Vec<_>>>()?.join(" ");
    conn.execute(
        "UPDATE assets SET tags_text = ?1 WHERE id = ?2",
        params![tags_text, asset_id],
    )?;
    Ok(())
}

fn tag_asset_ids(conn: &Connection, tag_id: i64) -> Result<Vec<i64>> {
    let mut statement = conn
        .prepare("SELECT DISTINCT asset_id FROM asset_tags WHERE tag_id = ?1 ORDER BY asset_id")?;
    let rows = statement.query_map([tag_id], |row| row.get::<_, i64>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn refresh_auto_tags_json(conn: &Connection, asset_id: i64) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT t.name, t.category, at.confidence, t.vocab_index
         FROM asset_tags at JOIN tags t ON t.id = at.tag_id
         WHERE at.asset_id = ?1 AND at.origin = 'auto'
         ORDER BY at.confidence DESC, t.name",
    )?;
    let rows = statement.query_map([asset_id], |row| {
        Ok(TagHit {
            tag: row.get(0)?,
            category: row.get(1)?,
            score: row.get::<_, f64>(2)? as f32,
            vocab_index: row.get::<_, Option<i64>>(3)?.map(|value| value as u32),
        })
    })?;
    let tags = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    conn.execute(
        "UPDATE assets SET tags_json = ?1 WHERE id = ?2",
        params![serde_json::to_string(&tags)?, asset_id],
    )?;
    Ok(())
}

fn upsert_tag(conn: &Connection, tag: &TagHit) -> Result<i64> {
    let existing: Option<(i64, Option<i64>)> = conn
        .query_row(
            "SELECT id, alias_of FROM tags WHERE name = ?1",
            [&tag.tag],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((id, alias_of)) = existing {
        // 旧名/被合并名只作为别名的入口，不再把资产挂到已合并的标签上。
        if let Some(canonical) = alias_of {
            return Ok(canonical);
        }
        conn.execute(
            "UPDATE tags SET category = ?1,
                vocab_index = COALESCE(?2, vocab_index)
             WHERE id = ?3",
            params![tag.category, tag.vocab_index.map(|value| value as i64), id],
        )?;
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO tags(name, category, vocab_index) VALUES(?1, ?2, ?3)",
        params![
            tag.tag,
            tag.category,
            tag.vocab_index.map(|value| value as i64)
        ],
    )?;
    conn.query_row("SELECT id FROM tags WHERE name = ?1", [&tag.tag], |row| {
        row.get(0)
    })
    .map_err(Into::into)
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        // F-040：视频容器 MIME；只做展示/导出用，不影响解码路径。
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("mkv") => "video/x-matroska",
        Some("webm") => "video/webm",
        Some("avi") => "video/x-msvideo",
        Some("wmv") => "video/x-ms-wmv",
        Some("flv") => "video/x-flv",
        Some("mpg") | Some("mpeg") | Some("vob") => "video/mpeg",
        Some("3gp") => "video/3gpp",
        Some("3g2") => "video/3gpp2",
        Some("ts") | Some("m2ts") | Some("mts") => "video/mp2t",
        Some("ogv") => "video/ogg",
        _ => "application/octet-stream",
    }
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn table_row_count(conn: &Connection, name: &str) -> Result<i64> {
    conn.query_row(&format!("SELECT COUNT(*) FROM \"{name}\""), [], |row| {
        row.get(0)
    })
    .with_context(|| format!("统计表 {name} 行数"))
}

fn migration_version(path: &Path) -> Result<Option<i64>> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open(path)?;
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap_or(0);
    if version >= SCHEMA_VERSION {
        return Ok(None);
    }
    let has_assets: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'assets'",
        [],
        |row| row.get(0),
    )?;
    if has_assets == 0 {
        return Ok(None);
    }
    Ok(Some(version))
}

fn backup_database(path: &Path, version: i64) -> Result<()> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let file_name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "kanalib.db".into());
    let backup = path.with_file_name(format!("{file_name}.bak-v{version}-{stamp}"));
    fs::copy(path, &backup).with_context(|| format!("backup database to {}", backup.display()))?;
    for suffix in ["-wal", "-shm"] {
        let mut source = path.as_os_str().to_owned();
        source.push(suffix);
        let source = PathBuf::from(source);
        if source.exists() {
            let mut target = backup.as_os_str().to_owned();
            target.push(suffix);
            fs::copy(&source, PathBuf::from(target))?;
        }
    }
    log::warn!(
        "migrating database from v{version}; backup at {}",
        backup.display()
    );
    Ok(())
}

fn ensure_asset_columns(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare("PRAGMA table_info(assets)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|column| column == "tag_attempts") {
        conn.execute_batch(
            "ALTER TABLE assets ADD COLUMN tag_attempts INTEGER NOT NULL DEFAULT 0;
             UPDATE assets SET tag_attempts = 3 WHERE tag_state = 'error';",
        )?;
    }
    if !columns.iter().any(|column| column == "favorite") {
        conn.execute_batch("ALTER TABLE assets ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0;")?;
    }
    if !columns.iter().any(|column| column == "star") {
        conn.execute_batch("ALTER TABLE assets ADD COLUMN star INTEGER NOT NULL DEFAULT 0;")?;
    }
    if !columns.iter().any(|column| column == "damage") {
        conn.execute_batch("ALTER TABLE assets ADD COLUMN damage TEXT;")?;
    }
    if !columns.iter().any(|column| column == "exif_state") {
        conn.execute_batch("ALTER TABLE assets ADD COLUMN exif_state TEXT NOT NULL DEFAULT '';")?;
    }
    if !columns.iter().any(|column| column == "captured_at") {
        conn.execute_batch("ALTER TABLE assets ADD COLUMN captured_at INTEGER;")?;
    }
    if !columns.iter().any(|column| column == "metadata_json") {
        conn.execute_batch(
            "ALTER TABLE assets ADD COLUMN metadata_json TEXT NOT NULL DEFAULT '{}';",
        )?;
    }
    // v10：`migrate` 已加列，这里再按列名守卫一次，覆盖「user_version 已写、
    // ALTER 前进程被杀」的窗口，以及跳过 migrate 的只读/手工打开。
    ensure_media_kind_column(conn)?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_assets_captured_at ON assets(captured_at);",
    )?;
    Ok(())
}

/// v9 → v10：补 `assets.media_kind`。
///
/// 老库（v9 及更早）没有这一列，旧行一律按 `'image'` 解释：`ALTER TABLE ... NOT NULL
/// DEFAULT 'image'` 让既有行读出该默认值，不需要回填或重写全表。列已存在时不做任何事，
/// 因此迁移中途失败后重开可以继续。
fn ensure_media_kind_column(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare("PRAGMA table_info(assets)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if columns.iter().any(|column| column == "media_kind") {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE assets ADD COLUMN media_kind TEXT NOT NULL DEFAULT 'image';")?;
    log::info!("v10: added assets.media_kind (existing rows are images)");
    Ok(())
}

/// 搜索/筛选下推使用的幂等索引；不改变 schema 版本，重复打开无副作用。
fn ensure_search_indexes(conn: &Connection) -> Result<()> {
    let separator = std::path::MAIN_SEPARATOR as u32;
    // 非名称排序的覆盖索引：列顺序与 `sort_primary_expr` + ORDER BY 完全一致，
    // 使命中很多的查询可以直接按索引顺序扫描并在取满一页后停止，不再对全部
    // 命中做临时 B 树排序。只建默认的降序方向（所有非名称排序的 UI 默认值）；
    // 升序仍走临时排序。索引里不重复写 `id`：普通索引自带 rowid 作为最后的
    // tie-breaker，且隐式 rowid 升序正好等于 `assets.id ASC`。
    conn.execute_batch(&format!(
        "CREATE INDEX IF NOT EXISTS idx_assets_sort_modified_desc
             ON assets(mtime DESC, lower(name) ASC, replace(path, char({separator}), char(1)) ASC);
         CREATE INDEX IF NOT EXISTS idx_assets_sort_captured_desc
             ON assets(COALESCE(captured_at, mtime) DESC, lower(name) ASC,
                       replace(path, char({separator}), char(1)) ASC);
         CREATE INDEX IF NOT EXISTS idx_assets_sort_size_desc
             ON assets(bytes DESC, lower(name) ASC, replace(path, char({separator}), char(1)) ASC);
         CREATE INDEX IF NOT EXISTS idx_assets_sort_rating_desc
             ON assets(CASE lower(rating) WHEN 'g' THEN 1 WHEN 's' THEN 2
                          WHEN 'q' THEN 3 WHEN 'e' THEN 4 ELSE 0 END DESC,
                       lower(name) ASC, replace(path, char({separator}), char(1)) ASC);"
    ))?;
    // 第四轮基准显示低基数的 Star/Favorite 与收益不足的 Resolution 用覆盖
    // 索引反而更慢；清掉开发/基准库可能留下的旧索引，避免查询计划退化。
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_assets_sort_resolution_desc;
         DROP INDEX IF EXISTS idx_assets_sort_star_desc;
         DROP INDEX IF EXISTS idx_assets_sort_favorite_desc;",
    )?;
    conn.execute_batch(&format!(
        "CREATE INDEX IF NOT EXISTS idx_assets_folder_path ON assets(folder, path);
         CREATE INDEX IF NOT EXISTS idx_assets_rating_nocase ON assets(rating COLLATE NOCASE);
         CREATE INDEX IF NOT EXISTS idx_assets_star ON assets(star);
         CREATE INDEX IF NOT EXISTS idx_assets_favorite ON assets(favorite);
         CREATE INDEX IF NOT EXISTS idx_assets_tagged ON assets(tagged);
         CREATE INDEX IF NOT EXISTS idx_assets_group_name ON assets(group_name);
         CREATE INDEX IF NOT EXISTS idx_assets_bytes ON assets(bytes);
         CREATE INDEX IF NOT EXISTS idx_assets_width ON assets(width);
         CREATE INDEX IF NOT EXISTS idx_assets_height ON assets(height);
         CREATE INDEX IF NOT EXISTS idx_assets_mtime ON assets(mtime);
         CREATE INDEX IF NOT EXISTS idx_assets_error_not_null ON assets(error) WHERE error IS NOT NULL;
         CREATE INDEX IF NOT EXISTS idx_assets_damage_not_null ON assets(damage) WHERE damage IS NOT NULL;
         CREATE INDEX IF NOT EXISTS idx_assets_name_lower
             ON assets(lower(name), replace(path, char({separator}), char(1)), id);"
    ))?;
    Ok(())
}

fn ensure_tag_columns(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare("PRAGMA table_info(tags)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    if !columns.iter().any(|column| column == "alias_of") {
        conn.execute_batch("ALTER TABLE tags ADD COLUMN alias_of INTEGER;")?;
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_tags_alias_of ON tags(alias_of);")?;
    Ok(())
}

/// 缩略图缓存淘汰后，用这张表把丢失的 512 档从原图按需重建。
fn ensure_thumb_sources(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS thumb_sources (
            thumb TEXT PRIMARY KEY,
            source TEXT NOT NULL,
            mtime INTEGER NOT NULL DEFAULT 0,
            bytes INTEGER NOT NULL DEFAULT 0,
            orientation INTEGER NOT NULL DEFAULT 1
        );",
    )?;
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM thumb_sources", [], |row| row.get(0))?;
    if count == 0 {
        conn.execute_batch(
            "INSERT OR IGNORE INTO thumb_sources(thumb, source, mtime, bytes, orientation)
             SELECT thumb, path, mtime, bytes,
                    COALESCE(CAST(json_extract(metadata_json, '$.orientation') AS INTEGER), 1)
             FROM assets WHERE thumb <> '';",
        )?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct SavedSearchPayload {
    filter: Filter,
    #[serde(default)]
    query: String,
}

/// 集合查询列顺序必须与 `row_to_saved_search` 一一对应。
const SAVED_SEARCH_COLUMNS: &str = "id, name, query_json, sort_key, sort_ascending, view_mode, \
     pane_mode, show_sidebar, show_extensions, pinned, position, updated_at";

/// 旧行的 `query_json` 只含 filter/query；解析失败时退回「全部 + 空搜索」而不是报错，
/// 与 v8 之前的读取行为一致。
fn row_to_saved_search(row: &rusqlite::Row<'_>) -> rusqlite::Result<SavedSearch> {
    let payload: Option<SavedSearchPayload> = serde_json::from_str(&row.get::<_, String>(2)?).ok();
    Ok(SavedSearch {
        id: row.get(0)?,
        name: row.get(1)?,
        filter: payload
            .as_ref()
            .map(|payload| payload.filter.clone())
            .unwrap_or(Filter::All),
        query: payload.map(|payload| payload.query).unwrap_or_default(),
        view: SavedSearchView {
            sort_key: row.get(3)?,
            sort_ascending: row.get::<_, i64>(4)? != 0,
            view_mode: row.get(5)?,
            pane_mode: row.get(6)?,
            show_sidebar: row.get::<_, i64>(7)? != 0,
            show_extensions: row.get::<_, i64>(8)? != 0,
        },
        pinned: row.get::<_, i64>(9)? != 0,
        position: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

/// v9 集合列：逐列检查后补齐，保证迁移中途失败后重开可以继续而不是报重复列。
fn ensure_saved_search_columns(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare("PRAGMA table_info(saved_searches)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for (name, declaration) in SAVED_SEARCH_V9_COLUMNS {
        if !columns.iter().any(|column| column == name) {
            conn.execute_batch(&format!(
                "ALTER TABLE saved_searches ADD COLUMN {name} {declaration};"
            ))?;
        }
    }
    // 迁移前的集合没有手动顺序：用 id 作为初始顺序；新集合写入 position >= 1，不受影响。
    conn.execute_batch("UPDATE saved_searches SET position = id WHERE position = 0;")?;
    Ok(())
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap_or(0);
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    let mut statement = conn
        .prepare("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'assets'")?;
    let has_assets: i64 = statement.query_row([], |row| row.get(0))?;
    let mut old = false;
    if has_assets > 0 {
        let mut statement = conn.prepare("PRAGMA table_info(assets)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        old = !columns
            .iter()
            .any(|column| column == "id" || column == "sha1");
    }
    if old {
        migrate_v1(conn)?;
    } else {
        conn.execute_batch(SCHEMA_V2)?;
    }
    conn.execute_batch(SCHEMA_V4)?;
    conn.execute_batch(SCHEMA_V5)?;
    conn.execute_batch(SCHEMA_V8)?;
    ensure_saved_search_columns(conn)?;
    conn.execute_batch(SCHEMA_V9)?;
    // v10（F-040）：视频作为新的 media_kind 需要 assets.media_kind；v9 库在此补列，
    // 既有行保持 'image'（读取语义不变），新库的表在 SCHEMA_V2 里已带该列。
    ensure_media_kind_column(conn)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn migrate_v1(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT path, folder, width, height, bytes, mtime, thumb, group_name, rating,
                tags_json, tagged, error
         FROM assets",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(OldAsset {
                path: row.get(0)?,
                folder: row.get(1)?,
                width: row.get(2)?,
                height: row.get(3)?,
                bytes: row.get(4)?,
                mtime: row.get(5)?,
                thumb: row.get(6)?,
                group_name: row.get(7)?,
                rating: row.get(8)?,
                tags_json: row.get(9)?,
                tagged: row.get(10)?,
                error: row.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    let tx = conn.unchecked_transaction()?;
    tx.execute_batch("DROP INDEX IF EXISTS idx_assets_folder;")?;
    tx.execute_batch("ALTER TABLE assets RENAME TO assets_v1;")?;
    tx.execute_batch(SCHEMA_V2)?;

    let folders = rows
        .iter()
        .map(|row| row.folder.clone())
        .collect::<HashSet<_>>();
    let mut source_ids = HashMap::new();
    for folder in folders {
        let config = serde_json::json!({ "path": folder }).to_string();
        tx.execute(
            "INSERT OR IGNORE INTO sources(kind, name, locator, config_json, read_only)
             VALUES('local', ?1, ?1, ?2, 1)",
            params![folder, config],
        )?;
        let id: i64 = tx.query_row(
            "SELECT id FROM sources WHERE kind = 'local' AND locator = ?1",
            [&folder],
            |row| row.get(0),
        )?;
        source_ids.insert(folder, id);
    }

    for row in rows {
        let tags = serde_json::from_str::<Vec<TagHit>>(&row.tags_json).unwrap_or_default();
        let tags_text = tags
            .iter()
            .map(|hit| hit.tag.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let name = Path::new(&row.path)
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| row.path.clone());
        let source_id = source_ids
            .get(&row.folder)
            .copied()
            .context("missing local source during migration")?;
        tx.execute(
            "INSERT INTO assets(
                source_id, path, folder, name, media_kind, mime, width, height, bytes, mtime,
                thumb, group_name, rating, tags_json, tags_text, tagged, error
             ) VALUES(?1, ?2, ?3, ?4, 'image', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                source_id,
                row.path,
                row.folder,
                name,
                mime_for(Path::new(&row.path)),
                row.width,
                row.height,
                row.bytes,
                row.mtime,
                row.thumb,
                row.group_name,
                row.rating,
                row.tags_json,
                tags_text,
                row.tagged,
                row.error,
            ],
        )?;
        let asset_id = tx.last_insert_rowid();
        for tag in &tags {
            let tag_id = upsert_tag(&tx, tag)?;
            tx.execute(
                "INSERT OR IGNORE INTO asset_tags(asset_id, tag_id, confidence, origin)
                 VALUES(?1, ?2, ?3, 'auto')",
                params![asset_id, tag_id, tag.score as f64],
            )?;
        }
    }
    tx.execute_batch("DROP TABLE assets_v1;")?;
    tx.commit()?;
    conn.execute("INSERT INTO assets_fts(assets_fts) VALUES('rebuild')", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Db, NameFold, SearchPageRequest, SearchSort, SortPlan, sort_cover_index, sort_primary_expr,
    };
    use crate::dedup::{DuplicateReview, ReviewState};
    use crate::media::ContentHashes;
    use crate::model::{Asset, Filter, MediaKind, SearchQuery, TagHit, TagReviewState};
    use rusqlite::Connection;
    use rusqlite::types::Value;
    use std::collections::HashSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn adds_damage_column_to_existing_assets_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE assets (
                id INTEGER PRIMARY KEY,
                path TEXT,
                tag_attempts INTEGER NOT NULL DEFAULT 0,
                favorite INTEGER NOT NULL DEFAULT 0,
                star INTEGER NOT NULL DEFAULT 0,
                tag_state TEXT NOT NULL DEFAULT ''
            );",
        )
        .unwrap();
        super::ensure_asset_columns(&conn).unwrap();
        let has_damage: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('assets') WHERE name = 'damage'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_damage, 1);
    }

    #[test]
    fn moving_asset_keeps_tags_favorite_and_star() {
        let root =
            std::env::temp_dir().join(format!("kanalib-rename-asset-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db = Db::open(&root.join("kanalib.db")).unwrap();
        let old = PathBuf::from("C:\\lib\\old.png");
        let new = PathBuf::from("C:\\lib\\renamed.png");
        let hashes = ContentHashes {
            sha1: "c".repeat(40),
            blake3: "d".repeat(64),
            phash: 42,
            decode_ok: true,
            decode_error: None,
        };
        db.upsert_meta("C:\\lib", &old, None, 10, 20, 100, 10, &hashes, false)
            .unwrap();
        let mut asset = db.load_asset(&old).unwrap();
        asset.tagged = true;
        asset.group = "角色: 测试".to_string();
        asset.rating = "S".to_string();
        asset.tags = vec![crate::model::TagHit {
            tag: "1girl".to_string(),
            category: "general".to_string(),
            score: 0.99,
            vocab_index: None,
        }];
        db.update_tags(&asset).unwrap();
        db.set_favorite(std::slice::from_ref(&old), true).unwrap();
        db.set_star(std::slice::from_ref(&old), 4).unwrap();
        db.add_manual_tags(std::slice::from_ref(&old), "manual_tag", "manual")
            .unwrap();

        let moved = db
            .move_asset_media(&old, &new, "C:\\lib", None, 10, 20, 100, 10, &hashes)
            .unwrap();
        assert_eq!(moved.path, new);
        assert!(moved.tagged);
        assert_eq!(moved.group, "角色: 测试");
        assert_eq!(moved.rating, "S");
        assert!(moved.favorite);
        assert_eq!(moved.star, 4);
        assert!(moved.tags.iter().any(|tag| tag.tag == "1girl"));
        assert!(moved.manual_tags.iter().any(|tag| tag.tag == "manual_tag"));
        assert!(!db.asset_paths("C:\\lib").unwrap().contains(&old));
        assert!(
            db.asset_paths_by_sha1("C:\\lib", &hashes.sha1)
                .unwrap()
                .contains(&new)
        );

        drop(db);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn renaming_and_merging_tags_keep_aliases_and_fts() {
        let root =
            std::env::temp_dir().join(format!("kanalib-tag-alias-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db = Db::open(&root.join("kanalib.db")).unwrap();
        let path = PathBuf::from("C:\\lib\\a.png");
        let hashes = ContentHashes {
            sha1: "a".repeat(40),
            blake3: "b".repeat(64),
            phash: 7,
            decode_ok: true,
            decode_error: None,
        };
        db.upsert_meta("C:\\lib", &path, None, 10, 20, 100, 10, &hashes, false)
            .unwrap();
        let mut asset = db.load_asset(&path).unwrap();
        asset.tagged = true;
        asset.tags = vec![
            crate::model::TagHit {
                tag: "1girl".into(),
                category: "general".into(),
                score: 0.9,
                vocab_index: None,
            },
            crate::model::TagHit {
                tag: "long_hair".into(),
                category: "general".into(),
                score: 0.8,
                vocab_index: None,
            },
        ];
        db.update_tags(&asset).unwrap();
        db.add_manual_tags(std::slice::from_ref(&path), "manual_tag", "manual")
            .unwrap();

        assert_eq!(db.rename_tag("long_hair", "longhair").unwrap(), 1);
        let asset = db.load_asset(&path).unwrap();
        assert!(asset.tags.iter().any(|tag| tag.tag == "longhair"));
        assert!(!asset.tags.iter().any(|tag| tag.tag == "long_hair"));
        assert_eq!(
            db.load_tag_aliases().unwrap().get("long_hair"),
            Some(&"longhair".to_string())
        );
        assert!(
            !db.search_paths_in(&["C:\\lib".into()], &["long_hair".into()])
                .unwrap()
                .is_empty()
        );
        assert!(
            !db.search_paths_in(&["C:\\lib".into()], &["longhair".into()])
                .unwrap()
                .is_empty()
        );

        assert_eq!(db.merge_tags("longhair", "1girl").unwrap(), 1);
        let asset = db.load_asset(&path).unwrap();
        assert!(asset.tags.iter().any(|tag| tag.tag == "1girl"));
        assert!(!asset.tags.iter().any(|tag| tag.tag == "longhair"));
        assert!(asset.manual_tags.iter().any(|tag| tag.tag == "manual_tag"));
        let aliases = db.load_tag_aliases().unwrap();
        assert_eq!(aliases.get("longhair"), Some(&"1girl".to_string()));
        assert_eq!(aliases.get("long_hair"), Some(&"1girl".to_string()));
        assert!(
            !db.search_paths_in(&["C:\\lib".into()], &["long_hair".into()])
                .unwrap()
                .is_empty()
        );
        drop(db);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn migrates_legacy_schema_and_searches() {
        let root = std::env::temp_dir().join(format!(
            "kanalib-db-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db_path = root.join("kanalib.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE assets (
                    path TEXT PRIMARY KEY,
                    folder TEXT NOT NULL,
                    width INTEGER NOT NULL DEFAULT 0,
                    height INTEGER NOT NULL DEFAULT 0,
                    bytes INTEGER NOT NULL DEFAULT 0,
                    mtime INTEGER NOT NULL DEFAULT 0,
                    thumb TEXT NOT NULL DEFAULT '',
                    group_name TEXT NOT NULL DEFAULT '',
                    rating TEXT NOT NULL DEFAULT '',
                    tags_json TEXT NOT NULL DEFAULT '[]',
                    tagged INTEGER NOT NULL DEFAULT 0,
                    error TEXT
                );
                CREATE INDEX idx_assets_folder ON assets(folder);
                "#,
            )
            .unwrap();
            let tags_a = r#"[{"tag":"1girl","category":"general","score":0.99},{"tag":"克拉拉","category":"character","score":0.9}]"#;
            let tags_b = r#"[{"tag":"sky","category":"general","score":0.8}]"#;
            conn.execute(
                "INSERT INTO assets(path, folder, width, height, bytes, mtime, tags_json, tagged)
                 VALUES(?1, 'C:\\lib', 100, 200, 300, 400, ?2, 1)",
                rusqlite::params!["C:\\lib\\克拉拉.png", tags_a],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO assets(path, folder, width, height, bytes, mtime, tags_json, tagged)
                 VALUES(?1, 'C:\\lib', 10, 20, 30, 40, ?2, 1)",
                rusqlite::params!["C:\\lib\\other.png", tags_b],
            )
            .unwrap();
        }

        {
            let db = Db::open(&db_path).unwrap();
            let assets = db.load_folder("C:\\lib").unwrap();
            assert_eq!(assets.len(), 2);
            let clara = assets
                .iter()
                .find(|asset| asset.path.ends_with("克拉拉.png"))
                .expect("migrated clara asset");
            assert_eq!(clara.tags.len(), 2);
            let hits = db.search_paths("C:\\lib", &["1girl".to_string()]).unwrap();
            assert_eq!(
                hits,
                std::collections::HashSet::from([PathBuf::from("C:\\lib\\克拉拉.png")])
            );
            let hits = db.search_paths("C:\\lib", &["克拉拉".to_string()]).unwrap();
            assert!(hits.contains(&PathBuf::from("C:\\lib\\克拉拉.png")));
            let hits = db.search_paths("C:\\lib", &["克拉".to_string()]).unwrap();
            assert!(hits.contains(&PathBuf::from("C:\\lib\\克拉拉.png")));
            let conn = Connection::open(&db_path).unwrap();
            let tag_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM tags", [], |row| row.get(0))
                .unwrap();
            assert_eq!(tag_count, 3);
            assert_eq!(db.tags_missing_vocab().unwrap().len(), 3);
            db.set_vocab_indices(&[("1girl".to_string(), 0)]).unwrap();
            assert_eq!(db.tags_missing_vocab().unwrap().len(), 2);

            let clara_path = PathBuf::from("C:\\lib\\克拉拉.png");
            db.set_favorite(&[clara_path.clone()], true).unwrap();
            db.set_star(&[clara_path.clone()], 4).unwrap();
            let clara = db
                .load_folder("C:\\lib")
                .unwrap()
                .into_iter()
                .find(|asset| asset.path.ends_with("克拉拉.png"))
                .expect("starred clara asset");
            assert!(clara.favorite);
            assert_eq!(clara.star, 4);

            db.add_manual_tags(&[clara_path.clone()], "manualtag", "manual")
                .unwrap();
            let clara = db
                .load_folder("C:\\lib")
                .unwrap()
                .into_iter()
                .find(|asset| asset.path.ends_with("克拉拉.png"))
                .expect("manual tagged clara");
            assert_eq!(clara.manual_tags.len(), 1);
            assert_eq!(clara.manual_tags[0].tag, "manualtag");
            assert_eq!(clara.manual_tags[0].category, "manual");
            let hits = db
                .search_paths("C:\\lib", &["manualtag".to_string()])
                .unwrap();
            assert!(hits.contains(&clara_path));
            let summary = db
                .list_tags("C:\\lib")
                .unwrap()
                .into_iter()
                .find(|tag| tag.name == "manualtag")
                .expect("manual tag summary");
            assert_eq!((summary.count, summary.manual_count), (1, 1));
            assert_eq!(
                db.remove_tags(&[clara_path.clone()], "manualtag").unwrap(),
                1
            );
            let clara = db
                .load_folder("C:\\lib")
                .unwrap()
                .into_iter()
                .find(|asset| asset.path.ends_with("克拉拉.png"))
                .expect("clara without manual tag");
            assert!(clara.manual_tags.is_empty());
            let hits = db
                .search_paths("C:\\lib", &["manualtag".to_string()])
                .unwrap();
            assert!(!hits.contains(&clara_path));

            let saved_id = db
                .save_saved_search("收藏", &Filter::Favorites, "is:fav")
                .unwrap();
            let saved = db.load_saved_searches().unwrap();
            assert_eq!(saved.len(), 1);
            assert_eq!(saved[0].name, "收藏");
            assert_eq!(saved[0].filter, Filter::Favorites);
            assert_eq!(saved[0].query, "is:fav");
            assert_eq!(db.delete_saved_search(saved_id).unwrap(), 1);
            assert!(db.load_saved_searches().unwrap().is_empty());

            let attempts = db
                .mark_error(Path::new("C:\\lib\\other.png"), "boom", None)
                .unwrap();
            assert_eq!(attempts, 1);
            let attempts = db
                .mark_error(Path::new("C:\\lib\\other.png"), "boom", None)
                .unwrap();
            assert_eq!(attempts, 2);
            let other = db
                .load_folder("C:\\lib")
                .unwrap()
                .into_iter()
                .find(|asset| asset.path.ends_with("other.png"))
                .expect("other asset");
            assert_eq!(other.tag_attempts, 2);

            let updated = db
                .upsert_meta(
                    "C:\\lib",
                    Path::new("C:\\lib\\other.png"),
                    None,
                    10,
                    20,
                    31,
                    41,
                    &ContentHashes {
                        sha1: "a".repeat(40),
                        blake3: "b".repeat(64),
                        phash: 1,
                        decode_ok: true,
                        decode_error: None,
                    },
                    true,
                )
                .unwrap();
            assert_eq!(updated.tag_attempts, 0);

            let retried = db.retry_failed("C:\\lib").unwrap();
            assert_eq!(retried, 1);
            let other = db
                .load_folder("C:\\lib")
                .unwrap()
                .into_iter()
                .find(|asset| asset.path.ends_with("other.png"))
                .expect("other asset");
            assert_eq!(other.tag_attempts, 0);
            assert!(other.error.is_none());
        }
        {
            let db = Db::open(&db_path).unwrap();
            assert_eq!(db.load_folder("C:\\lib").unwrap().len(), 2);
            let review = DuplicateReview {
                group_key: "aaa".to_string(),
                state: ReviewState::Done,
                keeper_path: Some(PathBuf::from("C:\\lib\\other.png")),
                member_count: 2,
                signature: "sig".to_string(),
            };
            db.save_duplicate_review(&review).unwrap();
            let loaded = db.load_duplicate_reviews().unwrap();
            let saved = loaded.get("aaa").expect("saved duplicate review");
            assert_eq!(saved.state, ReviewState::Done);
            assert_eq!(
                saved.keeper_path.as_deref(),
                Some(Path::new("C:\\lib\\other.png"))
            );

            let removed = db
                .delete_assets(&[PathBuf::from("C:\\lib\\other.png")])
                .unwrap();
            assert_eq!(removed, 1);
            assert_eq!(db.load_folder("C:\\lib").unwrap().len(), 1);
            let conn = Connection::open(&db_path).unwrap();
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, super::SCHEMA_VERSION);
        }
        assert!(fs::read_dir(&root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".bak-v")
        }));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tag_job_queue_persists_and_recovers() {
        let root = std::env::temp_dir().join(format!("kanalib-jobs-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db_path = root.join("kanalib.db");
        let hashes = |seed: &str| ContentHashes {
            sha1: format!("{seed:0<40}"),
            blake3: format!("{seed:0<64}"),
            phash: 1,
            decode_ok: true,
            decode_error: None,
        };
        {
            let db = Db::open(&db_path).unwrap();
            for name in ["a.png", "b.png"] {
                let path = format!("C:\\lib\\{name}");
                db.upsert_meta(
                    "C:\\lib",
                    Path::new(&path),
                    None,
                    10,
                    10,
                    100,
                    10,
                    &hashes(name),
                    false,
                )
                .unwrap();
            }
            db.enqueue_tag_jobs("C:\\lib").unwrap();
            let counts = db.tag_job_counts("C:\\lib").unwrap();
            assert_eq!((counts.pending, counts.running, counts.error), (2, 0, 0));

            let first = db.claim_tag_jobs("C:\\lib", 1).unwrap();
            assert_eq!(first.len(), 1);
            let counts = db.tag_job_counts("C:\\lib").unwrap();
            assert_eq!((counts.pending, counts.running), (1, 1));
            let all = db.claim_tag_jobs("C:\\lib", 10).unwrap();
            assert_eq!(all.len(), 1);
            db.recover_tag_jobs().unwrap();
            let counts = db.tag_job_counts("C:\\lib").unwrap();
            assert_eq!((counts.pending, counts.running, counts.error), (2, 0, 0));

            let jobs = db.claim_tag_jobs("C:\\lib", 10).unwrap();
            assert_eq!(jobs.len(), 2);
            db.complete_tag_job(jobs[0].id).unwrap();
            db.fail_tag_job(jobs[1].id, "boom").unwrap();
            let counts = db.tag_job_counts("C:\\lib").unwrap();
            assert_eq!((counts.pending, counts.running, counts.error), (0, 0, 1));
        }
        {
            let db = Db::open(&db_path).unwrap();
            let counts = db.tag_job_counts("C:\\lib").unwrap();
            assert_eq!((counts.pending, counts.running, counts.error), (0, 0, 1));
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn media_kind_and_mime_are_persisted_and_videos_skip_tag_queue() {
        let root = temp_root("media-kind");
        let db_path = root.join("kanalib.db");
        {
            let db = Db::open(&db_path).unwrap();
            let video = PathBuf::from("C:\\lib\\clip.MP4");
            let image = PathBuf::from("C:\\lib\\photo.png");
            let video_hashes = ContentHashes {
                sha1: "a".repeat(40),
                blake3: "b".repeat(64),
                phash: 0,
                decode_ok: false,
                decode_error: None,
            };
            db.upsert_meta(
                "C:\\lib",
                &video,
                None,
                320,
                240,
                100,
                10,
                &video_hashes,
                false,
            )
            .unwrap();
            db.upsert_meta("C:\\lib", &image, None, 10, 20, 100, 10, &hash('c'), false)
                .unwrap();

            let stored = db.stored_meta(&video).unwrap().unwrap();
            assert_eq!(stored.media_kind, MediaKind::Video);
            assert!(stored.phash.is_none(), "videos never store a pHash");
            let reused = stored.content_if_current(10, 100).unwrap();
            assert_eq!(reused.phash, 0);
            assert!(!reused.decode_ok);

            let image_stored = db.stored_meta(&image).unwrap().unwrap();
            assert_eq!(image_stored.media_kind, MediaKind::Image);
            assert!(db.load_asset(&video).unwrap().is_video());
            assert!(!db.load_asset(&image).unwrap().is_video());

            let (kind, mime): (String, String) = db
                .conn
                .query_row(
                    "SELECT media_kind, mime FROM assets WHERE path = ?1",
                    [video.to_string_lossy().to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!((kind.as_str(), mime.as_str()), ("video", "video/mp4"));

            // 视频不进入批量打标队列；图片仍按原逻辑排队。
            db.enqueue_tag_jobs("C:\\lib").unwrap();
            let jobs = db.claim_tag_jobs("C:\\lib", 10).unwrap();
            assert!(jobs.iter().all(|job| job.path != video));
            assert!(jobs.iter().any(|job| job.path == image));
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// v9 库没有 `assets.media_kind`：升级后旧图必须默认 image，视频可写可读，
    /// 且 `stored_meta`（读取 media_kind 的查询）不能报 no such column。
    #[test]
    fn migrates_v9_to_v10_and_keeps_existing_rows_as_images() {
        let root = temp_root("v9-v10");
        let db_path = root.join("kanalib.db");
        let image = PathBuf::from("C:\\lib\\old.png");
        let video = PathBuf::from("C:\\lib\\clip.mp4");
        let image_text = image.to_string_lossy().to_string();
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(super::SCHEMA_V2).unwrap();
            conn.execute_batch(super::SCHEMA_V4).unwrap();
            conn.execute_batch(super::SCHEMA_V5).unwrap();
            conn.execute_batch(super::SCHEMA_V8).unwrap();
            super::ensure_saved_search_columns(&conn).unwrap();
            conn.execute_batch(super::SCHEMA_V9).unwrap();
            conn.execute(
                "INSERT INTO sources(kind, name, locator) VALUES('local', '图库', 'C:\\lib')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO assets(
                    source_id, path, folder, name, media_kind, mime, width, height, bytes,
                    mtime, thumb, sha1, blake3, phash
                 ) VALUES(1, ?1, 'C:\\lib', 'old.png', 'image', 'image/png', 10, 20, 100, 10,
                    '', ?2, ?3, 7)",
                rusqlite::params![image_text, "a".repeat(40), "b".repeat(64)],
            )
            .unwrap();
            // 还原成 v9 的真实形态：assets 表没有 media_kind 列。
            conn.execute_batch("ALTER TABLE assets DROP COLUMN media_kind;")
                .unwrap();
            conn.pragma_update(None, "user_version", 9).unwrap();
        }

        let db = Db::open(&db_path).unwrap();
        {
            let conn = Connection::open(&db_path).unwrap();
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, 10, "v9 must upgrade to v10");
            assert_eq!(version, super::SCHEMA_VERSION);
            let kind: String = conn
                .query_row(
                    "SELECT media_kind FROM assets WHERE path = ?1",
                    [&image_text],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(kind, "image", "existing rows default to image");
        }

        // 读路径：旧图仍是 image，phash 与哈希不变。
        let stored = db.stored_meta(&image).unwrap().unwrap();
        assert_eq!(stored.media_kind, MediaKind::Image);
        assert_eq!(stored.phash, Some(7));
        assert_eq!(
            db.load_asset(&image).unwrap().media_kind(),
            MediaKind::Image
        );

        // 写路径：同一库里的视频写 video，图片写回 image。
        let video_hashes = ContentHashes {
            sha1: "c".repeat(40),
            blake3: "d".repeat(64),
            phash: 0,
            decode_ok: false,
            decode_error: None,
        };
        db.upsert_meta(
            "C:\\lib",
            &video,
            None,
            320,
            240,
            100,
            10,
            &video_hashes,
            false,
        )
        .unwrap();
        assert_eq!(
            db.stored_meta(&video).unwrap().map(|meta| meta.media_kind),
            Some(MediaKind::Video)
        );
        assert!(db.load_asset(&video).unwrap().is_video());

        db.upsert_meta("C:\\lib", &image, None, 10, 20, 100, 10, &hash('e'), false)
            .unwrap();
        assert_eq!(
            db.stored_meta(&image).unwrap().map(|meta| meta.media_kind),
            Some(MediaKind::Image)
        );
        // 视频不进入打标队列；升级后的旧图仍排队。
        db.enqueue_tag_jobs("C:\\lib").unwrap();
        let jobs = db.claim_tag_jobs("C:\\lib", 10).unwrap();
        assert!(jobs.iter().all(|job| job.path != video));
        assert!(jobs.iter().any(|job| job.path == image));
        let _ = fs::remove_dir_all(&root);
    }

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "kanalib-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn hash(seed: char) -> ContentHashes {
        ContentHashes {
            sha1: seed.to_string().repeat(40),
            blake3: seed.to_string().repeat(64),
            phash: 7,
            decode_ok: true,
            decode_error: None,
        }
    }

    fn candidate(tag: &str, category: &str, score: f32) -> crate::model::TagHit {
        crate::model::TagHit {
            tag: tag.to_string(),
            category: category.to_string(),
            score,
            vocab_index: None,
        }
    }

    #[test]
    fn migrates_v7_to_v8_and_preserves_data() {
        let root = temp_root("v7-v8");
        let db_path = root.join("kanalib.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(super::SCHEMA_V2).unwrap();
            conn.execute_batch(super::SCHEMA_V4).unwrap();
            conn.execute_batch(super::SCHEMA_V5).unwrap();
            conn.execute_batch(
                "INSERT INTO sources(kind, name, locator) VALUES('local', 'C:\\lib', 'C:\\lib');",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO assets(
                    source_id, path, folder, name, width, height, bytes, mtime, tagged,
                    tags_json, tags_text
                 ) VALUES(1, 'C:\\lib\\a.png', 'C:\\lib', 'a.png', 10, 20, 100, 10, 1,
                    '[{\"tag\":\"1girl\",\"category\":\"general\",\"score\":0.9}]', '1girl')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tags(name, category, vocab_index, threshold_override)
                 VALUES('1girl', 'general', 0, 0.33)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO asset_tags(asset_id, tag_id, confidence, origin)
                 VALUES(1, 1, 1.0, 'manual')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO settings(key, value) VALUES('theme', 'dark')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO jobs(kind, asset_id, state, attempts) VALUES('tag', 1, 'pending', 0)",
                [],
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 7).unwrap();
        }

        {
            let db = Db::open(&db_path).unwrap();
            let asset = db.load_asset(Path::new("C:\\lib\\a.png")).unwrap();
            assert!(asset.tagged);
            assert!(asset.tags.iter().any(|tag| tag.tag == "1girl"));
            assert!(asset.manual_tags.iter().any(|tag| tag.tag == "1girl"));
            assert_eq!(db.setting("theme").unwrap().as_deref(), Some("dark"));
            assert_eq!(
                db.tag_thresholds().unwrap().overrides.get("1girl").copied(),
                Some(0.33)
            );
            assert_eq!(db.tag_queue_totals(&["C:\\lib".into()]).unwrap().pending, 1);
            let conn = Connection::open(&db_path).unwrap();
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, super::SCHEMA_VERSION);
            let has_scores: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'asset_scores'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(has_scores, 1);
            drop(conn);
        }
        let backups = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".bak-v7-"))
            .count();
        assert_eq!(backups, 1);

        // 再打开一次：版本已是 8，不重复备份，数据保持。
        {
            let db = Db::open(&db_path).unwrap();
            assert_eq!(db.load_folder("C:\\lib").unwrap().len(), 1);
        }
        let backups_again = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".bak-v7-"))
            .count();
        assert_eq!(backups_again, 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn candidates_are_capped_and_threshold_recompute_keeps_manual_tags() {
        let root = temp_root("candidates");
        let db = Db::open(&root.join("kanalib.db")).unwrap();
        let path = PathBuf::from("C:\\lib\\a.png");
        db.upsert_meta("C:\\lib", &path, None, 10, 20, 100, 10, &hash('a'), false)
            .unwrap();
        db.add_manual_tags(std::slice::from_ref(&path), "keep_me", "manual")
            .unwrap();
        let asset = db.load_asset(&path).unwrap();

        let mut candidates = Vec::new();
        for index in 0..60 {
            candidates.push(candidate(
                &format!("g{index:02}"),
                "general",
                0.90 - index as f32 * 0.01,
            ));
        }
        for index in 0..30 {
            candidates.push(candidate(
                &format!("c{index:02}"),
                "character",
                0.80 - index as f32 * 0.01,
            ));
        }
        for index in 0..20 {
            candidates.push(candidate(
                &format!("p{index:02}"),
                "copyright",
                0.70 - index as f32 * 0.005,
            ));
        }
        for index in 0..30 {
            candidates.push(candidate(
                &format!("s{index:02}"),
                "style",
                0.60 - index as f32 * 0.01,
            ));
        }
        for index in 0..20 {
            candidates.push(candidate(
                &format!("m{index:02}"),
                "meta",
                0.50 - index as f32 * 0.01,
            ));
        }
        for (index, (tag, score)) in [
            ("rating:g", 0.90),
            ("rating:s", 0.50),
            ("rating:q", 0.20),
            ("rating:e", 0.10),
            ("rating:x", 0.05),
            ("rating:y", 0.01),
        ]
        .into_iter()
        .enumerate()
        {
            candidates.push(candidate(tag, "rating", score));
            let _ = index;
        }

        let stored = db.store_tag_result(&asset, Some(&candidates)).unwrap();
        let scores = db.asset_candidates(&path).unwrap();
        let count = |category: &str| {
            scores
                .iter()
                .filter(|candidate| candidate.category == category)
                .count()
        };
        assert_eq!(count("general"), 40);
        assert_eq!(count("character"), 30);
        assert_eq!(count("copyright"), 20);
        assert_eq!(count("style"), 30);
        assert_eq!(count("meta"), 20);
        assert_eq!(count("rating"), 4);
        assert!(!scores.iter().any(|candidate| candidate.tag == "g59"));
        assert!(!stored.tags.iter().any(|tag| tag.tag == "keep_me"));
        assert!(stored.manual_tags.iter().any(|tag| tag.tag == "keep_me"));
        assert_eq!(stored.rating, "g");
        assert!(stored.group.starts_with("作品: "));

        db.set_category_threshold("general", 0.545).unwrap();
        let summary = db
            .recompute_thresholds(std::slice::from_ref(&path))
            .unwrap();
        assert_eq!(summary.updated, 1);
        let recomputed = db.load_asset(&path).unwrap();
        assert_eq!(
            recomputed
                .tags
                .iter()
                .filter(|tag| tag.category == "general")
                .count(),
            36
        );
        assert!(
            recomputed
                .manual_tags
                .iter()
                .any(|tag| tag.tag == "keep_me")
        );

        db.set_tag_threshold("g00", Some(0.95)).unwrap();
        db.recompute_thresholds(std::slice::from_ref(&path))
            .unwrap();
        let recomputed = db.load_asset(&path).unwrap();
        assert!(!recomputed.tags.iter().any(|tag| tag.tag == "g00"));
        db.set_tag_threshold("g00", None).unwrap();
        db.recompute_thresholds(std::slice::from_ref(&path))
            .unwrap();
        assert!(
            db.load_asset(&path)
                .unwrap()
                .tags
                .iter()
                .any(|tag| tag.tag == "g00")
        );

        assert_eq!(db.set_category_threshold("general", 2.5).unwrap(), 1.0);
        assert_eq!(db.set_category_threshold("general", -1.0).unwrap(), 0.0);
        assert!(db.set_category_threshold("unknown", 0.2).is_err());
        assert_eq!(
            db.set_tag_threshold("never_used", Some(0.4)).unwrap(),
            Some(0.4)
        );
        assert_eq!(
            db.tag_thresholds()
                .unwrap()
                .overrides
                .get("never_used")
                .copied(),
            Some(0.4)
        );
        drop(db);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn low_confidence_candidates_can_be_accepted_or_ignored() {
        let root = temp_root("review");
        let db = Db::open(&root.join("kanalib.db")).unwrap();
        let path = PathBuf::from("C:\\lib\\a.png");
        db.upsert_meta("C:\\lib", &path, None, 10, 20, 100, 10, &hash('b'), false)
            .unwrap();
        let asset = db.load_asset(&path).unwrap();
        let candidates = vec![
            candidate("hit", "general", 0.90),
            candidate("low", "general", 0.10),
            candidate("original", "copyright", 0.50),
            candidate("rating:g", "rating", 0.80),
        ];
        db.store_tag_result(&asset, Some(&candidates)).unwrap();

        let pending = db.pending_tag_candidates(&["C:\\lib".into()], 10).unwrap();
        assert!(pending.iter().any(|candidate| candidate.tag == "low"));
        assert!(!pending.iter().any(|candidate| candidate.tag == "hit"));

        db.review_tag_candidate(&path, "low", TagReviewState::Accepted)
            .unwrap();
        assert!(
            db.load_asset(&path)
                .unwrap()
                .tags
                .iter()
                .any(|tag| tag.tag == "low")
        );
        assert!(
            db.pending_tag_candidates(&["C:\\lib".into()], 10)
                .unwrap()
                .is_empty()
        );

        db.review_tag_candidate(&path, "low", TagReviewState::Ignored)
            .unwrap();
        let asset = db.load_asset(&path).unwrap();
        assert!(!asset.tags.iter().any(|tag| tag.tag == "low"));
        assert!(asset.tags.iter().any(|tag| tag.tag == "hit"));
        let ignored = db
            .asset_candidates(&path)
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.tag == "low")
            .unwrap();
        assert_eq!(ignored.state, TagReviewState::Ignored);

        let changed = db
            .review_asset_candidates(&path, TagReviewState::Ignored)
            .unwrap();
        assert!(changed >= 3);
        assert!(
            db.pending_tag_candidates(&["C:\\lib".into()], 10)
                .unwrap()
                .is_empty()
        );

        // 阈值一旦降到候选分数以下，待审列表必须立即为空（单标签与类别两条路径）。
        db.review_tag_candidate(&path, "low", TagReviewState::Pending)
            .unwrap();
        assert!(
            db.pending_tag_candidates(&["C:\\lib".into()], 10)
                .unwrap()
                .iter()
                .any(|candidate| candidate.tag == "low")
        );
        db.set_tag_threshold("low", Some(0.05)).unwrap();
        assert!(
            db.pending_tag_candidates(&["C:\\lib".into()], 10)
                .unwrap()
                .is_empty()
        );
        db.set_tag_threshold("low", None).unwrap();
        db.set_category_threshold("general", 0.05).unwrap();
        assert!(
            db.pending_tag_candidates(&["C:\\lib".into()], 10)
                .unwrap()
                .is_empty()
        );
        drop(db);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn selective_retag_uses_candidates_and_queue_is_idempotent() {
        let root = temp_root("retag");
        let db = Db::open(&root.join("kanalib.db")).unwrap();
        let a = PathBuf::from("C:\\lib\\a.png");
        let b = PathBuf::from("C:\\lib\\b.png");
        let c = PathBuf::from("C:\\lib\\c.png");
        for (path, seed) in [(&a, 'c'), (&b, 'd'), (&c, 'e')] {
            db.upsert_meta("C:\\lib", path, None, 10, 20, 100, 10, &hash(seed), false)
                .unwrap();
        }
        db.add_manual_tags(std::slice::from_ref(&a), "keep_me", "manual")
            .unwrap();
        let asset = db.load_asset(&a).unwrap();
        db.store_tag_result(
            &asset,
            Some(&[
                candidate("hit", "general", 0.9),
                candidate("low", "general", 0.1),
            ]),
        )
        .unwrap();

        let summary = db
            .retag_paths(&[
                a.clone(),
                a.clone(),
                b.clone(),
                PathBuf::from("C:\\lib\\missing.png"),
            ])
            .unwrap();
        assert_eq!(summary.recomputed, 1);
        assert_eq!(summary.queued, 1);
        assert_eq!(summary.missing, 1);
        assert_eq!(db.tag_queue_totals(&["C:\\lib".into()]).unwrap().pending, 1);
        assert!(
            db.load_asset(&a)
                .unwrap()
                .manual_tags
                .iter()
                .any(|tag| tag.tag == "keep_me")
        );

        let again = db.retag_paths(&[b.clone()]).unwrap();
        assert_eq!((again.queued, again.recomputed), (1, 0));
        assert_eq!(db.tag_queue_totals(&["C:\\lib".into()]).unwrap().pending, 1);

        db.retag_paths(&[c.clone()]).unwrap();
        assert_eq!(db.prioritize_tag_jobs(&[b.clone()], 10).unwrap(), 1);
        let jobs = db.claim_tag_jobs("C:\\lib", 10).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].path, b);
        db.fail_tag_job(jobs[0].id, "boom").unwrap();
        let errors = db.tag_job_errors(&["C:\\lib".into()], 10).unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, b);
        assert_eq!(errors[0].last_error.as_deref(), Some("boom"));
        let totals = db.tag_queue_totals(&["C:\\lib".into()]).unwrap();
        assert_eq!((totals.error, totals.pending, totals.running), (1, 0, 1));
        drop(db);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn multi_root_library_loads_searches_and_counts() {
        let root =
            std::env::temp_dir().join(format!("kanalib-multi-root-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db_path = root.join("kanalib.db");
        let hashes = |seed: &str| ContentHashes {
            sha1: format!("{seed:0<40}"),
            blake3: format!("{seed:0<64}"),
            phash: 7,
            decode_ok: true,
            decode_error: None,
        };
        let db = Db::open(&db_path).unwrap();
        let a = db
            .upsert_meta(
                "C:\\lib",
                Path::new("C:\\lib\\alpha.png"),
                None,
                10,
                10,
                100,
                10,
                &hashes("a"),
                false,
            )
            .unwrap();
        let b = db
            .upsert_meta(
                "C:\\lib2",
                Path::new("C:\\lib2\\beta.png"),
                None,
                20,
                20,
                200,
                20,
                &hashes("b"),
                false,
            )
            .unwrap();
        db.add_manual_tags(&[a.path.clone(), b.path.clone()], "sharedtag", "manual")
            .unwrap();
        let roots = vec!["C:\\lib".to_string(), "C:\\lib2".to_string()];
        let assets = db.load_library(&roots).unwrap();
        assert_eq!(assets.len(), 2);
        assert!(
            assets
                .iter()
                .any(|asset| asset.library_root == PathBuf::from("C:\\lib2"))
        );
        assert!(
            assets
                .iter()
                .all(|asset| asset.manual_tags.iter().any(|tag| tag.tag == "sharedtag"))
        );
        let hits = db
            .search_paths_in(&roots, &["sharedtag".to_string()])
            .unwrap();
        assert_eq!(hits.len(), 2);
        let summary = db
            .list_tags_in(&roots)
            .unwrap()
            .into_iter()
            .find(|tag| tag.name == "sharedtag")
            .expect("multi-root tag summary");
        assert_eq!((summary.count, summary.manual_count), (2, 2));
        assert_eq!(db.load_library(&["C:\\lib".to_string()]).unwrap().len(), 1);
        let _ = fs::remove_dir_all(&root);
    }
    const SEARCH_SORTS: [SearchSort; 8] = [
        SearchSort::Name,
        SearchSort::Modified,
        SearchSort::Captured,
        SearchSort::Size,
        SearchSort::Resolution,
        SearchSort::Star,
        SearchSort::Favorite,
        SearchSort::Rating,
    ];

    fn legacy_search_paths(
        db: &Db,
        folders: &[String],
        tokens: &[String],
    ) -> HashSet<PathBuf> {
        if tokens.is_empty() || folders.is_empty() {
            return HashSet::new();
        }
        let placeholders = vec!["?"; folders.len()].join(", ");
        let use_fts = tokens.iter().all(|token| token.chars().count() >= 3);
        if use_fts {
            let query = tokens
                .iter()
                .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" ");
            let sql = format!(
                "SELECT path FROM assets
                 WHERE folder IN ({placeholders})
                   AND id IN (SELECT rowid FROM assets_fts WHERE assets_fts MATCH ?)"
            );
            let result = (|| -> anyhow::Result<HashSet<PathBuf>> {
                let mut statement = db.conn.prepare(&sql)?;
                let rows = statement.query_map(
                    rusqlite::params_from_iter(folders.iter().chain(std::iter::once(&query))),
                    |row| row.get::<_, String>(0),
                )?;
                Ok(rows
                    .collect::<rusqlite::Result<Vec<_>>>()?
                    .into_iter()
                    .map(PathBuf::from)
                    .collect())
            })();
            match result {
                Ok(paths) if !paths.is_empty() => return paths,
                Ok(_) => {}
                Err(_) => {}
            }
        }
        let mut sql = format!("SELECT path FROM assets WHERE folder IN ({placeholders})");
        let mut values = Vec::with_capacity(tokens.len());
        for (index, token) in tokens.iter().enumerate() {
            let slot = folders.len() + index + 1;
            sql.push_str(&format!(
                " AND (name LIKE ?{slot} ESCAPE '\\' OR path LIKE ?{slot} ESCAPE '\\' OR tags_text LIKE ?{slot} ESCAPE '\\')"
            ));
            values.push(format!("%{}%", super::escape_like(token)));
        }
        let mut statement = db.conn.prepare(&sql).unwrap();
        let mut parameters: Vec<&dyn rusqlite::ToSql> =
            Vec::with_capacity(folders.len() + values.len());
        for folder in folders {
            parameters.push(folder);
        }
        for value in &values {
            parameters.push(value);
        }
        let rows = statement
            .query_map(rusqlite::params_from_iter(parameters), |row| {
                row.get::<_, String>(0)
            })
            .unwrap();
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .into_iter()
            .map(PathBuf::from)
            .collect()
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

    fn memory_search_paths(
        db: &Db,
        roots: &[String],
        filter: &Filter,
        query: &SearchQuery,
        duplicates: &HashSet<PathBuf>,
        sort: SearchSort,
        ascending: bool,
    ) -> Vec<PathBuf> {
        let assets = db.load_library(roots).unwrap();
        let free = query.free_text();
        let fts_paths = if free.is_empty() {
            None
        } else {
            let tokens = free
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>();
            Some(legacy_search_paths(db, roots, &tokens))
        };
        let mut hits = assets
            .iter()
            .filter(|asset| {
                filter.matches(asset)
                    && query.matches(asset, fts_paths.as_ref(), Some(duplicates))
            })
            .collect::<Vec<&Asset>>();
        match sort {
            SearchSort::Name => {
                hits.sort_by_cached_key(|asset| {
                    (asset.title().to_lowercase(), asset.path.clone())
                });
                if !ascending {
                    hits.reverse();
                }
            }
            mode => {
                hits.sort_by_cached_key(|asset| {
                    let primary = match mode {
                        SearchSort::Modified => asset.mtime,
                        SearchSort::Captured => asset.taken_or_modified(),
                        SearchSort::Size => asset.bytes,
                        SearchSort::Resolution => asset.width as u64 * asset.height as u64,
                        SearchSort::Star => asset.star as u64,
                        SearchSort::Favorite => asset.favorite as u64,
                        SearchSort::Rating => rating_rank(&asset.rating),
                        SearchSort::Name => unreachable!(),
                    };
                    let primary = if ascending {
                        primary
                    } else {
                        u64::MAX - primary
                    };
                    (primary, asset.title().to_lowercase(), asset.path.clone())
                });
            }
        }
        hits.into_iter().map(|asset| asset.path.clone()).collect()
    }

    fn search_fixture() -> (PathBuf, Vec<String>, Db, HashSet<PathBuf>) {
        let root = temp_root("search-page");
        let db = Db::open(&root.join("kanalib.db")).unwrap();
        let roots = vec!["C:\\lib".to_string(), "C:\\lib2".to_string()];
        let dirs = ["a", "a!", "b", "c"];
        let names = [
            "Alpha.JPG",
            "alpha.jpg",
            "Éclair.jpg",
            "风景_合成.jpg",
            "sky_Beta.png",
            "beta.png",
            "omega.webp",
            "Synth_10.jpg",
            "zeta.png",
        ];
        let ratings = ["", "G", "S", "Q", "E"];
        let groups = ["角色: A", "作品: B", ""];
        for index in 0..64usize {
            let root = &roots[index % roots.len()];
            let name = if index < 3 {
                "same.jpg".to_string()
            } else {
                format!("{:02}_{}", index, names[index % names.len()])
            };
            let dir = dirs[index % dirs.len()];
            let path = PathBuf::from(format!("{root}\\{dir}\\{name}"));
            let hashes = ContentHashes {
                sha1: format!("{index:0>40}"),
                blake3: format!("{index:0>64}"),
                phash: index as u64 + 1,
                decode_ok: true,
                decode_error: None,
            };
            let width = [100u32, 200, 320, 64][index % 4];
            let height = [80u32, 100, 240, 64][index % 4];
            let bytes = 1_000u64 + index as u64 * 137;
            let mtime = 1_700_000_000u64 + index as u64 * 3_600;
            db.upsert_meta(
                root,
                &path,
                None,
                width,
                height,
                bytes,
                mtime,
                &hashes,
                false,
            )
            .unwrap();
            db.set_exif(
                &path,
                if index % 3 == 0 {
                    None
                } else if index % 17 == 0 {
                    Some(0)
                } else {
                    Some(1_680_000_000 + index as u64 * 86_400)
                },
                "{}",
            )
            .unwrap();
            if index % 4 != 0 {
                let mut asset = db.load_asset(&path).unwrap();
                asset.tagged = true;
                asset.rating = ratings[index % ratings.len()].to_string();
                asset.group = groups[index % groups.len()].to_string();
                let mut tags = Vec::new();
                if index % 2 == 0 {
                    tags.push(TagHit {
                        tag: "sky".to_string(),
                        category: "general".to_string(),
                        score: 0.9,
                        vocab_index: None,
                    });
                }
                if index % 3 == 0 {
                    tags.push(TagHit {
                        tag: "1girl".to_string(),
                        category: "general".to_string(),
                        score: 0.8,
                        vocab_index: None,
                    });
                }
                if index % 5 == 0 {
                    tags.push(TagHit {
                        tag: "克拉拉".to_string(),
                        category: "character".to_string(),
                        score: 0.7,
                        vocab_index: None,
                    });
                }
                if index % 7 == 0 {
                    tags.push(TagHit {
                        tag: "Étag".to_string(),
                        category: "style".to_string(),
                        score: 0.6,
                        vocab_index: None,
                    });
                }
                asset.tags = tags;
                db.update_tags(&asset).unwrap();
            }
            if index % 6 == 0 {
                let asset = db.load_asset(&path).unwrap();
                if asset.tagged {
                    db.add_manual_tags(std::slice::from_ref(&path), "manual_tag", "manual")
                        .unwrap();
                }
            }
            if index % 5 == 0 {
                db.set_favorite(std::slice::from_ref(&path), true).unwrap();
            }
            db.set_star(std::slice::from_ref(&path), (index % 6) as u8)
                .unwrap();
            if index % 11 == 0 {
                db.mark_error(&path, "boom", None).unwrap();
            }
            if index % 13 == 0 {
                db.conn
                    .execute(
                        "UPDATE assets SET damage = 'bad' WHERE path = ?1",
                        [path.to_string_lossy().to_string()],
                    )
                    .unwrap();
            }
        }
        let duplicates = db
            .load_library(&roots)
            .unwrap()
            .into_iter()
            .enumerate()
            .filter(|(index, _)| index % 5 == 0)
            .map(|(_, asset)| asset.path)
            .collect::<HashSet<_>>();
        (root, roots, db, duplicates)
    }

    fn search_cases(roots: &[String]) -> Vec<(Filter, String)> {
        vec![
            (Filter::All, String::new()),
            (Filter::All, "synthetic".to_string()),
            (Filter::All, "syn".to_string()),
            (Filter::All, "sky".to_string()),
            (Filter::All, "合成".to_string()),
            (Filter::All, "alpha sky".to_string()),
            (Filter::All, "synthetic nomatch".to_string()),
            (Filter::All, "rating:s".to_string()),
            (Filter::All, "-rating:s".to_string()),
            (Filter::All, "is:fav".to_string()),
            (Filter::All, "is:untagged".to_string()),
            (Filter::All, "is:failed".to_string()),
            (Filter::All, "is:damaged".to_string()),
            (Filter::All, "star:>=3".to_string()),
            (Filter::All, "star:2".to_string()),
            (Filter::All, "-star:2".to_string()),
            (Filter::All, "size:>500b".to_string()),
            (Filter::All, "width:>=100".to_string()),
            (Filter::All, "height:<150".to_string()),
            (Filter::All, "ratio:>1".to_string()),
            (Filter::All, "-ratio:>=1".to_string()),
            (Filter::All, "date:2024-02".to_string()),
            (Filter::All, "date:>2024-02-15".to_string()),
            (Filter::All, "date:<=2024".to_string()),
            (Filter::All, "date:2024-02-31".to_string()),
            (Filter::All, "group:角色".to_string()),
            (Filter::All, "name:alpha".to_string()),
            (Filter::All, "-name:alpha".to_string()),
            (Filter::All, "path:lib2".to_string()),
            (Filter::All, "tag:sky".to_string()),
            (Filter::All, "#SKY".to_string()),
            (Filter::All, "-tag:sky".to_string()),
            (Filter::All, "general:girl".to_string()),
            (Filter::All, "-general:girl".to_string()),
            (Filter::All, "source:local".to_string()),
            (Filter::All, "dup:true".to_string()),
            (Filter::All, "dup:true is:fav".to_string()),
            (Filter::Favorites, String::new()),
            (Filter::Rating("s".to_string()), String::new()),
            (Filter::Group("角色: A".to_string()), String::new()),
            (Filter::Tag("sky".to_string()), String::new()),
            (Filter::Tag("SKY".to_string()), String::new()),
            (Filter::Source(PathBuf::from(&roots[1])), String::new()),
            (Filter::Failed, String::new()),
            (Filter::Damaged, String::new()),
            (Filter::Untagged, String::new()),
            (Filter::Favorites, "synthetic".to_string()),
        ]
    }

    #[test]
    fn search_page_matches_memory_filter_sort_and_paging() {
        let (root, roots, db, duplicates) = search_fixture();
        for (filter, text) in search_cases(&roots) {
            let query = SearchQuery::parse(&text);
            for sort in SEARCH_SORTS {
                for ascending in [true, false] {
                    let expected = memory_search_paths(
                        &db,
                        &roots,
                        &filter,
                        &query,
                        &duplicates,
                        sort,
                        ascending,
                    );
                    let full = db
                        .search_page(
                            &roots,
                            SearchPageRequest {
                                filter: &filter,
                                query: &query,
                                sort,
                                ascending,
                                limit: 500,
                                cursor: None,
                                duplicate_paths: Some(&duplicates),
                                include_total: true,
                            },
                        )
                        .unwrap();
                    assert_eq!(
                        full.paths, expected,
                        "filter={filter:?} query={text:?} sort={sort:?} ascending={ascending}"
                    );
                    assert_eq!(full.total, Some(expected.len()));
                    assert!(full.next.is_none());
                    let mut actual = Vec::new();
                    let mut cursor = None;
                    let mut pages = 0;
                    loop {
                        let page = db
                            .search_page(
                                &roots,
                                SearchPageRequest {
                                    filter: &filter,
                                    query: &query,
                                    sort,
                                    ascending,
                                    limit: 3,
                                    cursor: cursor.as_ref(),
                                    duplicate_paths: Some(&duplicates),
                                    include_total: false,
                                },
                            )
                            .unwrap();
                        actual.extend(page.paths);
                        pages += 1;
                        assert!(pages < 1_000, "paging loop did not terminate");
                        match page.next {
                            Some(next) => cursor = Some(next),
                            None => break,
                        }
                    }
                    assert_eq!(
                        actual, expected,
                        "paged filter={filter:?} query={text:?} sort={sort:?} ascending={ascending}"
                    );
                    let unique = actual.iter().collect::<HashSet<_>>();
                    assert_eq!(unique.len(), actual.len(), "duplicate paged paths");
                }
            }
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_paths_short_and_mixed_tokens_keep_legacy_semantics() {
        let (root, roots, db, _) = search_fixture();
        let cases = [
            vec!["same".to_string()],
            vec!["syn".to_string()],
            vec!["synthetic".to_string()],
            vec!["alpha".to_string(), "sk".to_string()],
            vec!["synthetic".to_string(), "zz".to_string()],
            vec!["zzzz".to_string(), "a".to_string()],
            vec!["zzzz".to_string()],
            vec!["风景".to_string()],
            vec!["Éclair".to_string()],
            vec!["Éc".to_string(), "air".to_string()],
            vec!["alpha".to_string(), "sky".to_string()],
        ];
        for tokens in cases {
            let legacy = legacy_search_paths(&db, &roots, &tokens);
            let optimized = db.search_paths_in(&roots, &tokens).unwrap();
            assert_eq!(legacy, optimized, "tokens={tokens:?}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_page_edges_and_cursor_validation() {
        let (root, roots, db, duplicates) = search_fixture();
        let filter = Filter::All;
        let query = SearchQuery::parse("");
        assert!(
            db.search_page(
                &roots,
                SearchPageRequest {
                    filter: &filter,
                    query: &query,
                    sort: SearchSort::Name,
                    ascending: true,
                    limit: 0,
                    cursor: None,
                    duplicate_paths: None,
                    include_total: false,
                }
            )
            .is_err()
        );
        let first = db
            .search_page(
                &roots,
                SearchPageRequest {
                    filter: &filter,
                    query: &query,
                    sort: SearchSort::Name,
                    ascending: true,
                    limit: 2,
                    cursor: None,
                    duplicate_paths: None,
                    include_total: true,
                },
            )
            .unwrap();
        assert_eq!(first.paths.len(), 2);
        assert_eq!(first.total, Some(64));
        let cursor = first.next.expect("next cursor");
        assert!(
            db.search_page(
                &roots,
                SearchPageRequest {
                    filter: &filter,
                    query: &query,
                    sort: SearchSort::Star,
                    ascending: true,
                    limit: 2,
                    cursor: Some(&cursor),
                    duplicate_paths: None,
                    include_total: false,
                }
            )
            .is_err()
        );
        let dup_query = SearchQuery::parse("dup:true");
        let empty = db
            .search_page(
                &roots,
                SearchPageRequest {
                    filter: &filter,
                    query: &dup_query,
                    sort: SearchSort::Name,
                    ascending: true,
                    limit: 10,
                    cursor: None,
                    duplicate_paths: None,
                    include_total: true,
                },
            )
            .unwrap();
        assert!(empty.paths.is_empty());
        assert_eq!(empty.total, Some(0));
        let dup = db
            .search_page(
                &roots,
                SearchPageRequest {
                    filter: &filter,
                    query: &dup_query,
                    sort: SearchSort::Name,
                    ascending: true,
                    limit: 10,
                    cursor: None,
                    duplicate_paths: Some(&duplicates),
                    include_total: true,
                },
            )
            .unwrap();
        assert!(!dup.paths.is_empty());
        let impossible = SearchQuery::parse("source:webdav");
        assert!(
            db.search_page(
                &roots,
                SearchPageRequest {
                    filter: &filter,
                    query: &impossible,
                    sort: SearchSort::Name,
                    ascending: true,
                    limit: 10,
                    cursor: None,
                    duplicate_paths: None,
                    include_total: true,
                }
            )
            .unwrap()
            .paths
            .is_empty()
        );
        assert!(
            db.search_page(
                &[],
                SearchPageRequest {
                    filter: &filter,
                    query: &query,
                    sort: SearchSort::Name,
                    ascending: true,
                    limit: 10,
                    cursor: None,
                    duplicate_paths: None,
                    include_total: false,
                }
            )
            .unwrap()
            .paths
            .is_empty()
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// 320 行、每个排序键都有大量并列值的合成库：用来覆盖「命中很多 →
    /// 走覆盖索引/临时排序」与游标跨并列值翻页。名称全部为 ASCII，
    /// 因此排序索引可用；所有名称都含 `synthetic`，自由文本命中全库。
    fn broad_search_fixture(rows: usize) -> (PathBuf, Vec<String>, Db) {
        let root = temp_root("search-broad");
        let db = Db::open(&root.join("kanalib.db")).unwrap();
        let roots = vec!["C:\\broad".to_string()];
        let dirs = ["a", "a!", "b", "c", "d", "e", "f", "g"];
        let names = [
            "synthetic_same.jpg",
            "synthetic_Alpha.JPG",
            "synthetic_alpha.jpg",
            "synthetic_beta.png",
            "synthetic_sky_Beta.png",
            "synthetic_omega.webp",
            "synthetic_zeta.png",
            "synthetic_same.jpg",
        ];
        for index in 0..rows {
            let path = PathBuf::from(format!(
                "{}\\{}\\set{index:03}\\{}",
                roots[0],
                dirs[index % dirs.len()],
                names[index % names.len()]
            ));
            db.upsert_meta(
                &roots[0],
                &path,
                None,
                [100u32, 200, 320, 640][index % 4],
                [80u32, 100, 240, 480][index % 4],
                1_000 + (index % 3) as u64 * 500,
                1_700_000_000 + (index % 5) as u64 * 3_600,
                &hash('a'),
                false,
            )
            .unwrap();
        }
        db.conn
            .execute_batch(
                "UPDATE assets SET
                    mtime = 1700000000 + (id % 5) * 3600,
                    captured_at = CASE WHEN id % 7 = 0 THEN NULL
                                       ELSE 1680000000 + (id % 4) * 86400 END,
                    rating = CASE id % 5 WHEN 0 THEN '' WHEN 1 THEN 'G'
                                         WHEN 2 THEN 'S' WHEN 3 THEN 'Q' ELSE 'E' END,
                    star = id % 6,
                    favorite = id % 2,
                    group_name = CASE id % 3 WHEN 0 THEN '角色: A' ELSE '' END;",
            )
            .unwrap();
        (root, roots, db)
    }

    #[test]
    fn search_page_broad_sorts_use_cover_index_and_match_memory() {
        let (root, roots, db) = broad_search_fixture(320);
        let filter = Filter::All;
        let query = SearchQuery::parse("synthetic");
        let duplicates = HashSet::new();
        let limit = 5;
        // 320 > limit * 8（且 >= 256），search_page 会走“命中很多”的分支。
        for sort in SEARCH_SORTS {
            for ascending in [true, false] {
                let expected = memory_search_paths(
                    &db, &roots, &filter, &query, &duplicates, sort, ascending,
                );
                assert_eq!(expected.len(), 320);
                let mut actual = Vec::new();
                let mut cursor = None;
                loop {
                    let page = db
                        .search_page(
                            &roots,
                            SearchPageRequest {
                                filter: &filter,
                                query: &query,
                                sort,
                                ascending,
                                limit,
                                cursor: cursor.as_ref(),
                                duplicate_paths: None,
                                include_total: false,
                            },
                        )
                        .unwrap();
                    actual.extend(page.paths);
                    match page.next {
                        Some(next) => cursor = Some(next),
                        None => break,
                    }
                    assert!(actual.len() <= 320);
                }
                assert_eq!(
                    actual, expected,
                    "broad paging sort={sort:?} ascending={ascending}"
                );
                let unique = actual.iter().collect::<HashSet<_>>();
                assert_eq!(unique.len(), actual.len(), "duplicate broad paths");
            }
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_page_sort_cover_index_matches_order_by() {
        let (root, roots, db) = broad_search_fixture(400);
        let filter = Filter::All;
        let query = SearchQuery::parse("synthetic");
        let parts = query.parts();
        for sort in SEARCH_SORTS {
            let index = if sort == SearchSort::Name {
                Some("idx_assets_name_lower")
            } else {
                sort_cover_index(sort)
            };
            // 只对保留下来的正收益排序验证索引计划；被淘汰的排序继续走临时排序。
            let Some(index) = index else { continue };
            let request = SearchPageRequest {
                filter: &filter,
                query: &query,
                sort,
                ascending: false,
                limit: 5,
                cursor: None,
                duplicate_paths: None,
                include_total: false,
            };
            let mut built = db.build_search_sql(&roots, &request, &parts).unwrap();
            built.force_index = Some(index);
            let mut params = built.where_params.clone();
            params.push(Value::Integer(6));
            let plan = explain_with(&db, &built.page_sql(), &params);
            assert!(
                plan.iter().any(|line| line.contains(index)),
                "sort={sort:?} plan={plan:?}"
            );
            assert!(
                !plan.iter().any(|line| line.contains("TEMP B-TREE")),
                "sort={sort:?} still sorts: {plan:?}"
            );
        }
        // Unicode 名称折叠下 ORDER BY 使用 kanalib_lower，与索引里的
        // lower(name) 不一致；这时不能强推索引，宽查询退回临时排序。
        db.remember_name_fold("Éclair.jpg");
        db.invalidate_name_fold();
        assert_eq!(db.detect_name_fold().unwrap(), NameFold::Unicode);
        let request = SearchPageRequest {
            filter: &filter,
            query: &query,
            sort: SearchSort::Modified,
            ascending: false,
            limit: 5,
            cursor: None,
            duplicate_paths: None,
            include_total: false,
        };
        let mut built = db.build_search_sql(&roots, &request, &parts).unwrap();
        built.force_index = sort_cover_index(SearchSort::Modified);
        let mut params = built.where_params.clone();
        params.push(Value::Integer(6));
        let plan = explain_with(&db, &built.page_sql(), &params);
        assert!(
            plan.iter().any(|line| line.contains("TEMP B-TREE")),
            "unicode fold should not use lower() index: {plan:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[ignore = "manual query-plan probe with keyset cursor"]
    fn search_page_keyset_plan_probe() {
        let (root, roots, db) = broad_search_fixture(300);
        db.conn
            .execute_batch(
                "UPDATE assets SET mtime = 100, captured_at = NULL, star = 0,
                    favorite = 0, rating = 'S', bytes = 777, width = 10, height = 10;",
            )
            .unwrap();
        let filter = Filter::All;
        let query = SearchQuery::parse("synthetic");
        let parts = query.parts();
        let first = db
            .search_page(
                &roots,
                SearchPageRequest {
                    filter: &filter,
                    query: &query,
                    sort: SearchSort::Star,
                    ascending: false,
                    limit: 3,
                    cursor: None,
                    duplicate_paths: None,
                    include_total: false,
                },
            )
            .unwrap();
        let cursor = first.next.expect("cursor");
        let request = SearchPageRequest {
            filter: &filter,
            query: &query,
            sort: SearchSort::Star,
            ascending: false,
            limit: 3,
            cursor: Some(&cursor),
            duplicate_paths: None,
            include_total: false,
        };
        let mut built = db.build_search_sql(&roots, &request, &parts).unwrap();
        built.force_index = sort_cover_index(SearchSort::Rating);
        let mut params = built.where_params.clone();
        params.extend(built.keyset_params.iter().cloned());
        params.push(Value::Integer(4));
        eprintln!("page2 sql: {}", built.page_sql());
        eprintln!("page2 plan: {:?}", explain_with(&db, &built.page_sql(), &params));
        let started = std::time::Instant::now();
        for _ in 0..20 {
            let _ = db
                .search_page(
                    &roots,
                    SearchPageRequest {
                        filter: &filter,
                        query: &query,
                        sort: SearchSort::Star,
                        ascending: false,
                        limit: 3,
                        cursor: Some(&cursor),
                        duplicate_paths: None,
                        include_total: false,
                    },
                )
                .unwrap();
        }
        eprintln!(
            "page2 20x ms={:.1}",
            started.elapsed().as_secs_f64() * 1000.0
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_page_all_ties_paginate_without_gaps() {
        let (root, roots, db) = broad_search_fixture(300);
        // 所有排序主键都相同：顺序完全由名称/路径 tie-breaker 决定。
        db.conn
            .execute_batch(
                "UPDATE assets SET mtime = 100, captured_at = NULL, star = 0,
                    favorite = 0, rating = 'S', bytes = 777, width = 10, height = 10;",
            )
            .unwrap();
        let filter = Filter::All;
        let query = SearchQuery::parse("synthetic");
        let duplicates = HashSet::new();
        for sort in [
            SearchSort::Modified,
            SearchSort::Captured,
            SearchSort::Size,
            SearchSort::Resolution,
            SearchSort::Star,
            SearchSort::Favorite,
            SearchSort::Rating,
        ] {
            for ascending in [true, false] {
                let combo_started = std::time::Instant::now();
                let expected =
                    memory_search_paths(&db, &roots, &filter, &query, &duplicates, sort, ascending);
                let mut actual = Vec::new();
                let mut cursor = None;
                let mut first_page = true;
                loop {
                    let page = db
                        .search_page(
                            &roots,
                            SearchPageRequest {
                                filter: &filter,
                                query: &query,
                                sort,
                                ascending,
                                limit: 3,
                                cursor: cursor.as_ref(),
                                duplicate_paths: None,
                                include_total: false,
                            },
                        )
                        .unwrap();
                    if first_page {
                        first_page = false;
                    }
                    actual.extend(page.paths);
                    match page.next {
                        Some(next) => cursor = Some(next),
                        None => break,
                    }
                    assert!(actual.len() <= 300);
                }
                assert_eq!(
                    actual, expected,
                    "all-ties paging sort={sort:?} ascending={ascending}"
                );
                eprintln!(
                    "all-ties combo sort={sort:?} ascending={ascending} pages={} ms={:.1}",
                    actual.len() / 3,
                    combo_started.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_page_extent_handles_free_text_with_filters_and_totals() {
        let (root, roots, db) = broad_search_fixture(320);
        let filter = Filter::All;
        let query = SearchQuery::parse("synthetic is:failed");
        let empty = db
            .search_page(
                &roots,
                SearchPageRequest {
                    filter: &filter,
                    query: &query,
                    sort: SearchSort::Modified,
                    ascending: false,
                    limit: 5,
                    cursor: None,
                    duplicate_paths: None,
                    include_total: false,
                },
            )
            .unwrap();
        assert!(empty.paths.is_empty());
        assert_eq!(empty.total, None);

        // 只有 2 行命中额外筛选：走小结果分支但仍要按完整排序输出。
        db.conn
            .execute_batch(
                "UPDATE assets SET error = 'boom'
                 WHERE id IN (SELECT id FROM assets ORDER BY id LIMIT 2);",
            )
            .unwrap();
        let query = SearchQuery::parse("synthetic is:failed");
        let expected = memory_search_paths(
            &db,
            &roots,
            &filter,
            &query,
            &HashSet::new(),
            SearchSort::Modified,
            false,
        );
        assert_eq!(expected.len(), 2);
        let page = db
            .search_page(
                &roots,
                SearchPageRequest {
                    filter: &filter,
                    query: &query,
                    sort: SearchSort::Modified,
                    ascending: false,
                    limit: 5,
                    cursor: None,
                    duplicate_paths: None,
                    include_total: true,
                },
            )
            .unwrap();
        assert_eq!(page.paths, expected);
        assert_eq!(page.total, Some(2));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fts_extent_query_plan_prefers_fts_driver() {
        let (root, roots, db) = broad_search_fixture(320);
        let filter = Filter::All;
        let query = SearchQuery::parse("omega");
        let request = SearchPageRequest {
            filter: &filter,
            query: &query,
            sort: SearchSort::Name,
            ascending: true,
            limit: 512,
            cursor: None,
            duplicate_paths: None,
            include_total: false,
        };
        let parts = query.parts();
        let built = db.build_search_sql(&roots, &request, &parts).unwrap();
        let fts = built.fts_query.clone().expect("omega should use FTS");
        let (sql, mut params) = db
            .fts_extent_query(&roots, &request, &parts, &fts)
            .unwrap();
        params.push(Value::Integer(4097));
        let plan = explain_with(&db, &sql, &params);
        assert!(
            plan.iter().any(|line| line.contains("SCAN assets_fts")),
            "extent should drive from FTS: {plan:?}"
        );
        assert!(
            !plan.iter().any(|line| line.contains("idx_assets_folder")),
            "extent must not scan folder index per asset: {plan:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    fn asset_index_names(db: &Db) -> HashSet<String> {
        let mut statement = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'assets'")
            .unwrap();
        let rows = statement.query_map([], |row| row.get::<_, String>(0)).unwrap();
        rows.collect::<rusqlite::Result<HashSet<_>>>().unwrap()
    }

    fn explain(db: &Db, sql: &str) -> Vec<String> {
        let mut statement = db
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let rows = statement
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap();
        rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }

    fn explain_with(db: &Db, sql: &str, params: &[Value]) -> Vec<String> {
        let mut statement = db
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let rows = statement
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                row.get::<_, String>(3)
            })
            .unwrap();
        rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }

    #[test]
    fn search_indexes_are_idempotent_and_used() {
        let (root, _, db, _) = search_fixture();
        let names = asset_index_names(&db);
        for expected in [
            "idx_assets_folder_path",
            "idx_assets_rating_nocase",
            "idx_assets_star",
            "idx_assets_favorite",
            "idx_assets_tagged",
            "idx_assets_group_name",
            "idx_assets_bytes",
            "idx_assets_width",
            "idx_assets_height",
            "idx_assets_mtime",
            "idx_assets_error_not_null",
            "idx_assets_damage_not_null",
            "idx_assets_name_lower",
            "idx_assets_sort_modified_desc",
            "idx_assets_sort_captured_desc",
            "idx_assets_sort_size_desc",
            "idx_assets_sort_rating_desc",
        ] {
            assert!(names.contains(expected), "missing index {expected}");
        }
        for removed in [
            "idx_assets_sort_resolution_desc",
            "idx_assets_sort_star_desc",
            "idx_assets_sort_favorite_desc",
        ] {
            assert!(!names.contains(removed), "retired index still exists: {removed}");
        }
        drop(db);
        let db = Db::open(&root.join("kanalib.db")).unwrap();
        assert_eq!(names, asset_index_names(&db));
        let separator = std::path::MAIN_SEPARATOR as u32;
        let plan = explain(
            &db,
            &format!(
                "SELECT assets.path FROM assets
                 ORDER BY lower(assets.name), replace(assets.path, char({separator}), char(1)), assets.id
                 LIMIT 5"
            ),
        );
        assert!(
            plan.iter().any(|line| line.contains("idx_assets_name_lower")),
            "name plan: {plan:?}"
        );
        // 非名称排序的覆盖索引必须与 ORDER BY 完全匹配（含并列值 tie-breaker），
        // 否则 SQLite 仍会建临时 B 树。
        for sort in [
            SearchSort::Modified,
            SearchSort::Captured,
            SearchSort::Size,
            SearchSort::Rating,
        ] {
            let index = sort_cover_index(sort).unwrap();
            let primary = sort_primary_expr(sort).unwrap();
            let plan = explain(
                &db,
                &format!(
                    "SELECT assets.path FROM assets INDEXED BY {index}
                     ORDER BY {primary} DESC, lower(assets.name),
                              replace(assets.path, char({separator}), char(1)), assets.id
                     LIMIT 5"
                ),
            );
            assert!(
                plan.iter().any(|line| line.contains(index)),
                "{sort:?} plan: {plan:?}"
            );
            assert!(
                !plan.iter().any(|line| line.contains("TEMP B-TREE")),
                "{sort:?} still sorts: {plan:?}"
            );
        }
        let plan = explain(
            &db,
            "SELECT assets.path FROM assets WHERE assets.star = 3",
        );
        assert!(
            plan.iter().any(|line| line.contains("idx_assets_star")),
            "star plan: {plan:?}"
        );
        let plan = explain(
            &db,
            "SELECT assets.path FROM assets WHERE assets.rating = 'S' COLLATE NOCASE",
        );
        assert!(
            plan.iter().any(|line| line.contains("idx_assets_rating_nocase")),
            "rating plan: {plan:?}"
        );
        let plan = explain(
            &db,
            "SELECT assets.path FROM assets WHERE assets.folder = 'C:\\lib' AND assets.path > 'a'",
        );
        assert!(
            plan.iter()
                .any(|line| line.contains("idx_assets_folder_path")),
            "folder/path plan: {plan:?}"
        );
        let plan = explain(
            &db,
            "SELECT assets.path FROM assets WHERE assets.error IS NOT NULL",
        );
        assert!(
            plan.iter()
                .any(|line| line.contains("idx_assets_error_not_null")),
            "error plan: {plan:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    fn percentile(values: &[f64], percent: f64) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        let index = ((values.len() as f64 * percent / 100.0) as usize).min(values.len() - 1);
        values[index]
    }

    fn report_metrics(label: &str, mut values: Vec<f64>) {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        eprintln!(
            "search bench: {label} runs={} p50={:.2}ms p95={:.2}ms max={:.2}ms",
            values.len(),
            percentile(&values, 50.0),
            percentile(&values, 95.0),
            values.last().copied().unwrap_or(0.0)
        );
    }

    fn copy_bench_database(source: &Path, target: &Path) {
        fs::copy(source, target).unwrap();
        for suffix in ["-wal", "-shm"] {
            let mut side = source.as_os_str().to_owned();
            side.push(suffix);
            let side = PathBuf::from(side);
            if side.exists() {
                let mut target_side = target.as_os_str().to_owned();
                target_side.push(suffix);
                fs::copy(&side, PathBuf::from(target_side)).unwrap();
            }
        }
    }

    fn generate_bench_database(db_path: &Path, count: usize) {
        let db = Db::open(db_path).unwrap();
        let root = "C:\\bench".to_string();
        db.conn
            .execute(
                "INSERT OR IGNORE INTO sources(kind, name, locator, config_json, read_only)
                 VALUES('local', ?1, ?1, '{}', 1)",
                [root.as_str()],
            )
            .unwrap();
        let source_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM sources WHERE kind = 'local' AND locator = ?1",
                [root.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        let words = ["sky", "forest", "city", "portrait", "abstract", "gradient"];
        let chinese = ["风景", "角色", "合成", "图集"];
        let ratings = ["", "G", "S", "Q", "E"];
        let tx = db.conn.unchecked_transaction().unwrap();
        for index in 0..count {
            let name = format!(
                "synthetic_{index:05}_{}_{}.jpg",
                words[index % words.len()],
                chinese[index % chinese.len()]
            );
            let folder = format!("{root}\\set{:02}_{}", index % 20, words[index % words.len()]);
            let path = format!("{folder}\\{name}");
            let tags = if index % 3 == 0 {
                "[{\"tag\":\"sky\",\"category\":\"general\",\"score\":0.9}]"
            } else {
                "[]"
            };
            let tags_text = if index % 3 == 0 { "sky" } else { "" };
            tx.execute(
                "INSERT INTO assets(
                    source_id, path, folder, name, media_kind, mime, width, height, bytes,
                    mtime, captured_at, rating, group_name, tags_json, tags_text, tagged,
                    favorite, star, error, damage
                 ) VALUES(?1, ?2, ?3, ?4, 'image', 'image/jpeg', ?5, ?6, ?7, ?8, ?9,
                    ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                rusqlite::params![
                    source_id,
                    path,
                    root,
                    name,
                    [100i64, 200, 320, 640][index % 4],
                    [80i64, 100, 240, 480][index % 4],
                    1_000i64 + index as i64 * 37,
                    1_700_000_000i64 + index as i64 * 3_600,
                    (index % 4 != 0).then_some(1_680_000_000i64 + index as i64 * 86_400),
                    ratings[index % ratings.len()],
                    ["角色: A", "作品: B", ""][index % 3],
                    tags,
                    tags_text,
                    i64::from(index % 4 != 0),
                    i64::from(index % 5 == 0),
                    (index % 6) as i64,
                    (index % 101 == 0).then_some("failed"),
                    (index % 137 == 0).then_some("damaged"),
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        db.conn
            .execute("INSERT INTO assets_fts(assets_fts) VALUES('rebuild')", [])
            .unwrap();
    }

    fn bench_warm_page(
        label: &str,
        db: &Db,
        folders: &[String],
        filter: &Filter,
        query: &SearchQuery,
        sort: SearchSort,
        ascending: bool,
        limit: usize,
        runs: usize,
        plan: SortPlan,
    ) {
        for _ in 0..3 {
            db.search_page_with_plan(
                folders,
                SearchPageRequest::new(filter, query, sort, ascending, limit),
                plan,
            )
            .unwrap();
        }
        let hits = db
            .search_page_with_plan(
                folders,
                SearchPageRequest {
                    filter,
                    query,
                    sort,
                    ascending,
                    limit,
                    cursor: None,
                    duplicate_paths: None,
                    include_total: true,
                },
                plan,
            )
            .unwrap()
            .total
            .unwrap_or(0);
        let mut durations = Vec::with_capacity(runs);
        for _ in 0..runs {
            let started = std::time::Instant::now();
            let page = db
                .search_page_with_plan(
                    folders,
                    SearchPageRequest::new(filter, query, sort, ascending, limit),
                    plan,
                )
                .unwrap();
            assert!(page.paths.len() <= limit);
            durations.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        report_metrics(&format!("{label} hits={hits}"), durations);
    }

    #[test]
    #[ignore = "manual query-plan probe; set KANALIB_BENCH_KEEP_DB to a synthetic db"]
    fn search_sort_plan_probe() {
        let Some(source) = std::env::var_os("KANALIB_BENCH_KEEP_DB").map(PathBuf::from) else {
            return;
        };
        let temp = temp_root("plan-probe");
        let db_path = temp.join("probe.db");
        copy_bench_database(&source, &db_path);
        let db = Db::open(&db_path).unwrap();
        let separator = std::path::MAIN_SEPARATOR as u32;
        let name = "lower(assets.name)";
        let path = &format!("replace(assets.path, char({separator}), char(1))");
        let folder = "C:\\bench";
        let matcher = "\"synthetic\"";
        let sorts = [
            ("modified", "assets.mtime"),
            ("captured", "COALESCE(assets.captured_at, assets.mtime)"),
            ("size", "assets.bytes"),
            ("resolution", "assets.width * assets.height"),
            ("star", "assets.star"),
            ("favorite", "CASE WHEN assets.favorite <> 0 THEN 1 ELSE 0 END"),
            (
                "rating",
                "CASE lower(assets.rating) WHEN 'g' THEN 1 WHEN 's' THEN 2 \
                 WHEN 'q' THEN 3 WHEN 'e' THEN 4 ELSE 0 END",
            ),
        ];
        // 候选：每个排序键 × 方向一条复合索引（覆盖 ORDER BY 的全部列）。
        for (key, expr) in sorts {
            let unqualified = expr.replace("assets.", "");
            for (suffix, direction) in [("desc", "DESC"), ("asc", "ASC")] {
                db.conn
                    .execute_batch(&format!(
                        "CREATE INDEX IF NOT EXISTS idx_probe_{key}_{suffix}
                         ON assets({unqualified} {direction}, lower(name) ASC,
                                  replace(path, char({separator}), char(1)) ASC, id ASC);"
                    ))
                    .unwrap();
            }
        }
        let plan = |sql: &str| {
            let mut statement = db
                .conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap();
            let rows = statement
                .query_map([folder, matcher], |row| row.get::<_, String>(3))
                .unwrap();
            rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };
        let base_where = format!(
            "assets.folder IN (?) AND assets.id IN
             (SELECT rowid FROM assets_fts WHERE assets_fts MATCH ?)"
        );
        for (key, expr) in sorts {
            let select = format!("assets.path, {expr}, {name}, {path}, assets.id");
            for ascending in [false, true] {
                let (direction, suffix) = if ascending {
                    ("ASC", "asc")
                } else {
                    ("DESC", "desc")
                };
                let order = format!("{expr} {direction}, {name} ASC, {path} ASC, assets.id ASC");
                for (label, from) in [
                    ("auto", "assets".to_string()),
                    (suffix, format!("assets INDEXED BY idx_probe_{key}_{suffix}")),
                ] {
                    let sql = format!(
                        "SELECT {select} FROM {from} WHERE {base_where} ORDER BY {order} LIMIT 201"
                    );
                    eprintln!("probe {key}/{direction}/{label}: {:?}", plan(&sql));
                    let mut statement = db.conn.prepare(&sql).unwrap();
                    let mut best = f64::MAX;
                    for _ in 0..8 {
                        let started = std::time::Instant::now();
                        let rows = statement
                            .query_map([folder, matcher], |row| row.get::<_, String>(0))
                            .unwrap()
                            .count();
                        best = best.min(started.elapsed().as_secs_f64() * 1000.0);
                        assert!(rows > 0);
                    }
                    eprintln!("probe {key}/{direction}/{label}: best={best:.2}ms");
                }
            }
        }
        db.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        eprintln!(
            "probe db size: {} bytes",
            fs::metadata(&db_path).map(|meta| meta.len()).unwrap_or(0)
        );
        drop(db);
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    #[ignore = "single-query open/search probe; KANALIB_PROBE_DB"]
    fn probe_open_and_search_one_query() {
        let Some(path) = std::env::var_os("KANALIB_PROBE_DB").map(PathBuf::from) else {
            return;
        };
        let text = std::env::var("KANALIB_PROBE_QUERY")
            .unwrap_or_else(|_| "synthetic_0004".to_string());
        let limit = std::env::var("KANALIB_PROBE_LIMIT")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(512);
        let folder = std::env::var("KANALIB_PROBE_FOLDER")
            .unwrap_or_else(|_| "C:\\bench".to_string());
        let started = std::time::Instant::now();
        let db = Db::open(&path).unwrap();
        eprintln!("probe open={:?}", started.elapsed());
        let folders = vec![folder];
        let filter = Filter::All;
        let query = SearchQuery::parse(&text);
        let request = SearchPageRequest {
            filter: &filter,
            query: &query,
            sort: SearchSort::Name,
            ascending: true,
            limit,
            cursor: None,
            duplicate_paths: None,
            include_total: false,
        };
        let parts = query.parts();
        let started = std::time::Instant::now();
        let built = db
            .build_search_sql(&folders, &request, &parts)
            .unwrap();
        eprintln!("probe build={:?}", started.elapsed());
        let started = std::time::Instant::now();
        let extent = db
            .search_extent(&folders, &request, &parts, &built, limit * 8, None)
            .unwrap();
        eprintln!("probe extent={:?} {:?}", started.elapsed(), extent);
        let started = std::time::Instant::now();
        let mut statement = db.conn.prepare(&built.page_sql()).unwrap();
        let mut params = built.where_params.clone();
        params.extend(built.keyset_params.iter().cloned());
        params.push(Value::Integer(limit as i64 + 1));
        let rows = statement
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                row.get::<_, String>(0)
            })
            .unwrap();
        let mut count = 0;
        for row in rows {
            let _ = row.unwrap();
            count += 1;
        }
        eprintln!("probe page_exec={:?} rows={count}", started.elapsed());
        let started = std::time::Instant::now();
        let page = db.search_page(&folders, request.clone()).unwrap();
        eprintln!(
            "probe search_page={:?} limit={limit} rows={}",
            started.elapsed(),
            page.paths.len()
        );
        eprintln!("probe sql={}", built.page_sql());
        let mut statement = db
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", built.page_sql()))
            .unwrap();
        let mut plan_params = built.where_params.clone();
        plan_params.push(Value::Integer(limit as i64 + 1));
        let rows = statement
            .query_map(rusqlite::params_from_iter(plan_params.iter()), |row| {
                row.get::<_, String>(3)
            })
            .unwrap();
        for row in rows {
            eprintln!("probe plan {:?}", row.unwrap());
        }
    }

    #[test]
    #[ignore = "manual performance benchmark; set KANALIB_BENCH_DB or generate 10k rows"]
    fn search_page_benchmark() {
        let temp = temp_root("search-bench");
        let source = std::env::var_os("KANALIB_BENCH_DB").map(PathBuf::from);
        // `KANALIB_BENCH_KEEP_DB` 把合成库留在固定路径（首次生成，之后复用），
        // 让「改前/改后」两次基准跑在同一份库上，避免旧库/新库数字混用。
        let keep = std::env::var_os("KANALIB_BENCH_KEEP_DB").map(PathBuf::from);
        let db_path = match (&keep, &source) {
            (Some(keep), _) => {
                if !keep.exists() {
                    let count = std::env::var("KANALIB_BENCH_ROWS")
                        .ok()
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(10_000)
                        .max(1);
                    if let Some(parent) = keep.parent() {
                        fs::create_dir_all(parent).unwrap();
                    }
                    generate_bench_database(keep, count);
                }
                keep.clone()
            }
            (None, Some(source)) => {
                let db_path = temp.join("kanalib.db");
                copy_bench_database(source, &db_path);
                db_path
            }
            (None, None) => {
                let db_path = temp.join("kanalib.db");
                let count = std::env::var("KANALIB_BENCH_ROWS")
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(10_000)
                    .max(1);
                generate_bench_database(&db_path, count);
                db_path
            }
        };
        let runs = std::env::var("KANALIB_BENCH_RUNS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(30)
            .max(3);
        let db = Db::open(&db_path).unwrap();
        let folders = {
            let mut statement = db
                .conn
                .prepare("SELECT DISTINCT folder FROM assets ORDER BY folder")
                .unwrap();
            let rows = statement.query_map([], |row| row.get::<_, String>(0)).unwrap();
            rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };
        let total: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
            .unwrap();
        eprintln!(
            "search bench: source={} assets={total} folders={} runs={runs} db={}",
            source
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| {
                    if keep.is_some() {
                        "kept".to_string()
                    } else {
                        "generated".to_string()
                    }
                }),
            folders.len(),
            db_path.display()
        );

        // 同库对照：复制一份到临时目录并删掉 4 个产品排序覆盖索引（同时清理
        // 3 个已淘汰的实验索引），再用 `SortPlan::TempSort` 执行同一批查询，
        // 等价于改造前“按筛选索引取行 + 临时 B 树排序”的计划。
        let baseline_root = temp_root("search-baseline");
        let baseline_path = baseline_root.join("kanalib.db");
        copy_bench_database(&db_path, &baseline_path);
        let baseline_db = Db::open(&baseline_path).unwrap();
        baseline_db
            .conn
            .execute_batch(
                "DROP INDEX IF EXISTS idx_assets_sort_modified_desc;
                 DROP INDEX IF EXISTS idx_assets_sort_captured_desc;
                 DROP INDEX IF EXISTS idx_assets_sort_size_desc;
                 DROP INDEX IF EXISTS idx_assets_sort_resolution_desc;
                 DROP INDEX IF EXISTS idx_assets_sort_star_desc;
                 DROP INDEX IF EXISTS idx_assets_sort_favorite_desc;
                 DROP INDEX IF EXISTS idx_assets_sort_rating_desc;
                 PRAGMA wal_checkpoint(TRUNCATE);",
            )
            .unwrap();
        eprintln!(
            "search bench: baseline={} (same rows, 7 sort indexes dropped)",
            baseline_path.display()
        );

        let filter = Filter::All;
        // 基准分段：默认全跑；`KANALIB_BENCH_SECTIONS=sorts` 只跑排序对照，
        // 便于机器繁忙时缩短窗口（只是一个测试开关，不进产品路径）。
        let sections = std::env::var("KANALIB_BENCH_SECTIONS").unwrap_or_default();
        let section = |name: &str| {
            sections.is_empty()
                || sections
                    .split(',')
                    .any(|value| value.trim().eq_ignore_ascii_case(name))
        };
        for (label, text) in [
            ("free=synthetic", "synthetic"),
            ("free=合成", "合成"),
            ("free=sky", "sky"),
            ("free=1girl", "1girl"),
            ("free=syn(short)", "syn"),
            ("free=synthetic+sky(mixed)", "synthetic sky"),
        ] {
            if !section("name") {
                break;
            }
            let query = SearchQuery::parse(text);
            for (plan, suffix) in [
                (SortPlan::CoverIndex, "cover"),
                (SortPlan::TempSort, "baseline"),
            ] {
                let target = if plan == SortPlan::CoverIndex {
                    &db
                } else {
                    &baseline_db
                };
                bench_warm_page(
                    &format!("{suffix} name/sort={label}"),
                    target,
                    &folders,
                    &filter,
                    &query,
                    SearchSort::Name,
                    true,
                    200,
                    runs,
                    plan,
                );
            }
        }

        let query = SearchQuery::parse("synthetic");
        for sort in SEARCH_SORTS {
            if !section("sorts") {
                break;
            }
            for (plan, suffix) in [
                (SortPlan::CoverIndex, "cover"),
                (SortPlan::TempSort, "baseline"),
            ] {
                let target = if plan == SortPlan::CoverIndex {
                    &db
                } else {
                    &baseline_db
                };
                bench_warm_page(
                    &format!("{suffix} free=synthetic sort={sort:?}"),
                    target,
                    &folders,
                    &filter,
                    &query,
                    sort,
                    sort != SearchSort::Name,
                    200,
                    runs,
                    plan,
                );
            }
        }

        // 选择性查询 + 非名称排序：覆盖索引可能在“命中占比不高”时被迫扫过
        // 更多索引项，这里专门记录与旧计划的差异。
        for text in [
            "synthetic sky",
            "group:角色",
            "date:>2024",
            "rating:s",
            "star:>=3",
            "is:damaged",
        ] {
            if !section("selective") {
                break;
            }
            let query = SearchQuery::parse(text);
            for (plan, suffix) in [
                (SortPlan::CoverIndex, "cover"),
                (SortPlan::TempSort, "baseline"),
            ] {
                let target = if plan == SortPlan::CoverIndex {
                    &db
                } else {
                    &baseline_db
                };
                bench_warm_page(
                    &format!("{suffix} selective={text} sort=Modified"),
                    target,
                    &folders,
                    &filter,
                    &query,
                    SearchSort::Modified,
                    false,
                    200,
                    runs,
                    plan,
                );
            }
        }

        for (label, text) in [
            ("is:damaged", "is:damaged"),
            ("is:failed", "is:failed"),
            ("star:>=3", "star:>=3"),
            ("rating:s", "rating:s"),
            ("is:fav", "is:fav"),
            ("size:>5kb", "size:>5kb"),
            ("group:角色", "group:角色"),
            ("date:>2024", "date:>2024"),
        ] {
            if !section("filters") {
                break;
            }
            let query = SearchQuery::parse(text);
            bench_warm_page(
                &format!("filter={label}"),
                &db,
                &folders,
                &filter,
                &query,
                SearchSort::Name,
                true,
                200,
                runs,
                SortPlan::CoverIndex,
            );
        }

        // UI `run_search` 等价路径：state 每次查询都新开连接，先取
        // `MAX_SEARCH_PAGE` 条首页，超限时再取自由文本命中集合供内存回退。
        // 渲染/内存过滤在 UI 线程，核心侧不测（留给 E1）。
        if section("ui") {
            for (label, sort, ascending) in [
            ("ui-run-search name", SearchSort::Name, true),
            ("ui-run-search modified", SearchSort::Modified, false),
        ] {
            for text in ["synthetic", "sky"] {
                let query = SearchQuery::parse(text);
                let mut durations = Vec::with_capacity(runs);
                let mut open_ms = Vec::with_capacity(runs);
                let mut page_ms = Vec::with_capacity(runs);
                let mut open2_ms = Vec::with_capacity(runs);
                let mut paths_ms = Vec::with_capacity(runs);
                let mut truncated = false;
                let mut hits = 0usize;
                for _ in 0..runs {
                    let started = std::time::Instant::now();
                    let opened = Db::open(&db_path).unwrap();
                    open_ms.push(started.elapsed().as_secs_f64() * 1000.0);
                    let page_started = std::time::Instant::now();
                    let page = opened
                        .search_page(
                            &folders,
                            SearchPageRequest {
                                filter: &filter,
                                query: &query,
                                sort,
                                ascending,
                                limit: super::MAX_SEARCH_PAGE,
                                cursor: None,
                                duplicate_paths: None,
                                include_total: false,
                            },
                        )
                        .unwrap();
                    page_ms.push(page_started.elapsed().as_secs_f64() * 1000.0);
                    truncated = page.next.is_some();
                    hits = page.paths.len();
                    let free = query.free_text();
                    if !free.is_empty() {
                        let tokens =
                            free.split_whitespace().map(str::to_string).collect::<Vec<_>>();
                        let open_started = std::time::Instant::now();
                        let opened = Db::open(&db_path).unwrap();
                        open2_ms.push(open_started.elapsed().as_secs_f64() * 1000.0);
                        let paths_started = std::time::Instant::now();
                        opened.search_paths_in(&folders, &tokens).unwrap();
                        paths_ms.push(paths_started.elapsed().as_secs_f64() * 1000.0);
                    }
                    durations.push(started.elapsed().as_secs_f64() * 1000.0);
                }
                let label = format!("{label} free={text}");
                report_metrics(
                    &format!("{label} page={hits} truncated={truncated} open+page+fts"),
                    durations,
                );
                report_metrics(&format!("{label} db-open-1"), open_ms);
                report_metrics(&format!("{label} search-page"), page_ms);
                report_metrics(&format!("{label} db-open-2"), open2_ms);
                report_metrics(&format!("{label} fts-paths"), paths_ms);
            }
            }
        }

        // 冷入口：每次新建连接 + 搜索，等价于 AppState 当前打开方式。
        if section("cold") {
            let query = SearchQuery::parse("synthetic");
            let mut legacy = Vec::with_capacity(runs);
            let mut opened = Vec::with_capacity(runs);
            for _ in 0..runs {
                let started = std::time::Instant::now();
                let opened_db = Db::open(&db_path).unwrap();
                opened_db
                    .search_paths_in(&folders, &["synthetic".to_string()])
                    .unwrap();
                legacy.push(started.elapsed().as_secs_f64() * 1000.0);

                let started = std::time::Instant::now();
                let opened_db = Db::open(&db_path).unwrap();
                opened_db
                    .search_page(
                        &folders,
                        SearchPageRequest::new(&filter, &query, SearchSort::Name, true, 200),
                    )
                    .unwrap();
                opened.push(started.elapsed().as_secs_f64() * 1000.0);
            }
            report_metrics("open+legacy_search_paths free=synthetic", legacy);
            report_metrics("open+search_page name free=synthetic", opened);

            // keyset 第二页：不重复排序，验证分页成本。
            let first = db
                .search_page(
                    &folders,
                    SearchPageRequest::new(&filter, &query, SearchSort::Name, true, 200),
                )
                .unwrap();
            if let Some(cursor) = first.next {
                let mut durations = Vec::with_capacity(runs);
                for _ in 0..runs {
                    let started = std::time::Instant::now();
                    let page = db
                        .search_page(
                            &folders,
                            SearchPageRequest {
                                filter: &filter,
                                query: &query,
                                sort: SearchSort::Name,
                                ascending: true,
                                limit: 200,
                                cursor: Some(&cursor),
                                duplicate_paths: None,
                                include_total: false,
                            },
                        )
                        .unwrap();
                    assert_eq!(page.paths.len(), 200);
                    durations.push(started.elapsed().as_secs_f64() * 1000.0);
                }
                report_metrics("keyset page2 name free=synthetic", durations);
            }
        }
        drop(db);
        drop(baseline_db);
        let _ = fs::remove_dir_all(&baseline_root);
        if keep.is_none() {
            let _ = fs::remove_dir_all(&temp);
        }
    }

    #[test]
    fn name_fold_setting_is_cached_and_updated_by_upsert() {
        let root = temp_root("name-fold");
        let db = Db::open(&root.join("kanalib.db")).unwrap();
        let ascii = PathBuf::from("C:/lib/plain.jpg");
        db.upsert_meta("C:/lib", &ascii, None, 10, 10, 1_000, 1, &hash('a'), false)
            .unwrap();
        let query = SearchQuery::parse("");
        db.search_page(
            &["C:/lib".to_string()],
            SearchPageRequest::new(&Filter::All, &query, SearchSort::Name, true, 10),
        )
        .unwrap();
        assert_eq!(
            db.setting(super::NAME_FOLD_KEY).unwrap().as_deref(),
            Some("0")
        );
        let unicode = PathBuf::from("C:/lib/Éclair.jpg");
        db.upsert_meta("C:/lib", &unicode, None, 10, 10, 1_000, 1, &hash('b'), false)
            .unwrap();
        assert_eq!(
            db.setting(super::NAME_FOLD_KEY).unwrap().as_deref(),
            Some("1")
        );
        drop(db);
        let db = Db::open(&root.join("kanalib.db")).unwrap();
        assert_eq!(
            db.setting(super::NAME_FOLD_KEY).unwrap().as_deref(),
            Some("1")
        );
        let page = db
            .search_page(
                &["C:/lib".to_string()],
                SearchPageRequest::new(&Filter::All, &query, SearchSort::Name, true, 10),
            )
            .unwrap();
        assert_eq!(page.paths.len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn migrates_v8_to_v9_and_reads_legacy_saved_searches() {
        use crate::model::SavedSearchView;

        let root = temp_root("v8-v9");
        let db_path = root.join("kanalib.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(super::SCHEMA_V2).unwrap();
            conn.execute_batch(super::SCHEMA_V4).unwrap();
            conn.execute_batch(super::SCHEMA_V5).unwrap();
            conn.execute_batch(super::SCHEMA_V8).unwrap();
            conn.execute(
                "INSERT INTO saved_searches(name, query_json)
                 VALUES('旧收藏', '{\"filter\":\"Favorites\",\"query\":\"is:fav\"}')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO saved_searches(name, query_json)
                 VALUES('旧搜索', '{\"filter\":\"All\",\"query\":\"夕阳\"}')",
                [],
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 8).unwrap();
        }

        {
            let db = Db::open(&db_path).unwrap();
            let saved = db.load_saved_searches().unwrap();
            assert_eq!(saved.len(), 2);
            // 旧行按 id 顺序读取，视图字段为默认语义，updated_at 保持 0。
            assert_eq!(saved[0].name, "旧收藏");
            assert_eq!(saved[0].filter, Filter::Favorites);
            assert_eq!(saved[0].query, "is:fav");
            assert_eq!(saved[0].view, SavedSearchView::default());
            assert!(!saved[0].pinned);
            assert_eq!(saved[0].position, saved[0].id);
            assert_eq!(saved[0].updated_at, 0);
            assert_eq!(saved[1].name, "旧搜索");
            assert_eq!(saved[1].query, "夕阳");

            let conn = Connection::open(&db_path).unwrap();
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, super::SCHEMA_VERSION);
            for column in [
                "sort_key",
                "sort_ascending",
                "view_mode",
                "pane_mode",
                "show_sidebar",
                "show_extensions",
                "pinned",
                "position",
                "updated_at",
            ] {
                let present: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('saved_searches') WHERE name = ?1",
                        [column],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(present, 1, "缺少列 {column}");
            }
            let reverse_index: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'index' AND name = 'idx_collection_assets_asset'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(reverse_index, 1);
            drop(conn);

            // 迁移后新集合顺延在末尾，旧集合视图仍可正常使用。
            let created = db
                .create_saved_search(
                    "新集合",
                    &Filter::All,
                    "新",
                    &SavedSearchView {
                        sort_key: "name".into(),
                        ..SavedSearchView::default()
                    },
                )
                .unwrap();
            assert_eq!(created.position, 3);
            assert_eq!(created.view.sort_key, "name");
        }

        let backups = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".bak-v8-"))
            .count();
        assert_eq!(backups, 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn saved_search_edit_rename_pin_and_reorder_keep_ids() {
        use crate::model::{DuplicateNameError, OrganizationKind, SavedSearchView};

        let root = temp_root("saved-search-edit");
        let db_path = root.join("kanalib.db");
        let db = Db::open(&db_path).unwrap();
        let view = SavedSearchView {
            sort_key: "name".into(),
            sort_ascending: false,
            view_mode: "icons-lg".into(),
            pane_mode: "split".into(),
            show_sidebar: false,
            show_extensions: true,
        };
        let first = db
            .create_saved_search("收藏", &Filter::Favorites, "is:fav", &view)
            .unwrap();
        assert_eq!(first.view, view);
        assert!(first.updated_at > 0);
        let second = db
            .create_saved_search("全部", &Filter::All, "", &SavedSearchView::default())
            .unwrap();
        assert_eq!(db.load_saved_searches().unwrap()[0].id, first.id);

        let error = db
            .create_saved_search("收藏", &Filter::All, "", &SavedSearchView::default())
            .unwrap_err();
        let duplicate = error
            .downcast_ref::<DuplicateNameError>()
            .expect("duplicate");
        assert_eq!(duplicate.kind, OrganizationKind::SavedSearch);
        assert_eq!(duplicate.name, "收藏");

        // 重命名不改 id，也不改手动顺序。
        let renamed = db.rename_saved_search(second.id, "全部图片").unwrap();
        assert_eq!(renamed.id, second.id);
        assert_eq!(renamed.name, "全部图片");
        assert_eq!(renamed.position, second.position);
        let error = db.rename_saved_search(second.id, "收藏").unwrap_err();
        assert_eq!(
            error.downcast_ref::<DuplicateNameError>().map(|e| e.kind),
            Some(OrganizationKind::SavedSearch)
        );

        // 用当前视图更新：id 与名称不变，内容与视图一起覆盖。
        let updated = db
            .update_saved_search(
                first.id,
                &Filter::Tag("风景".into()),
                "山",
                &SavedSearchView {
                    view_mode: "tiles".into(),
                    ..view.clone()
                },
            )
            .unwrap();
        assert_eq!(updated.id, first.id);
        assert_eq!(updated.name, "收藏");
        assert_eq!(updated.filter, Filter::Tag("风景".into()));
        assert_eq!(updated.query, "山");
        assert_eq!(updated.view.view_mode, "tiles");
        assert_eq!(updated.view.sort_key, "name");

        // 置顶只改分组，不改手动顺序。
        let pinned = db.set_saved_search_pinned(second.id, true).unwrap();
        assert!(pinned.pinned);
        let ids = db
            .load_saved_searches()
            .unwrap()
            .into_iter()
            .map(|search| search.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![second.id, first.id]);
        db.set_saved_search_pinned(second.id, false).unwrap();

        assert_eq!(
            db.reorder_saved_searches(&[first.id, second.id]).unwrap(),
            2
        );
        assert!(db.reorder_saved_searches(&[first.id]).is_err());

        // 重启后顺序、置顶与视图保持。
        db.set_saved_search_pinned(second.id, true).unwrap();
        drop(db);
        let db = Db::open(&db_path).unwrap();
        let restored = db.load_saved_searches().unwrap();
        assert_eq!(restored[0].id, second.id);
        assert!(restored[0].pinned);
        assert_eq!(restored[1].id, first.id);
        assert_eq!(restored[1].position, 1);
        assert_eq!(restored[1].view.view_mode, "tiles");
        assert_eq!(restored[1].query, "山");
        assert_eq!(db.delete_saved_search(first.id).unwrap(), 1);
        assert_eq!(db.load_saved_searches().unwrap().len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn collections_track_batches_counts_and_survive_asset_deletion() {
        use crate::model::{DuplicateNameError, OrganizationKind};

        let root = temp_root("collections");
        let db_path = root.join("kanalib.db");
        let db = Db::open(&db_path).unwrap();
        let a = PathBuf::from("C:\\lib\\a.png");
        let b = PathBuf::from("C:\\lib\\b.png");
        let c = PathBuf::from("C:\\lib\\c.png");
        for (path, seed) in [(&a, 'a'), (&b, 'b'), (&c, 'c')] {
            db.upsert_meta("C:\\lib", path, None, 10, 20, 100, 10, &hash(seed), false)
                .unwrap();
        }

        let trip = db.create_collection("旅行").unwrap();
        let picks = db.create_collection("精选").unwrap();
        assert_eq!(trip.kind, "manual");
        assert_eq!(trip.asset_count, 0);
        let error = db.create_collection("旅行").unwrap_err();
        assert_eq!(
            error.downcast_ref::<DuplicateNameError>().map(|e| e.kind),
            Some(OrganizationKind::Collection)
        );

        assert_eq!(
            db.add_assets_to_collection(trip.id, &[a.clone(), b.clone()])
                .unwrap(),
            2
        );
        // 重复加入幂等；同一资产可属于多个相册。
        assert_eq!(
            db.add_assets_to_collection(trip.id, &[a.clone()]).unwrap(),
            0
        );
        assert_eq!(
            db.add_assets_to_collection(picks.id, &[a.clone()]).unwrap(),
            1
        );
        assert_eq!(db.collection_asset_count(trip.id).unwrap(), 2);
        assert_eq!(db.collection_asset_count(picks.id).unwrap(), 1);
        let listed = db.load_collections().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(
            listed
                .iter()
                .find(|collection| collection.id == trip.id)
                .unwrap()
                .asset_count,
            2
        );
        assert_eq!(
            db.collection_paths(trip.id).unwrap(),
            vec![a.clone(), b.clone()]
        );
        assert_eq!(db.load_collection_assets(trip.id).unwrap().len(), 2);
        assert_eq!(db.asset_collections(&a).unwrap().len(), 2);

        let renamed = db.rename_collection(trip.id, "旅行 2026").unwrap();
        assert_eq!(renamed.id, trip.id);
        assert_eq!(renamed.asset_count, 2);
        assert!(db.rename_collection(picks.id, "旅行 2026").is_err());
        assert_eq!(
            db.remove_assets_from_collection(trip.id, &[b.clone()])
                .unwrap(),
            1
        );
        assert_eq!(db.collection_asset_count(trip.id).unwrap(), 1);

        // 导出只写路径清单，已存在的目标拒绝覆盖。
        let export = root.join("trip.txt");
        assert_eq!(db.export_collection_paths(trip.id, &export).unwrap(), 1);
        let content = fs::read_to_string(&export).unwrap();
        assert_eq!(content, format!("{}\n", a.display()));
        assert!(db.export_collection_paths(trip.id, &export).is_err());

        // 删除相册只删关系，资产仍在。
        assert_eq!(db.delete_collection(trip.id).unwrap(), 1);
        assert_eq!(db.load_folder("C:\\lib").unwrap().len(), 3);
        assert_eq!(db.collection_asset_count(picks.id).unwrap(), 1);
        let orphan: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM collection_assets WHERE collection_id = ?1",
                [trip.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphan, 0);

        // 删除资产时级联清理关系行。
        assert_eq!(db.delete_assets(&[a.clone()]).unwrap(), 1);
        assert_eq!(db.collection_asset_count(picks.id).unwrap(), 0);
        let relations: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM collection_assets", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(relations, 0);

        // 来源移除只清关系行，资产行与相册本身保留。
        db.add_assets_to_collection(picks.id, &[b.clone(), c.clone()])
            .unwrap();
        assert_eq!(db.forget_source_collections("C:\\lib").unwrap(), 2);
        assert_eq!(db.collection_asset_count(picks.id).unwrap(), 0);
        assert_eq!(db.load_folder("C:\\lib").unwrap().len(), 2);

        drop(db);
        // 重启后相册与计数保持。
        let db = Db::open(&db_path).unwrap();
        assert_eq!(db.load_collections().unwrap().len(), 1);
        assert_eq!(db.load_collections().unwrap()[0].name, "精选");
        assert_eq!(db.load_collections().unwrap()[0].asset_count, 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn deleting_source_row_cascades_collection_links() {
        let root = temp_root("collection-source-cascade");
        let db_path = root.join("kanalib.db");
        let path = PathBuf::from("C:\\lib\\a.png");
        let collection_id = {
            let db = Db::open(&db_path).unwrap();
            db.upsert_meta("C:\\lib", &path, None, 10, 20, 100, 10, &hash('a'), false)
                .unwrap();
            let collection = db.create_collection("相册").unwrap();
            db.add_assets_to_collection(collection.id, &[path.clone()])
                .unwrap();
            collection.id
        };
        {
            // 直接删来源行：外键链 sources → assets → collection_assets。
            let conn = Connection::open(&db_path).unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            conn.execute("DELETE FROM sources WHERE locator = ?1", ["C:\\lib"])
                .unwrap();
            let assets: i64 = conn
                .query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
                .unwrap();
            let links: i64 = conn
                .query_row("SELECT COUNT(*) FROM collection_assets", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!((assets, links), (0, 0));
        }
        let db = Db::open(&db_path).unwrap();
        assert_eq!(db.collection_asset_count(collection_id).unwrap(), 0);
        assert!(db.load_collections().unwrap()[0].asset_count == 0);
        let _ = fs::remove_dir_all(&root);
    }
}
