//! Immich 只读客户端与增量同步（F-035 第一阶段）。
//!
//! 只允许读操作：`GET /api/server/*`、`POST /api/search/metadata`（只读查询，
//! 不修改服务端数据）与 `GET /api/assets/{id}/thumbnail`。不调用任何写回、
//! 删除、上传或原图下载端点。API key 只放在 `x-api-key` 请求头，不写日志。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::ACCEPT;
use serde::Deserialize;
use serde_json::Value;

use crate::sources::{
    Credentials, Source, SourceError, SourceErrorKind, SourceInfo, SourceKind, SourceStatus,
    SourceSyncContext, SourceSyncReport, immich_virtual_path,
};

/// 常见 Immich 版本端点，按新旧顺序探测；失败原因全部记录下来。
const VERSION_ENDPOINTS: &[&str] = &[
    "api/server/about",
    "api/server/version",
    "api/server-info/version",
    "api/server-info",
];
const DEFAULT_PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 10_000;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_THUMB_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub struct ImmichConfig {
    #[serde(default = "default_page_size")]
    pub page_size: usize,
    #[serde(default = "default_thumbnail_size")]
    pub thumbnail_size: String,
}

fn default_page_size() -> usize {
    DEFAULT_PAGE_SIZE
}

fn default_thumbnail_size() -> String {
    "thumbnail".to_string()
}

impl Default for ImmichConfig {
    fn default() -> Self {
        Self {
            page_size: DEFAULT_PAGE_SIZE,
            thumbnail_size: default_thumbnail_size(),
        }
    }
}

impl ImmichConfig {
    pub fn from_json(value: &str) -> Self {
        let mut config = serde_json::from_str::<Self>(value).unwrap_or_default();
        config.page_size = config.page_size.clamp(1, 1000);
        if config.thumbnail_size.trim().is_empty() {
            config.thumbnail_size = default_thumbnail_size();
        }
        config
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// 版本探测结果；`failures` 记录被跳过的端点及原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImmichVersion {
    pub endpoint: String,
    pub version: String,
    pub failures: Vec<String>,
}

/// 已解析的 Immich 来源。
pub struct ImmichSource {
    info: SourceInfo,
    config: ImmichConfig,
    base: reqwest::Url,
    timeout: Duration,
    connect_timeout: Duration,
    last: Mutex<LastState>,
}

#[derive(Clone, Debug, Default)]
struct LastState {
    online: bool,
    error: Option<SourceError>,
}

impl ImmichSource {
    pub fn new(info: SourceInfo, config: ImmichConfig) -> Result<Self, SourceError> {
        let base = normalize_base_url(&info.locator)?;
        Ok(Self {
            info,
            config,
            base,
            timeout: DEFAULT_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            last: Mutex::new(LastState {
                online: true,
                error: None,
            }),
        })
    }

    pub fn from_info(info: SourceInfo) -> Result<Self, SourceError> {
        Self::new(info, ImmichConfig::default())
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self.timeout = self.timeout.max(Duration::from_millis(1));
        self
    }

    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout.max(Duration::from_millis(1));
        self
    }

    pub fn config(&self) -> &ImmichConfig {
        &self.config
    }

    pub fn base_url(&self) -> &str {
        self.base.as_str()
    }

    fn client(&self) -> Result<Client, SourceError> {
        Client::builder()
            .timeout(self.timeout)
            .connect_timeout(self.connect_timeout)
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent(format!("Kanalib/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
                SourceError::new(
                    SourceErrorKind::Local,
                    format!("创建 HTTP 客户端失败：{error}"),
                )
            })
    }

    fn api_url(&self, path: &str) -> Result<reqwest::Url, SourceError> {
        let mut url = self.base.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|_| {
                SourceError::new(SourceErrorKind::Protocol, "来源地址不是有效的 HTTP URL")
            })?;
            segments.pop_if_empty();
            for segment in path.split('/').filter(|segment| !segment.is_empty()) {
                segments.push(segment);
            }
        }
        Ok(url)
    }

    fn thumbnail_url(&self, asset_id: &str) -> Result<reqwest::Url, SourceError> {
        let mut url = self.base.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|_| {
                SourceError::new(SourceErrorKind::Protocol, "来源地址不是有效的 HTTP URL")
            })?;
            segments.pop_if_empty();
            segments
                .push("api")
                .push("assets")
                .push(asset_id)
                .push("thumbnail");
        }
        url.query_pairs_mut()
            .append_pair("size", &self.config.thumbnail_size);
        Ok(url)
    }

    fn set_last(&self, online: bool, error: Option<SourceError>) {
        if let Ok(mut last) = self.last.lock() {
            last.online = online;
            last.error = error;
        }
    }

    fn api_key(&self, credentials: &Credentials) -> Result<String, SourceError> {
        credentials
            .api_key(&self.info.credential_account())
            .map_err(SourceError::from_anyhow)?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| SourceError::credential("尚未保存 Immich API key"))
    }

    fn search_page(
        &self,
        client: &Client,
        api_key: &str,
        page: usize,
        updated_after: Option<&str>,
    ) -> Result<(Vec<ImmichAsset>, Option<usize>, Option<usize>), SourceError> {
        let url = self.api_url("api/search/metadata")?;
        let mut body = serde_json::json!({
            "page": page,
            "size": self.config.page_size,
            "withExif": true,
            "type": "IMAGE",
        });
        if let Some(updated_after) = updated_after {
            body["updatedAfter"] = Value::String(updated_after.to_string());
        }
        let response = client
            .post(url.clone())
            .header("x-api-key", api_key)
            .header(ACCEPT, "application/json")
            .json(&body)
            .send()
            .map_err(|error| transport_error(error, &url))?;
        let response = ensure_success(response, &url)?;
        let value: Value = response.json().map_err(|error| {
            SourceError::new(
                SourceErrorKind::Protocol,
                format!("解析搜索响应失败：{error}"),
            )
            .with_endpoint(url.as_str())
        })?;
        parse_search_value(&value).map_err(|message| {
            SourceError::new(SourceErrorKind::Protocol, message).with_endpoint(url.as_str())
        })
    }

    fn download_thumbnail(
        &self,
        client: &Client,
        api_key: &str,
        asset_id: &str,
        target: &Path,
    ) -> Result<(), SourceError> {
        let url = self.thumbnail_url(asset_id)?;
        let response = client
            .get(url.clone())
            .header("x-api-key", api_key)
            .send()
            .map_err(|error| transport_error(error, &url))?;
        let response = ensure_success(response, &url)?;
        let bytes = response.bytes().map_err(|error| {
            SourceError::new(
                SourceErrorKind::Disconnected,
                format!("读取缩略图响应失败：{error}"),
            )
            .with_endpoint(url.as_str())
        })?;
        if bytes.is_empty() || bytes.len() > MAX_THUMB_BYTES {
            return Err(SourceError::new(
                SourceErrorKind::Protocol,
                format!("缩略图大小异常：{} 字节", bytes.len()),
            )
            .with_endpoint(url.as_str()));
        }
        if !looks_like_image(&bytes) {
            return Err(SourceError::new(
                SourceErrorKind::Protocol,
                "缩略图响应不是支持的图片格式",
            )
            .with_endpoint(url.as_str()));
        }
        write_atomic(target, &bytes).map_err(|error| {
            SourceError::new(
                SourceErrorKind::Local,
                format!("写入缩略图缓存失败：{error}"),
            )
            .with_endpoint(target.display().to_string())
        })
    }

    fn sync_inner(
        &self,
        context: &SourceSyncContext<'_>,
        credentials: &Credentials,
    ) -> Result<SourceSyncReport, SourceError> {
        let source_id = self
            .info
            .id
            .ok_or_else(|| SourceError::new(SourceErrorKind::Local, "远端来源缺少数据库 id"))?;
        let api_key = self.api_key(credentials)?;
        let client = self.client()?;
        let version = probe_version_with_client(&client, self.base.as_str(), &api_key)?;
        log::info!(
            "immich source {} version {} via {}",
            source_id,
            version.version,
            version.endpoint
        );

        let existing_cursor = self.info.sync_cursor.clone();
        let mut report = SourceSyncReport {
            source_id,
            ..Default::default()
        };
        let mut cursor: Option<(u64, String)> = None;
        let mut page = 1usize;
        loop {
            if context
                .cancel
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
            {
                report.cancelled = true;
                break;
            }
            let (items, total, next_page) =
                self.search_page(&client, &api_key, page, existing_cursor.as_deref())?;
            if let Some(total) = total {
                report.discovered = total;
            }
            if items.is_empty() {
                break;
            }
            let count = items.len();
            for item in &items {
                let Some(parsed) = ParsedItem::parse(item) else {
                    report.skipped += 1;
                    continue;
                };
                note_cursor(&mut cursor, parsed.updated_at.as_deref());
                if parsed.sha1.is_none() && parsed.checksum_error.is_some() {
                    report.failed += 1;
                    report
                        .warnings
                        .push(format!("资产 {} 的 checksum 无法解析为 SHA1", parsed.id));
                }
                match self.sync_item(context, &client, &api_key, &parsed) {
                    Ok(outcome) => {
                        if outcome.imported {
                            report.imported += 1;
                        } else {
                            report.updated += 1;
                        }
                        if outcome.thumbnail_downloaded {
                            report.thumbnails_downloaded += 1;
                        }
                        if outcome.thumbnail_reused {
                            report.thumbnails_reused += 1;
                        }
                        if let Some(warning) = outcome.warning {
                            report
                                .warnings
                                .push(format!("资产 {} 缩略图：{warning}", parsed.id));
                        }
                    }
                    Err(error) => {
                        report.failed += 1;
                        report
                            .warnings
                            .push(format!("资产 {} 入库失败：{error}", parsed.id));
                    }
                }
            }
            let next = next_page.unwrap_or(page + 1);
            if count < self.config.page_size || next <= page {
                break;
            }
            if page >= MAX_PAGES {
                return Err(SourceError::new(
                    SourceErrorKind::Protocol,
                    "Immich 分页超过安全上限",
                ));
            }
            page = next;
        }

        if report.cancelled {
            return Err(SourceError::cancelled());
        }
        let cursor = cursor.map(|(_, raw)| raw).or(existing_cursor);
        let now = unix_now();
        context
            .db
            .update_source_sync(source_id, cursor.as_deref(), now)
            .map_err(SourceError::from_anyhow)?;
        report.cursor = cursor;
        report.last_sync_at = Some(now);
        Ok(report)
    }

    fn sync_item(
        &self,
        context: &SourceSyncContext<'_>,
        client: &Client,
        api_key: &str,
        item: &ParsedItem,
    ) -> Result<ItemOutcome, SourceError> {
        let virtual_path = immich_virtual_path(&item.id);
        let stored = context
            .db
            .stored_meta(&virtual_path)
            .map_err(SourceError::from_anyhow)?;
        let target =
            crate::state::thumb_target(&virtual_path, context.cache_dir, item.mtime, item.bytes, 1);
        let reusable = stored.as_ref().is_some_and(|meta| {
            meta.mtime == item.mtime
                && meta.bytes == item.bytes
                && !meta.thumb.is_empty()
                && Path::new(&meta.thumb).exists()
        });
        let mut thumb = stored
            .as_ref()
            .and_then(|meta| (!meta.thumb.is_empty()).then(|| PathBuf::from(&meta.thumb)));
        let mut thumbnail_downloaded = false;
        let mut warning = None;
        if !reusable {
            match self.download_thumbnail(client, api_key, &item.id, &target) {
                Ok(()) => {
                    context
                        .db
                        .record_thumb_source(&target, &virtual_path, item.mtime, item.bytes, 1)
                        .map_err(SourceError::from_anyhow)?;
                    thumb = Some(target);
                    thumbnail_downloaded = true;
                }
                Err(error) => {
                    warning = Some(error.to_string());
                }
            }
        }
        context
            .db
            .upsert_remote_asset(
                self.info.id.unwrap_or_default(),
                &virtual_path.to_string_lossy(),
                &self.info.locator,
                &item.name,
                &item.mime,
                item.width,
                item.height,
                item.bytes,
                item.mtime,
                item.captured_at,
                item.sha1.as_deref(),
                thumb.as_deref(),
            )
            .map_err(SourceError::from_anyhow)?;
        if let Some(ref warning) = warning {
            log::warn!("immich thumbnail {}: {warning}", item.id);
        }
        Ok(ItemOutcome {
            imported: stored.is_none(),
            thumbnail_downloaded,
            thumbnail_reused: reusable,
            warning,
        })
    }
}

impl Source for ImmichSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn status(&self) -> SourceStatus {
        let last = self.last.lock().unwrap().clone();
        SourceStatus {
            kind: SourceKind::Immich,
            locator: self.info.locator.clone(),
            online: last.online,
            asset_count: 0,
            last_sync_at: self.info.last_sync_at,
            last_error: last.error,
        }
    }

    fn test_connection(&self, credentials: &Credentials) -> Result<SourceStatus, SourceError> {
        let api_key = self.api_key(credentials)?;
        let client = self.client()?;
        match probe_version_with_client(&client, self.base.as_str(), &api_key) {
            Ok(_) => {
                self.set_last(true, None);
                Ok(self.status())
            }
            Err(error) => {
                self.set_last(false, Some(error.clone()));
                Err(error)
            }
        }
    }

    fn sync(
        &self,
        context: &SourceSyncContext<'_>,
        credentials: &Credentials,
    ) -> Result<SourceSyncReport, SourceError> {
        match self.sync_inner(context, credentials) {
            Ok(report) => {
                self.set_last(true, None);
                Ok(report)
            }
            Err(error) => {
                if !matches!(error.kind, SourceErrorKind::Cancelled) {
                    self.set_last(false, Some(error.clone()));
                }
                Err(error)
            }
        }
    }
}

#[derive(Debug)]
struct ItemOutcome {
    imported: bool,
    thumbnail_downloaded: bool,
    thumbnail_reused: bool,
    warning: Option<String>,
}

/// 探测 Immich 版本：按新旧端点顺序尝试，401/403 立即失败，连接失败不再重试。
pub fn probe_version(
    base_url: &str,
    api_key: &str,
    timeout: Duration,
) -> Result<ImmichVersion, SourceError> {
    let base = normalize_base_url(base_url)?;
    let client = Client::builder()
        .timeout(timeout.max(Duration::from_millis(1)))
        .connect_timeout(timeout.max(Duration::from_millis(1)))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent(format!("Kanalib/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| {
            SourceError::new(
                SourceErrorKind::Local,
                format!("创建 HTTP 客户端失败：{error}"),
            )
        })?;
    probe_version_with_client(&client, base.as_str(), api_key)
}

fn probe_version_with_client(
    client: &Client,
    base_url: &str,
    api_key: &str,
) -> Result<ImmichVersion, SourceError> {
    let base = reqwest::Url::parse(base_url)
        .map_err(|_| SourceError::new(SourceErrorKind::Protocol, "来源地址不是有效的 URL"))?;
    let mut failures = Vec::new();
    for endpoint in VERSION_ENDPOINTS {
        let Some(url) = join_api_path(&base, endpoint) else {
            failures.push(format!("{endpoint}: 无法拼接 URL"));
            continue;
        };
        let response = match client
            .get(url.clone())
            .header("x-api-key", api_key)
            .header(ACCEPT, "application/json")
            .send()
        {
            Ok(response) => response,
            Err(error) => {
                let mapped = transport_error(error, &url);
                if mapped.is_transport() {
                    // 服务不可达/超时：继续探测其他端点没有意义。
                    return Err(SourceError {
                        failures: {
                            failures.push(format!("{endpoint}: {}", mapped.message));
                            failures
                        },
                        ..mapped
                    });
                }
                failures.push(format!("{endpoint}: {}", mapped.message));
                continue;
            }
        };
        if matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            return Err(SourceError::unauthorized(url.as_str()));
        }
        if !response.status().is_success() {
            failures.push(format!("{endpoint}: HTTP {}", response.status().as_u16()));
            continue;
        }
        let value: Value = match response.json() {
            Ok(value) => value,
            Err(error) => {
                failures.push(format!("{endpoint}: 响应不是 JSON（{error}）"));
                continue;
            }
        };
        match version_from_value(&value) {
            Some(version) => {
                return Ok(ImmichVersion {
                    endpoint: endpoint.to_string(),
                    version,
                    failures,
                });
            }
            None => failures.push(format!("{endpoint}: 响应缺少版本字段")),
        }
    }
    Err(SourceError {
        kind: SourceErrorKind::Protocol,
        message: "无法探测 Immich 版本（已尝试常见端点）".to_string(),
        endpoint: Some(base.as_str().to_string()),
        status: None,
        failures,
    })
}

fn version_from_value(value: &Value) -> Option<String> {
    if let Some(version) = value.get("version").and_then(Value::as_str) {
        return Some(version.trim().trim_start_matches('v').to_string());
    }
    let major = value.get("major").and_then(Value::as_u64)?;
    let minor = value.get("minor").and_then(Value::as_u64)?;
    let patch = value.get("patch").and_then(Value::as_u64)?;
    Some(format!("{major}.{minor}.{patch}"))
}

fn normalize_base_url(input: &str) -> Result<reqwest::Url, SourceError> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(SourceError::new(
            SourceErrorKind::Protocol,
            "Immich 地址不能为空",
        ));
    }
    let mut url = reqwest::Url::parse(trimmed)
        .map_err(|_| SourceError::new(SourceErrorKind::Protocol, "Immich 地址不是有效的 URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(SourceError::new(
            SourceErrorKind::Protocol,
            "Immich 地址只支持 http/https",
        ));
    }
    if url.host_str().is_none() {
        return Err(SourceError::new(
            SourceErrorKind::Protocol,
            "Immich 地址缺少主机名",
        ));
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn join_api_path(base: &reqwest::Url, path: &str) -> Option<reqwest::Url> {
    let mut url = base.clone();
    {
        let mut segments = url.path_segments_mut().ok()?;
        segments.pop_if_empty();
        for segment in path.split('/').filter(|segment| !segment.is_empty()) {
            segments.push(segment);
        }
    }
    Some(url)
}

fn ensure_success(
    response: reqwest::blocking::Response,
    url: &reqwest::Url,
) -> Result<reqwest::blocking::Response, SourceError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let kind = match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            return Err(SourceError::unauthorized(url.as_str()));
        }
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => SourceErrorKind::Timeout,
        _ => SourceErrorKind::Http,
    };
    Err(SourceError {
        kind,
        message: format!("Immich 返回 HTTP {}", status.as_u16()),
        endpoint: Some(url.as_str().to_string()),
        status: Some(status.as_u16()),
        failures: Vec::new(),
    })
}

fn transport_error(error: reqwest::Error, url: &reqwest::Url) -> SourceError {
    let kind = if error.is_timeout() {
        SourceErrorKind::Timeout
    } else if error.is_connect() || error.is_body() || error.is_decode() {
        SourceErrorKind::Disconnected
    } else {
        SourceErrorKind::Http
    };
    let message = if kind == SourceErrorKind::Timeout {
        "Immich 请求超时".to_string()
    } else if kind == SourceErrorKind::Disconnected {
        format!("无法连接 Immich：{}", error.without_url())
    } else {
        format!("Immich 请求失败：{}", error.without_url())
    };
    SourceError {
        kind,
        message,
        endpoint: Some(url.as_str().to_string()),
        status: None,
        failures: Vec::new(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImmichAsset {
    #[serde(default)]
    id: String,
    #[serde(rename = "type", default)]
    media_type: String,
    #[serde(default)]
    original_file_name: String,
    #[serde(default)]
    original_path: String,
    #[serde(default)]
    original_mime_type: String,
    #[serde(default)]
    file_created_at: Option<String>,
    #[serde(default)]
    file_modified_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    checksum: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    file_size_in_byte: Option<u64>,
    #[serde(default)]
    exif_info: Option<ImmichExif>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImmichExif {
    #[serde(default)]
    file_size_in_byte: Option<u64>,
    #[serde(default)]
    exif_image_width: Option<u32>,
    #[serde(default)]
    exif_image_height: Option<u32>,
    #[serde(default)]
    date_time_original: Option<String>,
}

#[derive(Debug)]
struct ParsedItem {
    id: String,
    name: String,
    mime: String,
    width: u32,
    height: u32,
    bytes: u64,
    mtime: u64,
    captured_at: Option<u64>,
    updated_at: Option<String>,
    sha1: Option<String>,
    checksum_error: Option<String>,
}

impl ParsedItem {
    fn parse(item: &ImmichAsset) -> Option<Self> {
        if item.id.is_empty() {
            return None;
        }
        if !item.media_type.is_empty() && !item.media_type.eq_ignore_ascii_case("IMAGE") {
            return None;
        }
        let exif = item.exif_info.clone().unwrap_or_default();
        let name = if !item.original_file_name.is_empty() {
            item.original_file_name.clone()
        } else {
            Path::new(&item.original_path)
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| item.id.clone())
        };
        let modified = item.file_modified_at.as_deref().and_then(parse_timestamp);
        let updated = item.updated_at.as_deref().and_then(parse_timestamp);
        let created = item.file_created_at.as_deref().and_then(parse_timestamp);
        let captured = exif
            .date_time_original
            .as_deref()
            .and_then(parse_timestamp)
            .or(created);
        let mtime = modified.or(updated).or(created).unwrap_or(0);
        let (sha1, checksum_error) = match item.checksum.as_deref() {
            Some(raw) if !raw.trim().is_empty() => match parse_checksum(raw) {
                Some(value) => (Some(value), None),
                None => (
                    None,
                    Some("checksum 既不是 base64 SHA1 也不是 40 位 hex".to_string()),
                ),
            },
            _ => (None, Some("缺少 checksum".to_string())),
        };
        Some(Self {
            id: item.id.clone(),
            name,
            mime: item.original_mime_type.clone(),
            width: exif.exif_image_width.or(item.width).unwrap_or(0),
            height: exif.exif_image_height.or(item.height).unwrap_or(0),
            bytes: exif
                .file_size_in_byte
                .or(item.file_size_in_byte)
                .unwrap_or(0),
            mtime,
            captured_at: captured,
            updated_at: item.updated_at.clone(),
            sha1,
            checksum_error,
        })
    }
}

fn parse_search_value(
    value: &Value,
) -> Result<(Vec<ImmichAsset>, Option<usize>, Option<usize>), String> {
    if let Some(array) = value.as_array() {
        let items = serde_json::from_value::<Vec<ImmichAsset>>(Value::Array(array.clone()))
            .map_err(|error| format!("解析资产列表失败：{error}"))?;
        let total = Some(items.len());
        return Ok((items, total, None));
    }
    let assets = value.get("assets").unwrap_or(value);
    let items_value = assets
        .get("items")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let items = serde_json::from_value::<Vec<ImmichAsset>>(items_value)
        .map_err(|error| format!("解析资产列表失败：{error}"))?;
    let total = assets
        .get("total")
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    let next_page = assets.get("nextPage").and_then(|value| {
        value
            .as_str()
            .and_then(|text| text.trim().parse::<usize>().ok())
            .or_else(|| value.as_u64().map(|number| number as usize))
    });
    Ok((items, total, next_page))
}

fn note_cursor(cursor: &mut Option<(u64, String)>, value: Option<&str>) {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return;
    };
    let timestamp = parse_timestamp(value).unwrap_or(0);
    match cursor {
        Some((current, _)) if *current >= timestamp => {}
        _ => *cursor = Some((timestamp, value.to_string())),
    }
}

/// Immich checksum：通常是 SHA1 的 base64（20 字节），也兼容 40 位 hex。
pub fn parse_checksum(raw: &str) -> Option<String> {
    let value = raw.trim();
    if is_hex_sha1(value) {
        return Some(value.to_ascii_lowercase());
    }
    for bytes in [
        STANDARD.decode(value).ok(),
        STANDARD_NO_PAD.decode(value).ok(),
        URL_SAFE.decode(value).ok(),
        URL_SAFE_NO_PAD.decode(value).ok(),
    ]
    .into_iter()
    .flatten()
    {
        if bytes.len() == 20 {
            return Some(hex_lower(&bytes));
        }
        if bytes.len() == 40
            && let Ok(text) = std::str::from_utf8(&bytes)
            && is_hex_sha1(text)
        {
            return Some(text.to_ascii_lowercase());
        }
    }
    None
}

fn is_hex_sha1(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 解析 Immich 的 ISO8601 时间戳为 Unix 秒；无法解析返回 None。
pub fn parse_timestamp(raw: &str) -> Option<u64> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(number) = value.parse::<u64>() {
        return Some(number);
    }
    let (date, rest) = if let Some((date, rest)) = value.split_once('T') {
        (date, rest)
    } else if let Some((date, rest)) = value.split_once(' ') {
        (date, rest)
    } else {
        (value, "")
    };
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, offset) = split_timezone(rest);
    let mut clock_parts = clock.split(':');
    let hour: u64 = clock_parts.next().unwrap_or("0").parse().ok()?;
    let minute: u64 = clock_parts.next().unwrap_or("0").parse().ok()?;
    let second_part = clock_parts.next().unwrap_or("0");
    let second_text = second_part.split('.').next().unwrap_or("0");
    let second: u64 = second_text.parse().ok()?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let base = days * 86_400 + (hour as i64) * 3_600 + (minute as i64) * 60 + second as i64;
    let base = base - offset;
    u64::try_from(base).ok()
}

fn split_timezone(rest: &str) -> (&str, i64) {
    if let Some(stripped) = rest.strip_suffix('Z') {
        return (stripped, 0);
    }
    if let Some(index) = rest.rfind(['+', '-']) {
        let (clock, zone) = rest.split_at(index);
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let zone = zone.trim_start_matches(['+', '-']);
        let mut parts = zone.split(':');
        let hours: i64 = parts
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let minutes: i64 = parts
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        return (clock, sign * (hours * 3_600 + minutes * 60));
    }
    (rest, 0)
}

/// Howard Hinnant 的 days_from_civil；返回 1970-01-01 起的天数。
pub(super) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = month as i64;
    let day = day as i64;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn looks_like_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8, 0xff])
        || bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"GIF8")
        || (bytes.starts_with(b"RIFF") && bytes.len() > 12 && &bytes[8..12] == b"WEBP")
        || bytes.starts_with(b"BM")
        || bytes.get(4..8).is_some_and(|kind| kind == b"ftyp")
}

fn write_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let stem = target
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "thumb".to_string());
    let temp = target.with_file_name(format!(".{stem}.{}-{nonce}.tmp", std::process::id()));
    std::fs::write(&temp, bytes)?;
    if let Err(error) = std::fs::rename(&temp, target) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
