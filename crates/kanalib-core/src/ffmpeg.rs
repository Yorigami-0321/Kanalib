//! Optional FFmpeg thumbnail backend. The binary is treated as an external
//! process: GPL builds are allowed for local use but must not be bundled.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use image::{DynamicImage, ImageFormat, ImageReader, RgbImage};

use crate::sidecar::project_root;

const DEFAULT_BATCH: usize = 32;
const MAX_BATCH: usize = 48;
const DEFAULT_TIMEOUT_SECS: u64 = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum License {
    Lgpl,
    Gpl,
    Unknown,
}

impl License {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lgpl => "LGPL",
            Self::Gpl => "GPL",
            Self::Unknown => "未知",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Build {
    pub path: PathBuf,
    pub version: String,
    pub configuration: String,
    pub license: License,
}

#[derive(Clone, Debug)]
pub struct ThumbJob {
    pub source: PathBuf,
    pub target: PathBuf,
    pub alpha: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Outcome {
    pub generated: usize,
    pub failed: usize,
    pub batches: usize,
}

impl Outcome {
    fn merge(&mut self, other: Self) {
        self.generated += other.generated;
        self.failed += other.failed;
        self.batches += other.batches;
    }
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

fn candidates() -> Vec<PathBuf> {
    // A system-wide ffmpeg wins; the program-local copy is the offline fallback.
    let mut paths = vec![PathBuf::from(binary_name())];
    let root = project_root();
    paths.push(
        root.join("runtime")
            .join("ffmpeg")
            .join("bin")
            .join(binary_name()),
    );
    if let Ok(dir) = std::env::current_exe()
        && let Some(dir) = dir.parent()
    {
        paths.push(
            dir.join("runtime")
                .join("ffmpeg")
                .join("bin")
                .join(binary_name()),
        );
    }
    #[cfg(debug_assertions)]
    paths.push(
        root.join("tools")
            .join("ffmpeg")
            .join("bin")
            .join(binary_name()),
    );
    paths
}

fn parse_version_output(text: &str) -> Result<Build> {
    let first = text
        .lines()
        .find(|line| line.starts_with("ffmpeg version "))
        .context("missing ffmpeg version line")?;
    let version = first
        .trim_start_matches("ffmpeg version ")
        .split(" Copyright")
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    let configuration = text
        .lines()
        .find_map(|line| line.strip_prefix("configuration:"))
        .unwrap_or_default()
        .trim()
        .to_string();
    let license = if configuration.is_empty() {
        License::Unknown
    } else if configuration
        .split_whitespace()
        .any(|flag| flag == "--enable-gpl")
    {
        License::Gpl
    } else {
        License::Lgpl
    };
    Ok(Build {
        path: PathBuf::new(),
        version,
        configuration,
        license,
    })
}

fn probe(path: &Path) -> Result<Build> {
    let mut command = Command::new(path);
    command
        .args(["-hide_banner", "-version"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut command);
    let output = command
        .output()
        .with_context(|| format!("run {}", path.display()))?;
    if !output.status.success() {
        bail!(
            "{} exited with {:?}: {}",
            path.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut build = parse_version_output(&text)?;
    build.path = path.to_path_buf();
    Ok(build)
}

fn log_license(build: &Build) {
    match build.license {
        License::Gpl => log::warn!(
            "ffmpeg: {} is a GPL build; do not redistribute it with Kanalib unless the whole distribution complies with the GPL",
            build.path.display()
        ),
        License::Lgpl => log::info!(
            "ffmpeg: {} version={} license={}",
            build.path.display(),
            build.version,
            build.license.label()
        ),
        License::Unknown => log::warn!(
            "ffmpeg: {} version={} has no configuration line; license is unknown",
            build.path.display(),
            build.version
        ),
    }
}

fn discover() -> Option<Build> {
    if let Some(value) = std::env::var_os("KANALIB_FFMPEG") {
        if value == "off" {
            log::info!("ffmpeg: disabled by KANALIB_FFMPEG=off");
            return None;
        }
        let path = PathBuf::from(value);
        return match probe(&path) {
            Ok(build) => {
                log_license(&build);
                Some(build)
            }
            Err(error) => {
                log::error!(
                    "ffmpeg: KANALIB_FFMPEG={} is not usable: {error:#}",
                    path.display()
                );
                None
            }
        };
    }
    let mut seen = Vec::new();
    for candidate in candidates() {
        if seen.iter().any(|path: &PathBuf| path == &candidate) {
            continue;
        }
        seen.push(candidate.clone());
        match probe(&candidate) {
            Ok(build) => {
                log_license(&build);
                return Some(build);
            }
            Err(error) => {
                if candidate.components().count() > 1 {
                    log::debug!("ffmpeg candidate {}: {error:#}", candidate.display());
                }
            }
        }
    }
    log::info!("ffmpeg: not found; thumbnails stay on the image crate");
    None
}

pub fn status() -> Option<&'static Build> {
    static BUILD: OnceLock<Option<Build>> = OnceLock::new();
    BUILD.get_or_init(discover).as_ref()
}

pub fn available() -> bool {
    status().is_some()
}

fn env_usize(key: &str, default: usize, min: usize, max: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|value| value.clamp(min, max))
        .unwrap_or(default)
}

fn worker_count() -> usize {
    let available = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4);
    env_usize("KANALIB_FFMPEG_WORKERS", (available / 2).clamp(2, 4), 1, 16)
}

fn batch_size() -> usize {
    env_usize("KANALIB_FFMPEG_BATCH", DEFAULT_BATCH, 4, MAX_BATCH)
}

fn timeout() -> Duration {
    let secs = std::env::var("KANALIB_FFMPEG_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|value| value.clamp(10, 600))
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

fn command(path: &Path) -> Command {
    let mut command = Command::new(path);
    command
        .args([
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-threads",
            "1",
            "-max_pixels",
            "268435456",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_console(&mut command);
    command
}

fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

fn scale_filter() -> &'static str {
    "scale=512:512:force_original_aspect_ratio=decrease"
}

pub fn alpha_source(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "webp" | "gif")
    )
}

fn temp_png(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_default();
    name.push(".ffmpeg.png");
    target.with_file_name(name)
}

fn wait_with_timeout(mut child: Child, limit: Duration) -> Result<(bool, String)> {
    let stderr = child.stderr.take();
    let reader = stderr.map(|mut stderr| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        })
    });
    let started = Instant::now();
    let timed_out = loop {
        if let Some(status) = child.try_wait()? {
            let _ = status;
            break false;
        }
        if started.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            break true;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stderr = reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    Ok((timed_out, stderr))
}

fn flatten_to_jpeg(image: DynamicImage, target: &Path) -> Result<()> {
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut rgb = RgbImage::new(width, height);
    for (source, target) in rgba.pixels().zip(rgb.pixels_mut()) {
        let alpha = source[3] as u32;
        let inverse = 255 - alpha;
        *target = image::Rgb([
            ((source[0] as u32 * alpha + 255 * inverse) / 255) as u8,
            ((source[1] as u32 * alpha + 255 * inverse) / 255) as u8,
            ((source[2] as u32 * alpha + 255 * inverse) / 255) as u8,
        ]);
    }
    DynamicImage::ImageRgb8(rgb)
        .save_with_format(target, ImageFormat::Jpeg)
        .with_context(|| format!("write {}", target.display()))
}

fn validate_jpeg(target: &Path) -> bool {
    ImageReader::open(target)
        .ok()
        .and_then(|reader| reader.with_guessed_format().ok())
        .and_then(|reader| reader.into_dimensions().ok())
        .is_some_and(|(width, height)| width > 0 && height > 0)
}

struct BatchReport {
    outcome: Outcome,
    failed: Vec<usize>,
    retryable: bool,
}

fn run_batch(ffmpeg: &Path, jobs: &[ThumbJob], limit: Duration) -> BatchReport {
    let mut outcome = Outcome {
        batches: 1,
        ..Outcome::default()
    };
    let mut report = BatchReport {
        outcome,
        failed: Vec::new(),
        retryable: true,
    };
    let mut command = command(ffmpeg);
    for job in jobs {
        command
            .arg("-protocol_whitelist")
            .arg("file")
            .arg("-i")
            .arg(&job.source);
    }
    for (index, job) in jobs.iter().enumerate() {
        let output = if job.alpha {
            temp_png(&job.target)
        } else {
            job.target.clone()
        };
        command
            .arg("-map")
            .arg(format!("{index}:v"))
            .arg("-vf")
            .arg(scale_filter())
            .arg("-frames:v")
            .arg("1")
            .arg("-map_metadata")
            .arg("-1");
        if !job.alpha {
            command.arg("-q:v").arg("5");
        }
        command.arg(&output);
    }
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            outcome.failed = jobs.len();
            log::warn!("ffmpeg batch spawn failed: {error}");
            report.outcome = outcome;
            report.retryable = false;
            return report;
        }
    };
    let (timed_out, stderr) = match wait_with_timeout(child, limit) {
        Ok(value) => value,
        Err(error) => {
            outcome.failed = jobs.len();
            log::warn!("ffmpeg batch wait failed: {error:#}");
            report.outcome = outcome;
            report.retryable = false;
            return report;
        }
    };
    if timed_out {
        log::warn!(
            "ffmpeg batch timed out after {}s for {} files",
            limit.as_secs(),
            jobs.len()
        );
        report.retryable = false;
    }
    for (index, job) in jobs.iter().enumerate() {
        if job.alpha {
            let temp = temp_png(&job.target);
            if temp.exists() {
                let converted = ImageReader::open(&temp)
                    .ok()
                    .and_then(|reader| reader.with_guessed_format().ok())
                    .and_then(|reader| reader.decode().ok())
                    .map(|image| flatten_to_jpeg(image, &job.target));
                let _ = std::fs::remove_file(&temp);
                match converted {
                    Some(Ok(())) => outcome.generated += 1,
                    _ => {
                        let _ = std::fs::remove_file(&job.target);
                        outcome.failed += 1;
                        report.failed.push(index);
                    }
                }
            } else {
                outcome.failed += 1;
                report.failed.push(index);
            }
        } else if validate_jpeg(&job.target) {
            outcome.generated += 1;
        } else {
            let _ = std::fs::remove_file(&job.target);
            outcome.failed += 1;
            report.failed.push(index);
        }
    }
    // A whole batch failing usually means a bad option or a broken binary,
    // not one corrupt image; retrying every input would only amplify it.
    if report.failed.len() == jobs.len() && jobs.len() > 1 {
        report.retryable = false;
    }
    if outcome.failed > 0 || timed_out {
        let excerpt = stderr
            .lines()
            .filter(|line| !line.trim().is_empty())
            .take(4)
            .collect::<Vec<_>>()
            .join(" | ");
        if !excerpt.is_empty() {
            log::warn!(
                "ffmpeg batch: {}/{} ok, {} failed: {excerpt}",
                outcome.generated,
                jobs.len(),
                outcome.failed
            );
        }
    }
    report.outcome = outcome;
    report
}

fn run_isolated(
    ffmpeg: &Path,
    jobs: &[ThumbJob],
    limit: Duration,
    depth: usize,
    cancel: &AtomicBool,
) -> Outcome {
    if jobs.is_empty() || cancel.load(Ordering::SeqCst) {
        return Outcome::default();
    }
    let report = run_batch(ffmpeg, jobs, limit);
    if report.failed.is_empty()
        || !report.retryable
        || jobs.len() <= 1
        || depth >= 6
        || cancel.load(Ordering::SeqCst)
    {
        return report.outcome;
    }
    let retry_jobs = report
        .failed
        .iter()
        .filter_map(|index| jobs.get(*index).cloned())
        .collect::<Vec<_>>();
    if retry_jobs.is_empty() {
        return report.outcome;
    }
    let mid = retry_jobs.len() / 2;
    let mut outcome = report.outcome;
    outcome.failed = 0;
    for slice in [&retry_jobs[..mid], &retry_jobs[mid..]] {
        let retried = run_isolated(ffmpeg, slice, limit, depth + 1, cancel);
        outcome.generated += retried.generated;
        outcome.failed += retried.failed;
        outcome.batches += retried.batches;
    }
    outcome
}

pub fn generate_thumbnails(
    jobs: &[ThumbJob],
    cancel: &AtomicBool,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Outcome {
    let Some(ffmpeg) = status() else {
        return Outcome::default();
    };
    if jobs.is_empty() {
        return Outcome::default();
    }
    let batch_size = batch_size();
    let batches = jobs.chunks(batch_size).collect::<Vec<&[ThumbJob]>>();
    let workers = worker_count().min(batches.len());
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results = Mutex::new(Outcome::default());
    let path = ffmpeg.path.clone();
    let limit = timeout();
    let total = jobs.len();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let next = &next;
            let done = &done;
            let results = &results;
            let batches = &batches;
            let path = &path;
            scope.spawn(move || {
                loop {
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let Some(batch) = batches.get(index) else {
                        break;
                    };
                    let outcome = run_isolated(path, batch, limit, 0, cancel);
                    let finished = done.fetch_add(batch.len(), Ordering::SeqCst) + batch.len();
                    progress(finished.min(total), total);
                    results.lock().unwrap().merge(outcome);
                }
            });
        }
    });
    results.into_inner().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gpl_and_lgpl_configuration() {
        let gpl = "ffmpeg version 7.1-full Copyright (c) 2000-2025\nconfiguration: --enable-gpl --enable-libx264 --enable-version3";
        let build = parse_version_output(gpl).unwrap();
        assert_eq!(build.license, License::Gpl);
        assert_eq!(build.version, "7.1-full");

        let lgpl = "ffmpeg version n8.1 Copyright (c) 2000-2026\nconfiguration: --enable-version3 --disable-libx264 --enable-shared";
        let build = parse_version_output(lgpl).unwrap();
        assert_eq!(build.license, License::Lgpl);

        let unknown = "ffmpeg version 6.0 Copyright (c) 2000-2024";
        assert_eq!(
            parse_version_output(unknown).unwrap().license,
            License::Unknown
        );
    }

    #[test]
    fn alpha_formats_use_the_png_path() {
        assert!(alpha_source(Path::new("a.PNG")));
        assert!(alpha_source(Path::new("a.webp")));
        assert!(alpha_source(Path::new("a.gif")));
        assert!(!alpha_source(Path::new("a.jpg")));
        assert!(!alpha_source(Path::new("a.bmp")));
    }

    #[test]
    fn batch_isolates_a_corrupt_input() {
        if !available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "kanalib-ffmpeg-corrupt-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut jobs = Vec::new();
        for index in 0..3 {
            let source = dir.join(format!("{index}.png"));
            image::RgbImage::from_pixel(32, 32, image::Rgb([20, 40, 80]))
                .save(&source)
                .unwrap();
            jobs.push(ThumbJob {
                source,
                target: dir.join(format!("{index}.jpg")),
                alpha: false,
            });
        }
        let bad = dir.join("bad.jpg");
        std::fs::write(&bad, b"not an image").unwrap();
        jobs.push(ThumbJob {
            source: bad,
            target: dir.join("bad-out.jpg"),
            alpha: false,
        });
        let outcome = generate_thumbnails(&jobs, &AtomicBool::new(false), &|_, _| {});
        assert_eq!(outcome.generated, 3);
        assert_eq!(outcome.failed, 1);
        for index in 0..3 {
            assert!(validate_jpeg(&dir.join(format!("{index}.jpg"))));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ffmpeg_batch_generates_thumbnail_and_flattens_alpha() {
        if !available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "kanalib-ffmpeg-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("alpha.png");
        let target = dir.join("alpha.jpg");
        image::RgbaImage::from_pixel(64, 32, image::Rgba([255, 0, 0, 128]))
            .save(&source)
            .unwrap();
        let outcome = generate_thumbnails(
            &[ThumbJob {
                source,
                target: target.clone(),
                alpha: true,
            }],
            &AtomicBool::new(false),
            &|_, _| {},
        );
        assert_eq!(outcome.generated, 1);
        assert!(validate_jpeg(&target));
        let image = ImageReader::open(&target)
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap()
            .to_rgb8();
        let pixel = image.get_pixel(32, 16);
        assert!(pixel[0] > 200 && pixel[1] > 80 && pixel[2] > 80);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
