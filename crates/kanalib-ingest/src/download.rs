//! 下载队列：`.part`、Range 续传、成功才 rename、部分成功与重启恢复。

use std::{io::Write, path::PathBuf, pin::Pin};

use anyhow::{Context, Result, bail};
use futures::StreamExt;
use image::{ImageFormat, ImageReader};
use sha1::Digest;
use sha1::Sha1;
use tracing::{info, warn};

use crate::db::{Db, DownloadResult, ItemRow, now};
use crate::source::{ImageRef, SourceAdapter, SourceError, SourceErrorKind, SourceWork};

pub struct DownloadConfig {
    pub media_dir: PathBuf,
    pub concurrency: usize,
    pub retry_max: u32,
    pub timeout: std::time::Duration,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            media_dir: PathBuf::from("media"),
            concurrency: 3,
            retry_max: 5,
            timeout: std::time::Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DownloadReport {
    pub succeeded: Vec<String>,
    pub failed: Vec<(String, String)>,
}

pub trait ImageTransport: Send + Sync {
    fn fetch<'a>(
        &self,
        url: &str,
        range_start: Option<u64>,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ImageResponse, SourceError>> + Send + 'a>>;
}

#[derive(Debug, Clone)]
pub struct ImageResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub content_range: Option<String>,
    pub content_length: Option<u64>,
    pub body: Vec<u8>,
}

impl ImageResponse {
    pub fn ok(body: Vec<u8>, content_type: &str) -> Self {
        Self {
            status: 200,
            content_type: Some(content_type.to_string()),
            content_range: None,
            content_length: Some(body.len() as u64),
            body,
        }
    }
}

pub fn recover(db: &Db, media_dir: &std::path::Path) -> Result<usize> {
    let changed = db.recover_running_downloads()?;
    let _ = media_dir;
    Ok(changed)
}

pub async fn run_queue(
    db: &Db,
    adapter: &dyn SourceAdapter,
    transport: &dyn ImageTransport,
    config: &DownloadConfig,
) -> Result<DownloadReport> {
    std::fs::create_dir_all(&config.media_dir)?;
    let mut report = DownloadReport {
        succeeded: Vec::new(),
        failed: Vec::new(),
    };

    let mut attempts_by_item = std::collections::HashMap::new();
    loop {
        db.requeue_due_paused_downloads()?;
        let pending = db.pending_item_ids()?;
        if pending.is_empty() {
            break;
        }
        let mut items = Vec::with_capacity(pending.len());
        for item_id in pending {
            if !db.claim_download(&item_id)? {
                continue;
            }
            let Some(item) = db.item_row(&item_id)? else {
                continue;
            };
            items.push(item);
        }
        if items.is_empty() {
            break;
        }
        let outcomes = futures::stream::iter(items)
            .map(|item| async move {
                let item_id = item.id.clone();
                let outcome = process_item(db, adapter, transport, &item, config).await;
                (item_id, outcome)
            })
            .buffer_unordered(config.concurrency.max(1))
            .collect::<Vec<_>>()
            .await;
        for (item_id, outcome) in outcomes {
            match outcome {
                Ok(()) => {
                    db.finish_download(&item_id)?;
                    report.succeeded.push(item_id);
                }
                Err(error) => {
                    let reason = format!("{error:#}");
                    let attempts = attempts_by_item.entry(item_id.clone()).or_insert(0u32);
                    *attempts += 1;
                    let queue = db.queue_row(&item_id)?;
                    let total_attempts = queue.map(|row| row.attempts).unwrap_or(*attempts);
                    if total_attempts >= config.retry_max {
                        warn!(item = %item_id, error = %reason, "download exhausted retries");
                    }
                    let retry_at = if total_attempts >= config.retry_max {
                        i64::MAX
                    } else {
                        now() + crate::ingest::backoff_delay(*attempts, None)
                    };
                    db.retry_download(&item_id, &reason, retry_at)?;
                    report.failed.push((item_id, reason));
                }
            }
        }
        if db.pending_item_ids()?.is_empty() {
            break;
        }
    }
    Ok(report)
}

async fn process_item(
    db: &Db,
    adapter: &dyn SourceAdapter,
    transport: &dyn ImageTransport,
    item: &ItemRow,
    config: &DownloadConfig,
) -> Result<()> {
    let image = match find_work(db, adapter, item).await {
        Ok(work) => adapter.resolve_image(&work, item.page).await,
        Err(error) => Err(SourceError::internal(format!("{error:#}"))),
    };
    let image = match image {
        Ok(image) => image,
        Err(error) => {
            handle_source_error(db, item, &error).await?;
            return Err(anyhow::Error::new(error));
        }
    };
    download_item(db, transport, item, &image, config).await
}

pub async fn download_item(
    db: &Db,
    transport: &dyn ImageTransport,
    item: &ItemRow,
    image: &ImageRef,
    config: &DownloadConfig,
) -> Result<()> {
    // Keep the semaphore permit outside SQLite write transactions.
    let originals = config.media_dir.join("originals");
    let thumbs = config.media_dir.join("thumbs");
    let parts = config.media_dir.join("parts");
    std::fs::create_dir_all(&originals)?;
    std::fs::create_dir_all(&thumbs)?;
    std::fs::create_dir_all(&parts)?;

    let ext = image
        .ext
        .clone()
        .or_else(|| item.ext.clone())
        .unwrap_or_else(|| "jpg".to_string());
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp") {
        bail!("unsupported image extension: {ext}");
    }
    let final_path = originals.join(format!("{}.{}", item.id, ext));
    if final_path.exists() {
        let bytes = std::fs::read(&final_path)?;
        return finalize_item(db, item, config, &final_path, &ext, bytes);
    }

    let part_path = parts.join(format!("{}.{}.part", item.id, ext));
    let resume_from = std::fs::metadata(&part_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let response = transport
        .fetch(&image.url, (resume_from > 0).then_some(resume_from))
        .await?;
    let content_type = response.content_type.as_deref().unwrap_or_default();
    if response.status == 404 {
        return Err(SourceError::not_found("image returned 404").into());
    }
    if response.status == 429 {
        return Err(SourceError::rate_limited(None).into());
    }
    if response.status == 401 || response.status == 403 {
        return Err(SourceError::auth("image authentication failed").into());
    }
    if !(200..300).contains(&response.status) && response.status != 206 {
        return Err(anyhow::anyhow!(
            "unexpected image status {}",
            response.status
        ));
    }
    let is_html = content_type.contains("text/html")
        || response
            .body
            .iter()
            .take(16)
            .copied()
            .collect::<Vec<_>>()
            .starts_with(b"<!DOCTYPE html");
    if is_html {
        bail!("server returned HTML instead of an image");
    }
    if let Some(expected) = image.expected_bytes
        && response.status == 200
        && response.body.len() as u64 != expected
    {
        bail!(
            "image byte count mismatch: expected {expected}, got {}",
            response.body.len()
        );
    }

    let mut part = if response.status == 206 {
        std::fs::OpenOptions::new().append(true).open(&part_path)?
    } else {
        let file = std::fs::File::create(&part_path)?;
        file.set_len(0)?;
        file
    };
    part.write_all(&response.body)?;
    part.flush()?;
    drop(part);

    let actual_total = std::fs::metadata(&part_path)?.len();
    let expected_total = if response.status == 206 {
        response.content_range.as_deref().and_then(|range| {
            range
                .rsplit('/')
                .next()
                .and_then(|value| value.trim().parse::<u64>().ok())
        })
    } else if let Some(content_range) = &response.content_range {
        content_range
            .rsplit('/')
            .next()
            .and_then(|value| value.trim().parse::<u64>().ok())
    } else {
        response.content_length
    };
    if let Some(total) = expected_total
        && actual_total != total
    {
        bail!("incomplete download: expected {total} bytes, got {actual_total}");
    }
    let bytes = std::fs::read(&part_path)?;
    let decoder = ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()?
        .decode()
        .context("downloaded bytes are not a valid image")?;
    drop(decoder);

    std::fs::rename(&part_path, &final_path).with_context(|| {
        format!(
            "renaming {} to {}",
            part_path.display(),
            final_path.display()
        )
    })?;
    finalize_item(db, item, config, &final_path, &ext, bytes)
}

fn finalize_item(
    db: &Db,
    item: &ItemRow,
    config: &DownloadConfig,
    final_path: &std::path::Path,
    ext: &str,
    bytes: Vec<u8>,
) -> Result<()> {
    let decoder = ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()?
        .decode()
        .context("stored image cannot be decoded")?;
    let (width, height) = (decoder.width(), decoder.height());
    let thumb_256 = write_thumbnail(&config.media_dir, item, &decoder, 256)?;
    let thumb_512 = write_thumbnail(&config.media_dir, item, &decoder, 512)?;
    let sha1 = Sha1::digest(&bytes);
    let sha1 = sha1
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    db.set_download_result(
        &item.id,
        &DownloadResult {
            path: final_path.to_string_lossy().into_owned(),
            ext: ext.to_string(),
            bytes: bytes.len() as u64,
            sha1,
            width,
            height,
        },
    )?;
    db.set_thumbnails(&item.id, &thumb_256, &thumb_512)?;
    info!(item = %item.id, width, height, bytes = bytes.len(), "download completed");
    Ok(())
}

fn write_thumbnail(
    media_dir: &std::path::Path,
    item: &ItemRow,
    image: &image::DynamicImage,
    size: u32,
) -> Result<String> {
    let tier = format!("t{size}");
    let dir = media_dir.join("thumbs").join(&tier);
    std::fs::create_dir_all(&dir)?;
    // 网页按 `/media/{tier}/<id>.jpg` 提供缩略图；这里直接生成同名 JPEG。
    let path = dir.join(format!("{}.jpg", item.id));
    let thumbnail = image.thumbnail(size, size);
    thumbnail.save_with_format(&path, ImageFormat::Jpeg)?;
    Ok(path.to_string_lossy().into_owned())
}

async fn handle_source_error(db: &Db, item: &ItemRow, error: &SourceError) -> Result<()> {
    match error.kind {
        SourceErrorKind::NotFound => {
            db.mark_work_deleted(&item.work_id)?;
        }
        SourceErrorKind::RateLimited | SourceErrorKind::Auth => {
            let retry_at = now() + error.retry_after.map_or(60, |delay| delay.as_secs() as i64);
            db.pause_downloads(Some(retry_at), &error.message)?;
        }
        SourceErrorKind::Network | SourceErrorKind::Internal => {}
    }
    Ok(())
}

async fn find_work(db: &Db, adapter: &dyn SourceAdapter, item: &ItemRow) -> Result<SourceWork> {
    let external_id = db
        .work_external_id(&item.work_id)?
        .ok_or_else(|| anyhow::anyhow!("work {} missing for item {}", item.work_id, item.id))?;
    Ok(adapter.fetch_work(&external_id).await?)
}
