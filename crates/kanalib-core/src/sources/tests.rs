//! Sources 单元测试：全部使用 target/ 下的隔离目录与回环 mock HTTP 服务。
//!
//! 不连接真实 Immich/WebDAV，不触碰系统凭据管理器；所有凭据都走
//! [`crate::sources::Credentials::in_memory`]。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::db::Db;
use crate::dedup::{DedupState, GroupKind};
use crate::sources::webdav::{format_http_date, parse_http_date};
use crate::sources::{
    Credentials, ImmichConfig, ImmichSource, LocalSource, Source, SourceErrorKind, SourceInfo,
    SourceKind, SourceSyncContext, WebDavConfig, WebDavSource, immich_virtual_path, is_remote_path,
    parse_checksum, parse_propfind, parse_timestamp, probe_version, source_infos,
    webdav_asset_path, webdav_relative_path, webdav_source_id,
};

// ---------------------------------------------------------------------------
// 隔离目录
// ---------------------------------------------------------------------------

fn test_dir(tag: &str) -> PathBuf {
    // 隔离目录放在 worktree 的 target/ 下，绝不触碰真实图库或系统数据根。
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let dir = workspace.join("target").join("sources-tests").join(format!(
        "{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(dir: &Path) {
    for _ in 0..40 {
        if !dir.exists() || std::fs::remove_dir_all(dir).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn write_test_image(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    image::RgbImage::from_pixel(4, 4, image::Rgb([10, 20, 30]))
        .save(path)
        .unwrap();
}

fn png_bytes() -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(2, 2, image::Rgb([200, 100, 50]))
        .write_to(&mut cursor, image::ImageFormat::Png)
        .unwrap();
    cursor.into_inner()
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
        .collect()
}

// ---------------------------------------------------------------------------
// 回环 mock HTTP
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct MockRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

#[derive(Clone, Debug)]
struct MockResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl MockResponse {
    fn json(status: u16, value: serde_json::Value) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: serde_json::to_vec(&value).unwrap(),
        }
    }

    fn xml(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), "application/xml".into())],
            body: body.into().into_bytes(),
        }
    }

    fn binary(status: u16, body: Vec<u8>, content_type: &str) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), content_type.to_string())],
            body,
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

type Handler = Arc<dyn Fn(&MockRequest) -> MockResponse + Send + Sync>;

struct MockServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<MockRequest>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MockServer {
    fn start(handler: Handler) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = {
            let requests = requests.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let handler = handler.clone();
                            let requests = requests.clone();
                            std::thread::spawn(move || serve(stream, handler, requests));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        Self {
            addr,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn requests(&self) -> Vec<MockRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve(
    mut stream: std::net::TcpStream,
    handler: Handler,
    requests: Arc<Mutex<Vec<MockRequest>>>,
) {
    // Windows 上 accept 出来的连接可能继承 listener 的非阻塞标记。
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(index) = find_header_end(&buffer) {
                    break index;
                }
                if buffer.len() > 1024 * 1024 {
                    return;
                }
            }
            Err(_) => return,
        }
    };
    let header_text = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(_) => break,
        }
    }
    let body = String::from_utf8_lossy(
        &buffer[body_start..(body_start + content_length).min(buffer.len())],
    )
    .to_string();
    let request = MockRequest {
        method,
        path,
        headers,
        body,
    };
    requests.lock().unwrap().push(request.clone());
    let response = handler(&request);
    let mut output = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        status_text(response.status),
        response.body.len()
    );
    for (name, value) in &response.headers {
        output.push_str(&format!("{name}: {value}\r\n"));
    }
    output.push_str("\r\n");
    let _ = stream.write_all(output.as_bytes());
    let _ = stream.write_all(&response.body);
    let _ = stream.flush();
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        207 => "Multi-Status",
        304 => "Not Modified",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

// ---------------------------------------------------------------------------
// Source 抽象 / 本地不回归
// ---------------------------------------------------------------------------

#[test]
fn local_source_wraps_existing_root_without_behavior_changes() {
    let dir = test_dir("local-source");
    let file = dir.join("one.png");
    write_test_image(&file);
    let source = LocalSource::new(dir.clone(), 1);
    let info = source.info();
    assert_eq!(info.kind, SourceKind::Local);
    assert!(!info.read_only);
    assert_eq!(source.root(), dir);
    let status = source.status();
    assert!(status.online);
    assert_eq!(status.asset_count, 1);
    assert!(
        source
            .test_connection(&Credentials::in_memory())
            .unwrap()
            .online
    );
    let db = Db::open(&dir.join("test.db")).unwrap();
    let error = source
        .sync(
            &SourceSyncContext {
                db: &db,
                cache_dir: &dir,
                cancel: None,
            },
            &Credentials::in_memory(),
        )
        .unwrap_err();
    assert_eq!(error.kind, SourceErrorKind::Unsupported);
    assert_eq!(info.kind.as_str(), "local");
    cleanup(&dir);
}

#[test]
fn remote_sources_are_listed_next_to_local_roots_and_paths_are_distinct() {
    let dir = test_dir("source-list");
    let record = crate::db::SourceRecord {
        id: 7,
        kind: "immich".to_string(),
        name: "Immich".to_string(),
        locator: "http://immich.local".to_string(),
        config_json: "{}".to_string(),
        read_only: true,
        last_sync_at: None,
        sync_cursor: None,
    };
    let sources = source_infos(&[dir.clone()], &[record]);
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0].kind, SourceKind::Local);
    assert_eq!(sources[1].kind, SourceKind::Immich);
    assert!(sources[1].read_only);

    let immich = immich_virtual_path("asset-1");
    let webdav = webdav_asset_path(3, "旅行/写真.jpg");
    assert!(is_remote_path(&immich));
    assert!(is_remote_path(&webdav));
    assert!(!is_remote_path(&dir.join("local.png")));
    assert_eq!(webdav_source_id(&webdav), Some(3));
    assert_eq!(
        webdav_relative_path(&webdav).as_deref(),
        Some("旅行/写真.jpg")
    );
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// Immich
// ---------------------------------------------------------------------------

#[test]
fn immich_checksum_and_timestamp_formats() {
    let raw = [
        0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
    ];
    let encoded = STANDARD.encode(raw);
    let expected = "000102030405060708090a0b0c0d0e0f10111213";
    assert_eq!(parse_checksum(&encoded).as_deref(), Some(expected));
    assert_eq!(
        parse_checksum(expected.to_uppercase().as_str()).as_deref(),
        Some(expected)
    );
    assert!(parse_checksum("not-a-checksum").is_none());
    assert_eq!(
        parse_timestamp("2026-01-02T03:04:05.000Z"),
        Some(1_767_323_045)
    );
    assert_eq!(
        parse_timestamp("2026-01-02T11:04:05+08:00"),
        Some(1_767_323_045)
    );
    assert!(parse_timestamp("").is_none());
}

#[test]
fn immich_probe_reports_version_and_unauthorized_without_leaking_key() {
    let server = MockServer::start(Arc::new(|request: &MockRequest| {
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/api/server/about");
        assert_eq!(
            request.headers.get("x-api-key").map(String::as_str),
            Some("good-key")
        );
        MockResponse::json(200, serde_json::json!({"version": "v1.119.0"}))
    }));
    let version = probe_version(&server.url(), "good-key", Duration::from_secs(2)).unwrap();
    assert_eq!(version.version, "1.119.0");
    assert_eq!(version.endpoint, "api/server/about");

    let denied = MockServer::start(Arc::new(|_request: &MockRequest| MockResponse::empty(401)));
    let error = probe_version(&denied.url(), "bad-key", Duration::from_secs(2)).unwrap_err();
    assert_eq!(error.kind, SourceErrorKind::Unauthorized);
    assert_eq!(error.status, Some(401));
    assert!(!error.to_string().contains("bad-key"));
}

fn immich_record(db: &Db, url: &str) -> (i64, crate::db::SourceRecord) {
    let id = db.upsert_source("immich", "Mock", url, "{}", true).unwrap();
    (id, db.source_record(id).unwrap().unwrap())
}

#[test]
fn immich_paginates_and_persists_updated_after_cursor() {
    let dir = test_dir("immich-pages");
    let db = Db::open(&dir.join("db.sqlite")).unwrap();
    let pages = Arc::new(AtomicUsize::new(0));
    let handler = {
        let pages = pages.clone();
        Arc::new(move |request: &MockRequest| {
            match (request.method.as_str(), request.path.as_str()) {
                ("GET", "/api/server/about") => {
                    MockResponse::json(200, serde_json::json!({"version": "1.119.0"}))
                }
                ("POST", "/api/search/metadata") => {
                    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
                    let page = body["page"].as_u64().unwrap();
                    let updated_after = body["updatedAfter"].as_str().map(str::to_string);
                    if let Some(cursor) = updated_after {
                        assert_eq!(cursor, "2026-01-02T00:00:00.000Z");
                        return MockResponse::json(
                            200,
                            serde_json::json!({"assets": {"total": 0, "nextPage": null, "items": []}}),
                        );
                    }
                    pages.fetch_add(1, Ordering::SeqCst);
                    let items = match page {
                        1 => vec![asset_json(
                            "a1",
                            "one.png",
                            "2026-01-01T00:00:00.000Z",
                            "000102030405060708090a0b0c0d0e0f10111213",
                        )],
                        2 => vec![asset_json(
                            "a2",
                            "two.png",
                            "2026-01-02T00:00:00.000Z",
                            "101112131415161718191a1b1c1d1e1f20212223",
                        )],
                        _ => Vec::new(),
                    };
                    let next = if page < 2 {
                        serde_json::json!(page + 1)
                    } else {
                        serde_json::Value::Null
                    };
                    MockResponse::json(
                        200,
                        serde_json::json!({"assets": {"total": 2, "nextPage": next, "items": items}}),
                    )
                }
                ("GET", path)
                    if path.starts_with("/api/assets/") && path.ends_with("/thumbnail") =>
                {
                    MockResponse::binary(200, png_bytes(), "image/png")
                }
                _ => MockResponse::empty(404),
            }
        }) as Handler
    };
    let server = MockServer::start(handler);
    let (id, record) = immich_record(&db, &server.url());
    let info = SourceInfo::from_record(&record).unwrap();
    let config = ImmichConfig {
        page_size: 1,
        thumbnail_size: "thumbnail".to_string(),
    };
    let source = ImmichSource::new(info.clone(), config)
        .unwrap()
        .with_timeout(Duration::from_secs(5));
    let credentials = Credentials::in_memory();
    credentials
        .set_api_key(&info.credential_account(), "test-key")
        .unwrap();
    let cache = dir.join("thumbs");
    let context = SourceSyncContext {
        db: &db,
        cache_dir: &cache,
        cancel: None,
    };
    let report = source.sync(&context, &credentials).unwrap();
    assert_eq!(report.imported, 2);
    assert_eq!(report.cursor.as_deref(), Some("2026-01-02T00:00:00.000Z"));
    assert_eq!(pages.load(Ordering::SeqCst), 3);
    let record = db.source_record(id).unwrap().unwrap();
    assert_eq!(
        record.sync_cursor.as_deref(),
        Some("2026-01-02T00:00:00.000Z")
    );
    assert_eq!(db.source_asset_count(id).unwrap(), 2);
    assert!(
        db.stored_meta(&immich_virtual_path("a1"))
            .unwrap()
            .is_some()
    );
    assert!(record.last_sync_at.is_some());

    // 第二次同步必须带上游标，只处理增量。
    let source = ImmichSource::new(
        SourceInfo::from_record(&record).unwrap(),
        ImmichConfig {
            page_size: 1,
            thumbnail_size: "thumbnail".to_string(),
        },
    )
    .unwrap()
    .with_timeout(Duration::from_secs(5));
    let report = source.sync(&context, &credentials).unwrap();
    assert_eq!(report.imported, 0);
    assert_eq!(report.cursor.as_deref(), Some("2026-01-02T00:00:00.000Z"));
    let search = server
        .requests()
        .into_iter()
        .filter(|request| request.path == "/api/search/metadata")
        .collect::<Vec<_>>();
    assert!(
        search
            .iter()
            .any(|request| request.body.contains("updatedAfter"))
    );
    cleanup(&dir);
}

fn asset_json(id: &str, name: &str, updated: &str, checksum_hex: &str) -> serde_json::Value {
    let bytes = hex_to_bytes(checksum_hex);
    serde_json::json!({
        "id": id,
        "type": "IMAGE",
        "originalFileName": name,
        "originalMimeType": "image/png",
        "fileCreatedAt": updated,
        "fileModifiedAt": "2025-01-01T00:00:00.000Z",
        "updatedAt": updated,
        "checksum": STANDARD.encode(bytes),
        "width": 4,
        "height": 4,
        "fileSizeInByte": 100,
        "exifInfo": {"exifImageWidth": 4, "exifImageHeight": 4}
    })
}

#[test]
fn immich_missing_key_and_disconnect_do_not_block_local_library() {
    let base = test_dir("immich-offline");
    let lib = base.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    write_test_image(&lib.join("local.png"));
    let state = crate::state::AppState::open_for_test_with_credentials(
        base.join("data"),
        Some(lib.clone()),
        Credentials::in_memory(),
    );
    state.rescan_root_blocking(lib.clone(), true).unwrap();
    assert_eq!(local_asset_count(&state), 1);

    let source = state
        .add_immich_source("Broken", "http://127.0.0.1:1")
        .unwrap();
    let id = source.id.unwrap();
    let error = state.sync_source_blocking(id).unwrap_err();
    assert_eq!(error.kind, SourceErrorKind::Credential);
    assert_eq!(local_asset_count(&state), 1);

    state.set_source_api_key(id, "temporary-key").unwrap();
    let error = state.sync_source_blocking(id).unwrap_err();
    assert!(matches!(
        error.kind,
        SourceErrorKind::Disconnected | SourceErrorKind::Timeout | SourceErrorKind::Http
    ));
    assert_eq!(local_asset_count(&state), 1);
    let db = Db::open(&base.join("data").join(".kanalib").join("kanalib.db")).unwrap();
    let record = db.source_record(id).unwrap().unwrap();
    assert!(record.sync_cursor.is_none());
    assert!(record.last_sync_at.is_none());
    drop(db);
    drop(state);
    cleanup(&base);
}

fn local_asset_count(state: &Arc<crate::state::AppState>) -> usize {
    state.shared.lock().unwrap().assets.read().unwrap().len()
}

#[test]
fn credentials_never_reach_database_settings_or_debug_output() {
    let base = test_dir("credentials");
    let state = crate::state::AppState::open_for_test_with_credentials(
        base.join("data"),
        None,
        Credentials::in_memory(),
    );
    let source = state
        .add_immich_source("Secure", "http://127.0.0.1:9")
        .unwrap();
    let id = source.id.unwrap();
    let secret = "very-secret-immich-key-42";
    state.set_source_api_key(id, secret).unwrap();
    assert!(state.has_source_api_key(id).unwrap());

    let db_path = base.join("data").join(".kanalib").join("kanalib.db");
    let db = Db::open(&db_path).unwrap();
    let record = db.source_record(id).unwrap().unwrap();
    assert!(!record.config_json.contains(secret));
    assert!(!record.locator.contains(secret));
    drop(db);
    assert!(!directory_contains(&base.join("data"), secret));

    let credentials = Credentials::in_memory();
    credentials.set_api_key("immich:http://x", secret).unwrap();
    assert!(!format!("{credentials:?}").contains(secret));
    state.clear_source_api_key(id).unwrap();
    assert!(!state.has_source_api_key(id).unwrap());
    drop(state);
    cleanup(&base);
}

fn directory_contains(dir: &Path, needle: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if directory_contains(&path, needle) {
                return true;
            }
        } else if let Ok(bytes) = std::fs::read(&path) {
            if bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
            {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// WebDAV
// ---------------------------------------------------------------------------

#[test]
fn propfind_parses_namespaced_multistatus_with_etag_and_last_modified() {
    let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/</D:href>
    <D:propstat><D:prop>
      <D:resourcetype><D:collection/></D:resourcetype>
      <D:getetag>"root-v1"</D:getetag>
    </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/%E5%86%99%E7%9C%9F/</D:href>
    <D:propstat><D:prop>
      <D:resourcetype><D:collection/></D:resourcetype>
      <D:getetag>"dir-v1"</D:getetag>
    </D:prop></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/%E5%86%99%E7%9C%9F/a%20b.jpg</D:href>
    <D:propstat><D:prop>
      <D:resourcetype/>
      <D:getcontentlength>123</D:getcontentlength>
      <D:getlastmodified>Wed, 21 Oct 2015 07:28:00 GMT</D:getlastmodified>
      <D:getetag>"file-v1"</D:getetag>
      <D:getcontenttype>image/jpeg</D:getcontenttype>
    </D:prop></D:propstat>
  </D:response>
</D:multistatus>"#;
    let entries = parse_propfind(xml, "/dav/").unwrap();
    assert_eq!(entries.len(), 3);
    assert!(entries[0].is_dir);
    assert_eq!(entries[0].path, "");
    assert_eq!(entries[1].path, "写真");
    assert!(entries[1].is_dir);
    assert_eq!(entries[2].path, "写真/a b.jpg");
    assert!(!entries[2].is_dir);
    assert_eq!(entries[2].size, 123);
    assert_eq!(entries[2].mtime, Some(1_445_412_480));
    assert_eq!(entries[2].etag.as_deref(), Some("\"file-v1\""));
    assert_eq!(entries[2].content_type.as_deref(), Some("image/jpeg"));
    assert_eq!(
        format_http_date(1_445_412_480),
        "Wed, 21 Oct 2015 07:28:00 GMT"
    );
    assert_eq!(
        parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT"),
        Some(1_445_412_480)
    );
}

fn dav_response(
    href: &str,
    is_dir: bool,
    size: u64,
    etag: Option<&str>,
    mtime: Option<&str>,
) -> String {
    let resource = if is_dir {
        "<d:resourcetype><d:collection/></d:resourcetype>".to_string()
    } else {
        format!("<d:resourcetype/><d:getcontentlength>{size}</d:getcontentlength>")
    };
    let etag = etag
        .map(|value| format!("<d:getetag>{value}</d:getetag>"))
        .unwrap_or_default();
    let mtime = mtime
        .map(|value| format!("<d:getlastmodified>{value}</d:getlastmodified>"))
        .unwrap_or_default();
    format!(
        "<d:response><d:href>{href}</d:href><d:propstat><d:prop>{resource}{etag}{mtime}\
         <d:getcontenttype>image/jpeg</d:getcontenttype></d:prop>\
         <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
    )
}

fn dav_multistatus(responses: Vec<String>) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><d:multistatus xmlns:d=\"DAV:\">{}</d:multistatus>",
        responses.join("")
    )
}

#[test]
fn webdav_sync_parses_tree_limits_concurrency_and_skips_sha1_grouping() {
    let dir = test_dir("webdav-sync");
    let db = Db::open(&dir.join("db.sqlite")).unwrap();
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let handler = {
        let active = active.clone();
        let max_active = max_active.clone();
        Arc::new(move |request: &MockRequest| {
            assert_eq!(request.method, "PROPFIND");
            let path = request.path.split('?').next().unwrap_or("");
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            max_active.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(25));
            let body = if path == "/dav/" {
                let mut responses = vec![dav_response(
                    "/dav/",
                    true,
                    0,
                    Some("\"root-v1\""),
                    Some("Wed, 21 Oct 2015 07:28:00 GMT"),
                )];
                for index in 0..8 {
                    responses.push(dav_response(
                        &format!("/dav/d{index}/"),
                        true,
                        0,
                        Some(&format!("\"d{index}-v1\"")),
                        None,
                    ));
                }
                dav_multistatus(responses)
            } else if let Some(index) = path
                .strip_prefix("/dav/d")
                .and_then(|rest| rest.strip_suffix('/'))
                .and_then(|value| value.parse::<usize>().ok())
            {
                dav_multistatus(vec![
                    dav_response(
                        &format!("/dav/d{index}/"),
                        true,
                        0,
                        Some(&format!("\"d{index}-v1\"")),
                        None,
                    ),
                    dav_response(
                        &format!("/dav/d{index}/img{index}.jpg"),
                        false,
                        42,
                        Some(&format!("\"f{index}-v1\"")),
                        Some("Wed, 21 Oct 2015 07:28:00 GMT"),
                    ),
                ])
            } else {
                active.fetch_sub(1, Ordering::SeqCst);
                return MockResponse::empty(404);
            };
            active.fetch_sub(1, Ordering::SeqCst);
            MockResponse::xml(207, body)
        }) as Handler
    };
    let server = MockServer::start(handler);
    let id = db
        .upsert_source(
            "webdav",
            "Mock DAV",
            &format!("{}/dav", server.url()),
            "{}",
            true,
        )
        .unwrap();
    let record = db.source_record(id).unwrap().unwrap();
    let info = SourceInfo::from_record(&record).unwrap();
    let source = WebDavSource::new(
        info.clone(),
        WebDavConfig {
            concurrency: 4,
            retries: 1,
            backoff_ms: 1,
        },
    )
    .unwrap()
    .with_timeout(Duration::from_secs(5));
    let credentials = Credentials::in_memory();
    credentials
        .set_secret(&info.credential_account(), "user:password")
        .unwrap();
    let cache = dir.join("cache");
    let context = SourceSyncContext {
        db: &db,
        cache_dir: &cache,
        cancel: None,
    };
    let report = source.sync(&context, &credentials).unwrap();
    assert_eq!(report.imported, 8);
    assert_eq!(report.discovered, 8);
    assert_eq!(max_active.load(Ordering::SeqCst), 4);
    assert!(
        server
            .requests()
            .iter()
            .all(|request| request.method == "PROPFIND")
    );
    assert!(server.requests().iter().all(|request| {
        request.headers.get("authorization").map(String::as_str)
            == Some("Basic dXNlcjpwYXNzd29yZA==")
    }));
    assert_eq!(db.source_asset_count(id).unwrap(), 8);
    let assets = db.load_library(&[info.locator.clone()]).unwrap();
    assert!(assets.iter().all(|asset| asset.sha1.is_none()));
    let mut dedup = DedupState::default();
    dedup.rebuild(&assets);
    assert!(dedup.groups.is_empty(), "WebDAV 没有 SHA1，不应参与归组");
    assert!(assets.iter().all(|asset| is_remote_path(&asset.path)));
    cleanup(&dir);
}

#[test]
fn webdav_retries_transient_errors_with_backoff() {
    let dir = test_dir("webdav-retry");
    let db = Db::open(&dir.join("db.sqlite")).unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let handler = {
        let attempts = attempts.clone();
        Arc::new(move |request: &MockRequest| {
            assert_eq!(request.method, "PROPFIND");
            let count = attempts.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                MockResponse::empty(503)
            } else {
                MockResponse::xml(
                    207,
                    dav_multistatus(vec![dav_response("/dav/", true, 0, None, None)]),
                )
            }
        }) as Handler
    };
    let server = MockServer::start(handler);
    let id = db
        .upsert_source(
            "webdav",
            "Retry",
            &format!("{}/dav", server.url()),
            "{}",
            true,
        )
        .unwrap();
    let info = SourceInfo::from_record(&db.source_record(id).unwrap().unwrap()).unwrap();
    let source = WebDavSource::new(
        info.clone(),
        WebDavConfig {
            concurrency: 2,
            retries: 2,
            backoff_ms: 1,
        },
    )
    .unwrap()
    .with_timeout(Duration::from_secs(5));
    let credentials = Credentials::in_memory();
    credentials
        .set_secret(&info.credential_account(), "user:password")
        .unwrap();
    let cache = dir.join("cache");
    let report = source
        .sync(
            &SourceSyncContext {
                db: &db,
                cache_dir: &cache,
                cancel: None,
            },
            &credentials,
        )
        .unwrap();
    assert_eq!(report.failed, 0);
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    cleanup(&dir);
}

#[test]
fn webdav_directory_cache_revalidates_with_etag_and_reuses_entries() {
    let dir = test_dir("webdav-cache");
    let db = Db::open(&dir.join("db.sqlite")).unwrap();
    let handler = Arc::new(|request: &MockRequest| {
        assert_eq!(request.method, "PROPFIND");
        let etag = request.headers.get("if-none-match").map(String::as_str);
        if etag == Some("\"v1\"") {
            return MockResponse::empty(304);
        }
        MockResponse::xml(
            207,
            dav_multistatus(vec![
                dav_response(
                    "/dav/",
                    true,
                    0,
                    Some("\"v1\""),
                    Some("Wed, 21 Oct 2015 07:28:00 GMT"),
                ),
                dav_response(
                    "/dav/photo.jpg",
                    false,
                    42,
                    Some("\"f1\""),
                    Some("Wed, 21 Oct 2015 07:28:00 GMT"),
                ),
            ]),
        )
    }) as Handler;
    let server = MockServer::start(handler);
    let id = db
        .upsert_source(
            "webdav",
            "Cache",
            &format!("{}/dav", server.url()),
            "{}",
            true,
        )
        .unwrap();
    let record = db.source_record(id).unwrap().unwrap();
    let info = SourceInfo::from_record(&record).unwrap();
    let source = WebDavSource::new(info.clone(), WebDavConfig::default())
        .unwrap()
        .with_timeout(Duration::from_secs(5));
    let credentials = Credentials::in_memory();
    credentials
        .set_secret(&info.credential_account(), "user:password")
        .unwrap();
    let cache = dir.join("cache");
    let context = SourceSyncContext {
        db: &db,
        cache_dir: &cache,
        cancel: None,
    };
    let first = source.sync(&context, &credentials).unwrap();
    assert_eq!(first.imported, 1);
    assert!(
        cache
            .join("webdav")
            .join(format!("source-{id}.json"))
            .is_file()
    );

    let second = source.sync(&context, &credentials).unwrap();
    assert_eq!(second.imported, 0);
    assert_eq!(second.skipped, 1);
    assert_eq!(second.discovered, 1);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].headers.get("if-none-match").map(String::as_str),
        Some("\"v1\"")
    );
    assert_eq!(db.source_asset_count(id).unwrap(), 1);
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// 跨源 SHA1 精确去重
// ---------------------------------------------------------------------------

#[test]
fn cross_source_sha1_groups_local_and_immich_and_stays_read_only() {
    let base = test_dir("cross-source");
    let lib = base.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let local_path = lib.join("local.png");
    write_test_image(&local_path);
    let hashes = crate::media::hash_file(&local_path).unwrap();
    let bytes = std::fs::metadata(&local_path).unwrap().len();
    let checksum = STANDARD.encode(hex_to_bytes(&hashes.sha1));

    let handler = {
        let checksum = checksum.clone();
        Arc::new(move |request: &MockRequest| {
            match (request.method.as_str(), request.path.as_str()) {
                ("GET", "/api/server/about") => {
                    MockResponse::json(200, serde_json::json!({"version": "1.119.0"}))
                }
                ("POST", "/api/search/metadata") => MockResponse::json(
                    200,
                    serde_json::json!({"assets": {"total": 1, "nextPage": null, "items": [{
                        "id": "remote-1",
                        "type": "IMAGE",
                        "originalFileName": "remote.png",
                        "originalMimeType": "image/png",
                        "fileCreatedAt": "2026-01-01T00:00:00.000Z",
                        "fileModifiedAt": "2026-01-01T00:00:00.000Z",
                        "updatedAt": "2026-01-01T00:00:00.000Z",
                        "checksum": checksum,
                        "width": 4,
                        "height": 4,
                        "fileSizeInByte": bytes,
                        "exifInfo": {"exifImageWidth": 4, "exifImageHeight": 4}
                    }]}}),
                ),
                ("GET", path)
                    if path.starts_with("/api/assets/") && path.ends_with("/thumbnail") =>
                {
                    MockResponse::binary(200, png_bytes(), "image/png")
                }
                _ => MockResponse::empty(404),
            }
        }) as Handler
    };
    let server = MockServer::start(handler);
    let state = crate::state::AppState::open_for_test_with_credentials(
        base.join("data"),
        Some(lib.clone()),
        Credentials::in_memory(),
    );
    state.rescan_root_blocking(lib.clone(), true).unwrap();
    let source = state.add_immich_source("Remote", &server.url()).unwrap();
    let id = source.id.unwrap();
    state.set_source_api_key(id, "test-key").unwrap();
    let report = state.sync_source_blocking(id).unwrap();
    assert_eq!(report.imported, 1);

    let remote_path = immich_virtual_path("remote-1");
    let shared = state.shared.lock().unwrap();
    let group = shared
        .dedup
        .groups
        .iter()
        .find(|group| group.key == hashes.sha1)
        .unwrap_or_else(|| panic!("missing cross-source group {}", hashes.sha1));
    assert!(group.assets.iter().any(|asset| asset.path == local_path));
    assert!(group.assets.iter().any(|asset| asset.path == remote_path));
    assert_eq!(group.assets.len(), 2);
    let remote = shared
        .assets
        .read()
        .unwrap()
        .iter()
        .find(|asset| asset.path == remote_path)
        .cloned()
        .unwrap();
    assert_eq!(remote.sha1.as_deref(), Some(hashes.sha1.as_str()));
    drop(shared);

    assert!(state.set_dedup_keeper(&hashes.sha1, local_path.clone(), GroupKind::Exact));
    let error = state.trash_dedup_group(&hashes.sha1).unwrap_err();
    assert!(
        error.to_string().contains("只读来源"),
        "unexpected error: {error}"
    );
    assert!(local_path.is_file());
    let db = Db::open(&base.join("data").join(".kanalib").join("kanalib.db")).unwrap();
    assert!(db.stored_meta(&remote_path).unwrap().is_some());
    drop(db);
    drop(state);
    cleanup(&base);
}
