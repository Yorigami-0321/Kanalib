//! 纯转换：静态分片条目与入库记录 → 共用 DTO。无 IO、无 DB。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{AuthorDto, WallItemDto, WorkDto, WorkPageDto};

/// `index.json` 分片条目的反序列化形状。
///
/// 与 `kanalib-publish` 的 `manifest::Item` 对齐；这里不依赖发布器，避免
/// 契约 crate 反向依赖业务流程。
#[derive(Clone, Debug, Deserialize)]
pub struct StaticItem {
    pub id: String,
    pub w: u32,
    pub h: u32,
    pub n: String,
    pub rt: String,
    pub st: u8,
    pub fav: bool,
    pub tags: Vec<(String, String, f64)>,
    pub mtime: u64,
    #[serde(default)]
    pub g: Option<String>,
    #[serde(default)]
    pub ph: Option<String>,
    #[serde(default)]
    pub orig: Option<String>,
    #[serde(default)]
    pub sharp: Option<u16>,
}

/// 入库侧标签记录。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IngestTag {
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub score: f64,
}

/// 入库侧条目记录（对应 `items` 表与作品/作者摘要）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IngestItem {
    pub id: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub rating: String,
    #[serde(default)]
    pub star: u8,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub tags: Vec<IngestTag>,
    #[serde(default)]
    pub mtime: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orig: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharp: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// box → 网页的内部批次信封。
///
/// `source`/`rules_version` 属于批次元数据；条目本身复用 [`IngestItem`]，
/// 由 `decision` 与 `revision` 包一层。网页只接受 `decision = "allow"`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IngestBatch {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_version: Option<String>,
    pub items: Vec<IngestBatchItem>,
}

/// 批次内的一条 allow 记录。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IngestBatchItem {
    #[serde(default = "default_revision")]
    pub revision: i64,
    pub decision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(flatten)]
    pub item: IngestItem,
}

/// box → 网页的删除事件信封；只置 `deleted = 1`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IngestDelete {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_version: Option<String>,
    pub items: Vec<IngestDeleteItem>,
}

/// 删除事件中的一条；`revision` 低于网页当前值时忽略。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IngestDeleteItem {
    pub id: String,
    #[serde(default = "default_revision")]
    pub revision: i64,
}

fn default_revision() -> i64 {
    1
}

/// 入库侧作者记录。
#[derive(Clone, Debug, PartialEq)]
pub struct IngestAuthor {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    pub work_count: u32,
    pub item_count: u32,
    pub covers: Vec<String>,
}

/// 入库侧作品页记录。
#[derive(Clone, Debug, PartialEq)]
pub struct IngestPage {
    pub id: String,
    pub page: u32,
    pub width: u32,
    pub height: u32,
    pub sharp: u32,
    pub orig: Option<String>,
}

/// 入库侧作品记录。
#[derive(Clone, Debug, PartialEq)]
pub struct IngestWork {
    pub id: String,
    pub source: String,
    pub external_id: Option<String>,
    pub title: String,
    pub author: Option<IngestAuthor>,
    pub page_count: u32,
    pub posted_at: Option<u64>,
    pub status: String,
    pub tags: Vec<IngestTag>,
    pub pages: Vec<IngestPage>,
}

impl From<StaticItem> for WallItemDto {
    fn from(item: StaticItem) -> Self {
        Self {
            id: item.id,
            w: item.w,
            h: item.h,
            n: item.n,
            rt: normalize_rating(&item.rt),
            st: item.st.min(5),
            fav: item.fav,
            tags: static_tags(item.tags),
            mtime: item.mtime,
            g: normalize_text(item.g),
            ph: value_lowercase(item.ph),
            orig: normalize_extension(item.orig),
            sharp: item.sharp.map(u32::from),
            wid: None,
            pc: None,
            pg: None,
            au: None,
            auid: None,
            del: None,
            title: None,
        }
    }
}

impl From<IngestItem> for WallItemDto {
    fn from(item: IngestItem) -> Self {
        Self {
            id: item.id,
            w: item.width,
            h: item.height,
            n: item.name,
            rt: normalize_rating(&item.rating),
            st: item.star.min(5),
            fav: item.favorite,
            tags: ingest_tags(item.tags),
            mtime: item.mtime,
            g: normalize_text(item.group),
            ph: value_lowercase(item.phash),
            orig: normalize_extension(item.orig),
            sharp: item.sharp,
            wid: item.work_id,
            pc: item.page_count,
            pg: item.page,
            au: normalize_text(item.author),
            auid: item.author_id,
            del: item.deleted,
            title: normalize_text(item.title),
        }
    }
}

impl From<IngestAuthor> for AuthorDto {
    fn from(author: IngestAuthor) -> Self {
        Self {
            id: author.id,
            name: author.name,
            avatar: author.avatar,
            work_count: author.work_count,
            item_count: author.item_count,
            covers: author.covers,
        }
    }
}

impl From<IngestPage> for WorkPageDto {
    fn from(page: IngestPage) -> Self {
        Self {
            id: page.id,
            page: page.page,
            w: page.width,
            h: page.height,
            sharp: page.sharp,
            orig: normalize_extension(page.orig),
        }
    }
}

impl From<IngestWork> for WorkDto {
    fn from(work: IngestWork) -> Self {
        Self {
            id: work.id,
            source: work.source,
            external_id: work.external_id,
            title: work.title,
            author: work.author.map(AuthorDto::from),
            page_count: work.page_count,
            posted_at: work.posted_at,
            status: work.status,
            tags: ingest_tags(work.tags),
            pages: work.pages.into_iter().map(WorkPageDto::from).collect(),
        }
    }
}

/// 分级归一化：小写/空串/未知值统一回退 G。
pub fn normalize_rating(rating: &str) -> String {
    let upper = rating.trim().to_ascii_uppercase();
    match upper.as_str() {
        "G" | "S" | "Q" | "E" => upper,
        _ => "G".to_string(),
    }
}

/// 条目 id 必须是 40 或 64 位小写 hex。
pub fn valid_item_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64)
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn static_tags(tags: Vec<(String, String, f64)>) -> Vec<(String, String, f32)> {
    let mut seen = HashSet::new();
    tags.into_iter()
        .filter_map(|(name, category, score)| {
            let name = name.trim();
            if name.is_empty() || !seen.insert(name.to_string()) {
                return None;
            }
            Some((name.to_string(), category, normalize_score(score)))
        })
        .collect()
}

fn ingest_tags(tags: Vec<IngestTag>) -> Vec<(String, String, f32)> {
    let mut seen = HashSet::new();
    tags.into_iter()
        .filter_map(|tag| {
            let name = tag.name.trim();
            if name.is_empty() || !seen.insert(name.to_string()) {
                return None;
            }
            Some((name.to_string(), tag.category, normalize_score(tag.score)))
        })
        .collect()
}

fn normalize_score(score: f64) -> f32 {
    if !score.is_finite() {
        return 0.0;
    }
    ((score.clamp(0.0, 1.0) * 1000.0).round() / 1000.0) as f32
}

fn normalize_text(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn normalize_extension(extension: Option<String>) -> Option<String> {
    value_lowercase(extension.map(|value| value.trim_start_matches('.').to_string()))
}

fn value_lowercase(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_ascii_lowercase())
        .filter(|text| !text.is_empty())
}
