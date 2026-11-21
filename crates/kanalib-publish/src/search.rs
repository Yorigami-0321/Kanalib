//! 全库搜索索引（契约 §2.1/§7）：tag 表去重 + item 存 tag 下标。
//!
//! 目标是把「名称 / 标签 / 分组 / 分级 / 星级」的检索从全量分片解析里解耦出来，同时
//! 控制首包体积：`tags` 只存去重后的标签名（覆盖 facets 上限之外的长尾），每个 item
//! 存一行 `[id, 名称, [tagIdx...], groupIdx, st, rt]`。行的顺序沿用分片顺序。
//!
//! 输出是确定性的：`tags` 按标签名升序，行内下标升序去重，`groups` 升序且不含空串
//! （未分组用 `-1` 表示），文件里没有时间戳，因此同一份分片的两次重建字节一致。

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::fsutil;
use crate::manifest::{Index, Item, SCHEMA_VERSION, Shard};

pub const SEARCH_INDEX_FILE: &str = "search-index.json";
/// 索引结构版本；删字段/改语义必须升版（加字段不升版）。
pub const SEARCH_INDEX_SCHEMA: u32 = 1;

/// 单行：`[id, 名称, [tagIdx...], groupIdx, st, rt]`；`groupIdx=-1` 表示未分组。
pub type SearchItem = (String, String, Vec<u32>, i32, u8, String);

/// 搜索索引：`tags`/`groups` 是共享表，`items` 按分片顺序引用它们。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchIndex {
    pub v: u32,
    /// 全库去重标签名，升序；`items[*][2]` 里的下标指向这里。
    pub tags: Vec<String>,
    /// 分组名去重升序，不含空串；未分组用 `groupIdx=-1`。
    pub groups: Vec<String>,
    pub items: Vec<SearchItem>,
}

impl SearchIndex {
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).context("序列化 search-index.json 失败")
    }

    pub fn write(&self, out_dir: &Path) -> Result<()> {
        let bytes = self.to_bytes()?;
        fsutil::write_atomic(&out_dir.join(SEARCH_INDEX_FILE), &bytes)
    }
}

#[derive(Clone, Debug)]
struct Row {
    id: String,
    name: String,
    star: u8,
    rating: &'static str,
    group: Option<String>,
    tags: Vec<u32>,
}

/// 增量构建器：扫描资产时逐条 push，finish 时统一排序 tag/group 表并回填下标。
#[derive(Clone, Debug, Default)]
pub struct SearchBuilder {
    tag_ids: HashMap<String, u32>,
    tag_names: Vec<String>,
    rows: Vec<Row>,
}

impl SearchBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// 收一条已经通过发布校验的 item；标签名去重后分配临时下标。
    pub fn push(&mut self, item: &Item) {
        let mut tags = Vec::with_capacity(item.tags.len());
        for (name, _category, _score) in &item.tags {
            let index = if let Some(&index) = self.tag_ids.get(name) {
                index
            } else {
                let index = self.tag_names.len() as u32;
                self.tag_names.push(name.clone());
                self.tag_ids.insert(name.clone(), index);
                index
            };
            tags.push(index);
        }
        tags.sort_unstable();
        tags.dedup();
        self.rows.push(Row {
            id: item.id.clone(),
            name: item.n.clone(),
            // 发布路径已在 build_item 归一化；重建路径读的是旧分片，这里兜底裁剪。
            star: item.st.min(5),
            rating: rating_token(&item.rt),
            group: item
                .g
                .as_deref()
                .map(str::trim)
                .filter(|group| !group.is_empty())
                .map(str::to_string),
            tags,
        });
    }

    pub fn finish(self) -> SearchIndex {
        // 原始 tag 表顺序就是 row.tags 里下标所指的顺序；排序后要按名字回查新下标。
        let original = self.tag_names;
        let mut tags = original.clone();
        tags.sort();
        let sorted_ids = tags
            .iter()
            .enumerate()
            .map(|(index, name)| (name.as_str(), index as u32))
            .collect::<HashMap<_, _>>();
        let remap = original
            .iter()
            .map(|name| sorted_ids[name.as_str()])
            .collect::<Vec<_>>();

        let mut group_names = BTreeSet::new();
        for row in &self.rows {
            if let Some(group) = &row.group {
                group_names.insert(group.clone());
            }
        }
        let groups = group_names.into_iter().collect::<Vec<_>>();
        let group_ids = groups
            .iter()
            .enumerate()
            .map(|(index, name)| (name.as_str(), index as i32))
            .collect::<HashMap<_, _>>();

        let items = self
            .rows
            .into_iter()
            .map(|mut row| {
                row.tags = row
                    .tags
                    .iter()
                    .map(|index| remap[*index as usize])
                    .collect();
                row.tags.sort_unstable();
                row.tags.dedup();
                let group = row
                    .group
                    .as_deref()
                    .and_then(|name| group_ids.get(name).copied())
                    .unwrap_or(-1);
                (
                    row.id,
                    row.name,
                    row.tags,
                    group,
                    row.star,
                    row.rating.to_string(),
                )
            })
            .collect();

        SearchIndex {
            v: SEARCH_INDEX_SCHEMA,
            tags,
            groups,
            items,
        }
    }
}

/// 分级归一化为契约要求的小写 token：`g`/`s`/`q`/`e`，缺失或未知回退空串。
fn rating_token(rating: &str) -> &'static str {
    match rating.trim().to_ascii_lowercase().as_str() {
        "g" => "g",
        "s" => "s",
        "q" => "q",
        "e" => "e",
        _ => "",
    }
}

#[derive(Clone, Debug)]
pub struct SearchStats {
    pub items: usize,
    pub tags: usize,
    pub bytes: usize,
    pub out_dir: PathBuf,
    pub written: bool,
}

impl fmt::Display for SearchStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "搜索索引：{} 条 / {} 个标签 / {} 字节（{}{}）",
            self.items,
            self.tags,
            self.bytes,
            if self.written {
                ""
            } else {
                "--dry-run 未写出；目标 "
            },
            self.out_dir.join(SEARCH_INDEX_FILE).display()
        )
    }
}

/// 只读现有导出目录（index.json + 分片）重建搜索索引。
///
/// 不打开数据库、不读账本、不碰任何图片；只写 `search-index.json`。
pub fn rebuild_search_index(out_dir: &Path, dry_run: bool) -> Result<SearchStats> {
    if !out_dir.is_dir() {
        bail!("导出目录不存在：{}", out_dir.display());
    }
    let index_path = out_dir.join("index.json");
    let index_bytes =
        fs::read(&index_path).with_context(|| format!("读取 {} 失败", index_path.display()))?;
    let index: Index = serde_json::from_slice(&index_bytes)
        .with_context(|| format!("解析 {} 失败", index_path.display()))?;
    if index.schema != SCHEMA_VERSION {
        bail!(
            "index.json schema={} 与当前契约 {} 不一致，先跑全量发布",
            index.schema,
            SCHEMA_VERSION
        );
    }

    let mut builder = SearchBuilder::new();
    for shard_ref in &index.shards {
        // index.json 是导出目录自己的产物，但重建是纯读操作，仍要拒绝绝对路径/.. 逃逸。
        let rel = std::path::Path::new(&shard_ref.path);
        if rel.is_absolute()
            || rel.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            bail!(
                "index.json 分片路径必须是导出目录内的相对路径：{}",
                shard_ref.path
            );
        }
        let path = out_dir.join(rel);
        let bytes = fs::read(&path).with_context(|| format!("读取分片 {} 失败", path.display()))?;
        let shard: Shard = serde_json::from_slice(&bytes)
            .with_context(|| format!("解析分片 {} 失败", path.display()))?;
        for item in &shard.items {
            builder.push(item);
        }
    }
    if builder.len() != index.count {
        bail!(
            "分片条目数 {} 与 index.count={} 不一致，先修复导出目录",
            builder.len(),
            index.count
        );
    }

    let search = builder.finish();
    let bytes = search.to_bytes()?;
    if !dry_run {
        search.write(out_dir)?;
    }
    Ok(SearchStats {
        items: search.items.len(),
        tags: search.tags.len(),
        bytes: bytes.len(),
        out_dir: out_dir.to_path_buf(),
        written: !dry_run,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(
        id: &str,
        name: &str,
        rating: &str,
        group: Option<&str>,
        tags: &[&str],
        star: u8,
    ) -> Item {
        Item {
            id: id.to_string(),
            w: 10,
            h: 10,
            n: name.to_string(),
            rt: rating.to_string(),
            g: group.map(|value| value.to_string()),
            st: star,
            fav: false,
            tags: tags
                .iter()
                .map(|name| ((*name).to_string(), "general".to_string(), 1.0))
                .collect(),
            ph: None,
            mtime: 1,
            orig: None,
            sharp: Some(0),
        }
    }

    #[test]
    fn tags_are_sorted_deduped_and_items_carry_group_star_rating() {
        let mut builder = SearchBuilder::new();
        builder.push(&item(
            "a",
            "a.png",
            "G",
            None,
            &["zeta", "alpha", "zeta"],
            3,
        ));
        builder.push(&item("b", "b.png", "S", Some(" 原创 "), &["alpha"], 5));
        let index = builder.finish();

        assert_eq!(index.v, 1);
        assert_eq!(index.tags, vec!["alpha", "zeta"]);
        // 未分组是 -1，空串不进 groups 表。
        assert_eq!(index.groups, vec!["原创"]);
        assert_eq!(
            index.items,
            vec![
                (
                    "a".to_string(),
                    "a.png".to_string(),
                    vec![0, 1],
                    -1,
                    3,
                    "g".to_string()
                ),
                (
                    "b".to_string(),
                    "b.png".to_string(),
                    vec![0],
                    0,
                    5,
                    "s".to_string()
                ),
            ]
        );
    }

    #[test]
    fn empty_tags_and_ungrouped_items_are_stable() {
        let mut builder = SearchBuilder::new();
        builder.push(&item("a", "a.png", "Q", None, &[], 0));
        let index = builder.finish();
        assert_eq!(index.tags, Vec::<String>::new());
        assert_eq!(index.groups, Vec::<String>::new());
        assert_eq!(index.items.len(), 1);
        assert_eq!(index.items[0].2, Vec::<u32>::new());
        assert_eq!(index.items[0].3, -1);
        assert_eq!(index.items[0].5, "q");
    }

    #[test]
    fn rating_token_matches_contract_values() {
        assert_eq!(rating_token("G"), "g");
        assert_eq!(rating_token(" s "), "s");
        assert_eq!(rating_token("Q"), "q");
        assert_eq!(rating_token("e"), "e");
        assert_eq!(rating_token(""), "");
        assert_eq!(rating_token("x"), "");
    }
}
