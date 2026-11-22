//! `ingest.toml` 主配置与密钥文件读取。
//!
//! 服务参数改完要重启；规则文件与凭据只在运行时读取，不进入数据库明文。

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IngestConfig {
    pub service: ServiceConfig,
    pub source: SourceConfig,
    pub download: DownloadSettings,
    pub tagging: TaggingSettings,
    pub push: PushSettings,
    pub media: MediaSettings,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServiceConfig {
    pub db: PathBuf,
    pub media_dir: PathBuf,
    pub bind: SocketAddr,
    pub public_base: Option<String>,
    pub rules_file: Option<PathBuf>,
    pub sync_interval_secs: u64,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            db: PathBuf::from("ingest.db"),
            media_dir: PathBuf::from("media"),
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8790),
            public_base: None,
            rules_file: None,
            sync_interval_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourceConfig {
    pub pixiv: PixivSettings,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PixivSettings {
    pub enabled: bool,
    pub bookmark_users: Vec<String>,
    /// 直接提供 access token 的文件；与 refresh token 二选一。
    pub access_token_file: Option<PathBuf>,
    /// OAuth refresh token 文件；轮换后会原子写回。
    pub refresh_token_file: Option<PathBuf>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub user_agent: Option<String>,
    /// 文档里的 keyring 引用；当前版本只支持文件，留作显式报错。
    pub credential_ref: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DownloadSettings {
    pub concurrency: usize,
    pub retry_max: u32,
    pub timeout_secs: u64,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            concurrency: 3,
            retry_max: 5,
            timeout_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaggingSettings {
    pub backend: Option<String>,
    pub endpoint: Option<String>,
    pub concurrency: usize,
    pub timeout_secs: u64,
}

impl Default for TaggingSettings {
    fn default() -> Self {
        Self {
            backend: None,
            endpoint: None,
            concurrency: 2,
            timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PushSettings {
    pub web_base: Option<String>,
    pub secret_file: Option<PathBuf>,
    pub batch_size: usize,
    pub max_attempts: u32,
    pub backoff_base_secs: u64,
    pub backoff_max_secs: u64,
    pub rules_version: Option<String>,
}

impl Default for PushSettings {
    fn default() -> Self {
        Self {
            web_base: None,
            secret_file: None,
            batch_size: 100,
            max_attempts: 5,
            backoff_base_secs: 5,
            backoff_max_secs: 600,
            rules_version: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MediaSettings {
    /// 原图签名共享密钥文件；与网页 `--media-signing-key` 相同。
    pub signing_key_file: Option<PathBuf>,
    /// 覆盖 `service.bind`，只绑 EasyTier 虚拟 IP。
    pub bind: Option<SocketAddr>,
}

pub fn load(path: Option<&Path>) -> Result<IngestConfig> {
    let Some(path) = path else {
        return Ok(IngestConfig::default());
    };
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading config {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))
}

/// 读取密钥文件；Unix 下拒绝 group/other 可读的权限。
pub fn read_secret(path: &Path) -> Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)
            .with_context(|| format!("reading secret metadata {}", path.display()))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            bail!(
                "secret file {} must not be group/other accessible (mode {:o})",
                path.display(),
                mode
            );
        }
    }
    let value = std::fs::read_to_string(path)
        .with_context(|| format!("reading secret {}", path.display()))?;
    let value = value.trim();
    if value.is_empty() {
        bail!("secret file {} is empty", path.display());
    }
    Ok(value.to_string())
}

/// 原子写入 refresh token；Unix 下固定 0600。
pub fn write_secret(path: &Path, value: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, value)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temp, path)?;
    Ok(())
}

impl PushSettings {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(30)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_config_shape_parses() {
        let config: IngestConfig = toml::from_str(
            r#"
[service]
bind = "10.126.126.3:8790"
db = "/var/lib/kanalib-ingest/ingest.db"
media_dir = "/var/lib/kanalib-ingest/media"
public_base = "http://kanalib-box.et.net:8790"
rules_file = "/etc/kanalib/rules.toml"

[source.pixiv]
enabled = true
bookmark_users = ["123"]
access_token_file = "/etc/kanalib/pixiv-token"

[download]
concurrency = 2

[push]
web_base = "http://kanalib-web.et.net:8788"
secret_file = "/etc/kanalib/ingest-hmac"
batch_size = 50

[media]
signing_key_file = "/etc/kanalib/media-key"
"#,
        )
        .expect("config");
        assert!(config.source.pixiv.enabled);
        assert_eq!(config.download.concurrency, 2);
        assert_eq!(config.push.batch_size, 50);
        assert_eq!(config.service.sync_interval_secs, 60);
    }

    #[test]
    fn missing_config_uses_safe_defaults() {
        let config = load(None).expect("defaults");
        assert!(!config.source.pixiv.enabled);
        assert_eq!(config.service.bind.port(), 8790);
        assert!(config.service.bind.ip().is_loopback());
    }
}
