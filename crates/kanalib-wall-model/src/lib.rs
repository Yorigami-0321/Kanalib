//! 三端共用的条目契约（W0）。
//!
//! 形状唯一来源：`docs/backlog/web-dynamic-backend.md` §4.1。静态 `index.json`
//! 的既有字段原样保留，动态只追加可选字段；本 crate 不含 HTTP、SQLite、文件 IO，
//! 也不依赖桌面 `kanalib-core` 的 DB 类型。

use serde::{Deserialize, Serialize};

pub mod convert;

pub use convert::{
    IngestAuthor, IngestBatch, IngestBatchItem, IngestDelete, IngestDeleteItem, IngestItem,
    IngestPage, IngestTag, IngestWork, StaticItem, normalize_rating, valid_item_id,
};

/// 墙上的单条资产。短字段名与静态契约 §4 一一对应。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WallItemDto {
    /// 40/64 位小写 hex；同时是缩略图文件名主干。
    pub id: String,
    pub w: u32,
    pub h: u32,
    /// 展示名（basename）。
    pub n: String,
    /// 分级 G/S/Q/E。
    pub rt: String,
    /// 星级 0–5。
    pub st: u8,
    pub fav: bool,
    /// `[name, category, score]` 三元组。
    pub tags: Vec<(String, String, f32)>,
    /// UTC 秒。
    pub mtime: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub g: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ph: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orig: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharp: Option<u32>,

    /// 作品 id；静态条目没有该字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wid: Option<String>,
    /// 作品页数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pc: Option<u32>,
    /// 页序，从 0 开始。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pg: Option<u32>,
    /// 作者展示名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub au: Option<String>,
    /// 作者 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auid: Option<String>,
    /// 已删除标记；正常条目缺省。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub del: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// 作品详情。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WorkDto {
    pub id: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<AuthorDto>,
    pub page_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posted_at: Option<u64>,
    pub status: String,
    pub tags: Vec<(String, String, f32)>,
    pub pages: Vec<WorkPageDto>,
}

/// 作品内的一页。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WorkPageDto {
    pub id: String,
    /// 页序，从 0 开始。
    pub page: u32,
    pub w: u32,
    pub h: u32,
    pub sharp: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orig: Option<String>,
}

/// 作者详情。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AuthorDto {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    pub work_count: u32,
    pub item_count: u32,
    pub covers: Vec<String>,
}

/// 收藏夹/集合详情。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CollectionDto {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub item_count: u32,
    pub work_count: u32,
    pub covers: Vec<String>,
}

/// 统一分页；`next` 是服务端生成的 keyset 游标。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PageDto<T> {
    pub items: Vec<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
}

/// 目录能力声明；`schema` 始终为 1，静态/动态靠这里区分。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Capabilities {
    /// static | api
    pub source: String,
    pub server_search: bool,
    pub server_filter: bool,
    pub can_write: bool,
    pub has_works: bool,
    pub has_authors: bool,
    pub has_collections: bool,
    pub media_tiers: Vec<String>,
    /// 原图是否走签名回源。
    pub original_proxy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const ITEM_ID: &str = "0123456789abcdef0123456789abcdef01234567";

    fn key_set(value: &Value) -> Vec<String> {
        let mut keys = value
            .as_object()
            .expect("DTO must serialize to an object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        keys
    }

    fn static_sample() -> StaticItem {
        StaticItem {
            id: ITEM_ID.to_string(),
            w: 1200,
            h: 1600,
            n: "001.jpg".to_string(),
            rt: "s".to_string(),
            st: 4,
            fav: true,
            tags: vec![("1girl".to_string(), "general".to_string(), 0.9961)],
            mtime: 1_700_000_000,
            g: Some("  原创  ".to_string()),
            ph: Some("00FF1234ABCD5678".to_string()),
            orig: Some(".JPG".to_string()),
            sharp: Some(1024),
        }
    }

    fn ingest_sample() -> IngestItem {
        IngestItem {
            id: ITEM_ID.to_string(),
            width: 1200,
            height: 1600,
            name: "001.jpg".to_string(),
            rating: "s".to_string(),
            star: 4,
            favorite: true,
            tags: vec![IngestTag {
                name: " 1girl ".to_string(),
                category: "general".to_string(),
                score: 0.9961,
            }],
            mtime: 1_700_000_000,
            group: Some("  原创  ".to_string()),
            phash: Some("00FF1234ABCD5678".to_string()),
            orig: Some(".JPG".to_string()),
            sharp: Some(1024),
            work_id: None,
            page_count: None,
            page: None,
            author: None,
            author_id: None,
            deleted: None,
            title: None,
        }
    }

    fn minimal_static_sample() -> StaticItem {
        StaticItem {
            id: ITEM_ID.to_string(),
            w: 800,
            h: 600,
            n: "a.png".to_string(),
            rt: "G".to_string(),
            st: 0,
            fav: false,
            tags: Vec::new(),
            mtime: 1_700_000_000,
            g: None,
            ph: None,
            orig: None,
            sharp: None,
        }
    }

    #[test]
    fn static_and_dynamic_samples_share_one_shape() {
        let static_json = serde_json::to_value(WallItemDto::from(static_sample())).unwrap();
        let dynamic_json = serde_json::to_value(WallItemDto::from(ingest_sample())).unwrap();

        assert_eq!(key_set(&static_json), key_set(&dynamic_json));
        assert_eq!(static_json, dynamic_json);
        assert_eq!(
            key_set(&static_json),
            vec![
                "fav", "g", "h", "id", "mtime", "n", "orig", "ph", "rt", "sharp", "st", "tags",
                "w",
            ]
        );
    }

    #[test]
    fn optional_fields_are_omitted_when_absent() {
        let json = serde_json::to_value(WallItemDto::from(minimal_static_sample())).unwrap();
        assert_eq!(
            key_set(&json),
            vec!["fav", "h", "id", "mtime", "n", "rt", "st", "tags", "w"]
        );

        let page = PageDto {
            items: vec![WallItemDto::from(minimal_static_sample())],
            next: None,
            total: None,
        };
        assert_eq!(key_set(&serde_json::to_value(page).unwrap()), vec!["items"]);
    }

    #[test]
    fn dynamic_extension_fields_are_additive() {
        let mut item = ingest_sample();
        item.work_id = Some("work-1".to_string());
        item.page_count = Some(3);
        item.page = Some(1);
        item.author = Some("作者".to_string());
        item.author_id = Some("author-1".to_string());
        item.deleted = Some(true);
        item.title = Some("作品标题".to_string());

        let json = serde_json::to_value(WallItemDto::from(item)).unwrap();
        let keys = key_set(&json);
        for key in ["wid", "pc", "pg", "au", "auid", "del", "title"] {
            assert!(keys.iter().any(|entry| entry == key), "missing {key}");
        }
        for key in ["id", "w", "h", "n", "rt", "st", "fav", "tags", "mtime"] {
            assert!(keys.iter().any(|entry| entry == key), "missing {key}");
        }
    }

    #[test]
    fn tags_are_deduped_clamped_and_rounded() {
        let mut item = ingest_sample();
        item.tags = vec![
            IngestTag {
                name: "1girl".to_string(),
                category: "general".to_string(),
                score: 1.5,
            },
            IngestTag {
                name: " 1girl ".to_string(),
                category: "manual".to_string(),
                score: 0.2,
            },
            IngestTag {
                name: "   ".to_string(),
                category: "general".to_string(),
                score: 0.5,
            },
            IngestTag {
                name: "solo".to_string(),
                category: "general".to_string(),
                score: 0.12345,
            },
        ];

        assert_eq!(
            WallItemDto::from(item).tags,
            vec![
                ("1girl".to_string(), "general".to_string(), 1.0),
                ("solo".to_string(), "general".to_string(), 0.123),
            ]
        );
    }

    #[test]
    fn rating_falls_back_to_g() {
        for (input, expected) in [
            ("", "G"),
            ("g", "G"),
            ("s", "S"),
            ("Q", "Q"),
            ("e", "E"),
            ("x", "G"),
        ] {
            assert_eq!(normalize_rating(input), expected);
        }
    }

    #[test]
    fn item_id_is_40_or_64_lowercase_hex() {
        assert!(valid_item_id(&"a".repeat(40)));
        assert!(valid_item_id(&"0".repeat(64)));
        assert!(!valid_item_id(&"A".repeat(40)));
        assert!(!valid_item_id(&"g".repeat(40)));
        assert!(!valid_item_id(&"a".repeat(39)));
        assert!(!valid_item_id(&"a".repeat(41)));
        assert!(!valid_item_id(""));
    }

    #[test]
    fn work_and_author_records_convert_to_dto() {
        let work = IngestWork {
            id: "work-1".to_string(),
            source: "pixiv".to_string(),
            external_id: Some("123".to_string()),
            title: "作品标题".to_string(),
            author: Some(IngestAuthor {
                id: "author-1".to_string(),
                name: "作者".to_string(),
                avatar: Some("a.jpg".to_string()),
                work_count: 2,
                item_count: 3,
                covers: vec![ITEM_ID.to_string()],
            }),
            page_count: 2,
            posted_at: Some(1_700_000_000),
            status: "active".to_string(),
            tags: vec![IngestTag {
                name: "1girl".to_string(),
                category: "general".to_string(),
                score: 0.5,
            }],
            pages: vec![IngestPage {
                id: ITEM_ID.to_string(),
                page: 0,
                width: 1200,
                height: 1600,
                sharp: 1024,
                orig: Some(".PNG".to_string()),
            }],
        };

        let dto = WorkDto::from(work);
        assert_eq!(dto.id, "work-1");
        assert_eq!(dto.author.as_ref().unwrap().name, "作者");
        assert_eq!(dto.author.as_ref().unwrap().work_count, 2);
        assert_eq!(dto.tags[0].2, 0.5);
        assert_eq!(dto.pages[0].orig.as_deref(), Some("png"));
    }
}
