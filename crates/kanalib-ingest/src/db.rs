//! 服务自己的 SQLite：schema v1、短事务与崩溃恢复。

use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use sha1::Digest;

use crate::source::{AiType, Rating};

pub const SCHEMA_VERSION: u32 = 7;

const MIGRATION_V1: &str = r#"
CREATE TABLE IF NOT EXISTS schema_meta (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS sources (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    config_json TEXT NOT NULL DEFAULT '{}',
    credential_ref TEXT,
    last_sync_at INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS authors (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    external_id TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    avatar_url TEXT,
    avatar_path TEXT,
    avatar_updated_at INTEGER,
    deleted INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER,
    UNIQUE(source, external_id)
);

CREATE TABLE IF NOT EXISTS works (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    external_id TEXT NOT NULL,
    author_id TEXT REFERENCES authors(id) ON DELETE SET NULL,
    title TEXT NOT NULL DEFAULT '',
    page_count INTEGER NOT NULL DEFAULT 1,
    posted_at INTEGER,
    rating TEXT NOT NULL DEFAULT 'G',
    ai_marked INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'active',
    metadata_synced_at INTEGER,
    next_retry_at INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(source, external_id)
);

CREATE TABLE IF NOT EXISTS items (
    id TEXT PRIMARY KEY,
    work_id TEXT NOT NULL REFERENCES works(id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    page INTEGER NOT NULL DEFAULT 0,
    original_url TEXT,
    original_path TEXT,
    ext TEXT,
    width INTEGER NOT NULL DEFAULT 0,
    height INTEGER NOT NULL DEFAULT 0,
    bytes INTEGER NOT NULL DEFAULT 0,
    sha1 TEXT,
    phash TEXT,
    thumb_256 TEXT,
    thumb_512 TEXT,
    decision TEXT NOT NULL DEFAULT 'pending'
        CHECK (decision IN ('pending','allow','block','review')),
    decision_reason TEXT,
    rules_version TEXT,
    reviewed_by TEXT,
    reviewed_at INTEGER,
    revision INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(work_id, page)
);

CREATE TABLE IF NOT EXISTS tags (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT '',
    translated_name TEXT,
    UNIQUE(name)
);

CREATE TABLE IF NOT EXISTS item_tags (
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    score REAL NOT NULL DEFAULT 0,
    origin TEXT NOT NULL DEFAULT 'model',
    PRIMARY KEY(item_id, tag_id, origin)
);

CREATE TABLE IF NOT EXISTS subscriptions (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL,
    user_external_id TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_seen_work_id TEXT,
    last_checked_at INTEGER,
    next_check_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(source, user_external_id)
);

CREATE TABLE IF NOT EXISTS download_queue (
    id INTEGER PRIMARY KEY,
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    state TEXT NOT NULL DEFAULT 'queued'
        CHECK (state IN ('queued','running','done','failed','paused')),
    priority INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_retry_at INTEGER,
    last_error TEXT,
    started_at INTEGER,
    finished_at INTEGER,
    UNIQUE(item_id)
);

CREATE TABLE IF NOT EXISTS decisions (
    id INTEGER PRIMARY KEY,
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    decision TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    rules_version TEXT NOT NULL,
    matched_rules_json TEXT NOT NULL DEFAULT '[]',
    tags_json TEXT NOT NULL DEFAULT '[]',
    decided_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS push_batches (
    id TEXT PRIMARY KEY,
    item_count INTEGER NOT NULL DEFAULT 0,
    payload_hash TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    next_retry_at INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    acked_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_items_decision ON items(decision, updated_at);
CREATE INDEX IF NOT EXISTS idx_items_work ON items(work_id, page);
CREATE INDEX IF NOT EXISTS idx_works_author ON works(author_id);
CREATE INDEX IF NOT EXISTS idx_queue_state ON download_queue(state, priority DESC, next_retry_at);
CREATE INDEX IF NOT EXISTS idx_decisions_item ON decisions(item_id, decided_at DESC);
CREATE INDEX IF NOT EXISTS idx_push_state ON push_batches(state, next_retry_at);
"#;

const MIGRATION_V2: &str = r#"
ALTER TABLE push_batches ADD COLUMN payload_json TEXT NOT NULL DEFAULT '';
ALTER TABLE push_batches ADD COLUMN kind TEXT NOT NULL DEFAULT 'allow';
"#;

const MIGRATION_V3: &str = r#"
ALTER TABLE works ADD COLUMN x_restrict INTEGER;
ALTER TABLE works ADD COLUMN sanity_level INTEGER;
ALTER TABLE works ADD COLUMN restriction_attributes TEXT NOT NULL DEFAULT '[]';
ALTER TABLE works ADD COLUMN ai_type TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE works ADD COLUMN tag_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE items ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0;

-- 旧行没有记录来源分级：不能把“当年没记”继续当成已确认全年龄。
UPDATE works SET rating = 'unknown' WHERE lower(rating) NOT IN ('all', 'r18', 'r18g');
UPDATE works SET rating = lower(rating) WHERE lower(rating) IN ('all', 'r18', 'r18g');
UPDATE works SET ai_type = CASE WHEN ai_marked = 1 THEN 'ai' ELSE 'unknown' END;
"#;

const MIGRATION_V4: &str = r#"
ALTER TABLE items ADD COLUMN pushed_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE items ADD COLUMN delete_pushed INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_items_push
    ON items(decision, pushed_revision, revision);
CREATE INDEX IF NOT EXISTS idx_items_delete
    ON items(delete_pushed, work_id);
"#;

const MIGRATION_V5: &str = r#"
DROP TABLE IF EXISTS rules;
ALTER TABLE decisions ADD COLUMN tag_source TEXT NOT NULL DEFAULT 'source';
ALTER TABLE decisions ADD COLUMN tag_count_used INTEGER NOT NULL DEFAULT 0;
"#;

const MIGRATION_V6: &str = r#"
CREATE TABLE IF NOT EXISTS rule_snapshots (
    version TEXT PRIMARY KEY,
    config_hash TEXT NOT NULL,
    config_json TEXT NOT NULL,
    loaded_at INTEGER NOT NULL DEFAULT (unixepoch()),
    loaded_by TEXT
);

CREATE INDEX IF NOT EXISTS idx_rule_snapshots_loaded
    ON rule_snapshots(loaded_at DESC);
"#;

const MIGRATION_V7: &str = r#"
CREATE TABLE IF NOT EXISTS rule_proposals (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    source_item_id TEXT,
    note TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT 'pending',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    decided_at INTEGER,
    decided_by TEXT,
    apply_error TEXT
);

CREATE INDEX IF NOT EXISTS idx_rule_proposals_state
    ON rule_proposals(state, created_at);
"#;

#[derive(Debug)]
pub struct Db {
    connection: Connection,
}

impl Db {
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating database directory {}", parent.display()))?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        Self::from_connection(connection)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    /// 只读连接：媒体回源等热路径不跑迁移，也不拿写锁。
    pub fn open_read(path: &Path) -> Result<Self> {
        let connection = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("opening database read-only {}", path.display()))?;
        connection.pragma_update(None, "busy_timeout", "5000")?;
        Ok(Self { connection })
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "busy_timeout", "5000")?;
        let mut db = Self { connection };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<()> {
        let tx = self.connection.transaction()?;
        let table_exists: bool = tx
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_meta'",
                [],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        let version = if table_exists {
            tx.query_row("SELECT MAX(version) FROM schema_meta", [], |row| {
                row.get::<_, Option<u32>>(0)
            })?
            .unwrap_or(0)
        } else {
            0
        };
        if version > SCHEMA_VERSION {
            bail!("database schema version {version} is newer than supported {SCHEMA_VERSION}");
        }
        if version < 1 {
            tx.execute_batch(MIGRATION_V1)?;
        }
        if version < 2 {
            tx.execute_batch(MIGRATION_V2)?;
        }
        if version < 3 {
            tx.execute_batch(MIGRATION_V3)?;
        }
        if version < 4 {
            tx.execute_batch(MIGRATION_V4)?;
        }
        if version < 5 {
            tx.execute_batch(MIGRATION_V5)?;
        }
        if version < 6 {
            tx.execute_batch(MIGRATION_V6)?;
        }
        if version < 7 {
            tx.execute_batch(MIGRATION_V7)?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO schema_meta(version) VALUES (?1)",
            params![SCHEMA_VERSION],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        Ok(self.connection.transaction()?)
    }

    pub fn recover_running_downloads(&self) -> Result<usize> {
        let changed = self.connection.execute(
            "UPDATE download_queue
                SET state='queued', started_at=NULL, next_retry_at=NULL,
                    last_error=COALESCE(last_error, 'interrupted by restart')
              WHERE state='running'",
            [],
        )?;
        Ok(changed)
    }

    /// 到期的 `paused` 行重新入队。
    ///
    /// 配额/限流暂停会写 `next_retry_at`；`pending_item_ids` 只认 queued/failed，
    /// 没有这一步，暂停行会永久停在队列里。人工暂停（`next_retry_at IS NULL`）
    /// 保持不动，等人工恢复。
    pub fn requeue_due_paused_downloads(&self) -> Result<usize> {
        let changed = self.connection.execute(
            "UPDATE download_queue
                SET state='queued'
              WHERE state='paused' AND next_retry_at IS NOT NULL AND next_retry_at <= ?1",
            params![now()],
        )?;
        Ok(changed)
    }

    pub fn sync_source(&self, id: &str, kind: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO sources(id, kind) VALUES (?1, ?2) ON CONFLICT(id) DO NOTHING",
            params![id, kind],
        )?;
        Ok(())
    }

    pub fn record_source_sync(&self, id: &str, at: i64) -> Result<()> {
        self.connection.execute(
            "UPDATE sources SET last_sync_at=?2, last_error=NULL WHERE id=?1",
            params![id, at],
        )?;
        Ok(())
    }

    pub fn persist_bookmark_image(
        &self,
        item_id: &str,
        url: &str,
        ext: Option<&str>,
    ) -> Result<()> {
        self.connection.execute(
            "UPDATE items
                SET original_url=?2, ext=COALESCE(?3, ext), favorite=1
              WHERE id=?1",
            params![item_id, url, ext],
        )?;
        Ok(())
    }

    pub fn upsert_author(tx: &Transaction<'_>, author: &SourceAuthorRow) -> Result<String> {
        let id = derived_id(
            "author",
            &format!("{}:{}", author.source, author.external_id),
        );
        let existing: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT name, avatar_url FROM authors WHERE source=?1 AND external_id=?2",
                params![author.source, author.external_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let now = now();
        match existing {
            None => {
                tx.execute(
                    "INSERT INTO authors(id, source, external_id, name, avatar_url, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        id,
                        author.source,
                        author.external_id,
                        author.name,
                        author.avatar_url,
                        now
                    ],
                )?;
            }
            Some((old_name, old_avatar)) => {
                if old_name != author.name || old_avatar.as_deref() != author.avatar_url.as_deref()
                {
                    tx.execute(
                        "UPDATE authors SET name=?1, avatar_url=?2, avatar_updated_at=?3,
                                updated_at=?3 WHERE id=?4",
                        params![author.name, author.avatar_url, now, id],
                    )?;
                }
            }
        }
        Ok(id)
    }

    pub fn upsert_work(tx: &Transaction<'_>, work: &SourceWorkRow) -> Result<String> {
        let author_id = work
            .author
            .as_ref()
            .map(|author| Self::upsert_author(tx, author))
            .transpose()?;
        let id = derived_id("work", &format!("{}:{}", work.source, work.external_id));
        let now = now();
        tx.execute(
            "INSERT INTO works(id, source, external_id, author_id, title, page_count, posted_at,
                                rating, ai_marked, ai_type, x_restrict, sanity_level,
                                restriction_attributes, tag_count, status, metadata_synced_at,
                                next_retry_at, last_error, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     ?17, NULL, ?18)
             ON CONFLICT(source, external_id) DO UPDATE SET
                author_id=excluded.author_id,
                title=excluded.title,
                page_count=excluded.page_count,
                posted_at=excluded.posted_at,
                rating=excluded.rating,
                ai_marked=excluded.ai_marked,
                ai_type=excluded.ai_type,
                x_restrict=excluded.x_restrict,
                sanity_level=excluded.sanity_level,
                restriction_attributes=excluded.restriction_attributes,
                tag_count=excluded.tag_count,
                status=CASE WHEN works.status='deleted' THEN 'deleted' ELSE excluded.status END,
                metadata_synced_at=excluded.metadata_synced_at,
                next_retry_at=excluded.next_retry_at,
                last_error=NULL,
                updated_at=excluded.updated_at",
            params![
                id,
                work.source,
                work.external_id,
                author_id,
                work.title,
                work.page_count,
                work.posted_at,
                work.rating.as_str(),
                i64::from(work.ai == AiType::Ai),
                work.ai.as_str(),
                work.x_restrict,
                work.sanity_level,
                work.restriction_attributes,
                work.tag_count,
                work.status,
                work.metadata_synced_at.unwrap_or(now),
                work.next_retry_at,
                now
            ],
        )?;
        let stored_id: String = tx.query_row(
            "SELECT id FROM works WHERE source=?1 AND external_id=?2",
            params![work.source, work.external_id],
            |row| row.get(0),
        )?;
        Ok(stored_id)
    }

    pub fn item_exists(&self, item_id: &str) -> Result<bool> {
        Ok(self
            .connection
            .query_row("SELECT 1 FROM items WHERE id=?1", params![item_id], |_| {
                Ok(())
            })
            .optional()?
            .is_some())
    }

    pub fn persist_page(
        tx: &Transaction<'_>,
        item_id: &str,
        work_id: &str,
        source: &str,
        page: u32,
        url: Option<&str>,
        ext: Option<&str>,
    ) -> Result<()> {
        let now = now();
        tx.execute(
            "INSERT INTO items(id, work_id, source, page, original_url, ext, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                original_url=COALESCE(excluded.original_url, items.original_url),
                ext=COALESCE(excluded.ext, items.ext),
                updated_at=excluded.updated_at",
            params![item_id, work_id, source, page, url, ext, now],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO download_queue(item_id, state) VALUES (?1, 'queued')",
            params![item_id],
        )?;
        Ok(())
    }

    pub fn record_work_tags(&self, work_id: &str, tags: &[TagRecord]) -> Result<()> {
        let tx = self.connection.unchecked_transaction()?;
        for tag in tags {
            let tag_id = upsert_tag(
                &tx,
                &tag.name,
                &tag.category,
                tag.translated_name.as_deref(),
            )?;
            let mut statement = tx.prepare("SELECT id FROM items WHERE work_id=?1")?;
            let item_ids = statement
                .query_map(params![work_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            for item_id in item_ids {
                tx.execute(
                    "INSERT OR IGNORE INTO item_tags(item_id, tag_id, score, origin)
                     VALUES (?1, ?2, ?3, 'pixiv')",
                    params![item_id, tag_id, tag.score],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn record_model_tags(&self, item_id: &str, tags: &[String]) -> Result<()> {
        let tx = self.connection.unchecked_transaction()?;
        for tag in tags {
            let tag_id = upsert_tag(&tx, tag, "model", None)?;
            tx.execute(
                "INSERT OR IGNORE INTO item_tags(item_id, tag_id, score, origin)
                 VALUES (?1, ?2, 1.0, 'model')",
                params![item_id, tag_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn mark_work_deleted(&self, work_id: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE works SET status='deleted', updated_at=?1 WHERE id=?2",
            params![now(), work_id],
        )?;
        Ok(())
    }

    pub fn finish_download(&self, item_id: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE download_queue SET state='done', last_error=NULL, finished_at=?1
              WHERE item_id=?2",
            params![now(), item_id],
        )?;
        Ok(())
    }

    pub fn retry_download(&self, item_id: &str, error: &str, retry_at: i64) -> Result<()> {
        self.connection.execute(
            "UPDATE download_queue
                SET state='failed', attempts=attempts+1, last_error=?2, next_retry_at=?3,
                    finished_at=?4
              WHERE item_id=?1",
            params![item_id, error, retry_at, now()],
        )?;
        Ok(())
    }

    pub fn pause_downloads(&self, retry_at: Option<i64>, error: &str) -> Result<usize> {
        Ok(self.connection.execute(
            "UPDATE download_queue
                SET state='paused', last_error=?1, next_retry_at=?2
              WHERE state IN ('queued','failed')",
            params![error, retry_at],
        )?)
    }

    pub fn claim_download(&self, item_id: &str) -> Result<bool> {
        let changed = self.connection.execute(
            "UPDATE download_queue
                SET state='running', started_at=?1, next_retry_at=NULL
              WHERE item_id=?2 AND state IN ('queued','failed')
                AND (next_retry_at IS NULL OR next_retry_at <= ?1)",
            params![now(), item_id],
        )?;
        Ok(changed == 1)
    }

    pub fn queue_row(&self, item_id: &str) -> Result<Option<QueueRow>> {
        self.connection
            .query_row(
                "SELECT id, item_id, state, attempts, next_retry_at, last_error
                   FROM download_queue WHERE item_id=?1",
                params![item_id],
                |row| {
                    Ok(QueueRow {
                        id: row.get(0)?,
                        item_id: row.get(1)?,
                        state: row.get(2)?,
                        attempts: row.get(3)?,
                        next_retry_at: row.get(4)?,
                        last_error: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn pending_item_ids(&self) -> Result<Vec<String>> {
        let mut statement = self.connection.prepare(
            "SELECT item_id FROM download_queue
              WHERE state IN ('queued','failed')
                AND (next_retry_at IS NULL OR next_retry_at <= ?1)
              ORDER BY priority DESC, next_retry_at, id",
        )?;
        statement
            .query_map(params![now()], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn item_row(&self, item_id: &str) -> Result<Option<ItemRow>> {
        self.connection
            .query_row(
                "SELECT id, work_id, source, page, original_url, original_path, ext, width, height,
                        bytes, sha1, thumb_256, thumb_512, decision
                   FROM items WHERE id=?1",
                params![item_id],
                |row| {
                    Ok(ItemRow {
                        id: row.get(0)?,
                        work_id: row.get(1)?,
                        source: row.get(2)?,
                        page: row.get(3)?,
                        original_url: row.get(4)?,
                        original_path: row.get(5)?,
                        ext: row.get(6)?,
                        width: row.get(7)?,
                        height: row.get(8)?,
                        bytes: row.get(9)?,
                        sha1: row.get(10)?,
                        thumb_256: row.get(11)?,
                        thumb_512: row.get(12)?,
                        decision: row.get(13)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn work_external_id(&self, work_id: &str) -> Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT external_id FROM works WHERE id=?1",
                params![work_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn all_items(&self) -> Result<Vec<ItemRow>> {
        let mut statement = self.connection.prepare(
            "SELECT id, work_id, source, page, original_url, original_path, ext, width, height,
                    bytes, sha1, thumb_256, thumb_512, decision
               FROM items ORDER BY work_id, page",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(ItemRow {
                id: row.get(0)?,
                work_id: row.get(1)?,
                source: row.get(2)?,
                page: row.get(3)?,
                original_url: row.get(4)?,
                original_path: row.get(5)?,
                ext: row.get(6)?,
                width: row.get(7)?,
                height: row.get(8)?,
                bytes: row.get(9)?,
                sha1: row.get(10)?,
                thumb_256: row.get(11)?,
                thumb_512: row.get(12)?,
                decision: row.get(13)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn pending_decision_items(&self) -> Result<Vec<PendingDecisionItem>> {
        let mut statement = self.connection.prepare(
            "SELECT i.id, i.source, i.original_path, i.thumb_512, i.thumb_256, w.rating,
                    w.x_restrict, w.sanity_level, w.restriction_attributes, w.ai_type,
                    w.tag_count, i.width, i.height, i.bytes, i.sha1
               FROM items i JOIN works w ON w.id = i.work_id
              WHERE i.decision = 'pending'
                AND i.original_path IS NOT NULL
              ORDER BY i.updated_at, i.id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(PendingDecisionItem {
                id: row.get(0)?,
                source: row.get(1)?,
                original_path: row.get(2)?,
                thumb_512: row.get(3)?,
                thumb_256: row.get(4)?,
                rating: row.get(5)?,
                x_restrict: row.get(6)?,
                sanity_level: row.get(7)?,
                restriction_attributes: row.get(8)?,
                ai_type: row.get(9)?,
                tag_count: row.get::<_, i64>(10)?.max(0) as usize,
                width: row.get(11)?,
                height: row.get(12)?,
                bytes: row.get::<_, i64>(13)?.max(0) as u64,
                sha1: row.get(14)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn item_tag_names(&self, item_id: &str) -> Result<Vec<(String, Option<String>)>> {
        let mut statement = self.connection.prepare(
            "SELECT t.name, t.translated_name
               FROM item_tags it JOIN tags t ON t.id = it.tag_id
              WHERE it.item_id = ?1
              ORDER BY t.name",
        )?;
        let rows = statement.query_map([item_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// 该条目是否已有模型补标结果；重跑规则时据此跳过第二次打标。
    pub fn has_model_tags(&self, item_id: &str) -> Result<bool> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM item_tags WHERE item_id = ?1 AND origin = 'model'",
            [item_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn item_author(&self, item_id: &str) -> Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT a.external_id
                   FROM items i JOIN works w ON w.id = i.work_id
                    LEFT JOIN authors a ON a.id = w.author_id
                  WHERE i.id = ?1",
                [item_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn source_enabled(&self, source: &str) -> Result<bool> {
        let enabled: Option<i64> = self
            .connection
            .query_row(
                "SELECT enabled FROM sources WHERE id = ?1",
                [source],
                |row| row.get(0),
            )
            .optional()?;
        Ok(enabled.unwrap_or(1) != 0)
    }

    /// 记录生效过的规则快照；同版本只保留最新一份副本。
    pub fn record_rule_snapshot(
        &self,
        version: &str,
        config_hash: &str,
        config_json: &str,
        loaded_by: &str,
    ) -> Result<()> {
        self.connection.execute(
            "INSERT INTO rule_snapshots(version, config_hash, config_json, loaded_at, loaded_by)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(version) DO UPDATE SET
                config_hash=excluded.config_hash,
                config_json=excluded.config_json,
                loaded_at=excluded.loaded_at,
                loaded_by=excluded.loaded_by",
            params![version, config_hash, config_json, now(), loaded_by],
        )?;
        Ok(())
    }

    pub fn rule_snapshot(&self, version: &str) -> Result<Option<RuleSnapshot>> {
        self.connection
            .query_row(
                "SELECT version, config_hash, config_json, loaded_at, loaded_by
                   FROM rule_snapshots WHERE version=?1",
                params![version],
                |row| {
                    Ok(RuleSnapshot {
                        version: row.get(0)?,
                        config_hash: row.get(1)?,
                        config_json: row.get(2)?,
                        loaded_at: row.get(3)?,
                        loaded_by: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn latest_rule_snapshot(&self) -> Result<Option<RuleSnapshot>> {
        self.connection
            .query_row(
                "SELECT version, config_hash, config_json, loaded_at, loaded_by
                   FROM rule_snapshots
                  ORDER BY loaded_at DESC, version DESC
                  LIMIT 1",
                [],
                |row| {
                    Ok(RuleSnapshot {
                        version: row.get(0)?,
                        config_hash: row.get(1)?,
                        config_json: row.get(2)?,
                        loaded_at: row.get(3)?,
                        loaded_by: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn insert_rule_proposal(
        &self,
        kind: &str,
        payload_json: &str,
        source_item_id: Option<&str>,
        note: &str,
    ) -> Result<i64> {
        self.connection.execute(
            "INSERT INTO rule_proposals(kind, payload_json, source_item_id, note)
             VALUES (?1, ?2, ?3, ?4)",
            params![kind, payload_json, source_item_id, note],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn rule_proposal(&self, id: i64) -> Result<Option<RuleProposal>> {
        self.connection
            .query_row(
                "SELECT id, kind, payload_json, source_item_id, note, state,
                        created_at, decided_at, decided_by, apply_error
                   FROM rule_proposals WHERE id=?1",
                params![id],
                rule_proposal_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn rule_proposals(&self, state: Option<&str>) -> Result<Vec<RuleProposal>> {
        let mut statement = self.connection.prepare(
            "SELECT id, kind, payload_json, source_item_id, note, state,
                    created_at, decided_at, decided_by, apply_error
               FROM rule_proposals
              WHERE (?1 IS NULL OR state = ?1)
              ORDER BY created_at DESC, id DESC",
        )?;
        let rows = statement.query_map(params![state], rule_proposal_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn set_rule_proposal_state(
        &self,
        id: i64,
        state: &str,
        decided_by: &str,
        apply_error: Option<&str>,
    ) -> Result<()> {
        self.connection.execute(
            "UPDATE rule_proposals
                SET state=?2, decided_at=?3, decided_by=?4, apply_error=?5
              WHERE id=?1",
            params![id, state, now(), decided_by, apply_error],
        )?;
        Ok(())
    }

    pub fn pending_proposal_count(&self) -> Result<i64> {
        Ok(self.connection.query_row(
            "SELECT COUNT(*) FROM rule_proposals WHERE state='pending'",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn decision_counts(&self) -> Result<Vec<(String, i64)>> {
        let mut statement = self
            .connection
            .prepare("SELECT decision, COUNT(*) FROM items GROUP BY decision")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn download_queue_counts(&self) -> Result<Vec<(String, i64)>> {
        let mut statement = self
            .connection
            .prepare("SELECT state, COUNT(*) FROM download_queue GROUP BY state")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn push_pending_count(&self) -> Result<i64> {
        Ok(self.connection.query_row(
            "SELECT COUNT(*) FROM push_batches
              WHERE state='pending' AND (next_retry_at IS NULL OR next_retry_at <= ?1)",
            params![now()],
            |row| row.get(0),
        )?)
    }

    pub fn review_backlog(&self) -> Result<(i64, Option<i64>)> {
        self.connection
            .query_row(
                "SELECT COUNT(*), MIN(updated_at) FROM items WHERE decision='review'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(Into::into)
    }

    /// 把条目放回 pending，交给下一轮决策；已 allow 的条目需要显式确认。
    pub fn requeue_decision(&self, item_id: &str, include_allow: bool) -> Result<bool> {
        let changed = self.connection.execute(
            "UPDATE items SET decision='pending', decision_reason=NULL, updated_at=?1
              WHERE id=?2
                AND (decision='review' OR decision='block' OR (?3 AND decision='allow'))",
            params![now(), item_id, i64::from(include_allow)],
        )?;
        Ok(changed == 1)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_decision(
        &self,
        item_id: &str,
        decision: &str,
        reason: &str,
        matched: &[String],
        rules_version: &str,
        tag_source: &str,
        tag_count_used: usize,
    ) -> Result<()> {
        let matched_json = serde_json::to_string(matched)?;
        let tx = self.connection.unchecked_transaction()?;
        tx.execute(
            "UPDATE items SET decision=?2, decision_reason=?3, rules_version=?4,
                    revision=revision+1, updated_at=?5
              WHERE id=?1",
            params![item_id, decision, reason, rules_version, now()],
        )?;
        tx.execute(
            "INSERT INTO decisions(item_id, decision, reason, rules_version,
                                   matched_rules_json, tag_source, tag_count_used, tags_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '[]')",
            params![
                item_id,
                decision,
                reason,
                rules_version,
                matched_json,
                tag_source,
                tag_count_used as i64
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_download_result(&self, item_id: &str, result: &DownloadResult) -> Result<()> {
        self.connection.execute(
            "UPDATE items SET original_path=?2, ext=?3, bytes=?4, sha1=?5, width=?6, height=?7,
                    revision=revision+1, updated_at=?8
              WHERE id=?1",
            params![
                item_id,
                result.path,
                result.ext,
                result.bytes,
                result.sha1,
                result.width,
                result.height,
                now()
            ],
        )?;
        Ok(())
    }

    pub fn set_thumbnails(&self, item_id: &str, thumb_256: &str, thumb_512: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE items SET thumb_256=?2, thumb_512=?3, updated_at=?4 WHERE id=?1",
            params![item_id, thumb_256, thumb_512, now()],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SourceAuthorRow {
    pub source: String,
    pub external_id: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub path: String,
    pub ext: String,
    pub bytes: u64,
    pub sha1: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct SourceWorkRow {
    pub source: String,
    pub external_id: String,
    pub author: Option<SourceAuthorRow>,
    pub title: String,
    pub page_count: u32,
    pub posted_at: Option<i64>,
    pub rating: Rating,
    pub ai: AiType,
    pub x_restrict: Option<i64>,
    pub sanity_level: Option<i64>,
    pub restriction_attributes: String,
    pub tag_count: i64,
    pub status: &'static str,
    pub tags: Vec<TagRecord>,
    pub metadata_synced_at: Option<i64>,
    pub next_retry_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TagRecord {
    pub name: String,
    pub category: String,
    pub score: f64,
    pub translated_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QueueRow {
    pub id: i64,
    pub item_id: String,
    pub state: String,
    pub attempts: u32,
    pub next_retry_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ItemRow {
    pub id: String,
    pub source: String,
    pub work_id: String,
    pub page: u32,
    pub original_url: Option<String>,
    pub original_path: Option<String>,
    pub ext: Option<String>,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub sha1: Option<String>,
    pub thumb_256: Option<String>,
    pub thumb_512: Option<String>,
    pub decision: String,
}

#[derive(Debug, Clone)]
pub struct RuleSnapshot {
    pub version: String,
    pub config_hash: String,
    pub config_json: String,
    pub loaded_at: i64,
    pub loaded_by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleProposal {
    pub id: i64,
    pub kind: String,
    pub payload_json: String,
    pub source_item_id: Option<String>,
    pub note: String,
    pub state: String,
    pub created_at: i64,
    pub decided_at: Option<i64>,
    pub decided_by: Option<String>,
    pub apply_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingDecisionItem {
    pub id: String,
    pub source: String,
    pub original_path: Option<String>,
    pub thumb_512: Option<String>,
    pub thumb_256: Option<String>,
    pub rating: String,
    pub x_restrict: Option<i64>,
    pub sanity_level: Option<i64>,
    pub restriction_attributes: String,
    pub ai_type: String,
    pub tag_count: usize,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub sha1: Option<String>,
}

fn upsert_tag(
    tx: &Transaction<'_>,
    name: &str,
    category: &str,
    translated_name: Option<&str>,
) -> Result<i64> {
    tx.execute(
        "INSERT INTO tags(name, category, translated_name) VALUES (?1, ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET category=excluded.category,
             translated_name=COALESCE(excluded.translated_name, tags.translated_name)",
        params![name, category, translated_name],
    )?;
    Ok(
        tx.query_row("SELECT id FROM tags WHERE name=?1", params![name], |row| {
            row.get(0)
        })?,
    )
}

fn rule_proposal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuleProposal> {
    Ok(RuleProposal {
        id: row.get(0)?,
        kind: row.get(1)?,
        payload_json: row.get(2)?,
        source_item_id: row.get(3)?,
        note: row.get(4)?,
        state: row.get(5)?,
        created_at: row.get(6)?,
        decided_at: row.get(7)?,
        decided_by: row.get(8)?,
        apply_error: row.get(9)?,
    })
}

fn derived_id(kind: &str, key: &str) -> String {
    let digest = sha1::Sha1::digest(format!("kanalib:{kind}:{key}").as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
