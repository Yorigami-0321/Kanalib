//! 网页自己的 SQLite：schema v2（本地发布新增 `items.origin`）、只读连接与测试样本。

use std::{collections::HashMap, path::Path, time::Duration};

use anyhow::{Context, Result};
use kanalib_wall_model::{
    AuthorDto, CollectionDto, IngestBatch, IngestBatchItem, IngestDelete, IngestTag, WallItemDto,
    WorkDto, WorkPageDto, normalize_rating, valid_item_id,
};
use rusqlite::types::Value as SqlValue;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Row, Transaction, functions::FunctionFlags, params,
    params_from_iter,
};
use serde::Serialize;

use crate::query::{
    Cursor, ItemFilter, ItemSort, SearchField, SearchTerm, SortKey, SortKind, parse_search,
};

pub const SCHEMA_VERSION: u32 = 2;

pub const SAMPLE_ITEM_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const SAMPLE_ITEM_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub const SAMPLE_AUTHOR: &str = "author-1";
pub const SAMPLE_WORK: &str = "work-1";

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS items (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    source_id TEXT,
    work_id TEXT,
    title TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL DEFAULT '',
    width INTEGER NOT NULL DEFAULT 0,
    height INTEGER NOT NULL DEFAULT 0,
    rating TEXT NOT NULL DEFAULT 'G',
    star INTEGER NOT NULL DEFAULT 0,
    favorite INTEGER NOT NULL DEFAULT 0,
    mtime INTEGER NOT NULL DEFAULT 0,
    posted_at INTEGER,
    orig_ext TEXT,
    sharp INTEGER NOT NULL DEFAULT 0,
    phash TEXT,
    group_name TEXT NOT NULL DEFAULT '',
    decision TEXT NOT NULL DEFAULT 'allow',
    rules_version TEXT,
    revision INTEGER NOT NULL DEFAULT 1,
    deleted INTEGER NOT NULL DEFAULT 0,
    origin TEXT NOT NULL DEFAULT 'ingest',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS works (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    external_id TEXT,
    title TEXT NOT NULL DEFAULT '',
    author_id TEXT,
    page_count INTEGER NOT NULL DEFAULT 1,
    posted_at INTEGER,
    status TEXT NOT NULL DEFAULT 'active',
    cover_item_id TEXT REFERENCES items(id) ON DELETE SET NULL,
    UNIQUE(source, external_id)
);

CREATE TABLE IF NOT EXISTS work_items (
    work_id TEXT NOT NULL REFERENCES works(id) ON DELETE CASCADE,
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    page INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(work_id, page),
    UNIQUE(item_id)
);

CREATE TABLE IF NOT EXISTS authors (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    external_id TEXT,
    name TEXT NOT NULL DEFAULT '',
    avatar_url TEXT,
    updated_at INTEGER,
    UNIQUE(source, external_id)
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
    origin TEXT NOT NULL DEFAULT 'ingest',
    PRIMARY KEY(item_id, tag_id, origin)
);

CREATE TABLE IF NOT EXISTS collections (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'favorite',
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS collection_items (
    collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    PRIMARY KEY(collection_id, item_id)
);

CREATE TABLE IF NOT EXISTS ingest_batches (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    item_count INTEGER NOT NULL DEFAULT 0,
    rules_version TEXT,
    received_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS desktop_devices (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    token_hash TEXT NOT NULL,
    can_pull INTEGER NOT NULL DEFAULT 1,
    can_push INTEGER NOT NULL DEFAULT 1,
    revoked INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    last_seen_at INTEGER
);

CREATE VIRTUAL TABLE IF NOT EXISTS item_search USING fts5(
    item_id UNINDEXED,
    name,
    tags,
    group_name,
    rating,
    tokenize = 'trigram'
);

CREATE INDEX IF NOT EXISTS idx_items_updated ON items(deleted, updated_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_items_work ON items(work_id);
CREATE INDEX IF NOT EXISTS idx_items_deleted_id ON items(deleted, id);
CREATE INDEX IF NOT EXISTS idx_items_name ON items(deleted, name COLLATE NOCASE, id);
CREATE INDEX IF NOT EXISTS idx_items_star ON items(deleted, star DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_items_rating ON items(
    deleted,
    CASE rating WHEN 'G' THEN 0 WHEN 'S' THEN 1 WHEN 'Q' THEN 2 WHEN 'E' THEN 3 ELSE 0 END,
    id
);
CREATE INDEX IF NOT EXISTS idx_items_favorite ON items(deleted, favorite DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_items_posted ON items(deleted, posted_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_works_author ON works(author_id);
CREATE INDEX IF NOT EXISTS idx_item_tags_tag ON item_tags(tag_id);
CREATE INDEX IF NOT EXISTS idx_tags_name ON tags(name);
CREATE INDEX IF NOT EXISTS idx_collection_items_item ON collection_items(item_id);
CREATE INDEX IF NOT EXISTS idx_desktop_devices_token ON desktop_devices(token_hash);
"#;

/// 重建 FTS5 搜索索引。入库批次或样本写入后调用；读连接不写库。
pub fn rebuild_search_index(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM item_search", [])?;
    conn.execute(
        "INSERT INTO item_search (rowid, item_id, name, tags, group_name, rating)
         SELECT i.rowid, i.id, replace(i.name, '_', ' '),
                COALESCE((SELECT group_concat(replace(t.name, '_', ' '), ' ')
                            FROM item_tags it JOIN tags t ON t.id = it.tag_id
                           WHERE it.item_id = i.id), ''),
                replace(i.group_name, '_', ' '), i.rating
           FROM items i WHERE i.deleted = 0 AND i.decision = 'allow'",
        [],
    )?;
    Ok(())
}

/// 启动时修复 FTS 索引为空或行数与活动条目不一致的情况。
pub fn repair_search_index_if_needed(path: &Path) -> Result<bool> {
    let mut conn = open(path)?;
    let tx = conn.transaction()?;
    let items: i64 = tx.query_row(
        "SELECT COUNT(*) FROM items WHERE deleted = 0 AND decision = 'allow'",
        [],
        |row| row.get(0),
    )?;
    let indexed: i64 = tx.query_row("SELECT COUNT(*) FROM item_search", [], |row| row.get(0))?;
    if items == indexed {
        return Ok(false);
    }
    rebuild_search_index(&tx)?;
    tx.commit()?;
    Ok(true)
}

/// 只刷新受影响的条目；`item_search.rowid` 与 `items.rowid` 对齐后可按主键删改。
pub fn refresh_search_index(conn: &Connection, item_ids: &[String]) -> Result<()> {
    if item_ids.is_empty() {
        return Ok(());
    }
    let mut delete = conn.prepare_cached(
        "DELETE FROM item_search
          WHERE rowid = (SELECT rowid FROM items WHERE id = ?1)",
    )?;
    let mut insert = conn.prepare_cached(
        "INSERT INTO item_search (rowid, item_id, name, tags, group_name, rating)
         SELECT i.rowid, i.id, replace(i.name, '_', ' '),
                COALESCE((SELECT group_concat(replace(t.name, '_', ' '), ' ')
                            FROM item_tags it JOIN tags t ON t.id = it.tag_id
                           WHERE it.item_id = i.id), ''),
                replace(i.group_name, '_', ' '), i.rating
           FROM items i WHERE i.id = ?1 AND i.deleted = 0 AND i.decision = 'allow'",
    )?;
    for item_id in item_ids {
        delete.execute([item_id])?;
        insert.execute([item_id])?;
    }
    Ok(())
}

fn register_functions(conn: &Connection) -> Result<()> {
    conn.create_scalar_function(
        "kanalib_order_hash",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let value: String = ctx.get(0)?;
            let seed: i64 = ctx.get(1)?;
            Ok(order_hash(&value, seed as u64) as i64)
        },
    )?;
    Ok(())
}

fn order_hash(value: &str, seed: u64) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 27;
    hash = hash.wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^ (hash >> 31)
}

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create db directory {}", parent.display()))?;
    }
    let conn =
        Connection::open(path).with_context(|| format!("open database {}", path.display()))?;
    register_functions(&conn)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    conn.execute_batch(SCHEMA_SQL)?;
    ensure_origin_column(&conn)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(conn)
}

/// 旧库没有 `origin` 列；只做一次 additive 迁移，不改动已有行。
fn ensure_origin_column(conn: &Connection) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('items') WHERE name = 'origin')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        conn.execute(
            "ALTER TABLE items ADD COLUMN origin TEXT NOT NULL DEFAULT 'ingest'",
            [],
        )?;
    }
    Ok(())
}

pub fn open_read(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open read-only database {}", path.display()))?;
    register_functions(&conn)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA temp_store = MEMORY; PRAGMA cache_size = -16000;")?;
    Ok(conn)
}

pub fn seed_sample(path: &Path) -> Result<()> {
    let mut conn = open(path)?;
    let tx = conn.transaction()?;
    tx.execute_batch(
        "DELETE FROM collection_items;
         DELETE FROM collections;
         DELETE FROM item_tags;
         DELETE FROM tags;
         DELETE FROM work_items;
         DELETE FROM works;
         DELETE FROM items;
         DELETE FROM authors;",
    )?;

    tx.execute(
        "INSERT INTO authors (id, source, external_id, name, avatar_url, updated_at)
         VALUES (?1, 'pixiv', '100', '作者', 'avatar.jpg', 1700000000)",
        params![SAMPLE_AUTHOR],
    )?;
    tx.execute(
        "INSERT INTO items
            (id, source, source_id, work_id, title, name, width, height, rating, star,
             favorite, mtime, posted_at, orig_ext, sharp, phash, group_name, decision,
             rules_version, revision, deleted, created_at, updated_at)
         VALUES (?1, 'pixiv', '200', ?2, '作品标题', 'a.jpg', 1200, 1600, 'G', 4, 1,
                 1700000000, 1690000000, 'jpg', 512, '0123456789abcdef', '原创', 'allow',
                 'rules-1', 1, 0, 1700000000, 1700000100)",
        params![SAMPLE_ITEM_A, SAMPLE_WORK],
    )?;
    tx.execute(
        "INSERT INTO items
            (id, source, source_id, work_id, title, name, width, height, rating, star,
             favorite, mtime, posted_at, orig_ext, sharp, phash, group_name, decision,
             rules_version, revision, deleted, created_at, updated_at)
         VALUES (?1, 'pixiv', '200', ?2, '作品标题', 'b.jpg', 800, 600, 'S', 2, 1,
                 1700000001, 1690000000, 'png', 0, 'fedcba9876543210', '原创', 'allow',
                 'rules-1', 1, 0, 1700000000, 1700000200)",
        params![SAMPLE_ITEM_B, SAMPLE_WORK],
    )?;
    tx.execute(
        "INSERT INTO works
            (id, source, external_id, title, author_id, page_count, posted_at, status, cover_item_id)
         VALUES (?1, 'pixiv', '200', '作品标题', ?2, 2, 1690000000, 'active', ?3)",
        params![SAMPLE_WORK, SAMPLE_AUTHOR, SAMPLE_ITEM_A],
    )?;
    tx.execute(
        "INSERT INTO work_items (work_id, item_id, page) VALUES (?1, ?2, 0), (?1, ?3, 1)",
        params![SAMPLE_WORK, SAMPLE_ITEM_A, SAMPLE_ITEM_B],
    )?;
    tx.execute_batch(
        "INSERT INTO tags (id, name, category) VALUES
            (1, '1girl', 'general'),
            (2, 'solo', 'general');",
    )?;
    tx.execute(
        "INSERT INTO item_tags (item_id, tag_id, score, origin) VALUES
            (?1, 1, 0.9, 'ingest'),
            (?1, 2, 0.5, 'ingest'),
            (?2, 1, 0.8, 'ingest')",
        params![SAMPLE_ITEM_A, SAMPLE_ITEM_B],
    )?;
    tx.execute(
        "INSERT INTO collections (id, name, kind) VALUES (1, '收藏', 'favorite')",
        [],
    )?;
    tx.execute(
        "INSERT INTO collection_items (collection_id, item_id) VALUES (1, ?1), (1, ?2)",
        params![SAMPLE_ITEM_A, SAMPLE_ITEM_B],
    )?;
    rebuild_search_index(&tx)?;
    tx.commit()?;
    Ok(())
}

/// 标签 facets 的单行形状，与静态契约 `index.facets.tags` 对齐。
#[derive(Clone, Debug, Serialize)]
pub struct FacetTag {
    pub name: String,
    pub category: String,
    pub count: u32,
    /// 该标签下一张代表图的 id，供总览页封面使用。
    pub cover: Option<String>,
}

/// `/api/index` 的分组 facet（带计数与封面）。
#[derive(Clone, Debug, Serialize)]
pub struct FacetGroup {
    pub name: String,
    pub count: u32,
    pub cover: Option<String>,
}

/// `/api/index` 的 facets。
#[derive(Clone, Debug, Serialize)]
pub struct IndexFacets {
    pub tags: Vec<FacetTag>,
    pub groups: Vec<FacetGroup>,
}

/// 分级计数；字段名与 `WallItemDto.rt` 对齐。
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct RatingCounts {
    pub g: u32,
    pub s: u32,
    pub q: u32,
    pub e: u32,
}

/// `/api/index` 的各类计数。
#[derive(Clone, Copy, Debug, Serialize)]
pub struct IndexCounts {
    pub items: u32,
    pub works: u32,
    pub authors: u32,
    pub collections: u32,
    pub favorite: u32,
    pub ratings: RatingCounts,
}

const ITEM_COLUMNS: &str = "i.id, i.width, i.height, i.name, i.rating, i.star, \
     i.favorite, i.mtime, i.group_name, i.phash, i.orig_ext, i.sharp, i.work_id, \
     w.page_count, wi.page, a.name, a.id, i.deleted, i.title";

const ITEM_FROM: &str = "FROM items i \
     LEFT JOIN works w ON w.id = i.work_id \
     LEFT JOIN work_items wi ON wi.item_id = i.id \
     LEFT JOIN authors a ON a.id = w.author_id";

const WORK_COLUMNS: &str =
    "w.id, w.source, w.external_id, w.title, w.author_id, w.page_count, w.posted_at, w.status";

pub fn count_works(conn: &Connection) -> Result<u32> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM works WHERE status = 'active'",
        [],
        |row| row.get(0),
    )?;
    Ok(as_u32(count))
}

pub fn count_authors(conn: &Connection) -> Result<u32> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM authors", [], |row| row.get(0))?;
    Ok(as_u32(count))
}

pub fn count_collections(conn: &Connection) -> Result<u32> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM collections", [], |row| row.get(0))?;
    Ok(as_u32(count))
}

pub fn get_item(conn: &Connection, id: &str) -> Result<Option<WallItemDto>> {
    let sql = format!(
        "SELECT {ITEM_COLUMNS} {ITEM_FROM}
          WHERE i.id = ?1 AND i.deleted = 0 AND i.decision = 'allow'"
    );
    let mut stmt = conn.prepare(&sql)?;
    let item = stmt.query_row([id], map_item_row).optional()?;
    let Some(mut item) = item else {
        return Ok(None);
    };
    let mut map = load_tag_map(conn, std::slice::from_ref(&item.id))?;
    item.tags = map.remove(&item.id).unwrap_or_default();
    Ok(Some(item))
}

/// 媒体 ETag 用；只取未删除条目的 `updated_at`。
pub fn item_updated_at(conn: &Connection, id: &str) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT updated_at FROM items WHERE id = ?1 AND deleted = 0 AND decision = 'allow'",
        [id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_works(conn: &Connection, limit: u32, offset: u32) -> Result<Vec<WorkDto>> {
    let sql = format!(
        "SELECT {WORK_COLUMNS} FROM works w \
         WHERE w.status = 'active' \
         ORDER BY w.posted_at DESC, w.id DESC LIMIT ?1 OFFSET ?2"
    );
    collect_works(conn, &sql, params![i64::from(limit), i64::from(offset)])
}

pub fn list_author_works(
    conn: &Connection,
    author_id: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<WorkDto>> {
    let sql = format!(
        "SELECT {WORK_COLUMNS} FROM works w
         WHERE w.author_id = ?1 AND w.status = 'active' \
         ORDER BY w.posted_at DESC, w.id DESC LIMIT ?2 OFFSET ?3"
    );
    collect_works(
        conn,
        &sql,
        params![author_id, i64::from(limit), i64::from(offset)],
    )
}

pub fn count_author_works(conn: &Connection, author_id: &str) -> Result<u32> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM works WHERE author_id = ?1 AND status = 'active'",
        [author_id],
        |row| row.get(0),
    )?;
    Ok(as_u32(count))
}

pub fn get_work(conn: &Connection, id: &str) -> Result<Option<WorkDto>> {
    let sql = format!(
        "SELECT {WORK_COLUMNS} FROM works w
         WHERE w.id = ?1 AND w.status = 'active'"
    );
    let mut stmt = conn.prepare(&sql)?;
    let work = stmt
        .query_row([id], |row| map_work_row(row, None))
        .optional()?;
    let Some(mut work) = work else {
        return Ok(None);
    };
    let author_id = work_author_id(conn, id)?;
    if let Some(author_id) = author_id {
        work.author = get_author(conn, &author_id)?;
    }
    work.tags = work_tags(conn, id)?;
    work.pages = work_pages(conn, id)?;
    Ok(Some(work))
}

pub fn list_authors(conn: &Connection, limit: u32, offset: u32) -> Result<Vec<AuthorDto>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.name, a.avatar_url,
                (SELECT COUNT(*) FROM works w
                  WHERE w.author_id = a.id AND w.status = 'active'),
                (SELECT COUNT(*) FROM items i JOIN works w ON w.id = i.work_id
                  WHERE w.author_id = a.id AND i.deleted = 0
                    AND i.decision = 'allow' AND w.status = 'active')
         FROM authors a ORDER BY a.name ASC, a.id ASC LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt.query_map(params![i64::from(limit), i64::from(offset)], map_author_row)?;
    let mut authors = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    for author in &mut authors {
        author.covers = author_covers(conn, &author.id, 4)?;
    }
    Ok(authors)
}

pub fn get_author(conn: &Connection, id: &str) -> Result<Option<AuthorDto>> {
    let author = conn
        .query_row(
            "SELECT a.id, a.name, a.avatar_url,
                    (SELECT COUNT(*) FROM works w
                      WHERE w.author_id = a.id AND w.status = 'active'),
                    (SELECT COUNT(*) FROM items i JOIN works w ON w.id = i.work_id
                      WHERE w.author_id = a.id AND i.deleted = 0
                        AND i.decision = 'allow' AND w.status = 'active')
             FROM authors a WHERE a.id = ?1",
            [id],
            map_author_row,
        )
        .optional()?;
    let Some(mut author) = author else {
        return Ok(None);
    };
    author.covers = author_covers(conn, id, 4)?;
    Ok(Some(author))
}

pub fn list_collections(conn: &Connection, limit: u32, offset: u32) -> Result<Vec<CollectionDto>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.name, c.kind,
                (SELECT COUNT(*) FROM collection_items ci JOIN items i ON i.id = ci.item_id
                  WHERE ci.collection_id = c.id AND i.deleted = 0
                    AND i.decision = 'allow'),
                (SELECT COUNT(DISTINCT i.work_id)
                   FROM collection_items ci JOIN items i ON i.id = ci.item_id
                  WHERE ci.collection_id = c.id AND i.deleted = 0
                    AND i.decision = 'allow'
                    AND i.work_id IS NOT NULL)
         FROM collections c ORDER BY c.id ASC LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt.query_map(
        params![i64::from(limit), i64::from(offset)],
        map_collection_row,
    )?;
    let mut collections = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    for collection in &mut collections {
        collection.covers = collection_covers(conn, collection.id, 4)?;
    }
    Ok(collections)
}

pub fn collection_exists(conn: &Connection, id: i64) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM collections WHERE id = ?1",
        [id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub fn count_collection_items(conn: &Connection, id: i64) -> Result<u32> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM collection_items ci JOIN items i ON i.id = ci.item_id
         WHERE ci.collection_id = ?1 AND i.deleted = 0 AND i.decision = 'allow'",
        [id],
        |row| row.get(0),
    )?;
    Ok(as_u32(count))
}

pub fn count_collection_works(conn: &Connection, id: i64) -> Result<u32> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT i.work_id) FROM collection_items ci
          JOIN items i ON i.id = ci.item_id
         WHERE ci.collection_id = ?1 AND i.deleted = 0 AND i.decision = 'allow'
           AND i.work_id IS NOT NULL",
        [id],
        |row| row.get(0),
    )?;
    Ok(as_u32(count))
}

pub fn list_collection_items(
    conn: &Connection,
    id: i64,
    limit: u32,
    offset: u32,
) -> Result<Vec<WallItemDto>> {
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM collection_items ci
          JOIN items i ON i.id = ci.item_id
          LEFT JOIN works w ON w.id = i.work_id
          LEFT JOIN work_items wi ON wi.item_id = i.id
          LEFT JOIN authors a ON a.id = w.author_id
         WHERE ci.collection_id = ?1 AND i.deleted = 0 AND i.decision = 'allow'
         ORDER BY i.updated_at DESC, i.id DESC LIMIT ?2 OFFSET ?3"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![id, i64::from(limit), i64::from(offset)],
        map_item_row,
    )?;
    let mut items = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    attach_tags(conn, &mut items)?;
    Ok(items)
}

pub fn list_collection_works(
    conn: &Connection,
    id: i64,
    limit: u32,
    offset: u32,
) -> Result<Vec<WorkDto>> {
    let sql = format!(
        "SELECT DISTINCT {WORK_COLUMNS} FROM collection_items ci
          JOIN items i ON i.id = ci.item_id
          JOIN works w ON w.id = i.work_id
         WHERE ci.collection_id = ?1 AND i.deleted = 0 AND i.decision = 'allow'
           AND w.status = 'active'
         ORDER BY w.posted_at DESC, w.id DESC LIMIT ?2 OFFSET ?3"
    );
    collect_works(conn, &sql, params![id, i64::from(limit), i64::from(offset)])
}

pub fn index_facets_filtered(conn: &Connection, filter: &ItemFilter) -> Result<IndexFacets> {
    let mut parts = filter_parts(filter);
    parts.inline_candidates();
    let where_sql = parts.where_sql();

    let tags_sql = format!(
        "SELECT t.name, t.category, COUNT(DISTINCT it.item_id), MIN(it.item_id)
           FROM item_tags it
           JOIN tags t ON t.id = it.tag_id
           JOIN items i ON i.id = it.item_id
          WHERE {where_sql}
          GROUP BY t.id
          ORDER BY COUNT(DISTINCT it.item_id) DESC, t.name ASC LIMIT 2000"
    );
    let mut stmt = conn.prepare(&tags_sql)?;
    let bindings = parts.bindings();
    let rows = stmt.query_map(bindings.as_slice(), |row| {
        Ok(FacetTag {
            name: row.get(0)?,
            category: row.get(1)?,
            count: as_u32(row.get(2)?),
            cover: row.get(3)?,
        })
    })?;
    let tags = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    let groups_sql = format!(
        "SELECT i.group_name, COUNT(*), MIN(i.id) FROM items i
          WHERE {where_sql} AND i.group_name <> ''
          GROUP BY i.group_name
          ORDER BY COUNT(*) DESC, i.group_name ASC LIMIT 200"
    );
    let mut stmt = conn.prepare(&groups_sql)?;
    let bindings = parts.bindings();
    let rows = stmt.query_map(bindings.as_slice(), |row| {
        Ok(FacetGroup {
            name: row.get(0)?,
            count: as_u32(row.get(1)?),
            cover: row.get(2)?,
        })
    })?;
    let groups = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(IndexFacets { tags, groups })
}

/// W2 的条目分页结果；`next` 已编码成不透明游标。
pub struct ItemPage {
    pub items: Vec<WallItemDto>,
    pub next: Option<Cursor>,
}

pub fn search_items(
    conn: &Connection,
    filter: &ItemFilter,
    sort: &ItemSort,
    cursor: Option<&Cursor>,
    limit: u32,
    offset: u32,
) -> Result<ItemPage> {
    let mut parts = filter_parts(filter);
    parts.inline_candidates();
    let sort_expr = sort_expression(&mut parts, sort);
    if let Some(cursor) = cursor {
        cursor_clause(&mut parts, sort, &sort_expr, cursor);
    }
    let limit_phrase = parts.param(SqlValue::Integer(i64::from(limit) + 1));
    let offset_sql = if cursor.is_none() && offset > 0 {
        format!(" OFFSET {offset}")
    } else {
        String::new()
    };
    let direction = if sort.ascending { "ASC" } else { "DESC" };
    let sql = format!(
        "SELECT {ITEM_COLUMNS}, {sort_expr} AS sort_key {ITEM_FROM}
          WHERE {}
          ORDER BY {sort_expr} {direction}, i.id {direction}
          LIMIT {limit_phrase}{offset_sql}",
        parts.where_sql()
    );

    let mut stmt = conn.prepare(&sql)?;
    let bindings = parts.bindings();
    let rows = stmt.query_map(bindings.as_slice(), |row| {
        let item = map_item_row(row)?;
        let key = match row.get::<_, SqlValue>(19)? {
            SqlValue::Integer(value) => SortKey::Int(value),
            SqlValue::Real(value) => SortKey::Int(value as i64),
            SqlValue::Text(value) => SortKey::Text(value),
            SqlValue::Null | SqlValue::Blob(_) => SortKey::Text(String::new()),
        };
        Ok((item, key))
    })?;
    let mut rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    let has_more = rows.len() > limit as usize;
    if has_more {
        rows.truncate(limit as usize);
    }
    let next = if has_more {
        rows.last().map(|(item, key)| Cursor {
            sort: sort.kind,
            ascending: sort.ascending,
            key: key.clone(),
            id: item.id.clone(),
            fingerprint: filter.fingerprint(sort.seed),
        })
    } else {
        None
    };

    let mut items = rows.into_iter().map(|(item, _)| item).collect::<Vec<_>>();
    attach_tags(conn, &mut items)?;
    Ok(ItemPage { items, next })
}

/// 详情页前后条目；`index`/`total` 是目标条目在当前筛选集合中的位置。
#[derive(Clone, Debug, Default, Serialize)]
pub struct ItemNeighbors {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev: Option<WallItemDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<WallItemDto>,
    pub index: u32,
    pub total: u32,
}

/// 按当前筛选/排序取目标条目的前一条与后一条。
///
/// 目标条目即使被筛选排除，也按它的排序键定位，保证深链仍有前后邻居。
pub fn item_neighbors(
    conn: &Connection,
    id: &str,
    filter: &ItemFilter,
    sort: &ItemSort,
) -> Result<Option<ItemNeighbors>> {
    let Some(key) = item_sort_key(conn, id, sort)? else {
        return Ok(None);
    };
    let prev = neighbor(conn, filter, sort, &key, id, false)?;
    let next = neighbor(conn, filter, sort, &key, id, true)?;

    let mut total_parts = filter_parts(filter);
    total_parts.inline_candidates();
    let total = total_parts.query_count(
        conn,
        &format!(
            "SELECT COUNT(*) FROM items i WHERE {}",
            total_parts.where_sql()
        ),
    )?;

    let mut before_parts = filter_parts(filter);
    before_parts.inline_candidates();
    let before_expr = sort_expression(&mut before_parts, sort);
    keyset_clause(&mut before_parts, sort, &before_expr, &key, id, false);
    let before = before_parts.query_count(
        conn,
        &format!(
            "SELECT COUNT(*) FROM items i WHERE {}",
            before_parts.where_sql()
        ),
    )?;

    Ok(Some(ItemNeighbors {
        prev,
        next,
        index: as_u32(before),
        total: as_u32(total),
    }))
}

fn item_sort_key(conn: &Connection, id: &str, sort: &ItemSort) -> Result<Option<SortKey>> {
    let mut parts = WhereParts::new();
    let sort_expr = sort_expression(&mut parts, sort);
    let id_param = parts.param(SqlValue::Text(id.to_string()));
    let sql = format!(
        "SELECT {sort_expr} FROM items i WHERE {} AND i.id = {id_param}",
        parts.where_sql()
    );
    let mut statement = conn.prepare(&sql)?;
    let bindings = parts.bindings();
    let value = statement
        .query_row(bindings.as_slice(), |row| row.get::<_, SqlValue>(0))
        .optional()?;
    Ok(value.map(sort_key_from_value))
}

fn neighbor(
    conn: &Connection,
    filter: &ItemFilter,
    sort: &ItemSort,
    key: &SortKey,
    id: &str,
    forward: bool,
) -> Result<Option<WallItemDto>> {
    let mut parts = filter_parts(filter);
    parts.inline_candidates();
    let sort_expr = sort_expression(&mut parts, sort);
    keyset_clause(&mut parts, sort, &sort_expr, key, id, forward);
    let direction = if sort.ascending { "ASC" } else { "DESC" };
    let order = if forward {
        direction
    } else {
        if sort.ascending { "DESC" } else { "ASC" }
    };
    let sql = format!(
        "SELECT {ITEM_COLUMNS} {ITEM_FROM}
          WHERE {}
          ORDER BY {sort_expr} {order}, i.id {order}
          LIMIT 1",
        parts.where_sql()
    );
    let mut statement = conn.prepare(&sql)?;
    let bindings = parts.bindings();
    let item = statement
        .query_row(bindings.as_slice(), map_item_row)
        .optional()?;
    let Some(mut item) = item else {
        return Ok(None);
    };
    attach_tags(conn, std::slice::from_mut(&mut item))?;
    Ok(Some(item))
}

fn keyset_clause(
    parts: &mut WhereParts,
    sort: &ItemSort,
    sort_expr: &str,
    key: &SortKey,
    id: &str,
    forward: bool,
) {
    let key = match key {
        SortKey::Int(value) => parts.param(SqlValue::Integer(*value)),
        SortKey::Text(value) => parts.param(SqlValue::Text(value.clone())),
    };
    let id = parts.param(SqlValue::Text(id.to_string()));
    let operator = if forward == sort.ascending { ">" } else { "<" };
    parts.and(format!(
        "({sort_expr} {operator} {key} OR ({sort_expr} = {key} AND i.id {operator} {id}))"
    ));
}

fn sort_key_from_value(value: SqlValue) -> SortKey {
    match value {
        SqlValue::Integer(value) => SortKey::Int(value),
        SqlValue::Real(value) => SortKey::Int(value as i64),
        SqlValue::Text(value) => SortKey::Text(value),
        SqlValue::Null | SqlValue::Blob(_) => SortKey::Text(String::new()),
    }
}

pub fn index_counts(conn: &Connection, filter: &ItemFilter) -> Result<IndexCounts> {
    let mut parts = filter_parts(filter);
    parts.inline_candidates();
    let where_sql = parts.where_sql();

    let stats_sql = format!(
        "SELECT COUNT(*),
                COALESCE(SUM(i.favorite), 0),
                COALESCE(SUM(CASE i.rating WHEN 'G' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE i.rating WHEN 'S' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE i.rating WHEN 'Q' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE i.rating WHEN 'E' THEN 1 ELSE 0 END), 0)
           FROM items i WHERE {where_sql}"
    );
    let mut stmt = conn.prepare(&stats_sql)?;
    let bindings = parts.bindings();
    let (items, favorite, g, s, q, e) = stmt.query_row(bindings.as_slice(), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
        ))
    })?;
    let works = parts.query_count(
        conn,
        &format!(
            "SELECT COUNT(DISTINCT i.work_id) FROM items i
              WHERE {where_sql} AND i.work_id IS NOT NULL"
        ),
    )?;
    let authors = parts.query_count(
        conn,
        &format!(
            "SELECT COUNT(DISTINCT w.author_id) FROM items i
               JOIN works w ON w.id = i.work_id
              WHERE {where_sql} AND w.author_id IS NOT NULL"
        ),
    )?;
    let collections = if filter.is_empty() {
        count_collections(conn)?
    } else {
        let count = parts.query_count(
            conn,
            &format!(
                "SELECT COUNT(DISTINCT ci.collection_id) FROM collection_items ci
                   JOIN items i ON i.id = ci.item_id
                  WHERE {where_sql}"
            ),
        )?;
        as_u32(count)
    };

    Ok(IndexCounts {
        items: as_u32(items),
        works: as_u32(works),
        authors: as_u32(authors),
        collections,
        favorite: as_u32(favorite),
        ratings: RatingCounts {
            g: as_u32(g),
            s: as_u32(s),
            q: as_u32(q),
            e: as_u32(e),
        },
    })
}

struct WhereParts {
    candidates: Vec<String>,
    clauses: Vec<String>,
    params: Vec<(String, SqlValue)>,
    next_param: usize,
}

impl WhereParts {
    fn new() -> Self {
        Self {
            candidates: Vec::new(),
            clauses: vec![
                "i.deleted = 0".to_string(),
                "i.decision = 'allow'".to_string(),
            ],
            params: Vec::new(),
            next_param: 0,
        }
    }

    fn and(&mut self, clause: String) {
        self.clauses.push(clause);
    }

    fn candidate(&mut self, select: String) {
        self.candidates.push(select);
    }

    fn param(&mut self, value: SqlValue) -> String {
        let name = format!("p{}", self.next_param);
        self.next_param += 1;
        self.params.push((format!(":{name}"), value));
        format!(":{name}")
    }

    fn where_sql(&self) -> String {
        self.clauses.join(" AND ")
    }

    fn candidate_sql(&self) -> Option<String> {
        if self.candidates.is_empty() {
            None
        } else {
            Some(self.candidates.join(" INTERSECT "))
        }
    }

    fn inline_candidates(&mut self) {
        if let Some(candidates) = self.candidate_sql() {
            self.clauses.push(format!("i.id IN ({candidates})"));
            self.candidates.clear();
        }
    }

    fn bindings(&self) -> Vec<(&str, &dyn rusqlite::ToSql)> {
        self.params
            .iter()
            .map(|(name, value)| (name.as_str(), value as &dyn rusqlite::ToSql))
            .collect()
    }

    fn query_count(&self, conn: &Connection, sql: &str) -> Result<i64> {
        let mut stmt = conn.prepare(sql)?;
        let bindings = self.bindings();
        Ok(stmt.query_row(bindings.as_slice(), |row| row.get(0))?)
    }
}

fn filter_parts(filter: &ItemFilter) -> WhereParts {
    let mut parts = WhereParts::new();
    if let Some(tag) = &filter.tag {
        let placeholder = parts.param(SqlValue::Text(tag.clone()));
        parts.and(format!(
            "EXISTS (SELECT 1 FROM item_tags itf
                       JOIN tags tf ON tf.id = itf.tag_id
                      WHERE tf.name = {placeholder} AND itf.item_id = i.id)"
        ));
    }
    if let Some(group) = &filter.group {
        let placeholder = parts.param(SqlValue::Text(group.clone()));
        parts.and(format!("i.group_name = {placeholder}"));
    }
    if let Some(rating) = &filter.rating {
        let placeholder = parts.param(SqlValue::Text(rating.to_ascii_uppercase()));
        parts.and(format!("i.rating = {placeholder}"));
    }
    if let Some(fav) = filter.fav {
        let placeholder = parts.param(SqlValue::Integer(if fav { 1 } else { 0 }));
        parts.and(format!("i.favorite = {placeholder}"));
    }
    if let Some(query) = &filter.q {
        for term in parse_search(query) {
            search_clause(&mut parts, &term);
        }
    }
    parts
}

fn search_clause(parts: &mut WhereParts, term: &SearchTerm) {
    // trigram FTS 需要至少 3 个字符；短词仍用 LIKE，保持子串语义。
    if term.value.chars().count() >= 3 {
        let columns = match term.field {
            SearchField::Any => "{name tags group_name rating}",
            SearchField::Name => "name",
            SearchField::Tag => "tags",
            SearchField::Group => "group_name",
            SearchField::Rating => "rating",
        };
        let escaped = term.value.replace('"', "\"\"");
        let query = format!("{columns} : \"{escaped}\"");
        let placeholder = parts.param(SqlValue::Text(query));
        if term.exclude {
            parts.and(format!(
                "i.id NOT IN (SELECT item_id FROM item_search
                               WHERE item_search MATCH {placeholder})"
            ));
        } else {
            parts.candidate(format!(
                "SELECT item_id FROM item_search WHERE item_search MATCH {placeholder}"
            ));
        }
        return;
    }

    let pattern = format!("%{}%", escape_like(&term.value));
    let placeholder = parts.param(SqlValue::Text(pattern));
    let any_tag = format!(
        "i.id IN (SELECT its.item_id FROM item_tags its
                   JOIN tags ts ON ts.id = its.tag_id
                  WHERE replace(ts.name, '_', ' ') LIKE {placeholder} ESCAPE '\\')"
    );
    let clause = match term.field {
        SearchField::Any => format!(
            "(replace(i.name, '_', ' ') LIKE {placeholder} ESCAPE '\\'
              OR replace(i.group_name, '_', ' ') LIKE {placeholder} ESCAPE '\\'
              OR i.rating LIKE {placeholder} ESCAPE '\\'
              OR {any_tag})"
        ),
        SearchField::Name => {
            format!("replace(i.name, '_', ' ') LIKE {placeholder} ESCAPE '\\'")
        }
        SearchField::Tag => any_tag,
        SearchField::Group => {
            format!("replace(i.group_name, '_', ' ') LIKE {placeholder} ESCAPE '\\'")
        }
        SearchField::Rating => format!(
            "(i.rating LIKE {placeholder} ESCAPE '\\'
              OR (CASE i.rating
                    WHEN 'G' THEN 'G 一般'
                    WHEN 'S' THEN 'S 敏感'
                    WHEN 'Q' THEN 'Q 存疑'
                    WHEN 'E' THEN 'E 限制'
                    ELSE i.rating END) LIKE {placeholder} ESCAPE '\\')"
        ),
    };
    parts.and(if term.exclude {
        format!("NOT ({clause})")
    } else {
        clause
    });
}

fn escape_like(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '%' => out.push_str("\\%"),
            '_' => out.push_str("\\_"),
            _ => out.push(ch),
        }
    }
    out
}

fn sort_expression(parts: &mut WhereParts, sort: &ItemSort) -> String {
    match sort.kind {
        SortKind::Updated => "i.updated_at".to_string(),
        SortKind::Posted => "COALESCE(i.posted_at, 0)".to_string(),
        SortKind::Name => "i.name COLLATE NOCASE".to_string(),
        SortKind::Star => "i.star".to_string(),
        SortKind::Favorite => "i.favorite".to_string(),
        SortKind::Rating => "CASE rating WHEN 'G' THEN 0 WHEN 'S' THEN 1 WHEN 'Q' THEN 2 \
             WHEN 'E' THEN 3 ELSE 0 END"
            .to_string(),
        // 同一 seed 得到同一顺序；换 seed 就是另一套稳定洗牌。
        SortKind::Random => {
            let seed = parts.param(SqlValue::Integer(sort.seed));
            format!("kanalib_order_hash(i.id, {seed})")
        }
    }
}

fn cursor_clause(parts: &mut WhereParts, sort: &ItemSort, sort_expr: &str, cursor: &Cursor) {
    let key = match &cursor.key {
        SortKey::Int(value) => parts.param(SqlValue::Integer(*value)),
        SortKey::Text(value) => parts.param(SqlValue::Text(value.clone())),
    };
    let id = parts.param(SqlValue::Text(cursor.id.clone()));
    let operator = if sort.ascending { ">" } else { "<" };
    parts.and(format!(
        "({sort_expr} {operator} {key} OR ({sort_expr} = {key} AND i.id {operator} {id}))"
    ));
}

/// 入库批次的施加结果。
#[derive(Clone, Copy, Debug, Default)]
pub struct IngestApply {
    pub duplicate: bool,
    pub inserted: u32,
    pub updated: u32,
    pub stale: u32,
}

impl IngestApply {
    pub fn accepted(self) -> u32 {
        self.inserted + self.updated
    }
}

/// 删除事件的结果；只置 `deleted = 1`，不物理删除。
#[derive(Clone, Copy, Debug, Default)]
pub struct DeleteApply {
    pub duplicate: bool,
    pub deleted: u32,
    pub stale: u32,
    pub missing: u32,
}

/// 施加 allow 批次。重复 `batch_id` 直接返回，不重复写。
pub fn apply_ingest_batch(
    conn: &mut Connection,
    batch_id: &str,
    batch: &IngestBatch,
    now: i64,
) -> Result<IngestApply> {
    let tx = conn.transaction()?;
    if !record_batch(
        &tx,
        batch_id,
        &batch.source,
        batch.items.len(),
        batch.rules_version.as_deref(),
        now,
    )? {
        return Ok(IngestApply {
            duplicate: true,
            ..IngestApply::default()
        });
    }

    let mut apply = IngestApply::default();
    let mut touched = Vec::new();
    let mut works = std::collections::BTreeSet::new();
    for entry in &batch.items {
        match upsert_ingest_item(&tx, batch, entry, now)? {
            ItemWrite::Inserted => {
                apply.inserted += 1;
                touched.push(entry.item.id.clone());
            }
            ItemWrite::Updated => {
                apply.updated += 1;
                touched.push(entry.item.id.clone());
            }
            ItemWrite::Stale => apply.stale += 1,
        }
        if let Some(work_id) = &entry.item.work_id {
            works.insert(work_id.clone());
        }
    }
    for work_id in &works {
        refresh_work_cover(&tx, work_id)?;
    }
    refresh_search_index(&tx, &touched)?;
    tx.commit()?;
    Ok(apply)
}

/// 施加删除事件；`revision` 低于网页当前值的条目视为过期，不覆盖。
pub fn apply_ingest_delete(
    conn: &mut Connection,
    batch_id: &str,
    delete: &IngestDelete,
    now: i64,
) -> Result<DeleteApply> {
    let tx = conn.transaction()?;
    if !record_batch(
        &tx,
        batch_id,
        &delete.source,
        delete.items.len(),
        delete.rules_version.as_deref(),
        now,
    )? {
        return Ok(DeleteApply {
            duplicate: true,
            ..DeleteApply::default()
        });
    }

    let mut apply = DeleteApply::default();
    let mut touched = Vec::new();
    let mut works = std::collections::BTreeSet::new();
    for entry in &delete.items {
        let current: Option<(i64, Option<String>)> = tx
            .query_row(
                "SELECT revision, work_id FROM items WHERE id = ?1",
                [&entry.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match current {
            None => apply.missing += 1,
            Some((revision, _)) if revision > entry.revision => apply.stale += 1,
            Some((_, work_id)) => {
                tx.execute(
                    "UPDATE items SET deleted = 1, revision = ?2, updated_at = ?3
                      WHERE id = ?1",
                    params![&entry.id, entry.revision, now],
                )?;
                if let Some(work_id) = work_id {
                    works.insert(work_id);
                }
                apply.deleted += 1;
                touched.push(entry.id.clone());
            }
        }
    }
    for work_id in &works {
        refresh_work_cover(&tx, work_id)?;
    }
    refresh_search_index(&tx, &touched)?;
    tx.commit()?;
    Ok(apply)
}

/// 封面必须指向仍可见的条目；以页码最小的活动条目兜底，没有条目时置空。
fn refresh_work_cover(tx: &Transaction<'_>, work_id: &str) -> Result<()> {
    tx.execute(
        "UPDATE works
            SET cover_item_id = (
                SELECT i.id
                  FROM items i
                  LEFT JOIN work_items wi ON wi.item_id = i.id
                 WHERE i.work_id = works.id AND i.deleted = 0
                   AND i.decision = 'allow'
                 ORDER BY COALESCE(wi.page, 2147483647), i.id
                 LIMIT 1)
          WHERE id = ?1",
        [work_id],
    )?;
    Ok(())
}

enum ItemWrite {
    Inserted,
    Updated,
    Stale,
}

fn record_batch(
    tx: &Transaction<'_>,
    batch_id: &str,
    source: &str,
    item_count: usize,
    rules_version: Option<&str>,
    now: i64,
) -> Result<bool> {
    let item_count = i64::try_from(item_count).unwrap_or(i64::MAX);
    let changed = tx.execute(
        "INSERT OR IGNORE INTO ingest_batches
            (id, source, item_count, rules_version, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![batch_id, source, item_count, rules_version, now],
    )?;
    Ok(changed == 1)
}

fn upsert_ingest_item(
    tx: &Transaction<'_>,
    batch: &IngestBatch,
    entry: &IngestBatchItem,
    now: i64,
) -> Result<ItemWrite> {
    let item = &entry.item;
    if !valid_item_id(&item.id) {
        anyhow::bail!("invalid item id {}", item.id);
    }
    let previous: Option<i64> = tx
        .query_row(
            "SELECT revision FROM items WHERE id = ?1",
            [&item.id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(revision) = previous
        && revision > entry.revision
    {
        return Ok(ItemWrite::Stale);
    }

    if let (Some(author_id), Some(author_name)) = (&item.author_id, &item.author) {
        tx.execute(
            "INSERT INTO authors (id, source, name, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                name = CASE WHEN excluded.name <> '' THEN excluded.name ELSE authors.name END,
                updated_at = excluded.updated_at",
            params![author_id, &batch.source, author_name.trim(), now],
        )?;
    }

    if let Some(work_id) = &item.work_id {
        tx.execute(
            "INSERT INTO works
                (id, source, title, author_id, page_count, status, cover_item_id)
             VALUES (?1, ?2, ?3, ?4, COALESCE(?5, 1), 'active', NULL)
             ON CONFLICT(id) DO UPDATE SET
                title = CASE WHEN excluded.title <> '' THEN excluded.title ELSE works.title END,
                author_id = COALESCE(excluded.author_id, works.author_id),
                page_count = COALESCE(?5, works.page_count)",
            params![
                work_id,
                &batch.source,
                item.title.as_deref().unwrap_or("").trim(),
                item.author_id.as_deref(),
                item.page_count.map(|count| i64::from(count.max(1))),
            ],
        )?;
    }

    tx.execute(
        "INSERT INTO items
            (id, source, source_id, work_id, title, name, width, height, rating, star,
             favorite, mtime, posted_at, orig_ext, sharp, phash, group_name, decision,
             rules_version, revision, deleted, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 ?11, ?12, NULL, ?13, ?14, ?15, ?16, 'allow',
                 ?17, ?18, 0, ?19, ?19)
         ON CONFLICT(id) DO UPDATE SET
            source = excluded.source,
            source_id = excluded.source_id,
            work_id = excluded.work_id,
            title = excluded.title,
            name = excluded.name,
            width = excluded.width,
            height = excluded.height,
            rating = excluded.rating,
            -- star/favorite 是网页侧用户状态；ingest 批次只负责来源元数据，
            -- 不能用 0/false 覆盖桌面端或用户之后的修改。
            star = items.star,
            favorite = items.favorite,
            mtime = excluded.mtime,
            orig_ext = excluded.orig_ext,
            sharp = excluded.sharp,
            phash = excluded.phash,
            group_name = excluded.group_name,
            decision = excluded.decision,
            rules_version = excluded.rules_version,
            revision = excluded.revision,
            deleted = 0,
            updated_at = excluded.updated_at",
        params![
            &item.id,
            &batch.source,
            entry.source_id.as_deref(),
            item.work_id.as_deref(),
            item.title.as_deref().unwrap_or("").trim(),
            item.name.trim(),
            i64::from(item.width),
            i64::from(item.height),
            normalize_rating(&item.rating),
            i64::from(item.star.min(5)),
            if item.favorite { 1 } else { 0 },
            i64::try_from(item.mtime).unwrap_or(i64::MAX),
            normalize_extension(item.orig.as_deref()),
            item.sharp.map(i64::from),
            item.phash
                .as_deref()
                .map(|value| value.trim().to_ascii_lowercase()),
            item.group.as_deref().unwrap_or("").trim(),
            batch.rules_version.as_deref(),
            entry.revision.max(1),
            now,
        ],
    )?;

    tx.execute(
        "DELETE FROM item_tags WHERE item_id = ?1 AND origin = 'ingest'",
        [&item.id],
    )?;
    for tag in &item.tags {
        let name = tag.name.trim();
        if name.is_empty() || name.len() > 256 {
            continue;
        }
        let tag_id = upsert_tag(tx, name, tag.category.trim())?;
        tx.execute(
            "INSERT INTO item_tags (item_id, tag_id, score, origin)
             VALUES (?1, ?2, ?3, 'ingest')
             ON CONFLICT(item_id, tag_id, origin) DO UPDATE SET score = excluded.score",
            params![&item.id, tag_id, normalize_score(tag.score),],
        )?;
    }

    if let Some(work_id) = &item.work_id {
        if let Some(page) = item.page {
            tx.execute(
                "INSERT OR REPLACE INTO work_items (work_id, item_id, page)
                 VALUES (?1, ?2, ?3)",
                params![work_id, &item.id, i64::from(page)],
            )?;
        }
    }

    Ok(match previous {
        Some(_) => ItemWrite::Updated,
        None => ItemWrite::Inserted,
    })
}

fn upsert_tag(tx: &Transaction<'_>, name: &str, category: &str) -> Result<i64> {
    tx.execute(
        "INSERT INTO tags (name, category) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET
            category = CASE WHEN tags.category = '' THEN excluded.category ELSE tags.category END",
        params![name, category],
    )?;
    let id = tx.query_row("SELECT id FROM tags WHERE name = ?1", [name], |row| {
        row.get(0)
    })?;
    Ok(id)
}

fn normalize_extension(value: Option<&str>) -> Option<String> {
    let value = value?.trim().trim_start_matches('.').to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 8
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(value)
}

/// 桌面端发布的本地条目元数据；原图与缩略图就绪前保持 `review`。
#[derive(Clone, Debug)]
pub struct LocalItem {
    pub id: String,
    pub source: String,
    pub name: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub rating: String,
    pub star: u8,
    pub favorite: bool,
    pub mtime: i64,
    pub posted_at: Option<i64>,
    pub orig_ext: String,
    pub group_name: String,
    pub work_id: Option<String>,
    pub work_title: String,
    pub page: Option<u32>,
    pub page_count: Option<u32>,
    pub tags: Vec<IngestTag>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalWrite {
    Created,
    Updated,
    /// 已存在但来源不是本地发布（例如 box 入库的同 id 条目），拒绝覆盖。
    Exists,
}

/// 写入/更新本地条目；只改元数据，不触碰 `decision`、`star`、`favorite`。
pub fn upsert_local_item(conn: &mut Connection, item: &LocalItem, now: i64) -> Result<LocalWrite> {
    if !valid_item_id(&item.id) {
        anyhow::bail!("invalid item id {}", item.id);
    }
    let tx = conn.transaction()?;
    let previous: Option<String> = tx
        .query_row(
            "SELECT origin FROM items WHERE id = ?1",
            [&item.id],
            |row| row.get(0),
        )
        .optional()?;
    if previous.as_deref().is_some_and(|origin| origin != "local") {
        return Ok(LocalWrite::Exists);
    }

    if let Some(work_id) = &item.work_id {
        tx.execute(
            "INSERT INTO works (id, source, title, page_count, status)
             VALUES (?1, ?2, ?3, COALESCE(?4, 1), 'draft')
             ON CONFLICT(id) DO UPDATE SET
                title = CASE WHEN excluded.title <> '' THEN excluded.title ELSE works.title END,
                page_count = COALESCE(?4, works.page_count)",
            params![
                work_id,
                &item.source,
                item.work_title.trim(),
                item.page_count.map(|count| i64::from(count.max(1))),
            ],
        )?;
    }

    tx.execute(
        "INSERT INTO items
            (id, source, work_id, title, name, width, height, rating, star, favorite,
             mtime, posted_at, orig_ext, group_name, decision, rules_version, revision,
             deleted, origin, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 ?11, ?12, ?13, ?14, 'review', 'local', 1, 0, 'local', ?15, ?15)
         ON CONFLICT(id) DO UPDATE SET
            source = excluded.source,
            work_id = excluded.work_id,
            title = excluded.title,
            name = excluded.name,
            width = excluded.width,
            height = excluded.height,
            rating = excluded.rating,
            mtime = excluded.mtime,
            posted_at = excluded.posted_at,
            orig_ext = excluded.orig_ext,
            group_name = excluded.group_name,
            deleted = 0,
            updated_at = excluded.updated_at",
        params![
            &item.id,
            &item.source,
            item.work_id.as_deref(),
            item.title.trim(),
            item.name.trim(),
            i64::from(item.width),
            i64::from(item.height),
            normalize_rating(&item.rating),
            i64::from(item.star.min(5)),
            if item.favorite { 1 } else { 0 },
            item.mtime,
            item.posted_at,
            &item.orig_ext,
            item.group_name.trim(),
            now,
        ],
    )?;

    tx.execute(
        "DELETE FROM item_tags WHERE item_id = ?1 AND origin = 'local'",
        [&item.id],
    )?;
    for tag in &item.tags {
        let name = tag.name.trim();
        if name.is_empty() || name.len() > 256 {
            continue;
        }
        let tag_id = upsert_tag(&tx, name, tag.category.trim())?;
        tx.execute(
            "INSERT INTO item_tags (item_id, tag_id, score, origin)
             VALUES (?1, ?2, ?3, 'local')
             ON CONFLICT(item_id, tag_id, origin) DO UPDATE SET score = excluded.score",
            params![&item.id, tag_id, normalize_score(tag.score)],
        )?;
    }

    if let (Some(work_id), Some(page)) = (&item.work_id, item.page) {
        tx.execute(
            "INSERT OR REPLACE INTO work_items (work_id, item_id, page) VALUES (?1, ?2, ?3)",
            params![work_id, &item.id, i64::from(page)],
        )?;
    }

    refresh_search_index(&tx, std::slice::from_ref(&item.id))?;
    tx.commit()?;
    Ok(match previous {
        Some(_) => LocalWrite::Updated,
        None => LocalWrite::Created,
    })
}

/// 原图与缩略图都写好后把本地条目放上墙；尺寸以实际解码结果为准。
pub fn finalize_local_item(
    conn: &mut Connection,
    id: &str,
    width: u32,
    height: u32,
    sharp: u32,
    now: i64,
) -> Result<bool> {
    let tx = conn.transaction()?;
    let work_id: Option<Option<String>> = tx
        .query_row(
            "SELECT work_id FROM items
              WHERE id = ?1 AND origin = 'local' AND deleted = 0",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(work_id) = work_id else {
        return Ok(false);
    };
    tx.execute(
        "UPDATE items
            SET decision = 'allow', width = ?2, height = ?3, sharp = ?4,
                revision = revision + 1, updated_at = ?5
          WHERE id = ?1",
        params![
            id,
            i64::from(width),
            i64::from(height),
            i64::from(sharp),
            now
        ],
    )?;
    if let Some(work_id) = work_id {
        tx.execute(
            "UPDATE works SET status = 'active' WHERE id = ?1 AND status = 'draft'",
            [&work_id],
        )?;
        refresh_work_cover(&tx, &work_id)?;
    }
    refresh_search_index(&tx, &[id.to_string()])?;
    tx.commit()?;
    Ok(true)
}

/// 原图路由用：条目来源与原始扩展名（本地原图据此落盘）。
pub fn item_origin_and_ext(
    conn: &Connection,
    id: &str,
) -> Result<Option<(String, Option<String>)>> {
    conn.query_row(
        "SELECT origin, orig_ext FROM items WHERE id = ?1 AND deleted = 0",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(Into::into)
}

/// 桌面 pull 的单行：DTO + 合并所需版本字段。
#[derive(Clone, Debug)]
pub struct PullItem {
    pub item: WallItemDto,
    pub revision: i64,
    pub updated_at: i64,
}

/// 按 `(updated_at, id)` keyset 拉取。
///
/// 只同步 allow 条目；已删除条目仍要返回 tombstone（`del=true`），供桌面下架。
pub fn pull_items(
    conn: &Connection,
    since_updated: i64,
    since_id: &str,
    limit: u32,
) -> Result<(Vec<PullItem>, bool)> {
    let sql = format!(
        "SELECT {ITEM_COLUMNS}, i.revision, i.updated_at {ITEM_FROM}
          WHERE i.decision = 'allow'
            AND (i.deleted = 1 OR i.work_id IS NULL OR w.status IS NULL OR w.status = 'active')
            AND (i.updated_at > ?1 OR (i.updated_at = ?1 AND i.id > ?2))
          ORDER BY i.updated_at ASC, i.id ASC
          LIMIT ?3"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![since_updated, since_id, i64::from(limit) + 1],
        |row| {
            Ok(PullItem {
                item: map_item_row(row)?,
                revision: row.get(19)?,
                updated_at: row.get(20)?,
            })
        },
    )?;
    let mut items = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = items.len() > limit as usize;
    if has_more {
        items.truncate(limit as usize);
    }
    let mut dtos = items.iter().map(|row| row.item.clone()).collect::<Vec<_>>();
    attach_tags(conn, &mut dtos)?;
    for (row, dto) in items.iter_mut().zip(dtos) {
        row.item = dto;
    }
    Ok((items, has_more))
}

/// 桌面 push 的单条更新；字段为 `None` 表示不改。
#[derive(Clone, Debug)]
pub struct PushUpdate {
    pub id: String,
    pub base_revision: i64,
    pub star: Option<u8>,
    pub favorite: Option<bool>,
    pub rating: Option<String>,
    pub tags: Option<Vec<IngestTag>>,
    pub collections: Option<Vec<i64>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PushServerState {
    pub revision: i64,
    pub star: u8,
    pub favorite: bool,
    pub rating: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PushConflict {
    pub id: String,
    pub base_revision: i64,
    pub server: PushServerState,
}

/// 桌面 push 的结果；冲突与缺失分开返回，前端不静默覆盖。
#[derive(Clone, Debug, Default)]
pub struct PushApply {
    pub applied: u32,
    pub conflicts: Vec<PushConflict>,
    pub missing: Vec<String>,
}

/// 乐观并发合并：`base_revision` 与服务端当前版本不一致即冲突。
pub fn apply_push(conn: &mut Connection, updates: &[PushUpdate], now: i64) -> Result<PushApply> {
    let tx = conn.transaction()?;
    let mut apply = PushApply::default();
    let mut search_dirty: Vec<String> = Vec::new();
    for update in updates {
        let current: Option<(i64, i64, i64, String)> = tx
            .query_row(
                "SELECT revision, star, favorite, rating FROM items WHERE id = ?1",
                [&update.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((revision, star, favorite, rating)) = current else {
            apply.missing.push(update.id.clone());
            continue;
        };
        if revision != update.base_revision {
            apply.conflicts.push(PushConflict {
                id: update.id.clone(),
                base_revision: update.base_revision,
                server: PushServerState {
                    revision,
                    star: star.clamp(0, 5) as u8,
                    favorite: favorite != 0,
                    rating,
                },
            });
            continue;
        }

        let next_revision = revision + 1;
        if let Some(star) = update.star {
            tx.execute(
                "UPDATE items SET star = ?2 WHERE id = ?1",
                params![&update.id, i64::from(star.min(5))],
            )?;
        }
        if let Some(favorite) = update.favorite {
            tx.execute(
                "UPDATE items SET favorite = ?2 WHERE id = ?1",
                params![&update.id, if favorite { 1 } else { 0 }],
            )?;
        }
        if let Some(rating) = &update.rating {
            let rating = normalize_rating(rating);
            tx.execute(
                "UPDATE items SET rating = ?2 WHERE id = ?1",
                params![&update.id, rating],
            )?;
            search_dirty.push(update.id.clone());
        }
        tx.execute(
            "UPDATE items SET revision = ?2, updated_at = ?3 WHERE id = ?1",
            params![&update.id, next_revision, now],
        )?;
        if let Some(tags) = &update.tags {
            tx.execute(
                "DELETE FROM item_tags WHERE item_id = ?1 AND origin = 'desktop'",
                [&update.id],
            )?;
            for tag in tags {
                let name = tag.name.trim();
                if name.is_empty() || name.len() > 256 {
                    continue;
                }
                let tag_id = upsert_tag(&tx, name, tag.category.trim())?;
                tx.execute(
                    "INSERT INTO item_tags (item_id, tag_id, score, origin)
                     VALUES (?1, ?2, ?3, 'desktop')
                     ON CONFLICT(item_id, tag_id, origin) DO UPDATE SET score = excluded.score",
                    params![&update.id, tag_id, normalize_score(tag.score)],
                )?;
            }
            search_dirty.push(update.id.clone());
        }
        if let Some(collections) = &update.collections {
            tx.execute(
                "DELETE FROM collection_items WHERE item_id = ?1",
                [&update.id],
            )?;
            for collection_id in collections {
                tx.execute(
                    "INSERT OR IGNORE INTO collection_items (collection_id, item_id)
                     SELECT id, ?2 FROM collections WHERE id = ?1",
                    params![collection_id, &update.id],
                )?;
            }
        }
        apply.applied += 1;
    }
    search_dirty.sort();
    search_dirty.dedup();
    refresh_search_index(&tx, &search_dirty)?;
    tx.commit()?;
    Ok(apply)
}

#[derive(Clone, Debug)]
pub struct DeviceAuth {
    pub id: String,
    pub can_pull: bool,
    pub can_push: bool,
}

/// 通过令牌哈希查设备；撤销或不存在返回 `None`。
pub fn authenticate_device(
    conn: &Connection,
    token_hash: &str,
    now: i64,
) -> Result<Option<DeviceAuth>> {
    let device = conn
        .query_row(
            "SELECT id, can_pull, can_push FROM desktop_devices
              WHERE token_hash = ?1 AND revoked = 0",
            [token_hash],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((id, can_pull, can_push)) = device else {
        return Ok(None);
    };
    conn.execute(
        "UPDATE desktop_devices SET last_seen_at = ?2 WHERE id = ?1",
        params![&id, now],
    )?;
    Ok(Some(DeviceAuth {
        id,
        can_pull: can_pull != 0,
        can_push: can_push != 0,
    }))
}

pub fn insert_device(conn: &Connection, id: &str, name: &str, token_hash: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO desktop_devices (id, name, token_hash, can_pull, can_push, revoked)
         VALUES (?1, ?2, ?3, 1, 1, 0)",
        params![id, name, token_hash],
    )?;
    Ok(())
}

pub fn item_is_live(conn: &Connection, id: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM items WHERE id = ?1 AND deleted = 0)",
        [id],
        |row| row.get(0),
    )?;
    Ok(exists != 0)
}

/// 公开网页可见：未删除、已审核 allow，且作品未整体下架。
pub fn item_is_public(conn: &Connection, id: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM items i
             LEFT JOIN works w ON w.id = i.work_id
             WHERE i.id = ?1
               AND i.deleted = 0
               AND i.decision = 'allow'
               AND (i.work_id IS NULL OR w.status IS NULL OR w.status = 'active')
         )",
        [id],
        |row| row.get(0),
    )?;
    Ok(exists != 0)
}

pub fn item_orig_ext(conn: &Connection, id: &str) -> Result<Option<String>> {
    conn.query_row("SELECT orig_ext FROM items WHERE id = ?1", [id], |row| {
        row.get(0)
    })
    .optional()
    .map(|value| value.flatten())
    .map_err(Into::into)
}

fn as_u32(value: i64) -> u32 {
    value.clamp(0, i64::from(u32::MAX)) as u32
}

fn as_u64(value: i64) -> u64 {
    value.max(0) as u64
}

fn opt_text(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn opt_ext(value: Option<String>) -> Option<String> {
    opt_text(value).map(|text| text.trim_start_matches('.').to_ascii_lowercase())
}

fn map_item_row(row: &Row<'_>) -> rusqlite::Result<WallItemDto> {
    let rating: String = row.get(4)?;
    let deleted: i64 = row.get(17)?;
    Ok(WallItemDto {
        id: row.get(0)?,
        w: as_u32(row.get(1)?),
        h: as_u32(row.get(2)?),
        n: row.get(3)?,
        rt: normalize_rating(&rating),
        st: row.get::<_, i64>(5)?.clamp(0, 5) as u8,
        fav: row.get::<_, i64>(6)? != 0,
        tags: Vec::new(),
        mtime: as_u64(row.get(7)?),
        g: opt_text(row.get(8)?),
        ph: opt_text(row.get(9)?).map(|text| text.to_ascii_lowercase()),
        orig: opt_ext(row.get(10)?),
        sharp: row.get::<_, Option<i64>>(11)?.map(as_u32),
        wid: row.get(12)?,
        pc: row.get::<_, Option<i64>>(13)?.map(as_u32),
        pg: row.get::<_, Option<i64>>(14)?.map(as_u32),
        au: opt_text(row.get(15)?),
        auid: row.get(16)?,
        del: (deleted != 0).then_some(true),
        title: opt_text(row.get(18)?),
    })
}

fn attach_tags(conn: &Connection, items: &mut [WallItemDto]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let ids = items.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
    let mut map = load_tag_map(conn, &ids)?;
    for item in items {
        item.tags = map.remove(&item.id).unwrap_or_default();
    }
    Ok(())
}

type ItemTagMap = HashMap<String, Vec<(String, String, f32)>>;

fn load_tag_map(conn: &Connection, ids: &[String]) -> Result<ItemTagMap> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT it.item_id, t.name, t.category, it.score
           FROM item_tags it JOIN tags t ON t.id = it.tag_id
          WHERE it.item_id IN ({placeholders})
          ORDER BY it.score DESC, t.name ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(ids.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, f64>(3)?,
        ))
    })?;

    let mut collected: HashMap<String, HashMap<String, (String, f64)>> = HashMap::new();
    for row in rows {
        let (item_id, name, category, score) = row?;
        let entry = collected
            .entry(item_id)
            .or_default()
            .entry(name)
            .or_insert((category, 0.0));
        if score > entry.1 {
            entry.1 = score;
        }
    }

    let mut result = HashMap::new();
    for (item_id, tags) in collected {
        let mut tags = tags
            .into_iter()
            .map(|(name, (category, score))| (name, category, normalize_score(score)))
            .collect::<Vec<_>>();
        tags.sort_by(|left, right| {
            right
                .2
                .partial_cmp(&left.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.0.cmp(&right.0))
        });
        result.insert(item_id, tags);
    }
    Ok(result)
}

fn normalize_score(score: f64) -> f32 {
    if !score.is_finite() {
        return 0.0;
    }
    ((score.clamp(0.0, 1.0) * 1000.0).round() / 1000.0) as f32
}

fn map_work_row(row: &Row<'_>, author: Option<AuthorDto>) -> rusqlite::Result<WorkDto> {
    Ok(WorkDto {
        id: row.get(0)?,
        source: row.get(1)?,
        external_id: row.get(2)?,
        title: row.get(3)?,
        author,
        page_count: as_u32(row.get(5)?),
        posted_at: row.get::<_, Option<i64>>(6)?.map(as_u64),
        status: row.get(7)?,
        tags: Vec::new(),
        pages: Vec::new(),
    })
}

fn collect_works(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<WorkDto>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params, |row| {
        let author_id: Option<String> = row.get(4)?;
        map_work_row(row, None).map(|work| (work, author_id))
    })?;
    let mut works = Vec::new();
    for row in rows {
        let (mut work, author_id) = row?;
        if let Some(author_id) = author_id {
            work.author = get_author(conn, &author_id)?;
        }
        works.push(work);
    }
    Ok(works)
}

fn work_author_id(conn: &Connection, id: &str) -> Result<Option<String>> {
    conn.query_row("SELECT author_id FROM works WHERE id = ?1", [id], |row| {
        row.get(0)
    })
    .optional()
    .map(|value| value.flatten())
    .map_err(Into::into)
}

fn work_tags(conn: &Connection, id: &str) -> Result<Vec<(String, String, f32)>> {
    let mut stmt = conn.prepare(
        "SELECT t.name, t.category, MAX(it.score) AS score
           FROM item_tags it
           JOIN tags t ON t.id = it.tag_id
           JOIN work_items wi ON wi.item_id = it.item_id
          WHERE wi.work_id = ?1
          GROUP BY t.id
          ORDER BY score DESC, t.name ASC",
    )?;
    let rows = stmt.query_map([id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            normalize_score(row.get::<_, f64>(2)?),
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn work_pages(conn: &Connection, id: &str) -> Result<Vec<WorkPageDto>> {
    let mut stmt = conn.prepare(
        "SELECT wi.item_id, wi.page, i.width, i.height, i.sharp, i.orig_ext
           FROM work_items wi JOIN items i ON i.id = wi.item_id
          WHERE wi.work_id = ?1 AND i.deleted = 0 AND i.decision = 'allow'
          ORDER BY wi.page ASC",
    )?;
    let rows = stmt.query_map([id], |row| {
        Ok(WorkPageDto {
            id: row.get(0)?,
            page: as_u32(row.get(1)?),
            w: as_u32(row.get(2)?),
            h: as_u32(row.get(3)?),
            sharp: as_u32(row.get(4)?),
            orig: opt_ext(row.get(5)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn map_author_row(row: &Row<'_>) -> rusqlite::Result<AuthorDto> {
    Ok(AuthorDto {
        id: row.get(0)?,
        name: row.get(1)?,
        avatar: row.get(2)?,
        work_count: as_u32(row.get(3)?),
        item_count: as_u32(row.get(4)?),
        covers: Vec::new(),
    })
}

fn author_covers(conn: &Connection, id: &str, limit: u32) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT i.id FROM items i JOIN works w ON w.id = i.work_id
          WHERE w.author_id = ?1 AND i.deleted = 0 AND i.decision = 'allow'
            AND w.status = 'active'
          ORDER BY i.updated_at DESC, i.id DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![id, i64::from(limit)], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn map_collection_row(row: &Row<'_>) -> rusqlite::Result<CollectionDto> {
    Ok(CollectionDto {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        item_count: as_u32(row.get(3)?),
        work_count: as_u32(row.get(4)?),
        covers: Vec::new(),
    })
}

fn collection_covers(conn: &Connection, id: i64, limit: u32) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT i.id FROM collection_items ci JOIN items i ON i.id = ci.item_id
          WHERE ci.collection_id = ?1 AND i.deleted = 0
          ORDER BY i.updated_at DESC, i.id DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![id, i64::from(limit)], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
