use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};
use exif::{In, Tag};
use image::{imageops::FilterType, DynamicImage};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::model::is_ffmpeg_only;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExifData {
    #[serde(default)]
    pub camera: String,
    #[serde(default)]
    pub lens: String,
    #[serde(default)]
    pub iso: String,
    #[serde(default)]
    pub aperture: String,
    #[serde(default)]
    pub shutter: String,
    #[serde(default)]
    pub focal_length: String,
    #[serde(default = "default_orientation")]
    pub orientation: u8,
}

fn default_orientation() -> u8 {
    1
}

impl Default for ExifData {
    fn default() -> Self {
        Self {
            camera: String::new(),
            lens: String::new(),
            iso: String::new(),
            aperture: String::new(),
            shutter: String::new(),
            focal_length: String::new(),
            orientation: 1,
        }
    }
}

impl ExifData {
    pub fn is_empty(&self) -> bool {
        self.camera.is_empty()
            && self.lens.is_empty()
            && self.iso.is_empty()
            && self.aperture.is_empty()
            && self.shutter.is_empty()
            && self.focal_length.is_empty()
    }
}

fn exif_field(exif: &exif::Exif, tag: Tag) -> Option<&exif::Field> {
    exif.get_field(tag, In::PRIMARY)
        .or_else(|| exif.fields().find(|field| field.tag == tag))
}

fn exif_text(exif: &exif::Exif, tag: Tag) -> Option<String> {
    let field = exif_field(exif, tag)?;
    let text = field.display_value().to_string();
    let text = text.trim().trim_matches('"').trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn exif_ascii(exif: &exif::Exif, tag: Tag) -> Option<String> {
    let field = exif_field(exif, tag)?;
    let exif::Value::Ascii(values) = &field.value else {
        return exif_text(exif, tag);
    };
    let bytes = values.first()?;
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_matches('\0').trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn parse_offset(text: &str) -> Option<i64> {
    let text = text.trim();
    let (sign, rest) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    let digits = rest.replace(':', "");
    let hours = digits.get(0..2)?.parse::<i64>().ok()?;
    let minutes = digits.get(2..4).unwrap_or("00").parse::<i64>().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
}

pub fn parse_exif_datetime(text: &str, offset: Option<&str>) -> Option<u64> {
    let text = text.trim();
    let (date, time) = text.split_once(' ')?;
    let mut date = date.split(':');
    let year = date.next()?.parse::<i64>().ok()?;
    let month = date.next()?.parse::<i64>().ok()?;
    let day = date.next()?.parse::<i64>().ok()?;
    let mut time = time.split(':');
    let hour = time.next()?.parse::<i64>().ok()?;
    let minute = time.next()?.parse::<i64>().ok()?;
    let second = time
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let days = days_from_civil(year, month, day);
    let mut seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    if let Some(offset) = offset.and_then(parse_offset) {
        seconds -= offset;
    }
    Some(seconds.max(0) as u64)
}

/// 尽力读取 EXIF；不支持的格式或损坏文件返回默认值，不影响入库。
pub fn read_exif(path: &Path) -> (ExifData, Option<u64>) {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return (ExifData::default(), None),
    };
    let exif = match exif::Reader::new().read_from_container(&mut BufReader::new(file)) {
        Ok(exif) => exif,
        Err(_) => return (ExifData::default(), None),
    };
    let orientation = exif_field(&exif, Tag::Orientation)
        .and_then(|field| field.value.get_uint(0))
        .map(|value| value.clamp(1, 8) as u8)
        .unwrap_or(1);
    let make = exif_ascii(&exif, Tag::Make).unwrap_or_default();
    let model = exif_ascii(&exif, Tag::Model).unwrap_or_default();
    let camera = match (make.trim(), model.trim()) {
        (make, model) if make.is_empty() => model.to_string(),
        (make, model) if model.is_empty() || model.starts_with(make) => make.to_string(),
        (make, model) => format!("{make} {model}"),
    };
    let date = exif_ascii(&exif, Tag::DateTimeOriginal)
        .or_else(|| exif_ascii(&exif, Tag::DateTimeDigitized))
        .or_else(|| exif_ascii(&exif, Tag::DateTime));
    let offset = exif_ascii(&exif, Tag::OffsetTimeOriginal);
    let captured_at = date
        .as_deref()
        .and_then(|value| parse_exif_datetime(value, offset.as_deref()));
    (
        ExifData {
            camera,
            lens: exif_ascii(&exif, Tag::LensModel).unwrap_or_default(),
            iso: exif_text(&exif, Tag::PhotographicSensitivity).unwrap_or_default(),
            aperture: exif_text(&exif, Tag::FNumber).unwrap_or_default(),
            shutter: exif_text(&exif, Tag::ExposureTime).unwrap_or_default(),
            focal_length: exif_text(&exif, Tag::FocalLength).unwrap_or_default(),
            orientation,
        },
        captured_at,
    )
}

#[derive(Clone, Debug, Default)]
pub struct ContentHashes {
    pub sha1: String,
    pub blake3: String,
    pub phash: u64,
    /// 图像能否被完整解码；文件哈希与损坏状态互不影响。
    pub decode_ok: bool,
    pub decode_error: Option<String>,
}

pub fn hash_file(path: &Path) -> Result<ContentHashes> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut sha1 = Sha1::new();
    let mut blake3 = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        sha1.update(&buffer[..read]);
        blake3.update(&buffer[..read]);
    }
    let decoded = image::ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(image::ImageError::IoError)
        .and_then(|reader| reader.decode());
    let ffmpeg_only = is_ffmpeg_only(path);
    let (phash, decode_ok, decode_error) = match decoded {
        Ok(image) => (perceptual_hash(&image), true, None),
        // `image` 0.25 has no AVIF decoder unless the native dav1d backend is
        // enabled. Thumbnails still come from ffmpeg, so a recognised AVIF
        // must not be reported as a damaged file just because phash is absent.
        Err(error) if ffmpeg_only && matches!(&error, image::ImageError::Unsupported(_)) => {
            if crate::ffmpeg::available() {
                log::info!(
                    "decode {} skipped: ffmpeg-only format, hashes kept, phash skipped",
                    path.display()
                );
                (0, false, None)
            } else {
                log::warn!(
                    "decode {} needs ffmpeg (not available); hashes kept, phash skipped",
                    path.display()
                );
                (0, false, Some(format!("需要 FFmpeg 解码 {}", ffmpeg_only_label(path))))
            }
        }
        Err(error) => {
            let label = classify_decode_error(&error);
            log::warn!(
                "decode {} failed ({label}): {error}; hashes kept, phash skipped",
                path.display()
            );
            (0, false, Some(label.to_string()))
        }
    };
    Ok(ContentHashes {
        sha1: format!("{:x}", sha1.finalize()),
        blake3: blake3.finalize().to_hex().to_string(),
        phash,
        decode_ok,
        decode_error,
    })
}

fn classify_decode_error(error: &image::ImageError) -> &'static str {
    let text = error.to_string().to_lowercase();
    if text.contains("unexpected end of file")
        || text.contains("truncated")
        || text.contains("eof")
        || text.contains("not enough bytes")
    {
        "文件不完整（图片数据被截断）"
    } else if text.contains("unsupported") || text.contains("unknown format") {
        "不支持的图片编码"
    } else {
        "图片解码失败，文件可能已损坏"
    }
}

fn ffmpeg_only_label(path: &Path) -> String {
    path.extension()
        .map(|value| value.to_string_lossy().to_uppercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "该格式".to_string())
}

/// 原始尺寸回退：`image` 没有解码器、但 ffmpeg 能生成缩略图的格式。
/// AVIF 从 ISO-BMFF 的 `meta/iprp/ipco/ispe` 读取主图尺寸，PSD 读 26 字节文件头，
/// 避免把 512 缩略图的尺寸误存成原图尺寸。
pub fn fallback_dimensions(path: &Path) -> Option<(u32, u32)> {
    if !is_ffmpeg_only(path) {
        return None;
    }
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("psd"))
    {
        return psd_dimensions(path);
    }
    iso_bmff_dimensions(path)
}

/// PSD 文件头：`8BPS` + 版本 1 + 通道数 + 高/宽（大端）+ 位深 + 颜色模式。
/// 只读取固定 26 字节，不解析图层/图像数据，也不为损坏文件分配大块内存。
fn psd_dimensions(path: &Path) -> Option<(u32, u32)> {
    let mut file = File::open(path).ok()?;
    let mut header = [0u8; 26];
    file.read_exact(&mut header).ok()?;
    if &header[0..4] != b"8BPS" || u16::from_be_bytes([header[4], header[5]]) != 1 {
        return None;
    }
    let channels = u16::from_be_bytes([header[12], header[13]]);
    let height = u32::from_be_bytes(header[14..18].try_into().ok()?);
    let width = u32::from_be_bytes(header[18..22].try_into().ok()?);
    let depth = u16::from_be_bytes([header[22], header[23]]);
    let mode = u16::from_be_bytes([header[24], header[25]]);
    let plausible = (1..=56).contains(&channels)
        && (1..=300_000).contains(&width)
        && (1..=300_000).contains(&height)
        && matches!(depth, 1 | 8 | 16 | 32)
        && mode <= 9;
    plausible.then_some((width, height))
}

fn iso_bmff_dimensions(path: &Path) -> Option<(u32, u32)> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut offset = 0u64;
    while offset.saturating_add(8) <= len {
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header).ok()?;
        let mut size = u32::from_be_bytes(header[0..4].try_into().ok()?) as u64;
        let mut header_len = 8u64;
        if size == 1 {
            let mut large = [0u8; 8];
            file.read_exact(&mut large).ok()?;
            size = u64::from_be_bytes(large);
            header_len = 16;
        } else if size == 0 {
            size = len - offset;
        }
        if size < header_len || offset.saturating_add(size) > len {
            break;
        }
        if &header[4..8] == b"meta" {
            let body_len = (size - header_len) as usize;
            if body_len > 16 * 1024 * 1024 {
                return None;
            }
            let mut body = vec![0u8; body_len];
            file.read_exact(&mut body).ok()?;
            return ispe_dimensions(&body);
        }
        offset = offset.saturating_add(size);
    }
    None
}

fn ispe_dimensions(body: &[u8]) -> Option<(u32, u32)> {
    let mut best: Option<(u32, u32)> = None;
    for index in 0..body.len().saturating_sub(15) {
        if &body[index..index + 4] != b"ispe" {
            continue;
        }
        let width = u32::from_be_bytes(body[index + 8..index + 12].try_into().ok()?);
        let height = u32::from_be_bytes(body[index + 12..index + 16].try_into().ok()?);
        if width == 0 || height == 0 || width > 1_000_000 || height > 1_000_000 {
            continue;
        }
        let area = width as u64 * height as u64;
        if best.is_none_or(|(w, h)| area > w as u64 * h as u64) {
            best = Some((width, height));
        }
    }
    best
}

pub fn perceptual_hash(image: &DynamicImage) -> u64 {
    let small = image
        .resize_exact(32, 32, FilterType::Triangle)
        .to_luma8();
    let mut pixels = vec![0f32; 32 * 32];
    for (index, pixel) in small.pixels().enumerate() {
        pixels[index] = pixel[0] as f32;
    }
    let coefficients = dct_2d(&pixels, 32);
    let mut low = Vec::with_capacity(64);
    for y in 0..8 {
        for x in 0..8 {
            low.push(coefficients[y * 32 + x]);
        }
    }
    let mut sorted = low.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = (sorted[31] + sorted[32]) / 2.0;
    let mut hash = 0u64;
    for (index, coefficient) in low.iter().enumerate() {
        if *coefficient > median {
            hash |= 1 << (63 - index);
        }
    }
    hash
}

fn dct_2d(input: &[f32], size: usize) -> Vec<f32> {
    let alpha = |frequency: usize| {
        if frequency == 0 {
            (1.0 / size as f32).sqrt()
        } else {
            (2.0 / size as f32).sqrt()
        }
    };
    let cos = |position: usize, frequency: usize| {
        (std::f32::consts::PI * (2 * position + 1) as f32 * frequency as f32
            / (2 * size) as f32)
            .cos()
    };
    let mut rows = vec![0f32; size * size];
    for y in 0..size {
        for u in 0..size {
            let mut sum = 0f32;
            for x in 0..size {
                sum += input[y * size + x] * cos(x, u);
            }
            rows[y * size + u] = alpha(u) * sum;
        }
    }
    let mut output = vec![0f32; size * size];
    for v in 0..size {
        for u in 0..size {
            let mut sum = 0f32;
            for y in 0..size {
                sum += rows[y * size + u] * cos(y, v);
            }
            output[v * size + u] = alpha(v) * sum;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{fallback_dimensions, hash_file, perceptual_hash};
    use image::{DynamicImage, GrayImage, Luma};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kanalib-media-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn perceptual_hash_is_stable_and_discriminating() {
        let gradient = GrayImage::from_fn(64, 64, |x, y| Luma([((x + y) * 2) as u8]));
        let inverted = GrayImage::from_fn(64, 64, |x, y| Luma([255 - ((x + y) * 2) as u8]));
        let a = perceptual_hash(&DynamicImage::ImageLuma8(gradient.clone()));
        let b = perceptual_hash(&DynamicImage::ImageLuma8(gradient));
        let c = perceptual_hash(&DynamicImage::ImageLuma8(inverted));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn exif_datetime_parses_offsets() {
        assert_eq!(
            super::parse_exif_datetime("2026:10:15 12:00:00", None),
            Some(1_792_065_600)
        );
        assert_eq!(
            super::parse_exif_datetime("2026:10:15 12:00:00", Some("+08:00")),
            Some(1_792_036_800)
        );
        assert_eq!(
            super::parse_exif_datetime("2026:10:15 12:00:00", Some("-05:00")),
            Some(1_792_083_600)
        );
        assert_eq!(super::parse_exif_datetime("not a date", None), None);
    }

    #[test]
    fn exif_defaults_deserialize_from_empty_metadata() {
        let data: super::ExifData = serde_json::from_str("{}").unwrap();
        assert_eq!(data.orientation, 1);
        assert!(data.is_empty());
    }

    #[test]
    fn truncated_image_keeps_content_hashes() {
        let dir = std::env::temp_dir().join(format!(
            "kanalib-truncated-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.jpg");
        let image = image::RgbImage::from_fn(128, 128, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, 80])
        });
        image.save(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..20.min(bytes.len())]).unwrap();

        let hashes = super::hash_file(&path).unwrap();
        assert!(!hashes.decode_ok);
        assert!(hashes.decode_error.is_some());
        assert_eq!(hashes.sha1.len(), 40);
        assert_eq!(hashes.blake3.len(), 64);
        assert_eq!(hashes.phash, 0);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn new_image_formats_decode_for_hashes() {
        let dir = temp_dir("formats");
        let tiff = dir.join("sample.tiff");
        let ico = dir.join("sample.ico");
        let qoi = dir.join("sample.qoi");
        let exr = dir.join("sample.exr");
        image::RgbImage::from_fn(48, 32, |x, y| {
            image::Rgb([(x * 5) as u8, (y * 7) as u8, 90])
        })
        .save(&tiff)
        .unwrap();
        image::RgbaImage::from_fn(48, 32, |x, y| {
            image::Rgba([(x * 5) as u8, (y * 7) as u8, 90, 255])
        })
        .save(&ico)
        .unwrap();
        image::RgbaImage::from_fn(48, 32, |x, y| {
            image::Rgba([(x * 5) as u8, (y * 7) as u8, 90, 200])
        })
        .save(&qoi)
        .unwrap();
        image::Rgb32FImage::from_fn(48, 32, |x, y| {
            image::Rgb([x as f32 / 48.0, y as f32 / 32.0, 0.5])
        })
        .save(&exr)
        .unwrap();

        for path in [&tiff, &ico, &qoi, &exr] {
            let hashes = hash_file(path).unwrap();
            assert!(hashes.decode_ok, "{} should decode", path.display());
            assert!(hashes.decode_error.is_none(), "{}", path.display());
            assert_ne!(hashes.phash, 0, "{}", path.display());
            assert_eq!(hashes.sha1.len(), 40);
            assert_eq!(hashes.blake3.len(), 64);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn truncated_new_format_keeps_content_hashes() {
        let dir = temp_dir("truncated-tiff");
        let path = dir.join("bad.tiff");
        image::RgbImage::from_pixel(64, 64, image::Rgb([20, 40, 80]))
            .save(&path)
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..32.min(bytes.len())]).unwrap();

        let hashes = hash_file(&path).unwrap();
        assert!(!hashes.decode_ok);
        assert!(hashes.decode_error.is_some());
        assert_eq!(hashes.sha1.len(), 40);
        assert_eq!(hashes.blake3.len(), 64);
        assert_eq!(hashes.phash, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 最小合法 PSD：26 字节文件头 + 三个空区块 + 未压缩平面数据。
    /// 测试里不需要图层/图像资源；`planes` 为空时等价于只有文件头的截断样本。
    fn write_psd(
        path: &Path,
        width: u32,
        height: u32,
        channels: u16,
        mode: u16,
        depth: u16,
        planes: &[Vec<u8>],
    ) {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"8BPS");
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&[0u8; 6]);
        bytes.extend_from_slice(&channels.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&depth.to_be_bytes());
        bytes.extend_from_slice(&mode.to_be_bytes());
        for _ in 0..3 {
            bytes.extend_from_slice(&0u32.to_be_bytes());
        }
        bytes.extend_from_slice(&0u16.to_be_bytes());
        for plane in planes {
            bytes.extend_from_slice(plane);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn psd_dimensions_read_header_and_reject_invalid() {
        let dir = temp_dir("psd-header");
        let path = dir
            .join("图库 测试")
            .join("a".repeat(60))
            .join("写真 サンプル.psd");
        write_psd(&path, 640, 480, 3, 3, 8, &[]);
        assert_eq!(fallback_dimensions(&path), Some((640, 480)));

        let truncated = dir.join("bad.psd");
        std::fs::write(&truncated, b"8BPS\x00\x01").unwrap();
        assert_eq!(fallback_dimensions(&truncated), None);

        let oversized = dir.join("huge.psd");
        write_psd(&oversized, 400_000, 10, 3, 3, 8, &[]);
        assert_eq!(fallback_dimensions(&oversized), None);

        assert_eq!(fallback_dimensions(&dir.join("sample.png")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn psd_hash_keeps_content_hashes_without_image_decoder() {
        let dir = temp_dir("psd-hash");
        let path = dir.join("sample.psd");
        write_psd(&path, 96, 64, 3, 3, 8, &[]);

        let hashes = hash_file(&path).unwrap();
        assert!(!hashes.decode_ok);
        assert_eq!(hashes.phash, 0);
        assert_eq!(hashes.sha1.len(), 40);
        assert_eq!(hashes.blake3.len(), 64);
        match hashes.decode_error {
            Some(message) => {
                assert!(message.contains("PSD"), "unexpected message: {message}");
                assert!(!message.contains("不完整"), "PSD is not a damaged file: {message}");
            }
            None => assert!(crate::ffmpeg::available()),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8 + payload.len());
        bytes.extend(((8 + payload.len()) as u32).to_be_bytes());
        bytes.extend(kind);
        bytes.extend(payload);
        bytes
    }

    #[test]
    fn avif_dimensions_read_ispe_box() {
        let dir = temp_dir("avif-header");
        let path = dir.join("sample.avif");
        let ispe = boxed(b"ispe", &[0, 0, 0, 0, 0, 0, 2, 128, 0, 0, 1, 104]);
        let meta = boxed(
            b"meta",
            &[[0u8, 0, 0, 0].as_slice(), ispe.as_slice()].concat(),
        );
        let ftyp = boxed(b"ftyp", b"avif\0\0\0\0avif");
        std::fs::write(&path, [ftyp, meta].concat()).unwrap();

        assert_eq!(fallback_dimensions(&path), Some((640, 360)));
        assert_eq!(fallback_dimensions(&dir.join("sample.png")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[ignore = "requires ffmpeg with AVIF support (for example KANALIB_FFMPEG=runtime/ffmpeg/bin/ffmpeg.exe)"]
    fn avif_ffmpeg_fallback_keeps_hashes_without_damage() {
        let Some(ffmpeg) = crate::ffmpeg::status() else {
            return;
        };
        let dir = temp_dir("avif");
        let png = dir.join("sample.png");
        let avif = dir.join("sample.avif");
        image::RgbImage::from_fn(96, 64, |x, y| {
            image::Rgb([(x * 2) as u8, (y * 3) as u8, 60])
        })
        .save(&png)
        .unwrap();
        let status = std::process::Command::new(&ffmpeg.path)
            .args(["-hide_banner", "-nostdin", "-v", "error", "-y", "-i"])
            .arg(&png)
            .args(["-frames:v", "1"])
            .arg(&avif)
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg failed to create the AVIF sample");

        assert!(crate::model::is_image(&avif));
        assert_eq!(fallback_dimensions(&avif), Some((96, 64)));
        let hashes = hash_file(&avif).unwrap();
        assert_eq!(hashes.sha1.len(), 40);
        assert_eq!(hashes.blake3.len(), 64);
        if hashes.decode_ok {
            assert_ne!(hashes.phash, 0);
        } else {
            // This build has no AVIF decoder in `image`; with ffmpeg present
            // the file must not be marked as damaged or unsupported.
            assert!(hashes.decode_error.is_none());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
