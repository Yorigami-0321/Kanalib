use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, ToSql, params};
use serde::{Deserialize, Serialize};

use crate::dedup::{DuplicateReview, ReviewState};
use crate::media::ContentHashes;
use crate::model::{Asset, Filter, SavedSearch, TagHit, TagSummary, normalize_tag_name};

const SCHEMA_VERSION: i64 = 7;

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
        self.sha1.as_deref().is_some_and(|value| !value.is_empty())
            && self
                .blake3
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            && self.phash.is_some()
    }

    pub fn content_if_current(&self, mtime: u64, bytes: u64) -> Option<ContentHashes> {
        if self.mtime != mtime || self.bytes != bytes {
            return None;
        }
        let sha1 = self.sha1.clone().filter(|value| !value.is_empty())?;
        let blake3 = self.blake3.clone().filter(|value| !value.is_empty())?;
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
        migrate(&conn)?;
        ensure_asset_columns(&conn)?;
        ensure_tag_columns(&conn)?;
        ensure_thumb_sources(&conn)?;
        conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_assets_path ON assets(path);")?;
        Ok(Self { conn })
    }

    pub fn load_folder(&self, folder: &str) -> Result<Vec<Asset>> {
        self.load_library(&[folder.to_string()])
    }

    pub fn asset_paths(&self, folder: &str) -> Result<Vec<PathBuf>> {
        let mut statement = self.conn.prepare("SELECT path FROM assets WHERE folder = ?1")?;
        let rows = statement.query_map([folder], |row| row.get::<_, String>(0))?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    pub fn asset_paths_by_sha1(&self, folder: &str, sha1: &str) -> Result<Vec<PathBuf>> {
        let mut statement = self.conn.prepare(
            "SELECT path FROM assets WHERE folder = ?1 AND sha1 = ?2",
        )?;
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
                        captured_at, metadata_json, exif_state
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
        let path_string = path.to_string_lossy().to_string();
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| path_string.clone());
        let thumb_string = thumb
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        let source_id = self.source_id(folder)?;
        let valid_hashes = !hashes.sha1.is_empty() && !hashes.blake3.is_empty();
        self.conn.execute(
            "INSERT INTO assets(
                source_id, path, folder, name, media_kind, mime, width, height, bytes, mtime,
                thumb, sha1, blake3, phash, damage
             ) VALUES(?1, ?2, ?3, ?4, 'image', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
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
                tag_attempts = CASE WHEN ?15 THEN 0 ELSE tag_attempts END",
            params![
                source_id,
                path_string,
                folder,
                name,
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
        let rows = statement.query_map(
            rusqlite::params_from_iter(folders.iter()),
            |row| {
                Ok(TagSummary {
                    name: row.get(0)?,
                    category: row.get(1)?,
                    count: row.get::<_, i64>(2)?.max(0) as usize,
                    manual_count: row.get::<_, i64>(3)?.max(0) as usize,
                })
            },
        )?;
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
        tx.execute(
            "UPDATE tags SET alias_of = NULL WHERE id = ?1",
            [target_id],
        )?;
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

    pub fn load_saved_searches(&self) -> Result<Vec<SavedSearch>> {
        let mut statement = self
            .conn
            .prepare("SELECT id, name, query_json FROM saved_searches ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            let payload: Option<SavedSearchPayload> =
                serde_json::from_str(&row.get::<_, String>(2)?).ok();
            Ok(SavedSearch {
                id: row.get(0)?,
                name: row.get(1)?,
                filter: payload
                    .as_ref()
                    .map(|payload| payload.filter.clone())
                    .unwrap_or(Filter::All),
                query: payload.map(|payload| payload.query).unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn save_saved_search(&self, name: &str, filter: &Filter, query: &str) -> Result<i64> {
        let payload = SavedSearchPayload {
            filter: filter.clone(),
            query: query.to_string(),
        };
        self.conn.execute(
            "INSERT INTO saved_searches(name, query_json) VALUES(?1, ?2)",
            params![name, serde_json::to_string(&payload)?],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_saved_search(&self, id: i64) -> Result<usize> {
        Ok(self
            .conn
            .execute("DELETE FROM saved_searches WHERE id = ?1", [id])?)
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
             WHERE folder = ?1 AND tagged = 0 AND tag_attempts < 3 AND deleted_at IS NULL",
            [folder],
        )?;
        tx.execute(
            "UPDATE jobs
             SET state = 'pending', attempts = 0, last_error = NULL, run_after = 0
             WHERE kind = 'tag' AND asset_id IN (
                SELECT id FROM assets
                WHERE folder = ?1 AND tagged = 0 AND tag_attempts < 3 AND deleted_at IS NULL
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
            deleted += tx.execute(
                "DELETE FROM assets WHERE path = ?1",
                [path.to_string_lossy().to_string()],
            )?;
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
        let old_string = old_path.to_string_lossy().to_string();
        let new_string = new_path.to_string_lossy().to_string();
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
            .query_row("SELECT id FROM assets WHERE path = ?1", [&new_string], |row| {
                row.get(0)
            })
            .optional()?;
        if conflict.is_some() {
            bail!("asset path already exists: {new_string}");
        }
        let old_id: Option<i64> = tx
            .query_row("SELECT id FROM assets WHERE path = ?1", [&old_string], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(old_id) = old_id else {
            bail!("asset not found: {old_string}");
        };
        tx.execute(
            "UPDATE assets SET
                source_id = ?1, path = ?2, folder = ?3, name = ?4, mime = ?5,
                width = ?6, height = ?7, bytes = ?8, mtime = ?9, thumb = ?10,
                sha1 = ?11, blake3 = ?12, phash = ?13, damage = ?14,
                thumb_state = 'done'
             WHERE id = ?15",
            params![
                source_id,
                new_string,
                folder,
                name,
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
            let result = (|| -> Result<HashSet<PathBuf>> {
                let mut statement = self.conn.prepare(&sql)?;
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
                Ok(paths) if !paths.is_empty() => return Ok(paths),
                Ok(_) => {}
                Err(error) => log::warn!("fts search failed, falling back to LIKE: {error:#}"),
            }
        }
        let mut sql = format!("SELECT path FROM assets WHERE folder IN ({placeholders})");
        let mut values = Vec::with_capacity(tokens.len());
        for (index, token) in tokens.iter().enumerate() {
            let slot = folders.len() + index + 1;
            sql.push_str(&format!(
                " AND (name LIKE ?{slot} ESCAPE '\\' OR path LIKE ?{slot} ESCAPE '\\' OR tags_text LIKE ?{slot} ESCAPE '\\')"
            ));
            values.push(format!("%{}%", escape_like(token)));
        }
        let mut statement = self.conn.prepare(&sql)?;
        let mut parameters: Vec<&dyn ToSql> = Vec::with_capacity(folders.len() + values.len());
        for folder in folders {
            parameters.push(folder);
        }
        for value in &values {
            parameters.push(value);
        }
        let rows = statement.query_map(rusqlite::params_from_iter(parameters), |row| {
            row.get::<_, String>(0)
        })?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    pub fn tags_missing_vocab(&self) -> Result<Vec<String>> {
        let mut statement = self
            .conn
            .prepare(
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

    fn load_asset(&self, path: &Path) -> Result<Asset> {
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

    fn manual_tags_for_folders(
        &self,
        folders: &[String],
    ) -> Result<HashMap<PathBuf, Vec<TagHit>>> {
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
    let mut statement =
        conn.prepare("SELECT DISTINCT asset_id FROM asset_tags WHERE tag_id = ?1 ORDER BY asset_id")?;
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
            params![
                tag.category,
                tag.vocab_index.map(|value| value as i64),
                id
            ],
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
        _ => "application/octet-stream",
    }
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
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
        conn.execute_batch("ALTER TABLE assets ADD COLUMN metadata_json TEXT NOT NULL DEFAULT '{}';")?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_assets_captured_at ON assets(captured_at);",
    )?;
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
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_tags_alias_of ON tags(alias_of);",
    )?;
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
    use super::Db;
    use crate::dedup::{DuplicateReview, ReviewState};
    use crate::media::ContentHashes;
    use crate::model::Filter;
    use rusqlite::Connection;
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
        let root = std::env::temp_dir().join(format!(
            "kanalib-rename-asset-test-{}",
            std::process::id()
        ));
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
        assert!(
            moved
                .manual_tags
                .iter()
                .any(|tag| tag.tag == "manual_tag")
        );
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
        let root = std::env::temp_dir().join(format!(
            "kanalib-tag-alias-test-{}",
            std::process::id()
        ));
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
        assert!(
            asset
                .manual_tags
                .iter()
                .any(|tag| tag.tag == "manual_tag")
        );
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
            let hits = db.search_paths("C:\\lib", &["manualtag".to_string()]).unwrap();
            assert!(hits.contains(&clara_path));
            let summary = db
                .list_tags("C:\\lib")
                .unwrap()
                .into_iter()
                .find(|tag| tag.name == "manualtag")
                .expect("manual tag summary");
            assert_eq!((summary.count, summary.manual_count), (1, 1));
            assert_eq!(db.remove_tags(&[clara_path.clone()], "manualtag").unwrap(), 1);
            let clara = db
                .load_folder("C:\\lib")
                .unwrap()
                .into_iter()
                .find(|asset| asset.path.ends_with("克拉拉.png"))
                .expect("clara without manual tag");
            assert!(clara.manual_tags.is_empty());
            let hits = db.search_paths("C:\\lib", &["manualtag".to_string()]).unwrap();
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
    fn multi_root_library_loads_searches_and_counts() {
        let root = std::env::temp_dir().join(format!(
            "kanalib-multi-root-test-{}",
            std::process::id()
        ));
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
        assert!(assets
            .iter()
            .any(|asset| asset.library_root == PathBuf::from("C:\\lib2")));
        assert!(assets
            .iter()
            .all(|asset| asset.manual_tags.iter().any(|tag| tag.tag == "sharedtag")));
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
        assert_eq!(
            db.load_library(&["C:\\lib".to_string()]).unwrap().len(),
            1
        );
        let _ = fs::remove_dir_all(&root);
    }
}
