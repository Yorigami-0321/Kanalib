//! I2 端到端：mock 来源 → 真实下载 → 规则决策（含 HTTP 补标），校验审计一致性。

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use axum::{Json, Router, extract::State, http::HeaderMap, http::StatusCode, routing::post};
use image::{ImageFormat, RgbaImage};
use kanalib_ingest::{
    db::Db,
    download::{self, DownloadConfig, ImageResponse, ImageTransport},
    ingest::{self, item_id},
    metrics::Metrics,
    pipeline, rules,
    rules_state::{ReloadOutcome, RulesState},
    source::{
        Rating, SourceAuthor, SourceError, SourceTag,
        mock::{MockPage, MockSource},
    },
    tagging::{HttpTaggingBackend, TaggingBackend},
};
use serde_json::{Value, json};
use tempfile::TempDir;

const RULES: &str = r#"
[rules]
version = "test-rules"

[rules.blacklist]
users = ["blocked-author"]
tags = ["guro", "blocked-tag"]
watch = ["tentative"]

[rules.tag_budget]
min_tags = 3
"#;

fn rules_config() -> Arc<rules::RuleConfig> {
    rules::load_str(RULES).unwrap().config
}

fn metrics() -> Metrics {
    Metrics::default()
}

fn png(tint: [u8; 3]) -> Vec<u8> {
    let image = RgbaImage::from_fn(600, 600, |x, y| {
        let shade = ((x + y) % 16) as u8;
        image::Rgba([tint[0], tint[1].saturating_add(shade), tint[2], 255])
    });
    let mut bytes = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    bytes
}

/// 按 URL 返回不同图片，保证不同作品 SHA-1 不同；相同 URL 复用同一份字节（去重用例）。
struct UrlImages {
    bytes: HashMap<String, Vec<u8>>,
}

impl UrlImages {
    fn for_pages(pages: &[MockPage]) -> Self {
        let mut bytes = HashMap::new();
        let mut tint = 10u8;
        for page in pages {
            bytes.entry(page.image_url.clone()).or_insert_with(|| {
                let generated = png([tint, 60, 180]);
                tint = tint.wrapping_add(37);
                generated
            });
        }
        Self { bytes }
    }
}

impl ImageTransport for UrlImages {
    fn fetch<'a>(
        &self,
        url: &str,
        _range_start: Option<u64>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ImageResponse, SourceError>> + Send + 'a>,
    > {
        let url = url.to_string();
        let body = self.bytes.get(&url).cloned();
        Box::pin(async move {
            body.map(|body| ImageResponse::ok(body, "image/png"))
                .ok_or_else(|| SourceError::not_found(format!("no image for {url}")))
        })
    }
}

#[derive(Clone)]
struct TaggerState {
    status: u16,
    responses: Arc<HashMap<String, Vec<String>>>,
    fallback: Arc<Vec<String>>,
    calls: Arc<AtomicUsize>,
}

async fn tag_handler(
    State(state): State<TaggerState>,
    headers: HeaderMap,
    _body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    state.calls.fetch_add(1, Ordering::SeqCst);
    let item = headers
        .get("x-kanalib-item-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let tags = state
        .responses
        .get(item)
        .cloned()
        .unwrap_or_else(|| state.fallback.as_ref().clone());
    (
        StatusCode::from_u16(state.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(json!({ "tags": tags })),
    )
}

async fn start_tagger(
    status: u16,
    responses: HashMap<String, Vec<String>>,
    fallback: Vec<String>,
) -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/tag", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let state = TaggerState {
        status,
        responses: Arc::new(responses),
        fallback: Arc::new(fallback),
        calls: Arc::clone(&calls),
    };
    let app = Router::new()
        .route("/tag", post(tag_handler))
        .with_state(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (endpoint, calls)
}

fn work(
    external_id: &str,
    author: &str,
    rating: Rating,
    x_restrict: Option<i64>,
    restriction_attributes: &[&str],
    tags: &[&str],
) -> MockPage {
    let mut work = MockSource::work(external_id, 1);
    work.author = Some(SourceAuthor {
        external_id: author.to_string(),
        name: format!("Author {author}"),
        avatar_url: None,
    });
    work.rating = rating;
    work.x_restrict = x_restrict;
    work.restriction_attributes = restriction_attributes
        .iter()
        .map(|value| value.to_string())
        .collect();
    work.tags = tags
        .iter()
        .map(|tag| SourceTag {
            name: tag.to_string(),
            category: "general".to_string(),
            score: 0.5,
            translated_name: None,
        })
        .collect();
    MockPage {
        image_url: format!("mock://image/{external_id}/0"),
        work,
    }
}

async fn ingest_and_download(db: &mut Db, pages: &[MockPage], dir: &Path) -> Result<()> {
    let mut adapter = MockSource::new("mock");
    adapter.bookmark_pages = vec![pages.to_vec()];
    ingest::ingest_bookmarks(db, &adapter, "user").await?;
    let config = DownloadConfig {
        media_dir: dir.to_path_buf(),
        concurrency: 2,
        retry_max: 1,
        timeout: Duration::from_secs(5),
    };
    let report = download::run_queue(db, &adapter, &UrlImages::for_pages(pages), &config).await?;
    assert!(
        report.failed.is_empty(),
        "download failed: {:?}",
        report.failed
    );
    assert_eq!(report.succeeded.len(), pages.len());
    Ok(())
}

fn item_decision(db: &Db, item: &str) -> String {
    db.connection()
        .query_row("SELECT decision FROM items WHERE id=?1", [item], |row| {
            row.get(0)
        })
        .unwrap()
}

fn latest_audit(db: &Db, item: &str) -> (String, String, String, i64) {
    db.connection()
        .query_row(
            "SELECT decision, reason, tag_source, tag_count_used FROM decisions
              WHERE item_id=?1 ORDER BY id DESC LIMIT 1",
            [item],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
}

fn assert_audits_match_items(db: &Db) {
    let mut statement = db
        .connection()
        .prepare("SELECT id, decision FROM items ORDER BY id")
        .unwrap();
    let items: Vec<(String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let audits: i64 = db
        .connection()
        .query_row("SELECT COUNT(*) FROM decisions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(audits as usize, items.len(), "每次决策必须且只落一行审计");
    for (item, decision) in items {
        let (audited, reason, _, _) = latest_audit(db, &item);
        assert_eq!(
            audited, decision,
            "items/decisions 不一致: {item} ({reason})"
        );
    }
}

fn model_tag_count(db: &Db, item: &str) -> i64 {
    db.connection()
        .query_row(
            "SELECT COUNT(*) FROM item_tags WHERE item_id=?1 AND origin='model'",
            [item],
            |row| row.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn chain_decides_allow_block_and_review_with_matching_audits() {
    let dir = TempDir::new().unwrap();
    let mut db = Db::open_in_memory().unwrap();
    let pages = vec![
        work(
            "clean",
            "author-a",
            Rating::All,
            Some(0),
            &[],
            &["art", "sky", "sea"],
        ),
        work(
            "blacklisted",
            "author-b",
            Rating::All,
            Some(0),
            &[],
            &["art", "guro", "sky"],
        ),
        work(
            "watched",
            "author-c",
            Rating::All,
            Some(0),
            &[],
            &["art", "tentative", "sky"],
        ),
        work(
            "blocked-author-work",
            "blocked-author",
            Rating::All,
            Some(0),
            &[],
            &["art", "sky", "sea"],
        ),
    ];
    ingest_and_download(&mut db, &pages, dir.path())
        .await
        .unwrap();

    let metrics = metrics();
    let report = pipeline::run_decisions(&db, &rules_config(), None, &metrics)
        .await
        .unwrap();
    assert_eq!(report.decided, 4);
    assert_eq!(report.allowed, 1);
    assert_eq!(report.blocked, 2);
    assert_eq!(report.reviewed, 1);
    assert_eq!(report.tagged, 0);
    let rendered = metrics.render("");
    assert!(rendered.contains("decision_total{decision=\"allow\"} 1"));
    assert!(rendered.contains("decision_total{decision=\"block\"} 2"));
    assert!(rendered.contains("decision_total{decision=\"review\"} 1"));
    assert!(rendered.contains("decision_reason_total{kind=\"tag_blacklist\"} 1"));
    assert!(rendered.contains("decision_reason_total{kind=\"watch\"} 1"));

    let clean = item_id("mock", "clean", 0);
    let blacklisted = item_id("mock", "blacklisted", 0);
    let watched = item_id("mock", "watched", 0);
    let blocked_author = item_id("mock", "blocked-author-work", 0);

    assert_eq!(item_decision(&db, &clean), "allow");
    assert_eq!(latest_audit(&db, &clean).2, "source");

    assert_eq!(item_decision(&db, &blacklisted), "block");
    let (decision, reason, _, _) = latest_audit(&db, &blacklisted);
    assert_eq!(decision, "block");
    assert_eq!(reason, "tag blacklist");

    assert_eq!(item_decision(&db, &watched), "review");
    assert_eq!(latest_audit(&db, &watched).1, "watch:tentative");

    assert_eq!(item_decision(&db, &blocked_author), "block");
    assert_eq!(latest_audit(&db, &blocked_author).1, "author blacklist");

    assert_audits_match_items(&db);
}

#[tokio::test]
async fn direct_decisions_never_call_tagger() {
    let dir = TempDir::new().unwrap();
    let mut db = Db::open_in_memory().unwrap();
    let pages = vec![
        work(
            "direct-clean",
            "author-a",
            Rating::All,
            Some(0),
            &[],
            &["art", "sky", "sea"],
        ),
        work(
            "direct-watch",
            "author-b",
            Rating::All,
            Some(0),
            &[],
            &["tentative"],
        ),
        work(
            "direct-blocked-tag",
            "author-d",
            Rating::All,
            Some(0),
            &[],
            &["guro"],
        ),
        work(
            "direct-blocked-author",
            "blocked-author",
            Rating::All,
            Some(0),
            &[],
            &["art"],
        ),
    ];
    ingest_and_download(&mut db, &pages, dir.path())
        .await
        .unwrap();
    let config = rules::load_str(
        r#"
[rules]
version = "direct-rules"
[rules.blacklist]
users = ["blocked-author"]
tags = ["guro"]
watch = ["tentative"]
[rules.tag_budget]
min_tags = 3
"#,
    )
    .unwrap()
    .config;
    let (endpoint, calls) = start_tagger(
        200,
        HashMap::new(),
        vec!["bird".to_string(), "cat".to_string(), "dog".to_string()],
    )
    .await;
    let backend: Arc<dyn TaggingBackend> =
        Arc::new(HttpTaggingBackend::new(endpoint, Duration::from_secs(5), 2).unwrap());

    let report = pipeline::run_decisions(&db, &config, Some(&backend), &metrics())
        .await
        .unwrap();
    assert_eq!(report.decided, 4);
    assert_eq!(report.allowed, 1);
    assert_eq!(report.blocked, 2);
    assert_eq!(report.reviewed, 1);
    assert_eq!(report.tagged, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_audits_match_items(&db);
}

#[tokio::test]
async fn short_tags_are_tagged_once_then_rechecked() {
    let dir = TempDir::new().unwrap();
    let mut db = Db::open_in_memory().unwrap();
    let pages = vec![
        work(
            "need-block",
            "author-a",
            Rating::All,
            Some(0),
            &[],
            &["オリジナル", "漫画"],
        ),
        work(
            "need-allow",
            "author-b",
            Rating::All,
            Some(0),
            &[],
            &["オリジナル", "漫画"],
        ),
        work("no-source-tags", "author-c", Rating::All, Some(0), &[], &[]),
    ];
    ingest_and_download(&mut db, &pages, dir.path())
        .await
        .unwrap();

    let need_block = item_id("mock", "need-block", 0);
    let need_allow = item_id("mock", "need-allow", 0);
    let no_source_tags = item_id("mock", "no-source-tags", 0);
    let mut responses = HashMap::new();
    responses.insert(
        need_block.clone(),
        vec!["guro".to_string(), "art".to_string()],
    );
    responses.insert(
        need_allow.clone(),
        vec!["sky".to_string(), "sea".to_string(), "tree".to_string()],
    );
    responses.insert(
        no_source_tags.clone(),
        vec!["bird".to_string(), "cat".to_string(), "dog".to_string()],
    );
    let (endpoint, calls) = start_tagger(200, responses, vec![]).await;
    let backend: Arc<dyn TaggingBackend> =
        Arc::new(HttpTaggingBackend::new(endpoint, Duration::from_secs(5), 2).unwrap());

    let metrics = metrics();
    let report = pipeline::run_decisions(&db, &rules_config(), Some(&backend), &metrics)
        .await
        .unwrap();
    assert_eq!(report.decided, 3);
    assert_eq!(report.allowed, 2);
    assert_eq!(report.blocked, 1);
    assert_eq!(report.reviewed, 0);
    assert_eq!(report.tagged, 3);
    assert_eq!(calls.load(Ordering::SeqCst), 3, "每条只补标一次");
    let rendered = metrics.render("");
    assert!(rendered.contains("tag_budget_hit_total{source=\"mock\"} 3"));
    assert!(!rendered.contains("tag_budget_short_total{source="));

    assert_eq!(item_decision(&db, &need_block), "block");
    assert_eq!(latest_audit(&db, &need_block).1, "tag blacklist");
    assert_eq!(latest_audit(&db, &need_block).2, "merged");
    assert_eq!(model_tag_count(&db, &need_block), 2);

    assert_eq!(item_decision(&db, &need_allow), "allow");
    let (_, _, tag_source, tag_count) = latest_audit(&db, &need_allow);
    assert_eq!(tag_source, "merged");
    assert_eq!(tag_count, 5, "2 条来源 + 3 条补标");
    assert_eq!(model_tag_count(&db, &need_allow), 3);

    assert_eq!(item_decision(&db, &no_source_tags), "allow");
    let (_, _, tag_source, tag_count) = latest_audit(&db, &no_source_tags);
    assert_eq!(tag_source, "model");
    assert_eq!(tag_count, 3);

    assert_audits_match_items(&db);
}

#[tokio::test]
async fn tagger_failure_empty_tags_and_missing_backend_never_allow() {
    let pages = vec![work(
        "short",
        "author-a",
        Rating::All,
        Some(0),
        &[],
        &["オリジナル"],
    )];

    // 500：重试后失败 → review
    let dir = TempDir::new().unwrap();
    let mut db = Db::open_in_memory().unwrap();
    ingest_and_download(&mut db, &pages, dir.path())
        .await
        .unwrap();
    let short = item_id("mock", "short", 0);
    let (endpoint, calls) = start_tagger(500, HashMap::new(), vec!["art".to_string()]).await;
    let backend: Arc<dyn TaggingBackend> =
        Arc::new(HttpTaggingBackend::new(endpoint, Duration::from_secs(5), 2).unwrap());
    let report = pipeline::run_decisions(&db, &rules_config(), Some(&backend), &metrics())
        .await
        .unwrap();
    assert_eq!(report.reviewed, 1);
    assert_eq!(report.allowed, 0);
    assert_eq!(item_decision(&db, &short), "review");
    assert_eq!(latest_audit(&db, &short).1, "model tagging failed");
    assert_eq!(model_tag_count(&db, &short), 0, "失败不得写入标签");
    assert_eq!(calls.load(Ordering::SeqCst), 2, "固定重试一次");

    // 200 但空标签 → 视为失败 → review
    let dir = TempDir::new().unwrap();
    let mut db = Db::open_in_memory().unwrap();
    ingest_and_download(&mut db, &pages, dir.path())
        .await
        .unwrap();
    let (endpoint, _) = start_tagger(200, HashMap::new(), vec![]).await;
    let backend: Arc<dyn TaggingBackend> =
        Arc::new(HttpTaggingBackend::new(endpoint, Duration::from_secs(5), 2).unwrap());
    pipeline::run_decisions(&db, &rules_config(), Some(&backend), &metrics())
        .await
        .unwrap();
    assert_eq!(item_decision(&db, &short), "review");

    // 没有后端 → review，不允许只看来源标签放行
    let dir = TempDir::new().unwrap();
    let mut db = Db::open_in_memory().unwrap();
    ingest_and_download(&mut db, &pages, dir.path())
        .await
        .unwrap();
    pipeline::run_decisions(&db, &rules_config(), None, &metrics())
        .await
        .unwrap();
    assert_eq!(item_decision(&db, &short), "review");
    assert_eq!(
        latest_audit(&db, &short).1,
        "tag budget short; tagger unavailable"
    );
}

#[tokio::test]
async fn r18g_three_routes_block_and_duplicate_hash_blocks_second_item() {
    let dir = TempDir::new().unwrap();
    let mut db = Db::open_in_memory().unwrap();
    let pages = vec![
        work(
            "r18g-rating",
            "author-a",
            Rating::R18G,
            Some(0),
            &[],
            &["art", "sky", "sea"],
        ),
        work(
            "r18g-xrestrict",
            "author-b",
            Rating::All,
            Some(2),
            &[],
            &["art", "sky", "sea"],
        ),
        work(
            "r18g-tag",
            "author-c",
            Rating::All,
            Some(0),
            &[],
            &["art", "R-18G", "sea"],
        ),
        work(
            "r18g-attribute",
            "author-d",
            Rating::All,
            Some(0),
            &["R18G"],
            &["art", "sky", "sea"],
        ),
        work(
            "dup-a",
            "author-e",
            Rating::All,
            Some(0),
            &[],
            &["art", "sky", "sea"],
        ),
        work(
            "dup-b",
            "author-f",
            Rating::All,
            Some(0),
            &[],
            &["art", "sky", "sea"],
        ),
    ];
    // dup-a / dup-b 共用同一 URL，下载出完全相同的字节与 SHA-1
    let mut pages = pages;
    pages[5].image_url = pages[4].image_url.clone();
    ingest_and_download(&mut db, &pages, dir.path())
        .await
        .unwrap();

    let report = pipeline::run_decisions(&db, &rules_config(), None, &metrics())
        .await
        .unwrap();
    assert_eq!(report.decided, 6);
    assert_eq!(report.blocked, 5);
    assert_eq!(report.allowed, 1);

    for (external_id, expected_route) in [
        ("r18g-rating", "rating:x_restrict"),
        ("r18g-xrestrict", "rating:x_restrict"),
        ("r18g-tag", "rating:tag"),
        ("r18g-attribute", "rating:restriction"),
    ] {
        let item = item_id("mock", external_id, 0);
        assert_eq!(item_decision(&db, &item), "block", "{external_id}");
        let (decision, reason, _, _) = latest_audit(&db, &item);
        assert_eq!(decision, "block");
        assert_eq!(reason, "r18g");
        let matched: String = db
            .connection()
            .query_row(
                "SELECT matched_rules_json FROM decisions WHERE item_id=?1
                  ORDER BY id DESC LIMIT 1",
                [&item],
                |row| row.get(0),
            )
            .unwrap();
        assert!(matched.contains(expected_route), "{external_id}: {matched}");
    }

    let dup_a = item_id("mock", "dup-a", 0);
    let dup_b = item_id("mock", "dup-b", 0);
    let mut allow = 0;
    let mut dedup_block = 0;
    for item in [&dup_a, &dup_b] {
        match item_decision(&db, item).as_str() {
            "allow" => allow += 1,
            "block" => {
                assert_eq!(latest_audit(&db, item).1, "duplicate hash");
                dedup_block += 1;
            }
            other => panic!("unexpected decision {other}"),
        }
    }
    assert_eq!((allow, dedup_block), (1, 1));

    assert_audits_match_items(&db);
}

#[tokio::test]
async fn rules_reload_switches_decision_version_end_to_end() {
    let dir = TempDir::new().unwrap();
    let rules_dir = TempDir::new().unwrap();
    let rules_path = rules_dir.path().join("rules.toml");
    std::fs::write(
        &rules_path,
        "[rules]\nversion = \"reload-1\"\n\n[rules.tag_budget]\nmin_tags = 3\n",
    )
    .unwrap();
    let mut db = Db::open_in_memory().unwrap();
    let state = RulesState::load(&db, Some(&rules_path)).unwrap();
    let first_hash = db.rule_snapshot("reload-1").unwrap().unwrap().config_hash;

    let pages = vec![work(
        "late-block",
        "author-a",
        Rating::All,
        Some(0),
        &[],
        &["art", "sky", "sea"],
    )];
    ingest_and_download(&mut db, &pages, dir.path())
        .await
        .unwrap();
    let item = item_id("mock", "late-block", 0);

    // 版本未升但内容变了：拒绝重载，旧快照与旧规则继续生效。
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(
        &rules_path,
        "[rules]\nversion = \"reload-1\"\n\n[rules.blacklist]\ntags = [\"sea\"]\n\n\
         [rules.tag_budget]\nmin_tags = 3\n",
    )
    .unwrap();
    let error = state.reload_if_changed(&db, "test").unwrap_err();
    assert!(error.to_string().contains("bump rules.version"));
    assert_eq!(state.current().version, "reload-1");
    assert_eq!(
        db.rule_snapshot("reload-1").unwrap().unwrap().config_hash,
        first_hash
    );

    // 升版本后重载成功，后续决策必须写新版本号。
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(
        &rules_path,
        "[rules]\nversion = \"reload-2\"\n\n[rules.blacklist]\ntags = [\"sea\"]\n\n\
         [rules.tag_budget]\nmin_tags = 3\n",
    )
    .unwrap();
    assert_eq!(
        state.reload_if_changed(&db, "test").unwrap(),
        ReloadOutcome::Reloaded
    );
    assert_eq!(state.current().version, "reload-2");

    let active = state.current();
    let report = pipeline::run_decisions(&db, &active.config, None, &metrics())
        .await
        .unwrap();
    assert_eq!(report.blocked, 1);
    assert_eq!(item_decision(&db, &item), "block");
    assert_eq!(latest_audit(&db, &item).1, "tag blacklist");
    let decided_version: String = db
        .connection()
        .query_row(
            "SELECT rules_version FROM decisions WHERE item_id=?1
              ORDER BY id DESC LIMIT 1",
            [&item],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(decided_version, "reload-2");
    assert!(db.rule_snapshot("reload-1").unwrap().is_some());
    assert!(db.rule_snapshot("reload-2").unwrap().is_some());
    assert_audits_match_items(&db);
}

#[tokio::test]
async fn one_item_error_is_reviewed_without_blocking_the_rest() {
    let dir = TempDir::new().unwrap();
    let mut db = Db::open_in_memory().unwrap();
    let pages = vec![
        work(
            "needs-model",
            "author-a",
            Rating::All,
            Some(0),
            &[],
            &["オリジナル"],
        ),
        work(
            "clean-next",
            "author-b",
            Rating::All,
            Some(0),
            &[],
            &["art", "sky", "sea"],
        ),
    ];
    ingest_and_download(&mut db, &pages, dir.path())
        .await
        .unwrap();

    // 只让模型补标写入失败，模拟单条落库异常。
    db.connection()
        .execute(
            "CREATE TRIGGER fail_model_tag_insert BEFORE INSERT ON item_tags
             WHEN NEW.origin = 'model'
             BEGIN SELECT RAISE(ABORT, 'model tag write blocked'); END",
            [],
        )
        .unwrap();

    let (endpoint, _) = start_tagger(
        200,
        HashMap::new(),
        vec!["bird".to_string(), "cat".to_string(), "dog".to_string()],
    )
    .await;
    let backend: Arc<dyn TaggingBackend> =
        Arc::new(HttpTaggingBackend::new(endpoint, Duration::from_secs(5), 2).unwrap());

    let report = pipeline::run_decisions(&db, &rules_config(), Some(&backend), &metrics())
        .await
        .unwrap();
    assert_eq!(report.decided, 2, "单条失败不得阻断后一条");
    assert_eq!(report.reviewed, 1);
    assert_eq!(report.allowed, 1);

    let failed = item_id("mock", "needs-model", 0);
    let clean = item_id("mock", "clean-next", 0);
    assert_eq!(item_decision(&db, &failed), "review");
    assert_eq!(latest_audit(&db, &failed).1, "decision error");
    assert_eq!(item_decision(&db, &clean), "allow");
    assert_eq!(latest_audit(&db, &clean).1, "all rules passed");
    assert_audits_match_items(&db);
}
