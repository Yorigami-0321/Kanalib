//! 导出契约 v1 的 DTO 与纯函数：id 规则（§5）、item 组装（§4）、facets 截断（§2）。
//!
//! 这里不碰文件系统也不碰数据库，方便对 id 规则、字段归一化和截断排序做单测。

use std::collections::HashMap;

use kanalib_core::model::Asset;
use serde::{Deserialize, Serialize};

/// 契约版本；加字段不升版，删字段/改语义必须升版。
pub const SCHEMA_VERSION: u32 = 1;
/// 每片条数（契约 §2/§3）。
pub const DEFAULT_SHARD_SIZE: usize = 5000;
/// facets.tags 上限（契约 §2）。
pub const MAX_FACET_TAGS: usize = 2000;
/// facets.groups 上限（契约 §2）。
pub const MAX_FACET_GROUPS: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShardRef {
    pub i: usize,
    pub path: String,
    pub count: usize,
}

/// 档位前缀；契约要求 t1024/t2048 未产出时也照常写出该键。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paths {
    pub t256: String,
    pub t512: String,
    pub t1024: String,
    pub t2048: String,
    pub original: String,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            t256: "t256/".to_string(),
            t512: "t512/".to_string(),
            t1024: "t1024/".to_string(),
            t2048: "t2048/".to_string(),
            original: "o/".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacetTag {
    pub name: String,
    pub category: String,
    pub count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facets {
    pub tags: Vec<FacetTag>,
    pub groups: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub schema: u32,
    pub generated_at: String,
    pub count: usize,
    pub shard_size: usize,
    pub shards: Vec<ShardRef>,
    pub facets: Facets,
    pub paths: Paths,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shard {
    pub schema: u32,
    pub index: usize,
    pub items: Vec<Item>,
}

/// 分片里的单条资产。字段名与契约 §4 一一对应（短名换首包体积）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// sha1（40）→ blake3（64）→ blake3(path_key::encode(path))，只允许 [0-9a-z]。
    pub id: String,
    pub w: u32,
    pub h: u32,
    /// 展示名：basename，不含目录。
    pub n: String,
    /// 分级 G/S/Q/E。
    pub rt: String,
    /// 分组原值；空/缺失表示未分组，不写该键。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub g: Option<String>,
    pub st: u8,
    pub fav: bool,
    /// [name, category, score] 三元组。
    pub tags: Vec<(String, String, f64)>,
    /// 16 位小写 hex pHash；缺失不出现。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ph: Option<String>,
    /// UTC 秒。
    pub mtime: u64,
    /// 原图小写扩展名（无点）；缺省表示无原图。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orig: Option<String>,
    /// 本条目可用的最高锐利档长边：0 = 无锐利档，1024/2048 = 有；缺省表示旧数据未声明（前端按未知处理）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharp: Option<u16>,
}

/// 计算资产 id（契约 §5）。只接受合法的小写 hex，非法值降级到下一个来源。
pub fn asset_id(asset: &Asset) -> String {
    if let Some(id) = asset
        .sha1
        .as_deref()
        .and_then(|value| valid_hex_id(value, 40))
    {
        return id;
    }
    if let Some(id) = asset
        .blake3
        .as_deref()
        .and_then(|value| valid_hex_id(value, 64))
    {
        return id;
    }
    let encoded = kanalib_core::path_key::encode(&asset.path);
    blake3::hash(encoded.as_bytes()).to_hex().to_string()
}

/// len 位、只含 [0-9a-f] 的 id；返回原样小写副本。
fn valid_hex_id(value: &str, len: usize) -> Option<String> {
    if value.len() != len {
        return None;
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    Some(value.to_string())
}

/// 分级归一化：库内存小写（g/s/q/e）或空串，契约要求大写且缺失回退 G。
pub fn normalize_rating(rating: &str) -> String {
    let upper = rating.trim().to_ascii_uppercase();
    match upper.as_str() {
        "G" | "S" | "Q" | "E" => upper,
        _ => "G".to_string(),
    }
}

/// 组装分片条目；orig 由调用方按实际导出的原图补齐。
pub fn build_item(asset: &Asset) -> Item {
    let name = asset
        .path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "image".to_string());
    let group = asset.group.trim();
    Item {
        id: asset_id(asset),
        w: asset.width,
        h: asset.height,
        n: name,
        rt: normalize_rating(&asset.rating),
        g: (!group.is_empty()).then(|| group.to_string()),
        st: asset.star.min(5),
        fav: asset.favorite,
        tags: tag_entries(asset),
        ph: asset.phash.map(|value| format!("{value:016x}")),
        mtime: asset.mtime,
        orig: None,
        sharp: None,
    }
}

/// 自动标签 + 人工标签，按名称去重（自动在先），score 保留 3 位小数并夹到 0-1。
pub fn tag_entries(asset: &Asset) -> Vec<(String, String, f64)> {
    let mut seen = std::collections::HashSet::new();
    let mut entries = Vec::new();
    for hit in asset.tags.iter().chain(asset.manual_tags.iter()) {
        let name = hit.tag.trim();
        if name.is_empty() || !seen.insert(name.to_string()) {
            continue;
        }
        let score = (f64::from(hit.score).clamp(0.0, 1.0) * 1000.0).round() / 1000.0;
        entries.push((name.to_string(), hit.category.clone(), score));
    }
    entries
}

/// 标签计数：name -> (category, 出现条目数)。
#[derive(Clone, Debug)]
pub struct TagCount {
    pub category: String,
    pub count: usize,
}

pub fn count_tags(counts: &mut HashMap<String, TagCount>, item: &Item) {
    for (name, category, _) in &item.tags {
        let entry = counts.entry(name.clone()).or_insert_with(|| TagCount {
            category: category.clone(),
            count: 0,
        });
        entry.count += 1;
    }
}

pub fn count_group(counts: &mut HashMap<String, usize>, item: &Item) {
    if let Some(group) = &item.g {
        *counts.entry(group.clone()).or_insert(0) += 1;
    }
}

/// count 降序、同 count 按名称升序，截断到 max。
pub fn build_tag_facets(counts: &HashMap<String, TagCount>, max: usize) -> Vec<FacetTag> {
    let mut list = counts
        .iter()
        .map(|(name, entry)| FacetTag {
            name: name.clone(),
            category: entry.category.clone(),
            count: entry.count,
        })
        .collect::<Vec<_>>();
    list.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    list.truncate(max);
    list
}

/// count 降序、同 count 按名称升序，截断到 max。
pub fn build_group_facets(counts: &HashMap<String, usize>, max: usize) -> Vec<String> {
    let mut list = counts
        .iter()
        .map(|(name, count)| (name.clone(), *count))
        .collect::<Vec<_>>();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    list.truncate(max);
    list.into_iter().map(|(name, _)| name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kanalib_core::model::TagHit;
    use std::path::PathBuf;

    fn tag(name: &str, category: &str, score: f32) -> TagHit {
        TagHit {
            tag: name.to_string(),
            category: category.to_string(),
            score,
            vocab_index: None,
        }
    }

    fn asset(name: &str) -> Asset {
        Asset {
            path: PathBuf::from("C:\\lib\\").join(name),
            library_root: PathBuf::from("C:\\lib"),
            thumb: None,
            tile_thumb: None,
            width: 800,
            height: 600,
            group: String::new(),
            rating: String::new(),
            favorite: false,
            star: 0,
            tags: Vec::new(),
            manual_tags: Vec::new(),
            tagged: false,
            tag_attempts: 0,
            error: None,
            damage: None,
            sha1: None,
            blake3: None,
            phash: None,
            bytes: 1024,
            mtime: 1_700_000_000,
            captured_at: None,
            metadata_json: String::new(),
        }
    }

    #[test]
    fn id_prefers_sha1_then_blake3_then_path_hash() {
        let sha1 = "0123456789abcdef0123456789abcdef01234567";
        let blake3 = "9".repeat(64);

        let mut with_both = asset("a.png");
        with_both.sha1 = Some(sha1.to_string());
        with_both.blake3 = Some(blake3.clone());
        assert_eq!(asset_id(&with_both), sha1);

        let mut with_blake3 = asset("a.png");
        with_blake3.blake3 = Some(blake3.clone());
        assert_eq!(asset_id(&with_blake3), blake3);

        let id = asset_id(&asset("a.png"));
        assert_eq!(id.len(), 64);
        assert!(
            id.bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        // 同一路径跨次发布稳定；不同路径不同。
        assert_eq!(asset_id(&asset("a.png")), id);
        assert_ne!(asset_id(&asset("b.png")), id);
    }

    #[test]
    fn invalid_hashes_fall_through_to_next_source() {
        let mut value = asset("a.png");
        value.sha1 = Some("XYZ".to_string()); // 长度与字符都不合法
        value.blake3 = Some("A".repeat(64)); // 大写不合法
        let id = asset_id(&value);
        assert_eq!(id.len(), 64);
        assert!(
            id.bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );

        // 长度对但含非 hex 字符同样降级。
        let mut value = asset("a.png");
        value.sha1 = Some("0123456789abcdef0123456789abcdef0123456z".to_string());
        assert_ne!(asset_id(&value), value.sha1.clone().unwrap());
    }

    #[test]
    fn item_normalises_fields_and_hides_paths() {
        let mut value = asset("图片 01.PNG");
        value.rating = "s".to_string();
        value.group = "  原创 ".to_string();
        value.star = 9;
        value.favorite = true;
        value.phash = Some(0x00ff_1234_abcd_5678);
        value.tags = vec![tag("1girl", "general", 0.9961), tag("highres", "meta", 1.0)];
        value.manual_tags = vec![tag("1girl", "manual", 1.0), tag("my_tag", "", 0.5)];
        let item = build_item(&value);
        assert_eq!(item.rt, "S");
        assert_eq!(item.g.as_deref(), Some("原创"));
        assert_eq!(item.st, 5);
        assert!(item.fav);
        assert_eq!(item.n, "图片 01.PNG");
        assert_eq!(item.ph.as_deref(), Some("00ff1234abcd5678"));
        assert_eq!(
            item.tags,
            vec![
                ("1girl".to_string(), "general".to_string(), 0.996),
                ("highres".to_string(), "meta".to_string(), 1.0),
                ("my_tag".to_string(), String::new(), 0.5),
            ]
        );
        let json = serde_json::to_string(&item).unwrap();
        assert!(!json.contains("C:"));
        assert!(!json.contains("lib"));
        assert!(json.contains("\"rt\":\"S\""));
        assert!(json.contains("\"g\":\"原创\""));
    }

    #[test]
    fn missing_optional_fields_are_omitted() {
        let item = build_item(&asset("a.png"));
        let json = serde_json::to_string(&item).unwrap();
        assert!(!json.contains("\"ph\""));
        assert!(!json.contains("\"orig\""));
        assert!(!json.contains("\"g\""));
        assert_eq!(item.rt, "G");
        assert_eq!(item.st, 0);
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
    fn facets_sorted_by_count_then_name_and_truncated() {
        let mut tags = HashMap::new();
        let mut item = build_item(&asset("a.png"));
        item.tags = vec![
            ("b".to_string(), "general".to_string(), 1.0),
            ("a".to_string(), "general".to_string(), 1.0),
        ];
        count_tags(&mut tags, &item);
        count_tags(&mut tags, &item);
        item.tags = vec![("c".to_string(), "meta".to_string(), 1.0)];
        count_tags(&mut tags, &item);

        let facets = build_tag_facets(&tags, MAX_FACET_TAGS);
        assert_eq!(facets.len(), 3);
        assert_eq!(facets[0].name, "a");
        assert_eq!(facets[0].count, 2);
        assert_eq!(facets[1].name, "b");
        assert_eq!(facets[2].name, "c");
        assert_eq!(facets[2].category, "meta");
        assert_eq!(build_tag_facets(&tags, 2).len(), 2);

        let mut groups = HashMap::new();
        let mut item = build_item(&asset("a.png"));
        item.g = Some("漫画".to_string());
        count_group(&mut groups, &item);
        count_group(&mut groups, &item);
        item.g = Some("原创".to_string());
        count_group(&mut groups, &item);
        assert_eq!(
            build_group_facets(&groups, MAX_FACET_GROUPS),
            vec!["漫画".to_string(), "原创".to_string()]
        );
        assert_eq!(build_group_facets(&groups, 1), vec!["漫画".to_string()]);
    }

    #[test]
    fn paths_defaults_match_contract() {
        let paths = Paths::default();
        assert_eq!(paths.t256, "t256/");
        assert_eq!(paths.t512, "t512/");
        assert_eq!(paths.t1024, "t1024/");
        assert_eq!(paths.t2048, "t2048/");
        assert_eq!(paths.original, "o/");
    }
}
