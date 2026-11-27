//! 桌面端原图的缩略图生成：与 box 入库保持同样的档位与画质。
//!
//! 只写 JPEG；网页端按 `Accept` 协商，没有 AVIF 时自动回退到 JPEG。

use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, bail};
use image::{ImageFormat, ImageReader, Limits, codecs::jpeg::JpegEncoder, imageops::FilterType};
use tokio::sync::Semaphore;

const JPEG_QUALITY: u8 = 90;
/// 2c2g 云主机：同时最多解码/缩放两张，避免把 web 拖进 OOM。
const MAX_CONCURRENCY: usize = 2;

static PERMITS: OnceLock<Semaphore> = OnceLock::new();
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
pub struct ThumbOutcome {
    pub width: u32,
    pub height: u32,
}

/// 解码 `source` 并按 256/512/1024/2048 档写 `media_root/t{size}/{id}.jpg`。
pub async fn generate(
    media_root: PathBuf,
    id: String,
    ext: String,
    source: PathBuf,
) -> Result<ThumbOutcome> {
    let permits = PERMITS.get_or_init(|| Semaphore::new(MAX_CONCURRENCY));
    let _permit = permits.acquire().await?;
    tokio::task::spawn_blocking(move || generate_blocking(&media_root, &id, &ext, &source)).await?
}

fn generate_blocking(
    media_root: &Path,
    id: &str,
    ext: &str,
    source: &Path,
) -> Result<ThumbOutcome> {
    let bytes = std::fs::read(source).context("read uploaded image")?;
    let mut reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .context("guess uploaded image format")?;
    let Some(format) = reader.format() else {
        bail!("uploaded bytes are not a supported image");
    };
    if !extension_matches(ext, format) {
        bail!("file extension does not match image format");
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    reader.limits(limits);
    let image = reader.decode().context("decode uploaded image")?;

    let (width, height) = (image.width(), image.height());
    for size in thumbnail_sizes(width, height) {
        let thumbnail = if width <= size && height <= size {
            image.clone()
        } else {
            image.resize(size, size, FilterType::Lanczos3)
        };
        let rgb = thumbnail.to_rgb8();
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
            .encode_image(&rgb)
            .with_context(|| format!("encode t{size} thumbnail"))?;
        write_atomic(
            &media_root
                .join(format!("t{size}"))
                .join(format!("{id}.jpg")),
            &jpeg,
        )?;
    }
    Ok(ThumbOutcome { width, height })
}

fn thumbnail_sizes(width: u32, height: u32) -> Vec<u32> {
    let side = width.max(height);
    let mut sizes = vec![256, 512];
    if side >= 1024 {
        sizes.push(1024);
    }
    if side >= 2048 {
        sizes.push(2048);
    }
    sizes
}

fn extension_matches(ext: &str, format: ImageFormat) -> bool {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => format == ImageFormat::Jpeg,
        "png" => format == ImageFormat::Png,
        "webp" => format == ImageFormat::WebP,
        "gif" => format == ImageFormat::Gif,
        _ => false,
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("thumbnail directory")?;
    std::fs::create_dir_all(dir)?;
    let file = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("thumbnail file name")?;
    let sequence = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!(".{file}.{}-{sequence}.tmp", std::process::id()));
    std::fs::write(&temp, bytes)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    Ok(())
}

/// 原图服务用：扩展名到 Content-Type。
pub(crate) fn content_type_for(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "avif" => Some("image/avif"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        _ => None,
    }
}
