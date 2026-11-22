//! I3：把 allow 批次与删除事件可靠推给网页，并签发/校验原图短期 URL。
//!
//! 批次先落 `push_batches`，重启后继续重试；HMAC 头与网页 W4 完全一致。

use std::{fmt, future::Future, pin::Pin, time::Duration};

use anyhow::{Context, Result, bail};
use hmac::{Hmac, Mac};
use kanalib_wall_model::{
    IngestBatch, IngestBatchItem, IngestDelete, IngestDeleteItem, IngestItem, IngestTag,
    valid_item_id,
};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::db::Db;

pub const ALLOW_PATH: &str = "/internal/ingest/batch";
pub const DELETE_PATH: &str = "/internal/ingest/delete";
pub const THUMB_PREFIX: &str = "/internal/ingest/thumb";
const ERROR_MAX_CHARS: usize = 512;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug)]
pub struct PushConfig {
    /// 网页 private listener，例如 `http://10.126.126.2:8788`。
    pub base_url: String,
    pub secret: String,
    pub timeout: Duration,
    pub max_attempts: u32,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
}

impl PushConfig {
    pub fn new(base_url: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            secret: secret.into(),
            timeout: Duration::from_secs(30),
            max_attempts: 5,
            backoff_base: Duration::from_secs(5),
            backoff_max: Duration::from_secs(600),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushKind {
    Allow,
    Delete,
}

impl PushKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Delete => "delete",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Allow => ALLOW_PATH,
            Self::Delete => DELETE_PATH,
        }
    }

    fn parse(raw: &str) -> Self {
        if raw == "delete" {
            Self::Delete
        } else {
            Self::Allow
        }
    }
}

#[derive(Clone, Debug)]
pub struct PushBatch {
    pub id: String,
    pub kind: PushKind,
    pub payload: Vec<u8>,
    pub attempts: u32,
}

#[derive(Clone, Debug)]
pub struct PushRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct PushResponse {
    pub status: u16,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct PushError(pub String);

impl fmt::Display for PushError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PushError {}

pub trait PushTransport {
    fn send<'a>(
        &'a self,
        request: PushRequest,
    ) -> Pin<Box<dyn Future<Output = Result<PushResponse, PushError>> + Send + 'a>>;
}

pub struct ReqwestPushTransport {
    client: reqwest::Client,
}

impl ReqwestPushTransport {
    pub fn new(timeout: Duration) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .context("build push HTTP client")?;
        Ok(Self { client })
    }
}

impl PushTransport for ReqwestPushTransport {
    fn send<'a>(
        &'a self,
        request: PushRequest,
    ) -> Pin<Box<dyn Future<Output = Result<PushResponse, PushError>> + Send + 'a>> {
        Box::pin(async move {
            let mut builder = self.client.post(&request.url).body(request.body);
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            let response = builder
                .send()
                .await
                .map_err(|error| PushError(error.to_string()))?;
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            Ok(PushResponse { status, body })
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Ack,
    Reject,
    Retry(Duration),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PushOutcome {
    pub acked: usize,
    pub retried: usize,
    pub rejected: usize,
}

/// 已持久化的待推批次；`now` 之前到期的先发。
pub fn pending(db: &Db, now: i64, limit: usize) -> Result<Vec<PushBatch>> {
    let mut statement = db.connection().prepare(
        "SELECT id, kind, payload_json, attempts FROM push_batches
          WHERE state = 'pending' AND (next_retry_at IS NULL OR next_retry_at <= ?1)
          ORDER BY created_at, id LIMIT ?2",
    )?;
    let rows = statement.query_map(params![now, limit as i64], |row| {
        Ok(PushBatch {
            id: row.get(0)?,
            kind: PushKind::parse(&row.get::<_, String>(1)?),
            payload: row.get::<_, String>(2)?.into_bytes(),
            attempts: row.get(3)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// 幂等入队；同 id 已存在时返回 `false`。
pub fn enqueue_allow(db: &Db, batch_id: &str, batch: &IngestBatch) -> Result<bool> {
    let payload = serde_json::to_vec(batch).context("serialize allow batch")?;
    enqueue(db, batch_id, PushKind::Allow, &payload, batch.items.len())
}

pub fn enqueue_delete(db: &Db, batch_id: &str, delete: &IngestDelete) -> Result<bool> {
    let payload = serde_json::to_vec(delete).context("serialize delete batch")?;
    enqueue(db, batch_id, PushKind::Delete, &payload, delete.items.len())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Enqueued {
    pub allow_batches: usize,
    pub allow_items: usize,
    pub delete_batches: usize,
    pub delete_items: usize,
}

/// 从库里已有的 `allow` 决策与已删除作品生成批次。
///
/// 这里不产生决策：规则层落地前 `items.decision` 仍是 `pending`，没有候选项可推。
pub fn enqueue_decisions(
    db: &Db,
    rules_version: Option<&str>,
    batch_size: usize,
) -> Result<Enqueued> {
    let batch_size = batch_size.clamp(1, 5_000);
    let mut outcome = Enqueued::default();

    for (source, ids) in allow_candidates(db)?.into_iter() {
        for chunk in ids.chunks(batch_size) {
            match build_allow_batch(db, chunk, rules_version) {
                Ok(batch) => {
                    enqueue_allow_batch(db, &batch, &mut outcome)?;
                }
                Err(error) => {
                    tracing::warn!(
                        %source,
                        %error,
                        items = chunk.len(),
                        "allow batch skipped; retrying items individually"
                    );
                    for id in chunk {
                        match build_allow_batch(db, std::slice::from_ref(id), rules_version) {
                            Ok(batch) => enqueue_allow_batch(db, &batch, &mut outcome)?,
                            Err(error) => {
                                tracing::warn!(%source, item = %id, %error, "allow item skipped");
                            }
                        }
                    }
                }
            }
        }
    }

    for (source, items) in delete_candidates(db)?.into_iter() {
        for chunk in items.chunks(batch_size) {
            let delete = IngestDelete {
                source: source.clone(),
                rules_version: rules_version.map(str::to_string),
                items: chunk.to_vec(),
            };
            let payload = serde_json::to_vec(&delete).context("serialize delete batch")?;
            if enqueue_delete(db, &delete_id_for(&payload), &delete)? {
                outcome.delete_batches += 1;
                outcome.delete_items += delete.items.len();
            }
        }
    }

    Ok(outcome)
}

fn enqueue_allow_batch(db: &Db, batch: &IngestBatch, outcome: &mut Enqueued) -> Result<()> {
    let payload = serde_json::to_vec(batch).context("serialize allow batch")?;
    if enqueue_allow(db, &batch_id_for(&payload), batch)? {
        outcome.allow_batches += 1;
        outcome.allow_items += batch.items.len();
    }
    Ok(())
}

fn allow_candidates(db: &Db) -> Result<std::collections::BTreeMap<String, Vec<String>>> {
    let mut statement = db.connection().prepare(
        "SELECT i.id, i.source
           FROM items i
           JOIN works w ON w.id = i.work_id
          WHERE i.decision = 'allow'
            AND i.revision > i.pushed_revision
            AND i.thumb_256 IS NOT NULL
            AND w.status != 'deleted'
            AND lower(w.rating) IN ('all', 'r18', 'g')
            AND EXISTS (
                SELECT 1 FROM decisions d
                 WHERE d.item_id = i.id AND d.decision = 'allow'
            )
          ORDER BY i.updated_at, i.id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut grouped = std::collections::BTreeMap::<String, Vec<String>>::new();
    for row in rows {
        let (id, source) = row?;
        grouped.entry(source).or_default().push(id);
    }
    Ok(grouped)
}

fn delete_candidates(db: &Db) -> Result<std::collections::BTreeMap<String, Vec<IngestDeleteItem>>> {
    let mut statement = db.connection().prepare(
        "SELECT i.id, i.revision, i.source
           FROM items i
           JOIN works w ON w.id = i.work_id
          WHERE w.status = 'deleted'
            AND i.pushed_revision > 0
            AND i.delete_pushed = 0
          ORDER BY i.updated_at, i.id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut grouped = std::collections::BTreeMap::<String, Vec<IngestDeleteItem>>::new();
    for row in rows {
        let (id, revision, source) = row?;
        grouped.entry(source).or_default().push(IngestDeleteItem {
            id,
            revision: revision.max(1),
        });
    }
    Ok(grouped)
}

/// 把已确认/失败的批次重置为待推送；用于备份恢复或人工重放。
pub fn replay(db: &Db, batch_id: &str) -> Result<bool> {
    let changed = db.connection().execute(
        "UPDATE push_batches
            SET state = 'pending', attempts = 0, next_retry_at = NULL,
                acked_at = NULL, last_error = NULL
          WHERE id = ?1",
        [batch_id],
    )?;
    Ok(changed > 0)
}

fn enqueue(
    db: &Db,
    batch_id: &str,
    kind: PushKind,
    payload: &[u8],
    item_count: usize,
) -> Result<bool> {
    if batch_id.trim().is_empty() {
        bail!("batch id must not be empty");
    }
    let changed = db.connection().execute(
        "INSERT OR IGNORE INTO push_batches
            (id, item_count, payload_hash, payload_json, kind, state, next_retry_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'pending', NULL, ?6)",
        params![
            batch_id,
            item_count as i64,
            sha256_hex(payload),
            String::from_utf8_lossy(payload).to_string(),
            kind.as_str(),
            now(),
        ],
    )?;
    Ok(changed > 0)
}

pub async fn flush<T: PushTransport>(
    db: &Db,
    config: &PushConfig,
    transport: &T,
    now_ts: i64,
    limit: usize,
) -> Result<PushOutcome> {
    let mut outcome = PushOutcome::default();
    for batch in pending(db, now_ts, limit)? {
        let attempts = batch.attempts + 1;
        if batch.kind == PushKind::Allow
            && let Err(error) = sync_thumbs(db, config, transport, &batch.payload, now_ts).await
        {
            let delay = backoff(attempts, config.backoff_base, config.backoff_max);
            mark_retry(
                db,
                &batch.id,
                attempts,
                config.max_attempts,
                now_ts + i64::try_from(delay.as_secs()).unwrap_or(i64::MAX),
                &error,
            )?;
            outcome.retried += 1;
            continue;
        }
        let request = signed_request(config, &batch, now_ts);
        match transport.send(request).await {
            Ok(response) => match classify_status(response.status, attempts, config) {
                Disposition::Ack => {
                    mark_acked(db, &batch, now_ts)?;
                    outcome.acked += 1;
                }
                Disposition::Reject => {
                    mark_rejected(db, &batch.id, attempts, &response.body)?;
                    outcome.rejected += 1;
                }
                Disposition::Retry(delay) => {
                    mark_retry(
                        db,
                        &batch.id,
                        attempts,
                        config.max_attempts,
                        now_ts + i64::try_from(delay.as_secs()).unwrap_or(i64::MAX),
                        &response.body,
                    )?;
                    outcome.retried += 1;
                }
            },
            Err(error) => {
                let delay = backoff(attempts, config.backoff_base, config.backoff_max);
                mark_retry(
                    db,
                    &batch.id,
                    attempts,
                    config.max_attempts,
                    now_ts + i64::try_from(delay.as_secs()).unwrap_or(i64::MAX),
                    &error.to_string(),
                )?;
                outcome.retried += 1;
            }
        }
    }
    Ok(outcome)
}

/// 批次里的 allow 条目必须先把缩略图送到网页，否则 `/media/t256/<id>.jpg` 会 404。
async fn sync_thumbs<T: PushTransport>(
    db: &Db,
    config: &PushConfig,
    transport: &T,
    payload: &[u8],
    now_ts: i64,
) -> Result<(), String> {
    let batch: IngestBatch =
        serde_json::from_slice(payload).map_err(|error| format!("invalid allow batch: {error}"))?;
    for entry in batch.items {
        for (tier, column) in [("t256", "thumb_256"), ("t512", "thumb_512")] {
            let path: Option<String> = db
                .connection()
                .query_row(
                    &format!("SELECT {column} FROM items WHERE id = ?1"),
                    [&entry.item.id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| format!("thumbnail lookup failed: {error}"))?
                .flatten();
            let Some(path) = path else {
                continue;
            };
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::warn!(path, %error, "thumbnail file missing; skipped");
                    continue;
                }
            };
            let request = thumb_request(config, &entry.item.id, tier, bytes, now_ts);
            match transport.send(request).await {
                Ok(response) if (200..300).contains(&response.status) => {}
                Ok(response) => {
                    return Err(format!(
                        "thumbnail {tier} for {} returned status {}",
                        entry.item.id, response.status
                    ));
                }
                Err(error) => {
                    return Err(format!(
                        "thumbnail {tier} for {} failed: {error}",
                        entry.item.id
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn classify_status(status: u16, attempts: u32, config: &PushConfig) -> Disposition {
    match status {
        200..=299 => Disposition::Ack,
        408 | 425 | 429 | 500..=599 => {
            Disposition::Retry(backoff(attempts, config.backoff_base, config.backoff_max))
        }
        400..=499 => Disposition::Reject,
        _ => Disposition::Retry(backoff(attempts, config.backoff_base, config.backoff_max)),
    }
}

pub fn backoff(attempts: u32, base: Duration, max: Duration) -> Duration {
    let exponent = attempts.saturating_sub(1).min(20);
    let factor = 1u64 << exponent;
    base.saturating_mul(factor.min(u64::from(u32::MAX)) as u32)
        .min(max)
}

fn mark_acked(db: &Db, batch: &PushBatch, now_ts: i64) -> Result<()> {
    let tx = db.connection().unchecked_transaction()?;
    tx.execute(
        "UPDATE push_batches
            SET state = 'acked', acked_at = ?2, last_error = NULL
          WHERE id = ?1",
        params![batch.id, now_ts],
    )?;
    match batch.kind {
        PushKind::Allow => match serde_json::from_slice::<IngestBatch>(&batch.payload) {
            Ok(parsed) => {
                for entry in parsed.items {
                    tx.execute(
                        "UPDATE items
                            SET pushed_revision = MAX(pushed_revision, ?2)
                          WHERE id = ?1",
                        params![entry.item.id, entry.revision],
                    )?;
                }
            }
            Err(error) => {
                tracing::warn!(%error, batch = %batch.id, "acked allow batch is not parseable");
            }
        },
        PushKind::Delete => match serde_json::from_slice::<IngestDelete>(&batch.payload) {
            Ok(parsed) => {
                for entry in parsed.items {
                    tx.execute(
                        "UPDATE items SET delete_pushed = 1 WHERE id = ?1",
                        params![entry.id],
                    )?;
                }
            }
            Err(error) => {
                tracing::warn!(%error, batch = %batch.id, "acked delete batch is not parseable");
            }
        },
    }
    tx.commit()?;
    Ok(())
}

fn mark_rejected(db: &Db, batch_id: &str, attempts: u32, error: &str) -> Result<()> {
    db.connection().execute(
        "UPDATE push_batches
            SET state = 'rejected', attempts = ?2, last_error = ?3
          WHERE id = ?1",
        params![batch_id, attempts, truncate(error)],
    )?;
    Ok(())
}

fn mark_retry(
    db: &Db,
    batch_id: &str,
    attempts: u32,
    max_attempts: u32,
    next_retry_at: i64,
    error: &str,
) -> Result<()> {
    let state = if attempts >= max_attempts {
        "failed"
    } else {
        "pending"
    };
    db.connection().execute(
        "UPDATE push_batches
            SET state = ?2, attempts = ?3, next_retry_at = ?4, last_error = ?5
          WHERE id = ?1",
        params![batch_id, state, attempts, next_retry_at, truncate(error)],
    )?;
    Ok(())
}

fn truncate(text: &str) -> String {
    text.chars().take(ERROR_MAX_CHARS).collect()
}

pub fn signed_request(config: &PushConfig, batch: &PushBatch, now_ts: i64) -> PushRequest {
    let payload = signature_payload(batch.kind.path(), &batch.id, now_ts, &batch.payload);
    PushRequest {
        url: format!("{}{}", config.base_url, batch.kind.path()),
        headers: vec![
            ("x-kanalib-batch-id".to_string(), batch.id.clone()),
            ("x-kanalib-timestamp".to_string(), now_ts.to_string()),
            (
                "x-kanalib-signature".to_string(),
                hmac_sha256_hex(config.secret.as_bytes(), payload.as_bytes()),
            ),
            ("content-type".to_string(), "application/json".to_string()),
        ],
        body: batch.payload.clone(),
    }
}

/// 与网页 `routes::ingest::signature_payload` 一致的规范串。
pub fn signature_payload(path: &str, batch_id: &str, timestamp: i64, body: &[u8]) -> String {
    format!(
        "POST\n{path}\n{batch_id}\n{timestamp}\n{}",
        sha256_hex(body)
    )
}

pub fn sign_payload(
    secret: &str,
    path: &str,
    batch_id: &str,
    timestamp: i64,
    body: &[u8],
) -> String {
    let payload = signature_payload(path, batch_id, timestamp, body);
    hmac_sha256_hex(secret.as_bytes(), payload.as_bytes())
}

/// 缩略图上传请求；tier 为 `t256`/`t512`/`t1024`/`t2048`。
pub fn thumb_request(
    config: &PushConfig,
    item_id: &str,
    tier: &str,
    body: Vec<u8>,
    now_ts: i64,
) -> PushRequest {
    let file = format!("{item_id}.jpg");
    let path = format!("{THUMB_PREFIX}/{tier}/{file}");
    let payload = signature_payload(&path, "", now_ts, &body);
    PushRequest {
        url: format!("{}{path}", config.base_url),
        headers: vec![
            ("x-kanalib-timestamp".to_string(), now_ts.to_string()),
            (
                "x-kanalib-signature".to_string(),
                hmac_sha256_hex(config.secret.as_bytes(), payload.as_bytes()),
            ),
            ("content-type".to_string(), "image/jpeg".to_string()),
        ],
        body,
    }
}

/// 从库里挑出显式 allow 的条目，任一非 allow 直接失败（fail-closed）。
pub fn build_allow_batch(
    db: &Db,
    item_ids: &[String],
    rules_version: Option<&str>,
) -> Result<IngestBatch> {
    let mut items = Vec::with_capacity(item_ids.len());
    let mut source = None;
    for item_id in item_ids {
        if !valid_item_id(item_id) {
            bail!("invalid item id {item_id}");
        }
        let row = db
            .connection()
            .query_row(
                "SELECT i.source, i.width, i.height, i.page, i.ext, i.original_path,
                        i.decision, i.revision, i.work_id, w.page_count, w.title,
                        a.id, a.name, i.updated_at, i.thumb_256, i.thumb_512,
                        w.rating, i.phash, i.favorite
                   FROM items i
                   JOIN works w ON w.id = i.work_id
                   LEFT JOIN authors a ON a.id = w.author_id
                  WHERE i.id = ?1",
                [item_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, u32>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, u32>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, Option<String>>(11)?,
                        row.get::<_, Option<String>>(12)?,
                        row.get::<_, i64>(13)?,
                        row.get::<_, Option<String>>(14)?,
                        row.get::<_, Option<String>>(15)?,
                        row.get::<_, String>(16)?,
                        row.get::<_, Option<String>>(17)?,
                        row.get::<_, i64>(18)?,
                    ))
                },
            )
            .optional()?
            .with_context(|| format!("item {item_id} not found"))?;
        let (
            source_name,
            width,
            height,
            page,
            ext,
            original_path,
            decision,
            revision,
            work_id,
            page_count,
            title,
            author_id,
            author,
            updated_at,
            thumb_256,
            thumb_512,
            rating,
            phash,
            favorite,
        ) = row;
        if decision != "allow" {
            bail!("item {item_id} decision is {decision}, refusing to push");
        }
        let decision_rows: i64 = db.connection().query_row(
            "SELECT COUNT(*) FROM decisions WHERE item_id = ?1 AND decision = 'allow'",
            [item_id],
            |row| row.get(0),
        )?;
        if decision_rows == 0 {
            bail!("item {item_id} has no allow decision record, refusing to push");
        }
        source.get_or_insert(source_name);
        let tags = item_tags(db, item_id)?;
        let name = original_path
            .as_deref()
            .and_then(|path| std::path::Path::new(path).file_name())
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| match ext.as_deref() {
                Some(ext) => format!("{item_id}.{ext}"),
                None => item_id.clone(),
            });
        let rating = match crate::source::Rating::parse(&rating) {
            crate::source::Rating::All => "G",
            crate::source::Rating::R18 => "E",
            crate::source::Rating::R18G => {
                bail!("item {item_id} is R-18G, refusing to push")
            }
            crate::source::Rating::Unknown => {
                bail!("item {item_id} has unknown rating, refusing to push")
            }
        };
        if thumb_256.is_none() && thumb_512.is_none() {
            bail!("item {item_id} has no thumbnails, refusing to push");
        }
        // 前端只认 0/1024/2048；512 会被丢弃并导致详情页试探不存在的档位。
        let sharp = Some(0);
        items.push(IngestBatchItem {
            revision: revision.max(1),
            decision: "allow".to_string(),
            source_id: None,
            item: IngestItem {
                id: item_id.clone(),
                width,
                height,
                name,
                rating: rating.to_string(),
                star: 0,
                favorite: favorite != 0,
                tags,
                mtime: updated_at.max(0) as u64,
                group: None,
                phash,
                orig: ext,
                sharp,
                work_id: Some(work_id),
                page_count: Some(page_count),
                page: Some(page),
                author,
                author_id,
                deleted: None,
                title: (!title.is_empty()).then_some(title),
            },
        });
    }
    Ok(IngestBatch {
        source: source.unwrap_or_else(|| "pixiv".to_string()),
        rules_version: rules_version.map(str::to_string),
        items,
    })
}

fn item_tags(db: &Db, item_id: &str) -> Result<Vec<IngestTag>> {
    let mut statement = db.connection().prepare(
        "SELECT t.name, t.category, it.score
           FROM item_tags it JOIN tags t ON t.id = it.tag_id
          WHERE it.item_id = ?1
          ORDER BY t.name",
    )?;
    let rows = statement.query_map([item_id], |row| {
        Ok(IngestTag {
            name: row.get(0)?,
            category: row.get(1)?,
            score: row.get(2)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn batch_id_for(payload: &[u8]) -> String {
    format!("allow-{}", &sha256_hex(payload)[..32])
}

pub fn delete_id_for(payload: &[u8]) -> String {
    format!("delete-{}", &sha256_hex(payload)[..32])
}

/// 与网页 `routes::original` 相同的规范串。
pub fn original_canonical(item_id: &str, expires: i64) -> String {
    format!("GET\n/original/{item_id}\n{expires}")
}

pub fn sign_original(signing_key: &str, item_id: &str, expires: i64) -> String {
    hmac_sha256_hex(
        signing_key.as_bytes(),
        original_canonical(item_id, expires).as_bytes(),
    )
}

pub fn signed_original_url(
    base_url: &str,
    signing_key: &str,
    item_id: &str,
    ttl_secs: u64,
    now_ts: i64,
) -> String {
    let expires = now_ts.saturating_add(ttl_secs.max(1) as i64);
    let signature = sign_original(signing_key, item_id, expires);
    format!(
        "{}/original/{item_id}?expires={expires}&sig={signature}",
        base_url.trim_end_matches('/')
    )
}

pub fn verify_original(
    signing_key: &str,
    item_id: &str,
    expires: i64,
    signature: &str,
    now_ts: i64,
) -> bool {
    if now_ts > expires {
        return false;
    }
    let Some(expected) = hex_decode(signature) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(signing_key.as_bytes()) else {
        return false;
    };
    mac.update(original_canonical(item_id, expires).as_bytes());
    mac.verify_slice(&expected).is_ok()
}

fn sha256_hex(data: &[u8]) -> String {
    hex_lower(&Sha256::digest(data))
}

fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    hex_lower(&mac.finalize().into_bytes())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(input: &str) -> Option<Vec<u8>> {
    let input = input.trim();
    if !input.len().is_multiple_of(2) {
        return None;
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for index in (0..bytes.len()).step_by(2) {
        out.push((hex_value(bytes[index])? << 4) | hex_value(bytes[index + 1])?);
    }
    Some(out)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn now() -> i64 {
    crate::db::now()
}
