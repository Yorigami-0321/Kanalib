//! 无登录 Pixiv 公开来源：榜单、标签搜索、指定作者。
//!
//! 只使用网页匿名可见的端点；收藏、私密内容和 R-18 不在范围内。
//! 图像 URL 仍指向 `i.pximg.net`，由下载层统一带 `Referer`。

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Result, bail};
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, REFERER, RETRY_AFTER, USER_AGENT};
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::warn;

use super::{
    BoxFuture, ImageRef, Page, SourceAdapter, SourceAuthor, SourceCapabilities, SourceError,
    SourceTag, SourceWork,
    pixiv::{
        ai_from_illust_ai_type, chrono_like_timestamp, classify_status, rating_from_x_restrict,
    },
};
use crate::config::PublicSettings;

pub const WEB_BASE: &str = "https://www.pixiv.net";
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36";

const MAX_PAGES_PER_TARGET: usize = 10;
const MAX_ITEMS_PER_TARGET: usize = 100;
const DEFAULT_ITEMS_PER_TARGET: usize = 50;
const AUTHOR_PAGE_SIZE: usize = 48;
const CACHE_MAX: usize = 1024;

#[derive(Debug, Clone)]
pub struct PublicResponse {
    pub status: u16,
    pub retry_after: Option<Duration>,
    pub body: Value,
}

pub trait PublicTransport: Send + Sync {
    fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<PublicResponse, SourceError>>;
}

pub struct ReqwestPublicTransport {
    client: reqwest::Client,
    user_agent: String,
}

impl ReqwestPublicTransport {
    pub fn new(timeout: Duration, user_agent: impl Into<String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| anyhow::anyhow!("building public transport: {error}"))?;
        Ok(Self {
            client,
            user_agent: user_agent.into(),
        })
    }
}

impl PublicTransport for ReqwestPublicTransport {
    fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<PublicResponse, SourceError>> {
        let client = self.client.clone();
        let user_agent = self.user_agent.clone();
        Box::pin(async move {
            let response = client
                .get(url)
                .header(USER_AGENT, user_agent)
                .header(REFERER, WEB_BASE)
                .header(ACCEPT, "application/json, text/plain, */*")
                .header(ACCEPT_LANGUAGE, "en-US,en;q=0.9")
                .send()
                .await
                .map_err(|error| {
                    warn!("public pixiv request failed");
                    SourceError::network(format!("{error}"))
                })?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_secs);
            let body = response.json::<Value>().await.unwrap_or(Value::Null);
            Ok(PublicResponse {
                status,
                retry_after,
                body,
            })
        })
    }
}

#[derive(Debug, Clone)]
enum Target {
    Ranking {
        mode: String,
        content: String,
        pages: usize,
        limit: usize,
    },
    Tag {
        tag: String,
        order: String,
        pages: usize,
        limit: usize,
    },
    Author {
        user_id: String,
        pages: usize,
        limit: usize,
    },
}

impl Target {
    fn pages(&self) -> usize {
        match self {
            Self::Ranking { pages, .. } | Self::Tag { pages, .. } | Self::Author { pages, .. } => {
                *pages
            }
        }
    }
}

#[derive(Default)]
struct Cache {
    pages: HashMap<String, Vec<String>>,
    authors: HashMap<String, SourceAuthor>,
}

impl Cache {
    fn remember_author(&mut self, author: &SourceAuthor) {
        if self.authors.len() >= CACHE_MAX {
            self.authors.clear();
        }
        self.authors
            .insert(author.external_id.clone(), author.clone());
    }

    fn author(&self, external_id: &str) -> Option<SourceAuthor> {
        self.authors.get(external_id).cloned()
    }
}

pub struct PublicSource {
    transport: Arc<dyn PublicTransport>,
    targets: Vec<Target>,
    cache: Mutex<Cache>,
}

impl PublicSource {
    pub fn new(settings: &PublicSettings, timeout: Duration) -> Result<Self> {
        if !settings.enabled {
            bail!("source.public is not enabled");
        }
        if settings.targets.is_empty() {
            bail!("source.public.enabled requires at least one [[source.public.targets]]");
        }
        let mut targets = Vec::with_capacity(settings.targets.len());
        for target in &settings.targets {
            targets.push(build_target(target)?);
        }
        let user_agent = settings
            .user_agent
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_USER_AGENT);
        let transport = ReqwestPublicTransport::new(timeout, user_agent)?;
        Ok(Self {
            transport: Arc::new(transport),
            targets,
            cache: Mutex::new(Cache::default()),
        })
    }

    async fn get_json(&self, url: &str) -> Result<Value, SourceError> {
        let response = self.transport.get_json(url).await?;
        classify_status(response.status, response.retry_after)?;
        if response.body.get("error").and_then(Value::as_bool) == Some(true) {
            let message = response
                .body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("pixiv public endpoint returned an error");
            return Err(SourceError::internal(message.to_string()));
        }
        Ok(response.body)
    }

    async fn fetch_target_page(
        &self,
        target_index: usize,
        page: usize,
    ) -> Result<Vec<SourceWork>, SourceError> {
        let Some(target) = self.targets.get(target_index) else {
            return Ok(Vec::new());
        };
        match target {
            Target::Ranking {
                mode,
                content,
                limit,
                ..
            } => {
                let url = format!(
                    "{WEB_BASE}/ranking.php?mode={mode}&content={content}&p={page}&format=json"
                );
                let body = self.get_json(&url).await?;
                let mut works = parse_ranking(&body)?;
                works.truncate(*limit);
                Ok(works)
            }
            Target::Tag {
                tag, order, limit, ..
            } => {
                let encoded = encode_component(tag);
                let url = format!(
                    "{WEB_BASE}/ajax/search/artworks/{encoded}?word={encoded}&p={page}\
                     &s_mode=s_tag&order={order}&lang=en"
                );
                let body = self.get_json(&url).await?;
                let mut works = parse_search(&body);
                works.truncate(*limit);
                let mut cache = self.cache.lock().await;
                for work in &works {
                    if let Some(author) = &work.author {
                        cache.remember_author(author);
                    }
                }
                Ok(works)
            }
            Target::Author { user_id, limit, .. } => {
                let url = format!("{WEB_BASE}/ajax/user/{user_id}/profile/all?lang=en");
                let body = self.get_json(&url).await?;
                let offset = page.saturating_sub(1) * *limit;
                let works = parse_author_works(&body, offset, *limit);
                let mut cache = self.cache.lock().await;
                for work in &works {
                    if let Some(author) = &work.author {
                        cache.remember_author(author);
                    }
                }
                Ok(works)
            }
        }
    }

    async fn fetch_pages(&self, external_id: &str) -> Result<Vec<String>, SourceError> {
        if let Some(pages) = self.cache.lock().await.pages.get(external_id) {
            return Ok(pages.clone());
        }
        let url = format!("{WEB_BASE}/ajax/illust/{external_id}/pages?lang=en");
        let body = self.get_json(&url).await?;
        let pages = parse_pages(&body)?;
        let mut cache = self.cache.lock().await;
        if cache.pages.len() >= CACHE_MAX {
            cache.pages.clear();
        }
        cache.pages.insert(external_id.to_string(), pages.clone());
        Ok(pages)
    }

    async fn fetch_detail(&self, external_id: &str) -> Result<SourceWork, SourceError> {
        let url = format!("{WEB_BASE}/ajax/illust/{external_id}?lang=en");
        let body = self.get_json(&url).await?;
        let mut work = parse_detail(&body)?;
        if work
            .author
            .as_ref()
            .is_some_and(|author| author.avatar_url.is_none())
            && let Some(user_id) = work
                .author
                .as_ref()
                .map(|author| author.external_id.clone())
        {
            let cached = self.cache.lock().await.author(&user_id);
            if let Some(author) = cached {
                work.author = Some(author);
            }
        }
        Ok(work)
    }
}

impl SourceAdapter for PublicSource {
    fn id(&self) -> &'static str {
        "public"
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            bookmarks: false,
            author_works: true,
            multiple_pages: true,
            ugoira: false,
        }
    }

    fn fetch_work(&self, external_id: &str) -> BoxFuture<'_, Result<SourceWork, SourceError>> {
        let external_id = external_id.to_string();
        Box::pin(async move { self.fetch_detail(&external_id).await })
    }

    fn fetch_author(&self, external_id: &str) -> BoxFuture<'_, Result<SourceAuthor, SourceError>> {
        let external_id = external_id.to_string();
        Box::pin(async move {
            if let Some(author) = self.cache.lock().await.author(&external_id) {
                return Ok(author);
            }
            let url = format!("{WEB_BASE}/ajax/user/{external_id}/profile/all?lang=en");
            let body = self.get_json(&url).await?;
            let works = parse_author_works(&body, 0, 1);
            let author = works
                .first()
                .and_then(|work| work.author.clone())
                .unwrap_or(SourceAuthor {
                    external_id: external_id.clone(),
                    name: external_id,
                    avatar_url: None,
                });
            self.cache.lock().await.remember_author(&author);
            Ok(author)
        })
    }

    fn list_author_works(
        &self,
        external_id: &str,
        cursor: Option<&str>,
    ) -> BoxFuture<'_, Result<Page<SourceWork>, SourceError>> {
        let external_id = external_id.to_string();
        let cursor = cursor
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        Box::pin(async move {
            let url = format!("{WEB_BASE}/ajax/user/{external_id}/profile/all?lang=en");
            let body = self.get_json(&url).await?;
            let works = parse_author_works(&body, cursor, AUTHOR_PAGE_SIZE);
            Ok(Page {
                next_cursor: (works.len() == AUTHOR_PAGE_SIZE)
                    .then(|| (cursor + AUTHOR_PAGE_SIZE).to_string()),
                items: works,
            })
        })
    }

    fn list_public_bookmarks(
        &self,
        _user_id: &str,
        _cursor: Option<&str>,
    ) -> BoxFuture<'_, Result<Page<SourceWork>, SourceError>> {
        Box::pin(async {
            Err(SourceError::internal(
                "public source has no bookmarks; configure ranking/tag/author targets",
            ))
        })
    }

    fn list_works(
        &self,
        cursor: Option<&str>,
    ) -> BoxFuture<'_, Result<Page<SourceWork>, SourceError>> {
        let cursor = cursor.map(str::to_string);
        Box::pin(async move {
            let (target_index, page) = parse_cursor(cursor.as_deref())?;
            if target_index >= self.targets.len() {
                return Ok(Page::default());
            }
            let items = self.fetch_target_page(target_index, page).await?;
            let pages = self.targets[target_index].pages();
            let next_cursor = if page < pages {
                Some(format!("{target_index}:{}", page + 1))
            } else if target_index + 1 < self.targets.len() {
                Some(format!("{}:1", target_index + 1))
            } else {
                None
            };
            Ok(Page { items, next_cursor })
        })
    }

    fn resolve_image(
        &self,
        work: &SourceWork,
        page: u32,
    ) -> BoxFuture<'_, Result<ImageRef, SourceError>> {
        let external_id = work.external_id.clone();
        Box::pin(async move {
            let pages = self.fetch_pages(&external_id).await?;
            let Some(url) = pages.get(page as usize) else {
                return Err(SourceError::not_found(format!(
                    "public work {external_id} has no page {page}"
                )));
            };
            let mut image = ImageRef::new(url.clone());
            if let Some(ext) = image_ext(url) {
                image = image.ext(ext);
            }
            Ok(image)
        })
    }
}

fn build_target(target: &crate::config::PublicTarget) -> Result<Target> {
    let pages = target.pages.unwrap_or(1);
    if pages == 0 {
        bail!("source.public target pages must be >= 1");
    }
    let pages = pages.min(MAX_PAGES_PER_TARGET);
    let limit = target
        .limit
        .unwrap_or(DEFAULT_ITEMS_PER_TARGET)
        .clamp(1, MAX_ITEMS_PER_TARGET);
    match target.kind.trim().to_ascii_lowercase().as_str() {
        "ranking" => {
            let mode = target.mode.as_deref().unwrap_or("daily").trim().to_string();
            if !matches!(
                mode.as_str(),
                "daily" | "weekly" | "monthly" | "rookie" | "original" | "male" | "female"
            ) {
                bail!("unsupported ranking mode {mode}");
            }
            let content = target
                .content
                .as_deref()
                .unwrap_or("illust")
                .trim()
                .to_string();
            if !matches!(content.as_str(), "illust" | "manga") {
                bail!("unsupported ranking content {content}");
            }
            Ok(Target::Ranking {
                mode,
                content,
                pages,
                limit,
            })
        }
        "tag" => {
            let tag = target.tag.as_deref().unwrap_or_default().trim();
            if tag.is_empty() {
                bail!("tag target requires a non-empty tag");
            }
            let order = target
                .order
                .as_deref()
                .unwrap_or("date_d")
                .trim()
                .to_string();
            Ok(Target::Tag {
                tag: tag.to_string(),
                order,
                pages,
                limit,
            })
        }
        "author" => {
            let user_id = target.user_id.as_deref().unwrap_or_default().trim();
            if user_id.is_empty() || !user_id.chars().all(|ch| ch.is_ascii_digit()) {
                bail!("author target requires a numeric user_id");
            }
            Ok(Target::Author {
                user_id: user_id.to_string(),
                pages,
                limit,
            })
        }
        other => bail!("unsupported public target kind {other}"),
    }
}

fn parse_cursor(cursor: Option<&str>) -> Result<(usize, usize), SourceError> {
    let Some(cursor) = cursor else {
        return Ok((0, 1));
    };
    let Some((index, page)) = cursor.split_once(':') else {
        return Err(SourceError::internal(format!(
            "invalid public cursor {cursor}"
        )));
    };
    let index = index
        .parse::<usize>()
        .map_err(|_| SourceError::internal(format!("invalid public cursor {cursor}")))?;
    let page = page
        .parse::<usize>()
        .map_err(|_| SourceError::internal(format!("invalid public cursor {cursor}")))?;
    Ok((index, page.max(1)))
}

fn parse_ranking(body: &Value) -> Result<Vec<SourceWork>, SourceError> {
    let contents = body
        .get("contents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    contents
        .iter()
        .map(parse_ranking_item)
        .collect::<Result<Vec<_>, _>>()
}

fn parse_ranking_item(item: &Value) -> Result<SourceWork, SourceError> {
    let external_id = value_string(item.get("illust_id"))
        .ok_or_else(|| SourceError::internal("ranking item has no illust_id"))?;
    let author_id = value_string(item.get("user_id"));
    let author = author_id.map(|external_id| SourceAuthor {
        external_id,
        name: value_string(item.get("user_name")).unwrap_or_default(),
        avatar_url: value_string(item.get("profile_img")),
    });
    Ok(SourceWork {
        external_id,
        author,
        title: value_string(item.get("title")).unwrap_or_default(),
        page_count: value_u32(item.get("illust_page_count")).unwrap_or(1).max(1),
        posted_at: value_i64(item.get("illust_upload_timestamp")).or_else(|| {
            value_string(item.get("date"))
                .as_deref()
                .and_then(parse_japanese_date)
        }),
        rating: rating_from_x_restrict(value_i64(item.get("x_restrict"))),
        x_restrict: value_i64(item.get("x_restrict")),
        ai: ai_from_illust_ai_type(value_i64(item.get("ai_type"))),
        sanity_level: None,
        restriction_attributes: Vec::new(),
        tags: string_tags(item.get("tags")),
        is_ugoira: value_string(item.get("illust_type")).as_deref() == Some("2"),
    })
}

fn parse_search(body: &Value) -> Vec<SourceWork> {
    let data = body
        .get("body")
        .and_then(|body| body.get("illustManga"))
        .and_then(|value| value.get("data"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let translations = body
        .get("body")
        .and_then(|body| body.get("tagTranslation"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    data.iter()
        .filter_map(|item| parse_search_item(item, &translations).transpose())
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default()
}

fn parse_search_item(
    item: &Value,
    translations: &serde_json::Map<String, Value>,
) -> Result<Option<SourceWork>, SourceError> {
    let Some(external_id) = value_string(item.get("id")) else {
        return Ok(None);
    };
    let author = value_string(item.get("userId")).map(|external_id| SourceAuthor {
        external_id,
        name: value_string(item.get("userName")).unwrap_or_default(),
        avatar_url: value_string(item.get("profileImageUrl")),
    });
    let tags = item
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .map(|name| SourceTag {
                    name: name.to_string(),
                    category: "pixiv".to_string(),
                    score: 0.0,
                    translated_name: translations
                        .get(name)
                        .and_then(|value| value.get("en"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(SourceWork {
        external_id,
        author,
        title: value_string(item.get("title")).unwrap_or_default(),
        page_count: value_u32(item.get("pageCount")).unwrap_or(1).max(1),
        posted_at: value_string(item.get("createDate"))
            .as_deref()
            .and_then(chrono_like_timestamp),
        rating: rating_from_x_restrict(value_i64(item.get("xRestrict"))),
        x_restrict: value_i64(item.get("xRestrict")),
        ai: ai_from_illust_ai_type(value_i64(item.get("aiType"))),
        sanity_level: None,
        restriction_attributes: Vec::new(),
        tags,
        is_ugoira: value_i64(item.get("illustType")) == Some(2),
    }))
}

fn parse_author_works(body: &Value, offset: usize, limit: usize) -> Vec<SourceWork> {
    let Some(body) = body.get("body") else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for key in ["illusts", "manga"] {
        let Some(map) = body.get(key).and_then(Value::as_object) else {
            continue;
        };
        for (id, value) in map {
            items.push((id.clone(), value.clone()));
        }
    }
    items.sort_by(|left, right| {
        value_i64(Some(&Value::String(right.0.clone())))
            .unwrap_or(0)
            .cmp(&value_i64(Some(&Value::String(left.0.clone()))).unwrap_or(0))
    });
    items
        .into_iter()
        .skip(offset)
        .take(limit)
        .filter_map(|(id, value)| parse_author_item(&id, &value))
        .collect()
}

fn parse_author_item(id: &str, value: &Value) -> Option<SourceWork> {
    if let Some(title) = value.as_str() {
        return Some(SourceWork {
            external_id: id.to_string(),
            author: None,
            title: title.to_string(),
            page_count: 0,
            posted_at: None,
            rating: super::Rating::Unknown,
            x_restrict: None,
            ai: super::AiType::Unknown,
            sanity_level: None,
            restriction_attributes: Vec::new(),
            tags: Vec::new(),
            is_ugoira: false,
        });
    }
    let author = value_string(value.get("userId")).map(|external_id| SourceAuthor {
        external_id,
        name: value_string(value.get("userName")).unwrap_or_default(),
        avatar_url: value_string(value.get("profileImageUrl")),
    });
    Some(SourceWork {
        external_id: id.to_string(),
        author,
        title: value_string(value.get("title")).unwrap_or_default(),
        page_count: value_u32(value.get("pageCount")).unwrap_or(0),
        posted_at: value_string(value.get("createDate"))
            .as_deref()
            .and_then(chrono_like_timestamp),
        rating: rating_from_x_restrict(value_i64(value.get("xRestrict"))),
        x_restrict: value_i64(value.get("xRestrict")),
        ai: ai_from_illust_ai_type(value_i64(value.get("aiType"))),
        sanity_level: None,
        restriction_attributes: Vec::new(),
        tags: string_tags(value.get("tags")),
        is_ugoira: value_i64(value.get("illustType")) == Some(2),
    })
}

fn parse_detail(body: &Value) -> Result<SourceWork, SourceError> {
    let body = body
        .get("body")
        .ok_or_else(|| SourceError::internal("detail response has no body"))?;
    let external_id = value_string(body.get("id"))
        .or_else(|| value_string(body.get("illustId")))
        .ok_or_else(|| SourceError::internal("detail response has no id"))?;
    let author = value_string(body.get("userId")).map(|external_id| SourceAuthor {
        external_id,
        name: value_string(body.get("userName")).unwrap_or_default(),
        avatar_url: value_string(body.get("profileImageUrl")),
    });
    let tags = body
        .get("tags")
        .and_then(|tags| tags.get("tags"))
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| {
                    let name = value_string(tag.get("tag"))?;
                    Some(SourceTag {
                        name,
                        category: "pixiv".to_string(),
                        score: 0.0,
                        translated_name: tag
                            .get("translation")
                            .and_then(|value| value.get("en"))
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(SourceWork {
        external_id,
        author,
        title: value_string(body.get("title"))
            .or_else(|| value_string(body.get("illustTitle")))
            .unwrap_or_default(),
        page_count: value_u32(body.get("pageCount")).unwrap_or(1).max(1),
        posted_at: value_string(body.get("createDate"))
            .as_deref()
            .and_then(chrono_like_timestamp),
        rating: rating_from_x_restrict(value_i64(body.get("xRestrict"))),
        x_restrict: value_i64(body.get("xRestrict")),
        ai: ai_from_illust_ai_type(value_i64(body.get("aiType"))),
        sanity_level: None,
        restriction_attributes: Vec::new(),
        tags,
        is_ugoira: value_i64(body.get("illustType")) == Some(2),
    })
}

fn parse_pages(body: &Value) -> Result<Vec<String>, SourceError> {
    let pages = body
        .get("body")
        .and_then(Value::as_array)
        .ok_or_else(|| SourceError::internal("pages response has no body"))?;
    pages
        .iter()
        .enumerate()
        .map(|(index, page)| {
            page.get("urls")
                .and_then(|urls| urls.get("original"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    SourceError::internal(format!(
                        "pages response has no original for page {index}"
                    ))
                })
        })
        .collect()
}

fn string_tags(value: Option<&Value>) -> Vec<SourceTag> {
    value
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .map(|name| SourceTag {
                    name: name.to_string(),
                    category: "pixiv".to_string(),
                    score: 0.0,
                    translated_name: None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn value_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn value_i64(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(value) => value.as_i64(),
        Value::String(value) => value.parse::<i64>().ok(),
        _ => None,
    }
}

fn value_u32(value: Option<&Value>) -> Option<u32> {
    value_i64(value).and_then(|value| u32::try_from(value).ok())
}

fn parse_japanese_date(value: &str) -> Option<i64> {
    let value = value.trim();
    let (date, time) = value.split_once(' ')?;
    let date = date.trim_end_matches('日');
    let mut parts = date.split('年');
    let year = parts.next()?.parse::<i64>().ok()?;
    let rest = parts.next()?;
    let (month, day) = rest.split_once('月')?;
    let month = month.parse::<i64>().ok()?;
    let day = day.parse::<i64>().ok()?;
    let mut clock = time.split(':');
    let hour = clock.next()?.parse::<i64>().ok()?;
    let minute = clock.next()?.parse::<i64>().ok()?;
    let second = clock.next().unwrap_or("0").parse::<i64>().ok()?;
    Some(
        super::pixiv::days_from_civil(year, month, day) * 86_400
            + hour * 3_600
            + minute * 60
            + second,
    )
}

fn image_ext(url: &str) -> Option<String> {
    let path = url.split('?').next().unwrap_or(url);
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp").then_some(ext)
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking_and_search_items_parse() {
        let ranking = serde_json::json!({
            "contents": [{
                "illust_id": 152301042,
                "title": "rank title",
                "user_id": "743604",
                "user_name": "author",
                "profile_img": "https://i.pximg.net/avatar.jpg",
                "illust_page_count": "2",
                "illust_type": "0",
                "tags": ["a", "b"],
                "illust_upload_timestamp": 1_700_000_000
            }]
        });
        let works = parse_ranking(&ranking).unwrap();
        assert_eq!(works[0].external_id, "152301042");
        assert_eq!(works[0].page_count, 2);
        assert_eq!(works[0].tags.len(), 2);
        assert_eq!(works[0].posted_at, Some(1_700_000_000));

        let search = serde_json::json!({
            "error": false,
            "body": {
                "illustManga": {"data": [{
                    "id": "152402991",
                    "title": "search title",
                    "userId": "103600511",
                    "userName": "author",
                    "profileImageUrl": "https://i.pximg.net/avatar.jpg",
                    "pageCount": 1,
                    "illustType": 0,
                    "xRestrict": 0,
                    "aiType": 1,
                    "tags": ["花"],
                    "createDate": "2026-11-23T18:23:02+09:00"
                }]},
                "tagTranslation": {"花": {"en": "flower"}}
            }
        });
        let works = parse_search(&search);
        assert_eq!(works[0].external_id, "152402991");
        assert_eq!(works[0].tags[0].translated_name.as_deref(), Some("flower"));
        assert_eq!(works[0].ai.as_str(), "no");
    }

    #[test]
    fn detail_and_pages_parse() {
        let detail = serde_json::json!({
            "error": false,
            "body": {
                "id": "152294495",
                "title": "detail title",
                "userId": "59688623",
                "userName": "author",
                "pageCount": 2,
                "illustType": 0,
                "xRestrict": 0,
                "aiType": 1,
                "createDate": "2026-11-20T15:11:00+00:00",
                "tags": {"tags": [{"tag": "花", "translation": {"en": "flower"}}]}
            }
        });
        let work = parse_detail(&detail).unwrap();
        assert_eq!(work.external_id, "152294495");
        assert_eq!(work.page_count, 2);
        assert_eq!(work.tags[0].translated_name.as_deref(), Some("flower"));
        assert_eq!(work.rating, super::super::Rating::All);

        let pages = serde_json::json!({
            "error": false,
            "body": [
                {"urls": {"original": "https://i.pximg.net/img-original/a_p0.jpg"}},
                {"urls": {"original": "https://i.pximg.net/img-original/a_p1.png"}}
            ]
        });
        let pages = parse_pages(&pages).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(image_ext(&pages[1]).as_deref(), Some("png"));
    }

    #[test]
    fn cursor_advances_across_targets() {
        assert_eq!(parse_cursor(None).unwrap(), (0, 1));
        assert_eq!(parse_cursor(Some("1:2")).unwrap(), (1, 2));
        assert!(parse_cursor(Some("bad")).is_err());
    }
}
