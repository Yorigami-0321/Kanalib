//! 诊断快照与日志路径约定。
//!
//! 这里只做只读采集：不打开写连接、不修改 schema、不读取环境变量。
//! UI（任务 D）把运行时已经持有的计数/来源/设备信息传入 [`DiagnosticsContext`]，
//! 未提供的字段由本模块从数据目录和只读 SQLite 连接补齐。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use walkdir::WalkDir;

use crate::model::{DiagnosticSnapshot, SourceScanStatus};

/// 数据目录中的数据库文件名。
pub const DB_FILE_NAME: &str = "kanalib.db";
/// 数据目录中的缩略图缓存目录名。
pub const THUMB_CACHE_DIR_NAME: &str = "thumbs";
/// 数据目录中的日志子目录名。
pub const LOG_DIR_NAME: &str = "logs";
/// 默认滚动日志文件名（当前写入文件，不含滚动后缀）。
pub const LOG_FILE_NAME: &str = "kanalib.log";

/// 默认数据库路径：`<data_dir>/kanalib.db`。
pub fn default_db_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DB_FILE_NAME)
}

/// 默认缩略图缓存目录：`<data_dir>/thumbs`。
pub fn default_thumb_cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(THUMB_CACHE_DIR_NAME)
}

/// 默认日志路径：`<data_dir>/logs/kanalib.log`。
///
/// 正常 GUI 启动由 root crate 的 `logging` 模块使用该路径；
/// `KANALIB_LOG_FILE` 覆盖与 headless 模式的固定路径不经过这里。
pub fn default_log_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LOG_DIR_NAME).join(LOG_FILE_NAME)
}

/// 单个文件字节数；文件不存在或不是普通文件时返回 0。
pub fn file_bytes(path: &Path) -> u64 {
    fs::metadata(path)
        .map(|meta| if meta.is_file() { meta.len() } else { 0 })
        .unwrap_or(0)
}

/// 递归统计目录内普通文件字节数；不跟随符号链接，权限错误跳过。
pub fn directory_size(dir: &Path) -> u64 {
    WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
        .map(|meta| meta.len())
        .sum()
}

/// UI 已持有的运行时信息；所有计数都是可选的覆盖值。
///
/// 传入 `Some` 时优先于数据库只读查询与默认值；`sources` 为空时快照中也不会有来源。
#[derive(Clone, Debug, Default)]
pub struct DiagnosticsContext {
    /// 当前图库资产总数（UI 的 `Counts`/`Shared` 已有）。
    pub asset_count: Option<usize>,
    /// 打标队列等待数。
    pub queue_pending: Option<usize>,
    /// 打标队列运行数。
    pub queue_running: Option<usize>,
    /// 打标队列错误数。
    pub queue_error: Option<usize>,
    /// 各来源最近扫描状态。
    pub sources: Vec<SourceScanStatus>,
    /// 最近一次扫描时间（Unix 秒）；未提供时取来源中的最大值。
    pub last_scan: Option<u64>,
    /// 最近一次错误；未提供时取第一个来源错误。
    pub last_error: Option<String>,
    /// sidecar 推理设备（如 `cuda`/`cpu`）。
    pub sidecar_device: Option<String>,
    /// 实际日志文件路径；未提供时使用 [`default_log_path`]。
    pub log_path: Option<PathBuf>,
}

/// 采集诊断快照。
///
/// 数据目录、数据库或缓存不存在时不会失败，对应大小/计数为 0。
/// 只读数据库查询失败同样降级为默认值，避免诊断页因单个组件异常而不可用。
pub fn collect(data_dir: &Path, context: DiagnosticsContext) -> Result<DiagnosticSnapshot> {
    let DiagnosticsContext {
        asset_count,
        queue_pending,
        queue_running,
        queue_error,
        mut sources,
        last_scan,
        last_error,
        sidecar_device,
        log_path,
    } = context;

    let db_path = default_db_path(data_dir);
    let thumb_cache_dir = default_thumb_cache_dir(data_dir);
    let db_counts = read_db_counts(&db_path);

    let asset_count = asset_count.or(db_counts.assets).unwrap_or(0);
    let queue_pending = queue_pending.or(db_counts.pending).unwrap_or(0);
    let queue_running = queue_running.or(db_counts.running).unwrap_or(0);
    let queue_error = queue_error.or(db_counts.error).unwrap_or(0);

    for source in &mut sources {
        if let Some(error) = &source.error {
            source.error = Some(redact_text(error));
        }
    }
    // 稳定输出：来源按根目录排序，便于复制到 issue 后对比。
    sources.sort_by(|left, right| left.root.cmp(&right.root));

    let last_scan =
        last_scan.or_else(|| sources.iter().filter_map(|source| source.last_scan).max());
    let last_error = last_error
        .or_else(|| sources.iter().find_map(|source| source.error.clone()))
        .map(|error| redact_text(&error));

    Ok(DiagnosticSnapshot {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: data_dir.to_path_buf(),
        db_path,
        db_bytes: file_bytes(&default_db_path(data_dir)),
        thumb_cache_dir: thumb_cache_dir.clone(),
        thumb_cache_bytes: directory_size(&thumb_cache_dir),
        asset_count,
        queue_pending,
        queue_running,
        queue_error,
        sources,
        last_scan,
        last_error,
        log_path: Some(log_path.unwrap_or_else(|| default_log_path(data_dir))),
        sidecar_device,
    })
}

#[derive(Clone, Copy, Debug, Default)]
struct DbCounts {
    assets: Option<usize>,
    pending: Option<usize>,
    running: Option<usize>,
    error: Option<usize>,
}

/// 以只读方式打开数据库并查询计数；任何失败都返回空值。
fn read_db_counts(path: &Path) -> DbCounts {
    let mut counts = DbCounts::default();
    if !path.is_file() {
        return counts;
    }
    let Ok(conn) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return counts;
    };
    counts.assets = conn
        .query_row("SELECT COUNT(*) FROM assets", [], |row| {
            row.get::<_, i64>(0)
        })
        .ok()
        .map(|value| value.max(0) as usize);

    if let Ok(mut statement) =
        conn.prepare("SELECT state, COUNT(*) FROM jobs WHERE kind = 'tag' GROUP BY state")
    {
        let mut pending = 0usize;
        let mut running = 0usize;
        let mut error = 0usize;
        if let Ok(rows) = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as usize,
            ))
        }) {
            for row in rows.flatten() {
                match row.0.as_str() {
                    "pending" => pending = row.1,
                    "running" => running = row.1,
                    "error" => error = row.1,
                    _ => {}
                }
            }
            counts.pending = Some(pending);
            counts.running = Some(running);
            counts.error = Some(error);
        }
    }
    counts
}

impl DiagnosticSnapshot {
    /// 纯文本快照；字段顺序与标签固定，便于直接复制到 issue/聊天。
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        text.push_str("Kanalib 诊断信息\n");
        text.push_str(&format!("版本: {}\n", self.app_version));
        text.push_str(&format!("数据目录: {}\n", self.data_dir.display()));
        text.push_str(&format!(
            "数据库: {} ({})\n",
            self.db_path.display(),
            format_bytes(self.db_bytes)
        ));
        text.push_str(&format!(
            "缩略图缓存: {} ({})\n",
            self.thumb_cache_dir.display(),
            format_bytes(self.thumb_cache_bytes)
        ));
        text.push_str(&format!("资产总数: {}\n", self.asset_count));
        text.push_str(&format!(
            "打标队列: pending={} running={} error={}\n",
            self.queue_pending, self.queue_running, self.queue_error
        ));
        text.push_str(&format!(
            "推理设备: {}\n",
            self.sidecar_device.as_deref().unwrap_or("未知")
        ));
        text.push_str(&format!(
            "最近扫描: {}\n",
            self.last_scan
                .map(|value| value.to_string())
                .unwrap_or_else(|| "从未".to_string())
        ));
        text.push_str(&format!(
            "最近错误: {}\n",
            self.last_error.as_deref().unwrap_or("无")
        ));
        text.push_str(&format!(
            "日志: {}\n",
            self.log_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "未启用".to_string())
        ));
        text.push_str(&format!("来源: {}\n", self.sources.len()));
        for source in &self.sources {
            text.push_str(&format!(
                "- {} | assets={} | last_scan={} | online={} | error={}\n",
                source.root.display(),
                source.asset_count,
                source
                    .last_scan
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                if source.online { "yes" } else { "no" },
                source.error.as_deref().unwrap_or("-")
            ));
        }
        text
    }

    /// serde JSON 快照；用于复制/附件，不包含环境变量或凭据。
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}

/// 人类可读的字节数（二进制单位）。
pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let value = bytes as f64;
    if value >= GIB {
        format!("{:.1} GiB", value / GIB)
    } else if value >= MIB {
        format!("{:.1} MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// 对错误文本做最小脱敏：URL 中的 userinfo 与常见凭据键值。
///
/// 保留用户可见的文件路径；不读取、不输出任何环境变量。
pub fn redact_text(input: &str) -> String {
    let text = redact_url_credentials(input);
    let mut text = text;
    for key in [
        "password",
        "passwd",
        "token",
        "secret",
        "api_key",
        "apikey",
        "authorization",
    ] {
        text = redact_key_value(&text, key);
    }
    text
}

fn redact_url_credentials(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(scheme) = rest.find("://") {
        let after_scheme = scheme + 3;
        out.push_str(&rest[..after_scheme]);
        let tail = &rest[after_scheme..];
        let end = tail
            .find(|ch: char| {
                ch.is_whitespace()
                    || matches!(
                        ch,
                        '/' | '?' | '#' | '"' | '\'' | ')' | ']' | '}' | ',' | ';'
                    )
            })
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        if let Some(at) = authority.rfind('@') {
            let userinfo = &authority[..at];
            if userinfo.is_empty() {
                out.push_str(authority);
            } else {
                out.push_str("<redacted>@");
                out.push_str(&authority[at + 1..]);
            }
        } else {
            out.push_str(authority);
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

fn redact_key_value(input: &str, key: &str) -> String {
    input
        .split_inclusive(char::is_whitespace)
        .map(|token| {
            let lower = token.to_ascii_lowercase();
            let Some(position) = lower.find(key) else {
                return token.to_string();
            };
            let boundary_ok = position == 0
                || !token[..position]
                    .chars()
                    .last()
                    .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
            if !boundary_ok {
                return token.to_string();
            }
            let after_key = &token[position + key.len()..];
            let trimmed = after_key.trim_start();
            let separator = trimmed.chars().next();
            if !matches!(separator, Some('=') | Some(':')) {
                return token.to_string();
            }
            let value_start = position + key.len() + (after_key.len() - trimmed.len()) + 1;
            let value = &token[value_start..];
            let value_end = value.trim_end_matches(|ch: char| {
                ch.is_whitespace() || matches!(ch, ',' | ';' | ')' | ']' | '}' | '"' | '\'')
            });
            if value_end.is_empty() {
                return token.to_string();
            }
            format!(
                "{}<redacted>{}",
                &token[..value_start],
                &value[value_end.len()..]
            )
        })
        .collect()
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
            "kanalib-diagnostics-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn create_db(path: &Path, assets: usize, pending: usize, running: usize, error: usize) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE assets (id INTEGER PRIMARY KEY);
             CREATE TABLE jobs (
                 id INTEGER PRIMARY KEY,
                 kind TEXT NOT NULL,
                 state TEXT NOT NULL DEFAULT 'pending'
             );",
        )
        .unwrap();
        for id in 1..=assets {
            conn.execute("INSERT INTO assets(id) VALUES (?1)", [id as i64])
                .unwrap();
        }
        for (state, count) in [("pending", pending), ("running", running), ("error", error)] {
            for _ in 0..count {
                conn.execute("INSERT INTO jobs(kind, state) VALUES ('tag', ?1)", [state])
                    .unwrap();
            }
        }
        // 非 tag 任务不应进入快照队列计数。
        conn.execute(
            "INSERT INTO jobs(kind, state) VALUES ('scan', 'pending')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn default_paths_match_data_dir_layout() {
        let data_dir = Path::new("C:/data/.kanalib");
        assert_eq!(
            default_log_path(data_dir),
            data_dir.join("logs").join("kanalib.log")
        );
        assert_eq!(default_db_path(data_dir), data_dir.join("kanalib.db"));
        assert_eq!(default_thumb_cache_dir(data_dir), data_dir.join("thumbs"));
    }

    #[test]
    fn collect_reads_sizes_counts_and_defaults() {
        let data_dir = temp_dir("collect");
        let db_path = default_db_path(&data_dir);
        create_db(&db_path, 3, 2, 1, 4);
        let expected_db_bytes = fs::metadata(&db_path).unwrap().len();
        assert!(expected_db_bytes > 0);

        let cache = default_thumb_cache_dir(&data_dir);
        fs::create_dir_all(cache.join("nested/deeper")).unwrap();
        fs::write(cache.join("a.jpg"), vec![1u8; 100]).unwrap();
        fs::write(cache.join("nested/b.jpg"), vec![1u8; 200]).unwrap();
        fs::write(cache.join("nested/deeper/c.jpg"), vec![1u8; 300]).unwrap();
        // 目录本身不计数；非普通文件也不会被当成缓存字节。
        fs::create_dir_all(cache.join("empty")).unwrap();

        let source_root = data_dir.join("library");
        let sources = vec![SourceScanStatus {
            root: source_root.clone(),
            last_scan: Some(1_700_000_000),
            asset_count: 3,
            counts: Default::default(),
            error: Some("sync failed for https://user:hunter2@example.com/dav".to_string()),
            online: true,
        }];

        let snapshot = collect(
            &data_dir,
            DiagnosticsContext {
                sources,
                sidecar_device: Some("cuda".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(snapshot.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(snapshot.data_dir, data_dir);
        assert_eq!(snapshot.db_path, db_path);
        assert_eq!(snapshot.db_bytes, expected_db_bytes);
        assert_eq!(snapshot.thumb_cache_dir, cache);
        assert_eq!(snapshot.thumb_cache_bytes, 600);
        assert_eq!(snapshot.asset_count, 3);
        assert_eq!(snapshot.queue_pending, 2);
        assert_eq!(snapshot.queue_running, 1);
        assert_eq!(snapshot.queue_error, 4);
        assert_eq!(snapshot.last_scan, Some(1_700_000_000));
        assert_eq!(snapshot.sidecar_device.as_deref(), Some("cuda"));
        assert_eq!(snapshot.log_path, Some(default_log_path(&data_dir)));
        let error = snapshot.last_error.as_deref().unwrap();
        assert!(!error.contains("hunter2"), "credential leaked: {error}");
        assert!(error.contains("example.com"));

        let json = snapshot.to_json().unwrap();
        assert!(!json.contains("hunter2"));
        assert!(json.contains("\"app_version\""));
        let text = snapshot.to_text();
        assert!(text.contains("资产总数: 3"));
        assert!(text.contains("缩略图缓存:"));
        assert!(text.contains("kanalib.log"));
        assert!(!text.contains("hunter2"));
        assert!(!text.contains("KANALIB_LOG_FILE"));
        assert!(!text.contains("RUST_LOG"));
    }

    #[test]
    fn context_values_override_database_and_error_is_sanitized() {
        let data_dir = temp_dir("override");
        create_db(&default_db_path(&data_dir), 9, 9, 9, 9);
        let custom_log = data_dir.join("custom/kanalib.log");

        let snapshot = collect(
            &data_dir,
            DiagnosticsContext {
                asset_count: Some(1),
                queue_pending: Some(0),
                queue_running: Some(2),
                queue_error: Some(3),
                last_error: Some("token=super-secret-value failed".to_string()),
                log_path: Some(custom_log.clone()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(snapshot.asset_count, 1);
        assert_eq!(snapshot.queue_pending, 0);
        assert_eq!(snapshot.queue_running, 2);
        assert_eq!(snapshot.queue_error, 3);
        assert_eq!(snapshot.log_path, Some(custom_log.clone()));
        let error = snapshot.last_error.as_deref().unwrap();
        assert!(!error.contains("super-secret-value"));
        assert!(error.contains("<redacted>"));
        assert!(!snapshot.to_text().contains("super-secret-value"));
        assert!(!snapshot.to_json().unwrap().contains("super-secret-value"));
    }

    #[test]
    fn missing_data_files_collect_zero_and_keep_paths() {
        let data_dir = temp_dir("missing");
        let snapshot = collect(&data_dir, DiagnosticsContext::default()).unwrap();
        assert_eq!(snapshot.db_bytes, 0);
        assert_eq!(snapshot.thumb_cache_bytes, 0);
        assert_eq!(snapshot.asset_count, 0);
        assert_eq!(
            (
                snapshot.queue_pending,
                snapshot.queue_running,
                snapshot.queue_error
            ),
            (0, 0, 0)
        );
        assert_eq!(snapshot.last_scan, None);
        assert_eq!(snapshot.log_path, Some(default_log_path(&data_dir)));
        assert!(snapshot.sources.is_empty());
    }

    #[test]
    fn text_format_is_stable_and_readable() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(10 * 1024 * 1024), "10.0 MiB");
        assert_eq!(
            redact_text("https://alice:pw@host/x"),
            "https://<redacted>@host/x"
        );
        assert_eq!(
            redact_text("password=abc token:def"),
            "password=<redacted> token:<redacted>"
        );
    }
}
