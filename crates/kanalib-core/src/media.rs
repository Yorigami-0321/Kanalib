use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result};
use image::{imageops::FilterType, DynamicImage};
use sha1::{Digest, Sha1};

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
    let (phash, decode_ok, decode_error) = match decoded {
        Ok(image) => (perceptual_hash(&image), true, None),
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
    use super::perceptual_hash;
    use image::{DynamicImage, GrayImage, Luma};

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
}
