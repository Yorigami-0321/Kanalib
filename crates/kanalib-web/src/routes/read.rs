//! 公开只读 API：健康检查、索引、条目、作品、作者、收藏夹与标签。

use axum::{
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use kanalib_wall_model::{Capabilities, PageDto, valid_item_id};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    AppState, db,
    query::{Cursor, ItemFilter, ItemSort, SortKind, decode_cursor, encode_cursor},
    response::{api_error, db_unavailable, internal, json_response, not_found},
};

const DEFAULT_LIMIT: u32 = 60;
const MAX_LIMIT: u32 = 200;

#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl PageQuery {
    fn limit(&self) -> u32 {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }

    fn offset(&self) -> u32 {
        self.offset.unwrap_or(0).min(1_000_000)
    }
}

/// `/api/items` 与 `/api/index` 的查询参数。
#[derive(Debug, Default, Deserialize)]
pub struct ItemQuery {
    pub q: Option<String>,
    pub tag: Option<String>,
    pub group: Option<String>,
    pub rating: Option<String>,
    pub fav: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub seed: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<String>,
    pub offset: Option<String>,
}

impl ItemQuery {
    fn filter(&self) -> ItemFilter {
        ItemFilter {
            q: clean(&self.q),
            tag: clean(&self.tag),
            group: clean(&self.group),
            rating: clean(&self.rating).map(|rating| rating.to_ascii_uppercase()),
            fav: self.fav.as_deref().and_then(parse_fav),
        }
    }

    fn sort(&self) -> ItemSort {
        let kind = self
            .sort
            .as_deref()
            .map(SortKind::parse)
            .unwrap_or(SortKind::Updated);
        let ascending = matches!(
            self.order
                .as_deref()
                .map(str::trim)
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("asc")
        );
        let seed = self
            .seed
            .as_deref()
            .and_then(|raw| raw.parse::<i64>().ok())
            .unwrap_or(0);
        ItemSort {
            kind,
            ascending,
            seed,
        }
    }

    fn limit(&self) -> Result<u32, ApiError> {
        match self.limit.as_deref() {
            None => Ok(DEFAULT_LIMIT),
            Some(raw) => raw
                .parse::<u32>()
                .map(|value| value.clamp(1, MAX_LIMIT))
                .map_err(|_| ApiError::invalid_limit("limit must be an integer")),
        }
    }

    fn offset(&self) -> Result<u32, ApiError> {
        match self.offset.as_deref() {
            None => Ok(0),
            Some(raw) => raw
                .parse::<u32>()
                .map(|value| value.min(1_000_000))
                .map_err(|_| ApiError::invalid_limit("offset must be an integer")),
        }
    }

    fn cursor(&self, sort: &ItemSort, filter: &ItemFilter) -> Result<Option<Cursor>, ApiError> {
        let Some(raw) = self
            .cursor
            .as_deref()
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
        else {
            return Ok(None);
        };
        let cursor =
            decode_cursor(raw).ok_or_else(|| ApiError::invalid_cursor("cursor is invalid"))?;
        if cursor.sort != sort.kind || cursor.ascending != sort.ascending {
            return Err(ApiError::invalid_cursor(
                "cursor does not match sort or order",
            ));
        }
        if cursor.fingerprint != filter.fingerprint(sort.seed) {
            return Err(ApiError::invalid_cursor(
                "cursor does not match filters or seed",
            ));
        }
        Ok(Some(cursor))
    }

    fn page(&self) -> Result<(u32, u32, ItemSort, Option<Cursor>), ApiError> {
        let sort = self.sort();
        let filter = self.filter();
        let limit = self.limit()?;
        let offset = self.offset()?;
        let cursor = self.cursor(&sort, &filter)?;
        Ok((limit, offset, sort, cursor))
    }
}

fn clean(value: &Option<String>) -> Option<String> {
    value
        .as_ref()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn parse_fav(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

struct ApiError {
    code: &'static str,
    message: &'static str,
}

impl ApiError {
    fn invalid_limit(message: &'static str) -> Self {
        Self {
            code: "invalid_limit",
            message,
        }
    }

    fn invalid_cursor(message: &'static str) -> Self {
        Self {
            code: "invalid_cursor",
            message,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        api_error(StatusCode::BAD_REQUEST, self.code, self.message)
    }
}

#[derive(Serialize)]
struct IndexResponse {
    schema: u32,
    api_version: u32,
    count: u32,
    counts: db::IndexCounts,
    facets: db::IndexFacets,
    paths: serde_json::Value,
    capabilities: Capabilities,
}

enum DbOutcome<T> {
    Ready(T),
    OpenFailed(anyhow::Error),
    QueryFailed(anyhow::Error),
}

async fn run_db<T, F>(state: &AppState, query: F) -> Result<T, Box<Response>>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> anyhow::Result<T> + Send + 'static,
{
    let db_path = state.db_path.as_ref().clone();
    let outcome = tokio::task::spawn_blocking(move || match db::open_read(&db_path) {
        Ok(conn) => match query(&conn) {
            Ok(value) => DbOutcome::Ready(value),
            Err(error) => DbOutcome::QueryFailed(error),
        },
        Err(error) => DbOutcome::OpenFailed(error),
    })
    .await;

    match outcome {
        Ok(DbOutcome::Ready(value)) => Ok(value),
        Ok(DbOutcome::OpenFailed(error)) => {
            tracing::warn!(%error, "database open failed");
            Err(Box::new(db_unavailable()))
        }
        Ok(DbOutcome::QueryFailed(error)) => {
            tracing::error!(%error, "database query failed");
            Err(Box::new(internal()))
        }
        Err(error) => {
            tracing::error!(%error, "database task failed");
            Err(Box::new(internal()))
        }
    }
}

pub async fn health(State(state): State<AppState>) -> Response {
    let db_path = state.db_path.as_ref().clone();
    let media_root = state.media_root.as_ref().clone();
    let (db_ok, media_ok) =
        tokio::task::spawn_blocking(move || (db::open_read(&db_path).is_ok(), media_root.is_dir()))
            .await
            .unwrap_or((false, false));

    if db_ok {
        json_response(
            StatusCode::OK,
            json!({
                "ok": true,
                "db": true,
                "media_root": media_ok,
            }),
        )
    } else {
        json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "ok": false,
                "db": false,
                "media_root": media_ok,
            }),
        )
    }
}

pub async fn index(State(state): State<AppState>, Query(query): Query<ItemQuery>) -> Response {
    let filter = query.filter();
    let result = run_db(&state, move |conn| {
        let counts = db::index_counts(conn, &filter)?;
        let facets = db::index_facets_filtered(conn, &filter)?;
        Ok((counts, facets))
    })
    .await;

    match result {
        Ok((counts, facets)) => json_response(
            StatusCode::OK,
            IndexResponse {
                schema: db::SCHEMA_VERSION,
                api_version: 1,
                count: counts.items,
                counts,
                facets,
                paths: json!({
                    "t256": "/media/t256/",
                    "t512": "/media/t512/",
                    "t1024": "/media/t1024/",
                    "t2048": "/media/t2048/",
                }),
                capabilities: api_capabilities(),
            },
        ),
        Err(response) => *response,
    }
}

fn api_capabilities() -> Capabilities {
    Capabilities {
        source: "api".to_string(),
        server_search: true,
        server_filter: true,
        can_write: false,
        has_works: true,
        has_authors: true,
        has_collections: true,
        media_tiers: vec![
            "t256".to_string(),
            "t512".to_string(),
            "t1024".to_string(),
            "t2048".to_string(),
        ],
        // 公网 index 永不提供原图入口；原图只在 private listener 给桌面端。
        original_proxy: false,
        generated_at: None,
    }
}

pub async fn items(State(state): State<AppState>, Query(query): Query<ItemQuery>) -> Response {
    let (limit, offset, sort, cursor) = match query.page() {
        Ok(page) => page,
        Err(error) => return error.into_response(),
    };
    let filter = query.filter();
    let started = std::time::Instant::now();
    let result = run_db(&state, move |conn| {
        let page = db::search_items(conn, &filter, &sort, cursor.as_ref(), limit, offset)?;
        Ok(PageDto {
            items: page.items,
            next: page.next.as_ref().map(encode_cursor),
            // 计数由 /api/index 提供；分页端点只做 keyset 翻页，避免每页全量 COUNT。
            total: None,
        })
    })
    .await;
    let elapsed = started.elapsed();
    if elapsed > std::time::Duration::from_millis(200) {
        tracing::warn!(
            elapsed_ms = elapsed.as_millis() as u64,
            limit,
            "slow items query"
        );
    }

    match result {
        Ok(page) => json_response(StatusCode::OK, page),
        Err(response) => *response,
    }
}

pub async fn item(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !valid_item_id(&id) {
        return not_found();
    }
    let lookup = id.clone();
    let result = run_db(&state, move |conn| {
        let Some(item) = db::get_item(conn, &lookup)? else {
            return Ok(None);
        };
        let updated = db::item_updated_at(conn, &lookup)?.unwrap_or(0);
        Ok(Some((item, updated)))
    })
    .await;
    match result {
        Ok(Some((item, updated))) => {
            let mut response = json_response(StatusCode::OK, item);
            if let Ok(etag) = HeaderValue::from_str(&format!("W/\"{id}-{updated}-item\"")) {
                response.headers_mut().insert(header::ETAG, etag);
            }
            response
        }
        Ok(None) => not_found(),
        Err(response) => *response,
    }
}

/// 详情页前后条目；参数与 `/api/items` 一致，按同一筛选/排序定位。
pub async fn neighbors(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ItemQuery>,
) -> Response {
    if !valid_item_id(&id) {
        return not_found();
    }
    let filter = query.filter();
    let sort = query.sort();
    let result = run_db(&state, move |conn| {
        if !db::item_is_public(conn, &id)? {
            return Ok(None);
        }
        db::item_neighbors(conn, &id, &filter, &sort)
    })
    .await;
    match result {
        Ok(Some(neighbors)) => json_response(StatusCode::OK, neighbors),
        Ok(None) => not_found(),
        Err(response) => *response,
    }
}

pub async fn works(State(state): State<AppState>, Query(query): Query<PageQuery>) -> Response {
    let limit = query.limit();
    let offset = query.offset();
    let result = run_db(&state, move |conn| {
        let total = db::count_works(conn)?;
        let works = db::list_works(conn, limit, offset)?;
        Ok(PageDto {
            items: works,
            next: None,
            total: Some(total),
        })
    })
    .await;

    match result {
        Ok(page) => json_response(StatusCode::OK, page),
        Err(response) => *response,
    }
}

pub async fn work(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let result = run_db(&state, move |conn| db::get_work(conn, &id)).await;
    match result {
        Ok(Some(work)) => json_response(StatusCode::OK, work),
        Ok(None) => not_found(),
        Err(response) => *response,
    }
}

pub async fn authors(State(state): State<AppState>, Query(query): Query<PageQuery>) -> Response {
    let limit = query.limit();
    let offset = query.offset();
    let result = run_db(&state, move |conn| {
        let total = db::count_authors(conn)?;
        let authors = db::list_authors(conn, limit, offset)?;
        Ok(PageDto {
            items: authors,
            next: None,
            total: Some(total),
        })
    })
    .await;

    match result {
        Ok(page) => json_response(StatusCode::OK, page),
        Err(response) => *response,
    }
}

pub async fn author(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let result = run_db(&state, move |conn| db::get_author(conn, &id)).await;
    match result {
        Ok(Some(author)) => json_response(StatusCode::OK, author),
        Ok(None) => not_found(),
        Err(response) => *response,
    }
}

pub async fn author_works(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Response {
    let limit = query.limit();
    let offset = query.offset();
    let result = run_db(&state, move |conn| {
        let total = db::count_author_works(conn, &id)?;
        let works = db::list_author_works(conn, &id, limit, offset)?;
        Ok(PageDto {
            items: works,
            next: None,
            total: Some(total),
        })
    })
    .await;

    match result {
        Ok(page) => json_response(StatusCode::OK, page),
        Err(response) => *response,
    }
}

pub async fn collections(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Response {
    let limit = query.limit();
    let offset = query.offset();
    let result = run_db(&state, move |conn| {
        let total = db::count_collections(conn)?;
        let collections = db::list_collections(conn, limit, offset)?;
        Ok(PageDto {
            items: collections,
            next: None,
            total: Some(total),
        })
    })
    .await;

    match result {
        Ok(page) => json_response(StatusCode::OK, page),
        Err(response) => *response,
    }
}

pub async fn collection(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Response {
    let Ok(collection_id) = id.parse::<i64>() else {
        return not_found();
    };
    let limit = query.limit();
    let offset = query.offset();
    let result = run_db(&state, move |conn| {
        if !db::collection_exists(conn, collection_id)? {
            return Ok(None);
        }
        let total = db::count_collection_items(conn, collection_id)?;
        let items = db::list_collection_items(conn, collection_id, limit, offset)?;
        Ok(Some(PageDto {
            items,
            next: None,
            total: Some(total),
        }))
    })
    .await;

    match result {
        Ok(Some(page)) => json_response(StatusCode::OK, page),
        Ok(None) => not_found(),
        Err(response) => *response,
    }
}

pub async fn collection_works(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Response {
    let Ok(collection_id) = id.parse::<i64>() else {
        return not_found();
    };
    let limit = query.limit();
    let offset = query.offset();
    let result = run_db(&state, move |conn| {
        if !db::collection_exists(conn, collection_id)? {
            return Ok(None);
        }
        let total = db::count_collection_works(conn, collection_id)?;
        let works = db::list_collection_works(conn, collection_id, limit, offset)?;
        Ok(Some(PageDto {
            items: works,
            next: None,
            total: Some(total),
        }))
    })
    .await;

    match result {
        Ok(Some(page)) => json_response(StatusCode::OK, page),
        Ok(None) => not_found(),
        Err(response) => *response,
    }
}

pub async fn tags(State(state): State<AppState>, Query(query): Query<ItemQuery>) -> Response {
    let limit = match query.limit() {
        Ok(limit) => limit,
        Err(error) => return error.into_response(),
    };
    let filter = query.filter();
    let result = run_db(&state, move |conn| {
        let mut facets = db::index_facets_filtered(conn, &filter)?;
        facets.tags.truncate(limit as usize);
        Ok(facets.tags)
    })
    .await;
    match result {
        Ok(tags) => json_response(
            StatusCode::OK,
            PageDto::<db::FacetTag> {
                items: tags,
                next: None,
                total: None,
            },
        ),
        Err(response) => *response,
    }
}
