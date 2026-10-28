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

/// 与 sidecar 模型配置保持一致的类别顺序；阈值与候选上限都按这个集合工作。
pub const TAG_CATEGORIES: &[&str] = &[
    "general",
    "character",
    "copyright",
    "style",
    "meta",
    "rating",
];

/// sidecar 未提供类别默认阈值时的兜底（与 pixai-tagger-v1.0 config 对齐）。
pub fn default_category_thresholds() -> HashMap<String, f32> {
    HashMap::from([
        ("general".to_string(), 0.17),
        ("character".to_string(), 0.27),
        ("copyright".to_string(), 0.24),
        ("style".to_string(), 0.15),
        ("meta".to_string(), 0.17),
        ("rating".to_string(), 0.41),
    ])
}

/// 每类最多保留的候选数。取 40 与 sidecar 原有 `top` 截断一致，保证同阈值重算
/// 不会比旧行为少标签；rating 只有 4 个词表项。每资产最坏 204 行，行数有界。
pub fn candidate_limit(category: &str) -> usize {
    match category {
        "general" | "character" | "copyright" | "style" | "meta" => 40,
        "rating" => 4,
        _ => 0,
    }
}

/// 低于该分数的候选不落库；阈值最低可调到 0，但无法恢复未保存的极低分候选。
pub const TAG_CANDIDATE_MIN_SCORE: f32 = 0.005;

#[derive(Clone, Debug)]
pub struct TagThresholds {
    pub categories: HashMap<String, f32>,
    /// 键含别名；读取时优先单标签覆盖，其次类别默认值。
    pub overrides: HashMap<String, f32>,
}

impl Default for TagThresholds {
    fn default() -> Self {
        Self {
            categories: default_category_thresholds(),
            overrides: HashMap::new(),
        }
    }
}

impl TagThresholds {
    pub fn threshold(&self, category: &str, tag: &str) -> f32 {
        self.overrides
            .get(tag)
            .copied()
            .or_else(|| self.overrides.get(&tag.to_lowercase()).copied())
            .unwrap_or_else(|| {
                self.categories
                    .get(&category.to_ascii_lowercase())
                    .copied()
                    .unwrap_or(0.2)
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TagReviewState {
    Pending,
    Accepted,
    Ignored,
}

impl TagReviewState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Ignored => "ignored",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "accepted" => Self::Accepted,
            "ignored" => Self::Ignored,
            _ => Self::Pending,
        }
    }
}

/// 低置信度审核的一条候选；`threshold` 是查询时的有效阈值（含单标签覆盖）。
#[derive(Clone, Debug)]
pub struct TagCandidateReview {
    pub path: PathBuf,
    pub tag: String,
    pub category: String,
    pub score: f32,
    pub vocab_index: Option<u32>,
    pub threshold: f32,
    pub state: TagReviewState,
    pub reviewed_at: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct TagRecomputeSummary {
    pub updated: usize,
    pub skipped: usize,
}

#[derive(Clone, Debug, Default)]
pub struct RetagSummary {
    pub recomputed: usize,
    pub queued: usize,
    pub missing: usize,
}

#[derive(Clone, Debug)]
pub struct TagJobError {
    pub job_id: i64,
    pub path: PathBuf,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub priority: i64,
}

#[derive(Clone, Debug, Default)]
pub struct TagQueueTotals {
    pub pending: usize,
    pub running: usize,
    pub error: usize,
    pub done: usize,
    pub total: usize,
}

/// 后台打标队列的运行时状态（暂停/取消、吞吐、ETA、错误与设备）。
#[derive(Clone, Debug, Default)]
pub struct TagQueueStatus {
    pub paused: bool,
    pub cancelling: bool,
    pub pending: usize,
    pub running: usize,
    pub error: usize,
    pub done: usize,
    /// 本轮开始时待处理总数。
    pub total: usize,
    /// 本轮已完成数。
    pub completed: usize,
    /// 本轮失败数。
    pub failed: usize,
    pub elapsed_secs: f64,
    pub throughput_per_sec: f32,
    pub eta_secs: Option<u64>,
    pub last_error: Option<String>,
    pub last_error_path: Option<PathBuf>,
    pub device: Option<String>,
    pub gpu_available: Option<bool>,
    pub message: Option<String>,
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
    /// 可读的文件级异常（如“文件不完整”）；与打标失败分开，修复后自动清除。
    pub damage: Option<String>,
    pub sha1: Option<String>,
    pub blake3: Option<String>,
    pub phash: Option<u64>,
    pub bytes: u64,
    pub mtime: u64,
    /// EXIF 拍摄时间（UTC 秒）；缺失时回退 `mtime`（见 `taken_or_modified`）。
    pub captured_at: Option<u64>,
    /// 原始 EXIF 摘要 JSON；检查器按需反序列化，避免为每张图常驻结构体。
    pub metadata_json: String,
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

    pub fn taken_or_modified(&self) -> u64 {
        self.captured_at.unwrap_or(self.mtime)
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
    pub damaged: usize,
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
    damaged: usize,
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
        if asset.damage.is_some() {
            self.damaged += 1;
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
            *self.sources.entry(asset.library_root.clone()).or_default() += 1;
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
        if asset.damage.is_some() {
            self.damaged = self.damaged.saturating_sub(1);
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
            damaged: self.damaged,
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
    Damaged,
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
            Self::Damaged => "损坏".to_string(),
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
            Self::Damaged => asset.damage.is_some(),
            Self::Favorites => asset.favorite,
            Self::Rating(rating) => asset.rating.eq_ignore_ascii_case(rating),
            Self::Group(group) => &asset.group == group,
            Self::Tag(tag) => asset.all_tags().any(|hit| &hit.tag == tag),
            Self::Source(root) => &asset.library_root == root,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NumOp {
    Eq,
    Gt,
    Ge,
    Lt,
    Le,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum NumField {
    Size,
    Width,
    Height,
    Ratio,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NumericFilter {
    pub(crate) field: NumField,
    pub(crate) op: NumOp,
    pub(crate) value: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct DateFilter {
    pub(crate) op: NumOp,
    pub(crate) year: i32,
    pub(crate) month: u8,
    pub(crate) day: Option<u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct ScopedTag {
    pub(crate) category: String,
    pub(crate) value: String,
}

#[derive(Clone, Debug, Default)]
pub struct SearchQuery {
    free: Vec<String>,
    exclude_free: Vec<String>,
    tags: Vec<String>,
    exclude_tags: Vec<String>,
    scoped_tags: Vec<ScopedTag>,
    exclude_scoped_tags: Vec<ScopedTag>,
    groups: Vec<String>,
    exclude_groups: Vec<String>,
    names: Vec<String>,
    exclude_names: Vec<String>,
    paths: Vec<String>,
    exclude_paths: Vec<String>,
    rating: Option<String>,
    exclude_rating: Option<String>,
    tagged: Option<bool>,
    favorite: Option<bool>,
    failed: Option<bool>,
    damaged: Option<bool>,
    star: Option<u8>,
    star_min: Option<u8>,
    star_max: Option<u8>,
    exclude_star: Option<u8>,
    numeric: Vec<NumericFilter>,
    exclude_numeric: Vec<NumericFilter>,
    dates: Vec<DateFilter>,
    exclude_dates: Vec<DateFilter>,
    duplicates_only: bool,
    impossible: bool,
    warnings: Vec<String>,
}

/// 只读视图：供同 crate 的 SQL 下推构造使用，不改变解析/匹配语义。
pub(crate) struct SearchQueryParts<'a> {
    pub(crate) free: &'a [String],
    pub(crate) exclude_free: &'a [String],
    pub(crate) tags: &'a [String],
    pub(crate) exclude_tags: &'a [String],
    pub(crate) scoped_tags: &'a [ScopedTag],
    pub(crate) exclude_scoped_tags: &'a [ScopedTag],
    pub(crate) groups: &'a [String],
    pub(crate) exclude_groups: &'a [String],
    pub(crate) names: &'a [String],
    pub(crate) exclude_names: &'a [String],
    pub(crate) paths: &'a [String],
    pub(crate) exclude_paths: &'a [String],
    pub(crate) rating: Option<&'a str>,
    pub(crate) exclude_rating: Option<&'a str>,
    pub(crate) tagged: Option<bool>,
    pub(crate) favorite: Option<bool>,
    pub(crate) failed: Option<bool>,
    pub(crate) damaged: Option<bool>,
    pub(crate) star: Option<u8>,
    pub(crate) star_min: Option<u8>,
    pub(crate) star_max: Option<u8>,
    pub(crate) exclude_star: Option<u8>,
    pub(crate) numeric: &'a [NumericFilter],
    pub(crate) exclude_numeric: &'a [NumericFilter],
    pub(crate) dates: &'a [DateFilter],
    pub(crate) exclude_dates: &'a [DateFilter],
    pub(crate) duplicates_only: bool,
    pub(crate) impossible: bool,
}

fn parse_comparison(value: &str) -> (NumOp, &str) {
    for (prefix, op) in [
        (">=", NumOp::Ge),
        ("<=", NumOp::Le),
        (">", NumOp::Gt),
        ("<", NumOp::Lt),
        ("=", NumOp::Eq),
    ] {
        if let Some(rest) = value.strip_prefix(prefix) {
            return (op, rest.trim());
        }
    }
    (NumOp::Eq, value.trim())
}

fn parse_size_value(value: &str) -> Option<f64> {
    let lower = value.trim().to_ascii_lowercase();
    let (number, factor) = if let Some(rest) = lower.strip_suffix("gb") {
        (rest, 1024.0 * 1024.0 * 1024.0)
    } else if let Some(rest) = lower.strip_suffix("mb") {
        (rest, 1024.0 * 1024.0)
    } else if let Some(rest) = lower.strip_suffix("kb") {
        (rest, 1024.0)
    } else if let Some(rest) = lower.strip_suffix('b') {
        (rest, 1.0)
    } else {
        (lower.as_str(), 1.0)
    };
    number
        .trim()
        .parse::<f64>()
        .ok()
        .map(|value| value * factor)
}

fn parse_date_filter(value: &str) -> Option<DateFilter> {
    let (op, value) = parse_comparison(value);
    let mut parts = value.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts
        .next()
        .map(|value| value.parse::<u8>().ok())
        .unwrap_or(Some(1))?;
    let day = parts.next().map(|value| value.parse::<u8>().ok()).flatten();
    if !(1..=12).contains(&month) || day.is_some_and(|day| !(1..=31).contains(&day)) {
        return None;
    }
    Some(DateFilter {
        op,
        year,
        month,
        day,
    })
}

fn date_parts(seconds: u64) -> (i32, u8, u8) {
    let days = (seconds / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year as i32, month as u8, day as u8)
}

/// `date_parts` 的逆运算：公历年月日 → 1970-01-01 起的天数。
/// SQL 下推用它把日期比较变成整数比较，结果与 `date_parts` 的字典序一致。
pub(crate) fn day_number(year: i32, month: u8, day: u8) -> i64 {
    let year = if month <= 2 {
        i64::from(year) - 1
    } else {
        i64::from(year)
    };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = i64::from(month);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn compare_numbers(op: NumOp, actual: f64, target: f64) -> bool {
    match op {
        NumOp::Eq => (actual - target).abs() < f64::EPSILON,
        NumOp::Gt => actual > target,
        NumOp::Ge => actual >= target,
        NumOp::Lt => actual < target,
        NumOp::Le => actual <= target,
    }
}

fn compare_date(op: NumOp, actual: (i32, u8, u8), filter: &DateFilter) -> bool {
    let target = (filter.year, filter.month, filter.day.unwrap_or(1));
    match op {
        NumOp::Eq => {
            if filter.day.is_some() {
                actual == target
            } else {
                actual.0 == filter.year && actual.1 == filter.month
            }
        }
        NumOp::Gt => actual > target,
        NumOp::Ge => actual >= target,
        NumOp::Lt => actual < target,
        NumOp::Le => actual <= target,
    }
}

fn scoped_category(prefix: &str) -> Option<&'static str> {
    Some(match prefix {
        "general" => "general",
        "character" => "character",
        "copyright" => "copyright",
        "style" => "style",
        "meta" => "meta",
        "manual" => "manual",
        _ => return None,
    })
}

impl SearchQuery {
    pub fn parse(input: &str) -> Self {
        let mut query = Self::default();
        for raw in input.split_whitespace() {
            let (negated, token) = match raw.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, raw),
            };
            if token.is_empty() {
                continue;
            }
            let lower = token.to_ascii_lowercase();
            let (prefix, value) = token
                .split_once(':')
                .map(|(prefix, value)| (prefix.to_ascii_lowercase(), value.trim()))
                .unwrap_or_default();
            if let Some(category) = scoped_category(&prefix) {
                if value.is_empty() {
                    query.warnings.push(format!("条件缺少值：{token}"));
                } else {
                    let scoped = ScopedTag {
                        category: category.to_string(),
                        value: value.trim_start_matches('#').to_string(),
                    };
                    if negated {
                        query.exclude_scoped_tags.push(scoped);
                    } else {
                        query.scoped_tags.push(scoped);
                    }
                }
            } else if lower.starts_with("tag:") {
                let value = token[4..].trim_start_matches('#').trim();
                if !value.is_empty() {
                    if negated {
                        query.exclude_tags.push(value.to_string());
                    } else {
                        query.tags.push(value.to_string());
                    }
                }
            } else if lower.starts_with("group:") {
                let value = token[6..].trim();
                if !value.is_empty() {
                    if negated {
                        query.exclude_groups.push(value.to_string());
                    } else {
                        query.groups.push(value.to_string());
                    }
                }
            } else if lower.starts_with("name:") {
                let value = token[5..].trim();
                if !value.is_empty() {
                    if negated {
                        query.exclude_names.push(value.to_lowercase());
                    } else {
                        query.names.push(value.to_lowercase());
                    }
                }
            } else if lower.starts_with("path:") {
                let value = token[5..].trim();
                if !value.is_empty() {
                    if negated {
                        query.exclude_paths.push(value.to_lowercase());
                    } else {
                        query.paths.push(value.to_lowercase());
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
                    ("failed", _) => query.failed = Some(!negated),
                    ("damaged", _) => query.damaged = Some(!negated),
                    (value, _) => query.warnings.push(format!("未知状态：is:{value}")),
                }
            } else if lower.starts_with("star:") {
                let (op, value) = parse_comparison(&token[5..]);
                let Ok(star) = value.parse::<u8>() else {
                    query.warnings.push(format!("无法解析星级：{token}"));
                    continue;
                };
                let star = star.min(5);
                if negated {
                    match op {
                        NumOp::Ge => query.star_max = Some(star.saturating_sub(1)),
                        NumOp::Gt => query.star_max = Some(star),
                        NumOp::Le => query.star_min = Some(star + 1),
                        NumOp::Lt => query.star_min = Some(star),
                        NumOp::Eq => query.exclude_star = Some(star),
                    }
                } else {
                    match op {
                        NumOp::Eq => query.star = Some(star),
                        NumOp::Ge => query.star_min = Some(star),
                        NumOp::Gt => query.star_min = Some((star + 1).min(5)),
                        NumOp::Le => query.star_max = Some(star),
                        NumOp::Lt => query.star_max = Some(star.saturating_sub(1)),
                    }
                }
            } else if lower.starts_with("size:")
                || lower.starts_with("width:")
                || lower.starts_with("height:")
                || lower.starts_with("ratio:")
            {
                let (field, raw_value) = if lower.starts_with("size:") {
                    (NumField::Size, &token[5..])
                } else if lower.starts_with("width:") {
                    (NumField::Width, &token[6..])
                } else if lower.starts_with("height:") {
                    (NumField::Height, &token[7..])
                } else {
                    (NumField::Ratio, &token[6..])
                };
                let (op, value) = parse_comparison(raw_value);
                let parsed = if matches!(field, NumField::Size) {
                    parse_size_value(value)
                } else {
                    value.parse::<f64>().ok()
                };
                let Some(value) = parsed else {
                    query.warnings.push(format!("无法解析数值：{token}"));
                    continue;
                };
                let filter = NumericFilter { field, op, value };
                if negated {
                    query.exclude_numeric.push(filter);
                } else {
                    query.numeric.push(filter);
                }
            } else if lower.starts_with("date:") {
                let Some(filter) = parse_date_filter(&token[5..]) else {
                    query.warnings.push(format!("无法解析日期：{token}"));
                    continue;
                };
                if negated {
                    query.exclude_dates.push(filter);
                } else {
                    query.dates.push(filter);
                }
            } else if lower.starts_with("source:") {
                if lower[7..].trim() != "local" {
                    query.impossible = true;
                    query.warnings.push(format!("暂不支持远程来源：{token}"));
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
            } else if let Some((unknown, _)) = token.split_once(':')
                && !unknown.is_empty()
                && unknown.len() <= 20
                && unknown
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            {
                query
                    .warnings
                    .push(format!("未知搜索条件：{token}（按自由文本处理）"));
                if negated {
                    query.exclude_free.push(token.to_string());
                } else {
                    query.free.push(token.to_string());
                }
            } else if negated {
                query.exclude_free.push(token.to_string());
            } else {
                query.free.push(token.to_string());
            }
        }
        query
    }

    pub fn free_text(&self) -> String {
        self.free.join(" ")
    }

    pub(crate) fn parts(&self) -> SearchQueryParts<'_> {
        SearchQueryParts {
            free: &self.free,
            exclude_free: &self.exclude_free,
            tags: &self.tags,
            exclude_tags: &self.exclude_tags,
            scoped_tags: &self.scoped_tags,
            exclude_scoped_tags: &self.exclude_scoped_tags,
            groups: &self.groups,
            exclude_groups: &self.exclude_groups,
            names: &self.names,
            exclude_names: &self.exclude_names,
            paths: &self.paths,
            exclude_paths: &self.exclude_paths,
            rating: self.rating.as_deref(),
            exclude_rating: self.exclude_rating.as_deref(),
            tagged: self.tagged,
            favorite: self.favorite,
            failed: self.failed,
            damaged: self.damaged,
            star: self.star,
            star_min: self.star_min,
            star_max: self.star_max,
            exclude_star: self.exclude_star,
            numeric: &self.numeric,
            exclude_numeric: &self.exclude_numeric,
            dates: &self.dates,
            exclude_dates: &self.exclude_dates,
            duplicates_only: self.duplicates_only,
            impossible: self.impossible,
        }
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// 把标签筛选里的旧名/合并名替换成规范名；自由文本靠 FTS 的别名并集处理。
    pub fn resolve_tag_aliases(&mut self, aliases: &HashMap<String, String>) {
        if aliases.is_empty() {
            return;
        }
        for tag in self.tags.iter_mut().chain(self.exclude_tags.iter_mut()) {
            if let Some(canonical) = aliases.get(tag.as_str()) {
                *tag = canonical.clone();
            }
        }
        for tag in self
            .scoped_tags
            .iter_mut()
            .chain(self.exclude_scoped_tags.iter_mut())
        {
            if let Some(canonical) = aliases.get(tag.value.as_str()) {
                tag.value = canonical.clone();
            }
        }
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
        if let Some(failed) = self.failed {
            if asset.error.is_some() != failed {
                return false;
            }
        }
        if let Some(damaged) = self.damaged {
            if asset.damage.is_some() != damaged {
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
        if let Some(max) = self.star_max {
            if asset.star > max {
                return false;
            }
        }
        if let Some(star) = self.exclude_star {
            if asset.star == star {
                return false;
            }
        }
        for scoped in &self.scoped_tags {
            if !asset.all_tags().any(|hit| {
                hit.category.eq_ignore_ascii_case(&scoped.category)
                    && hit
                        .tag
                        .to_lowercase()
                        .contains(&scoped.value.to_lowercase())
            }) {
                return false;
            }
        }
        for scoped in &self.exclude_scoped_tags {
            if asset.all_tags().any(|hit| {
                hit.category.eq_ignore_ascii_case(&scoped.category)
                    && hit
                        .tag
                        .to_lowercase()
                        .contains(&scoped.value.to_lowercase())
            }) {
                return false;
            }
        }
        for group in &self.groups {
            if !asset.group.to_lowercase().contains(&group.to_lowercase()) {
                return false;
            }
        }
        for group in &self.exclude_groups {
            if asset.group.to_lowercase().contains(&group.to_lowercase()) {
                return false;
            }
        }
        for name in &self.names {
            if !asset.title().to_lowercase().contains(name) {
                return false;
            }
        }
        for name in &self.exclude_names {
            if asset.title().to_lowercase().contains(name) {
                return false;
            }
        }
        let asset_path = asset.path.to_string_lossy().to_lowercase();
        for path in &self.paths {
            if !asset_path.contains(path) {
                return false;
            }
        }
        for path in &self.exclude_paths {
            if asset_path.contains(path) {
                return false;
            }
        }
        for filter in &self.numeric {
            let actual = match filter.field {
                NumField::Size => asset.bytes as f64,
                NumField::Width => asset.width as f64,
                NumField::Height => asset.height as f64,
                NumField::Ratio => {
                    if asset.height == 0 {
                        return false;
                    }
                    asset.width as f64 / asset.height as f64
                }
            };
            if !compare_numbers(filter.op, actual, filter.value) {
                return false;
            }
        }
        for filter in &self.exclude_numeric {
            let actual = match filter.field {
                NumField::Size => Some(asset.bytes as f64),
                NumField::Width => Some(asset.width as f64),
                NumField::Height => Some(asset.height as f64),
                NumField::Ratio => {
                    (asset.height > 0).then(|| asset.width as f64 / asset.height as f64)
                }
            };
            if actual.is_some_and(|actual| compare_numbers(filter.op, actual, filter.value)) {
                return false;
            }
        }
        let captured = date_parts(asset.taken_or_modified());
        for filter in &self.dates {
            if !compare_date(filter.op, captured, filter) {
                return false;
            }
        }
        for filter in &self.exclude_dates {
            if compare_date(filter.op, captured, filter) {
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
        if !self.exclude_free.is_empty() {
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
            for token in &self.exclude_free {
                if haystack.contains(&token.to_lowercase()) {
                    return false;
                }
            }
        }
        true
    }
}

/// 智能集合保存的视图状态；应用集合时由调用方恢复排序、视图模式与窗格开关。
///
/// 旧库里的集合没有这些字段：迁移后按「默认排序 / 默认视图 / 侧栏与扩展面板可见」
/// 读取，因此空字符串与 `true` 是合法的历史语义，不是缺失值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedSearchView {
    /// 排序键；空字符串表示沿用当前排序（旧集合语义）。
    pub sort_key: String,
    pub sort_ascending: bool,
    /// 6 种视图之一的键；空字符串表示沿用当前视图。
    pub view_mode: String,
    /// 窗格模式（`pane_mode`）；空字符串表示沿用当前窗格。
    pub pane_mode: String,
    pub show_sidebar: bool,
    pub show_extensions: bool,
}

impl Default for SavedSearchView {
    fn default() -> Self {
        Self {
            sort_key: String::new(),
            sort_ascending: true,
            view_mode: String::new(),
            pane_mode: String::new(),
            show_sidebar: true,
            show_extensions: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SavedSearch {
    pub id: i64,
    pub name: String,
    pub filter: Filter,
    pub query: String,
    /// 保存时的排序/视图/窗格状态；旧集合为默认值。
    pub view: SavedSearchView,
    /// 置顶的集合排在未置顶之前。
    pub pinned: bool,
    /// 手动顺序：数值小的在前；相同再按 id。
    pub position: i64,
    /// 最近一次创建、重命名或用当前视图更新的 Unix 时间（秒）；0 表示迁移前的旧集合。
    pub updated_at: i64,
}

/// 手动相册（`collections` 表）及其资产数；同一资产可属于多个相册。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    /// 目前只有 `manual`；保留列以便将来区分动态相册。
    pub kind: String,
    pub asset_count: usize,
}

/// 同名冲突发生在哪类组织对象上。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrganizationKind {
    SavedSearch,
    Collection,
}

/// 同名冲突：调用方用 `downcast_ref::<DuplicateNameError>()` 判定，不依赖错误文案。
/// 空名称等参数错误仍走普通的 `anyhow` 错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateNameError {
    pub kind: OrganizationKind,
    pub name: String,
}

impl DuplicateNameError {
    pub fn new(kind: OrganizationKind, name: &str) -> Self {
        Self {
            kind,
            name: name.to_string(),
        }
    }
}

impl std::fmt::Display for DuplicateNameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            OrganizationKind::SavedSearch => {
                write!(formatter, "已存在同名智能集合「{}」", self.name)
            }
            OrganizationKind::Collection => write!(formatter, "已存在同名相册「{}」", self.name),
        }
    }
}

impl std::error::Error for DuplicateNameError {}

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

/// 完整扫描确认磁盘缺失、但数量较大时需要用户确认的清理批次。
#[derive(Clone, Debug)]
pub struct PendingCleanup {
    pub roots: Vec<PathBuf>,
    pub paths: HashSet<PathBuf>,
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
    /// 打标队列的运行时控制与进度；后台线程只通过这里读写。
    pub tag_status: TagQueueStatus,
    pub dedup: DedupState,
    pub saved_searches: Vec<SavedSearch>,
    /// 只影响侧栏标签面的显示，不修改资产标签本身。
    pub blocked_tags: HashSet<String>,
    /// 旧标签名/合并名 → 规范标签名；搜索与筛选会在内存里先映射一次。
    pub tag_aliases: HashMap<String, String>,
    /// 来源显示别名；键为根目录，只影响 UI，不改磁盘目录。
    pub source_aliases: HashMap<PathBuf, String>,
    /// 各来源最近一次扫描的运行时状态；键为绝对根目录，不落库。
    pub source_statuses: HashMap<PathBuf, SourceScanStatus>,
    /// 来源级扫描排除规则；键为绝对根目录，与 settings 的 `scan.filters` 同步。
    pub scan_filters: HashMap<PathBuf, ScanFilterConfig>,
    /// 持久忽略列表；与 settings 的 `library.ignore` 同步。
    pub ignore_rules: crate::scan_filter::IgnoreRules,
    /// 当前扫描的结构化进度；UI 只读。
    pub scan_progress: ScanProgress,
    /// 最近一次扫描结束后的结构化摘要。
    pub scan_summary: Option<ScanSummary>,
    /// 磁盘上已缺失、等待用户确认是否从图库移除的批量记录。
    pub pending_cleanup: Option<PendingCleanup>,
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
        let source_statuses = roots
            .iter()
            .map(|root| {
                (
                    root.clone(),
                    SourceScanStatus {
                        root: root.clone(),
                        online: root.is_dir(),
                        ..SourceScanStatus::default()
                    },
                )
            })
            .collect();
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
            tag_status: TagQueueStatus::default(),
            dedup,
            saved_searches: Vec::new(),
            blocked_tags: HashSet::new(),
            tag_aliases: HashMap::new(),
            source_aliases: HashMap::new(),
            source_statuses,
            scan_filters: HashMap::new(),
            ignore_rules: Default::default(),
            scan_progress: ScanProgress::default(),
            scan_summary: None,
            pending_cleanup: None,
        }
    }

    /// 单个来源的运行时状态；未初始化时返回 None。
    pub fn source_status(&self, root: &Path) -> Option<&SourceScanStatus> {
        self.source_statuses.get(root)
    }

    /// 取或创建来源状态；调用方应先确保 root 在 `roots` 中。
    pub fn source_status_mut(&mut self, root: &Path) -> &mut SourceScanStatus {
        let root = root.to_path_buf();
        self.source_statuses
            .entry(root.clone())
            .or_insert_with(|| SourceScanStatus {
                root: root.clone(),
                online: root.is_dir(),
                ..SourceScanStatus::default()
            })
    }

    pub fn set_source_status(&mut self, status: SourceScanStatus) {
        self.source_statuses.insert(status.root.clone(), status);
    }

    /// 刷新所有来源的在线状态与资产数，并按根目录排序返回快照。
    pub fn refresh_source_statuses(&mut self) -> Vec<SourceScanStatus> {
        let mut counts: HashMap<PathBuf, usize> = HashMap::new();
        {
            let assets = self.assets.read().unwrap();
            for asset in assets.iter() {
                *counts.entry(asset.library_root.clone()).or_default() += 1;
            }
        }
        self.source_statuses
            .retain(|root, _| self.roots.contains(root));
        let roots = self.roots.clone();
        for root in roots {
            let online = root.is_dir();
            let asset_count = counts.get(&root).copied().unwrap_or(0);
            let status = self
                .source_statuses
                .entry(root.clone())
                .or_insert_with(|| SourceScanStatus {
                    root: root.clone(),
                    ..SourceScanStatus::default()
                });
            status.root = root;
            status.online = online;
            status.asset_count = asset_count;
        }
        let mut list = self.source_statuses.values().cloned().collect::<Vec<_>>();
        list.sort_by(|a, b| a.root.cmp(&b.root));
        list
    }

    pub fn set_queue_counts(&mut self, pending: usize, running: usize, error: usize) {
        self.queue_pending = pending;
        self.queue_running = running;
        self.queue_error = error;
        self.tag_status.pending = pending;
        self.tag_status.running = running;
        self.tag_status.error = error;
    }

    pub fn pause_tag_queue(&mut self) {
        self.tag_status.paused = true;
        self.tag_status.cancelling = false;
        self.tag_status.message = Some("打标已暂停，当前批完成后不再认领新任务".into());
    }

    pub fn resume_tag_queue(&mut self) {
        self.tag_status.paused = false;
        self.tag_status.cancelling = false;
        self.tag_status.message = Some("打标已继续".into());
    }

    pub fn cancel_tag_queue(&mut self) {
        self.tag_status.cancelling = true;
        self.tag_status.paused = false;
        self.tag_status.message = Some("正在取消打标，已完成结果会保留".into());
    }

    /// 更新本轮进度并计算吞吐/ETA；`elapsed_secs` 为本轮累计耗时。
    pub fn set_tag_progress(
        &mut self,
        completed: usize,
        failed: usize,
        total: usize,
        elapsed_secs: f64,
    ) {
        let status = &mut self.tag_status;
        if total > 0 {
            status.total = total;
        }
        status.completed = completed;
        status.failed = failed;
        status.elapsed_secs = elapsed_secs;
        status.throughput_per_sec = if elapsed_secs > 0.0 {
            completed as f32 / elapsed_secs as f32
        } else {
            0.0
        };
        let remaining = status.total.saturating_sub(completed);
        status.eta_secs = if status.throughput_per_sec > 0.0 {
            Some((remaining as f32 / status.throughput_per_sec).ceil() as u64)
        } else {
            None
        };
    }

    pub fn set_tag_device(&mut self, device: &str) {
        self.tag_status.device = Some(device.to_string());
        self.tag_status.gpu_available = Some(device.to_ascii_lowercase().contains("cuda"));
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

    /// 文件被重命名/移动后，用同一评分与标签替换旧路径，避免重新打标。
    pub fn move_asset(&mut self, old_path: &Path, asset: Asset) {
        let new_path = asset.path.clone();
        {
            let mut assets = self.assets.write().unwrap();
            if let Some(index) = assets.iter().position(|item| item.path == old_path) {
                self.counts.remove(&assets[index]);
                self.counts.add(&asset);
                assets[index] = asset;
            } else {
                self.counts.add(&asset);
                assets.push(asset);
            }
        }
        if self.selected.as_deref() == Some(old_path) {
            self.selected = Some(new_path);
        }
        self.revision = self.revision.wrapping_add(1);
    }
}

pub const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "gif", "bmp", "tif", "tiff", "ico", "qoi", "exr", "avif",
];

/// 本构建里 `image` 没有解码器、只能交给 ffmpeg 的扩展名。
/// AVIF 的 `image` feature 只包含编码器；解码需要 `avif-native`（dav1d 原生库）。
pub const FFMPEG_ONLY_EXTENSIONS: &[&str] = &["avif"];

fn extension_lower(path: &Path) -> Option<String> {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
}

pub fn is_image(path: &Path) -> bool {
    extension_lower(path)
        .map(|ext| IMAGE_EXTENSIONS.iter().any(|known| *known == ext))
        .unwrap_or(false)
}

/// 仅能由 ffmpeg 解码的图片格式；`image` 回退对其不可用。
pub fn is_ffmpeg_only(path: &Path) -> bool {
    extension_lower(path)
        .map(|ext| FFMPEG_ONLY_EXTENSIONS.iter().any(|known| *known == ext))
        .unwrap_or(false)
}

/// 单个来源（本地根目录）最近一次扫描的运行时状态；不落库。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SourceScanStatus {
    /// 来源根目录（已规范化的绝对路径）。
    pub root: PathBuf,
    /// 最近一次扫描完成时间（Unix 秒）；None 表示尚未完整扫描。
    pub last_scan: Option<u64>,
    /// 该来源当前在图库中的资产数。
    pub asset_count: usize,
    /// 最近一次扫描的计数明细。
    pub counts: ScanCounts,
    /// 最近一次扫描错误；成功时为 None。
    pub error: Option<String>,
    /// 来源当前是否可访问；离线来源不参与缺失清理。
    pub online: bool,
}

/// 一次来源扫描的计数明细。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScanCounts {
    /// 遍历发现的图片文件数。
    pub discovered: usize,
    /// 新增入库数。
    pub imported: usize,
    /// 更新（mtime/size 变化）数。
    pub updated: usize,
    /// 未变化或命中忽略规则而跳过的文件数。
    #[serde(default)]
    pub skipped: usize,
    /// 磁盘缺失后从图库移除数。
    pub removed: usize,
    /// 读取/解码失败数。
    pub failed: usize,
}

/// 扫描阶段；UI 用它区分“遍历/哈希/缩略图/打标/对账”。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanPhase {
    #[default]
    Idle,
    Discovering,
    Hashing,
    Thumbnails,
    Tagging,
    Reconciling,
    Done,
    Failed,
}

/// 扫描进度快照；由核心线更新，UI 只读。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScanProgress {
    pub phase: ScanPhase,
    /// 当前阶段已完成数量。
    pub done: usize,
    /// 当前阶段总量；未知时为 0。
    pub total: usize,
    /// 当前处理的文件路径。
    pub current: Option<PathBuf>,
    /// 当前来源根目录。
    pub source: Option<PathBuf>,
    /// 当前来源在本次扫描中的序号（从 1 开始）。
    #[serde(default)]
    pub source_index: usize,
    /// 本次扫描的来源总数。
    #[serde(default)]
    pub source_total: usize,
    /// 每秒处理文件数；无法估算时为 0。
    pub throughput: f32,
    /// 预计剩余秒数；无法估算时为 None。
    pub eta_secs: Option<u64>,
}

/// 单个来源在一次扫描结束后的结构化结果。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanSourceSummary {
    pub root: PathBuf,
    pub counts: ScanCounts,
    /// 该来源本次扫描耗时（秒）。
    pub elapsed_secs: f64,
    /// 是否因用户停止而提前结束。
    pub cancelled: bool,
    /// 该来源扫描错误；成功时为 None。
    pub error: Option<String>,
}

/// 一次完整扫描（可能包含多个来源）结束后的结构化摘要。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanSummary {
    /// 遍历发现的图片文件数。
    pub files: usize,
    /// 新增入库数。
    pub imported: usize,
    /// 更新（mtime/size 变化）数。
    pub updated: usize,
    /// 未变化或命中忽略规则而跳过的文件数。
    pub skipped: usize,
    /// 磁盘缺失后从图库移除数。
    pub removed: usize,
    /// 读取/解码/打标失败数。
    pub failed: usize,
    /// 本次扫描总耗时（秒）。
    pub elapsed_secs: f64,
    /// 是否被用户停止。
    pub cancelled: bool,
    /// 按来源拆分的明细。
    pub sources: Vec<ScanSourceSummary>,
}

impl ScanSummary {
    /// 追加一个来源的扫描结果并累加总计。
    pub fn record(
        &mut self,
        root: &Path,
        counts: ScanCounts,
        elapsed_secs: f64,
        cancelled: bool,
        error: Option<String>,
    ) {
        self.files += counts.discovered;
        self.imported += counts.imported;
        self.updated += counts.updated;
        self.skipped += counts.skipped;
        self.removed += counts.removed;
        self.failed += counts.failed;
        self.cancelled |= cancelled;
        self.sources.push(ScanSourceSummary {
            root: root.to_path_buf(),
            counts,
            elapsed_secs,
            cancelled,
            error,
        });
    }
}

/// 关键表在备份库中的行数。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TableCount {
    pub table: String,
    pub rows: i64,
}

/// 一次数据库备份的结果与校验信息。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BackupReport {
    /// 源数据库路径。
    pub source: PathBuf,
    /// 备份文件路径。
    pub destination: PathBuf,
    /// 备份文件字节数。
    pub bytes: u64,
    /// 备份耗时（毫秒）。
    pub elapsed_ms: u64,
    /// 备份库关键表行数。
    pub tables: Vec<TableCount>,
    /// 备份库 `PRAGMA integrity_check` 是否通过。
    pub integrity_ok: bool,
    /// 校验差异或非致命错误说明。
    pub warnings: Vec<String>,
}

/// 扫描排除规则；按来源根目录序列化为 JSON 存于 settings。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanFilterConfig {
    /// 排除 glob（相对来源根目录）；如 `**/.git/**`、`*_cache`。
    pub exclude_globs: Vec<String>,
    /// 是否包含隐藏文件/目录（以 `.` 开头）。
    pub include_hidden: bool,
    /// 小于该字节数的文件跳过；None 不限制。
    pub min_bytes: Option<u64>,
    /// 大于该字节数的文件跳过；None 不限制。
    pub max_bytes: Option<u64>,
}

impl Default for ScanFilterConfig {
    /// 默认保持改造前的扫描语义：包含隐藏文件、不限制大小、无排除 glob。
    fn default() -> Self {
        Self {
            exclude_globs: Vec::new(),
            include_hidden: true,
            min_bytes: None,
            max_bytes: None,
        }
    }
}

impl ScanFilterConfig {
    /// 是否配置了任一过滤条件；未配置时扫描可走无过滤快路径。
    pub fn is_active(&self) -> bool {
        !self.exclude_globs.is_empty()
            || !self.include_hidden
            || self.min_bytes.is_some()
            || self.max_bytes.is_some()
    }
}

/// 诊断页展示的只读快照；不包含凭据与完整环境变量。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DiagnosticSnapshot {
    /// 应用版本（`CARGO_PKG_VERSION`）。
    pub app_version: String,
    /// 数据目录（通常为 exe 同级的 `.kanalib`）。
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub db_bytes: u64,
    pub thumb_cache_dir: PathBuf,
    pub thumb_cache_bytes: u64,
    /// 图库资产总数。
    pub asset_count: usize,
    pub queue_pending: usize,
    pub queue_running: usize,
    pub queue_error: usize,
    /// 各来源最近扫描状态。
    pub sources: Vec<SourceScanStatus>,
    /// 最近一次扫描时间（Unix 秒）。
    pub last_scan: Option<u64>,
    /// 最近一次错误（扫描/任务），可为空。
    pub last_error: Option<String>,
    /// 日志文件路径；未启用文件日志时为 None。
    pub log_path: Option<PathBuf>,
    /// 推理设备（如 `cuda`/`cpu`）；未知时为 None。
    pub sidecar_device: Option<String>,
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
            damage: None,
            sha1: None,
            blake3: None,
            phash: None,
            bytes: 0,
            mtime: 0,
            captured_at: None,
            metadata_json: String::new(),
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
        let mut a = asset("a.png", true, "g", "原创", vec![tag("1girl", "general")]);
        a.manual_tags.push(tag("custom_tag", "manual"));
        let counts = CountAccumulator::from_assets(&[a.clone()]);
        assert_eq!(
            counts.view().tags,
            vec![("1girl".to_string(), 1), ("custom_tag".to_string(), 1)]
        );
        assert!(Filter::Tag("custom_tag".to_string()).matches(&a));
        let query = SearchQuery::parse("tag:custom_tag");
        assert!(query.matches(&a, None, None));
        assert_eq!(a.top_tags(2).len(), 2);
    }

    #[test]
    fn extended_search_syntax_filters_categories_status_numbers_and_dates() {
        let mut a = asset(
            "a.png",
            true,
            "g",
            "作品: 崩坏",
            vec![
                tag("clara", "character"),
                tag("honkai:_star_rail", "copyright"),
                tag("1girl", "general"),
            ],
        );
        a.bytes = 12 * 1024 * 1024;
        a.width = 4000;
        a.height = 3000;
        a.star = 4;
        a.favorite = true;
        a.mtime = 1_792_065_600;
        a.damage = Some("truncated".into());

        let query = SearchQuery::parse(
            "character:clara copyright:honkai group:崩坏 is:fav star:>=3 size:>10mb width:>=4000 ratio:>1.3 date:2026-10",
        );
        assert!(query.matches(&a, None, None), "{:?}", query.warnings());
        assert!(query.warnings().is_empty());
        assert!(SearchQuery::parse("is:damaged").matches(&a, None, None));
        assert!(!SearchQuery::parse("is:failed").matches(&a, None, None));
        assert!(!SearchQuery::parse("size:<10mb").matches(&a, None, None));
        assert!(!SearchQuery::parse("date:2026-09").matches(&a, None, None));
        assert!(SearchQuery::parse("date:>=2026-10-01 date:<=2026-10-31").matches(&a, None, None));
        assert!(!SearchQuery::parse("-clara").matches(&a, None, None));
        assert!(SearchQuery::parse("-nothere").matches(&a, None, None));
        assert!(SearchQuery::parse("star:<=4").matches(&a, None, None));
        assert!(!SearchQuery::parse("-star:>=4").matches(&a, None, None));
        assert!(
            !SearchQuery::parse("character:clara -copyright:star_rail").matches(&a, None, None)
        );
        assert!(!SearchQuery::parse("foo:bar").warnings().is_empty());
        assert!(!SearchQuery::parse("date:2026-13").warnings().is_empty());
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
    fn queue_status_tracks_pause_cancel_throughput_and_eta() {
        let mut shared = Shared::new(
            vec![PathBuf::from("lib")],
            Vec::new(),
            Default::default(),
            crate::dedup::DEFAULT_SIMILAR_THRESHOLD,
            Vec::new(),
        );
        shared.set_queue_counts(10, 0, 2);
        assert_eq!(
            (shared.tag_status.pending, shared.tag_status.error),
            (10, 2)
        );

        shared.pause_tag_queue();
        assert!(shared.tag_status.paused);
        shared.set_tag_progress(4, 1, 10, 2.0);
        assert_eq!(shared.tag_status.completed, 4);
        assert!((shared.tag_status.throughput_per_sec - 2.0).abs() < f32::EPSILON);
        assert_eq!(shared.tag_status.eta_secs, Some(3));

        shared.resume_tag_queue();
        assert!(!shared.tag_status.paused);

        shared.cancel_tag_queue();
        assert!(shared.tag_status.cancelling);
        assert!(!shared.tag_status.paused);
    }

    #[test]
    fn thresholds_resolve_category_and_tag_overrides() {
        let mut thresholds = TagThresholds::default();
        assert!((thresholds.threshold("general", "1girl") - 0.17).abs() < 1e-6);
        thresholds.overrides.insert("1girl".to_string(), 0.5);
        assert!((thresholds.threshold("general", "1girl") - 0.5).abs() < 1e-6);
        assert!((thresholds.threshold("general", "other") - 0.17).abs() < 1e-6);
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

    #[test]
    fn image_extensions_accept_new_formats_case_insensitively() {
        for name in [
            "a.jpg", "a.JPEG", "a.png", "a.webp", "a.gif", "a.bmp", "a.tif", "a.TIFF", "a.ico",
            "a.qoi", "a.exr", "a.avif",
        ] {
            assert!(is_image(Path::new(name)), "{name} should be an image");
        }
        for name in ["a.heic", "a.dng", "a.psd", "a.txt", "noext"] {
            assert!(!is_image(Path::new(name)), "{name} should not be an image");
        }
        assert!(is_ffmpeg_only(Path::new("a.AVIF")));
        assert!(!is_ffmpeg_only(Path::new("a.tiff")));
    }
    #[test]
    fn day_number_round_trips_date_parts() {
        for (year, month, day) in [
            (1970, 1, 1),
            (2024, 2, 29),
            (2000, 2, 29),
            (2023, 12, 31),
            (2024, 3, 1),
        ] {
            let number = day_number(year, month, day);
            assert_eq!(date_parts((number as u64) * 86_400), (year, month, day));
        }
        assert!(day_number(2024, 2, 28) < day_number(2024, 2, 29));
        assert!(day_number(2024, 2, 29) < day_number(2024, 3, 1));
    }

}
