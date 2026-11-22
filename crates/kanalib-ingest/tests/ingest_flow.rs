use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::Result;
use image::{ImageFormat, RgbaImage};
use kanalib_ingest::{
    db::{Db, now},
    download::{self, DownloadConfig, ImageResponse, ImageTransport},
    ingest::{self, item_id},
    source::{
        SourceAdapter, SourceError, SourceErrorKind,
        mock::{MockPage, MockSource},
        pixiv::{ApiGate, PixivResponse, PixivSource, PixivTransport, classify_status},
    },
};
use rusqlite::params;
use tempfile::TempDir;
use tokio::sync::Mutex;

struct Transport {
    bytes: Vec<u8>,
    interrupted: Arc<Mutex<bool>>,
    ranges: Arc<Mutex<Vec<Option<u64>>>>,
}

impl ImageTransport for Transport {
    fn fetch<'a>(
        &self,
        _url: &str,
        range_start: Option<u64>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ImageResponse, SourceError>> + Send + 'a>,
    > {
        let ranges = Arc::clone(&self.ranges);
        let interrupted = Arc::clone(&self.interrupted);
        let bytes = self.bytes.clone();
        Box::pin(async move {
            ranges.lock().await.push(range_start);
            if *interrupted.lock().await {
                *interrupted.lock().await = false;
                return Ok(ImageResponse::ok(bytes[..4].to_vec(), "image/png"));
            }
            let start = range_start.unwrap_or(0) as usize;
            let body = bytes[start..].to_vec();
            let mut response = ImageResponse::ok(body, "image/png");
            if start > 0 {
                response.status = 206;
                response.content_range = Some(format!("bytes {start}-{}/*", bytes.len()));
                response.content_length = Some((bytes.len() - start) as u64);
            }
            Ok(response)
        })
    }
}

struct ScriptedPixiv {
    statuses: Arc<Mutex<Vec<u16>>>,
    calls: Arc<Mutex<usize>>,
}

impl PixivTransport for ScriptedPixiv {
    fn get<'a>(
        &self,
        _path: &'a str,
        _params: &'a [(&'a str, String)],
        _auth: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<PixivResponse, SourceError>> + Send + 'a>,
    > {
        let calls = Arc::clone(&self.calls);
        let statuses = Arc::clone(&self.statuses);
        Box::pin(async move {
            let mut calls = calls.lock().await;
            let index = *calls;
            *calls += 1;
            drop(calls);
            let statuses = statuses.lock().await;
            let status = *statuses
                .get(index)
                .unwrap_or(statuses.last().unwrap_or(&200));
            Ok(PixivResponse {
                status,
                retry_after: Some(Duration::from_secs(7)),
                body: serde_json::json!({"illusts": []}),
            })
        })
    }
}

struct HtmlTransport;

impl ImageTransport for HtmlTransport {
    fn fetch<'a>(
        &self,
        _url: &str,
        _range_start: Option<u64>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ImageResponse, SourceError>> + Send + 'a>,
    > {
        Box::pin(async {
            Ok(ImageResponse::ok(
                b"<!DOCTYPE html><html>blocked</html>".to_vec(),
                "text/html",
            ))
        })
    }
}

fn png_bytes() -> Vec<u8> {
    let image = RgbaImage::from_fn(8, 8, |x, y| {
        image::Rgba([x as u8 * 20, y as u8 * 20, 80, 255])
    });
    let mut bytes = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    bytes
}

fn config(dir: &TempDir) -> DownloadConfig {
    DownloadConfig {
        media_dir: dir.path().to_path_buf(),
        concurrency: 2,
        retry_max: 3,
        timeout: Duration::from_secs(5),
    }
}

fn work_page(work_id: &str) -> MockPage {
    MockPage {
        work: MockSource::work(work_id, 1),
        image_url: format!("mock://{work_id}"),
    }
}

fn work_count(db: &Db) -> usize {
    db.all_items().unwrap().len()
}

#[tokio::test]
async fn mock_bookmarks_two_pages_second_page_is_fast_catch_up() {
    let mut db = Db::open_in_memory().unwrap();
    let first = work_page("first");
    let second = work_page("second");
    let mut adapter = MockSource::new("mock");
    adapter.bookmark_pages = vec![vec![first.clone()], vec![second.clone()]];

    let first_run = ingest::ingest_bookmarks(&mut db, &adapter, "user")
        .await
        .unwrap();
    assert_eq!(first_run.fetched_pages, 2);
    assert!(!first_run.stopped_early);
    assert_eq!(work_count(&db), 2);

    let second_run = ingest::ingest_bookmarks(&mut db, &adapter, "user")
        .await
        .unwrap();
    assert_eq!(second_run.fetched_pages, 1);
    assert!(second_run.stopped_early);
    assert_eq!(work_count(&db), 2);
}

#[tokio::test]
async fn downloading_bookmarks_preserves_page_order_and_creates_thumbnails() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open_in_memory().unwrap();
    let bytes = png_bytes();
    let mut adapter = MockSource::new("mock");
    let multi_page = MockSource::work("multi", 3);
    adapter.works.push(multi_page.clone());
    adapter.bookmark_pages.push(vec![MockPage {
        work: multi_page,
        image_url: "mock://multi".to_string(),
    }]);

    ingest::ingest_bookmarks(&mut db, &adapter, "user")
        .await
        .unwrap();
    for page in 0..3 {
        db.persist_bookmark_image(
            &item_id("mock", "multi", page),
            &format!("mock://multi/{page}"),
            Some("png"),
        )
        .unwrap();
    }
    let transport = Transport {
        bytes: bytes.clone(),
        interrupted: Arc::new(Mutex::new(false)),
        ranges: Arc::new(Mutex::new(Vec::new())),
    };

    let report = download::run_queue(&db, &adapter, &transport, &config(&dir))
        .await
        .unwrap();
    assert_eq!(report.succeeded.len(), 3, "failed: {:?}", report.failed);
    assert_eq!(report.failed, Vec::<(String, String)>::new());
    assert!(report.failed.is_empty());

    let items = db.all_items().unwrap();
    let pages: Vec<u32> = items.iter().map(|item| item.page).collect();
    assert_eq!(pages, vec![0, 1, 2]);
    for item in &items {
        assert!(item.original_path.is_some());
        assert!(item.thumb_256.is_some() && item.thumb_512.is_some());
        assert_eq!(item.bytes, bytes.len() as u64);
    }
}

#[tokio::test]
async fn interrupted_download_resumes_from_partial_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open_in_memory().unwrap();
    let bytes = png_bytes();
    let mut adapter = MockSource::new("mock");
    adapter.bookmark_pages.push(vec![work_page("resume")]);
    ingest::ingest_bookmarks(&mut db, &adapter, "user")
        .await
        .unwrap();
    db.persist_bookmark_image(&item_id("mock", "resume", 0), "mock://resume", Some("png"))
        .unwrap();

    let interrupted = Arc::new(Mutex::new(true));
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let transport = Transport {
        bytes: bytes.clone(),
        interrupted: Arc::clone(&interrupted),
        ranges: Arc::clone(&ranges),
    };
    let config = config(&dir);

    let first = download::run_queue(&db, &adapter, &transport, &config)
        .await
        .unwrap();
    assert_eq!(first.failed.len(), 1);
    assert_eq!(*ranges.lock().await, vec![None]);

    let part = config
        .media_dir
        .join("parts")
        .join(format!("{}.png.part", item_id("mock", "resume", 0)));
    assert!(part.exists());

    let recovered = download::recover(&db, &config.media_dir).unwrap();

    assert_eq!(recovered, 0);
    db.connection()
        .execute(
            "UPDATE download_queue SET state='queued', next_retry_at=NULL WHERE item_id=?1",
            rusqlite::params![item_id("mock", "resume", 0)],
        )
        .unwrap();
    let second = download::run_queue(&db, &adapter, &transport, &config)
        .await
        .unwrap();
    assert_eq!(second.succeeded.len(), 1, "second: {:?}", second.failed);
    assert!(
        first.failed[0].1.contains("download"),
        "first: {:?}",
        first.failed
    );
    assert_eq!(*ranges.lock().await, vec![None, Some(4)]);
    assert!(!part.exists());
}

#[tokio::test]
async fn html_error_page_does_not_become_an_image() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open_in_memory().unwrap();
    let mut adapter = MockSource::new("mock");
    adapter.bookmark_pages.push(vec![work_page("html")]);
    ingest::ingest_bookmarks(&mut db, &adapter, "user")
        .await
        .unwrap();
    db.persist_bookmark_image(&item_id("mock", "html", 0), "mock://html", Some("png"))
        .unwrap();

    let report = download::run_queue(&db, &adapter, &HtmlTransport, &config(&dir))
        .await
        .unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].1.contains("HTML"));
}

#[tokio::test]
async fn rate_limit_sets_retry_after_and_pauses_queue() {
    let mut db = Db::open_in_memory().unwrap();
    let mut seed = MockSource::new("mock");
    seed.bookmark_pages.push(vec![work_page("rate")]);
    ingest::ingest_bookmarks(&mut db, &seed, "user")
        .await
        .unwrap();
    let statuses = Arc::new(Mutex::new(vec![429]));
    let calls = Arc::new(Mutex::new(0));
    let transport = Arc::new(ScriptedPixiv {
        statuses: Arc::clone(&statuses),
        calls: Arc::clone(&calls),
    });
    let adapter = PixivSource::new(transport.clone());
    let error = adapter
        .list_public_bookmarks("user", None)
        .await
        .unwrap_err();
    assert_eq!(error.kind, SourceErrorKind::RateLimited);
    assert_eq!(error.retry_after, Some(Duration::from_secs(7)));

    ingest::handle_source_error(&db, &adapter, None, &error)
        .await
        .unwrap();
    let retry_at: Option<i64> = db
        .connection()
        .query_row("SELECT MIN(next_retry_at) FROM download_queue", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(
        retry_at.is_some(),
        "queue: {:?}",
        db.connection()
            .query_row(
                "SELECT state, next_retry_at, last_error FROM download_queue",
                [],
                |r| Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<String>>(2)?
                ))
            )
            .unwrap()
    );
    assert!(retry_at.unwrap() > now());
    let backoff = ingest::backoff_delay(1, Some(Duration::from_secs(7)));
    assert!((7..=9).contains(&backoff));
}

#[tokio::test]
async fn auth_failure_pauses_whole_batch_and_gate_blocks_next_call() {
    let statuses = Arc::new(Mutex::new(vec![401]));
    let transport = Arc::new(ScriptedPixiv {
        statuses: Arc::clone(&statuses),
        calls: Arc::new(Mutex::new(0)),
    });
    let adapter = PixivSource::new(transport);
    let error = adapter.fetch_work("123").await.unwrap_err();
    assert_eq!(error.kind, SourceErrorKind::Auth);
    let state = adapter.gate_state().snapshot().await;
    assert!(state.paused_until > now());
    let blocked = adapter.fetch_author("456").await.unwrap_err();
    assert_eq!(blocked.kind, SourceErrorKind::RateLimited);
}

#[tokio::test]
async fn not_found_marks_work_deleted_without_removing_metadata() {
    let mut db = Db::open_in_memory().unwrap();
    let mut adapter = MockSource::new("mock");
    adapter.bookmark_pages.push(vec![work_page("gone")]);
    ingest::ingest_bookmarks(&mut db, &adapter, "user")
        .await
        .unwrap();
    let work_id: String = db
        .connection()
        .query_row("SELECT id FROM works", [], |row| row.get(0))
        .unwrap();
    let item_count = work_count(&db);

    let error = SourceError::not_found("removed");
    ingest::handle_source_error(&db, &adapter, Some(&work_id), &error)
        .await
        .unwrap();
    let status: String = db
        .connection()
        .query_row(
            "SELECT status FROM works WHERE id=?1",
            params![work_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "deleted");
    assert_eq!(work_count(&db), item_count);
}

#[tokio::test]
async fn api_gate_uses_exponential_backoff_on_repeated_429() {
    let gate = ApiGate::new();
    let mut delays = Vec::new();
    for _ in 0..3 {
        let error = SourceError::rate_limited(None);
        let until = gate.record_error(&error).await;
        delays.push(until - now());
    }
    assert!(delays[1] > delays[0]);
    assert!(delays[2] >= delays[1]);
    assert!(classify_status(404, None).unwrap_err().kind == SourceErrorKind::NotFound);
}

#[allow(dead_code)]
fn unused_path(_: &PathBuf) {}
