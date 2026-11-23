//! 来源适配边界：主流程只依赖 trait，不依赖具体站点。

use std::{future::Future, pin::Pin, time::Duration};

use serde::{Deserialize, Serialize};

pub mod mock;
pub mod pixiv;
pub mod public;

use crate::db::SourceAuthorRow;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Pixiv 分级：`x_restrict` 的四态映射。
///
/// 缺失或未知值必须落到 [`Rating::Unknown`]，不能折叠成全年龄。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rating {
    All,
    R18,
    R18G,
    #[default]
    Unknown,
}

impl Rating {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::R18 => "r18",
            Self::R18G => "r18g",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "all" | "g" | "general" => Self::All,
            "r18" | "r-18" | "e" => Self::R18,
            "r18g" | "r-18g" => Self::R18G,
            _ => Self::Unknown,
        }
    }
}

impl std::fmt::Display for Rating {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Pixiv `illust_ai_type` 的三态；`0`/缺失都是未知，不能当成非 AI。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiType {
    No,
    Ai,
    #[default]
    Unknown,
}

impl AiType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::No => "no",
            Self::Ai => "ai",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "no" | "false" | "1" => Self::No,
            "ai" | "true" | "2" => Self::Ai,
            _ => Self::Unknown,
        }
    }
}

impl std::fmt::Display for AiType {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceCapabilities {
    pub bookmarks: bool,
    pub author_works: bool,
    pub multiple_pages: bool,
    pub ugoira: bool,
}

impl Default for SourceCapabilities {
    fn default() -> Self {
        Self {
            bookmarks: true,
            author_works: true,
            multiple_pages: false,
            ugoira: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceWork {
    pub external_id: String,
    pub author: Option<SourceAuthor>,
    pub title: String,
    pub page_count: u32,
    pub posted_at: Option<i64>,
    pub rating: Rating,
    /// 来源原始 `x_restrict`；`None` 表示来源没有返回该字段。
    pub x_restrict: Option<i64>,
    pub ai: AiType,
    pub sanity_level: Option<i64>,
    pub restriction_attributes: Vec<String>,
    pub tags: Vec<SourceTag>,
    pub is_ugoira: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceAuthor {
    pub external_id: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceTag {
    pub name: String,
    pub category: String,
    pub score: f64,
    pub translated_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageRef {
    pub url: String,
    pub ext: Option<String>,
    /// 来源侧图片字节数提示，仅用于校验参考，不作为是否完整的唯一依据。
    pub expected_bytes: Option<u64>,
}

impl ImageRef {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ext: None,
            expected_bytes: None,
        }
    }

    pub fn ext(mut self, ext: impl Into<String>) -> Self {
        self.ext = Some(ext.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            next_cursor: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceErrorKind {
    Auth,
    NotFound,
    RateLimited,
    Network,
    Internal,
}

impl SourceErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::NotFound => "not_found",
            Self::RateLimited => "rate_limited",
            Self::Network => "network",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    pub kind: SourceErrorKind,
    pub message: String,
    pub retry_after: Option<Duration>,
}

impl SourceError {
    pub fn new(kind: SourceErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn retry_after(mut self, retry_after: Duration) -> Self {
        self.retry_after = Some(retry_after);
        self
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(SourceErrorKind::NotFound, message)
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(SourceErrorKind::Auth, message)
    }

    pub fn rate_limited(retry_after: Option<Duration>) -> Self {
        let mut error = Self::new(SourceErrorKind::RateLimited, "source rate limit");
        error.retry_after = retry_after;
        error
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(SourceErrorKind::Network, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(SourceErrorKind::Internal, message)
    }
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}

impl std::error::Error for SourceError {}

pub trait SourceAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn capabilities(&self) -> SourceCapabilities;
    fn fetch_work(&self, external_id: &str) -> BoxFuture<'_, Result<SourceWork, SourceError>>;
    fn fetch_author(&self, external_id: &str) -> BoxFuture<'_, Result<SourceAuthor, SourceError>>;
    fn list_author_works(
        &self,
        external_id: &str,
        cursor: Option<&str>,
    ) -> BoxFuture<'_, Result<Page<SourceWork>, SourceError>>;
    fn list_public_bookmarks(
        &self,
        user_id: &str,
        cursor: Option<&str>,
    ) -> BoxFuture<'_, Result<Page<SourceWork>, SourceError>>;
    /// 匿名发现流：榜单、标签搜索或作者作品。默认来源没有这条流。
    fn list_works(
        &self,
        _cursor: Option<&str>,
    ) -> BoxFuture<'_, Result<Page<SourceWork>, SourceError>> {
        Box::pin(async { Ok(Page::default()) })
    }
    fn resolve_image(
        &self,
        work: &SourceWork,
        page: u32,
    ) -> BoxFuture<'_, Result<ImageRef, SourceError>>;
}

pub fn author_row(source: &str, author: &SourceAuthor) -> SourceAuthorRow {
    SourceAuthorRow {
        source: source.to_string(),
        external_id: author.external_id.clone(),
        name: author.name.clone(),
        avatar_url: author.avatar_url.clone(),
    }
}
