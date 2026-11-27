//! W1 只读后端集成测试：临时 SQLite + 临时媒体目录。

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use kanalib_wall_model::{IngestBatch, IngestDelete, IngestDeleteItem};
use kanalib_web::{AppState, crypto, db, routes, state::WriteConfig};
use rusqlite::params;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower::ServiceExt;

struct Fixture {
    _dir: tempfile::TempDir,
    public: Router,
    private: Router,
}

fn fixture() -> Fixture {
    build_fixture(false)
}

fn web_fixture() -> Fixture {
    build_fixture(true)
}

fn build_fixture(with_web: bool) -> Fixture {
    build_fixture_with(with_web, None)
}

fn build_fixture_with(with_web: bool, writes: Option<WriteConfig>) -> Fixture {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("kanalib_web=debug")
        .try_init();
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("kanalib.db");
    db::seed_sample(&db_path).expect("seed sample");

    let media = dir.path().join("media");
    for tier in ["t256", "t512", "t1024", "t2048"] {
        std::fs::create_dir_all(media.join(tier)).expect("media tier");
    }
    std::fs::write(
        media
            .join("t512")
            .join(format!("{}.jpg", db::SAMPLE_ITEM_A)),
        b"0123456789abcdef",
    )
    .expect("write t512");
    std::fs::write(
        media
            .join("t1024")
            .join(format!("{}.jpg", db::SAMPLE_ITEM_A)),
        b"t1024-payload",
    )
    .expect("write t1024");

    let web_root = with_web.then(|| {
        let web = dir.path().join("web");
        std::fs::create_dir_all(&web).expect("web root");
        std::fs::write(web.join("index.html"), b"<html>kanalib-spa</html>").expect("index");
        web
    });

    let state = AppState::new(db_path, media, web_root);
    let state = match writes {
        Some(writes) => state.with_writes(writes),
        None => state,
    };
    Fixture {
        _dir: dir,
        public: routes::public_router(state.clone()),
        private: routes::private_router(state),
    }
}

fn seed_extra_items(db_path: &std::path::Path, count: usize) {
    let mut conn = db::open(db_path).expect("open writable db");
    let tx = conn.transaction().expect("transaction");
    tx.execute(
        "INSERT OR IGNORE INTO tags (id, name, category) VALUES
            (10, 'tag-even', 'general'),
            (11, 'tag-odd', 'general'),
            (12, 'common', 'general')",
        [],
    )
    .expect("seed tags");
    for index in 0..count {
        let id = format!("{:040x}", 1000 + index);
        let group = ["alpha", "beta", "gamma"][index % 3];
        let name = format!("{group}-{index:02}");
        let rating = ["G", "S", "Q", "E"][index % 4];
        let star = (index % 6) as i64;
        let favorite = i64::from(index % 2 == 0);
        let timestamp = 1_700_100_000 + index as i64;
        tx.execute(
            "INSERT INTO items
                (id, source, title, name, width, height, rating, star, favorite,
                 mtime, group_name, deleted, created_at, updated_at)
             VALUES (?1, 'pixiv', '', ?2, 100, 100, ?3, ?4, ?5, ?6, ?7, 0, ?6, ?6)",
            params![&id, name, rating, star, favorite, timestamp, group],
        )
        .expect("insert extra item");
        let primary = if index % 2 == 0 { 10 } else { 11 };
        tx.execute(
            "INSERT INTO item_tags (item_id, tag_id, score, origin) VALUES
                (?1, ?2, 0.5, 'ingest'),
                (?1, 12, 0.4, 'ingest')",
            params![&id, primary],
        )
        .expect("insert extra tags");
    }
    db::rebuild_search_index(&tx).expect("rebuild fts");
    tx.commit().expect("commit extras");
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .expect("checkpoint extras");
}

fn extra_fixture() -> Fixture {
    let fixture = fixture();
    seed_extra_items(&fixture._dir.path().join("kanalib.db"), 30);
    fixture
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(Body::empty()).expect("request");
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, headers, body)
}

async fn get_json(app: &Router, uri: &str) -> (StatusCode, Value) {
    let (status, _, body) = send(app, Method::GET, uri, &[]).await;
    let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, value)
}

fn header_text(headers: &HeaderMap, name: header::HeaderName) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn health_and_index() {
    let fixture = fixture();

    let (status, body) = get_json(&fixture.public, "/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["db"], true);
    assert_eq!(body["media_root"], true);

    let (status, body) = get_json(&fixture.public, "/api/index").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["schema"], 2);
    assert_eq!(body["api_version"], 1);
    assert_eq!(body["count"], 2);
    assert_eq!(body["counts"]["works"], 1);
    assert_eq!(body["counts"]["favorite"], 2);
    assert_eq!(body["counts"]["ratings"]["g"], 1);
    assert_eq!(body["counts"]["ratings"]["s"], 1);
    assert_eq!(body["capabilities"]["source"], "api");
    assert_eq!(body["capabilities"]["server_filter"], true);
    assert_eq!(body["capabilities"]["server_search"], true);
    assert_eq!(body["capabilities"]["can_write"], false);
    assert_eq!(body["capabilities"]["has_works"], true);
    assert_eq!(body["paths"]["t512"], "/media/t512/");
    let tags = body["facets"]["tags"].as_array().expect("tags");
    assert!(!tags.is_empty());
    assert!(tags[0]["cover"].is_string(), "tag facet needs a cover id");
    let groups = body["facets"]["groups"].as_array().expect("groups");
    assert_eq!(groups[0]["name"], "原创");
    assert_eq!(groups[0]["count"], 2);
    assert!(
        groups[0]["cover"].is_string(),
        "group facet needs a cover id"
    );
}

#[tokio::test]
async fn items_list_and_detail() {
    let fixture = fixture();

    let (status, body) = get_json(&fixture.public, "/api/items?limit=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().expect("items").len(), 1);
    assert!(body["total"].is_null());
    let (status, body) = get_json(&fixture.public, "/api/index").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["count"], 2);

    let uri = format!("/api/items/{}", db::SAMPLE_ITEM_A);
    let (status, body) = get_json(&fixture.public, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], db::SAMPLE_ITEM_A);
    assert_eq!(body["rt"], "G");
    assert_eq!(body["wid"], db::SAMPLE_WORK);
    assert_eq!(body["pc"], 2);
    assert!(body["tags"].as_array().expect("tags").len() >= 2);

    let (status, headers, _) = send(&fixture.public, Method::GET, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(header_text(&headers, header::ETAG).starts_with("W/\""));

    let missing = format!("/api/items/{}", "c".repeat(40));
    let (status, body) = get_json(&fixture.public, &missing).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");

    let (status, _) = get_json(&fixture.public, "/api/items/not-an-id").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn works_authors_collections_and_tags() {
    let fixture = fixture();

    let (status, body) = get_json(&fixture.public, "/api/works").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["id"], db::SAMPLE_WORK);

    let uri = format!("/api/works/{}", db::SAMPLE_WORK);
    let (status, body) = get_json(&fixture.public, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["page_count"], 2);
    assert_eq!(body["pages"].as_array().expect("pages").len(), 2);
    assert_eq!(body["pages"][0]["page"], 0);
    assert_eq!(body["pages"][0]["id"], db::SAMPLE_ITEM_A);
    assert_eq!(body["author"]["id"], db::SAMPLE_AUTHOR);

    let (status, body) = get_json(&fixture.public, "/api/authors").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["work_count"], 1);
    assert_eq!(body["items"][0]["item_count"], 2);

    let uri = format!("/api/authors/{}/works", db::SAMPLE_AUTHOR);
    let (status, body) = get_json(&fixture.public, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);

    let (status, body) = get_json(&fixture.public, "/api/collections").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["item_count"], 2);
    assert_eq!(body["items"][0]["work_count"], 1);

    let (status, body) = get_json(&fixture.public, "/api/collections/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 2);

    let (status, body) = get_json(&fixture.public, "/api/collections/1/works").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);

    let (status, _) = get_json(&fixture.public, "/api/collections/999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = get_json(&fixture.public, "/api/tags?limit=10").await;
    assert_eq!(status, StatusCode::OK);
    let tags = body["items"].as_array().expect("tags");
    let first = tags
        .iter()
        .find(|tag| tag["name"] == "1girl")
        .expect("1girl facet");
    assert_eq!(first["count"], 2);
}

#[tokio::test]
async fn media_full_fallback_headers() {
    let fixture = fixture();
    let uri = format!("/media/t256/{}.jpg", db::SAMPLE_ITEM_A);
    let (status, headers, body) = send(&fixture.public, Method::GET, &uri, &[]).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"0123456789abcdef");
    assert_eq!(header_text(&headers, header::ACCEPT_RANGES), "bytes");
    assert_eq!(
        header_text(&headers, header::CACHE_CONTROL),
        "public, max-age=3600"
    );
    assert_eq!(header_text(&headers, header::CONTENT_TYPE), "image/jpeg");
    assert!(header_text(&headers, header::ETAG).starts_with("W/\""));
    assert_eq!(header_text(&headers, header::CONTENT_LENGTH), "16");

    let (status, headers, body) = send(&fixture.public, Method::HEAD, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty());
    assert_eq!(header_text(&headers, header::CONTENT_LENGTH), "16");
}

#[tokio::test]
async fn media_negotiates_avif_and_keeps_jpeg_fallback() {
    let fixture = fixture();
    let avif = b"\x00\x00\x00\x18ftypavif\x00\x00\x00\x00avifmif1".to_vec();
    std::fs::write(
        fixture
            ._dir
            .path()
            .join(format!("media/t256/{}.avif", db::SAMPLE_ITEM_A)),
        &avif,
    )
    .expect("write avif");
    let uri = format!("/media/t256/{}.jpg", db::SAMPLE_ITEM_A);

    let (status, headers, body) = send(
        &fixture.public,
        Method::GET,
        &uri,
        &[("accept", "image/avif,image/webp,*/*;q=0.8")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, avif);
    assert_eq!(header_text(&headers, header::CONTENT_TYPE), "image/avif");
    assert_eq!(header_text(&headers, header::VARY), "Accept");

    let (status, headers, body) = send(
        &fixture.public,
        Method::GET,
        &uri,
        &[("accept", "image/jpeg")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"0123456789abcdef");
    assert_eq!(header_text(&headers, header::CONTENT_TYPE), "image/jpeg");

    let (status, headers, body) = send(
        &fixture.public,
        Method::GET,
        &uri,
        &[("accept", "image/avif;q=0,image/jpeg")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"0123456789abcdef");
    assert_eq!(header_text(&headers, header::CONTENT_TYPE), "image/jpeg");

    let (status, headers, body) = send(
        &fixture.public,
        Method::GET,
        &uri,
        &[("accept", "image/avif;q=0,*/*;q=1")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"0123456789abcdef");
    assert_eq!(header_text(&headers, header::CONTENT_TYPE), "image/jpeg");

    let (status, headers, body) = send(
        &fixture.public,
        Method::GET,
        &uri,
        &[("accept", "IMAGE/AVIF;Q=1")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, avif);
    assert_eq!(header_text(&headers, header::CONTENT_TYPE), "image/avif");

    let (status, headers, body) = send(&fixture.public, Method::GET, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"0123456789abcdef");
    assert_eq!(header_text(&headers, header::CONTENT_TYPE), "image/jpeg");
}

#[tokio::test]
async fn media_range_and_etag() {
    let fixture = fixture();
    let uri = format!("/media/t512/{}.jpg", db::SAMPLE_ITEM_A);

    let (status, headers, body) = send(
        &fixture.public,
        Method::GET,
        &uri,
        &[("range", "bytes=4-7")],
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, b"4567");
    assert_eq!(header_text(&headers, header::CONTENT_RANGE), "bytes 4-7/16");
    assert_eq!(header_text(&headers, header::CONTENT_LENGTH), "4");

    let (status, headers, _) = send(&fixture.public, Method::GET, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let etag = header_text(&headers, header::ETAG);
    assert!(!etag.is_empty());

    let (status, headers, body) = send(
        &fixture.public,
        Method::GET,
        &uri,
        &[("if-none-match", &etag)],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());
    assert_eq!(header_text(&headers, header::ETAG), etag);

    let path = fixture
        ._dir
        .path()
        .join(format!("media/t512/{}.jpg", db::SAMPLE_ITEM_A));
    std::fs::write(&path, b"0123456789abcdef-replaced").expect("replace media");
    let (status, headers, _) = send(&fixture.public, Method::GET, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(
        header_text(&headers, header::ETAG),
        etag,
        "etag must change when file contents change without updated_at"
    );
}

#[tokio::test]
async fn media_missing_invalid_and_unsatisfiable() {
    let fixture = fixture();

    let uri = format!("/media/t512/{}.jpg", db::SAMPLE_ITEM_B);
    let (status, _) = get_json(&fixture.public, &uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let uri = format!("/media/bogus/{}.jpg", db::SAMPLE_ITEM_A);
    let (status, _) = get_json(&fixture.public, &uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = get_json(&fixture.public, "/media/t512/not-hex.jpg").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let uri = format!("/media/t512/{}.png", db::SAMPLE_ITEM_A);
    let (status, _) = get_json(&fixture.public, &uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = get_json(&fixture.public, "/media/t512/..%5C..%5Ckanalib.db").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let uri = format!("/media/t512/{}.jpg", db::SAMPLE_ITEM_A);
    let (status, headers, _) = send(
        &fixture.public,
        Method::GET,
        &uri,
        &[("range", "bytes=999-")],
    )
    .await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(header_text(&headers, header::CONTENT_RANGE), "bytes */16");
}

#[tokio::test]
async fn public_hides_private_and_write_routes() {
    let fixture = fixture();

    let (status, body) = get_json(&fixture.public, "/internal/ingest/batch").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");

    let (status, _, _) = send(&fixture.public, Method::POST, "/api/desktop/push", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let uri = format!("/api/items/{}/original", db::SAMPLE_ITEM_A);
    let (status, _) = get_json(&fixture.public, &uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn private_listener_minimal() {
    let fixture = fixture();

    let (status, body) = get_json(&fixture.private, "/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);

    let (status, _) = get_json(&fixture.private, "/api/index").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/desktop/push")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"items":[]}"#))
        .expect("request");
    let (status, _, _) = execute(&fixture.private, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _, _) = send(
        &fixture.private,
        Method::POST,
        "/internal/ingest/batch",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn spa_fallback_serves_index_but_api_stays_json_404() {
    let fixture = web_fixture();

    let (status, _, body) = send(&fixture.public, Method::GET, "/", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains("kanalib-spa"));

    let (status, _, body) = send(&fixture.public, Method::GET, "/tags/1girl", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains("kanalib-spa"));

    let (status, body) = get_json(&fixture.public, "/api/does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn item_neighbors_follow_sort_and_filter() {
    let fixture = fixture();
    let uri = format!("/api/items/{}/neighbors", db::SAMPLE_ITEM_A);
    let (status, body) = get_json(&fixture.public, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["prev"]["id"], db::SAMPLE_ITEM_B);
    assert!(body.get("next").is_none());
    assert_eq!(body["index"], 1);
    assert_eq!(body["total"], 2);

    let uri = format!("{uri}?sort=name&order=asc");
    let (status, body) = get_json(&fixture.public, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("prev").is_none());
    assert_eq!(body["next"]["id"], db::SAMPLE_ITEM_B);
    assert_eq!(body["index"], 0);
    assert_eq!(body["total"], 2);

    let uri = format!("/api/items/{}/neighbors?tag=nope", db::SAMPLE_ITEM_A);
    let (status, body) = get_json(&fixture.public, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("prev").is_none());
    assert!(body.get("next").is_none());
    assert_eq!(body["total"], 0);

    let (status, _) = get_json(&fixture.public, "/api/items/not-an-id/neighbors").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn w2_pagination_is_stable_and_complete() {
    let fixture = extra_fixture();

    let mut uri = "/api/items?limit=7&sort=star&order=desc".to_string();
    let mut seen = HashSet::new();
    let mut pages = 0;
    let mut previous_star: Option<u64> = None;
    loop {
        let (status, body) = get_json(&fixture.public, &uri).await;
        assert_eq!(status, StatusCode::OK);
        let items = body["items"].as_array().expect("items");
        assert!(items.len() <= 7);
        for item in items {
            let id = item["id"].as_str().expect("id");
            assert!(seen.insert(id.to_string()), "duplicate id {id}");
            let star = item["st"].as_u64().expect("star");
            if let Some(previous) = previous_star {
                assert!(star <= previous, "star order went backwards");
            }
            previous_star = Some(star);
        }
        pages += 1;
        let Some(next) = body["next"].as_str() else {
            break;
        };
        uri = format!("/api/items?limit=7&sort=star&order=desc&cursor={next}");
        assert!(pages < 20, "pagination did not terminate");
    }
    assert_eq!(pages, 5);
    assert_eq!(seen.len(), 32);

    let mut uri = "/api/items?limit=9&sort=name&order=asc".to_string();
    let mut names = Vec::new();
    loop {
        let (status, body) = get_json(&fixture.public, &uri).await;
        assert_eq!(status, StatusCode::OK);
        for item in body["items"].as_array().expect("items") {
            names.push(item["n"].as_str().expect("name").to_string());
        }
        let Some(next) = body["next"].as_str() else {
            break;
        };
        uri = format!("/api/items?limit=9&sort=name&order=asc&cursor={next}");
        assert!(names.len() <= 32);
    }
    assert_eq!(names.len(), 32);
    assert!(
        names.windows(2).all(|pair| pair[0] <= pair[1]),
        "name order is not ascending"
    );

    let (_, first) = get_json(&fixture.public, "/api/items?limit=5&sort=random&seed=42").await;
    let (_, second) = get_json(&fixture.public, "/api/items?limit=5&sort=random&seed=42").await;
    assert_eq!(first["items"], second["items"]);
}

#[tokio::test]
async fn w2_search_filters_and_facets_stay_consistent() {
    let fixture = extra_fixture();

    let cases = [
        ("/api/items?q=name%3Aalpha", 10u64),
        ("/api/items?q=tag%3Atag-even", 15),
        ("/api/items?q=-alpha", 22),
        ("/api/items?q=rating%3A%E6%95%8F%E6%84%9F", 9),
        ("/api/items?tag=tag-even", 15),
        ("/api/items?group=beta", 10),
        ("/api/items?rating=E", 7),
        ("/api/items?fav=1", 17),
        ("/api/items?q=alpha&rating=G", 3),
    ];
    for (uri, expected) in cases {
        let (status, body) = get_json(&fixture.public, uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(
            body["items"].as_array().expect("items").len() as u64,
            expected.min(60),
            "{uri}"
        );
        let index_uri = uri.replacen("/api/items?", "/api/index?", 1);
        let (status, body) = get_json(&fixture.public, &index_uri).await;
        assert_eq!(status, StatusCode::OK, "{index_uri}");
        assert_eq!(body["count"], expected, "{index_uri}");
    }

    let (status, body) = get_json(&fixture.public, "/api/index?tag=tag-even").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["count"], 15);
    assert_eq!(body["counts"]["items"], 15);
    let facet = body["facets"]["tags"]
        .as_array()
        .expect("facet tags")
        .iter()
        .find(|entry| entry["name"] == "tag-even")
        .expect("tag-even facet");
    assert_eq!(facet["count"], 15);
    assert!(facet["cover"].is_string(), "tag facet needs a cover id");

    let (status, body) = get_json(&fixture.public, "/api/index?fav=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["counts"]["items"], 17);
    assert_eq!(body["counts"]["favorite"], 17);

    let (status, body) = get_json(&fixture.public, "/api/index?rating=E").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["counts"]["items"], 7);
    assert_eq!(body["counts"]["ratings"]["e"], 7);
    assert_eq!(body["counts"]["ratings"]["g"], 0);

    let (status, body) = get_json(&fixture.public, "/api/tags?tag=tag-even&limit=10").await;
    assert_eq!(status, StatusCode::OK);
    let facet = body["items"]
        .as_array()
        .expect("tags")
        .iter()
        .find(|entry| entry["name"] == "tag-even")
        .expect("tag-even tag facet");
    assert_eq!(facet["count"], 15);
}

#[tokio::test]
async fn w2_rejects_bad_cursor_limit_and_sort_switch() {
    let fixture = extra_fixture();

    let (status, body) = get_json(&fixture.public, "/api/items?cursor=garbage").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_cursor");

    let (status, body) = get_json(&fixture.public, "/api/items?limit=abc").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_limit");

    let (status, body) = get_json(&fixture.public, "/api/items?limit=1&sort=star").await;
    assert_eq!(status, StatusCode::OK);
    let cursor = body["next"].as_str().expect("next cursor");
    let uri = format!("/api/items?limit=1&sort=name&cursor={cursor}");
    let (status, body) = get_json(&fixture.public, &uri).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_cursor");
}

#[tokio::test]
async fn w2_random_seed_splits_order_and_cursor_is_pinned_to_filter() {
    let fixture = extra_fixture();
    let ids = |body: &Value| -> Vec<String> {
        body["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|item| item["id"].as_str().expect("id").to_string())
            .collect()
    };

    let (status, seed_one) =
        get_json(&fixture.public, "/api/items?limit=32&sort=random&seed=1").await;
    assert_eq!(status, StatusCode::OK);
    let (status, seed_one_again) =
        get_json(&fixture.public, "/api/items?limit=32&sort=random&seed=1").await;
    assert_eq!(status, StatusCode::OK);
    let (status, seed_two) =
        get_json(&fixture.public, "/api/items?limit=32&sort=random&seed=2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ids(&seed_one),
        ids(&seed_one_again),
        "same seed must be stable"
    );
    assert_ne!(
        ids(&seed_one),
        ids(&seed_two),
        "different seed must reorder"
    );

    let (status, body) = get_json(&fixture.public, "/api/items?limit=7&sort=random&seed=1").await;
    assert_eq!(status, StatusCode::OK);
    let cursor = body["next"].as_str().expect("next cursor");

    let uri = format!("/api/items?limit=7&sort=random&seed=2&cursor={cursor}");
    let (status, body) = get_json(&fixture.public, &uri).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_cursor");

    let uri = format!("/api/items?limit=7&sort=random&seed=1&rating=G&cursor={cursor}");
    let (status, body) = get_json(&fixture.public, &uri).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_cursor");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual 10k / 32-concurrency p95 check"]
async fn w2_10k_32_concurrent_p95_under_120ms() {
    let fixture = fixture();
    seed_extra_items(&fixture._dir.path().join("kanalib.db"), 10_000);
    let app = fixture.public.clone();

    let (status, body) = get_json(&app, "/api/items?limit=60").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().expect("items").len(), 60);
    let (status, body) = get_json(&app, "/api/index").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["count"], 10_002);

    for (uri, budget) in [
        ("/api/items?limit=60", 120u64),
        ("/api/items?limit=60&sort=star&order=desc", 120),
        ("/api/items?limit=60&tag=tag-even&sort=name&order=asc", 120),
        ("/api/items?limit=60&sort=random&seed=7", 120),
        ("/api/items?limit=60&q=alpha", 200),
    ] {
        let started = Instant::now();
        let (status, _) = get_json(&app, uri).await;
        assert_eq!(status, StatusCode::OK);
        eprintln!("single {uri}: {:?} (budget {budget}ms)", started.elapsed());

        let mut handles = Vec::new();
        for _ in 0..32 {
            let app = app.clone();
            let uri = uri.to_string();
            handles.push(tokio::spawn(async move {
                let mut samples = Vec::new();
                for _ in 0..5 {
                    let started = Instant::now();
                    let (status, _) = get_json(&app, &uri).await;
                    assert_eq!(status, StatusCode::OK);
                    samples.push(started.elapsed());
                }
                samples
            }));
        }

        let mut samples = Vec::new();
        for handle in handles {
            samples.extend(handle.await.expect("worker"));
        }
        samples.sort();
        let index = ((samples.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
        let p95 = samples[index];
        assert!(
            p95 < Duration::from_millis(budget),
            "{uri} p95 = {p95:?} over {} samples (budget {budget}ms)",
            samples.len()
        );
        eprintln!("32x {uri}: p95={p95:?} over {} samples", samples.len());
    }

    let started = Instant::now();
    let (status, body) = get_json(&app, "/api/index?tag=tag-even").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["count"], 5_000);
    eprintln!("single /api/index?tag=tag-even: {:?}", started.elapsed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual DB concurrency profile"]
async fn w2_10k_db_only_concurrency_profile() {
    use kanalib_web::query::{ItemFilter, ItemSort, SortKind};

    let fixture = fixture();
    seed_extra_items(&fixture._dir.path().join("kanalib.db"), 10_000);
    let db_path = fixture._dir.path().join("kanalib.db");

    for workers in [1usize, 2, 4, 8, 16, 32] {
        let started = Instant::now();
        let mut handles = Vec::new();
        for worker in 0..workers {
            let path = db_path.clone();
            handles.push(tokio::task::spawn_blocking(move || {
                let conn = db::open_read(&path).expect("open");
                let mut samples = Vec::new();
                for round in 0..5 {
                    let filter = match (worker + round) % 4 {
                        1 => ItemFilter {
                            q: Some("alpha".to_string()),
                            ..Default::default()
                        },
                        2 => ItemFilter {
                            tag: Some("tag-even".to_string()),
                            ..Default::default()
                        },
                        _ => ItemFilter::default(),
                    };
                    let sort = match (worker + round) % 4 {
                        2 => ItemSort {
                            kind: SortKind::Name,
                            ascending: true,
                            seed: 0,
                        },
                        3 => ItemSort {
                            kind: SortKind::Random,
                            ascending: false,
                            seed: 7,
                        },
                        _ => ItemSort {
                            kind: SortKind::Star,
                            ascending: false,
                            seed: 0,
                        },
                    };
                    let sample_started = Instant::now();
                    db::search_items(&conn, &filter, &sort, None, 60, 0).expect("query");
                    samples.push(sample_started.elapsed());
                }
                samples
            }));
        }
        let mut samples = Vec::new();
        for handle in handles {
            samples.extend(handle.await.expect("worker"));
        }
        samples.sort();
        let index = ((samples.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
        eprintln!(
            "workers={workers:02} p95={:?} wall={:?}",
            samples[index],
            started.elapsed()
        );
    }

    for workers in [1usize, 8, 32] {
        let started = Instant::now();
        let mut handles = Vec::new();
        for _ in 0..workers {
            let path = db_path.clone();
            handles.push(tokio::task::spawn_blocking(move || {
                let conn = db::open_read(&path).expect("open");
                let mut samples = Vec::new();
                for _ in 0..5 {
                    let sample_started = Instant::now();
                    let count: i64 = conn
                        .query_row("SELECT COUNT(*) FROM items WHERE deleted = 0", [], |row| {
                            row.get(0)
                        })
                        .expect("count");
                    std::hint::black_box(count);
                    samples.push(sample_started.elapsed());
                }
                samples
            }));
        }
        let mut samples = Vec::new();
        for handle in handles {
            samples.extend(handle.await.expect("worker"));
        }
        samples.sort();
        let index = ((samples.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
        eprintln!(
            "count-only workers={workers:02} p95={:?} wall={:?}",
            samples[index],
            started.elapsed()
        );
    }

    for (label, filter, sort) in [
        (
            "star",
            ItemFilter::default(),
            ItemSort {
                kind: SortKind::Star,
                ascending: false,
                seed: 0,
            },
        ),
        (
            "q-alpha",
            ItemFilter {
                q: Some("alpha".to_string()),
                ..Default::default()
            },
            ItemSort {
                kind: SortKind::Updated,
                ascending: false,
                seed: 0,
            },
        ),
        (
            "tag-name",
            ItemFilter {
                tag: Some("tag-even".to_string()),
                ..Default::default()
            },
            ItemSort {
                kind: SortKind::Name,
                ascending: true,
                seed: 0,
            },
        ),
        (
            "random",
            ItemFilter::default(),
            ItemSort {
                kind: SortKind::Random,
                ascending: false,
                seed: 7,
            },
        ),
    ] {
        let started = Instant::now();
        let mut handles = Vec::new();
        for _ in 0..32 {
            let path = db_path.clone();
            let filter = filter.clone();
            handles.push(tokio::task::spawn_blocking(move || {
                let conn = db::open_read(&path).expect("open");
                let mut samples = Vec::new();
                for _ in 0..5 {
                    let sample_started = Instant::now();
                    db::search_items(&conn, &filter, &sort, None, 60, 0).expect("query");
                    samples.push(sample_started.elapsed());
                }
                samples
            }));
        }
        let mut samples = Vec::new();
        for handle in handles {
            samples.extend(handle.await.expect("worker"));
        }
        samples.sort();
        let index = ((samples.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
        eprintln!(
            "{label} workers=32 p95={:?} wall={:?}",
            samples[index],
            started.elapsed()
        );
    }
}

#[test]
#[ignore = "manual query plan profile"]
fn w2_query_plan_profile() {
    let fixture = fixture();
    seed_extra_items(&fixture._dir.path().join("kanalib.db"), 10_000);
    let conn = db::open_read(&fixture._dir.path().join("kanalib.db")).expect("open");

    let queries = [
        (
            "q-alpha-count",
            "SELECT COUNT(*) FROM items i WHERE i.deleted = 0 AND (
                lower(replace(i.name, '_', ' ')) LIKE '%alpha%' ESCAPE '\\'
                OR lower(replace(i.group_name, '_', ' ')) LIKE '%alpha%' ESCAPE '\\'
                OR lower(i.rating) LIKE '%alpha%' ESCAPE '\\'
                OR i.id IN (SELECT its.item_id FROM item_tags its
                              JOIN tags ts ON ts.id = its.tag_id
                             WHERE lower(replace(ts.name, '_', ' ')) LIKE '%alpha%' ESCAPE '\\'))",
        ),
        (
            "tag-count",
            "SELECT COUNT(*) FROM items i WHERE i.deleted = 0 AND
                i.id IN (SELECT itf.item_id FROM item_tags itf
                           JOIN tags tf ON tf.id = itf.tag_id
                          WHERE tf.name = 'tag-even')",
        ),
        (
            "tag-join-count",
            "SELECT COUNT(*) FROM items i
               JOIN item_tags itf ON itf.item_id = i.id
               JOIN tags tf ON tf.id = itf.tag_id
              WHERE i.deleted = 0 AND tf.name = 'tag-even'",
        ),
        (
            "star-page",
            "SELECT i.id, i.star FROM items i WHERE i.deleted = 0
              ORDER BY i.star DESC, i.id DESC LIMIT 61",
        ),
        (
            "random-page",
            "SELECT i.id, i.id AS sort_key FROM items i WHERE i.deleted = 0
              ORDER BY i.id DESC, i.id DESC LIMIT 61",
        ),
        (
            "tag-candidate-page",
            "SELECT i.id, i.name FROM (
                SELECT DISTINCT itf.item_id FROM item_tags itf
                  JOIN tags tf ON tf.id = itf.tag_id
                 WHERE tf.name = 'tag-even') cand
              CROSS JOIN items i
              WHERE cand.item_id = i.id AND i.deleted = 0
              ORDER BY i.name COLLATE NOCASE ASC, i.id ASC LIMIT 61",
        ),
        (
            "q-candidate-page",
            r#"SELECT i.id, i.updated_at FROM (
                SELECT item_id FROM item_search
                 WHERE item_search MATCH 'name : "alpha"') cand
              CROSS JOIN items i
              WHERE cand.item_id = i.id AND i.deleted = 0
              ORDER BY i.updated_at DESC, i.id DESC LIMIT 61"#,
        ),
    ];

    for (label, sql) in queries {
        eprintln!("--- {label}");
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .expect("prepare");
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(3))
            .expect("plan");
        for row in rows {
            eprintln!("  {}", row.expect("row"));
        }
    }
}

#[test]
#[ignore = "manual component profile"]
fn w2_component_profile() {
    let fixture = fixture();
    seed_extra_items(&fixture._dir.path().join("kanalib.db"), 10_000);
    let conn = db::open_read(&fixture._dir.path().join("kanalib.db")).expect("open");

    let cases = [
        (
            "fts-match",
            r#"SELECT item_id FROM item_search WHERE item_search MATCH 'name : "alpha"'"#,
        ),
        (
            "q-count-in",
            r#"SELECT COUNT(*) FROM items i WHERE i.deleted = 0 AND i.id IN (
                SELECT item_id FROM item_search WHERE item_search MATCH 'name : "alpha"')"#,
        ),
        (
            "q-page-in",
            r#"SELECT i.id, i.star FROM items i WHERE i.deleted = 0 AND i.id IN (
                SELECT item_id FROM item_search WHERE item_search MATCH 'name : "alpha"')
              ORDER BY i.star DESC, i.id DESC LIMIT 61"#,
        ),
        (
            "tag-count-exists",
            "SELECT COUNT(*) FROM items i WHERE i.deleted = 0 AND EXISTS (
                SELECT 1 FROM item_tags itf JOIN tags tf ON tf.id = itf.tag_id
                 WHERE tf.name = 'tag-even' AND itf.item_id = i.id)",
        ),
        (
            "tag-page-exists",
            "SELECT i.id, i.name FROM items i WHERE i.deleted = 0 AND EXISTS (
                SELECT 1 FROM item_tags itf JOIN tags tf ON tf.id = itf.tag_id
                 WHERE tf.name = 'tag-even' AND itf.item_id = i.id)
              ORDER BY i.name COLLATE NOCASE ASC, i.id ASC LIMIT 61",
        ),
        (
            "tag-facets-exists",
            "SELECT t.name, COUNT(*) FROM item_tags it
               JOIN tags t ON t.id = it.tag_id
               JOIN items i ON i.id = it.item_id
              WHERE i.deleted = 0 AND EXISTS (
                SELECT 1 FROM item_tags itf JOIN tags tf ON tf.id = itf.tag_id
                 WHERE tf.name = 'tag-even' AND itf.item_id = i.id)
              GROUP BY t.id ORDER BY COUNT(*) DESC, t.name ASC LIMIT 2000",
        ),
    ];

    for (label, sql) in cases {
        let mut stmt = conn.prepare(sql).expect("prepare");
        let run = |stmt: &mut rusqlite::Statement<'_>| {
            let rows = stmt
                .query_map([], |row| row.get::<_, rusqlite::types::Value>(0))
                .expect("query");
            rows.count()
        };
        for _ in 0..3 {
            std::hint::black_box(run(&mut stmt));
        }
        let started = Instant::now();
        for _ in 0..20 {
            std::hint::black_box(run(&mut stmt));
        }
        eprintln!("{label}: {:?}/iter", started.elapsed() / 20);
    }

    for (label, sql) in [
        ("tag-page-exists", cases[4].1),
        ("tag-count-exists", cases[3].1),
        ("q-page-in", cases[2].1),
    ] {
        for workers in [1usize, 8, 32] {
            let path = fixture._dir.path().join("kanalib.db");
            let mut handles = Vec::new();
            let started = Instant::now();
            for _ in 0..workers {
                let path = path.clone();
                handles.push(std::thread::spawn(move || {
                    let conn = db::open_read(&path).expect("open");
                    let mut stmt = conn.prepare(sql).expect("prepare");
                    let mut samples = Vec::new();
                    for _ in 0..5 {
                        let sample = Instant::now();
                        let rows = stmt
                            .query_map([], |row| row.get::<_, rusqlite::types::Value>(0))
                            .expect("query");
                        std::hint::black_box(rows.count());
                        samples.push(sample.elapsed());
                    }
                    samples
                }));
            }
            let mut samples = Vec::new();
            for handle in handles {
                samples.extend(handle.join().expect("thread"));
            }
            samples.sort();
            let index = ((samples.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
            eprintln!(
                "{label} workers={workers:02} p95={:?} wall={:?}",
                samples[index],
                started.elapsed()
            );
        }
    }
}

const INGEST_SECRET: &str = "w4-ingest-secret";
const DEVICE_TOKEN: &str = "w4-device-token";
const BOX_ORIGIN: &str = "http://box.test:8789";
const NEW_ITEM: &str = "cccccccccccccccccccccccccccccccccccccccc";
const NEW_ITEM_2: &str = "dddddddddddddddddddddddddddddddddddddddd";
const MISSING_ITEM: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

fn write_config(upload_limit: u64) -> WriteConfig {
    WriteConfig {
        ingest_secret: Some(Arc::from(INGEST_SECRET)),
        box_origin: Some(Arc::from(BOX_ORIGIN)),
        media_signing_key: Some(Arc::from("w4-media-key")),
        upload_limit_bytes: upload_limit,
        signed_url_ttl: Duration::from_secs(300),
    }
}

fn write_fixture() -> Fixture {
    write_fixture_with_limit(32 * 1024 * 1024)
}

fn write_fixture_with_limit(upload_limit: u64) -> Fixture {
    let fixture = build_fixture_with(false, Some(write_config(upload_limit)));
    let conn = db::open(&fixture._dir.path().join("kanalib.db")).expect("open write db");
    db::insert_device(
        &conn,
        "dev_test",
        "test device",
        &crypto::sha256_hex(DEVICE_TOKEN.as_bytes()),
    )
    .expect("insert device");
    fixture
}

async fn execute(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, headers, body)
}

fn signed_headers_with_path(
    path: &str,
    body: &[u8],
    batch_id: &str,
    timestamp: i64,
    secret: &str,
) -> Vec<(String, String)> {
    let payload = format!(
        "POST\n{path}\n{batch_id}\n{timestamp}\n{}",
        crypto::sha256_hex(body)
    );
    vec![
        ("x-kanalib-batch-id".to_string(), batch_id.to_string()),
        ("x-kanalib-timestamp".to_string(), timestamp.to_string()),
        (
            "x-kanalib-signature".to_string(),
            crypto::hmac_sha256_hex(secret.as_bytes(), payload.as_bytes()).expect("sign"),
        ),
        ("content-type".to_string(), "application/json".to_string()),
    ]
}

fn signed_headers(body: &[u8], batch_id: &str, timestamp: i64) -> Vec<(String, String)> {
    signed_headers_with_path(
        "/internal/ingest/batch",
        body,
        batch_id,
        timestamp,
        INGEST_SECRET,
    )
}

fn signed_delete_headers(body: &[u8], batch_id: &str, timestamp: i64) -> Vec<(String, String)> {
    signed_headers_with_path(
        "/internal/ingest/delete",
        body,
        batch_id,
        timestamp,
        INGEST_SECRET,
    )
}

fn signed_thumb_headers(
    path: &str,
    body: &[u8],
    batch_id: &str,
    timestamp: i64,
    content_type: &str,
) -> Vec<(String, String)> {
    let payload = format!(
        "POST\n{path}\n{batch_id}\n{timestamp}\n{}",
        crypto::sha256_hex(body)
    );
    vec![
        ("x-kanalib-batch-id".to_string(), batch_id.to_string()),
        ("x-kanalib-timestamp".to_string(), timestamp.to_string()),
        (
            "x-kanalib-signature".to_string(),
            crypto::hmac_sha256_hex(INGEST_SECRET.as_bytes(), payload.as_bytes()).expect("sign"),
        ),
        ("content-type".to_string(), content_type.to_string()),
    ]
}

async fn post_signed(
    app: &Router,
    uri: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(Method::POST).uri(uri);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let request = builder.body(Body::from(body.to_vec())).expect("request");
    let (status, _, body) = execute(app, request).await;
    let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, value)
}

fn batch_body(items: Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "source": "pixiv",
        "rules_version": "rules-w4",
        "items": items,
    }))
    .expect("batch json")
}

fn allow_item(id: &str) -> Value {
    serde_json::json!({
        "revision": 1,
        "decision": "allow",
        "source_id": "300",
        "id": id,
        "width": 100,
        "height": 200,
        "name": "w4.jpg",
        "rating": "s",
        "star": 4,
        "favorite": true,
        "tags": [{"name": "w4-tag", "category": "general", "score": 0.9}],
        "mtime": 1_700_000_000u64,
        "group": "w4-group",
        "orig": ".JPG",
        "sharp": 512,
        "work_id": "w4-work",
        "page_count": 2,
        "page": 0,
        "author": "W4 Author",
        "author_id": "w4-author",
        "title": "W4 Title"
    })
}

#[tokio::test]
async fn ingest_requires_fresh_signature() {
    let fixture = write_fixture();
    let body = batch_body(serde_json::json!([allow_item(NEW_ITEM)]));
    let now = crypto::unix_now();

    let (status, _) = post_signed(
        &fixture.private,
        "/internal/ingest/batch",
        &[("x-kanalib-batch-id".to_string(), "b-unsigned".to_string())],
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let mut headers = signed_headers(&body, "b-bad", now);
    headers[2].1 = "deadbeef".to_string();
    let (status, _) =
        post_signed(&fixture.private, "/internal/ingest/batch", &headers, &body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let mut headers = signed_headers(&body, "b-stale", now - 3600);
    headers[1].1 = (now - 3600).to_string();
    let (status, _) =
        post_signed(&fixture.private, "/internal/ingest/batch", &headers, &body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let mut headers = signed_headers(&body, "b-missing-id", now);
    headers.remove(0);
    let (status, _) =
        post_signed(&fixture.private, "/internal/ingest/batch", &headers, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // batch id 在签名串内：换 id 不能复用旧签名绕过幂等键。
    let mut headers = signed_headers(&body, "b-signed-id", now);
    headers[0].1 = "b-other-id".to_string();
    let (status, _) =
        post_signed(&fixture.private, "/internal/ingest/batch", &headers, &body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 时间戳同样在签名串内：窗口内改时间戳也不能重放。
    let mut headers = signed_headers(&body, "b-signed-ts", now);
    headers[1].1 = (now + 10).to_string();
    let (status, _) =
        post_signed(&fixture.private, "/internal/ingest/batch", &headers, &body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ingest_allow_batch_is_idempotent() {
    let fixture = write_fixture();
    let body = batch_body(serde_json::json!([allow_item(NEW_ITEM)]));
    let headers = signed_headers(&body, "b-allow", crypto::unix_now());

    let (status, value) =
        post_signed(&fixture.private, "/internal/ingest/batch", &headers, &body).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["inserted"], 1);
    assert_eq!(value["accepted"], 1);

    let (status, value) =
        post_signed(&fixture.private, "/internal/ingest/batch", &headers, &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["duplicate"], true);
    assert_eq!(value["accepted"], 0);

    let conn = db::open(&fixture._dir.path().join("kanalib.db")).expect("db");
    let items: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM items WHERE id = ?1",
            [NEW_ITEM],
            |row| row.get(0),
        )
        .expect("count item");
    assert_eq!(items, 1);
    let batches: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ingest_batches WHERE id = 'b-allow'",
            [],
            |row| row.get(0),
        )
        .expect("count batch");
    assert_eq!(batches, 1);
    let tags: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM item_tags it JOIN tags t ON t.id = it.tag_id
              WHERE it.item_id = ?1 AND t.name = 'w4-tag'",
            [NEW_ITEM],
            |row| row.get(0),
        )
        .expect("count tags");
    assert_eq!(tags, 1);

    let (status, item) = get_json(&fixture.public, &format!("/api/items/{NEW_ITEM}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item["id"], NEW_ITEM);
    assert_eq!(item["rt"], "S");
    assert_eq!(item["st"], 4);
}

#[test]
fn ingest_refreshes_page_count_and_cover_without_stale_pointers() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("kanalib.db");
    db::seed_sample(&path).expect("seed");
    let mut conn = db::open(&path).expect("db");

    let mut item = allow_item(NEW_ITEM);
    item["work_id"] = serde_json::json!(db::SAMPLE_WORK);
    item["page"] = serde_json::json!(2);
    item["page_count"] = serde_json::json!(5);
    let batch: IngestBatch =
        serde_json::from_value(serde_json::json!({"source": "pixiv", "items": [item]}))
            .expect("batch");
    db::apply_ingest_batch(&mut conn, "b-work-1", &batch, 10).expect("apply");
    let pages: i64 = conn
        .query_row(
            "SELECT page_count FROM works WHERE id = ?1",
            [db::SAMPLE_WORK],
            |row| row.get(0),
        )
        .expect("pages");
    assert_eq!(pages, 5);

    // 来源修正页数时应能下调，而不是只增不减。
    let mut lower = allow_item(NEW_ITEM);
    lower["revision"] = serde_json::json!(2);
    lower["work_id"] = serde_json::json!(db::SAMPLE_WORK);
    lower["page"] = serde_json::json!(2);
    lower["page_count"] = serde_json::json!(3);
    let batch: IngestBatch =
        serde_json::from_value(serde_json::json!({"source": "pixiv", "items": [lower]}))
            .expect("batch");
    db::apply_ingest_batch(&mut conn, "b-work-2", &batch, 11).expect("apply");
    let pages: i64 = conn
        .query_row(
            "SELECT page_count FROM works WHERE id = ?1",
            [db::SAMPLE_WORK],
            |row| row.get(0),
        )
        .expect("pages");
    assert_eq!(pages, 3);

    let cover = |conn: &rusqlite::Connection| -> Option<String> {
        conn.query_row(
            "SELECT cover_item_id FROM works WHERE id = ?1",
            [db::SAMPLE_WORK],
            |row| row.get(0),
        )
        .expect("cover")
    };
    assert_eq!(cover(&conn).as_deref(), Some(db::SAMPLE_ITEM_A));
    for (batch_id, item_id) in [
        ("b-del-a", db::SAMPLE_ITEM_A),
        ("b-del-b", db::SAMPLE_ITEM_B),
    ] {
        let delete = IngestDelete {
            source: "pixiv".to_string(),
            rules_version: None,
            items: vec![IngestDeleteItem {
                id: item_id.to_string(),
                revision: 99,
            }],
        };
        db::apply_ingest_delete(&mut conn, batch_id, &delete, 12).expect("delete");
    }
    assert_eq!(cover(&conn).as_deref(), Some(NEW_ITEM));
}

#[tokio::test]
async fn ingest_thumb_upload_writes_media_and_rejects_bad_signature() {
    let fixture = write_fixture();
    let path = format!("/internal/ingest/thumb/t256/{NEW_ITEM}.jpg");
    let body = b"\xff\xd8\xff\xe0fake-jpeg-payload".to_vec();
    let headers = signed_thumb_headers(
        &path,
        &body,
        "thumb-batch-1",
        crypto::unix_now(),
        "image/jpeg",
    );

    let (status, value) = post_signed(&fixture.private, &path, &headers, &body).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["ok"], true);
    let stored = fixture
        ._dir
        .path()
        .join("media/t256")
        .join(format!("{NEW_ITEM}.jpg"));
    assert_eq!(std::fs::read(&stored).expect("thumb file"), body);

    let mut bad = headers.clone();
    bad[1].1 = (crypto::unix_now() + 1).to_string();
    let (status, _) = post_signed(&fixture.private, &path, &bad, &body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let empty_batch: Vec<(String, String)> = headers
        .iter()
        .filter(|(name, _)| name != "x-kanalib-batch-id")
        .cloned()
        .collect();
    let (status, value) = post_signed(&fixture.private, &path, &empty_batch, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "missing_batch_id");

    let (status, _) = post_signed(
        &fixture.private,
        &path,
        &headers,
        b"\xff\xd8\xfftampered".as_slice(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ingest_thumb_upload_accepts_avif_and_rejects_format_mismatch() {
    let fixture = write_fixture();
    let path = format!("/internal/ingest/thumb/t512/{NEW_ITEM}.avif");
    let body = b"\x00\x00\x00\x18ftypavif\x00\x00\x00\x00avifmif1".to_vec();
    let headers = signed_thumb_headers(
        &path,
        &body,
        "thumb-batch-2",
        crypto::unix_now(),
        "image/avif",
    );

    let (status, value) = post_signed(&fixture.private, &path, &headers, &body).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let stored = fixture
        ._dir
        .path()
        .join(format!("media/t512/{NEW_ITEM}.avif"));
    assert_eq!(std::fs::read(&stored).expect("avif thumb"), body);

    let jpeg_path = format!("/internal/ingest/thumb/t512/{NEW_ITEM}.jpg");
    let mismatched = signed_thumb_headers(
        &jpeg_path,
        &body,
        "thumb-batch-2",
        crypto::unix_now(),
        "image/avif",
    );
    let (status, _) = post_signed(&fixture.private, &jpeg_path, &mismatched, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let invalid = b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00heicmif1".to_vec();
    let invalid_headers = signed_thumb_headers(
        &path,
        &invalid,
        "thumb-batch-2",
        crypto::unix_now(),
        "image/avif",
    );
    let (status, _) = post_signed(&fixture.private, &path, &invalid_headers, &invalid).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn ingest_update_keeps_user_star_and_favorite() {
    let fixture = write_fixture();
    let first = batch_body(serde_json::json!([allow_item(NEW_ITEM)]));
    let headers = signed_headers(&first, "b-user-state-1", crypto::unix_now());
    let (status, _) =
        post_signed(&fixture.private, "/internal/ingest/batch", &headers, &first).await;
    assert_eq!(status, StatusCode::OK);

    let mut second = allow_item(NEW_ITEM);
    second["star"] = serde_json::json!(0);
    second["favorite"] = serde_json::json!(false);
    second["rating"] = serde_json::json!("G");
    let second = batch_body(serde_json::json!([second]));
    let headers = signed_headers(&second, "b-user-state-2", crypto::unix_now());
    let (status, _) = post_signed(
        &fixture.private,
        "/internal/ingest/batch",
        &headers,
        &second,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, item) = get_json(&fixture.public, &format!("/api/items/{NEW_ITEM}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item["st"], 4);
    assert_eq!(item["fav"], true);
    assert_eq!(item["rt"], "G");
}

#[tokio::test]
async fn ingest_rejects_unknown_rating() {
    let fixture = write_fixture();
    let mut item = allow_item(NEW_ITEM);
    item["rating"] = serde_json::json!("unknown");
    let body = batch_body(serde_json::json!([item]));
    let headers = signed_headers(&body, "b-unknown-rating", crypto::unix_now());
    let (status, value) =
        post_signed(&fixture.private, "/internal/ingest/batch", &headers, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "invalid_rating");
}

#[tokio::test]
async fn ingest_accepts_rotated_secret_overlap() {
    let writes = WriteConfig {
        ingest_secret: Some(Arc::from("old-secret,new-secret")),
        ..write_config(32 * 1024 * 1024)
    };
    let fixture = build_fixture_with(false, Some(writes));
    let body = batch_body(serde_json::json!([allow_item(NEW_ITEM)]));
    let headers = signed_headers_with_path(
        "/internal/ingest/batch",
        &body,
        "b-rotate",
        crypto::unix_now(),
        "new-secret",
    );
    let (status, value) =
        post_signed(&fixture.private, "/internal/ingest/batch", &headers, &body).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["inserted"], 1);
}

#[tokio::test]
async fn ingest_non_allow_rejects_whole_batch() {
    let fixture = write_fixture();
    let mut blocked = allow_item(NEW_ITEM_2);
    blocked["decision"] = Value::String("block".to_string());
    let body = batch_body(serde_json::json!([allow_item(NEW_ITEM), blocked]));
    let headers = signed_headers(&body, "b-blocked", crypto::unix_now());

    let (status, value) =
        post_signed(&fixture.private, "/internal/ingest/batch", &headers, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "non_allow_batch");

    let conn = db::open(&fixture._dir.path().join("kanalib.db")).expect("db");
    let items: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM items WHERE id IN (?1, ?2)",
            params![NEW_ITEM, NEW_ITEM_2],
            |row| row.get(0),
        )
        .expect("count items");
    assert_eq!(items, 0);
    let batches: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ingest_batches WHERE id = 'b-blocked'",
            [],
            |row| row.get(0),
        )
        .expect("count batches");
    assert_eq!(batches, 0);
}

#[tokio::test]
async fn ingest_delete_only_tombstones() {
    let fixture = write_fixture();
    let body = batch_body(serde_json::json!([allow_item(NEW_ITEM)]));
    let headers = signed_headers(&body, "b-del-allow", crypto::unix_now());
    let (status, _) =
        post_signed(&fixture.private, "/internal/ingest/batch", &headers, &body).await;
    assert_eq!(status, StatusCode::OK);

    let delete = serde_json::to_vec(&serde_json::json!({
        "source": "pixiv",
        "items": [{"id": NEW_ITEM, "revision": 2}]
    }))
    .expect("delete json");
    let headers = signed_delete_headers(&delete, "b-del", crypto::unix_now());
    let (status, value) = post_signed(
        &fixture.private,
        "/internal/ingest/delete",
        &headers,
        &delete,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["deleted"], 1);

    let (status, _) = get_json(&fixture.public, &format!("/api/items/{NEW_ITEM}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let conn = db::open(&fixture._dir.path().join("kanalib.db")).expect("db");
    let deleted: i64 = conn
        .query_row(
            "SELECT deleted FROM items WHERE id = ?1",
            [NEW_ITEM],
            |row| row.get(0),
        )
        .expect("tombstone");
    assert_eq!(deleted, 1);

    let (status, value) = post_signed(
        &fixture.private,
        "/internal/ingest/delete",
        &headers,
        &delete,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["duplicate"], true);

    let stale = serde_json::to_vec(&serde_json::json!({
        "source": "pixiv",
        "items": [{"id": NEW_ITEM, "revision": 1}]
    }))
    .expect("stale json");
    let headers = signed_delete_headers(&stale, "b-del-stale", crypto::unix_now());
    let (status, value) = post_signed(
        &fixture.private,
        "/internal/ingest/delete",
        &headers,
        &stale,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["stale"], 1);
}

#[tokio::test]
async fn desktop_pull_requires_token_and_pages() {
    let fixture = write_fixture();
    let (status, _, _) = send(&fixture.private, Method::GET, "/api/desktop/pull", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let auth = format!("Bearer {DEVICE_TOKEN}");
    let (status, _, body) = send(
        &fixture.private,
        Method::GET,
        "/api/desktop/pull?limit=1",
        &[("authorization", auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let first: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(first["has_more"], true);
    let next = first["next"].as_str().expect("next cursor").to_string();
    assert_eq!(first["items"].as_array().expect("items").len(), 1);
    assert!(first["items"][0]["media"]["t512"].as_str().is_some());
    assert!(first["items"][0]["media"]["original"].as_str().is_some());

    let uri = format!("/api/desktop/pull?limit=1&since={next}");
    let (status, _, body) = send(
        &fixture.private,
        Method::GET,
        &uri,
        &[("authorization", auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let second: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(second["has_more"], false);
    let first_id = first["items"][0]["id"].as_str().unwrap();
    let second_id = second["items"][0]["id"].as_str().unwrap();
    assert_ne!(first_id, second_id);

    let conn = db::open(&fixture._dir.path().join("kanalib.db")).expect("db");
    conn.execute(
        "UPDATE desktop_devices SET revoked = 1 WHERE id = 'dev_test'",
        [],
    )
    .expect("revoke");
    let (status, _, _) = send(
        &fixture.private,
        Method::GET,
        "/api/desktop/pull",
        &[("authorization", auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn desktop_pull_excludes_non_allow_but_keeps_tombstones() {
    let fixture = write_fixture();
    let conn = db::open(&fixture._dir.path().join("kanalib.db")).expect("db");
    conn.execute(
        "UPDATE items SET decision = 'review' WHERE id = ?1",
        [db::SAMPLE_ITEM_B],
    )
    .expect("mark review");
    conn.execute(
        "UPDATE items SET deleted = 1, revision = 2, updated_at = 1800000000 WHERE id = ?1",
        [db::SAMPLE_ITEM_A],
    )
    .expect("tombstone");
    drop(conn);

    let auth = format!("Bearer {DEVICE_TOKEN}");
    let (status, _, body) = send(
        &fixture.private,
        Method::GET,
        "/api/desktop/pull?limit=10",
        &[("authorization", auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body).expect("json");
    let items = value["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], db::SAMPLE_ITEM_A);
    assert_eq!(items[0]["del"], true);
}

#[tokio::test]
async fn desktop_push_reports_conflict_and_missing() {
    let fixture = write_fixture();
    let auth = format!("Bearer {DEVICE_TOKEN}");
    let body = serde_json::to_vec(&serde_json::json!({
        "items": [{
            "id": db::SAMPLE_ITEM_A,
            "base_revision": 1,
            "star": 5,
            "favorite": false,
            "rating": "E"
        }]
    }))
    .expect("push json");
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/desktop/push")
        .header("authorization", &auth)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .expect("request");
    let (status, _, response) = execute(&fixture.private, request).await;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&response).expect("json");
    assert_eq!(value["applied"], 1);

    let (status, item) = get_json(
        &fixture.public,
        &format!("/api/items/{}", db::SAMPLE_ITEM_A),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item["st"], 5);
    assert_eq!(item["rt"], "E");
    assert_eq!(item["fav"], false);

    let body = serde_json::to_vec(&serde_json::json!({
        "items": [
            {"id": db::SAMPLE_ITEM_A, "base_revision": 1, "star": 2},
            {"id": MISSING_ITEM, "base_revision": 1, "star": 2}
        ]
    }))
    .expect("push json");
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/desktop/push")
        .header("authorization", &auth)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .expect("request");
    let (status, _, response) = execute(&fixture.private, request).await;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&response).expect("json");
    assert_eq!(value["applied"], 0);
    assert_eq!(value["conflicts"].as_array().expect("conflicts").len(), 1);
    assert_eq!(value["conflicts"][0]["server"]["revision"], 2);
    assert_eq!(value["missing"][0], MISSING_ITEM);
}

#[tokio::test]
async fn desktop_push_rejects_invalid_rating_and_oversized_tags() {
    let fixture = write_fixture();
    let auth = format!("Bearer {DEVICE_TOKEN}");

    for (body, code) in [
        (
            serde_json::json!({
                "items": [{
                    "id": db::SAMPLE_ITEM_A,
                    "base_revision": 1,
                    "rating": "unknown"
                }]
            }),
            "invalid_rating",
        ),
        (
            serde_json::json!({
                "items": [{
                    "id": db::SAMPLE_ITEM_A,
                    "base_revision": 1,
                    "tags": [{"name": "tag", "category": "x".repeat(65)}]
                }]
            }),
            "invalid_push",
        ),
    ] {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/desktop/push")
            .header("authorization", &auth)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .expect("request");
        let (status, _, response) = execute(&fixture.private, request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let value: Value = serde_json::from_slice(&response).expect("json");
        assert_eq!(value["error"]["code"], code);
    }
}

#[tokio::test]
async fn desktop_upload_streams_and_enforces_cap() {
    let fixture = write_fixture_with_limit(16);
    let auth = format!("Bearer {DEVICE_TOKEN}");
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/desktop/upload")
        .header("authorization", &auth)
        .header("x-kanalib-item-id", db::SAMPLE_ITEM_A)
        .body(Body::from(vec![7u8; 8]))
        .expect("request");
    let (status, _, response) = execute(&fixture.private, request).await;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&response).expect("json");
    assert_eq!(value["bytes"], 8);
    assert_eq!(value["file"], format!("uploads/{}.jpg", db::SAMPLE_ITEM_A));
    let uploaded = fixture
        ._dir
        .path()
        .join("media")
        .join("uploads")
        .join(format!("{}.jpg", db::SAMPLE_ITEM_A));
    assert_eq!(std::fs::metadata(uploaded).expect("uploaded file").len(), 8);

    let uri = format!("/api/desktop/uploads/{}.jpg", db::SAMPLE_ITEM_A);
    let (status, headers, body) = send(
        &fixture.private,
        Method::GET,
        &uri,
        &[("authorization", &auth)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, vec![7u8; 8]);
    assert_eq!(header_text(&headers, header::CONTENT_LENGTH), "8");
    assert_eq!(header_text(&headers, header::ACCEPT_RANGES), "bytes");

    let (status, headers, body) = send(
        &fixture.private,
        Method::GET,
        &uri,
        &[("authorization", &auth), ("range", "bytes=2-5")],
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, vec![7u8; 4]);
    assert_eq!(header_text(&headers, header::CONTENT_RANGE), "bytes 2-5/8");

    let (status, _, _) = send(
        &fixture.private,
        Method::GET,
        &uri,
        &[("authorization", &auth), ("range", "bytes=100-")],
    )
    .await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);

    let (status, headers, body) = send(
        &fixture.private,
        Method::HEAD,
        &uri,
        &[("authorization", &auth)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty());
    assert_eq!(header_text(&headers, header::CONTENT_LENGTH), "8");

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/desktop/upload")
        .header("authorization", &auth)
        .header("x-kanalib-item-id", db::SAMPLE_ITEM_A)
        .body(Body::from(vec![7u8; 32]))
        .expect("request");
    let (status, _, response) = execute(&fixture.private, request).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let value: Value = serde_json::from_slice(&response).expect("json");
    assert_eq!(value["error"]["code"], "upload_too_large");
}

#[tokio::test]
async fn desktop_local_publish_creates_visible_item_with_thumbs_and_original() {
    let fixture = build_fixture_with(
        false,
        Some(WriteConfig {
            ingest_secret: Some(Arc::from(INGEST_SECRET)),
            box_origin: None,
            media_signing_key: Some(Arc::from("w4-media-key")),
            upload_limit_bytes: 32 * 1024 * 1024,
            signed_url_ttl: Duration::from_secs(300),
        }),
    );
    let conn = db::open(&fixture._dir.path().join("kanalib.db")).expect("open write db");
    db::insert_device(
        &conn,
        "dev_local",
        "local device",
        &crypto::sha256_hex(DEVICE_TOKEN.as_bytes()),
    )
    .expect("insert device");
    let auth = format!("Bearer {DEVICE_TOKEN}");

    let mut jpeg = Vec::new();
    let pixels = image::RgbImage::from_fn(1200, 800, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 96])
    });
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 88)
        .encode_image(&pixels)
        .expect("encode jpeg");

    let body = serde_json::json!({
        "items": [{
            "id": NEW_ITEM,
            "name": "local-test.jpg",
            "title": "Local Test",
            "width": 1200,
            "height": 800,
            "rating": "G",
            "orig": "jpg",
            "tags": [{"name": "local", "category": "general", "score": 0.5}],
            "work_id": "local-work-1",
            "work_title": "Local Album",
            "page": 1,
            "page_count": 1,
            "group": "local",
            "mtime": 1_700_000_000i64
        }, {
            "id": db::SAMPLE_ITEM_A,
            "name": "collision.jpg",
            "width": 100,
            "height": 100,
            "orig": "jpg"
        }]
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/desktop/items")
        .header("authorization", &auth)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .expect("request");
    let (status, _, response) = execute(&fixture.private, request).await;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&response).expect("json");
    assert_eq!(value["results"][0]["status"], "created", "{value}");
    assert_eq!(value["results"][1]["status"], "exists");

    // 元数据已写入但原图/缩略图尚未到位：暂存条目与草稿作品都不能出现在公网。
    for uri in [
        format!("/api/items/{NEW_ITEM}"),
        "/api/works/local-work-1".to_string(),
        format!("/media/t256/{NEW_ITEM}.jpg"),
    ] {
        let (status, _, _) = send(&fixture.public, Method::GET, &uri, &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri} must stay hidden");
    }
    let (status, _, body) = send(&fixture.public, Method::GET, "/api/items?limit=100", &[]).await;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body).expect("json");
    assert!(
        !value["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|item| item["id"] == NEW_ITEM)
    );

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/desktop/upload")
        .header("authorization", &auth)
        .header("x-kanalib-item-id", NEW_ITEM)
        .body(Body::from(jpeg.clone()))
        .expect("request");
    let (status, _, response) = execute(&fixture.private, request).await;
    assert_eq!(status, StatusCode::OK, "{response:?}");
    let value: Value = serde_json::from_slice(&response).expect("json");
    assert_eq!(value["file"], format!("originals/{NEW_ITEM}.jpg"));
    let media = fixture._dir.path().join("media");
    for tier in ["t256", "t512", "t1024"] {
        let path = media.join(tier).join(format!("{NEW_ITEM}.jpg"));
        let bytes = std::fs::read(path).expect("thumbnail");
        assert!(bytes.starts_with(&[0xFF, 0xD8, 0xFF]), "{tier} is jpeg");
    }
    assert!(!media.join("t2048").join(format!("{NEW_ITEM}.jpg")).exists());

    let (status, _, body) = send(
        &fixture.public,
        Method::GET,
        &format!("/api/items/{NEW_ITEM}"),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(value["id"], NEW_ITEM);

    let (status, _, _) = send(&fixture.public, Method::GET, "/api/works/local-work-1", &[]).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, body) = send(
        &fixture.public,
        Method::GET,
        "/api/items?q=local-test&limit=10",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body).expect("json");
    assert!(
        value["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|item| item["id"] == NEW_ITEM)
    );

    let (status, headers, body) = send(
        &fixture.public,
        Method::GET,
        &format!("/media/t256/{NEW_ITEM}.jpg"),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with(&[0xFF, 0xD8, 0xFF]));
    assert_eq!(header_text(&headers, header::CONTENT_TYPE), "image/jpeg");

    let (status, headers, _) = send(
        &fixture.private,
        Method::GET,
        &format!("/api/items/{NEW_ITEM}/original"),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let location = header_text(&headers, header::LOCATION);
    assert!(location.starts_with(&format!("/original/{NEW_ITEM}?expires=")));
    let (status, _, body) = send(&fixture.private, Method::GET, &location, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, jpeg);

    let (status, _, body) = send(
        &fixture.public,
        Method::GET,
        &format!("/original/{NEW_ITEM}?expires=1&sig={}", "0".repeat(64)),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!body.starts_with(&[0xFF, 0xD8, 0xFF]));
}

#[tokio::test]
async fn original_proxy_signs_short_lived_box_url() {
    let fixture = write_fixture();
    let uri = format!("/api/items/{}/original", db::SAMPLE_ITEM_A);
    let (status, headers, _) = send(&fixture.private, Method::GET, &uri, &[]).await;
    assert_eq!(status, StatusCode::FOUND);
    let location = header_text(&headers, header::LOCATION);
    let (base, query) = location.split_once('?').expect("query");
    assert_eq!(base, format!("/original/{}", db::SAMPLE_ITEM_A));
    assert!(
        !location.contains(BOX_ORIGIN),
        "location must not leak the box origin: {location}"
    );
    let mut expires = None;
    let mut signature = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').expect("pair");
        match key {
            "expires" => expires = Some(value.to_string()),
            "sig" => signature = Some(value.to_string()),
            _ => {}
        }
    }
    let expires = expires.expect("expires");
    let signature = signature.expect("sig");
    let canonical = format!("GET\n/original/{}\n{expires}", db::SAMPLE_ITEM_A);
    assert_eq!(
        crypto::hmac_sha256_hex(b"w4-media-key", canonical.as_bytes()).expect("sign"),
        signature
    );

    let (status, _, _) = send(
        &fixture.private,
        Method::GET,
        &format!("/api/items/{MISSING_ITEM}/original"),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let conn = db::open(&fixture._dir.path().join("kanalib.db")).expect("db");
    conn.execute(
        "UPDATE items SET decision = 'review' WHERE id = ?1",
        [db::SAMPLE_ITEM_B],
    )
    .expect("mark review");
    let (status, _, _) = send(
        &fixture.private,
        Method::GET,
        &format!("/api/items/{}/original", db::SAMPLE_ITEM_B),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let plain = build_fixture(false);
    let (status, _, _) = send(
        &plain.private,
        Method::GET,
        &format!("/api/items/{}/original", db::SAMPLE_ITEM_A),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

const BOX_BODY: &[u8] = b"01234567";

async fn mock_box_original(
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
    method: Method,
    headers: HeaderMap,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    if id != db::SAMPLE_ITEM_A
        || !query.contains_key("expires")
        || !query
            .get("sig")
            .is_some_and(|signature| signature.len() == 64)
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let (status, content_range, payload): (StatusCode, Option<&str>, &[u8]) =
        match header_text(&headers, header::RANGE).as_str() {
            "" => (StatusCode::OK, None, BOX_BODY),
            "bytes=2-4" => (
                StatusCode::PARTIAL_CONTENT,
                Some("bytes 2-4/8"),
                &BOX_BODY[2..=4],
            ),
            _ => (StatusCode::RANGE_NOT_SATISFIABLE, Some("bytes */8"), &[]),
        };
    let body = if method == Method::HEAD {
        Vec::new()
    } else {
        payload.to_vec()
    };
    let mut response = (status, body).into_response();
    let out = response.headers_mut();
    out.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    out.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    out.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=300"),
    );
    out.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&payload.len().to_string()).expect("content length"),
    );
    if let Some(value) = content_range {
        out.insert(header::CONTENT_RANGE, HeaderValue::from_static(value));
    }
    response
}

#[tokio::test]
async fn private_listener_proxies_original_bytes_and_range() {
    let box_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind mock box");
    let box_addr = box_listener.local_addr().expect("mock box addr");
    let box_router = Router::new().route("/original/{id}", axum::routing::get(mock_box_original));
    tokio::spawn(async move {
        let _ = axum::serve(box_listener, box_router).await;
    });

    let mut config = write_config(32 * 1024 * 1024);
    config.box_origin = Some(Arc::from(format!("http://{box_addr}")));
    let fixture = build_fixture_with(false, Some(config));

    let sign_uri = format!("/api/items/{}/original", db::SAMPLE_ITEM_A);
    let (status, headers, _) = send(&fixture.private, Method::GET, &sign_uri, &[]).await;
    assert_eq!(status, StatusCode::FOUND);
    let location = header_text(&headers, header::LOCATION);
    assert!(location.starts_with("/original/"), "{location}");

    let (status, headers, body) = send(&fixture.private, Method::GET, &location, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, BOX_BODY);
    assert_eq!(header_text(&headers, header::CONTENT_TYPE), "image/jpeg");
    assert_eq!(header_text(&headers, header::CONTENT_LENGTH), "8");
    assert_eq!(header_text(&headers, header::ACCEPT_RANGES), "bytes");
    assert_eq!(
        header_text(&headers, header::CACHE_CONTROL),
        "private, max-age=300"
    );

    let (status, headers, body) = send(
        &fixture.private,
        Method::GET,
        &location,
        &[("range", "bytes=2-4")],
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, b"234");
    assert_eq!(header_text(&headers, header::CONTENT_RANGE), "bytes 2-4/8");
    assert_eq!(header_text(&headers, header::CONTENT_LENGTH), "3");

    let (status, headers, body) = send(&fixture.private, Method::HEAD, &location, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty());
    assert_eq!(header_text(&headers, header::CONTENT_LENGTH), "8");

    let public_uris = [
        location.clone(),
        format!("/original/{}", db::SAMPLE_ITEM_A),
        format!("/original/{}?expires=1&sig=zz", db::SAMPLE_ITEM_A),
    ];
    for uri in &public_uris {
        let (status, _, _) = send(&fixture.public, Method::GET, uri, &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "public {uri}");
    }
    let (status, _, _) = send(
        &fixture.private,
        Method::GET,
        &format!("/original/{}", db::SAMPLE_ITEM_A),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(
        &fixture.private,
        Method::GET,
        &format!("/original/{}?expires=1&sig=zz", db::SAMPLE_ITEM_A),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn tag_facets_count_distinct_items() {
    let fixture = fixture();
    let conn = db::open(&fixture._dir.path().join("kanalib.db")).expect("db");
    conn.execute(
        "INSERT INTO item_tags (item_id, tag_id, score, origin)
         VALUES (?1, 1, 0.1, 'desktop')",
        [db::SAMPLE_ITEM_A],
    )
    .expect("duplicate tag row");
    drop(conn);

    let (status, body) = get_json(&fixture.public, "/api/index").await;
    assert_eq!(status, StatusCode::OK);
    let tag = body["facets"]["tags"]
        .as_array()
        .expect("tags")
        .iter()
        .find(|tag| tag["name"] == "1girl")
        .expect("1girl facet");
    assert_eq!(
        tag["count"], 2,
        "same item/tag with another origin must not inflate facet count"
    );
}

#[tokio::test]
async fn public_listener_has_no_write_routes() {
    let fixture = write_fixture();
    for (method, uri) in [
        (Method::POST, "/internal/ingest/batch"),
        (Method::POST, "/internal/ingest/delete"),
        (Method::POST, "/api/desktop/push"),
        (Method::POST, "/api/desktop/upload"),
        (Method::GET, "/api/desktop/pull"),
        (
            Method::GET,
            "/api/items/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/original",
        ),
    ] {
        let (status, _, _) = send(&fixture.public, method, uri, &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
}
