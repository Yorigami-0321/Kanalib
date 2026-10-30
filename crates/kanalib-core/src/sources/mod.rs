//! 统一来源抽象：本地根目录与只读远端来源（F-035）。
//!
//! 本模块只负责「来源」这一层：来源身份、只读能力、凭据与同步报告。
//! 本地来源的磁盘遍历/监听/清理仍由 [`crate::state`] 的扫描线程负责；
//! [`LocalSource`] 只是把现有 roots 包装成统一的只读视图，行为与数据不变。
//!
//! 远端资产使用稳定虚拟路径 `immich://<asset_id>`，`assets.folder` 存放来源
//! locator（base URL）。这样 `load_library`/搜索/标签/去重可以按 folder 直接
//! 覆盖远端资产，而本地扫描、监听、缺失清理仍只遍历 `Shared::roots`。

mod immich;
mod local;
mod webdav;

pub use immich::{
    ImmichConfig, ImmichSource, ImmichVersion, parse_checksum, parse_timestamp, probe_version,
};
pub use local::LocalSource;
pub use webdav::{DavEntry, WebDavConfig, WebDavSource, parse_propfind};

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::db::SourceRecord;

/// Windows Credential Manager 中统一使用的 service 名；API key 只存在这里。
pub const KEYRING_SERVICE: &str = "Kanalib";
/// Immich 资产虚拟路径前缀；`immich://<asset_id>`。
pub const IMMICH_PATH_PREFIX: &str = "immich://";
/// WebDAV 资产虚拟路径前缀；`webdav://<source_id>/<相对路径>`。
pub const WEBDAV_PATH_PREFIX: &str = "webdav://";

/// 来源类型。数据库 `sources.kind` 存小写字符串。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Local,
    Immich,
    #[serde(rename = "webdav")]
    WebDav,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Immich => "immich",
            Self::WebDav => "webdav",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "local" => Some(Self::Local),
            "immich" => Some(Self::Immich),
            "webdav" => Some(Self::WebDav),
            _ => None,
        }
    }

    /// 本地来源可写；所有远端来源在本阶段一律只读。
    pub fn is_read_only(self) -> bool {
        !matches!(self, Self::Local)
    }

    pub fn is_remote(self) -> bool {
        matches!(self, Self::Immich | Self::WebDav)
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 来源的持久身份；`id` 来自 `sources.id`，本地来源由 roots 惰性包装时为空。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub id: Option<i64>,
    pub kind: SourceKind,
    pub name: String,
    /// 本地来源为绝对根目录；远端来源为 base URL。
    pub locator: String,
    pub read_only: bool,
    pub last_sync_at: Option<u64>,
    pub sync_cursor: Option<String>,
}

impl SourceInfo {
    pub fn from_record(record: &SourceRecord) -> Option<Self> {
        let kind = SourceKind::from_db(&record.kind)?;
        Some(Self {
            id: Some(record.id),
            kind,
            name: record.name.clone(),
            locator: record.locator.clone(),
            read_only: record.read_only || kind.is_read_only(),
            last_sync_at: record.last_sync_at,
            sync_cursor: record.sync_cursor.clone(),
        })
    }

    /// `assets.folder` / `load_library` 使用的统一键。
    pub fn folder(&self) -> &str {
        &self.locator
    }

    pub fn is_remote(&self) -> bool {
        self.kind.is_remote()
    }

    /// 凭据账号：kind + locator；不含密钥，允许写入日志。
    pub fn credential_account(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.locator)
    }
}

/// 同步/探测失败的分类；`message` 永远不含 API key。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceErrorKind {
    Unauthorized,
    Timeout,
    Disconnected,
    Http,
    Protocol,
    Credential,
    Local,
    Unsupported,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct SourceError {
    pub kind: SourceErrorKind,
    /// 面向用户的短消息（已脱敏，不含密钥）。
    pub message: String,
    /// 触发的端点（不含查询串中的敏感参数）。
    pub endpoint: Option<String>,
    /// HTTP 状态码（若适用）。
    pub status: Option<u16>,
    /// 版本探测时各候选端点的失败原因，便于诊断 API 差异。
    pub failures: Vec<String>,
}

impl SourceError {
    pub fn new(kind: SourceErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            endpoint: None,
            status: None,
            failures: Vec::new(),
        }
    }

    pub fn unauthorized(endpoint: impl Into<String>) -> Self {
        let endpoint = endpoint.into();
        Self {
            kind: SourceErrorKind::Unauthorized,
            message: format!("认证失败或没有权限：{endpoint}"),
            endpoint: Some(endpoint),
            status: Some(401),
            failures: Vec::new(),
        }
    }

    pub fn credential(message: impl Into<String>) -> Self {
        Self::new(SourceErrorKind::Credential, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(SourceErrorKind::Unsupported, message)
    }

    pub fn cancelled() -> Self {
        Self::new(SourceErrorKind::Cancelled, "同步已取消")
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    pub fn from_anyhow(error: anyhow::Error) -> Self {
        Self::new(SourceErrorKind::Local, format!("{error:#}"))
    }

    pub fn is_transport(&self) -> bool {
        matches!(
            self.kind,
            SourceErrorKind::Timeout | SourceErrorKind::Disconnected
        )
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(endpoint) = &self.endpoint {
            write!(f, "（{endpoint}）")?;
        }
        if !self.failures.is_empty() {
            write!(f, "；候选端点：{}", self.failures.join("；"))?;
        }
        Ok(())
    }
}

impl std::error::Error for SourceError {}

/// 来源运行时状态；供核心 API 与后续 UI 只读使用。
#[derive(Clone, Debug)]
pub struct SourceStatus {
    pub kind: SourceKind,
    pub locator: String,
    pub online: bool,
    pub asset_count: usize,
    pub last_sync_at: Option<u64>,
    pub last_error: Option<SourceError>,
}

impl SourceStatus {
    pub fn ok(info: &SourceInfo, asset_count: usize) -> Self {
        Self {
            kind: info.kind,
            locator: info.locator.clone(),
            online: true,
            asset_count,
            last_sync_at: info.last_sync_at,
            last_error: None,
        }
    }
}

/// 一次来源同步的计数；失败/取消时由调用方决定是否展示。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SourceSyncReport {
    pub source_id: i64,
    /// API 返回的资产总数（含被过滤的类型）。
    pub discovered: usize,
    /// 新入库的远端资产数。
    pub imported: usize,
    /// 已存在但元数据有更新的远端资产数。
    pub updated: usize,
    /// 未变化或类型不是 IMAGE 而跳过的数量。
    pub skipped: usize,
    /// 元数据/缩略图失败但未中断同步的数量。
    pub failed: usize,
    pub thumbnails_downloaded: usize,
    pub thumbnails_reused: usize,
    /// 成功后写回的增量游标（最大 updatedAt）。
    pub cursor: Option<String>,
    pub last_sync_at: Option<u64>,
    pub cancelled: bool,
    /// 已脱敏的警告（如非法 checksum、缩略图失败）；不含密钥。
    pub warnings: Vec<String>,
}

/// 同步所需的本地上下文；网络 IO 不持有 `Shared` 锁。
pub struct SourceSyncContext<'a> {
    pub db: &'a crate::db::Db,
    pub cache_dir: &'a Path,
    pub cancel: Option<&'a std::sync::atomic::AtomicBool>,
}

/// 统一来源接口。同步入口只被后台线程调用，禁止在 GPUI 线程直接执行。
pub trait Source: Send + Sync {
    fn info(&self) -> &SourceInfo;
    fn status(&self) -> SourceStatus;
    fn test_connection(&self, credentials: &Credentials) -> Result<SourceStatus, SourceError>;
    fn sync(
        &self,
        context: &SourceSyncContext<'_>,
        credentials: &Credentials,
    ) -> Result<SourceSyncReport, SourceError>;

    fn is_read_only(&self) -> bool {
        self.info().read_only || self.info().kind.is_read_only()
    }
}

/// 把本地 roots 与数据库中的远端来源合并为统一列表；本地来源 id 为 None。
pub fn source_infos(roots: &[PathBuf], records: &[SourceRecord]) -> Vec<SourceInfo> {
    let mut list = roots
        .iter()
        .map(|root| LocalSource::info_for(root))
        .collect::<Vec<_>>();
    for record in records {
        let Some(info) = SourceInfo::from_record(record) else {
            log::warn!("unknown source kind in database: {}", record.kind);
            continue;
        };
        if info.kind.is_remote() {
            list.push(info);
        }
    }
    list
}

/// 是否为远端虚拟路径；本地扫描/监听/清理必须先排除这类路径。
pub fn is_remote_path(path: &Path) -> bool {
    remote_kind_for_path(path).is_some()
}

pub fn remote_kind_for_path(path: &Path) -> Option<SourceKind> {
    let text = path.to_string_lossy();
    if text.starts_with(IMMICH_PATH_PREFIX) {
        return Some(SourceKind::Immich);
    }
    if text.starts_with(WEBDAV_PATH_PREFIX) {
        return Some(SourceKind::WebDav);
    }
    None
}

pub fn immich_virtual_path(asset_id: &str) -> PathBuf {
    PathBuf::from(format!("{IMMICH_PATH_PREFIX}{asset_id}"))
}

/// WebDAV 虚拟路径带上来源 id，避免不同服务器上的同路径文件互相覆盖。
pub fn webdav_asset_path(source_id: i64, relative: &str) -> PathBuf {
    let relative = relative.trim_matches('/');
    PathBuf::from(format!("{WEBDAV_PATH_PREFIX}{source_id}/{relative}"))
}

pub fn webdav_source_id(path: &Path) -> Option<i64> {
    let text = path.to_string_lossy();
    let rest = text.strip_prefix(WEBDAV_PATH_PREFIX)?;
    rest.split('/').next()?.parse().ok()
}

pub fn webdav_relative_path(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    let rest = text.strip_prefix(WEBDAV_PATH_PREFIX)?;
    let (_, relative) = rest.split_once('/')?;
    (!relative.is_empty()).then(|| relative.to_string())
}

pub fn immich_asset_id(path: &Path) -> Option<String> {
    path.to_string_lossy()
        .strip_prefix(IMMICH_PATH_PREFIX)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// 凭据后端抽象：生产用 Windows Credential Manager，测试注入内存后端。
pub trait CredentialBackend: Send + Sync {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>>;
    fn set(&self, service: &str, account: &str, secret: &str) -> Result<()>;
    fn delete(&self, service: &str, account: &str) -> Result<()>;
}

/// Windows Credential Manager；service 固定为 [`KEYRING_SERVICE`]。
pub struct SystemCredentialBackend;

impl CredentialBackend for SystemCredentialBackend {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(service, account)
            .with_context(|| format!("打开系统凭据失败：{account}"))?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(anyhow!("读取系统凭据失败：{error}")),
        }
    }

    fn set(&self, service: &str, account: &str, secret: &str) -> Result<()> {
        let entry = keyring::Entry::new(service, account)
            .with_context(|| format!("打开系统凭据失败：{account}"))?;
        entry
            .set_password(secret)
            .with_context(|| format!("写入系统凭据失败：{account}"))
    }

    fn delete(&self, service: &str, account: &str) -> Result<()> {
        let entry = keyring::Entry::new(service, account)
            .with_context(|| format!("打开系统凭据失败：{account}"))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(anyhow!("删除系统凭据失败：{error}")),
        }
    }
}

/// 测试/注入用内存后端；不写任何系统凭据。
#[derive(Default)]
pub struct MemoryCredentialBackend {
    entries: Mutex<HashMap<(String, String), String>>,
}

impl MemoryCredentialBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CredentialBackend for MemoryCredentialBackend {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .get(&(service.to_string(), account.to_string()))
            .cloned())
    }

    fn set(&self, service: &str, account: &str, secret: &str) -> Result<()> {
        self.entries.lock().unwrap().insert(
            (service.to_string(), account.to_string()),
            secret.to_string(),
        );
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<()> {
        self.entries
            .lock()
            .unwrap()
            .remove(&(service.to_string(), account.to_string()));
        Ok(())
    }
}

/// 凭据门面。API key 只通过 [`CredentialBackend`] 读写，绝不进数据库、
/// config_json、日志或错误消息。
pub struct Credentials {
    service: String,
    backend: Arc<dyn CredentialBackend>,
}

impl Credentials {
    pub fn system() -> Self {
        Self {
            service: KEYRING_SERVICE.to_string(),
            backend: Arc::new(SystemCredentialBackend),
        }
    }

    pub fn with_backend(service: impl Into<String>, backend: Arc<dyn CredentialBackend>) -> Self {
        Self {
            service: service.into(),
            backend,
        }
    }

    /// 隔离的测试门面：service 带 `-test`，且只用内存后端。
    pub fn in_memory() -> Self {
        Self::with_backend(
            format!("{KEYRING_SERVICE}-test"),
            Arc::new(MemoryCredentialBackend::new()),
        )
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn set_api_key(&self, account: &str, api_key: &str) -> Result<()> {
        self.set_secret(account, api_key)
    }

    pub fn clear_api_key(&self, account: &str) -> Result<()> {
        self.clear_secret(account)
    }

    pub fn has_api_key(&self, account: &str) -> Result<bool> {
        self.has_secret(account)
    }

    pub fn api_key(&self, account: &str) -> Result<Option<String>> {
        self.secret(account)
    }

    /// 通用秘密：Immich 为 API key，WebDAV 为 `user:password`（无冒号按 Bearer token）。
    pub fn set_secret(&self, account: &str, secret: &str) -> Result<()> {
        let secret = secret.trim();
        if secret.is_empty() {
            return self.clear_secret(account);
        }
        self.backend.set(&self.service, account, secret)
    }

    pub fn clear_secret(&self, account: &str) -> Result<()> {
        self.backend.delete(&self.service, account)
    }

    pub fn has_secret(&self, account: &str) -> Result<bool> {
        Ok(self.secret(account)?.is_some())
    }

    pub fn secret(&self, account: &str) -> Result<Option<String>> {
        self.backend.get(&self.service, account)
    }

    /// 用已保存的凭据探测 Immich 版本；不会把密钥写入返回值。
    pub fn test_immich(
        &self,
        info: &SourceInfo,
        timeout: Duration,
    ) -> Result<ImmichVersion, SourceError> {
        let api_key = self
            .api_key(&info.credential_account())
            .map_err(SourceError::from_anyhow)?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| SourceError::credential("尚未保存 Immich API key"))?;
        probe_version(&info.locator, &api_key, timeout)
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("service", &self.service)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
