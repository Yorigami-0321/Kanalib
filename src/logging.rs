//! 应用日志初始化（F-062）。
//!
//! 路径优先级：
//! 1. `KANALIB_LOG_FILE`：精确路径覆盖（追加写，不滚动），保持既有调试习惯；
//! 2. `KANALIB_HEADLESS`：`<project_root>/.kanalib/headless.log`，保持既有 headless 行为；
//! 3. 正常 GUI：`<project_root>/.kanalib/logs/kanalib.log`，按大小滚动并清理过期文件。
//!
//! 滚动默认值：10 MiB / 保留 5 个滚动文件 / 保留 14 天，分别可用
//! `KANALIB_LOG_MAX_BYTES`、`KANALIB_LOG_KEEP`、`KANALIB_LOG_RETENTION_DAYS` 覆盖
//! （`KANALIB_LOG_RETENTION_DAYS=0` 关闭按天数清理）。
//! 日志级别沿用 `RUST_LOG`，未设置时默认 `info`。
//!
//! 初始化失败不会阻止应用启动；文件日志不可用时退回 stderr，并已用 eprintln 提示。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use flexi_logger::{Cleanup, Criterion, FileSpec, Logger, LoggerHandle, Naming, WriteMode};
use kanalib_core::diagnostics::default_log_path;
use kanalib_core::sidecar;

/// 单个滚动日志文件默认上限：10 MiB。
pub const DEFAULT_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// 默认保留的滚动日志文件数。
pub const DEFAULT_LOG_KEEP_FILES: usize = 5;
/// 默认日志保留天数（0 表示关闭按天数清理）。
pub const DEFAULT_LOG_RETENTION_DAYS: u64 = 14;
/// 默认日志文件基名。
const LOG_BASENAME: &str = "kanalib";
/// 滚动文件的时间戳中缀格式；当前文件本身不带中缀，保持 `kanalib.log`。
const ROTATED_INFIX_FORMAT: &str = "r%Y-%m-%d_%H-%M-%S";

static LOGGER_HANDLE: OnceLock<LoggerHandle> = OnceLock::new();
static LOG_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 初始化日志，返回实际写入的日志文件路径。
///
/// 未启用文件日志（初始化失败或退回 stderr）时返回 `None`。
/// 可重复调用；只有第一次生效，后续返回相同结果。
pub fn init() -> Option<PathBuf> {
    LOG_PATH.get_or_init(init_once).clone()
}

fn init_once() -> Option<PathBuf> {
    let settings = LogSettings::from_env();
    if let Some(path) = std::env::var_os("KANALIB_LOG_FILE").map(PathBuf::from) {
        return start_exact(&path);
    }

    let data_dir = sidecar::project_root().join(".kanalib");
    if std::env::var_os("KANALIB_HEADLESS").is_some() {
        if let Err(error) = fs::create_dir_all(&data_dir) {
            eprintln!(
                "create headless log directory {}: {error}",
                data_dir.display()
            );
        }
        return start_exact(&data_dir.join("headless.log"));
    }

    let path = default_log_path(&data_dir);
    if let Some(directory) = path.parent() {
        if let Err(error) = fs::create_dir_all(directory) {
            eprintln!("create log directory {}: {error}", directory.display());
        }
        let removed =
            cleanup_old_logs(directory, &path, settings.retention_days, SystemTime::now());
        if removed > 0 {
            eprintln!("removed {removed} expired log file(s)");
        }
    }
    start_rolling(&path, settings)
}

/// 精确路径模式：追加写，不滚动（`KANALIB_LOG_FILE` / headless）。
fn start_exact(path: &Path) -> Option<PathBuf> {
    let file_spec = match FileSpec::try_from(path) {
        Ok(spec) => spec,
        Err(error) => {
            eprintln!("invalid log file path {}: {error}", path.display());
            return start_stderr();
        }
    };
    let logger = base_logger()
        .log_to_file(file_spec)
        .append()
        .write_mode(WriteMode::Direct);
    start_logger(logger, path, None)
}

/// 默认滚动模式：按大小滚动，保留固定数量文件。
fn start_rolling(path: &Path, settings: LogSettings) -> Option<PathBuf> {
    let Some(file_spec) = rolling_file_spec(path) else {
        eprintln!("invalid log file path {}", path.display());
        return start_stderr();
    };
    let logger = base_logger()
        .log_to_file(file_spec)
        .append()
        .write_mode(WriteMode::Direct)
        .rotate(
            Criterion::Size(settings.max_bytes),
            rolling_naming(),
            Cleanup::KeepLogFiles(settings.keep_files),
        );
    start_logger(logger, path, Some(settings))
}

fn start_logger(logger: Logger, path: &Path, settings: Option<LogSettings>) -> Option<PathBuf> {
    match logger.start() {
        Ok(handle) => {
            if LOGGER_HANDLE.set(handle).is_ok() {
                if let Some(settings) = settings {
                    log::info!(
                        "file logging enabled: {} (max_bytes={}, keep={}, retention_days={})",
                        path.display(),
                        settings.max_bytes,
                        settings.keep_files,
                        settings.retention_days,
                    );
                } else {
                    log::info!("file logging enabled: {}", path.display());
                }
            }
            Some(path.to_path_buf())
        }
        Err(error) => {
            eprintln!("init file logging {}: {error}", path.display());
            start_stderr()
        }
    }
}

/// 文件日志初始化失败时退回 stderr；返回 `None` 表示没有日志文件。
fn start_stderr() -> Option<PathBuf> {
    match base_logger().log_to_stderr().start() {
        Ok(handle) => {
            let _ = LOGGER_HANDLE.set(handle);
            None
        }
        Err(error) => {
            eprintln!("init logging: {error}");
            None
        }
    }
}

fn base_logger() -> Logger {
    match Logger::try_with_env_or_str("info") {
        Ok(logger) => logger.format(flexi_logger::detailed_format),
        Err(error) => {
            eprintln!("invalid RUST_LOG ({error}); falling back to info");
            Logger::try_with_str("info")
                .expect("info is a valid log specification")
                .format(flexi_logger::detailed_format)
        }
    }
}

fn rolling_file_spec(path: &Path) -> Option<FileSpec> {
    let directory = path.parent()?;
    let basename = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| LOG_BASENAME.to_string());
    let suffix = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_string())
        .unwrap_or_else(|| "log".to_string());
    Some(
        FileSpec::default()
            .directory(directory)
            .basename(basename)
            .suffix(suffix)
            .suppress_timestamp(),
    )
}

/// 当前文件不带中缀（`kanalib.log`），滚动文件带时间戳（`kanalib_r...log`）。
fn rolling_naming() -> Naming {
    Naming::TimestampsCustomFormat {
        current_infix: Some(""),
        format: ROTATED_INFIX_FORMAT,
    }
}

/// 日志滚动/保留配置；解析失败或越界时使用默认值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LogSettings {
    max_bytes: u64,
    keep_files: usize,
    retention_days: u64,
}

impl Default for LogSettings {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_LOG_MAX_BYTES,
            keep_files: DEFAULT_LOG_KEEP_FILES,
            retention_days: DEFAULT_LOG_RETENTION_DAYS,
        }
    }
}

impl LogSettings {
    fn from_env() -> Self {
        Self::from_values(
            std::env::var("KANALIB_LOG_MAX_BYTES").ok().as_deref(),
            std::env::var("KANALIB_LOG_KEEP").ok().as_deref(),
            std::env::var("KANALIB_LOG_RETENTION_DAYS").ok().as_deref(),
        )
    }

    fn from_values(
        max_bytes: Option<&str>,
        keep_files: Option<&str>,
        retention_days: Option<&str>,
    ) -> Self {
        let defaults = Self::default();
        Self {
            // 小于 1 KiB 的上限会让滚动过于频繁，视为无效配置。
            max_bytes: parse_u64(max_bytes)
                .filter(|value| *value >= 1024)
                .unwrap_or(defaults.max_bytes),
            keep_files: parse_u64(keep_files)
                .map(|value| value.clamp(1, 100) as usize)
                .unwrap_or(defaults.keep_files),
            // 0 是合法值：关闭按天数清理。
            retention_days: parse_u64(retention_days).unwrap_or(defaults.retention_days),
        }
    }
}

fn parse_u64(value: Option<&str>) -> Option<u64> {
    value.and_then(|value| value.trim().parse::<u64>().ok())
}

/// 启动时按修改时间删除超过 `retention_days` 的滚动日志。
///
/// 不删除当前写入文件；只处理 `kanalib*.log`，避免误删同目录其他文件。
/// 返回删除数量。`retention_days == 0` 时不做任何事。
fn cleanup_old_logs(
    directory: &Path,
    current: &Path,
    retention_days: u64,
    now: SystemTime,
) -> usize {
    if retention_days == 0 {
        return 0;
    }
    let Some(cutoff) = now.checked_sub(Duration::from_secs(
        retention_days.saturating_mul(24 * 60 * 60),
    )) else {
        return 0;
    };
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path == current || !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let is_log = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("log"));
        if !name.starts_with(LOG_BASENAME) || !is_log {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        if modified < cutoff && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!(
            "kanalib-logging-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn set_modified(path: &Path, time: SystemTime) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(time).unwrap();
    }

    #[test]
    fn settings_use_defaults_and_clamp_invalid_values() {
        assert_eq!(
            LogSettings::from_values(None, None, None),
            LogSettings::default()
        );
        assert_eq!(
            LogSettings::from_values(Some("2048"), Some("3"), Some("30")),
            LogSettings {
                max_bytes: 2048,
                keep_files: 3,
                retention_days: 30,
            }
        );
        // 上限过小/非数字回退默认，保留数夹在 1..=100，天数 0 合法。
        assert_eq!(
            LogSettings::from_values(Some("100"), Some("0"), Some("0")),
            LogSettings {
                max_bytes: DEFAULT_LOG_MAX_BYTES,
                keep_files: 1,
                retention_days: 0,
            }
        );
        assert_eq!(
            LogSettings::from_values(Some("nope"), Some("1000"), Some("-1")),
            LogSettings {
                max_bytes: DEFAULT_LOG_MAX_BYTES,
                keep_files: 100,
                retention_days: DEFAULT_LOG_RETENTION_DAYS,
            }
        );
    }

    #[test]
    fn rolling_spec_keeps_current_name_and_timestamps_rotated_files() {
        let data_dir = temp_dir("spec");
        let directory = data_dir.join("logs");
        fs::create_dir_all(&directory).unwrap();
        let current = directory.join("kanalib.log");
        let spec = rolling_file_spec(&current).unwrap();
        assert_eq!(spec.as_pathbuf(Some("")), current);
        let rotated = spec.as_pathbuf(Some("r2026-10-27_11-00-00"));
        assert_eq!(rotated, directory.join("kanalib_r2026-10-27_11-00-00.log"));
        // 默认路径与 core 的约定一致。
        assert_eq!(default_log_path(&data_dir), current);
    }

    #[test]
    fn cleanup_removes_only_expired_rotated_logs() {
        let directory = temp_dir("cleanup");
        let now = SystemTime::now();
        let old = directory.join("kanalib_r2026-01-01_00-00-00.log");
        let fresh = directory.join("kanalib_r2026-10-27_00-00-00.log");
        let current = directory.join("kanalib.log");
        let other = directory.join("other_r2026-01-01_00-00-00.log");
        for path in [&old, &fresh, &current, &other] {
            fs::write(path, b"x").unwrap();
        }
        let thirty_days_ago = now - Duration::from_secs(30 * 24 * 60 * 60);
        // 当前文件即使很旧也不能删；其他前缀不归本应用管理。
        for path in [&old, &current, &other] {
            set_modified(path, thirty_days_ago);
        }
        set_modified(&fresh, now);

        let removed = cleanup_old_logs(&directory, &current, 14, now);
        assert_eq!(removed, 1);
        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(current.exists());
        assert!(other.exists());

        // 0 天表示关闭清理。
        assert_eq!(cleanup_old_logs(&directory, &current, 0, now), 0);
        assert!(current.exists());
    }
}
