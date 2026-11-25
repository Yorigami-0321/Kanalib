//! 下载队列：`.part`、Range 续传、成功才 rename、部分成功与重启恢复。

use std::{io::Write, path::PathBuf, pin::Pin};

use anyhow::{Context, Result, bail};
use futures::StreamExt;
use image::{
    ExtendedColorType, ImageEncoder, ImageReader,
    codecs::{avif::AvifEncoder, jpeg::JpegEncoder},
    imageops::FilterType,
};
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

fn thumbnail_pair_ready(media_dir: &std::path::Path, item_id: &str, size: u32) -> bool {
    let dir = media_dir.join("thumbs").join(format!("t{size}"));
    ["jpg", "avif"]
        .iter()
        .all(|extension| dir.join(format!("{item_id}.{extension}")).is_file())
        && dir.join(format!(".{item_id}.ready")).is_file()
}

fn mark_thumbnail_ready(media_dir: &std::path::Path, item_id: &str, size: u32) -> Result<()> {
    let dir = media_dir.join("thumbs").join(format!("t{size}"));
    if !["jpg", "avif"]
        .iter()
        .all(|extension| dir.join(format!("{item_id}.{extension}")).is_file())
    {
        return Ok(());
    }
    std::fs::write(dir.join(format!(".{item_id}.ready")), b"ready")?;
    Ok(())
}

pub fn backfill_thumbnails(db: &Db, media_dir: &std::path::Path) -> Result<usize> {
    let mut statement = db.connection().prepare(
        "SELECT id, original_path FROM items
          WHERE original_path IS NOT NULL AND decision='allow'
          ORDER BY id",
    )?;
    let items = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    let mut refreshed = 0;
    for (item_id, original_path) in items {
        let complete = [256, 512]
            .iter()
            .all(|size| thumbnail_pair_ready(media_dir, &item_id, *size));
        if complete {
            continue;
        }
        let bytes = match std::fs::read(&original_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                warn!(item = %item_id, path = %original_path, %error, "thumbnail backfill source missing");
                continue;
            }
        };
        let decoded = match ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(anyhow::Error::from)
            .and_then(|reader| reader.decode().map_err(anyhow::Error::from))
        {
            Ok(decoded) => decoded,
            Err(error) => {
                warn!(item = %item_id, %error, "thumbnail backfill decode failed");
                continue;
            }
        };
        let thumb_256 = match write_thumbnail(media_dir, &item_id, &decoded, 256) {
            Ok(path) => path,
            Err(error) => {
                warn!(item = %item_id, %error, "t256 thumbnail backfill failed");
                continue;
            }
        };
        let thumb_512 = match write_thumbnail(media_dir, &item_id, &decoded, 512) {
            Ok(path) => path,
            Err(error) => {
                warn!(item = %item_id, %error, "t512 thumbnail backfill failed");
                continue;
            }
        };
        db.connection().execute(
            "UPDATE items SET thumb_256=?2, thumb_512=?3, revision=revision+1, updated_at=?4
              WHERE id=?1 AND decision='allow'",
            rusqlite::params![item_id, thumb_256, thumb_512, now()],
        )?;
        for size in [256, 512] {
            if let Err(error) = mark_thumbnail_ready(media_dir, &item_id, size) {
                warn!(item = %item_id, size, %error, "thumbnail ready marker failed");
            }
        }
        refreshed += 1;
        info!(item = %item_id, "thumbnails refreshed");
    }
    Ok(refreshed)
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
    let thumb_256 = write_thumbnail(&config.media_dir, &item.id, &decoder, 256)?;
    let thumb_512 = write_thumbnail(&config.media_dir, &item.id, &decoder, 512)?;
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
    for size in [256, 512] {
        if let Err(error) = mark_thumbnail_ready(&config.media_dir, &item.id, size) {
            warn!(item = %item.id, size, %error, "thumbnail ready marker failed");
        }
    }
    info!(item = %item.id, width, height, bytes = bytes.len(), "download completed");
    Ok(())
}

const THUMBNAIL_JPEG_QUALITY: u8 = 90;
const THUMBNAIL_AVIF_QUALITY: u8 = 90;
const THUMBNAIL_AVIF_SPEED: u8 = 8;

fn write_thumbnail(
    media_dir: &std::path::Path,
    item_id: &str,
    image: &image::DynamicImage,
    size: u32,
) -> Result<String> {
    let tier = format!("t{size}");
    let dir = media_dir.join("thumbs").join(&tier);
    std::fs::create_dir_all(&dir)?;
    let thumbnail = if image.width() <= size && image.height() <= size {
        image.clone()
    } else {
        image.resize(size, size, FilterType::Lanczos3)
    };

    let jpeg_path = dir.join(format!("{item_id}.jpg"));
    let rgb = thumbnail.to_rgb8();
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, THUMBNAIL_JPEG_QUALITY).encode_image(&rgb)?;
    std::fs::write(&jpeg_path, jpeg)?;

    let avif_path = dir.join(format!("{item_id}.avif"));
    match encode_avif(&thumbnail) {
        Ok(bytes) => {
            if let Err(error) = std::fs::write(&avif_path, bytes) {
                warn!(%error, item = %item_id, tier, "AVIF thumbnail write failed; keeping JPEG");
            }
        }
        Err(error) => warn!(%error, item = %item_id, tier, "AVIF thumbnail failed; keeping JPEG"),
    }
    Ok(jpeg_path.to_string_lossy().into_owned())
}

fn encode_avif(image: &image::DynamicImage) -> Result<Vec<u8>> {
    let rgba = image.to_rgba8();
    let mut bytes = Vec::new();
    AvifEncoder::new_with_speed_quality(&mut bytes, THUMBNAIL_AVIF_SPEED, THUMBNAIL_AVIF_QUALITY)
        .write_image(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
        ExtendedColorType::Rgba8,
    )?;
    Ok(bytes)
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;
    use tempfile::TempDir;

    #[test]
    fn thumbnail_writes_avif_and_high_quality_jpeg_fallback() {
        let dir = TempDir::new().unwrap();
        let source = image::DynamicImage::ImageRgba8(RgbaImage::from_fn(1024, 512, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255])
        }));

        let jpeg_path =
            write_thumbnail(dir.path(), "a".repeat(40).as_str(), &source, 256).expect("thumbnail");
        let jpeg_path = std::path::Path::new(&jpeg_path);
        let avif_path = jpeg_path.with_extension("avif");
        let jpeg = std::fs::read(jpeg_path).expect("jpeg");
        let avif = std::fs::read(&avif_path).expect("avif");

        assert!(jpeg.starts_with(&[0xFF, 0xD8, 0xFF]));
        assert!(avif.len() > 12 && &avif[4..8] == b"ftyp");
        assert!(avif.windows(4).any(|brand| brand == b"avif"));
        let decoded = ImageReader::new(std::io::Cursor::new(jpeg))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((decoded.width(), decoded.height()), (256, 128));

        let ispe = avif.windows(4).position(|value| value == b"ispe").unwrap();
        let width = u32::from_be_bytes(avif[ispe + 8..ispe + 12].try_into().unwrap());
        let height = u32::from_be_bytes(avif[ispe + 12..ispe + 16].try_into().unwrap());
        assert_eq!((width, height), (256, 128));
    }

    #[test]
    fn thumbnail_does_not_upscale_small_sources() {
        let dir = TempDir::new().unwrap();
        let source = image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            32,
            16,
            image::Rgba([1, 2, 3, 255]),
        ));
        let jpeg_path =
            write_thumbnail(dir.path(), "b".repeat(40).as_str(), &source, 256).expect("thumbnail");
        let decoded = ImageReader::new(std::io::Cursor::new(std::fs::read(jpeg_path).unwrap()))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((decoded.width(), decoded.height()), (32, 16));
    }
}
