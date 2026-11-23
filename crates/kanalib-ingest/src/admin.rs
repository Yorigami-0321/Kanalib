//! 私网管理页与审核/规则 API；只应监听回环或 EasyTier 虚拟 IP。

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Result, bail};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    db::{Db, now},
    metrics::{Metrics, escape_label},
    review::{self, ReviewAction},
    rules_edit::{ApplyOutcome, ProposalKind, RulesEditor, payload_value},
    rules_state::RulesState,
};

#[derive(Clone)]
pub struct AdminState {
    pub db_path: Arc<PathBuf>,
    pub rules: Arc<RulesState>,
    pub rules_path: Option<PathBuf>,
    pub editor: Arc<RulesEditor>,
    pub metrics: Arc<Metrics>,
}

type ItemPaths = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

impl AdminState {
    pub fn new(
        db_path: PathBuf,
        rules: Arc<RulesState>,
        rules_path: Option<PathBuf>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            db_path: Arc::new(db_path),
            rules,
            rules_path,
            editor: Arc::new(RulesEditor::default()),
            metrics,
        }
    }
}

pub fn router(state: AdminState) -> Router {
    Router::new()
        .route("/admin", get(page))
        .route("/admin/", get(page))
        .route("/admin/api/health", get(health))
        .route("/admin/api/status", get(status))
        .route("/admin/api/metrics", get(metrics_text))
        .route("/admin/api/review", get(review_list))
        .route("/admin/api/review/{id}/allow", post(review_allow))
        .route("/admin/api/review/{id}/reject", post(review_reject))
        .route("/admin/api/items/{id}/thumb", get(item_thumb))
        .route("/admin/api/items/{id}/original", get(item_original))
        .route("/admin/api/items/{id}/recheck", post(recheck_item))
        .route(
            "/admin/api/proposals",
            get(list_proposals).post(create_proposal),
        )
        .route("/admin/api/proposals/{id}/preview", get(preview_proposal))
        .route("/admin/api/proposals/{id}/apply", post(apply_proposal))
        .route("/admin/api/proposals/{id}/reject", post(reject_proposal))
        .route("/admin/api/rules/reload", post(reload_rules))
        .with_state(state)
}

async fn page() -> Html<&'static str> {
    Html(ADMIN_PAGE)
}

async fn health(State(state): State<AdminState>) -> Response {
    let active = state.rules.current();
    Json(json!({
        "ok": true,
        "schema": crate::db::SCHEMA_VERSION,
        "rules_version": active.version,
        "rules_hash": active.config_hash,
    }))
    .into_response()
}

async fn status(State(state): State<AdminState>) -> Response {
    match blocking_ok(move || status_json(&state)).await {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

fn status_json(state: &AdminState) -> Result<Value> {
    let db = Db::open_read(&state.db_path)?;
    let active = state.rules.current();
    let latest = db.latest_rule_snapshot()?;
    let (review_count, review_oldest) = db.review_backlog()?;
    Ok(json!({
        "ok": true,
        "rules": {
            "version": active.version,
            "hash": active.config_hash,
            "path": state.rules_path.as_ref().map(|path| path.display().to_string()),
            "loaded_at": latest.as_ref().map(|snapshot| snapshot.loaded_at),
            "pending_proposals": db.pending_proposal_count()?,
            "tag_blacklist_empty": active.config.tag_blacklist_is_empty(),
            "last_error": state.rules.last_error(),
        },
        "items": db.decision_counts()?.into_iter().collect::<BTreeMap<_, _>>(),
        "download_queue": db.download_queue_counts()?.into_iter().collect::<BTreeMap<_, _>>(),
        "review": { "count": review_count, "oldest_updated_at": review_oldest },
        "push_pending": db.push_pending_count()?,
    }))
}

async fn metrics_text(State(state): State<AdminState>) -> Response {
    match blocking_ok(move || metrics_body(&state)).await {
        Ok(text) => {
            let mut response = Response::new(Body::from(text));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; version=0.0.4"),
            );
            response
        }
        Err(response) => response,
    }
}

fn metrics_body(state: &AdminState) -> Result<String> {
    let db = Db::open_read(&state.db_path)?;
    let mut gauges = String::new();
    for (queue, count) in db.download_queue_counts()? {
        gauges.push_str(&format!(
            "queue_depth{{state=\"{}\"}} {count}\n",
            escape_label(&queue)
        ));
    }
    let (review, oldest) = db.review_backlog()?;
    gauges.push_str(&format!("review_queue_depth {review}\n"));
    gauges.push_str(&format!(
        "review_age_seconds {}\n",
        oldest.map_or(0, |at| (now() - at).max(0))
    ));
    gauges.push_str(&format!("push_pending {}\n", db.push_pending_count()?));
    let active = state.rules.current();
    gauges.push_str(&format!(
        "rules_version_info{{version=\"{}\",config_hash=\"{}\"}} 1\n",
        escape_label(&active.version),
        escape_label(&active.config_hash)
    ));
    Ok(state.metrics.render(&gauges))
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    limit: Option<usize>,
}

async fn review_list(State(state): State<AdminState>, Query(query): Query<LimitQuery>) -> Response {
    let limit = query.limit.unwrap_or(60).clamp(1, 200);
    match blocking_ok(move || review_json(&state.db_path, limit)).await {
        Ok(values) => Json(values).into_response(),
        Err(response) => response,
    }
}

fn review_json(db_path: &Path, limit: usize) -> Result<Vec<Value>> {
    let db = Db::open_read(db_path)?;
    let mut statement = db.connection().prepare(
        "SELECT i.id, i.thumb_512, i.thumb_256, i.decision_reason, i.width, i.height,
                i.updated_at, i.source,
                (SELECT MAX(score) FROM item_tags WHERE item_id = i.id),
                w.external_id, w.title, w.rating, w.x_restrict,
                w.restriction_attributes, a.external_id, a.name
           FROM items i JOIN works w ON w.id = i.work_id
            LEFT JOIN authors a ON a.id = w.author_id
          WHERE i.decision = 'review'
          ORDER BY i.updated_at DESC, i.id
          LIMIT ?1",
    )?;
    let rows = statement.query_map([limit as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, u32>(4)?,
            row.get::<_, u32>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, Option<f64>>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, String>(10)?,
            row.get::<_, String>(11)?,
            row.get::<_, Option<i64>>(12)?,
            row.get::<_, String>(13)?,
            row.get::<_, Option<String>>(14)?,
            row.get::<_, Option<String>>(15)?,
        ))
    })?;
    let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut values = Vec::with_capacity(rows.len());
    for (
        id,
        thumb_512,
        thumb_256,
        reason,
        width,
        height,
        updated_at,
        source,
        score,
        work_external_id,
        title,
        rating,
        x_restrict,
        restriction_attributes,
        author_external_id,
        author_name,
    ) in rows
    {
        let tags = db
            .item_tag_names(&id)?
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        let r18g = review::is_r18g_item(&db, &id, &rating, x_restrict, &restriction_attributes)?;
        values.push(json!({
            "id": id,
            "thumb": thumb_512.or(thumb_256).map(|_| format!("/admin/api/items/{id}/thumb")),
            "original": format!("/admin/api/items/{id}/original"),
            "reason": reason,
            "width": width,
            "height": height,
            "updated_at": updated_at,
            "source": source,
            "score": score,
            "work_id": work_external_id,
            "title": title,
            "rating": rating,
            "author_id": author_external_id,
            "author_name": author_name,
            "tags": tags,
            "r18g": r18g,
            "allowable": !r18g,
        }));
    }
    Ok(values)
}

#[derive(Debug, Deserialize, Default)]
struct ActorRequest {
    reviewer: Option<String>,
}

impl ActorRequest {
    fn reviewer(&self) -> String {
        self.reviewer
            .clone()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "operator".to_string())
    }
}

async fn review_allow(
    State(state): State<AdminState>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<ActorRequest>,
) -> Response {
    review_action(state, id, ReviewAction::Allow, body.reviewer()).await
}

async fn review_reject(
    State(state): State<AdminState>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<ActorRequest>,
) -> Response {
    review_action(state, id, ReviewAction::Reject, body.reviewer()).await
}

async fn review_action(
    state: AdminState,
    id: String,
    action: ReviewAction,
    reviewer: String,
) -> Response {
    let db_path = state.db_path.clone();
    let metrics = state.metrics.clone();
    match blocking_conflict(move || {
        let mut db = Db::open(&db_path)?;
        let decision = review::decide(&mut db, &id, action, &reviewer)?;
        metrics.record_decision(&decision);
        metrics.record_reason(match action {
            ReviewAction::Allow => "review approved",
            ReviewAction::Reject => "review rejected",
        });
        Ok(json!({ "ok": true, "decision": decision }))
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

enum ItemFile {
    Thumb,
    Original,
}

async fn item_thumb(State(state): State<AdminState>, AxumPath(id): AxumPath<String>) -> Response {
    serve_item_file(state, id, ItemFile::Thumb).await
}

async fn item_original(
    State(state): State<AdminState>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    serve_item_file(state, id, ItemFile::Original).await
}

async fn serve_item_file(state: AdminState, id: String, kind: ItemFile) -> Response {
    if !kanalib_wall_model::valid_item_id(&id) {
        return not_found();
    }
    let db_path = state.db_path.clone();
    let lookup = id.clone();
    let path = match blocking_ok(move || {
        let db = Db::open_read(&db_path)?;
        let row: Option<ItemPaths> = db
            .connection()
            .query_row(
                "SELECT thumb_512, thumb_256, original_path, ext FROM items WHERE id=?1",
                [lookup],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        Ok(row)
    })
    .await
    {
        Ok(row) => row,
        Err(response) => return response,
    };
    let Some((thumb_512, thumb_256, original_path, ext)) = path else {
        return not_found();
    };
    let (path, extension) = match kind {
        ItemFile::Thumb => {
            let path = thumb_512.or(thumb_256);
            match path {
                Some(path) => {
                    let extension = extension_of(&path);
                    (path, extension)
                }
                None => return not_found(),
            }
        }
        ItemFile::Original => match original_path {
            Some(path) => {
                let extension = ext.unwrap_or_else(|| extension_of(&path));
                (path, extension)
            }
            None => return not_found(),
        },
    };
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(_) => return not_found(),
    };
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(&extension)),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Debug, Deserialize, Default)]
struct RecheckRequest {
    force: Option<bool>,
}

async fn recheck_item(
    State(state): State<AdminState>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<RecheckRequest>,
) -> Response {
    let force = body.force.unwrap_or(false);
    match blocking_conflict(move || {
        let db = Db::open(&state.db_path)?;
        let current: Option<String> = db
            .connection()
            .query_row("SELECT decision FROM items WHERE id=?1", [&id], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(current) = current else {
            bail!("item {id} not found");
        };
        if current == "pending" {
            return Ok(json!({ "ok": true, "queued": false, "decision": current }));
        }
        if current == "allow" && !force {
            bail!("item {id} is already allow; confirm force to requeue");
        }
        let queued = db.requeue_decision(&id, force)?;
        Ok(json!({ "ok": true, "queued": queued, "previous": current }))
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

#[derive(Debug, Deserialize, Default)]
struct ProposalQuery {
    state: Option<String>,
}

async fn list_proposals(
    State(state): State<AdminState>,
    Query(query): Query<ProposalQuery>,
) -> Response {
    match blocking_ok(move || proposals_json(&state.db_path, query.state.as_deref())).await {
        Ok(values) => Json(values).into_response(),
        Err(response) => response,
    }
}

async fn preview_proposal(
    State(state): State<AdminState>,
    AxumPath(id): AxumPath<i64>,
) -> Response {
    let Some(rules_path) = state.rules_path.clone() else {
        return bad_request("rules_file is not configured");
    };
    let db_path = Arc::clone(&state.db_path);
    let editor = Arc::clone(&state.editor);
    match blocking_ok(move || preview_json(&editor, &db_path, &rules_path, id)).await {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

fn preview_json(editor: &RulesEditor, db_path: &Path, rules_path: &Path, id: i64) -> Result<Value> {
    let db = Db::open_read(db_path)?;
    let proposal = db
        .rule_proposal(id)?
        .ok_or_else(|| anyhow::anyhow!("proposal {id} not found"))?;
    let kind = ProposalKind::parse(&proposal.kind)?;
    let preview = editor.preview(&db, rules_path, id)?;
    let action = if kind.is_add() { "add" } else { "remove" };
    let table = format!("rules.blacklist.{}", kind.table_key());
    let summary = format!(
        "{} {:?} → {table}；rules.version {} → {}",
        if kind.is_add() { "追加" } else { "移除" },
        preview.value,
        preview.current_version,
        preview.next_version
    );
    Ok(json!({
        "id": id,
        "kind": proposal.kind,
        "action": action,
        "table": table,
        "value": preview.value,
        "from_version": preview.current_version,
        "to_version": preview.next_version,
        "already_satisfied": preview.already_satisfied,
        "diff": preview.diff,
        "summary": summary,
    }))
}

fn proposals_json(db_path: &Path, state: Option<&str>) -> Result<Vec<Value>> {
    let db = Db::open_read(db_path)?;
    Ok(db
        .rule_proposals(state)?
        .into_iter()
        .map(|proposal| {
            let kind = ProposalKind::parse(&proposal.kind).ok();
            let value = kind.and_then(|kind| payload_value(kind, &proposal.payload_json).ok());
            json!({
                "id": proposal.id,
                "kind": proposal.kind,
                "value": value,
                "source_item_id": proposal.source_item_id,
                "note": proposal.note,
                "state": proposal.state,
                "created_at": proposal.created_at,
                "decided_at": proposal.decided_at,
                "decided_by": proposal.decided_by,
                "apply_error": proposal.apply_error,
            })
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct NewProposal {
    kind: String,
    value: String,
    source_item_id: Option<String>,
    #[serde(default)]
    note: String,
}

async fn create_proposal(
    State(state): State<AdminState>,
    Json(body): Json<NewProposal>,
) -> Response {
    let kind = match ProposalKind::parse(&body.kind) {
        Ok(kind) => kind,
        Err(error) => return bad_request(error.to_string()),
    };
    let state_for_task = state.clone();
    match blocking_conflict(move || {
        let db = Db::open(&state_for_task.db_path)?;
        let id = state_for_task.editor.create_proposal(
            &db,
            kind,
            &body.value,
            body.source_item_id.as_deref(),
            &body.note,
        )?;
        Ok(json!({ "ok": true, "id": id }))
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

async fn apply_proposal(
    State(state): State<AdminState>,
    AxumPath(id): AxumPath<i64>,
    Json(body): Json<ActorRequest>,
) -> Response {
    let Some(rules_path) = state.rules_path.clone() else {
        return bad_request("rules_file is not configured");
    };
    let reviewer = body.reviewer();
    let state_for_task = state.clone();
    match blocking_conflict(move || {
        let db = Db::open(&state_for_task.db_path)?;
        let outcome = match state_for_task.editor.apply(&db, &rules_path, id, &reviewer) {
            Ok(outcome) => outcome,
            Err(error) => {
                state_for_task
                    .editor
                    .record_apply_error(&db, id, &error.to_string())?;
                return Err(error);
            }
        };
        match state_for_task.rules.reload(&db, "admin-apply") {
            Ok(_) => {
                let version = outcome.version().to_string();
                let already_satisfied = matches!(outcome, ApplyOutcome::AlreadySatisfied { .. });
                Ok(json!({
                    "ok": true,
                    "version": version,
                    "already_satisfied": already_satisfied,
                }))
            }
            Err(error) => {
                state_for_task
                    .editor
                    .record_apply_error(&db, id, &error.to_string())?;
                Err(anyhow::anyhow!("rules written but reload failed: {error}"))
            }
        }
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

async fn reject_proposal(
    State(state): State<AdminState>,
    AxumPath(id): AxumPath<i64>,
    Json(body): Json<ActorRequest>,
) -> Response {
    let reviewer = body.reviewer();
    match blocking_conflict(move || {
        let db = Db::open(&state.db_path)?;
        state.editor.reject(&db, id, &reviewer)?;
        Ok(json!({ "ok": true }))
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

async fn reload_rules(State(state): State<AdminState>) -> Response {
    match blocking_conflict(move || {
        let db = Db::open(&state.db_path)?;
        match state.rules.reload(&db, "admin") {
            Ok(crate::rules_state::ReloadOutcome::Reloaded) => {
                let active = state.rules.current();
                Ok(json!({
                    "ok": true,
                    "reloaded": true,
                    "version": active.version,
                    "hash": active.config_hash,
                }))
            }
            Ok(crate::rules_state::ReloadOutcome::Unchanged) => {
                Ok(json!({ "ok": true, "reloaded": false }))
            }
            Ok(crate::rules_state::ReloadOutcome::Skipped) => {
                Err(anyhow::anyhow!("rules_file is not configured"))
            }
            Err(error) => Err(error),
        }
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

#[allow(clippy::result_large_err)]
async fn blocking_ok<T, F>(task: F) -> Result<T, Response>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(task).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(internal(error.to_string())),
        Err(error) => Err(internal(error.to_string())),
    }
}

#[allow(clippy::result_large_err)]
async fn blocking_conflict<T, F>(task: F) -> Result<T, Response>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(task).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(conflict(error.to_string())),
        Err(error) => Err(internal(error.to_string())),
    }
}

fn extension_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_string()
}

fn content_type(extension: &str) -> &'static str {
    match extension.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn bad_request(message: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "ok": false, "error": message.into() })),
    )
        .into_response()
}

fn conflict(message: impl Into<String>) -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({ "ok": false, "error": message.into() })),
    )
        .into_response()
}

fn internal(message: impl Into<String>) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "ok": false, "error": message.into() })),
    )
        .into_response()
}

const ADMIN_PAGE: &str = r##"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Kanalib Ingest</title>
<style>
:root{color-scheme:dark;--bg:#0d1117;--panel:#161b22;--line:#30363d;--text:#e6edf3;--dim:#8b949e;--accent:#2f81f7;--ok:#3fb950;--bad:#f85149}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:14px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}
header{display:flex;justify-content:space-between;align-items:center;padding:20px 28px;border-bottom:1px solid var(--line);gap:16px;flex-wrap:wrap}
h1{font-size:20px;margin:0}h2{font-size:15px;margin:0 0 12px}
main{padding:24px 28px;display:grid;gap:28px;max-width:1400px}
button{background:var(--panel);color:var(--text);border:1px solid var(--line);border-radius:6px;padding:6px 12px;cursor:pointer}
button:hover{border-color:var(--accent)}button.primary{background:var(--accent);border-color:var(--accent);color:#fff}
button.danger{color:var(--bad)}button:disabled{opacity:.45;cursor:not-allowed}
input,select{background:var(--panel);border:1px solid var(--line);color:var(--text);border-radius:6px;padding:6px 10px}
.pills{display:flex;gap:8px;flex-wrap:wrap;padding:0 28px 8px}
.pill{border:1px solid var(--line);border-radius:999px;padding:3px 10px;color:var(--dim);font-size:12px}
.pill b{color:var(--text);font-weight:600}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(260px,1fr));gap:14px}
.card{background:var(--panel);border:1px solid var(--line);border-radius:10px;overflow:hidden;display:flex;flex-direction:column}
.card img{width:100%;height:200px;object-fit:cover;background:#000;display:block}
.card .body{padding:10px 12px;display:grid;gap:6px}
.meta{color:var(--dim);font-size:12px}.tags{color:var(--dim);font-size:12px;max-height:42px;overflow:auto}
.row{display:flex;gap:8px;flex-wrap:wrap;align-items:center}
.actions{display:flex;gap:8px;padding:0 12px 12px}
.r18g{color:var(--bad);font-weight:600}
table{width:100%;border-collapse:collapse;background:var(--panel);border:1px solid var(--line);border-radius:8px;overflow:hidden}
th,td{text-align:left;padding:8px 10px;border-bottom:1px solid var(--line);vertical-align:top}
th{color:var(--dim);font-weight:500;font-size:12px}
tr:last-child td{border-bottom:0}
.empty{color:var(--dim);padding:20px;border:1px dashed var(--line);border-radius:10px;text-align:center}
.error{color:var(--bad);font-size:12px}
.form{display:flex;gap:8px;flex-wrap:wrap;margin-bottom:12px}
.form input[type=text]{min-width:220px;flex:1}
.diff{background:#0b0f14;border:1px solid var(--line);border-radius:8px;padding:10px;max-height:380px;overflow:auto;font:12px/1.5 ui-monospace,SFMono-Regular,Consolas,monospace;white-space:pre}
.diff .add{color:var(--ok)}.diff .remove{color:var(--bad)}.diff .context{color:var(--dim)}
#diff-panel{margin-top:16px}
</style>
</head>
<body>
<header>
  <div><h1>Kanalib Ingest</h1><div class="meta" id="rules-meta">loading…</div></div>
  <div class="row"><button id="reload-btn" class="primary">重载规则</button><span id="flash" class="meta"></span></div>
</header>
<div class="pills" id="pills"></div>
<main>
  <section>
    <h2>审核队列 <span class="meta" id="review-count"></span></h2>
    <div id="review" class="grid"></div>
  </section>
  <section>
    <h2>规则草稿</h2>
    <form class="form" id="proposal-form">
      <select id="kind">
        <option value="add_tag">拉黑标签</option>
        <option value="remove_tag">移除标签</option>
        <option value="add_user">拉黑作者</option>
        <option value="remove_user">移除作者</option>
      </select>
      <input type="text" id="value" placeholder="标签或 Pixiv user id" required>
      <input type="text" id="note" placeholder="备注（可选）">
      <button class="primary" type="submit">新建草稿</button>
    </form>
    <table>
      <thead><tr><th>#</th><th>操作</th><th>值</th><th>状态</th><th>备注 / 错误</th><th></th></tr></thead>
      <tbody id="proposals"></tbody>
    </table>
    <section id="diff-panel" hidden>
      <h2>草稿预览 <span class="meta" id="diff-summary"></span></h2>
      <div class="diff" id="diff-body"></div>
      <div class="row" style="margin-top:10px">
        <button class="primary" id="diff-apply">确认应用</button>
        <button id="diff-cancel">取消</button>
      </div>
    </section>
  </section>
</main>
<script>
const $ = (id) => document.getElementById(id);
const esc = (v) => String(v ?? "").replace(/[&<>"']/g, (c) => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
async function api(path, options) {
  const response = await fetch(path, options);
  const text = await response.text();
  let data;
  try { data = JSON.parse(text); } catch { data = { error: text }; }
  if (!response.ok) throw new Error(data.error || `HTTP ${response.status}`);
  return data;
}
async function post(path, body) {
  return api(path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body || {}) });
}
function flash(message, bad) {
  const node = $("flash");
  node.textContent = message;
  node.className = bad ? "error" : "meta";
  setTimeout(() => { node.textContent = ""; }, 6000);
}
async function load() {
  try {
    const [status, review, proposals] = await Promise.all([
      api("/admin/api/status"),
      api("/admin/api/review?limit=60"),
      api("/admin/api/proposals?state=pending"),
    ]);
    renderStatus(status);
    renderReview(review);
    renderProposals(proposals);
  } catch (error) {
    flash(error.message, true);
  }
}
function renderStatus(status) {
  const rules = status.rules;
  $("rules-meta").textContent = `${rules.path || "(default rules)"} · hash ${rules.hash.slice(0, 12)}…`;
  const queue = Object.entries(status.download_queue).map(([k, v]) => `${k} ${v}`).join(" · ") || "0";
  $("pills").innerHTML = [
    `版本 <b>${esc(rules.version)}</b>`,
    rules.loaded_at ? `上次加载 <b>${new Date(rules.loaded_at * 1000).toLocaleString()}</b>` : "",
    `review <b>${status.review.count}</b>`,
    `待应用草稿 <b>${rules.pending_proposals}</b>`,
    `下载队列 <b>${esc(queue)}</b>`,
    `推送待发 <b>${status.push_pending}</b>`,
    rules.tag_blacklist_empty ? '<span class="error">标签黑名单为空：不会按标签拦截</span>' : "",
    rules.last_error ? `<span class="error">上次重载失败：${esc(rules.last_error)}</span>` : "",
  ].filter(Boolean).map((text) => `<span class="pill">${text}</span>`).join("");
}
function renderReview(items) {
  $("review-count").textContent = items.length ? `(${items.length})` : "";
  $("review").innerHTML = items.length ? items.map((item) => `
    <article class="card">
      <a href="${esc(item.original)}" target="_blank" rel="noreferrer"><img loading="lazy" src="${esc(item.thumb || "")}" alt=""></a>
      <div class="body">
        <div><b>${esc(item.title || item.work_id)}</b></div>
        <div class="meta">${esc(item.source || "")} · ${esc(item.author_name || "")} · ${esc(item.author_id || "")} · ${item.width}×${item.height} · score ${item.score ?? "—"}</div>
        <div class="meta">${esc(item.reason || "")}</div>
        <div class="tags">${item.tags.map(esc).join(" · ")}</div>
        ${item.r18g ? '<div class="r18g">R-18G 终态拒绝</div>' : ""}
      </div>
      <div class="actions">
        <button ${item.allowable ? "" : "disabled"} onclick="decide('${esc(item.id)}','allow')">放行</button>
        <button class="danger" onclick="decide('${esc(item.id)}','reject')">拒绝</button>
        <button onclick="recheck('${esc(item.id)}')">按规则重跑</button>
        ${item.author_id ? `<button onclick="propose('add_user','${esc(item.author_id)}','review ${esc(item.id)}')">拉黑作者</button>` : ""}
        ${item.tags[0] ? `<button onclick="propose('add_tag','${esc(item.tags[0])}','review ${esc(item.id)}')">拉黑首个标签</button>` : ""}
      </div>
    </article>`).join("") : '<div class="empty">审核队列为空</div>';
}
function renderProposals(items) {
  $("proposals").innerHTML = items.length ? items.map((item) => `
    <tr>
      <td>${item.id}</td>
      <td>${esc(item.kind)}</td>
      <td>${esc(item.value || "")}</td>
      <td>${esc(item.state)}</td>
      <td>${esc(item.note || "")}${item.apply_error ? `<div class="error">${esc(item.apply_error)}</div>` : ""}</td>
      <td class="row">
        <button class="primary" onclick="previewProposal(${item.id})">预览</button>
        <button class="danger" onclick="rejectProposal(${item.id})">驳回</button>
      </td>
    </tr>`).join("") : '<tr><td colspan="6" class="empty">没有待应用草稿</td></tr>';
}
async function decide(id, action) {
  try { await post(`/admin/api/review/${id}/${action}`, { reviewer: "operator" }); flash(action === "allow" ? "已放行" : "已拒绝"); await load(); }
  catch (error) { flash(error.message, true); }
}
async function propose(kind, value, note) {
  try { await post("/admin/api/proposals", { kind, value, note }); flash("草稿已创建"); await load(); }
  catch (error) { flash(error.message, true); }
}
let previewId = null;
function hidePreview() {
  previewId = null;
  $("diff-panel").hidden = true;
  $("diff-body").innerHTML = "";
  $("diff-summary").textContent = "";
}
async function previewProposal(id) {
  try {
    const preview = await api(`/admin/api/proposals/${id}/preview`);
    previewId = id;
    $("diff-summary").textContent = preview.summary;
    const lines = preview.diff || [];
    $("diff-body").innerHTML = lines.length
      ? lines.map((line) => {
          const sign = line.kind === "add" ? "+" : line.kind === "remove" ? "-" : " ";
          return `<div class="${esc(line.kind)}">${esc(`${sign} ${line.text}`)}</div>`;
        }).join("")
      : '<div class="context">规则文件无需改动（草稿已满足）。</div>';
    $("diff-panel").hidden = false;
    $("diff-panel").scrollIntoView({ behavior: "smooth", block: "nearest" });
  }
  catch (error) { flash(error.message, true); }
}
async function confirmPreview() {
  if (previewId === null) return;
  try {
    const result = await post(`/admin/api/proposals/${previewId}/apply`, { reviewer: "operator" });
    flash(`已应用，规则 ${result.version}`);
    hidePreview();
    await load();
  } catch (error) { flash(error.message, true); }
}
$("diff-apply").addEventListener("click", confirmPreview);
$("diff-cancel").addEventListener("click", hidePreview);
async function recheck(id) {
  try {
    const result = await post(`/admin/api/items/${id}/recheck`, { force: false });
    flash(result.queued ? "已排队，下一轮决策生效" : "条目已在待决策队列");
    await load();
  } catch (error) { flash(error.message, true); }
}
async function rejectProposal(id) {
  try { await post(`/admin/api/proposals/${id}/reject`, { reviewer: "operator" }); flash("草稿已驳回"); await load(); }
  catch (error) { flash(error.message, true); }
}
$("reload-btn").addEventListener("click", async () => {
  try { const result = await post("/admin/api/rules/reload"); flash(result.reloaded ? `已重载 ${result.version}` : "规则未变化"); await load(); }
  catch (error) { flash(error.message, true); }
});
$("proposal-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  await propose($("kind").value, $("value").value, $("note").value);
  $("value").value = ""; $("note").value = "";
});
load();
setInterval(load, 15000);
</script>
</body>
</html>"##;
