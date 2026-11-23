use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use kanalib_ingest::{
    db::Db,
    push::{
        self, PushConfig, PushError, PushRequest, PushResponse, PushTransport, batch_id_for,
        build_allow_batch, enqueue_allow, enqueue_decisions, enqueue_delete, flush, pending,
        signed_original_url, signed_request, verify_original,
    },
};
use kanalib_wall_model::{IngestDelete, IngestDeleteItem};
use rusqlite::params;

const ITEM_ID: &str = "cccccccccccccccccccccccccccccccccccccccc";
const WORK_ID: &str = "work-push";
const AUTHOR_ID: &str = "author-push";

struct Scripted {
    responses: Arc<Mutex<VecDeque<Result<u16, String>>>>,
    requests: Arc<Mutex<Vec<PushRequest>>>,
}

impl Scripted {
    fn new(responses: Vec<Result<u16, String>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn requests(&self) -> Vec<PushRequest> {
        self.requests.lock().expect("requests").clone()
    }
}

impl PushTransport for Scripted {
    fn send<'a>(
        &'a self,
        request: PushRequest,
    ) -> Pin<Box<dyn Future<Output = Result<PushResponse, PushError>> + Send + 'a>> {
        self.requests.lock().expect("requests").push(request);
        let next = self
            .responses
            .lock()
            .expect("responses")
            .pop_front()
            .unwrap_or(Ok(200));
        Box::pin(async move {
            match next {
                Ok(status) => Ok(PushResponse {
                    status,
                    body: format!("status {status}"),
                }),
                Err(error) => Err(PushError(error)),
            }
        })
    }
}

fn seeded_db() -> Db {
    let db = Db::open_in_memory().expect("db");
    db.connection()
        .execute(
            "INSERT INTO authors (id, source, external_id, name)
             VALUES (?1, 'pixiv', '1', 'Push Author')",
            [AUTHOR_ID],
        )
        .expect("author");
    db.connection()
        .execute(
            "INSERT INTO works (id, source, external_id, author_id, title, page_count)
             VALUES (?1, 'pixiv', '2', ?2, 'Push Title', 1)",
            params![WORK_ID, AUTHOR_ID],
        )
        .expect("work");
    db.connection()
        .execute(
            "INSERT INTO items
                (id, work_id, source, page, width, height, ext, original_path,
                 thumb_256, thumb_512, decision, revision, updated_at)
             VALUES (?1, ?2, 'pixiv', 0, 1200, 1600, 'jpg', 'pages/push.jpg',
                     'thumbs/t256/push.jpg', 'thumbs/t512/push.jpg',
                     'allow', 3, 1_700_000_000)",
            params![ITEM_ID, WORK_ID],
        )
        .expect("item");
    db.connection()
        .execute(
            "INSERT INTO tags (name, category) VALUES ('push-tag', 'general')",
            [],
        )
        .expect("tag");
    db.connection()
        .execute(
            "INSERT INTO item_tags (item_id, tag_id, score, origin)
             SELECT ?1, id, 0.8, 'model' FROM tags WHERE name = 'push-tag'",
            [ITEM_ID],
        )
        .expect("item tag");
    db.connection()
        .execute(
            "INSERT INTO decisions (item_id, decision, reason, rules_version)
             VALUES (?1, 'allow', 'test', 'rules-1')",
            [ITEM_ID],
        )
        .expect("decision");
    db
}

fn config() -> PushConfig {
    let mut config = PushConfig::new("http://127.0.0.1:8788", "push-secret");
    config.max_attempts = 3;
    config.backoff_base = Duration::from_secs(5);
    config.backoff_max = Duration::from_secs(60);
    config
}

#[test]
fn allow_batch_is_built_from_live_rows_and_fails_closed() {
    let db = seeded_db();
    let batch = build_allow_batch(&db, &[ITEM_ID.to_string()], Some("rules-1")).expect("batch");
    assert_eq!(batch.source, "pixiv");
    assert_eq!(batch.rules_version.as_deref(), Some("rules-1"));
    assert_eq!(batch.items.len(), 1);
    let entry = &batch.items[0];
    assert_eq!(entry.decision, "allow");
    assert_eq!(entry.revision, 3);
    assert_eq!(entry.item.name, "push.jpg");
    assert_eq!(entry.item.width, 1200);
    assert_eq!(entry.item.height, 1600);
    assert_eq!(entry.item.work_id.as_deref(), Some(WORK_ID));
    assert_eq!(entry.item.page_count, Some(1));
    assert_eq!(entry.item.author.as_deref(), Some("Push Author"));
    assert_eq!(entry.item.author_id.as_deref(), Some(AUTHOR_ID));
    assert_eq!(entry.item.title.as_deref(), Some("Push Title"));
    assert_eq!(entry.item.orig.as_deref(), Some("jpg"));
    assert_eq!(entry.item.mtime, 1_700_000_000);
    assert_eq!(entry.item.rating, "G");
    assert_eq!(entry.item.sharp, Some(0));
    assert!(!entry.item.favorite);
    assert_eq!(entry.item.phash, None);
    assert_eq!(entry.item.tags.len(), 1);
    assert_eq!(entry.item.tags[0].name, "push-tag");

    db.connection()
        .execute(
            "UPDATE items SET decision = 'review' WHERE id = ?1",
            [ITEM_ID],
        )
        .expect("review");
    let error = build_allow_batch(&db, &[ITEM_ID.to_string()], None).expect_err("must fail");
    assert!(error.to_string().contains("refusing to push"));

    db.connection()
        .execute(
            "UPDATE items SET decision = 'allow' WHERE id = ?1",
            [ITEM_ID],
        )
        .expect("allow");
    db.connection()
        .execute("DELETE FROM decisions WHERE item_id = ?1", [ITEM_ID])
        .expect("drop decision");
    let error = build_allow_batch(&db, &[ITEM_ID.to_string()], None).expect_err("must fail");
    assert!(error.to_string().contains("no allow decision record"));
}

#[test]
fn enqueue_is_idempotent_and_persists_payload() {
    let db = seeded_db();
    let batch = build_allow_batch(&db, &[ITEM_ID.to_string()], None).expect("batch");
    let payload = serde_json::to_vec(&batch).expect("json");
    let id = batch_id_for(&payload);
    assert!(enqueue_allow(&db, &id, &batch).expect("enqueue"));
    assert!(!enqueue_allow(&db, &id, &batch).expect("duplicate"));

    let rows = pending(&db, 0, 10).expect("pending");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].payload, payload);

    let delete = IngestDelete {
        source: "pixiv".to_string(),
        rules_version: None,
        items: vec![IngestDeleteItem {
            id: ITEM_ID.to_string(),
            revision: 4,
        }],
    };
    assert!(enqueue_delete(&db, "delete-1", &delete).expect("enqueue delete"));
    assert!(!enqueue_delete(&db, "delete-1", &delete).expect("duplicate delete"));
    assert_eq!(pending(&db, 0, 10).expect("pending").len(), 2);
}

#[tokio::test]
async fn flush_retries_5xx_then_acks_with_signed_headers() {
    let db = seeded_db();
    let batch = build_allow_batch(&db, &[ITEM_ID.to_string()], None).expect("batch");
    let payload = serde_json::to_vec(&batch).expect("json");
    let id = batch_id_for(&payload);
    enqueue_allow(&db, &id, &batch).expect("enqueue");
    let transport = Scripted::new(vec![Ok(503), Ok(200)]);
    let config = config();

    let outcome = flush(&db, &config, &transport, 1_000, 10)
        .await
        .expect("flush");
    assert_eq!(outcome.retried, 1);
    assert_eq!(outcome.acked, 0);
    let row: (String, i64, Option<i64>) = db
        .connection()
        .query_row(
            "SELECT state, attempts, next_retry_at FROM push_batches WHERE id = ?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("row");
    assert_eq!(row.0, "pending");
    assert_eq!(row.1, 1);
    assert_eq!(row.2, Some(1_005));

    let outcome = flush(&db, &config, &transport, 1_005, 10)
        .await
        .expect("flush");
    assert_eq!(outcome.acked, 1);
    assert_eq!(pending(&db, 2_000, 10).expect("pending").len(), 0);
    assert!(push::replay(&db, &id).expect("replay"));
    assert_eq!(pending(&db, 2_000, 10).expect("replayed").len(), 1);
    assert!(!push::replay(&db, "missing-batch").expect("missing"));

    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    let first = &requests[0];
    assert_eq!(first.url, "http://127.0.0.1:8788/internal/ingest/batch");
    assert_eq!(first.body, payload);
    let header = |name: &str| {
        first
            .headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or_default()
    };
    assert_eq!(header("x-kanalib-batch-id"), id);
    assert_eq!(header("x-kanalib-timestamp"), "1000");
    assert_eq!(header("x-kanalib-signature").len(), 64);
}

#[tokio::test]
async fn flush_rejects_4xx_and_stops_retrying() {
    let db = seeded_db();
    let batch = build_allow_batch(&db, &[ITEM_ID.to_string()], None).expect("batch");
    let payload = serde_json::to_vec(&batch).expect("json");
    let id = batch_id_for(&payload);
    enqueue_allow(&db, &id, &batch).expect("enqueue");
    let transport = Scripted::new(vec![Ok(400)]);

    let outcome = flush(&db, &config(), &transport, 1_000, 10)
        .await
        .expect("flush");
    assert_eq!(outcome.rejected, 1);
    let state: String = db
        .connection()
        .query_row(
            "SELECT state FROM push_batches WHERE id = ?1",
            [&id],
            |row| row.get(0),
        )
        .expect("state");
    assert_eq!(state, "rejected");
    let outcome = flush(&db, &config(), &transport, 2_000, 10)
        .await
        .expect("flush");
    assert_eq!(outcome.acked + outcome.retried + outcome.rejected, 0);
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn flush_marks_failed_after_max_attempts_and_retries_network_errors() {
    let db = seeded_db();
    let batch = build_allow_batch(&db, &[ITEM_ID.to_string()], None).expect("batch");
    let payload = serde_json::to_vec(&batch).expect("json");
    let id = batch_id_for(&payload);
    enqueue_allow(&db, &id, &batch).expect("enqueue");
    let transport = Scripted::new(vec![Err("offline".to_string()), Ok(500), Ok(500)]);
    let mut config = config();
    config.max_attempts = 2;

    flush(&db, &config, &transport, 1_000, 10)
        .await
        .expect("flush");
    let (state, attempts): (String, i64) = db
        .connection()
        .query_row(
            "SELECT state, attempts FROM push_batches WHERE id = ?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("row");
    assert_eq!(state, "pending");
    assert_eq!(attempts, 1);

    flush(&db, &config, &transport, 1_005, 10)
        .await
        .expect("flush");
    let (state, attempts, error): (String, i64, Option<String>) = db
        .connection()
        .query_row(
            "SELECT state, attempts, last_error FROM push_batches WHERE id = ?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("row");
    assert_eq!(state, "failed");
    assert_eq!(attempts, 2);
    assert!(error.expect("error").contains("status 500"));
}

#[test]
fn original_url_round_trips_and_expires() {
    let url = signed_original_url("http://box.test:8789/", "media-key", ITEM_ID, 300, 1_000);
    assert!(url.starts_with(&format!(
        "http://box.test:8789/original/{ITEM_ID}?expires=1300&sig="
    )));
    let query = url.split_once('?').expect("query").1;
    let mut expires = 0;
    let mut signature = String::new();
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').expect("pair");
        match key {
            "expires" => expires = value.parse().expect("expires"),
            "sig" => signature = value.to_string(),
            _ => {}
        }
    }
    assert_eq!(expires, 1_300);
    assert!(verify_original(
        "media-key",
        ITEM_ID,
        expires,
        &signature,
        1_000
    ));
    assert!(!verify_original(
        "media-key",
        ITEM_ID,
        expires,
        &signature,
        1_301
    ));
    assert!(!verify_original(
        "wrong-key",
        ITEM_ID,
        expires,
        &signature,
        1_000
    ));
    assert!(!verify_original(
        "media-key",
        ITEM_ID,
        expires,
        "deadbeef",
        1_000
    ));
}

#[test]
fn signed_request_targets_delete_path() {
    let batch = push::PushBatch {
        id: "delete-1".to_string(),
        kind: push::PushKind::Delete,
        payload: br#"{"items":[]}"#.to_vec(),
        attempts: 0,
    };
    let request = signed_request(&config(), &batch, 42);
    assert_eq!(request.url, "http://127.0.0.1:8788/internal/ingest/delete");
    assert_eq!(request.body, br#"{"items":[]}"#);
}

#[test]
fn signed_request_covers_batch_id_and_timestamp() {
    let batch = push::PushBatch {
        id: "allow-1".to_string(),
        kind: push::PushKind::Allow,
        payload: br#"{"items":[]}"#.to_vec(),
        attempts: 0,
    };
    let request = signed_request(&config(), &batch, 42);
    let signature = request
        .headers
        .iter()
        .find(|(name, _)| name == "x-kanalib-signature")
        .map(|(_, value)| value.clone())
        .expect("signature");
    assert_eq!(
        signature,
        push::sign_payload(
            "push-secret",
            push::ALLOW_PATH,
            "allow-1",
            42,
            &batch.payload
        )
    );
    assert_ne!(
        signature,
        push::sign_payload(
            "push-secret",
            push::ALLOW_PATH,
            "allow-2",
            42,
            &batch.payload
        )
    );
    assert_ne!(
        signature,
        push::sign_payload(
            "push-secret",
            push::ALLOW_PATH,
            "allow-1",
            43,
            &batch.payload
        )
    );
}

#[test]
fn allow_batch_maps_ratings_and_refuses_unknown() {
    let db = seeded_db();
    db.connection()
        .execute("UPDATE works SET rating = 'r18' WHERE id = ?1", [WORK_ID])
        .expect("r18");
    let batch = build_allow_batch(&db, &[ITEM_ID.to_string()], None).expect("batch");
    assert_eq!(batch.items[0].item.rating, "E");

    for rating in ["r18g", "unknown", "g"] {
        db.connection()
            .execute(
                "UPDATE works SET rating = ?2 WHERE id = ?1",
                params![WORK_ID, rating],
            )
            .expect("rating");
        let result = build_allow_batch(&db, &[ITEM_ID.to_string()], None);
        if rating == "g" {
            assert_eq!(result.expect("known rating").items[0].item.rating, "G");
        } else {
            assert!(result.is_err(), "{rating} must not be pushed");
        }
    }
}

#[test]
fn enqueue_decisions_only_picks_unpushed_allow_rows() {
    let db = seeded_db();
    let enqueued = enqueue_decisions(&db, Some("rules-1"), 100).expect("enqueue");
    assert_eq!(enqueued.allow_batches, 1);
    assert_eq!(enqueued.allow_items, 1);
    assert_eq!(enqueued.delete_batches, 0);
    assert_eq!(pending(&db, 0, 10).expect("pending").len(), 1);

    let again = enqueue_decisions(&db, Some("rules-1"), 100).expect("enqueue");
    assert_eq!((again.allow_batches, again.allow_items), (0, 0));

    db.connection()
        .execute(
            "UPDATE items SET decision = 'review' WHERE id = ?1",
            [ITEM_ID],
        )
        .expect("review");
    db.connection()
        .execute(
            "UPDATE items SET pushed_revision = revision WHERE id = ?1",
            [ITEM_ID],
        )
        .expect("mark pushed");
    let none = enqueue_decisions(&db, None, 100).expect("enqueue");
    assert_eq!((none.allow_batches, none.allow_items), (0, 0));
}

#[tokio::test]
async fn ack_records_pushed_revision_and_delete_progress() {
    let db = seeded_db();
    let enqueued = enqueue_decisions(&db, None, 100).expect("enqueue");
    assert_eq!(enqueued.allow_items, 1);
    let transport = Scripted::new(Vec::new());
    flush(&db, &config(), &transport, 1_000, 10)
        .await
        .expect("flush allow");
    let pushed: i64 = db
        .connection()
        .query_row(
            "SELECT pushed_revision FROM items WHERE id = ?1",
            [ITEM_ID],
            |row| row.get(0),
        )
        .expect("pushed");
    assert_eq!(pushed, 3);

    db.connection()
        .execute(
            "UPDATE works SET status = 'deleted' WHERE id = ?1",
            [WORK_ID],
        )
        .expect("delete work");
    let enqueued = enqueue_decisions(&db, None, 100).expect("enqueue delete");
    assert_eq!(enqueued.delete_batches, 1);
    assert_eq!(enqueued.delete_items, 1);
    let delete_ids: Vec<String> = pending(&db, 2_000, 10)
        .expect("pending")
        .into_iter()
        .map(|batch| batch.id)
        .collect();
    assert_eq!(delete_ids.len(), 1);
    assert!(delete_ids[0].starts_with("delete-"));

    flush(&db, &config(), &transport, 2_000, 10)
        .await
        .expect("flush delete");
    let delete_pushed: i64 = db
        .connection()
        .query_row(
            "SELECT delete_pushed FROM items WHERE id = ?1",
            [ITEM_ID],
            |row| row.get(0),
        )
        .expect("delete pushed");
    assert_eq!(delete_pushed, 1);
    let after = enqueue_decisions(&db, None, 100).expect("enqueue");
    assert_eq!(
        (after.allow_batches, after.delete_batches),
        (0, 0),
        "nothing should be enqueued twice"
    );
}

#[test]
fn thumb_request_signs_path_and_body() {
    let config = config();
    let request = push::thumb_request(&config, ITEM_ID, "t256", b"\xff\xd8\xffjpeg".to_vec(), 99);
    assert_eq!(
        request.url,
        format!("http://127.0.0.1:8788/internal/ingest/thumb/t256/{ITEM_ID}.jpg")
    );
    let signature = request
        .headers
        .iter()
        .find(|(name, _)| name == "x-kanalib-signature")
        .map(|(_, value)| value.clone())
        .expect("signature");
    assert_eq!(
        signature,
        push::sign_payload(
            "push-secret",
            &format!("/internal/ingest/thumb/t256/{ITEM_ID}.jpg"),
            "",
            99,
            &request.body
        )
    );
}
