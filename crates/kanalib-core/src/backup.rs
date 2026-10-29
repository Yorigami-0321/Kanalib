//! F-061 数据库备份核心（任务 A）：WAL checkpoint、rusqlite backup、
//! 备份库完整性/计数校验、自动备份计划/保留策略与「重启后应用」的恢复流程。
//!
//! 计划存 `settings` 表的 JSON（键 `backup.plan`），不新增 schema。恢复不在运行中
//! 替换数据库：候选库校验通过后复制为 `kanalib.restore-pending.db` 并写标记，
//! 下次 [`crate::state::AppState`] 打开前由 [`apply_pending_restore`] 原子替换；
//! 这样避免 Windows 上打开句柄、watcher 与缩略图内存状态互相干扰。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db::{BackupInspection, Db};
use crate::model::{BackupReport, TableCount};

/// 数据目录下存放数据库备份的子目录名。
pub const BACKUPS_DIR_NAME: &str = "backups";

/// 自动备份计划在 `settings` 表中的键；值为 JSON，不新增 schema。
pub const BACKUP_PLAN_KEY: &str = "backup.plan";

/// 恢复候选被复制到的暂存文件名；重启时由 [`apply_pending_restore`] 换上。
pub const PENDING_RESTORE_DB_NAME: &str = "kanalib.restore-pending.db";

/// 待恢复标记（JSON）文件名；只有标记存在时才应用暂存库。
pub const PENDING_RESTORE_MARKER_NAME: &str = "kanalib.restore-pending.json";

/// 备份目录（`data_dir/backups`），只计算路径，不创建目录。
pub fn backups_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(BACKUPS_DIR_NAME)
}

/// 自动备份频率；到期口径是「距最近一份本应用生成的备份满一个间隔」。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackupFrequency {
    #[default]
    Daily,
    Weekly,
}

impl BackupFrequency {
    pub fn label(self) -> &'static str {
        match self {
            Self::Daily => "每天",
            Self::Weekly => "每周",
        }
    }

    pub fn interval_secs(self) -> u64 {
        match self {
            Self::Daily => 86_400,
            Self::Weekly => 604_800,
        }
    }
}

/// 自动备份计划；序列化为 JSON 存入 `settings.backup.plan`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupPlan {
    pub enabled: bool,
    pub frequency: BackupFrequency,
    /// 保留最近多少份本应用生成的备份（至少 1）。
    pub keep_files: usize,
    /// 只保留最近多少天的备份；None 表示不按天数清理。
    pub keep_days: Option<u32>,
}

impl Default for BackupPlan {
    fn default() -> Self {
        Self {
            enabled: false,
            frequency: BackupFrequency::Daily,
            keep_files: 7,
            keep_days: None,
        }
    }
}

impl BackupPlan {
    /// 把用户/旧配置写入的越界值收拢到安全范围。
    pub fn sanitized(mut self) -> Self {
        self.keep_files = self.keep_files.clamp(1, 365);
        self.keep_days = self.keep_days.map(|days| days.clamp(1, 3_650));
        self
    }

    /// 解析 JSON；无效内容回退默认计划（不启用），避免坏配置阻断启动。
    pub fn parse(json: Option<&str>) -> Self {
        json.and_then(|value| serde_json::from_str::<Self>(value).ok())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str::<Self>(json).map(Self::sanitized)
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.clone().sanitized())
    }

    /// 下一次到期时间。启用且从未备份时立即到期；时钟回拨也按到期处理。
    pub fn next_due_at(&self, now: SystemTime, last: Option<SystemTime>) -> Option<SystemTime> {
        if !self.enabled {
            return None;
        }
        let Some(last) = last else {
            return Some(now);
        };
        if last >= now {
            return Some(now);
        }
        last.checked_add(Duration::from_secs(self.frequency.interval_secs()))
            .or(Some(now))
    }

    pub fn is_due(&self, now: SystemTime, last: Option<SystemTime>) -> bool {
        match self.next_due_at(now, last) {
            Some(due) => now >= due,
            None => false,
        }
    }
}

/// 备份计划读写；解析失败只记警告并回退默认值。
pub fn load_plan(db: &Db) -> BackupPlan {
    match db.setting(BACKUP_PLAN_KEY) {
        Ok(Some(json)) => match BackupPlan::from_json(&json) {
            Ok(plan) => plan,
            Err(error) => {
                log::warn!("parse backup plan: {error}");
                BackupPlan::default()
            }
        },
        Ok(None) => BackupPlan::default(),
        Err(error) => {
            log::warn!("read backup plan: {error:#}");
            BackupPlan::default()
        }
    }
}

pub fn save_plan(db: &Db, plan: &BackupPlan) -> Result<()> {
    let json = plan
        .to_json()
        .context("序列化自动备份计划失败")?;
    db.set_setting(BACKUP_PLAN_KEY, &json)
}

/// 运行时状态快照；UI 只读，E1 用它接底栏/命令面板。
#[derive(Clone, Debug, Default)]
pub struct BackupStatus {
    /// 当前计划（每次读取时从 `settings` 重新解析）。
    pub plan: BackupPlan,
    /// 是否正在执行备份/恢复准备。
    pub running: bool,
    pub last_at: Option<SystemTime>,
    pub last_path: Option<PathBuf>,
    pub last_error: Option<String>,
    /// 最近一次因扫描/打标跳过的原因；成功后清空。
    pub skipped_reason: Option<String>,
    pub skipped_at: Option<SystemTime>,
    /// 计划到期时间；未启用时为 None。
    pub next_due: Option<SystemTime>,
    /// 已写入但需重启应用的恢复请求。
    pub pending_restore: Option<PendingRestore>,
}

/// 一份已暂存、等待重启应用的恢复请求。
#[derive(Clone, Debug, Default)]
pub struct PendingRestore {
    pub candidate: PathBuf,
    pub safety_backup: Option<PathBuf>,
    pub staged: PathBuf,
    pub created_at: Option<SystemTime>,
    pub user_version: i64,
}

/// 保留策略清理结果；只包含本应用生成的备份文件。
#[derive(Clone, Debug, Default)]
pub struct PruneReport {
    pub removed: Vec<PathBuf>,
    pub kept: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// 恢复准备结果；`requires_restart` 为 true 时重启后生效。
#[derive(Clone, Debug, Default)]
pub struct RestoreReport {
    pub candidate: PathBuf,
    pub safety_backup: Option<PathBuf>,
    pub staged: PathBuf,
    pub tables: Vec<TableCount>,
    pub user_version: i64,
    pub warnings: Vec<String>,
    pub requires_restart: bool,
}

/// 重启时实际应用恢复的结果。
#[derive(Clone, Debug, Default)]
pub struct RestoreApplied {
    pub candidate: PathBuf,
    pub safety_backup: Option<PathBuf>,
    pub tables: Vec<TableCount>,
    pub user_version: i64,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PendingRestoreMarker {
    candidate: PathBuf,
    staged: String,
    safety_backup: Option<PathBuf>,
    created_unix_secs: u64,
    user_version: i64,
    tables: Vec<TableCount>,
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

/// 恢复前的安全备份路径（`backups/kanalib-pre-restore-*.db`）。
///
/// 名称刻意不含纯时间戳，因此不会被自动保留策略清理；用户可手动删除。
pub fn safety_backup_path(data_dir: &Path) -> PathBuf {
    let dir = backups_dir(data_dir);
    let stamp = utc_stamp(SystemTime::now());
    let mut candidate = dir.join(format!("kanalib-pre-restore-{stamp}.db"));
    let mut attempt = 1;
    while candidate.exists() {
        candidate = dir.join(format!("kanalib-pre-restore-{stamp}-{attempt}.db"));
        attempt += 1;
    }
    candidate
}

/// 是否为自动备份生成的严格文件名：`kanalib-YYYYMMDD-HHMMSS.db`
/// 或同秒去重后缀 `kanalib-YYYYMMDD-HHMMSS-N.db`。
///
/// 保留策略只删除满足此命名的文件；`kanalib-pre-restore-*.db`、用户自己命名
/// 的 `.db` 与目录都不会被删除。
pub fn is_managed_backup_name(name: &str) -> bool {
    parse_managed_stamp(name).is_some()
}

fn parse_managed_stamp(name: &str) -> Option<SystemTime> {
    let rest = name.strip_prefix("kanalib-")?.strip_suffix(".db")?;
    let stamp = if rest.len() == 15 {
        rest
    } else if rest.len() > 16 && rest.as_bytes()[15] == b'-' {
        let suffix = &rest[16..];
        if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        &rest[..15]
    } else {
        return None;
    };
    parse_stamp(stamp)
}

/// 解析 `YYYYMMDD-HHMMSS` 为 UTC 时间；非法日期/时间返回 None。
fn parse_stamp(stamp: &str) -> Option<SystemTime> {
    let bytes = stamp.as_bytes();
    if bytes.len() != 15 || bytes[8] != b'-' {
        return None;
    }
    if !bytes[..8].iter().all(u8::is_ascii_digit) || !bytes[9..].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> i64 {
        stamp[range].parse::<i64>().unwrap_or(-1)
    };
    let year = number(0..4);
    let month = number(4..6);
    let day = number(6..8);
    let hour = number(9..11);
    let minute = number(11..13);
    let second = number(13..15);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    let days = days_from_civil(year, month as u32, day as u32);
    let secs = days
        .checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second)?;
    if secs < 0 {
        return None;
    }
    UNIX_EPOCH.checked_add(Duration::from_secs(secs as u64))
}

/// 公历日期换算为 1970-01-01 起的天数（Howard Hinnant 算法）。
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = (year - era * 400) as u64;
    let month = month as u64;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// 备份文件按名称解析出的 UTC 时间；非本应用命名返回 None。
pub fn managed_backup_time(path: &Path) -> Option<SystemTime> {
    let name = path.file_name()?.to_str()?;
    parse_managed_stamp(name)
}

/// 最近一份本应用生成的备份（按文件名时间，而非 mtime）。
pub fn latest_managed_backup(dir: &Path) -> Option<(PathBuf, SystemTime)> {
    let mut latest: Option<(PathBuf, SystemTime)> = None;
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().map(|kind| kind.is_file()).unwrap_or(false) {
            continue;
        }
        let Some(time) = managed_backup_time(&path) else {
            continue;
        };
        if latest.as_ref().is_none_or(|(_, current)| time > *current) {
            latest = Some((path, time));
        }
    }
    latest
}

/// 按计划清理本应用生成的备份；绝不碰用户文件或其他目录内容。
///
/// 规则：按文件名时间从新到旧，删除超过 `keep_files` 的旧文件；设置
/// `keep_days` 时再删除超过该天数的旧文件。无论如何至少保留最新一份。
pub fn prune_backups(dir: &Path, plan: &BackupPlan, now: SystemTime) -> Result<PruneReport> {
    let mut report = PruneReport::default();
    if !dir.is_dir() {
        return Ok(report);
    }
    let plan = plan.clone().sanitized();
    let mut managed: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("读取备份目录 {}", dir.display()))? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report.warnings.push(format!("读取备份目录项失败：{error}"));
                continue;
            }
        };
        let path = entry.path();
        if !entry.file_type().map(|kind| kind.is_file()).unwrap_or(false) {
            continue;
        }
        let Some(time) = managed_backup_time(&path) else {
            continue;
        };
        managed.push((time, path));
    }
    managed.sort_by(|(time_a, path_a), (time_b, path_b)| {
        time_a.cmp(time_b).then_with(|| path_a.cmp(path_b))
    });
    let total = managed.len();
    for (index, (time, path)) in managed.into_iter().enumerate() {
        let rank_from_newest = total - index;
        // 永远保留最新一份，避免 keep_days 把备份清空。
        if rank_from_newest <= 1 {
            report.kept.push(path);
            continue;
        }
        let too_many = rank_from_newest > plan.keep_files;
        let too_old = plan
            .keep_days
            .map(|days| {
                now.duration_since(time)
                    .map(|age| age > Duration::from_secs(u64::from(days) * 86_400))
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if !too_many && !too_old {
            report.kept.push(path);
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => {
                log::info!("prune backup {}", path.display());
                report.removed.push(path);
            }
            Err(error) => {
                report
                    .warnings
                    .push(format!("删除旧备份 {} 失败：{error}", path.display()));
                report.kept.push(path);
            }
        }
    }
    Ok(report)
}

/// 供 UI 展示的 UTC 日期时间（`YYYY-MM-DD HH:MM UTC`）。
pub fn format_utc_datetime(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rem / 3_600,
        (rem % 3_600) / 60
    )
}

/// 校验候选恢复库：完整性、关键表、schema 版本；不修改候选文件。
fn validate_restore_candidate(path: &Path) -> Result<(BackupInspection, Vec<String>)> {
    let inspection = Db::inspect_database(path)?;
    if !inspection.integrity_ok() {
        bail!(
            "候选备份完整性校验失败：{}",
            inspection.integrity
        );
    }
    for required in ["settings", "assets"] {
        if inspection.table_rows(required).is_none() {
            bail!("候选备份缺少关键表 {required}，不能恢复");
        }
    }
    let mut warnings = Vec::new();
    let current = Db::schema_version();
    if inspection.user_version <= 0 {
        bail!("候选备份缺少有效的 schema 版本（user_version=0），不是可恢复的 Kanalib 数据库");
    }
    if inspection.user_version > current {
        bail!(
            "候选备份来自更新版本（schema v{}），当前仅支持 v{current}",
            inspection.user_version
        );
    }
    if inspection.user_version < current {
        warnings.push(format!(
            "候选备份为旧版本 schema v{}，重启打开时会执行现有迁移（迁移前自动另存备份）",
            inspection.user_version
        ));
    }
    Ok((inspection, warnings))
}

fn tables_equal(left: &[TableCount], right: &[TableCount]) -> bool {
    left.len() == right.len()
        && left.iter().all(|entry| {
            right
                .iter()
                .any(|other| other.table == entry.table && other.rows == entry.rows)
        })
}

fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

fn remove_database_files(path: &Path) {
    let _ = fs::remove_file(path);
    for suffix in ["-wal", "-shm", "-journal"] {
        let _ = fs::remove_file(sidecar(path, suffix));
    }
}

/// 准备恢复：只读校验候选、安全备份当前库、把候选复制为待应用暂存库并写标记。
///
/// 运行中的应用不会被替换；调用方需保证扫描/打标未运行，并提示用户重启。
/// 失败时当前库不变；可能留下安全备份（有效备份文件，便于人工回退）。
pub fn stage_restore(
    data_dir: &Path,
    db_path: &Path,
    candidate: &Path,
) -> Result<RestoreReport> {
    if !candidate.is_file() {
        bail!("候选备份不存在：{}", candidate.display());
    }
    if same_file(candidate, db_path) {
        bail!("不能把当前数据库当作恢复候选");
    }
    let staged = data_dir.join(PENDING_RESTORE_DB_NAME);
    let marker_path = data_dir.join(PENDING_RESTORE_MARKER_NAME);
    if marker_path.exists() || staged.exists() {
        bail!("已有待恢复的备份，重启 Kanalib 应用或先移除待恢复标记后再试");
    }
    let (inspection, mut warnings) = validate_restore_candidate(candidate)?;

    fs::create_dir_all(data_dir)
        .with_context(|| format!("创建数据目录 {}", data_dir.display()))?;
    ensure_backups_dir(data_dir)?;
    let safety = if db_path.is_file() {
        let target = safety_backup_path(data_dir);
        match backup_database(db_path, &target, false) {
            Ok(report) => Some(report.destination),
            Err(error) => {
                bail!("恢复前创建安全备份失败，已中止：{error:#}");
            }
        }
    } else {
        warnings.push("当前数据库不存在，未创建恢复前安全备份".to_string());
        None
    };

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = data_dir.join(format!(
        ".{PENDING_RESTORE_DB_NAME}.{}-{nonce}.tmp",
        std::process::id()
    ));
    let result = stage_restore_copy(candidate, &temp, &staged, &inspection);
    if let Err(error) = result {
        remove_database_files(&temp);
        remove_database_files(&staged);
        let _ = fs::remove_file(&marker_path);
        if let Some(safety) = &safety {
            // 安全备份是有效文件，保留并提示；不自动删除用户可用的回退点。
            log::warn!("restore staged copy failed; safety backup kept at {}", safety.display());
        }
        return Err(error);
    }

    let marker = PendingRestoreMarker {
        candidate: candidate.to_path_buf(),
        staged: PENDING_RESTORE_DB_NAME.to_string(),
        safety_backup: safety.clone(),
        created_unix_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        user_version: inspection.user_version,
        tables: inspection.tables.clone(),
    };
    let marker_tmp = data_dir.join(format!(".{PENDING_RESTORE_MARKER_NAME}.{nonce}.tmp"));
    let marker_json = serde_json::to_vec_pretty(&marker).context("序列化待恢复标记失败")?;
    if let Err(error) = fs::write(&marker_tmp, &marker_json)
        .and_then(|()| fs::rename(&marker_tmp, &marker_path))
    {
        let _ = fs::remove_file(&marker_tmp);
        remove_database_files(&staged);
        let _ = fs::remove_file(&marker_path);
        bail!("写入待恢复标记失败：{error}");
    }
    log::warn!(
        "restore staged: {} -> {} (safety: {:?})",
        candidate.display(),
        staged.display(),
        safety
    );
    Ok(RestoreReport {
        candidate: candidate.to_path_buf(),
        safety_backup: safety,
        staged,
        tables: inspection.tables,
        user_version: inspection.user_version,
        warnings,
        requires_restart: true,
    })
}

fn stage_restore_copy(
    candidate: &Path,
    temp: &Path,
    staged: &Path,
    expected: &BackupInspection,
) -> Result<()> {
    fs::copy(candidate, temp)
        .with_context(|| format!("复制候选备份到 {}", temp.display()))?;
    for suffix in ["-wal", "-shm"] {
        let source = sidecar(candidate, suffix);
        if source.is_file() {
            fs::copy(&source, sidecar(temp, suffix))
                .with_context(|| format!("复制备份边车文件 {}", source.display()))?;
        }
    }
    let conn = Connection::open(temp)
        .with_context(|| format!("打开恢复暂存库 {}", temp.display()))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    // 合并候选可能的 WAL，并转成单文件 DELETE 模式，重启时可直接替换主库。
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    });
    conn.query_row("PRAGMA journal_mode=DELETE", [], |row| {
        row.get::<_, String>(0)
    })
    .with_context(|| "恢复暂存库切换到 DELETE 日志模式失败".to_string())?;
    drop(conn);
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(sidecar(temp, suffix));
    }
    let staged_inspection = Db::inspect_database(temp)?;
    if !staged_inspection.integrity_ok() {
        bail!(
            "恢复暂存库完整性校验失败：{}",
            staged_inspection.integrity
        );
    }
    if staged_inspection.user_version != expected.user_version {
        bail!(
            "恢复暂存库 schema 版本不一致（候选 v{}，暂存 v{}）",
            expected.user_version,
            staged_inspection.user_version
        );
    }
    if !tables_equal(&staged_inspection.tables, &expected.tables) {
        bail!("恢复暂存库关键表行数与候选不一致，已中止");
    }
    fs::rename(temp, staged)
        .with_context(|| format!("保存恢复暂存库 {}", staged.display()))?;
    Ok(())
}

/// 重启时应用待恢复备份：校验暂存库、替换当前库并复核；失败回滚。
///
/// 必须在任何 `Db::open` 之前调用（[`crate::state::AppState::open`] 已接入）。
pub fn apply_pending_restore(data_dir: &Path) -> Result<Option<RestoreApplied>> {
    let marker_path = data_dir.join(PENDING_RESTORE_MARKER_NAME);
    if !marker_path.is_file() {
        return Ok(None);
    }
    let staged = data_dir.join(PENDING_RESTORE_DB_NAME);
    let validation = (|| -> Result<(PendingRestoreMarker, BackupInspection)> {
        let bytes = fs::read(&marker_path)
            .with_context(|| format!("读取待恢复标记 {}", marker_path.display()))?;
        let marker: PendingRestoreMarker =
            serde_json::from_slice(&bytes).context("解析待恢复标记失败")?;
        if marker.staged != PENDING_RESTORE_DB_NAME {
            bail!("待恢复标记中的暂存文件名无效：{}", marker.staged);
        }
        if !staged.is_file() {
            bail!("待恢复暂存库不存在：{}", staged.display());
        }
        let inspection = Db::inspect_database(&staged)?;
        if !inspection.integrity_ok() {
            bail!(
                "待恢复暂存库完整性校验失败：{}",
                inspection.integrity
            );
        }
        if inspection.user_version != marker.user_version {
            bail!(
                "待恢复暂存库 schema 版本与标记不一致（标记 v{}，暂存 v{}）",
                marker.user_version,
                inspection.user_version
            );
        }
        if !tables_equal(&inspection.tables, &marker.tables) {
            bail!("待恢复暂存库关键表行数与标记不一致");
        }
        Ok((marker, inspection))
    })();
    let (marker, _inspection) = match validation {
        Ok(value) => value,
        Err(error) => {
            // 校验失败发生在替换之前：清掉坏请求，当前库保持原样。
            remove_database_files(&staged);
            let _ = fs::remove_file(&marker_path);
            bail!("待恢复备份校验失败，已丢弃恢复请求并保留当前数据库：{error:#}");
        }
    };

    let db_path = data_dir.join("kanalib.db");
    let stamp = utc_stamp(SystemTime::now());
    let rollback = data_dir.join(format!("kanalib.db.rollback-{stamp}"));
    let had_current = db_path.is_file();
    if had_current {
        // 先把当前库 WAL 落盘，改名后回滚才能保持最新状态。
        let conn = Connection::open(&db_path)
            .with_context(|| format!("打开当前数据库 {}", db_path.display()))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        });
        drop(conn);
        for suffix in ["-wal", "-shm", "-journal"] {
            let _ = fs::remove_file(sidecar(&db_path, suffix));
        }
        fs::rename(&db_path, &rollback)
            .with_context(|| format!("备份当前数据库到 {}", rollback.display()))?;
    }

    let apply_result = (|| -> Result<()> {
        fs::copy(&staged, &db_path)
            .with_context(|| format!("复制待恢复库到 {}", db_path.display()))?;
        let applied = Db::inspect_database(&db_path)?;
        if !applied.integrity_ok() {
            bail!("恢复后的数据库完整性校验失败：{}", applied.integrity);
        }
        if !tables_equal(&applied.tables, &marker.tables) {
            bail!("恢复后的数据库关键表行数与标记不一致");
        }
        Ok(())
    })();

    match apply_result {
        Ok(()) => {
            let _ = fs::remove_file(&rollback);
            let _ = fs::remove_file(&staged);
            let _ = fs::remove_file(&marker_path);
            log::warn!(
                "pending restore applied: {} (safety: {:?})",
                marker.candidate.display(),
                marker.safety_backup
            );
            Ok(Some(RestoreApplied {
                candidate: marker.candidate,
                safety_backup: marker.safety_backup,
                tables: marker.tables,
                user_version: marker.user_version,
                warnings: Vec::new(),
            }))
        }
        Err(error) => {
            remove_database_files(&db_path);
            let rollback_error = if had_current {
                match fs::rename(&rollback, &db_path) {
                    Ok(()) => None,
                    Err(rollback_error) => Some(rollback_error),
                }
            } else {
                None
            };
            let _ = fs::remove_file(&staged);
            let _ = fs::remove_file(&marker_path);
            match rollback_error {
                Some(rollback_error) => bail!(
                    "{error:#}；且回滚当前数据库失败：{rollback_error}；安全备份：{}",
                    marker
                        .safety_backup
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "无".to_string())
                ),
                None => bail!("应用恢复失败，已保留当前数据库：{error:#}"),
            }
        }
    }
}

/// 读取待恢复信息（UI 展示用）；无效标记返回 None。
pub fn pending_restore(data_dir: &Path) -> Option<PendingRestore> {
    let marker_path = data_dir.join(PENDING_RESTORE_MARKER_NAME);
    let bytes = fs::read(&marker_path).ok()?;
    let marker: PendingRestoreMarker = serde_json::from_slice(&bytes).ok()?;
    Some(PendingRestore {
        candidate: marker.candidate,
        safety_backup: marker.safety_backup,
        staged: data_dir.join(&marker.staged),
        created_at: UNIX_EPOCH.checked_add(Duration::from_secs(marker.created_unix_secs)),
        user_version: marker.user_version,
    })
}

/// 丢弃待恢复请求（不应用）；供 UI/人工取消。
pub fn discard_pending_restore(data_dir: &Path) -> Result<bool> {
    let marker_path = data_dir.join(PENDING_RESTORE_MARKER_NAME);
    let staged = data_dir.join(PENDING_RESTORE_DB_NAME);
    let existed = marker_path.exists() || staged.exists();
    remove_database_files(&staged);
    match fs::remove_file(&marker_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("删除待恢复标记 {}", marker_path.display()));
        }
    }
    Ok(existed)
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

    #[test]
    fn backup_plan_parses_clamps_and_schedules() {
        use super::{BackupFrequency, BackupPlan};

        let plan = BackupPlan::parse(Some(
            r#"{"enabled":true,"frequency":"weekly","keep_files":0,"keep_days":0}"#,
        ));
        assert!(plan.enabled);
        assert_eq!(plan.frequency, BackupFrequency::Weekly);
        assert_eq!(plan.keep_files, 1);
        assert_eq!(plan.keep_days, Some(1));
        assert!(!BackupPlan::parse(Some("not json")).enabled);
        assert!(!BackupPlan::parse(None).enabled);
        let parsed = BackupPlan::from_json(
            r#"{"enabled":true,"frequency":"daily","keep_files":3}"#,
        )
        .unwrap();
        assert_eq!(parsed.keep_files, 3);
        assert_eq!(parsed.keep_days, None);

        let daily = BackupPlan {
            enabled: true,
            frequency: BackupFrequency::Daily,
            keep_files: 7,
            keep_days: None,
        };
        let start = UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        assert_eq!(daily.next_due_at(start, None), Some(start));
        assert!(!daily.is_due(start + Duration::from_secs(23 * 3_600), Some(start)));
        // 跨天：昨晚 23:00 备份，今天 01:00 仍未到期；次日 23:30 到期。
        let evening = start;
        assert!(!daily.is_due(evening + Duration::from_secs(2 * 3_600), Some(evening)));
        assert!(daily.is_due(evening + Duration::from_secs(24 * 3_600 + 1_800), Some(evening)));
        assert_eq!(
            daily.next_due_at(evening + Duration::from_secs(25 * 3_600), Some(evening)),
            Some(evening + Duration::from_secs(86_400))
        );

        let weekly = BackupPlan {
            frequency: BackupFrequency::Weekly,
            ..daily.clone()
        };
        assert!(!weekly.is_due(start + Duration::from_secs(6 * 86_400), Some(start)));
        assert!(weekly.is_due(start + Duration::from_secs(8 * 86_400), Some(start)));
        // 时钟回拨：上次备份时间在“未来”时按立即到期处理。
        assert_eq!(
            daily.next_due_at(start, Some(start + Duration::from_secs(3_600))),
            Some(start)
        );
        let disabled = BackupPlan {
            enabled: false,
            ..daily
        };
        assert_eq!(disabled.next_due_at(start, None), None);
        assert!(!disabled.is_due(start, None));
    }

    #[test]
    fn backup_plan_round_trips_through_settings() {
        use super::{BackupFrequency, BackupPlan};

        let root = temp_root("plan-save");
        let db = Db::open(&root.join("kanalib.db")).unwrap();
        assert!(!super::load_plan(&db).enabled);
        let plan = BackupPlan {
            enabled: true,
            frequency: BackupFrequency::Weekly,
            keep_files: 3,
            keep_days: Some(10),
        };
        super::save_plan(&db, &plan).unwrap();
        assert_eq!(super::load_plan(&db), plan);
        drop(db);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn retention_prunes_only_generated_backups() {
        use super::{BackupPlan, ensure_backups_dir, prune_backups, utc_stamp};

        let root = temp_root("prune");
        let dir = ensure_backups_dir(&root).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut managed = Vec::new();
        for days_ago in [4u64, 3, 2, 1] {
            let path = dir.join(format!(
                "kanalib-{}.db",
                utc_stamp(now - Duration::from_secs(days_ago * 86_400))
            ));
            fs::write(&path, b"backup").unwrap();
            managed.push(path);
        }
        let user_files = [
            "kanalib-notes.db".to_string(),
            "my-backup.db".to_string(),
            "kanalib-pre-restore-20260101-000000.db".to_string(),
        ];
        for name in &user_files {
            fs::write(dir.join(name), b"user").unwrap();
        }
        assert!(super::is_managed_backup_name("kanalib-20260101-000000.db"));
        assert!(super::is_managed_backup_name("kanalib-20260101-000000-2.db"));
        assert!(!super::is_managed_backup_name("kanalib-notes.db"));
        assert!(!super::is_managed_backup_name("kanalib-pre-restore-20260101-000000.db"));
        assert!(!super::is_managed_backup_name("my-backup.db"));

        let plan = BackupPlan {
            enabled: true,
            keep_files: 2,
            keep_days: None,
            ..Default::default()
        };
        let report = prune_backups(&dir, &plan, now).unwrap();
        assert_eq!(report.removed.len(), 2, "{:?}", report.removed);
        assert!(!managed[0].exists(), "oldest should be pruned");
        assert!(!managed[1].exists(), "second oldest should be pruned");
        assert!(managed[2].exists());
        assert!(managed[3].exists());
        for name in &user_files {
            assert!(dir.join(name).exists(), "user file {name} must survive");
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn retention_days_never_removes_newest_backup() {
        use super::{BackupPlan, ensure_backups_dir, prune_backups, utc_stamp};

        let root = temp_root("prune-days");
        let dir = ensure_backups_dir(&root).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        for days_ago in [10u64, 5, 1] {
            fs::write(
                dir.join(format!(
                    "kanalib-{}.db",
                    utc_stamp(now - Duration::from_secs(days_ago * 86_400))
                )),
                b"backup",
            )
            .unwrap();
        }
        let plan = BackupPlan {
            enabled: true,
            keep_files: 10,
            keep_days: Some(2),
            ..Default::default()
        };
        let report = prune_backups(&dir, &plan, now).unwrap();
        assert_eq!(report.removed.len(), 2, "{:?}", report.removed);

        let only = dir.join(format!(
            "kanalib-{}.db",
            utc_stamp(now - Duration::from_secs(30 * 86_400))
        ));
        let only_root = temp_root("prune-oldest");
        let only_dir = ensure_backups_dir(&only_root).unwrap();
        let only_path = only_dir.join(only.file_name().unwrap());
        fs::write(&only_path, b"backup").unwrap();
        let plan = BackupPlan {
            enabled: true,
            keep_days: Some(1),
            ..Default::default()
        };
        prune_backups(&only_dir, &plan, now).unwrap();
        assert!(only_path.exists(), "the only/newest backup must never be pruned");
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&only_root).unwrap();
    }

    #[test]
    fn stage_restore_rejects_bad_candidates_without_touching_current() {
        use super::{pending_restore, stage_restore};

        let root = temp_root("restore-bad");
        let data = root.join("data");
        fs::create_dir_all(&data).unwrap();
        let db_path = data.join("kanalib.db");
        {
            let db = Db::open(&db_path).unwrap();
            db.set_setting("k", "v").unwrap();
        }

        let corrupt = root.join("corrupt.db");
        fs::write(&corrupt, b"not a database").unwrap();
        let err = stage_restore(&data, &db_path, &corrupt)
            .unwrap_err()
            .to_string();
        assert!(!err.is_empty(), "{err}");
        assert_eq!(
            Db::open(&db_path).unwrap().setting("k").unwrap().as_deref(),
            Some("v")
        );
        assert!(pending_restore(&data).is_none());
        assert!(!data.join(super::PENDING_RESTORE_DB_NAME).exists());

        let partial = root.join("partial.db");
        {
            let conn = Connection::open(&partial).unwrap();
            conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);")
                .unwrap();
        }
        let err = stage_restore(&data, &db_path, &partial)
            .unwrap_err()
            .to_string();
        assert!(err.contains("assets"), "{err}");
        assert_eq!(
            Db::open(&db_path).unwrap().setting("k").unwrap().as_deref(),
            Some("v")
        );

        let err = stage_restore(&data, &db_path, &db_path)
            .unwrap_err()
            .to_string();
        assert!(err.contains("当前数据库"), "{err}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn stage_and_apply_restore_replaces_database_and_keeps_safety_backup() {
        use super::{apply_pending_restore, pending_restore, stage_restore};

        let root = temp_root("restore-apply");
        let data = root.join("data");
        fs::create_dir_all(&data).unwrap();
        let db_path = data.join("kanalib.db");
        {
            let db = Db::open(&db_path).unwrap();
            db.set_setting("k", "v").unwrap();
        }
        let candidate = root.join("candidate.db");
        {
            let db = Db::open(&candidate).unwrap();
            db.set_setting("k", "restored").unwrap();
        }

        let report = stage_restore(&data, &db_path, &candidate).unwrap();
        assert!(report.requires_restart);
        assert!(report.staged.is_file());
        assert!(report.safety_backup.as_ref().unwrap().is_file());
        assert!(pending_restore(&data).is_some());
        // 运行中不替换：当前库仍是旧数据。
        assert_eq!(
            Db::open(&db_path).unwrap().setting("k").unwrap().as_deref(),
            Some("v")
        );

        let applied = apply_pending_restore(&data).unwrap().unwrap();
        assert_eq!(applied.candidate, candidate);
        assert_eq!(
            Db::open(&db_path).unwrap().setting("k").unwrap().as_deref(),
            Some("restored")
        );
        let safety = applied.safety_backup.unwrap();
        assert_eq!(
            Db::open(&safety).unwrap().setting("k").unwrap().as_deref(),
            Some("v")
        );
        assert!(pending_restore(&data).is_none());
        assert!(!report.staged.exists());
        let rollback_left = fs::read_dir(&data)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with("kanalib.db.rollback-"))
            .collect::<Vec<_>>();
        assert!(rollback_left.is_empty(), "{rollback_left:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn apply_pending_restore_rejects_tampered_marker_and_keeps_current() {
        use super::{apply_pending_restore, discard_pending_restore, stage_restore};

        let root = temp_root("restore-tampered");
        let data = root.join("data");
        fs::create_dir_all(&data).unwrap();
        let db_path = data.join("kanalib.db");
        {
            let db = Db::open(&db_path).unwrap();
            db.set_setting("k", "v").unwrap();
        }
        let candidate = root.join("candidate.db");
        {
            let db = Db::open(&candidate).unwrap();
            db.set_setting("k", "restored").unwrap();
        }
        stage_restore(&data, &db_path, &candidate).unwrap();

        let marker_path = data.join(super::PENDING_RESTORE_MARKER_NAME);
        let mut marker: serde_json::Value =
            serde_json::from_slice(&fs::read(&marker_path).unwrap()).unwrap();
        let tables = marker["tables"].as_array_mut().unwrap();
        let assets = tables
            .iter_mut()
            .find(|entry| entry["table"] == "assets")
            .unwrap();
        assets["rows"] = serde_json::json!(assets["rows"].as_i64().unwrap() + 1);
        fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();

        let err = apply_pending_restore(&data).unwrap_err().to_string();
        assert!(err.contains("行数"), "{err}");
        assert_eq!(
            Db::open(&db_path).unwrap().setting("k").unwrap().as_deref(),
            Some("v")
        );
        // 校验失败在替换前清理坏请求，避免每次启动重复失败；当前库不变。
        assert!(!marker_path.exists());
        assert!(!discard_pending_restore(&data).unwrap());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn restore_from_wal_backup_preserves_uncheckpointed_rows() {
        use super::{apply_pending_restore, stage_restore};

        let root = temp_root("restore-wal");
        let source = root.join("source").join("kanalib.db");
        let db = Db::open(&source).unwrap();
        db.set_setting("wal.marker", "1").unwrap();
        assert!(
            fs::metadata(sidecar(&source, "-wal")).unwrap().len() > 0,
            "source should still have uncheckpointed WAL pages"
        );
        let candidate = root.join("wal-backup.db");
        db.backup_to(&candidate).unwrap();
        drop(db);

        let data = root.join("data");
        let db_path = data.join("kanalib.db");
        fs::create_dir_all(&data).unwrap();
        {
            let current = Db::open(&db_path).unwrap();
            current.set_setting("k", "v").unwrap();
        }
        stage_restore(&data, &db_path, &candidate).unwrap();
        apply_pending_restore(&data).unwrap().unwrap();
        let restored = Db::open(&db_path).unwrap();
        assert_eq!(
            restored.setting("wal.marker").unwrap().as_deref(),
            Some("1")
        );
        assert_eq!(restored.setting("k").unwrap(), None);
        drop(restored);
        fs::remove_dir_all(&root).unwrap();
    }
}
