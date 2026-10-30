//! WebDAV 只读来源（F-035 第二阶段）。
//!
//! 只发送 `PROPFIND`（Depth 0/1）请求，不发送 PUT/DELETE/MOVE/COPY，也不下载
//! original；资产只记录路径、大小、Last-Modified 与 MIME，缩略图由本地后续
//! 处理。ETag 用于目录缓存与 `If-None-Match` 条件请求，Last-Modified 用于
//! 变更检测。凭据只从 `Credentials`（Windows Credential Manager / 测试内存后端）
//! 读取，绝不写入数据库、settings 或日志。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use quick_xml::Reader;
use quick_xml::escape::unescape;
use quick_xml::events::Event;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, IF_MODIFIED_SINCE, IF_NONE_MATCH};
use serde::{Deserialize, Serialize};

use crate::sources::webdav_asset_path;
use crate::sources::{
    Credentials, Source, SourceError, SourceErrorKind, SourceInfo, SourceKind, SourceStatus,
    SourceSyncContext, SourceSyncReport,
};

const DEFAULT_CONCURRENCY: usize = 4;
const MAX_CONCURRENCY: usize = 8;
const DEFAULT_RETRIES: u32 = 2;
const DEFAULT_BACKOFF_MS: u64 = 40;
const MAX_BACKOFF_MS: u64 = 2_000;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEPTH_ONE_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:">
  <d:prop>
    <d:resourcetype/>
    <d:getcontentlength/>
    <d:getlastmodified/>
    <d:getetag/>
    <d:getcontenttype/>
  </d:prop>
</d:propfind>"#;
const DEPTH_ZERO_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:">
  <d:prop><d:resourcetype/><d:getetag/></d:prop>
</d:propfind>"#;

/// WebDAV 同步配置；`config_json` 只保存这些非敏感参数。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebDavConfig {
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default = "default_retries")]
    pub retries: u32,
    #[serde(default = "default_backoff_ms")]
    pub backoff_ms: u64,
}

fn default_concurrency() -> usize {
    DEFAULT_CONCURRENCY
}

fn default_retries() -> u32 {
    DEFAULT_RETRIES
}

fn default_backoff_ms() -> u64 {
    DEFAULT_BACKOFF_MS
}

impl Default for WebDavConfig {
    fn default() -> Self {
        Self {
            concurrency: DEFAULT_CONCURRENCY,
            retries: DEFAULT_RETRIES,
            backoff_ms: DEFAULT_BACKOFF_MS,
        }
    }
}

impl WebDavConfig {
    pub fn from_json(value: &str) -> Self {
        let mut config = serde_json::from_str::<Self>(value).unwrap_or_default();
        config.concurrency = config.concurrency.clamp(1, MAX_CONCURRENCY);
        config.retries = config.retries.min(5);
        config.backoff_ms = config.backoff_ms.min(MAX_BACKOFF_MS);
        config
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    fn backoff(&self, attempt: u32) -> Duration {
        let factor = 1u64 << attempt.min(6);
        Duration::from_millis(self.backoff_ms.saturating_mul(factor).min(MAX_BACKOFF_MS))
    }
}

/// PROPFIND 中的一条记录（含目录自身）；`path` 是相对来源根目录的解码路径。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DavEntry {
    pub href: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<u64>,
    pub etag: Option<String>,
    pub content_type: Option<String>,
}

/// 已解析的目录列表；`self_entry` 通常是第一条（Depth 1 的集合自身）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct CachedDir {
    etag: Option<String>,
    last_modified: Option<u64>,
    entries: Vec<DavEntry>,
}

#[derive(Default, Serialize, Deserialize)]
struct DirectoryCache {
    version: u32,
    dirs: HashMap<String, CachedDir>,
}

pub struct WebDavSource {
    info: SourceInfo,
    config: WebDavConfig,
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

#[derive(Debug)]
struct Listing {
    dir: CachedDir,
    not_modified: bool,
}

impl WebDavSource {
    pub fn new(info: SourceInfo, config: WebDavConfig) -> Result<Self, SourceError> {
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
        Self::new(info, WebDavConfig::default())
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout.max(Duration::from_millis(1));
        self
    }

    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout.max(Duration::from_millis(1));
        self
    }

    pub fn config(&self) -> &WebDavConfig {
        &self.config
    }

    pub fn base_url(&self) -> &str {
        &self.info.locator
    }

    fn client(&self) -> Result<Client, SourceError> {
        Client::builder()
            .timeout(self.timeout)
            .connect_timeout(self.connect_timeout)
            .redirect(reqwest::redirect::Policy::none())
            // WebDAV 请求数量少且目录并发，禁用空闲连接复用避免服务器主动
            // 关闭连接时把下一批 PROPFIND 发到已失效的池化连接上。
            .pool_max_idle_per_host(0)
            .user_agent(format!("Kanalib/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
                SourceError::new(
                    SourceErrorKind::Local,
                    format!("创建 HTTP 客户端失败：{error}"),
                )
            })
    }

    fn set_last(&self, online: bool, error: Option<SourceError>) {
        if let Ok(mut last) = self.last.lock() {
            last.online = online;
            last.error = error;
        }
    }

    fn secret(&self, credentials: &Credentials) -> Result<String, SourceError> {
        credentials
            .secret(&self.info.credential_account())
            .map_err(SourceError::from_anyhow)?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| SourceError::credential("尚未保存 WebDAV 凭据（user:password）"))
    }

    fn dir_url(&self, relative: &str) -> Result<reqwest::Url, SourceError> {
        let mut url = self.base.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|_| {
                SourceError::new(SourceErrorKind::Protocol, "WebDAV 地址不是有效的 HTTP URL")
            })?;
            segments.pop_if_empty();
            for segment in relative.split('/').filter(|segment| !segment.is_empty()) {
                segments.push(segment);
            }
            // 目录 URL 统一以 `/` 结尾，避免服务器 301 丢失 PROPFIND 方法。
            segments.push("");
        }
        Ok(url)
    }

    fn fetch_listing(
        &self,
        client: &Client,
        authorization: &str,
        relative: &str,
        depth: &str,
        cached: Option<&CachedDir>,
        cancel: Option<&AtomicBool>,
    ) -> Result<Listing, SourceError> {
        let url = self.dir_url(relative)?;
        let method = reqwest::Method::from_bytes(b"PROPFIND").map_err(|error| {
            SourceError::new(
                SourceErrorKind::Local,
                format!("PROPFIND 方法无效：{error}"),
            )
        })?;
        let body = if depth == "0" {
            DEPTH_ZERO_BODY
        } else {
            DEPTH_ONE_BODY
        };
        let mut attempt = 0u32;
        loop {
            if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
                return Err(SourceError::cancelled());
            }
            let mut request = client
                .request(method.clone(), url.clone())
                .header("depth", depth)
                .header("Content-Type", "application/xml; charset=utf-8")
                .header(AUTHORIZATION, authorization)
                .body(body);
            if let Some(etag) = cached.and_then(|entry| entry.etag.as_deref()) {
                request = request.header(IF_NONE_MATCH, normalize_etag(etag));
            } else if let Some(modified) = cached.and_then(|entry| entry.last_modified) {
                request = request.header(IF_MODIFIED_SINCE, format_http_date(modified));
            }
            let response = match request.send() {
                Ok(response) => response,
                Err(error) => {
                    let mapped = transport_error(error, &url);
                    if mapped.is_transport() && attempt < self.config.retries {
                        std::thread::sleep(self.config.backoff(attempt));
                        attempt += 1;
                        continue;
                    }
                    return Err(mapped);
                }
            };
            let status = response.status();
            if status.as_u16() == 304 {
                let dir = cached.cloned().unwrap_or_default();
                return Ok(Listing {
                    dir,
                    not_modified: true,
                });
            }
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                return Err(SourceError::unauthorized(url.as_str()));
            }
            if status.as_u16() == 207 || status.is_success() {
                let etag = response
                    .headers()
                    .get("etag")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                let text = response.text().map_err(|error| {
                    SourceError::new(
                        SourceErrorKind::Disconnected,
                        format!("读取 PROPFIND 响应失败：{error}"),
                    )
                    .with_endpoint(url.as_str())
                })?;
                let entries = parse_propfind_with_root(&text, url.path(), self.base.path())?;
                let self_entry = entries
                    .iter()
                    .find(|entry| entry.path == relative.trim_matches('/'));
                let dir = CachedDir {
                    etag: self_entry.and_then(|entry| entry.etag.clone()).or(etag),
                    last_modified: self_entry.and_then(|entry| entry.mtime),
                    entries,
                };
                return Ok(Listing {
                    dir,
                    not_modified: false,
                });
            }
            if status.as_u16() == 429 || status.is_server_error() {
                if attempt < self.config.retries {
                    let wait =
                        retry_after(&response).unwrap_or_else(|| self.config.backoff(attempt));
                    std::thread::sleep(wait.min(Duration::from_millis(MAX_BACKOFF_MS)));
                    attempt += 1;
                    continue;
                }
            }
            return Err(SourceError {
                kind: if status.as_u16() == 429 || status.is_server_error() {
                    SourceErrorKind::Http
                } else {
                    SourceErrorKind::Protocol
                },
                message: format!("WebDAV 返回 HTTP {}", status.as_u16()),
                endpoint: Some(url.as_str().to_string()),
                status: Some(status.as_u16()),
                failures: Vec::new(),
            });
        }
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
        let secret = self.secret(credentials)?;
        let authorization = authorization_header(&secret);
        let client = self.client()?;
        let cache_path = cache_file(context.cache_dir, source_id);
        let mut cache = load_cache(&cache_path);
        let mut report = SourceSyncReport {
            source_id,
            ..Default::default()
        };

        let mut queue = VecDeque::from([String::new()]);
        let mut visited = HashSet::new();
        let mut cached_dirs = 0usize;
        let cancel = context.cancel;
        while !queue.is_empty() {
            if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
                report.cancelled = true;
                break;
            }
            let take = queue.len().min(self.config.concurrency.max(1));
            let batch = (0..take)
                .filter_map(|_| queue.pop_front())
                .collect::<Vec<_>>();
            let results = std::thread::scope(|scope| {
                let mut handles = Vec::with_capacity(batch.len());
                for dir in &batch {
                    let cached = cache.dirs.get(dir).cloned();
                    let client = &client;
                    let authorization = &authorization;
                    handles.push(scope.spawn(move || {
                        let result = self.fetch_listing(
                            client,
                            authorization,
                            dir,
                            "1",
                            cached.as_ref(),
                            cancel,
                        );
                        (dir.clone(), result)
                    }));
                }
                handles
                    .into_iter()
                    .map(|handle| {
                        handle.join().unwrap_or_else(|_| {
                            (
                                String::new(),
                                Err(SourceError::new(
                                    SourceErrorKind::Local,
                                    "WebDAV 工作线程异常退出",
                                )),
                            )
                        })
                    })
                    .collect::<Vec<_>>()
            });

            for (dir, result) in results {
                match result {
                    Ok(listing) => {
                        visited.insert(dir.clone());
                        if listing.not_modified {
                            cached_dirs += 1;
                        }
                        if listing.dir.etag.is_some()
                            || listing.dir.last_modified.is_some()
                            || listing.not_modified
                        {
                            cache.dirs.insert(dir.clone(), listing.dir.clone());
                        }
                        let entries = listing.dir.entries;
                        report.discovered += entries
                            .iter()
                            .filter(|entry| !entry.is_dir && is_supported_image(&entry.path))
                            .count();
                        for entry in &entries {
                            let entry_path = entry.path.trim_matches('/');
                            if entry_path.is_empty() || entry_path == dir {
                                continue;
                            }
                            if parent_dir(entry_path) != dir.trim_matches('/') {
                                continue;
                            }
                            if entry.is_dir {
                                if !visited.contains(entry_path) {
                                    queue.push_back(entry_path.to_string());
                                }
                                continue;
                            }
                            if !is_supported_image(entry_path) {
                                report.skipped += 1;
                                continue;
                            }
                            match self.upsert_entry(context, entry) {
                                Ok((true, _)) => report.imported += 1,
                                Ok((false, true)) => report.updated += 1,
                                Ok((false, false)) => report.skipped += 1,
                                Err(error) => {
                                    report.failed += 1;
                                    report
                                        .warnings
                                        .push(format!("{} 入库失败：{error}", entry_path));
                                }
                            }
                        }
                    }
                    Err(error) if error.kind == SourceErrorKind::Cancelled => {
                        report.cancelled = true;
                    }
                    Err(error) if error.status == Some(404) => {
                        cache.dirs.remove(&dir);
                        report.failed += 1;
                        report
                            .warnings
                            .push(format!("目录 {dir} 不可访问：{}", error.message));
                    }
                    Err(error) if error.kind == SourceErrorKind::Unauthorized => {
                        return Err(error);
                    }
                    Err(error) => {
                        report.failed += 1;
                        report
                            .warnings
                            .push(format!("目录 {dir} 同步失败：{}", error.message));
                    }
                }
            }
        }

        if report.cancelled {
            return Err(SourceError::cancelled());
        }
        // 目录缓存只保留本轮访问到的目录，避免远端删除后无限增长。
        cache.dirs.retain(|dir, _| visited.contains(dir));
        if let Err(error) = save_cache(&cache_path, &cache) {
            log::warn!("webdav directory cache save failed: {error:#}");
        }
        let now = unix_now();
        context
            .db
            .update_source_sync(source_id, None, now)
            .map_err(SourceError::from_anyhow)?;
        report.last_sync_at = Some(now);
        log::info!(
            "webdav source {source_id} synced: discovered={} imported={} cached_dirs={cached_dirs}",
            report.discovered,
            report.imported
        );
        Ok(report)
    }

    fn upsert_entry(
        &self,
        context: &SourceSyncContext<'_>,
        entry: &DavEntry,
    ) -> Result<(bool, bool), SourceError> {
        let source_id = self
            .info
            .id
            .ok_or_else(|| SourceError::new(SourceErrorKind::Local, "远端来源缺少数据库 id"))?;
        let path = webdav_asset_path(source_id, &entry.path);
        let stored = context
            .db
            .stored_meta(&path)
            .map_err(SourceError::from_anyhow)?;
        let name = entry
            .path
            .rsplit('/')
            .next()
            .filter(|value| !value.is_empty())
            .unwrap_or(&entry.path);
        let mime = entry
            .content_type
            .clone()
            .filter(|value| value.starts_with("image/"))
            .unwrap_or_else(|| crate::db::mime_for(Path::new(name)).to_string());
        let mtime = entry.mtime.unwrap_or(0);
        let imported = stored.is_none();
        let changed = stored
            .as_ref()
            .is_none_or(|meta| meta.mtime != mtime || meta.bytes != entry.size);
        if !changed {
            return Ok((false, false));
        }
        context
            .db
            .upsert_remote_asset(
                source_id,
                &path.to_string_lossy(),
                &self.info.locator,
                name,
                &mime,
                0,
                0,
                entry.size,
                mtime,
                None,
                // WebDAV 没有 SHA1，绝不为此下载 original；不参与 SHA1 归组。
                None,
                None,
            )
            .map_err(SourceError::from_anyhow)?;
        Ok((imported, true))
    }

    fn test(&self, credentials: &Credentials) -> Result<SourceStatus, SourceError> {
        let secret = self.secret(credentials)?;
        let authorization = authorization_header(&secret);
        let client = self.client()?;
        self.fetch_listing(&client, &authorization, "", "0", None, None)?;
        Ok(self.status())
    }
}

impl Source for WebDavSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn status(&self) -> SourceStatus {
        let last = self.last.lock().unwrap().clone();
        SourceStatus {
            kind: SourceKind::WebDav,
            locator: self.info.locator.clone(),
            online: last.online,
            asset_count: 0,
            last_sync_at: self.info.last_sync_at,
            last_error: last.error,
        }
    }

    fn test_connection(&self, credentials: &Credentials) -> Result<SourceStatus, SourceError> {
        match self.test(credentials) {
            Ok(status) => {
                self.set_last(true, None);
                Ok(status)
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
                if error.kind != SourceErrorKind::Cancelled {
                    self.set_last(false, Some(error.clone()));
                }
                Err(error)
            }
        }
    }
}

/// 解析 PROPFIND 207 响应；`request_path` 是本次请求的 URL 路径（用于 href 归一化）。
pub fn parse_propfind(xml: &str, request_path: &str) -> Result<Vec<DavEntry>, SourceError> {
    parse_propfind_with_root(xml, request_path, request_path)
}

/// 同上，但把 href 映射到来源根目录 `root_path` 下的相对路径。
fn parse_propfind_with_root(
    xml: &str,
    request_path: &str,
    root_path: &str,
) -> Result<Vec<DavEntry>, SourceError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut entries = Vec::new();
    let mut current: Option<DavEntry> = None;
    let mut field: Option<&'static str> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => match event.local_name().as_ref() {
                b"response" => {
                    field = None;
                    current = Some(DavEntry {
                        href: String::new(),
                        path: String::new(),
                        is_dir: false,
                        size: 0,
                        mtime: None,
                        etag: None,
                        content_type: None,
                    })
                }
                b"href" => field = Some("href"),
                b"getcontentlength" => field = Some("size"),
                b"getlastmodified" => field = Some("mtime"),
                b"getetag" => field = Some("etag"),
                b"getcontenttype" => field = Some("content_type"),
                _ => {}
            },
            Ok(Event::Empty(event)) => {
                if event.local_name().as_ref() == b"collection"
                    && let Some(entry) = current.as_mut()
                {
                    entry.is_dir = true;
                }
            }
            Ok(Event::Text(text)) => {
                let value = text
                    .xml10_content()
                    .ok()
                    .and_then(|decoded| unescape(&decoded).ok().map(|value| value.into_owned()))
                    .unwrap_or_default();
                if let (Some(entry), Some(field)) = (current.as_mut(), field) {
                    match field {
                        "href" => entry.href.push_str(&value),
                        "size" => entry.size = value.trim().parse().unwrap_or(0),
                        "mtime" => entry.mtime = parse_http_date(&value),
                        "etag" => entry.etag = Some(value.trim().to_string()),
                        "content_type" => {
                            entry.content_type = Some(value.trim().to_string());
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::CData(text)) => {
                let value = String::from_utf8_lossy(text.as_ref()).to_string();
                if let (Some(entry), Some(field)) = (current.as_mut(), field) {
                    match field {
                        "href" => entry.href.push_str(&value),
                        "size" => entry.size = value.trim().parse().unwrap_or(0),
                        "mtime" => entry.mtime = parse_http_date(&value),
                        "etag" => entry.etag = Some(value.trim().to_string()),
                        "content_type" => {
                            entry.content_type = Some(value.trim().to_string());
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::End(event)) => match event.local_name().as_ref() {
                b"href" | b"getcontentlength" | b"getlastmodified" | b"getetag"
                | b"getcontenttype" => field = None,
                b"response" => {
                    field = None;
                    if let Some(mut entry) = current.take() {
                        if !entry.href.trim().is_empty() {
                            entry.path = href_to_relative(&entry.href, request_path, root_path)
                                .unwrap_or_default();
                            entries.push(entry);
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(SourceError::new(
                    SourceErrorKind::Protocol,
                    format!("解析 PROPFIND XML 失败：{error}"),
                ));
            }
            _ => {}
        }
    }
    Ok(entries)
}

fn href_to_relative(href: &str, request_path: &str, root_path: &str) -> Option<String> {
    let trimmed = href.trim();
    if trimmed.is_empty() {
        return None;
    }
    let raw_path = if let Ok(url) = reqwest::Url::parse(trimmed) {
        url.path().to_string()
    } else if trimmed.starts_with('/') {
        trimmed.split(['?', '#']).next()?.to_string()
    } else {
        format!("{}/{}", request_path.trim_end_matches('/'), trimmed)
    };
    let decoded = percent_decode(raw_path.trim_end_matches('/'));
    let base = percent_decode(root_path.trim_end_matches('/'));
    let relative = if base.is_empty() || base == "/" {
        decoded.trim_start_matches('/').to_string()
    } else {
        decoded
            .strip_prefix(&base)?
            .trim_start_matches('/')
            .to_string()
    };
    Some(relative)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &input[index + 1..index + 3];
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn normalize_etag(etag: &str) -> String {
    let value = etag.trim();
    if value.starts_with('"') || value.starts_with("W/") {
        value.to_string()
    } else {
        format!("\"{value}\"")
    }
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("")
}

fn is_supported_image(path: &str) -> bool {
    crate::model::is_image(Path::new(path))
}

fn authorization_header(secret: &str) -> String {
    match secret.split_once(':') {
        Some((user, password)) => {
            format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
        }
        None => format!("Bearer {secret}"),
    }
}

fn normalize_base_url(input: &str) -> Result<reqwest::Url, SourceError> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(SourceError::new(
            SourceErrorKind::Protocol,
            "WebDAV 地址不能为空",
        ));
    }
    let mut url = reqwest::Url::parse(trimmed)
        .map_err(|_| SourceError::new(SourceErrorKind::Protocol, "WebDAV 地址不是有效的 URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(SourceError::new(
            SourceErrorKind::Protocol,
            "WebDAV 地址只支持 http/https",
        ));
    }
    if url.host_str().is_none() {
        return Err(SourceError::new(
            SourceErrorKind::Protocol,
            "WebDAV 地址缺少主机名",
        ));
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&format!("{path}/"));
    Ok(url)
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
        "WebDAV 请求超时".to_string()
    } else if kind == SourceErrorKind::Disconnected {
        format!("无法连接 WebDAV：{}", error.without_url())
    } else {
        format!("WebDAV 请求失败：{}", error.without_url())
    };
    SourceError {
        kind,
        message,
        endpoint: Some(url.as_str().to_string()),
        status: None,
        failures: Vec::new(),
    }
}

fn retry_after(response: &reqwest::blocking::Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(2)))
}

/// WebDAV 时间格式：RFC 1123（`Wed, 21 Oct 2015 07:28:00 GMT`），也兼容 ISO8601。
pub(super) fn parse_http_date(raw: &str) -> Option<u64> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(parsed) = crate::sources::parse_timestamp(value) {
        return Some(parsed);
    }
    let cleaned = value.replace(',', " ");
    let parts = cleaned.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 5 {
        return None;
    }
    let day: u32 = parts[1].parse().ok()?;
    let month = match parts[2].to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let year: i64 = parts[3].parse().ok()?;
    let mut clock = parts[4].split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next().unwrap_or("0").parse().ok()?;
    let second: i64 = clock.next().unwrap_or("0").parse().ok()?;
    let days = super::immich::days_from_civil(year, month, day);
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

/// 生成 `If-Modified-Since` 使用的 RFC 1123 时间（UTC）。
pub(super) fn format_http_date(unix: u64) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = (unix / 86_400) as i64;
    let seconds = unix % 86_400;
    let weekday = WEEKDAYS[((days + 4).rem_euclid(7)) as usize];
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!(
        "{weekday}, {day:02} {} {year:04} {hour:02}:{minute:02}:{second:02} GMT",
        MONTHS[(month - 1) as usize]
    )
}

/// Howard Hinnant 的 civil_from_days，与 `immich::days_from_civil` 互逆。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    (year, month as u32, day as u32)
}

fn cache_file(cache_dir: &Path, source_id: i64) -> PathBuf {
    cache_dir
        .join("webdav")
        .join(format!("source-{source_id}.json"))
}

fn load_cache(path: &Path) -> DirectoryCache {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let mut cache = serde_json::from_str::<DirectoryCache>(&text).unwrap_or_else(|error| {
                log::warn!("webdav directory cache parse failed: {error}");
                DirectoryCache {
                    version: 1,
                    dirs: HashMap::new(),
                }
            });
            if cache.version == 0 {
                cache.version = 1;
            }
            cache
        }
        Err(_) => DirectoryCache {
            version: 1,
            dirs: HashMap::new(),
        },
    }
}

fn save_cache(path: &Path, cache: &DirectoryCache) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = path.with_extension(format!("{}-{nonce}.tmp", std::process::id()));
    let text = serde_json::to_string(cache).unwrap_or_else(|_| "{}".to_string());
    std::fs::write(&temp, text)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
