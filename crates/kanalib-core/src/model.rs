use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::dedup::{DedupState, DuplicateReview};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TagHit {
    pub tag: String,
    pub category: String,
    pub score: f32,
    #[serde(default)]
    pub vocab_index: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub path: PathBuf,
    /// 该资产所属的本地根目录。
    pub library_root: PathBuf,
    pub thumb: Option<PathBuf>,
    /// 网格用小档缩略图（由 `thumb` 同目录的 `-t256.jpg` 派生，不落库）；缺失时回退 `thumb`。
    pub tile_thumb: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    pub group: String,
    pub rating: String,
    pub favorite: bool,
    pub star: u8,
    pub tags: Vec<TagHit>,
    /// 人工添加的标签；与自动标签分开存储，搜索、计数与 Prompt 合并使用。
    pub manual_tags: Vec<TagHit>,
    pub tagged: bool,
    pub tag_attempts: u32,
    pub error: Option<String>,
    pub sha1: Option<String>,
    pub blake3: Option<String>,
    pub phash: Option<u64>,
    pub bytes: u64,
    pub mtime: u64,
}

impl Asset {
    pub fn title(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    pub fn all_tags(&self) -> impl Iterator<Item = &TagHit> {
        self.tags.iter().chain(self.manual_tags.iter())
    }

    pub fn top_tags(&self, count: usize) -> Vec<&TagHit> {
        self.all_tags()
            .filter(|tag| matches!(tag.category.as_str(), "general" | "manual"))
            .take(count)
            .collect()
    }

    pub fn summary(&self) -> String {
        let tags = self
            .top_tags(2)
            .into_iter()
            .map(|tag| format!("{} {:.2}", tag.tag, tag.score))
            .collect::<Vec<_>>();
        if tags.is_empty() {
            self.group.clone()
        } else {
            tags.join(" · ")
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Counts {
    pub total: usize,
    pub tagged: usize,
    pub untagged: usize,
    pub failed: usize,
    pub favorites: usize,
    pub ratings: Vec<(String, usize)>,
    pub groups: Vec<(String, usize)>,
    pub tags: Vec<(String, usize)>,
    pub sources: Vec<(PathBuf, usize)>,
}

#[derive(Clone, Debug, Default)]
pub struct CountAccumulator {
    total: usize,
    tagged: usize,
    untagged: usize,
    failed: usize,
    favorites: usize,
    ratings: HashMap<String, usize>,
    groups: HashMap<String, usize>,
    tags: HashMap<String, usize>,
    sources: HashMap<PathBuf, usize>,
}

impl CountAccumulator {
    pub fn from_assets(assets: &[Asset]) -> Self {
        let mut counts = Self::default();
        for asset in assets {
            counts.add(asset);
        }
        counts
    }

    pub fn add(&mut self, asset: &Asset) {
        self.total += 1;
        if asset.tagged {
            self.tagged += 1;
        } else {
            self.untagged += 1;
        }
        if asset.error.is_some() {
            self.failed += 1;
        }
        if asset.favorite {
            self.favorites += 1;
        }
        if !asset.rating.is_empty() {
            *self.ratings.entry(asset.rating.to_uppercase()).or_default() += 1;
        }
        if !asset.group.is_empty() {
            *self.groups.entry(asset.group.clone()).or_default() += 1;
        }
        if !asset.library_root.as_os_str().is_empty() {
            *self
                .sources
                .entry(asset.library_root.clone())
                .or_default() += 1;
        }
        let mut seen = HashSet::new();
        for tag in asset
            .all_tags()
            .filter(|tag| matches!(tag.category.as_str(), "general" | "manual"))
        {
            if seen.insert(tag.tag.clone()) {
                *self.tags.entry(tag.tag.clone()).or_default() += 1;
            }
        }
    }

    pub fn remove(&mut self, asset: &Asset) {
        self.total = self.total.saturating_sub(1);
        if asset.tagged {
            self.tagged = self.tagged.saturating_sub(1);
        } else {
            self.untagged = self.untagged.saturating_sub(1);
        }
        if asset.error.is_some() {
            self.failed = self.failed.saturating_sub(1);
        }
        if asset.favorite {
            self.favorites = self.favorites.saturating_sub(1);
        }
        if !asset.rating.is_empty() {
            decrement(&mut self.ratings, &asset.rating.to_uppercase());
        }
        if !asset.group.is_empty() {
            decrement(&mut self.groups, &asset.group);
        }
        if !asset.library_root.as_os_str().is_empty() {
            decrement_path(&mut self.sources, &asset.library_root);
        }
        let mut seen = HashSet::new();
        for tag in asset
            .all_tags()
            .filter(|tag| matches!(tag.category.as_str(), "general" | "manual"))
        {
            if seen.insert(tag.tag.clone()) {
                decrement(&mut self.tags, &tag.tag);
            }
        }
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn tagged_count(&self) -> usize {
        self.tagged
    }

    pub fn view(&self) -> Counts {
        Counts {
            total: self.total,
            tagged: self.tagged,
            untagged: self.untagged,
            failed: self.failed,
            favorites: self.favorites,
            ratings: top(&self.ratings, 8),
            groups: top(&self.groups, 12),
            tags: top(&self.tags, 18),
            sources: top_paths(&self.sources, 24),
        }
    }
}

fn top_paths(map: &HashMap<PathBuf, usize>, limit: usize) -> Vec<(PathBuf, usize)> {
    let mut rows = map
        .iter()
        .map(|(path, count)| (path.clone(), *count))
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rows.truncate(limit);
    rows
}

fn decrement_path(map: &mut HashMap<PathBuf, usize>, key: &Path) {
    let remove = match map.get_mut(key) {
        Some(value) => {
            *value = value.saturating_sub(1);
            *value == 0
        }
        None => false,
    };
    if remove {
        map.remove(key);
    }
}

fn top(map: &HashMap<String, usize>, limit: usize) -> Vec<(String, usize)> {
    let mut rows = map
        .iter()
        .map(|(name, count)| (name.clone(), *count))
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rows.truncate(limit);
    rows
}

fn decrement(map: &mut HashMap<String, usize>, key: &str) {
    let remove = match map.get_mut(key) {
        Some(value) => {
            *value = value.saturating_sub(1);
            *value == 0
        }
        None => false,
    };
    if remove {
        map.remove(key);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Filter {
    All,
    Untagged,
    Failed,
    Favorites,
    Rating(String),
    Group(String),
    Tag(String),
    Source(PathBuf),
}

impl Filter {
    pub fn label(&self) -> String {
        match self {
            Self::All => "全部图片".to_string(),
            Self::Untagged => "未打标".to_string(),
            Self::Failed => "失败".to_string(),
            Self::Favorites => "收藏".to_string(),
            Self::Rating(rating) => format!("评级 {rating}"),
            Self::Group(group) => group.clone(),
            Self::Tag(tag) => format!("#{tag}"),
            Self::Source(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string()),
        }
    }

    pub fn matches(&self, asset: &Asset) -> bool {
        match self {
            Self::All => true,
            Self::Untagged => !asset.tagged,
            Self::Failed => asset.error.is_some(),
            Self::Favorites => asset.favorite,
            Self::Rating(rating) => asset.rating.eq_ignore_ascii_case(rating),
            Self::Group(group) => &asset.group == group,
            Self::Tag(tag) => asset.all_tags().any(|hit| &hit.tag == tag),
            Self::Source(root) => &asset.library_root == root,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SearchQuery {
    free: Vec<String>,
    tags: Vec<String>,
    exclude_tags: Vec<String>,
    rating: Option<String>,
    exclude_rating: Option<String>,
    tagged: Option<bool>,
    favorite: Option<bool>,
    star: Option<u8>,
    star_min: Option<u8>,
    exclude_star: Option<u8>,
    duplicates_only: bool,
    impossible: bool,
}

impl SearchQuery {
    pub fn parse(input: &str) -> Self {
        let mut query = Self::default();
        for raw in input.split_whitespace() {
            let (negated, token) = match raw.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, raw),
            };
            let lower = token.to_ascii_lowercase();
            if lower.starts_with("tag:") {
                let value = token[4..].trim_start_matches('#').trim();
                if !value.is_empty() {
                    if negated {
                        query.exclude_tags.push(value.to_string());
                    } else {
                        query.tags.push(value.to_string());
                    }
                }
            } else if lower.starts_with("rating:") {
                let value = lower[7..].trim();
                if !value.is_empty() {
                    if negated {
                        query.exclude_rating = Some(value.to_string());
                    } else {
                        query.rating = Some(value.to_string());
                    }
                }
            } else if lower.starts_with("is:") {
                match (&lower[3..], negated) {
                    ("untagged", false) | ("tagged", true) => query.tagged = Some(false),
                    ("tagged", false) | ("untagged", true) => query.tagged = Some(true),
                    ("fav", _) | ("favorite", _) | ("favourite", _) => {
                        query.favorite = Some(!negated)
                    }
                    _ => {}
                }
            } else if lower.starts_with("star:") {
                let value = token[5..].trim();
                if let Some(min) = value.strip_prefix(">=") {
                    if let Ok(min) = min.trim().parse::<u8>() {
                        query.star_min = Some(min.min(5));
                    }
                } else if let Ok(star) = value.parse::<u8>() {
                    let star = star.min(5);
                    if negated {
                        query.exclude_star = Some(star);
                    } else {
                        query.star = Some(star);
                    }
                }
            } else if lower.starts_with("source:") {
                if lower[7..].trim() != "local" {
                    query.impossible = true;
                }
            } else if lower.starts_with("dup:") {
                if !negated && lower[4..].trim() == "true" {
                    query.duplicates_only = true;
                }
            } else if token.starts_with('#') {
                let value = token[1..].trim();
                if !value.is_empty() {
                    query.tags.push(value.to_string());
                }
            } else if !negated {
                query.free.push(token.to_string());
            }
        }
        query
    }

    pub fn free_text(&self) -> String {
        self.free.join(" ")
    }

    pub fn matches(
        &self,
        asset: &Asset,
        fts_paths: Option<&HashSet<PathBuf>>,
        duplicate_paths: Option<&HashSet<PathBuf>>,
    ) -> bool {
        if self.impossible {
            return false;
        }
        if self.duplicates_only {
            let Some(paths) = duplicate_paths else {
                return false;
            };
            if !paths.contains(&asset.path) {
                return false;
            }
        }
        if let Some(rating) = &self.rating {
            if !asset.rating.eq_ignore_ascii_case(rating) {
                return false;
            }
        }
        if let Some(rating) = &self.exclude_rating {
            if asset.rating.eq_ignore_ascii_case(rating) {
                return false;
            }
        }
        if let Some(tagged) = self.tagged {
            if asset.tagged != tagged {
                return false;
            }
        }
        if let Some(favorite) = self.favorite {
            if asset.favorite != favorite {
                return false;
            }
        }
        if let Some(star) = self.star {
            if asset.star != star {
                return false;
            }
        }
        if let Some(min) = self.star_min {
            if asset.star < min {
                return false;
            }
        }
        if let Some(star) = self.exclude_star {
            if asset.star == star {
                return false;
            }
        }
        for tag in &self.tags {
            if !asset
                .all_tags()
                .any(|hit| hit.tag.eq_ignore_ascii_case(tag))
            {
                return false;
            }
        }
        for tag in &self.exclude_tags {
            if asset
                .all_tags()
                .any(|hit| hit.tag.eq_ignore_ascii_case(tag))
            {
                return false;
            }
        }
        if !self.free.is_empty() {
            if let Some(paths) = fts_paths {
                if !paths.contains(&asset.path) {
                    return false;
                }
            } else {
                let tags = asset
                    .all_tags()
                    .map(|hit| hit.tag.to_lowercase())
                    .collect::<Vec<_>>()
                    .join(" ");
                let haystack = format!(
                    "{} {} {}",
                    asset.title().to_lowercase(),
                    asset.path.to_string_lossy().to_lowercase(),
                    tags
                );
                for token in &self.free {
                    if !haystack.contains(&token.to_lowercase()) {
                        return false;
                    }
                }
            }
        }
        true
    }
}

#[derive(Clone, Debug)]
pub struct SavedSearch {
    pub id: i64,
    pub name: String,
    pub filter: Filter,
    pub query: String,
}

#[derive(Clone, Debug)]
pub struct TagSummary {
    pub name: String,
    pub category: String,
    pub count: usize,
    pub manual_count: usize,
}

pub fn normalize_tag_name(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('#')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_")
        .to_lowercase()
}

#[derive(Debug)]
pub struct Shared {
    pub roots: Vec<PathBuf>,
    pub assets: Arc<RwLock<Vec<Asset>>>,
    pub counts: CountAccumulator,
    pub revision: u64,
    pub status: String,
    pub error: Option<String>,
    pub filter: Filter,
    pub selected: Option<PathBuf>,
    pub scanning: bool,
    pub sidecar_device: Option<String>,
    pub queue_pending: usize,
    pub queue_running: usize,
    pub queue_error: usize,
    pub dedup: DedupState,
    pub saved_searches: Vec<SavedSearch>,
    /// 只影响侧栏标签面的显示，不修改资产标签本身。
    pub blocked_tags: HashSet<String>,
    /// 来源显示别名；键为根目录，只影响 UI，不改磁盘目录。
    pub source_aliases: HashMap<PathBuf, String>,
}

impl Shared {
    pub fn new(
        roots: Vec<PathBuf>,
        assets: Vec<Asset>,
        reviews: std::collections::HashMap<String, DuplicateReview>,
        similar_threshold: u8,
        protected_paths: Vec<PathBuf>,
    ) -> Self {
        let status = if assets.is_empty() {
            if roots.is_empty() {
                "请添加文件夹".to_string()
            } else {
                "准备扫描".to_string()
            }
        } else {
            format!("已加载 {} 张图片", assets.len())
        };
        let selected = assets.first().map(|asset| asset.path.clone());
        let counts = CountAccumulator::from_assets(&assets);
        let mut dedup = DedupState::with_reviews_and_threshold(reviews, similar_threshold);
        dedup.set_protected_paths(protected_paths);
        dedup.rebuild(&assets);
        Self {
            roots,
            assets: Arc::new(RwLock::new(assets)),
            counts,
            revision: 1,
            status,
            error: None,
            filter: Filter::All,
            selected,
            scanning: false,
            sidecar_device: None,
            queue_pending: 0,
            queue_running: 0,
            queue_error: 0,
            dedup,
            saved_searches: Vec::new(),
            blocked_tags: HashSet::new(),
            source_aliases: HashMap::new(),
        }
    }

    pub fn set_queue_counts(&mut self, pending: usize, running: usize, error: usize) {
        self.queue_pending = pending;
        self.queue_running = running;
        self.queue_error = error;
    }

    pub fn sort_assets(&mut self) {
        self.assets.write().unwrap().sort_by(|a, b| {
            a.title()
                .to_lowercase()
                .cmp(&b.title().to_lowercase())
                .then_with(|| a.path.cmp(&b.path))
        });
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn refresh_dedup(&mut self) {
        let assets = self.assets.read().unwrap();
        self.dedup.rebuild(&assets);
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn refresh_after_removal(&mut self, removed: &HashSet<PathBuf>) {
        let assets = self.assets.read().unwrap();
        self.dedup.rebuild_exact(&assets);
        self.dedup.prune_similar(removed);
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn remove_assets(&mut self, paths: &HashSet<PathBuf>) -> usize {
        let mut removed = Vec::new();
        {
            let mut assets = self.assets.write().unwrap();
            assets.retain(|asset| {
                if paths.contains(&asset.path) {
                    removed.push(asset.clone());
                    false
                } else {
                    true
                }
            });
        }
        for asset in &removed {
            self.counts.remove(asset);
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|path| paths.contains(path))
        {
            self.selected = None;
        }
        if !removed.is_empty() {
            self.revision = self.revision.wrapping_add(1);
        }
        removed.len()
    }

    pub fn upsert(&mut self, asset: Asset) {
        let mut assets = self.assets.write().unwrap();
        if let Some(index) = assets.iter().position(|item| item.path == asset.path) {
            self.counts.remove(&assets[index]);
            self.counts.add(&asset);
            assets[index] = asset;
        } else {
            self.counts.add(&asset);
            assets.push(asset);
        }
        drop(assets);
        self.revision = self.revision.wrapping_add(1);
    }
}

pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .map(|ext| {
            let ext = ext.to_string_lossy().to_lowercase();
            IMAGE_EXTENSIONS.iter().any(|known| *known == ext)
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str, category: &str) -> TagHit {
        TagHit {
            tag: name.to_string(),
            category: category.to_string(),
            score: 1.0,
            vocab_index: None,
        }
    }

    fn asset(path: &str, tagged: bool, rating: &str, group: &str, tags: Vec<TagHit>) -> Asset {
        Asset {
            path: PathBuf::from(path),
            library_root: PathBuf::from("lib"),
            thumb: None,
            tile_thumb: None,
            width: 0,
            height: 0,
            group: group.to_string(),
            rating: rating.to_string(),
            favorite: false,
            star: 0,
            tags,
            manual_tags: Vec::new(),
            tagged,
            tag_attempts: 0,
            error: None,
            sha1: None,
            blake3: None,
            phash: None,
            bytes: 0,
            mtime: 0,
        }
    }

    #[test]
    fn counts_accumulate_and_remove() {
        let mut a = asset(
            "a.png",
            true,
            "g",
            "原创",
            vec![
                tag("1girl", "general"),
                tag("solo", "general"),
                tag("highres", "meta"),
            ],
        );
        a.error = Some("boom".to_string());
        let b = asset(
            "b.png",
            false,
            "s",
            "作品: X",
            vec![tag("1girl", "general")],
        );
        let mut counts = CountAccumulator::from_assets(&[a.clone(), b]);
        let view = counts.view();
        assert_eq!((view.total, view.tagged, view.untagged), (2, 1, 1));
        assert_eq!(view.failed, 1);
        assert_eq!(
            view.ratings,
            vec![("G".to_string(), 1), ("S".to_string(), 1)]
        );
        assert_eq!(view.groups.len(), 2);
        assert_eq!(
            view.tags,
            vec![("1girl".to_string(), 2), ("solo".to_string(), 1)]
        );

        counts.remove(&a);
        let view = counts.view();
        assert_eq!((view.total, view.tagged, view.untagged), (1, 0, 1));
        assert_eq!(view.failed, 0);
        assert_eq!(view.ratings, vec![("S".to_string(), 1)]);
        assert_eq!(view.tags, vec![("1girl".to_string(), 1)]);
    }

    #[test]
    fn favorites_and_stars_filter_and_count() {
        let mut favorite = asset("a.png", true, "g", "原创", vec![]);
        favorite.favorite = true;
        favorite.star = 4;
        let mut plain = asset("b.png", true, "s", "作品: X", vec![]);
        plain.star = 2;
        let counts = CountAccumulator::from_assets(&[favorite.clone(), plain.clone()]);
        assert_eq!(counts.view().favorites, 1);

        let query = SearchQuery::parse("is:fav star:>=3");
        assert!(query.matches(&favorite, None, None));
        assert!(!query.matches(&plain, None, None));

        let query = SearchQuery::parse("-is:favorite star:2");
        assert!(!query.matches(&favorite, None, None));
        assert!(query.matches(&plain, None, None));

        assert!(Filter::Favorites.matches(&favorite));
        assert!(!Filter::Favorites.matches(&plain));
    }

    #[test]
    fn manual_tags_join_counts_filters_and_search() {
        let mut a = asset(
            "a.png",
            true,
            "g",
            "原创",
            vec![tag("1girl", "general")],
        );
        a.manual_tags.push(tag("custom_tag", "manual"));
        let counts = CountAccumulator::from_assets(&[a.clone()]);
        assert_eq!(
            counts.view().tags,
            vec![
                ("1girl".to_string(), 1),
                ("custom_tag".to_string(), 1)
            ]
        );
        assert!(Filter::Tag("custom_tag".to_string()).matches(&a));
        let query = SearchQuery::parse("tag:custom_tag");
        assert!(query.matches(&a, None, None));
        assert_eq!(a.top_tags(2).len(), 2);
    }

    #[test]
    fn dup_true_requires_the_duplicate_path_set() {
        let a = asset("a.png", true, "g", "原创", vec![]);
        let query = SearchQuery::parse("dup:true");
        assert!(!query.matches(&a, None, None));
        let paths = HashSet::from([PathBuf::from("a.png")]);
        assert!(query.matches(&a, None, Some(&paths)));
    }

    #[test]
    fn shared_upsert_updates_counts_and_revision() {
        let old = asset("a.png", true, "g", "原创", vec![tag("1girl", "general")]);
        let mut shared = Shared::new(
            vec![PathBuf::from("lib")],
            vec![old.clone()],
            Default::default(),
            crate::dedup::DEFAULT_SIMILAR_THRESHOLD,
            Vec::new(),
        );
        assert_eq!(shared.revision, 1);
        assert_eq!(shared.counts.view().tagged, 1);

        let new = asset("a.png", false, "s", "作品: X", vec![tag("sky", "general")]);
        shared.upsert(new);
        let view = shared.counts.view();
        assert_eq!(shared.revision, 2);
        assert_eq!(view.total, 1);
        assert_eq!(view.tagged, 0);
        assert_eq!(view.ratings, vec![("S".to_string(), 1)]);
        assert_eq!(view.tags, vec![("sky".to_string(), 1)]);
    }
}
