//! 规则快照与热重载：`rules.toml` 仍是唯一权威，数据库只存只读副本。
//!
//! 重载失败一律保留旧快照；版本号未升而内容变化时默认拒绝，避免“同一版本两种判定”。

use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result, bail};
use tracing::warn;

use crate::{
    db::Db,
    rules::{self, LoadedRules},
};

pub const DEFAULT_RULES: &str = r#"
[rules]
version = "default"
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// 没有配置规则文件，只在启动时使用内置默认规则。
    Skipped,
    /// 文件未变化，或内容与当前快照完全一致。
    Unchanged,
    /// 已切换到新版本，并写入 `rule_snapshots`。
    Reloaded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified_nanos: u128,
    len: u64,
}

/// 当前生效规则 + 文件变更检测。
#[derive(Debug)]
pub struct RulesState {
    path: Option<PathBuf>,
    current: RwLock<Arc<LoadedRules>>,
    observed: Mutex<Option<FileStamp>>,
    last_error: Mutex<Option<String>>,
    reload_lock: Mutex<()>,
}

impl RulesState {
    /// 启动时加载规则并立即写入快照。
    /// 未配置 `rules_file` 与解析失败一样拒绝启动：不存在默认 allow 的兜底路径。
    pub fn load(db: &Db, path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else {
            bail!(
                "service.rules_file is not configured; refusing to start without a rules file \
                 (fail-closed, see docs/backlog/ingest-service.md §7.6)"
            );
        };
        let loaded = rules::load_file(path)?;
        Self::startup(db, loaded, Some(path.to_path_buf()))
    }

    /// 只给 mock 来源的本地冒烟使用：没有真实内容需要过滤，才允许内置默认规则。
    /// 真实来源必须走 `load`，缺规则文件一律拒绝启动。
    pub fn load_without_file(db: &Db) -> Result<Self> {
        warn!(
            "rules_file is not configured; using built-in default rules because the source is mock"
        );
        let loaded = rules::load_str(DEFAULT_RULES)?;
        Self::startup(db, loaded, None)
    }

    fn startup(db: &Db, loaded: LoadedRules, path: Option<PathBuf>) -> Result<Self> {
        if let Some(existing) = db.rule_snapshot(&loaded.version)?
            && existing.config_hash != loaded.config_hash
        {
            if loaded.config.rules.config_hash_check {
                bail!(
                    "rules.version {} was already applied with a different config_hash; \
                     bump rules.version and restart",
                    loaded.version
                );
            }
            warn!(
                version = %loaded.version,
                "config_hash_check is disabled; replacing the existing snapshot"
            );
        }
        record_snapshot(db, &loaded, "startup")?;
        warn_if_empty_tag_blacklist(&loaded);
        let observed = path.as_deref().and_then(file_stamp);
        Ok(Self {
            path,
            current: RwLock::new(Arc::new(loaded)),
            observed: Mutex::new(observed),
            last_error: Mutex::new(None),
            reload_lock: Mutex::new(()),
        })
    }

    pub fn current(&self) -> Arc<LoadedRules> {
        Arc::clone(&read_lock(&self.current))
    }

    pub fn last_error(&self) -> Option<String> {
        lock(&self.last_error).clone()
    }

    /// 仅在文件时间戳/长度变化时尝试重载。失败时旧快照继续生效。
    pub fn reload_if_changed(&self, db: &Db, trigger: &str) -> Result<ReloadOutcome> {
        let Some(path) = self.path.as_deref() else {
            return Ok(ReloadOutcome::Skipped);
        };
        let _guard = lock(&self.reload_lock);
        let stamp = file_stamp(path);
        if let Some(stamp) = stamp {
            let mut observed = lock(&self.observed);
            if *observed == Some(stamp) {
                return Ok(ReloadOutcome::Unchanged);
            }
            // 先记住观察值：坏文件不会每轮重复刷日志，改好后再试。
            *observed = Some(stamp);
        }
        self.remember(self.apply(db, path, trigger))
    }

    /// 强制重读文件（SIGHUP / 管理接口）；版本与校验规则同样生效。
    pub fn reload(&self, db: &Db, trigger: &str) -> Result<ReloadOutcome> {
        let Some(path) = self.path.as_deref() else {
            return Ok(ReloadOutcome::Skipped);
        };
        let _guard = lock(&self.reload_lock);
        if let Some(stamp) = file_stamp(path) {
            *lock(&self.observed) = Some(stamp);
        }
        self.remember(self.apply(db, path, trigger))
    }

    fn remember(&self, result: Result<ReloadOutcome>) -> Result<ReloadOutcome> {
        let mut last_error = lock(&self.last_error);
        match &result {
            Ok(_) => *last_error = None,
            Err(error) => *last_error = Some(error.to_string()),
        }
        drop(last_error);
        result
    }

    fn apply(&self, db: &Db, path: &Path, trigger: &str) -> Result<ReloadOutcome> {
        let loaded = rules::load_file(path)?;
        let current = self.current();
        if loaded.version == current.version {
            if loaded.config_hash == current.config_hash {
                *lock(&self.observed) = file_stamp(path);
                return Ok(ReloadOutcome::Unchanged);
            }
            if loaded.config.rules.config_hash_check {
                bail!(
                    "rules.version {} unchanged but config_hash changed; bump rules.version and retry",
                    loaded.version
                );
            }
            warn!(
                version = %loaded.version,
                "config_hash_check is disabled; accepting changed rules without a version bump"
            );
        } else if compare_versions(&loaded.version, &current.version) == Ordering::Less {
            bail!(
                "rules.version {} is older than active {}; rules_version must not decrease",
                loaded.version,
                current.version
            );
        }
        record_snapshot(db, &loaded, trigger)?;
        warn_if_empty_tag_blacklist(&loaded);
        let version = loaded.version.clone();
        *write_lock(&self.current) = Arc::new(loaded);
        *lock(&self.observed) = file_stamp(path);
        warn!(version = %version, trigger, "active rules switched");
        Ok(ReloadOutcome::Reloaded)
    }
}

fn warn_if_empty_tag_blacklist(loaded: &LoadedRules) {
    if loaded.config.tag_blacklist_is_empty() {
        warn!(
            version = %loaded.version,
            "rules.blacklist.tags is empty; no item can be blocked by tag"
        );
    }
}

/// 比较 `2026-11-22.2` 这类点分版本；数字段按数值比，混合段时数字更新。
pub fn compare_versions(left: &str, right: &str) -> Ordering {
    fn tokens(value: &str) -> Vec<&str> {
        value
            .split(|character: char| !character.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .collect()
    }
    let left = tokens(left);
    let right = tokens(right);
    for (a, b) in left.iter().zip(right.iter()) {
        let ordering = match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(a), Ok(b)) => a.cmp(&b),
            (Ok(_), Err(_)) => Ordering::Greater,
            (Err(_), Ok(_)) => Ordering::Less,
            (Err(_), Err(_)) => a.cmp(b),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

fn record_snapshot(db: &Db, loaded: &LoadedRules, loaded_by: &str) -> Result<()> {
    let config_json =
        serde_json::to_string(loaded.config.as_ref()).context("serialize rule snapshot")?;
    db.record_rule_snapshot(
        &loaded.version,
        &loaded.config_hash,
        &config_json,
        loaded_by,
    )
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified_nanos = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(FileStamp {
        modified_nanos,
        len: metadata.len(),
    })
}

fn read_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poison| poison.into_inner())
}

fn write_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|poison| poison.into_inner())
}

fn lock<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, version: &str, extra: &str) {
        std::fs::write(path, format!("[rules]\nversion = \"{version}\"\n{extra}")).unwrap();
    }

    #[test]
    fn version_comparison_orders_dotted_numbers() {
        assert_eq!(
            compare_versions("2026-11-22.1", "2026-11-22.2"),
            Ordering::Less
        );
        assert_eq!(
            compare_versions("2026-11-22.10", "2026-11-22.9"),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions("2026-11-22.1", "2026-11-22.1"),
            Ordering::Equal
        );
        assert_eq!(compare_versions("v1", "2026-11-22.1"), Ordering::Less);
    }

    #[test]
    fn startup_records_snapshot_and_reload_bumps_version() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("rules.toml");
        write(
            &path,
            "2026-11-22.1",
            "[rules.blacklist]\ntags = [\"one\"]\n",
        );
        let db = Db::open_in_memory().unwrap();
        let state = RulesState::load(&db, Some(&path)).unwrap();

        let first = db.rule_snapshot("2026-11-22.1").unwrap().unwrap();
        assert_eq!(first.loaded_by.as_deref(), Some("startup"));
        assert!(first.config_json.contains("\"one\""));

        write(
            &path,
            "2026-11-22.2",
            "[rules.blacklist]\ntags = [\"two\"]\n",
        );
        assert_eq!(state.reload(&db, "test").unwrap(), ReloadOutcome::Reloaded);
        assert_eq!(state.current().version, "2026-11-22.2");
        assert_eq!(
            state.current().config.rules.blacklist.tags,
            vec!["two".to_string()]
        );
        assert!(db.rule_snapshot("2026-11-22.1").unwrap().is_some());
        let second = db.rule_snapshot("2026-11-22.2").unwrap().unwrap();
        assert_eq!(second.loaded_by.as_deref(), Some("test"));
    }

    #[test]
    fn changed_hash_without_version_bump_is_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("rules.toml");
        write(
            &path,
            "2026-11-22.1",
            "[rules.blacklist]\ntags = [\"one\"]\n",
        );
        let db = Db::open_in_memory().unwrap();
        let state = RulesState::load(&db, Some(&path)).unwrap();
        let before = state.current();

        write(
            &path,
            "2026-11-22.1",
            "[rules.blacklist]\ntags = [\"two\"]\n",
        );
        let error = state.reload(&db, "test").unwrap_err();
        assert!(error.to_string().contains("bump rules.version"));
        assert_eq!(state.current().version, before.version);
        assert_eq!(state.current().config_hash, before.config_hash);
        assert_eq!(
            state.current().config.rules.blacklist.tags,
            vec!["one".to_string()]
        );
    }

    #[test]
    fn config_hash_check_false_allows_same_version_reload() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("rules.toml");
        write(
            &path,
            "2026-11-22.1",
            "config_hash_check = false\n[rules.blacklist]\ntags = [\"one\"]\n",
        );
        let db = Db::open_in_memory().unwrap();
        let state = RulesState::load(&db, Some(&path)).unwrap();
        let first = db.rule_snapshot("2026-11-22.1").unwrap().unwrap();

        write(
            &path,
            "2026-11-22.1",
            "config_hash_check = false\n[rules.blacklist]\ntags = [\"two\"]\n",
        );
        assert_eq!(state.reload(&db, "test").unwrap(), ReloadOutcome::Reloaded);
        let second = db.rule_snapshot("2026-11-22.1").unwrap().unwrap();
        assert_ne!(first.config_hash, second.config_hash);
        assert!(second.config_json.contains("\"two\""));
    }

    #[test]
    fn older_version_and_broken_file_keep_the_old_snapshot() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("rules.toml");
        write(&path, "2026-11-22.2", "");
        let db = Db::open_in_memory().unwrap();
        let state = RulesState::load(&db, Some(&path)).unwrap();
        let before = state.current();

        write(&path, "2026-11-22.1", "");
        let error = state.reload(&db, "test").unwrap_err();
        assert!(error.to_string().contains("must not decrease"));
        assert_eq!(state.current().version, "2026-11-22.2");

        std::fs::write(&path, "[rules]\nversion =\n").unwrap();
        assert!(state.reload(&db, "test").is_err());
        assert_eq!(state.current().version, before.version);
        assert_eq!(state.current().config_hash, before.config_hash);
    }

    #[test]
    fn reload_if_changed_skips_unchanged_and_picks_up_new_version() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("rules.toml");
        write(&path, "2026-11-22.1", "");
        let db = Db::open_in_memory().unwrap();
        let state = RulesState::load(&db, Some(&path)).unwrap();
        assert_eq!(
            state.reload_if_changed(&db, "watch").unwrap(),
            ReloadOutcome::Unchanged
        );

        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&path, "2026-11-22.2", "");
        assert_eq!(
            state.reload_if_changed(&db, "watch").unwrap(),
            ReloadOutcome::Reloaded
        );
        assert_eq!(state.current().version, "2026-11-22.2");
        assert_eq!(
            state.reload_if_changed(&db, "watch").unwrap(),
            ReloadOutcome::Unchanged
        );
    }

    #[test]
    fn missing_rules_file_refuses_to_start() {
        let db = Db::open_in_memory().unwrap();
        let error = RulesState::load(&db, None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("service.rules_file is not configured"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn mock_only_default_still_records_snapshot() {
        let db = Db::open_in_memory().unwrap();
        let state = RulesState::load_without_file(&db).unwrap();
        assert_eq!(state.current().version, "default");
        assert_eq!(
            state.reload_if_changed(&db, "watch").unwrap(),
            ReloadOutcome::Skipped
        );
        assert!(db.rule_snapshot("default").unwrap().is_some());
    }

    #[test]
    fn restart_refuses_same_version_with_different_content() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("rules.toml");
        write(
            &path,
            "2026-11-22.1",
            "[rules.blacklist]\ntags = [\"one\"]\n",
        );
        let db = Db::open_in_memory().unwrap();
        let first = RulesState::load(&db, Some(&path)).unwrap();
        let hash = first.current().config_hash.clone();
        drop(first);

        write(
            &path,
            "2026-11-22.1",
            "[rules.blacklist]\ntags = [\"two\"]\n",
        );
        let error = RulesState::load(&db, Some(&path)).unwrap_err();
        assert!(error.to_string().contains("different config_hash"));
        let snapshot = db.rule_snapshot("2026-11-22.1").unwrap().unwrap();
        assert_eq!(snapshot.config_hash, hash);
        assert!(snapshot.config_json.contains("\"one\""));
    }

    #[test]
    fn empty_tag_blacklist_still_loads_and_is_visible() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("rules.toml");
        write(&path, "2026-11-22.1", "");
        let db = Db::open_in_memory().unwrap();
        let state = RulesState::load(&db, Some(&path)).unwrap();
        assert!(state.current().config.tag_blacklist_is_empty());
        assert!(db.rule_snapshot("2026-11-22.1").unwrap().is_some());
    }
}
