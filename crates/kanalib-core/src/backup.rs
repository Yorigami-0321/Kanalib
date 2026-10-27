//! F-061 数据库备份核心（任务 A）：WAL checkpoint、rusqlite backup、
//! 备份库完整性/计数校验。接口见 [`crate::model::BackupReport`]。
//!
//! 这里只负责「备份 + 校验」：自动定时、恢复流程与设置页入口由后续任务接入。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::db::Db;
use crate::model::BackupReport;

/// 数据目录下存放数据库备份的子目录名。
pub const BACKUPS_DIR_NAME: &str = "backups";

/// 备份目录（`data_dir/backups`），只计算路径，不创建目录。
pub fn backups_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(BACKUPS_DIR_NAME)
}

/// 创建并返回备份目录；UI 在首次备份前调用一次即可。
pub fn ensure_backups_dir(data_dir: &Path) -> Result<PathBuf> {
    let dir = backups_dir(data_dir);
    fs::create_dir_all(&dir).with_context(|| format!("创建备份目录 {}", dir.display()))?;
    Ok(dir)
}

/// 带时间戳的默认备份路径（`backups/kanalib-YYYYMMDD-HHMMSS.db`，UTC）；
/// 同秒重复调用时追加 `-1`、`-2` 后缀，避免覆盖已有备份。
pub fn default_backup_path(data_dir: &Path) -> PathBuf {
    let dir = backups_dir(data_dir);
    let stamp = utc_stamp(SystemTime::now());
    let mut candidate = dir.join(format!("kanalib-{stamp}.db"));
    let mut attempt = 1;
    while candidate.exists() {
        candidate = dir.join(format!("kanalib-{stamp}-{attempt}.db"));
        attempt += 1;
    }
    candidate
}

/// 备份 `db_path` 到 `destination`。
///
/// 只依赖数据库路径，不访问 `AppState`，UI 可直接在后台线程调用。
/// `scanning` 为 true 时明确拒绝（扫描期间库处于大量写入状态），错误信息可直接
/// 展示给用户。目标文件已存在时拒绝覆盖；失败不会修改源库。
pub fn backup_database(
    db_path: &Path,
    destination: &Path,
    scanning: bool,
) -> Result<BackupReport> {
    if scanning {
        bail!("图库正在扫描，数据库持续写入，请等待扫描结束后再备份。");
    }
    if !db_path.is_file() {
        bail!("源数据库不存在：{}", db_path.display());
    }
    if destination == db_path {
        bail!("备份文件不能与源数据库相同：{}", destination.display());
    }
    if destination.exists() {
        bail!("备份文件已存在，拒绝覆盖：{}", destination.display());
    }
    let db = Db::open(db_path).with_context(|| format!("打开源数据库 {}", db_path.display()))?;
    db.backup_to(destination)
}

fn utc_stamp(now: SystemTime) -> String {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 把 1970-01-01 起的天数换算为公历年月日（Howard Hinnant 算法）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod test {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, UNIX_EPOCH};

    use rusqlite::{Connection, OpenFlags};

    use super::{backup_database, backups_dir, default_backup_path, ensure_backups_dir};
    use crate::db::Db;
    use crate::media::ContentHashes;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "kanalib-backup-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn sidecar(path: &std::path::Path, suffix: &str) -> PathBuf {
        let mut value = path.as_os_str().to_owned();
        value.push(suffix);
        PathBuf::from(value)
    }

    fn read_integrity(path: &std::path::Path) -> String {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn backup_copies_uncheckpointed_wal_data_and_verifies() {
        let root = temp_root("core");
        let db_path = root.join("kanalib.db");
        let backup_path = root.join("backup.db");
        {
            let db = Db::open(&db_path).unwrap();
            db.set_setting("ui.view", "list").unwrap();
            db.set_setting("ui.sidebar_width", "260").unwrap();
            db.upsert_meta(
                "C:\\lib",
                Path::new("C:\\lib\\a.png"),
                None,
                100,
                80,
                1234,
                77,
                &ContentHashes {
                    sha1: "a".repeat(40),
                    blake3: "b".repeat(64),
                    phash: 1,
                    decode_ok: true,
                    decode_error: None,
                },
                false,
            )
            .unwrap();

            // 写入后 WAL 中仍有未 checkpoint 的页。
            let wal = sidecar(&db_path, "-wal");
            assert!(wal.exists(), "source db should be in WAL mode");
            assert!(fs::metadata(&wal).unwrap().len() > 0);

            let report = db.backup_to(&backup_path).unwrap();
            assert_eq!(report.destination, backup_path);
            assert!(report.source.ends_with("kanalib.db"), "{:?}", report.source);
            assert!(report.integrity_ok);
            assert!(report.bytes > 0);
            assert!(report.warnings.is_empty(), "{:?}", report.warnings);
            let count = |table: &str| {
                report
                    .tables
                    .iter()
                    .find(|entry| entry.table == table)
                    .unwrap_or_else(|| panic!("missing table count for {table}"))
                    .rows
            };
            assert_eq!(count("settings"), 2);
            assert_eq!(count("assets"), 1);
            for name in ["tags", "asset_tags", "jobs", "sources"] {
                assert!(
                    report.tables.iter().any(|entry| entry.table == name),
                    "missing table count for {name}"
                );
            }
            assert!(
                !sidecar(&backup_path, "-wal").exists(),
                "backup should be self-contained"
            );
        }

        assert_eq!(read_integrity(&backup_path), "ok");

        // 备份库可独立打开，设置值与资产一致。
        let copy = Db::open(&backup_path).unwrap();
        assert_eq!(copy.setting("ui.view").unwrap().as_deref(), Some("list"));
        assert_eq!(
            copy.setting("ui.sidebar_width").unwrap().as_deref(),
            Some("260")
        );
        let assets = copy.load_folder("C:\\lib").unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].width, 100);
        assert_eq!(assets[0].height, 80);
        assert_eq!(assets[0].bytes, 1234);
        assert_eq!(
            copy.stored_meta(Path::new("C:\\lib\\a.png"))
                .unwrap()
                .map(|meta| meta.mtime),
            Some(77)
        );
        drop(copy);

        let conn = Connection::open_with_flags(&backup_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let archived: i64 = conn
            .query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(archived, 1);
        drop(conn);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn backup_failures_leave_source_intact() {
        let root = temp_root("fail");
        let db_path = root.join("kanalib.db");
        let db = Db::open(&db_path).unwrap();
        db.set_setting("k", "v").unwrap();

        // 目标已存在：拒绝覆盖且内容不变。
        let existing = root.join("existing.db");
        fs::write(&existing, b"not a database").unwrap();
        let err = db.backup_to(&existing).unwrap_err().to_string();
        assert!(err.contains("已存在"), "{err}");
        assert_eq!(fs::read(&existing).unwrap(), b"not a database");

        // 父目录不存在。
        let missing = root.join("missing-dir").join("backup.db");
        let err = db.backup_to(&missing).unwrap_err().to_string();
        assert!(err.contains("目录不存在"), "{err}");
        assert!(!missing.exists());

        // 扫描中拒绝。
        let scanned = root.join("scan.db");
        let err = backup_database(&db_path, &scanned, true).unwrap_err().to_string();
        assert!(err.contains("扫描"), "{err}");
        assert!(!scanned.exists());

        // 失败后没有留下本次创建的临时产物。
        let leftovers: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        drop(db);
        // 源库仍可打开且数据不变。
        let again = Db::open(&db_path).unwrap();
        assert_eq!(again.setting("k").unwrap().as_deref(), Some("v"));
        drop(again);

        // 自由函数走完整流程（含 Db::open）。
        let report = backup_database(&db_path, &root.join("ok.db"), false).unwrap();
        assert!(report.integrity_ok);
        assert_eq!(
            report
                .tables
                .iter()
                .find(|entry| entry.table == "settings")
                .unwrap()
                .rows,
            1
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn default_backup_path_is_timestamped_and_unique() {
        let root = temp_root("path");
        assert_eq!(backups_dir(&root), root.join("backups"));
        let dir = ensure_backups_dir(&root).unwrap();
        assert!(dir.is_dir());

        let first = default_backup_path(&root);
        assert_eq!(first.parent().unwrap(), dir);
        let name = first.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("kanalib-"), "{name}");
        assert!(name.ends_with(".db"), "{name}");
        assert!(!first.exists());

        fs::write(&first, b"existing").unwrap();
        let second = default_backup_path(&root);
        assert_ne!(first, second);
        assert!(!second.exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn utc_stamp_formats_dates() {
        let epoch = UNIX_EPOCH + Duration::from_secs(1_775_000_000);
        let stamp = super::utc_stamp(epoch);
        assert_eq!(stamp.len(), 15, "{stamp}");
        assert!(stamp.chars().all(|c| c.is_ascii_digit() || c == '-'), "{stamp}");
        assert_eq!(super::utc_stamp(UNIX_EPOCH + Duration::from_secs(0)), "19700101-000000");
        assert_eq!(
            super::utc_stamp(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "20000229-000000"
        );
    }
}
