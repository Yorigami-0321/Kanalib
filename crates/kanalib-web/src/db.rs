//! 网页自己的 SQLite：schema v1、只读连接与测试样本。

use std::{collections::HashMap, path::Path, time::Duration};

use anyhow::{Context, Result};
use kanalib_wall_model::{
    AuthorDto, CollectionDto, IngestBatch, IngestBatchItem, IngestDelete, IngestTag, WallItemDto,
    WorkDto, WorkPageDto, normalize_rating, valid_item_id,
};
use rusqlite::types::Value as SqlValue;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Row, Transaction, params, params_from_iter,
};
use serde::Serialize;

use crate::query::{
    Cursor, ItemFilter, ItemSort, SearchField, SearchTerm, SortKey, SortKind, parse_search,
};

pub const SCHEMA_VERSION: u32 = 1;

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
        "INSERT INTO item_search (item_id, name, tags, group_name, rating)
         SELECT i.id, replace(i.name, '_', ' '),
                COALESCE((SELECT group_concat(replace(t.name, '_', ' '), ' ')
                            FROM item_tags it JOIN tags t ON t.id = it.tag_id
                           WHERE it.item_id = i.id), ''),
                replace(i.group_name, '_', ' '), i.rating
           FROM items i WHERE i.deleted = 0",
        [],
    )?;
    Ok(())
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
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    conn.execute_batch(SCHEMA_SQL)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(conn)
}

pub fn open_read(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open read-only database {}", path.display()))?;
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

pub fn count_items(conn: &Connection) -> Result<u32> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM items WHERE deleted = 0", [], |row| {
        row.get(0)
    })?;
    Ok(as_u32(count))
}

pub fn count_works(conn: &Connection) -> Result<u32> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM works", [], |row| row.get(0))?;
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

pub fn list_items(conn: &Connection, limit: u32, offset: u32) -> Result<Vec<WallItemDto>> {
    let sql = format!(
        "SELECT {ITEM_COLUMNS} {ITEM_FROM} \
         WHERE i.deleted = 0 \
         ORDER BY i.updated_at DESC, i.id DESC LIMIT ?1 OFFSET ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![i64::from(limit), i64::from(offset)], map_item_row)?;
    let mut items = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    attach_tags(conn, &mut items)?;
    Ok(items)
}

pub fn get_item(conn: &Connection, id: &str) -> Result<Option<WallItemDto>> {
    let sql = format!("SELECT {ITEM_COLUMNS} {ITEM_FROM} WHERE i.id = ?1 AND i.deleted = 0");
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
        "SELECT updated_at FROM items WHERE id = ?1 AND deleted = 0",
        [id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_works(conn: &Connection, limit: u32, offset: u32) -> Result<Vec<WorkDto>> {
    let sql = format!(
        "SELECT {WORK_COLUMNS} FROM works w \
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
        "SELECT {WORK_COLUMNS} FROM works w WHERE w.author_id = ?1 \
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
        "SELECT COUNT(*) FROM works WHERE author_id = ?1",
        [author_id],
        |row| row.get(0),
    )?;
    Ok(as_u32(count))
}

pub fn get_work(conn: &Connection, id: &str) -> Result<Option<WorkDto>> {
    let sql = format!("SELECT {WORK_COLUMNS} FROM works w WHERE w.id = ?1");
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
                (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id),
                (SELECT COUNT(*) FROM items i JOIN works w ON w.id = i.work_id
                  WHERE w.author_id = a.id AND i.deleted = 0)
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
                    (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id),
                    (SELECT COUNT(*) FROM items i JOIN works w ON w.id = i.work_id
                      WHERE w.author_id = a.id AND i.deleted = 0)
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
                  WHERE ci.collection_id = c.id AND i.deleted = 0),
                (SELECT COUNT(DISTINCT i.work_id)
                   FROM collection_items ci JOIN items i ON i.id = ci.item_id
                  WHERE ci.collection_id = c.id AND i.deleted = 0
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
         WHERE ci.collection_id = ?1 AND i.deleted = 0",
        [id],
        |row| row.get(0),
    )?;
    Ok(as_u32(count))
}

pub fn count_collection_works(conn: &Connection, id: i64) -> Result<u32> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT i.work_id) FROM collection_items ci
          JOIN items i ON i.id = ci.item_id
         WHERE ci.collection_id = ?1 AND i.deleted = 0 AND i.work_id IS NOT NULL",
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
         WHERE ci.collection_id = ?1 AND i.deleted = 0
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
         WHERE ci.collection_id = ?1 AND i.deleted = 0
         ORDER BY w.posted_at DESC, w.id DESC LIMIT ?2 OFFSET ?3"
    );
    collect_works(conn, &sql, params![id, i64::from(limit), i64::from(offset)])
}

pub fn list_tags(conn: &Connection, limit: u32) -> Result<Vec<FacetTag>> {
    let mut stmt = conn.prepare(
        "SELECT t.name, t.category, COUNT(*), MIN(it.item_id)
           FROM item_tags it
           JOIN tags t ON t.id = it.tag_id
           JOIN items i ON i.id = it.item_id
          WHERE i.deleted = 0
          GROUP BY t.id
          ORDER BY COUNT(*) DESC, t.name ASC LIMIT ?1",
    )?;
    let rows = stmt.query_map([i64::from(limit)], |row| {
        Ok(FacetTag {
            name: row.get(0)?,
            category: row.get(1)?,
            count: as_u32(row.get(2)?),
            cover: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn group_facets(conn: &Connection, limit: u32) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT group_name FROM items
          WHERE deleted = 0 AND group_name <> ''
          GROUP BY group_name
          ORDER BY COUNT(*) DESC, group_name ASC LIMIT ?1",
    )?;
    let rows = stmt.query_map([i64::from(limit)], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn index_facets(conn: &Connection) -> Result<IndexFacets> {
    index_facets_filtered(conn, &ItemFilter::default())
}

pub fn index_facets_filtered(conn: &Connection, filter: &ItemFilter) -> Result<IndexFacets> {
    let mut parts = filter_parts(filter);
    parts.inline_candidates();
    let where_sql = parts.where_sql();

    let tags_sql = format!(
        "SELECT t.name, t.category, COUNT(*), MIN(it.item_id)
           FROM item_tags it
           JOIN tags t ON t.id = it.tag_id
           JOIN items i ON i.id = it.item_id
          WHERE {where_sql}
          GROUP BY t.id
          ORDER BY COUNT(*) DESC, t.name ASC LIMIT 2000"
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
    let sort_expr = sort_expression(sort);
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
        })
    } else {
        None
    };

    let mut items = rows.into_iter().map(|(item, _)| item).collect::<Vec<_>>();
    attach_tags(conn, &mut items)?;
    Ok(ItemPage { items, next })
}

pub fn count_items_filtered(conn: &Connection, filter: &ItemFilter) -> Result<u32> {
    let mut parts = filter_parts(filter);
    parts.inline_candidates();
    let sql = format!("SELECT COUNT(*) FROM items i WHERE {}", parts.where_sql());
    let mut stmt = conn.prepare(&sql)?;
    let bindings = parts.bindings();
    let count: i64 = stmt.query_row(bindings.as_slice(), |row| row.get(0))?;
    Ok(as_u32(count))
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
            clauses: vec!["i.deleted = 0".to_string()],
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
            "(i.name LIKE {placeholder} ESCAPE '\\'
              OR i.group_name LIKE {placeholder} ESCAPE '\\'
              OR i.rating LIKE {placeholder} ESCAPE '\\'
              OR {any_tag})"
        ),
        SearchField::Name => {
            format!("i.name LIKE {placeholder} ESCAPE '\\'")
        }
        SearchField::Tag => any_tag,
        SearchField::Group => {
            format!("i.group_name LIKE {placeholder} ESCAPE '\\'")
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
            _ => out.push(ch),
        }
    }
    out
}

fn sort_expression(sort: &ItemSort) -> String {
    match sort.kind {
        SortKind::Updated => "i.updated_at".to_string(),
        SortKind::Posted => "COALESCE(i.posted_at, 0)".to_string(),
        SortKind::Name => "i.name COLLATE NOCASE".to_string(),
        SortKind::Star => "i.star".to_string(),
        SortKind::Favorite => "i.favorite".to_string(),
        SortKind::Rating => "CASE rating WHEN 'G' THEN 0 WHEN 'S' THEN 1 WHEN 'Q' THEN 2 \
             WHEN 'E' THEN 3 ELSE 0 END"
            .to_string(),
        // id 是内容哈希，直接按 id 排就是稳定的伪随机顺序，且能走主键索引。
        SortKind::Random => "i.id".to_string(),
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
    if batch_exists(conn, batch_id)? {
        return Ok(IngestApply {
            duplicate: true,
            ..IngestApply::default()
        });
    }

    let tx = conn.transaction()?;
    let mut apply = IngestApply::default();
    for entry in &batch.items {
        match upsert_ingest_item(&tx, batch, entry, now)? {
            ItemWrite::Inserted => apply.inserted += 1,
            ItemWrite::Updated => apply.updated += 1,
            ItemWrite::Stale => apply.stale += 1,
        }
    }
    record_batch(
        &tx,
        batch_id,
        &batch.source,
        batch.items.len(),
        batch.rules_version.as_deref(),
        now,
    )?;
    rebuild_search_index(&tx)?;
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
    if batch_exists(conn, batch_id)? {
        return Ok(DeleteApply {
            duplicate: true,
            ..DeleteApply::default()
        });
    }

    let tx = conn.transaction()?;
    let mut apply = DeleteApply::default();
    for entry in &delete.items {
        let current: Option<i64> = tx
            .query_row(
                "SELECT revision FROM items WHERE id = ?1",
                [&entry.id],
                |row| row.get(0),
            )
            .optional()?;
        match current {
            None => apply.missing += 1,
            Some(revision) if revision > entry.revision => apply.stale += 1,
            Some(_) => {
                tx.execute(
                    "UPDATE items SET deleted = 1, revision = ?2, updated_at = ?3
                      WHERE id = ?1",
                    params![&entry.id, entry.revision, now],
                )?;
                apply.deleted += 1;
            }
        }
    }
    record_batch(
        &tx,
        batch_id,
        &delete.source,
        delete.items.len(),
        delete.rules_version.as_deref(),
        now,
    )?;
    rebuild_search_index(&tx)?;
    tx.commit()?;
    Ok(apply)
}

enum ItemWrite {
    Inserted,
    Updated,
    Stale,
}

fn batch_exists(conn: &Connection, batch_id: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM ingest_batches WHERE id = ?1)",
        [batch_id],
        |row| row.get(0),
    )?;
    Ok(exists != 0)
}

fn record_batch(
    tx: &Transaction<'_>,
    batch_id: &str,
    source: &str,
    item_count: usize,
    rules_version: Option<&str>,
    now: i64,
) -> Result<()> {
    let item_count = i64::try_from(item_count).unwrap_or(i64::MAX);
    tx.execute(
        "INSERT INTO ingest_batches (id, source, item_count, rules_version, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![batch_id, source, item_count, rules_version, now],
    )?;
    Ok(())
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
             VALUES (?1, ?2, ?3, ?4, ?5, 'active', NULL)
             ON CONFLICT(id) DO UPDATE SET
                title = CASE WHEN excluded.title <> '' THEN excluded.title ELSE works.title END,
                author_id = COALESCE(excluded.author_id, works.author_id),
                page_count = MAX(works.page_count, excluded.page_count)",
            params![
                work_id,
                &batch.source,
                item.title.as_deref().unwrap_or("").trim(),
                item.author_id.as_deref(),
                i64::from(item.page_count.unwrap_or(1).max(1)),
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
        if item.page.unwrap_or(0) == 0 {
            tx.execute(
                "UPDATE works SET cover_item_id = COALESCE(cover_item_id, ?2) WHERE id = ?1",
                params![work_id, &item.id],
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
            category = CASE WHEN excluded.category <> '' THEN excluded.category ELSE tags.category END",
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

/// 桌面 pull 的单行：DTO + 合并所需版本字段。
#[derive(Clone, Debug)]
pub struct PullItem {
    pub item: WallItemDto,
    pub revision: i64,
    pub updated_at: i64,
}

/// 按 `(updated_at, id)` keyset 拉取，包含已删除条目，方便桌面同步下架。
pub fn pull_items(
    conn: &Connection,
    since_updated: i64,
    since_id: &str,
    limit: u32,
) -> Result<(Vec<PullItem>, bool)> {
    let sql = format!(
        "SELECT {ITEM_COLUMNS}, i.revision, i.updated_at {ITEM_FROM}
          WHERE (i.updated_at > ?1 OR (i.updated_at = ?1 AND i.id > ?2))
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
    let mut search_dirty = false;
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
            search_dirty = true;
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
            search_dirty = true;
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
    if search_dirty {
        rebuild_search_index(&tx)?;
    }
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
          WHERE wi.work_id = ?1 AND i.deleted = 0
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
          WHERE w.author_id = ?1 AND i.deleted = 0
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
