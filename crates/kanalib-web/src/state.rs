use std::{path::PathBuf, sync::Arc, time::Duration};

#[derive(Clone, Debug)]
pub struct AppState {
    pub db_path: Arc<PathBuf>,
    pub media_root: Arc<PathBuf>,
    pub web_root: Option<Arc<PathBuf>>,
    pub writes: Arc<WriteConfig>,
    pub version: &'static str,
}

impl AppState {
    pub fn new(db_path: PathBuf, media_root: PathBuf, web_root: Option<PathBuf>) -> Self {
        Self {
            db_path: Arc::new(db_path),
            media_root: Arc::new(media_root),
            web_root: web_root.map(Arc::new),
            writes: Arc::new(WriteConfig::default()),
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    pub fn with_writes(mut self, writes: WriteConfig) -> Self {
        self.writes = Arc::new(writes);
        self
    }
}

/// 私网写路由与原创回源配置。
///
/// 默认全空：未配置时写接口返回 503、原图回源返回 404，公开 listener 照常只读。
#[derive(Clone, Debug)]
pub struct WriteConfig {
    /// box → 网页批次签名共享密钥。
    pub ingest_secret: Option<Arc<str>>,
    /// box 原图服务根地址，例如 `http://10.126.126.3:8789`；私网 listener
    /// 用它反代 `/original/{id}`，地址不会出现在客户端可见的 `Location` 里。
    pub box_origin: Option<Arc<str>>,
    /// 网页签发 box 原图 URL 的媒体密钥。
    pub media_signing_key: Option<Arc<str>>,
    /// 桌面端上传单文件上限。
    pub upload_limit_bytes: u64,
    /// 原图签名 URL 有效期。
    pub signed_url_ttl: Duration,
}

impl Default for WriteConfig {
    fn default() -> Self {
        Self {
            ingest_secret: None,
            box_origin: None,
            media_signing_key: None,
            upload_limit_bytes: 32 * 1024 * 1024,
            signed_url_ttl: Duration::from_secs(300),
        }
    }
}
