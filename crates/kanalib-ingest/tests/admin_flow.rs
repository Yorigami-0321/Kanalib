//! 管理页 API：审核放行/拒绝、规则草稿应用与快照重载。

use std::{net::SocketAddr, path::Path, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use kanalib_ingest::{
    admin::{self, AdminState},
    db::Db,
    metrics::Metrics,
    rules_state::RulesState,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

const RULES: &str = r#"# operator notes stay put
[rules]
version = "2026-11-22.1"

[rules.blacklist]
users = ["999"]
tags = ["existing"]
watch = ["tentative"]

[rules.tag_budget]
min_tags = 3
"#;

fn item_id(seed: char) -> String {
    seed.to_string().repeat(40)
}

struct Fixture {
    _dir: TempDir,
    db_path: std::path::PathBuf,
    rules_path: std::path::PathBuf,
    state: Arc<RulesState>,
}

fn fixture() -> Fixture {
    fixture_with(RULES)
}

fn fixture_with(rules: &str) -> Fixture {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("ingest.db");
    let rules_path = dir.path().join("rules.toml");
    std::fs::write(&rules_path, rules).unwrap();
    let db = Db::open(&db_path).unwrap();
    seed(&db, dir.path());
    let state = Arc::new(RulesState::load(&db, Some(&rules_path)).unwrap());
    Fixture {
        _dir: dir,
        db_path,
        rules_path,
        state,
    }
}

fn seed(db: &Db, dir: &Path) {
    let thumb = dir.join("thumb.png");
    std::fs::write(&thumb, b"\x89PNG\r\n\x1a\n").unwrap();
    let clean = item_id('a');
    let r18g = item_id('b');
    db.connection()
        .execute(
            "INSERT INTO authors(id, source, external_id, name)
             VALUES ('author', 'pixiv', '123', 'Author')",
            [],
        )
        .unwrap();
    db.connection()
        .execute(
            "INSERT INTO works(id, source, external_id, author_id, title, rating, x_restrict)
             VALUES ('work-a', 'pixiv', '100', 'author', 'Clean', 'all', 0),
                    ('work-b', 'pixiv', '101', 'author', 'Terminal', 'all', 2)",
            [],
        )
        .unwrap();
    db.connection()
        .execute(
            "INSERT INTO items(id, work_id, source, decision, decision_reason, rules_version,
                               thumb_512, width, height)
             VALUES (?1, 'work-a', 'pixiv', 'review', 'watch:tentative', '2026-11-22.1', ?3, 900, 900),
                    (?2, 'work-b', 'pixiv', 'review', 'r18g', '2026-11-22.1', ?3, 900, 900)",
            rusqlite::params![clean, r18g, thumb.display().to_string()],
        )
        .unwrap();
    db.connection()
        .execute(
            "INSERT INTO tags(id, name) VALUES (1, 'art'), (2, 'R-18G')",
            [],
        )
        .unwrap();
    db.connection()
        .execute(
            "INSERT INTO item_tags(item_id, tag_id) VALUES (?1, 1), (?2, 2)",
            rusqlite::params![clean, r18g],
        )
        .unwrap();
    db.connection()
        .execute(
            "INSERT INTO decisions(item_id, decision, reason, rules_version, matched_rules_json,
                                   tag_source, tag_count_used)
             VALUES (?1, 'review', 'watch:tentative', '2026-11-22.1', '[\"watch:tentative\"]', 'source', 1)",
            rusqlite::params![clean],
        )
        .unwrap();
}

fn app(fixture: &Fixture) -> Router {
    admin::router(AdminState::new(
        fixture.db_path.clone(),
        Arc::clone(&fixture.state),
        Some(fixture.rules_path.clone()),
        Arc::new(Metrics::default()),
    ))
}

fn authed_app(fixture: &Fixture) -> Router {
    admin::router(
        AdminState::new(
            fixture.db_path.clone(),
            Arc::clone(&fixture.state),
            Some(fixture.rules_path.clone()),
            Arc::new(Metrics::default()),
        )
        .with_auth(Some("test-token".to_string()), Vec::new()),
    )
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn post(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn decision(db_path: &Path, id: &str) -> String {
    let db = Db::open_read(db_path).unwrap();
    db.connection()
        .query_row("SELECT decision FROM items WHERE id=?1", [id], |row| {
            row.get(0)
        })
        .unwrap()
}

#[tokio::test]
async fn health_status_and_review_list_expose_current_state() {
    let fixture = fixture();
    let app = app(&fixture);

    let response = app.clone().oneshot(get("/admin/api/health")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let health = body_json(response).await;
    assert_eq!(health["ok"], true);
    assert_eq!(health["rules_version"], "2026-11-22.1");

    let response = app.clone().oneshot(get("/admin/api/status")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let status = body_json(response).await;
    assert_eq!(status["rules"]["version"], "2026-11-22.1");
    assert_eq!(status["rules"]["tag_blacklist_empty"], false);
    assert_eq!(status["review"]["count"], 2);

    let response = app.oneshot(get("/admin/api/review")).await.unwrap();
    let review = body_json(response).await;
    let items = review.as_array().unwrap();
    assert_eq!(items.len(), 2);
    let terminal = items
        .iter()
        .find(|item| item["r18g"] == true)
        .expect("r18g item");
    assert_eq!(terminal["allowable"], false);
}

#[tokio::test]
async fn admin_api_requires_token_when_configured() {
    let fixture = fixture();
    let app = authed_app(&fixture);

    let response = app.clone().oneshot(get("/admin/api/health")).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "liveness probe must not require the admin token"
    );

    let response = app.clone().oneshot(get("/admin/api/status")).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let request = Request::builder()
        .uri("/admin/api/status")
        .header("x-admin-token", "test-token")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_api_enforces_allowed_ips_when_connect_info_is_present() {
    let fixture = fixture();
    let app = admin::router(
        AdminState::new(
            fixture.db_path.clone(),
            Arc::clone(&fixture.state),
            Some(fixture.rules_path.clone()),
            Arc::new(Metrics::default()),
        )
        .with_auth(None, vec!["127.0.0.1".parse().unwrap()]),
    );

    let allowed = Request::builder()
        .uri("/admin/api/status")
        .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1234))))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(allowed).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let denied = Request::builder()
        .uri("/admin/api/status")
        .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 2], 1234))))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(denied).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn allow_updates_item_and_keeps_matched_rules_json() {
    let fixture = fixture();
    let app = app(&fixture);
    let id = item_id('a');

    let response = app
        .clone()
        .oneshot(post(
            &format!("/admin/api/review/{id}/allow"),
            json!({ "reviewer": "tester" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(decision(&fixture.db_path, &id), "allow");

    let db = Db::open_read(&fixture.db_path).unwrap();
    let (matched, reviewer): (String, Option<String>) = db
        .connection()
        .query_row(
            "SELECT d.matched_rules_json, i.reviewed_by
               FROM items i JOIN decisions d ON d.item_id = i.id
              WHERE i.id = ?1 ORDER BY d.id DESC LIMIT 1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(matched, "[\"watch:tentative\"]");
    assert!(serde_json::from_str::<Value>(&matched).is_ok());
    assert_eq!(reviewer.as_deref(), Some("tester"));
}

#[tokio::test]
async fn r18g_review_item_cannot_be_allowed() {
    let fixture = fixture();
    let app = app(&fixture);
    let id = item_id('b');

    let response = app
        .oneshot(post(
            &format!("/admin/api/review/{id}/allow"),
            json!({ "reviewer": "tester" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(decision(&fixture.db_path, &id), "review");
}

#[tokio::test]
async fn proposal_apply_rewrites_file_bumps_version_and_reloads() {
    let fixture = fixture();
    let app = app(&fixture);

    let response = app
        .clone()
        .oneshot(post(
            "/admin/api/proposals",
            json!({ "kind": "add_tag", "value": "GROO", "note": "from review" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let created = body_json(response).await;
    let proposal_id = created["id"].as_i64().unwrap();

    let response = app
        .clone()
        .oneshot(get(&format!("/admin/api/proposals/{proposal_id}/preview")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let preview = body_json(response).await;
    assert_eq!(preview["from_version"], "2026-11-22.1");
    assert_eq!(preview["to_version"], "2026-11-22.2");
    assert!(preview["summary"].as_str().unwrap().contains("groo"));
    let diff = preview["diff"].as_array().unwrap();
    assert!(
        diff.iter()
            .any(|line| line["kind"] == "add" && line["text"].as_str().unwrap().contains("groo")),
        "preview must carry the added line"
    );

    let response = app
        .clone()
        .oneshot(post(
            &format!("/admin/api/proposals/{proposal_id}/apply"),
            json!({ "reviewer": "tester" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let applied = body_json(response).await;
    assert_eq!(applied["version"], "2026-11-22.2");

    let text = std::fs::read_to_string(&fixture.rules_path).unwrap();
    assert!(text.contains("# operator notes stay put"));
    assert!(text.contains("\"groo\""));
    assert_eq!(fixture.state.current().version, "2026-11-22.2");
    assert!(
        fixture
            .state
            .current()
            .config
            .rules
            .blacklist
            .tags
            .contains(&"groo".to_string())
    );

    let response = app
        .oneshot(get("/admin/api/proposals?state=pending"))
        .await
        .unwrap();
    let pending = body_json(response).await;
    assert!(pending.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn recheck_requeues_review_and_requires_force_for_allow() {
    let fixture = fixture();
    let app = app(&fixture);
    let id = item_id('a');

    let response = app
        .clone()
        .oneshot(post(
            &format!("/admin/api/items/{id}/recheck"),
            json!({ "force": false }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(decision(&fixture.db_path, &id), "pending");

    let response = app
        .clone()
        .oneshot(post(
            &format!("/admin/api/review/{id}/allow"),
            json!({ "reviewer": "tester" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "pending is not review"
    );

    let db = Db::open(&fixture.db_path).unwrap();
    db.apply_decision(&id, "allow", "manual", &[], "2026-11-22.1", "source", 1)
        .unwrap();
    drop(db);

    let response = app
        .clone()
        .oneshot(post(
            &format!("/admin/api/items/{id}/recheck"),
            json!({ "force": false }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let response = app
        .oneshot(post(
            &format!("/admin/api/items/{id}/recheck"),
            json!({ "force": true }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(decision(&fixture.db_path, &id), "pending");
}

#[tokio::test]
async fn admin_page_contains_diff_preview_panel() {
    let fixture = fixture();
    let app = app(&fixture);

    let response = app.oneshot(get("/admin")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(html.contains("id=\"diff-panel\""), "{html}");
    assert!(html.contains("previewProposal("), "{html}");
}

#[tokio::test]
async fn empty_tag_blacklist_is_exposed_in_status() {
    let fixture = fixture_with("[rules]\nversion = \"2026-11-22.1\"\n");
    let app = app(&fixture);

    let response = app.oneshot(get("/admin/api/status")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let status = body_json(response).await;
    assert_eq!(status["rules"]["tag_blacklist_empty"], true);
}

#[tokio::test]
async fn metrics_expose_rules_version_with_config_hash() {
    let fixture = fixture();
    let app = app(&fixture);

    let response = app.oneshot(get("/admin/api/metrics")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        text.contains("rules_version_info{version=\"2026-11-22.1\",config_hash=\""),
        "{text}"
    );
    assert!(text.contains("} 1\n"), "{text}");
}
