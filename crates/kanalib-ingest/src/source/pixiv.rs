//! Pixiv 适配骨架：认证由外部注入，节流/错误分类/退避在此统一处理。
//! 传输层通过 trait 注入，测试不发任何真实请求，也不会记录令牌。

use std::{path::PathBuf, pin::Pin, sync::Arc, time::Duration};

use reqwest::header::RETRY_AFTER;
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::warn;

use super::{
    AiType, BoxFuture, ImageRef, Page, Rating, SourceAdapter, SourceAuthor, SourceCapabilities,
    SourceError, SourceErrorKind, SourceTag, SourceWork,
};
use crate::ingest::backoff_delay;

pub const API_BASE: &str = "https://app-api.pixiv.net";
pub const OAUTH_TOKEN_URL: &str = "https://oauth.secure.pixiv.net/auth/token";
pub const DEFAULT_USER_AGENT: &str = "Kanalib/0.1 (Pixiv ingest)";

#[derive(Debug, Clone)]
pub struct PixivResponse {
    pub status: u16,
    pub retry_after: Option<Duration>,
    pub body: serde_json::Value,
}

pub trait PixivTransport: Send + Sync {
    fn get<'a>(
        &self,
        path: &'a str,
        params: &'a [(&'a str, String)],
        auth: bool,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<PixivResponse, SourceError>> + Send + 'a>>;
}

#[derive(Debug, Clone, Default)]
pub struct ApiGateState {
    pub paused_until: i64,
    pub consecutive_429: u32,
}

#[derive(Debug, Clone)]
pub struct ApiGate {
    state: Arc<Mutex<ApiGateState>>,
}

impl Default for ApiGate {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiGate {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ApiGateState::default())),
        }
    }

    pub async fn snapshot(&self) -> ApiGateState {
        self.state.lock().await.clone()
    }

    pub async fn wait(&self) -> Result<(), SourceError> {
        let state = self.state.lock().await;
        let now = crate::db::now();
        if state.paused_until > now {
            return Err(SourceError::rate_limited(Some(Duration::from_secs(
                (state.paused_until - now) as u64,
            ))));
        }
        Ok(())
    }

    pub async fn record_error(&self, error: &SourceError) -> i64 {
        let mut state = self.state.lock().await;
        let delay = match error.kind {
            SourceErrorKind::RateLimited => {
                state.consecutive_429 += 1;
                error.retry_after.map_or_else(
                    || backoff_delay(state.consecutive_429, None),
                    |duration| duration.as_secs().max(1) as i64,
                )
            }
            SourceErrorKind::Auth => {
                state.paused_until = crate::db::now() + 900;
                900
            }
            _ => 0,
        };
        if delay > 0 {
            state.paused_until = state.paused_until.max(crate::db::now() + delay);
        }
        state.paused_until
    }

    pub async fn clear(&self) {
        let mut state = self.state.lock().await;
        state.paused_until = 0;
        state.consecutive_429 = 0;
    }
}

pub struct PixivSource {
    transport: Arc<dyn PixivTransport>,
    gate: ApiGate,
}

impl PixivSource {
    pub fn new(transport: Arc<dyn PixivTransport>) -> Self {
        Self {
            transport,
            gate: ApiGate::new(),
        }
    }

    pub fn with_gate(transport: Arc<dyn PixivTransport>, gate: ApiGate) -> Self {
        Self { transport, gate }
    }

    pub fn gate_state(&self) -> &ApiGate {
        &self.gate
    }

    async fn get_json(
        &self,
        path: &str,
        params: Vec<(&str, String)>,
        auth: bool,
    ) -> Result<serde_json::Value, SourceError> {
        self.gate.wait().await?;
        let response = self.transport.get(path, &params, auth).await?;
        if let Err(error) = classify_status(response.status, response.retry_after) {
            self.gate.record_error(&error).await;
            return Err(error);
        }
        Ok(response.body)
    }
}

impl SourceAdapter for PixivSource {
    fn id(&self) -> &'static str {
        "pixiv"
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            bookmarks: true,
            author_works: true,
            multiple_pages: true,
            // ugoira 需要 zip 解包与 GIF 合成；在实现前显式关闭，避免排入永远失败的任务。
            ugoira: false,
        }
    }

    fn fetch_work(&self, external_id: &str) -> BoxFuture<'_, Result<SourceWork, SourceError>> {
        let external_id = external_id.to_string();
        Box::pin(async move {
            let value = self
                .get_json(
                    "/v1/illust/detail",
                    vec![("illust_id", external_id.clone())],
                    true,
                )
                .await?;
            parse_work(value.get("illust").unwrap_or(&value))
        })
    }

    fn fetch_author(&self, external_id: &str) -> BoxFuture<'_, Result<SourceAuthor, SourceError>> {
        let external_id = external_id.to_string();
        Box::pin(async move {
            let value = self
                .get_json(
                    "/v1/user/detail",
                    vec![("user_id", external_id.clone())],
                    true,
                )
                .await?;
            let wire: WireAuthor =
                serde_json::from_value(value.get("user").cloned().unwrap_or(value.clone()))
                    .map_err(|error| {
                        SourceError::internal(format!("invalid pixiv author: {error}"))
                    })?;
            parse_author(&wire)
        })
    }

    fn list_author_works(
        &self,
        external_id: &str,
        cursor: Option<&str>,
    ) -> BoxFuture<'_, Result<Page<SourceWork>, SourceError>> {
        let external_id = external_id.to_string();
        let cursor = cursor.map(str::to_string);
        Box::pin(async move {
            let mut params = vec![("user_id", external_id)];
            if let Some(cursor) = cursor {
                params.push(("offset", cursor));
            }
            let value = self.get_json("/v1/user/illusts", params, true).await?;
            parse_work_page(&value)
        })
    }

    fn list_public_bookmarks(
        &self,
        user_id: &str,
        cursor: Option<&str>,
    ) -> BoxFuture<'_, Result<Page<SourceWork>, SourceError>> {
        let user_id = user_id.to_string();
        let cursor = cursor.map(str::to_string);
        Box::pin(async move {
            let mut params = vec![("user_id", user_id)];
            if let Some(cursor) = cursor {
                params.push(("offset", cursor));
            }
            let value = self
                .get_json("/v1/user/bookmarks/illust", params, true)
                .await?;
            parse_work_page(&value)
        })
    }

    fn resolve_image(
        &self,
        work: &SourceWork,
        page: u32,
    ) -> BoxFuture<'_, Result<ImageRef, SourceError>> {
        let external_id = work.external_id.clone();
        let is_ugoira = work.is_ugoira;
        Box::pin(async move {
            if is_ugoira {
                return Err(SourceError::internal(
                    "pixiv ugoira download is not implemented",
                ));
            }
            let mut image =
                ImageRef::new(format!("pixiv://illust/{external_id}/{page}")).ext("jpg");
            let value = self
                .get_json("/v1/illust/detail", vec![("illust_id", external_id)], true)
                .await?;
            let illust = value.get("illust").unwrap_or(&value);
            if let Some(pages) = illust.get("metadata").and_then(|meta| meta.get("pages"))
                && let Some(item) = pages.get(page as usize)
                && let Some(url) = item.get("image_urls").and_then(|urls| urls.get("large"))
                && let Some(url) = url.as_str()
            {
                image = ImageRef::new(url).ext("jpg");
            }
            Ok(image)
        })
    }
}

pub fn classify_status(status: u16, retry_after: Option<Duration>) -> Result<(), SourceError> {
    match status {
        401 | 403 => Err(SourceError::auth("pixiv access token rejected")),
        404 => Err(SourceError::not_found("pixiv resource not found")),
        429 => Err(SourceError::rate_limited(retry_after)),
        500..=599 => Err(SourceError::internal(format!(
            "pixiv server error {status}"
        ))),
        200..=299 => Ok(()),
        other => Err(SourceError::internal(format!(
            "unexpected pixiv status {other}"
        ))),
    }
}

#[derive(Debug, Deserialize, Default)]
struct WireWork {
    #[serde(default)]
    id: serde_json::Value,
    #[serde(default)]
    title: String,
    #[serde(default)]
    page_count: Option<u32>,
    #[serde(default)]
    page_count_alt: Option<u32>,
    #[serde(default)]
    posted_at: Option<i64>,
    #[serde(default)]
    create_date: Option<String>,
    #[serde(default)]
    x_restrict: Option<i64>,
    #[serde(default)]
    illust_ai_type: Option<i64>,
    #[serde(default)]
    sanity_level: Option<i64>,
    #[serde(default)]
    restriction_attributes: Option<Vec<String>>,
    #[serde(default)]
    is_ugoira: Option<bool>,
    #[serde(default)]
    type_: Option<String>,
    #[serde(default)]
    user: Option<WireAuthor>,
    #[serde(default)]
    author: Option<WireAuthor>,
    #[serde(default)]
    tags: Vec<WireTag>,
}

#[derive(Debug, Deserialize, Default)]
struct WireAuthor {
    #[serde(default)]
    id: serde_json::Value,
    #[serde(default)]
    name: String,
    #[serde(default)]
    profile_image_url: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct WireTag {
    #[serde(default)]
    name: String,
    #[serde(default)]
    score: Option<f64>,
    #[serde(default)]
    translated_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WirePage {
    works: Vec<serde_json::Value>,
    #[serde(default)]
    next_offset: Option<u64>,
}

fn parse_work(value: &serde_json::Value) -> Result<SourceWork, SourceError> {
    let wire: WireWork = serde_json::from_value(value.clone())
        .map_err(|error| SourceError::internal(format!("invalid pixiv work: {error}")))?;
    let external_id = wire
        .id
        .as_str()
        .map(str::to_string)
        .or_else(|| wire.id.as_u64().map(|id| id.to_string()))
        .ok_or_else(|| SourceError::internal("pixiv work has no id"))?;
    let author = wire
        .user
        .as_ref()
        .or(wire.author.as_ref())
        .map(parse_author)
        .transpose()?;
    let is_ugoira = wire.is_ugoira.unwrap_or(false) || wire.type_.as_deref() == Some("ugoira");
    Ok(SourceWork {
        external_id,
        author,
        title: wire.title,
        page_count: wire.page_count.or(wire.page_count_alt).unwrap_or(1).max(1),
        posted_at: wire
            .posted_at
            .or_else(|| wire.create_date.as_deref().and_then(chrono_like_timestamp)),
        rating: rating_from_x_restrict(wire.x_restrict),
        x_restrict: wire.x_restrict,
        ai: ai_from_illust_ai_type(wire.illust_ai_type),
        sanity_level: wire.sanity_level,
        restriction_attributes: wire
            .restriction_attributes
            .unwrap_or_default()
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect(),
        tags: wire
            .tags
            .into_iter()
            .map(|tag| SourceTag {
                name: tag.name,
                category: "pixiv".to_string(),
                score: tag.score.unwrap_or(0.0),
                translated_name: tag.translated_name.filter(|name| !name.trim().is_empty()),
            })
            .collect(),
        is_ugoira,
    })
}

/// Pixiv `x_restrict`：0 全年龄、1 R-18、2 R-18G；缺失/未知一律 Unknown。
pub(crate) fn rating_from_x_restrict(value: Option<i64>) -> Rating {
    match value {
        Some(0) => Rating::All,
        Some(1) => Rating::R18,
        Some(2) => Rating::R18G,
        _ => Rating::Unknown,
    }
}

/// Pixiv `illust_ai_type`：1 非 AI、2 AI、0/缺失未知。
pub(crate) fn ai_from_illust_ai_type(value: Option<i64>) -> AiType {
    match value {
        Some(1) => AiType::No,
        Some(2) => AiType::Ai,
        _ => AiType::Unknown,
    }
}

fn parse_author(wire: &WireAuthor) -> Result<SourceAuthor, SourceError> {
    let external_id = wire
        .id
        .as_str()
        .map(str::to_string)
        .or_else(|| wire.id.as_u64().map(|id| id.to_string()))
        .ok_or_else(|| SourceError::internal("pixiv author has no id"))?;
    Ok(SourceAuthor {
        external_id,
        name: wire.name.clone(),
        avatar_url: wire.profile_image_url.clone(),
    })
}

fn parse_work_page(value: &serde_json::Value) -> Result<Page<SourceWork>, SourceError> {
    let works = value
        .get("illusts")
        .or_else(|| value.get("works"))
        .cloned()
        .unwrap_or(serde_json::Value::Array(Vec::new()));
    let wire: WirePage = serde_json::from_value(serde_json::json!({ "works": works }))
        .map_err(|error| SourceError::internal(format!("invalid pixiv page: {error}")))?;
    let next_offset = value
        .get("next_offset")
        .and_then(serde_json::Value::as_u64)
        .or(wire.next_offset);
    Ok(Page {
        items: wire
            .works
            .iter()
            .map(parse_work)
            .collect::<Result<Vec<_>, _>>()?,
        next_cursor: next_offset.map(|offset| offset.to_string()),
    })
}

pub(crate) fn chrono_like_timestamp(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let digits =
        |start: usize, end: usize| -> Option<i64> { value[start..end].parse::<i64>().ok() };
    let (year, month, day) = (digits(0, 4)?, digits(5, 7)?, digits(8, 10)?);
    let (hour, minute, second) = (digits(11, 13)?, digits(14, 16)?, digits(17, 19)?);
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month_prime + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 访问令牌来源；刷新实现需要异步，测试仍可注入固定令牌。
pub trait PixivTokenProvider: Send + Sync {
    fn access_token<'a>(&'a self) -> BoxFuture<'a, Option<String>>;
}

impl PixivTokenProvider for String {
    fn access_token<'a>(&'a self) -> BoxFuture<'a, Option<String>> {
        let token = self.trim().to_string();
        Box::pin(async move { (!token.is_empty()).then_some(token) })
    }
}

#[derive(Debug)]
struct TokenState {
    access_token: String,
    refresh_token: String,
    expires_at: i64,
}

/// Pixiv OAuth refresh token 轮换：缓存 access token，过期前刷新并原子写回新 refresh token。
pub struct RefreshingTokenProvider {
    client: reqwest::Client,
    client_id: String,
    client_secret: String,
    user_agent: String,
    refresh_token_file: Option<PathBuf>,
    state: Mutex<TokenState>,
}

impl RefreshingTokenProvider {
    pub fn new(
        client: reqwest::Client,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        refresh_token: impl Into<String>,
        refresh_token_file: Option<PathBuf>,
        user_agent: Option<String>,
    ) -> Self {
        Self {
            client,
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            user_agent: user_agent.unwrap_or_else(|| DEFAULT_USER_AGENT.to_string()),
            refresh_token_file,
            state: Mutex::new(TokenState {
                access_token: String::new(),
                refresh_token: refresh_token.into(),
                expires_at: 0,
            }),
        }
    }

    async fn refresh(&self, state: &mut TokenState) -> Option<String> {
        let refresh_token = state.refresh_token.clone();
        let form = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("include_policy", "true"),
        ];
        let response = self
            .client
            .post(OAUTH_TOKEN_URL)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .form(&form)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                warn!(%error, "pixiv token refresh request failed");
                return None;
            }
        };
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            warn!(%status, "pixiv token refresh rejected");
            return None;
        }
        let envelope: OAuthEnvelope = match serde_json::from_str(&body) {
            Ok(envelope) => envelope,
            Err(error) => {
                warn!(%error, "pixiv token refresh returned invalid JSON");
                return None;
            }
        };
        if let Some(errors) = &envelope.errors {
            warn!(%errors, "pixiv token refresh returned errors");
        }
        let Some(response) = envelope.response else {
            warn!("pixiv token refresh response has no token payload");
            return None;
        };
        if response.access_token.trim().is_empty() {
            warn!("pixiv token refresh returned an empty access token");
            return None;
        }
        let expires_in = response.expires_in.unwrap_or(3_600).max(60);
        state.access_token = response.access_token;
        state.expires_at = crate::db::now().saturating_add(expires_in);
        if let Some(rotated) = response
            .refresh_token
            .filter(|token| !token.trim().is_empty())
            && rotated != state.refresh_token
        {
            state.refresh_token.clone_from(&rotated);
            if let Some(path) = &self.refresh_token_file
                && let Err(error) = crate::config::write_secret(path, &rotated)
            {
                warn!(
                    path = %path.display(),
                    %error,
                    "pixiv refresh token changed but could not be persisted"
                );
            }
        }
        Some(state.access_token.clone())
    }
}

impl PixivTokenProvider for RefreshingTokenProvider {
    fn access_token<'a>(&'a self) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let now = crate::db::now();
            let mut state = self.state.lock().await;
            if !state.access_token.is_empty() && state.expires_at > now.saturating_add(60) {
                return Some(state.access_token.clone());
            }
            self.refresh(&mut state).await
        })
    }
}

#[derive(Debug, Deserialize)]
struct OAuthEnvelope {
    #[serde(default)]
    response: Option<OAuthResponse>,
    #[serde(default)]
    errors: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct OAuthResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

pub struct ReqwestPixivTransport {
    client: reqwest::Client,
    token: Option<Arc<dyn PixivTokenProvider>>,
}

impl ReqwestPixivTransport {
    /// 不携带凭据的传输层；只能访问公开端点。
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            token: None,
        }
    }

    pub fn with_access_token(client: reqwest::Client, token: impl Into<String>) -> Self {
        Self {
            client,
            token: Some(Arc::new(token.into())),
        }
    }

    pub fn with_token_provider(
        client: reqwest::Client,
        provider: Arc<dyn PixivTokenProvider>,
    ) -> Self {
        Self {
            client,
            token: Some(provider),
        }
    }

    /// 用 refresh token 自动换取 access token；轮换后的令牌写回同一文件。
    pub fn with_refresh_token(
        client: reqwest::Client,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        refresh_token: impl Into<String>,
        refresh_token_file: Option<PathBuf>,
        user_agent: Option<String>,
    ) -> Self {
        let provider = RefreshingTokenProvider::new(
            client.clone(),
            client_id,
            client_secret,
            refresh_token,
            refresh_token_file,
            user_agent,
        );
        Self {
            client,
            token: Some(Arc::new(provider)),
        }
    }
}

impl PixivTransport for ReqwestPixivTransport {
    fn get<'a>(
        &self,
        path: &'a str,
        params: &'a [(&'a str, String)],
        auth: bool,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<PixivResponse, SourceError>> + Send + 'a>>
    {
        let client = self.client.clone();
        let provider = if auth { self.token.clone() } else { None };
        let url = format!("{API_BASE}{path}");
        let params: Vec<(String, String)> = params
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect();
        Box::pin(async move {
            let token = match provider {
                Some(provider) => provider.access_token().await,
                None => None,
            };
            if auth && token.is_none() {
                return Err(SourceError::auth("pixiv access token is not configured"));
            }
            let mut request = client.get(url).query(&params);
            if let Some(token) = token {
                request = request.bearer_auth(token);
            }
            let response = request.send().await.map_err(|error| {
                warn!("pixiv request failed");
                SourceError::network(format!("{error}"))
            })?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_secs);
            let body = response
                .json::<serde_json::Value>()
                .await
                .unwrap_or(serde_json::Value::Null);
            Ok(PixivResponse {
                status,
                retry_after,
                body,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_base_uses_the_app_api_host() {
        assert_eq!(API_BASE, "https://app-api.pixiv.net");
    }

    #[test]
    fn x_restrict_and_ai_fields_map_fail_closed() {
        let work = parse_work(&serde_json::json!({
            "id": 123,
            "title": "r18g",
            "x_restrict": 2,
            "illust_ai_type": 2,
            "sanity_level": 6,
            "restriction_attributes": ["r18g", " "],
            "page_count": 1,
            "tags": [{"name": "tag", "translated_name": "翻译"}]
        }))
        .expect("work");
        assert_eq!(work.rating, Rating::R18G);
        assert_eq!(work.x_restrict, Some(2));
        assert_eq!(work.ai, AiType::Ai);
        assert_eq!(work.sanity_level, Some(6));
        assert_eq!(work.restriction_attributes, vec!["r18g"]);
        assert_eq!(work.tags[0].translated_name.as_deref(), Some("翻译"));

        let missing = parse_work(&serde_json::json!({
            "id": 124,
            "title": "missing",
            "page_count": 1
        }))
        .expect("work");
        assert_eq!(missing.rating, Rating::Unknown);
        assert_eq!(missing.ai, AiType::Unknown);
        assert_eq!(missing.x_restrict, None);
    }

    #[tokio::test]
    async fn reqwest_transport_fails_closed_without_token() {
        let transport = ReqwestPixivTransport::new(reqwest::Client::new());
        let error = transport
            .get("/v1/illust/detail", &[], true)
            .await
            .expect_err("auth required");
        assert_eq!(error.kind, SourceErrorKind::Auth);
    }
}
