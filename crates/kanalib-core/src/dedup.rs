use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::model::{Asset, SimilarHit, SimilarQueryResult, SimilarThresholdPreview};

pub const DEFAULT_SIMILAR_THRESHOLD: u8 = 10;
/// 相似阈值语义与去重 6/10/14 预设统一：64 位 pHash 汉明距离 ≤ 阈值。
pub const MIN_SIMILAR_THRESHOLD: u8 = 0;
pub const MAX_SIMILAR_THRESHOLD: u8 = 32;
pub const SIMILAR_THRESHOLD_PRESETS: [u8; 3] = [6, 10, 14];
/// 已存 pHash 数不超过该规模时，阈值预览与全库分组完全一致（精确计算）。
pub const PREVIEW_EXACT_LIMIT: usize = 4096;
/// 超过 [`PREVIEW_EXACT_LIMIT`] 时的抽样种子数；采样比例 = 该值 / 已存 pHash 数。
pub const PREVIEW_SAMPLE_LIMIT: usize = 4096;
/// 链式合并判定每个组件最多看多少个成员，避免高阈值下的组爆炸。
const PREVIEW_CHAIN_MEMBERS: usize = 256;
const PREVIEW_SAMPLE_SALT: u64 = 0x6A09_E667_F3BC_C908;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupKind {
    #[default]
    Exact,
    Similar,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewState {
    #[default]
    Pending,
    Ignored,
    Related,
    Done,
}

impl ReviewState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ignored => "ignored",
            Self::Related => "related",
            Self::Done => "done",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "ignored" => Self::Ignored,
            "related" => Self::Related,
            "done" => Self::Done,
            _ => Self::Pending,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DuplicateReview {
    pub group_key: String,
    pub state: ReviewState,
    pub keeper_path: Option<PathBuf>,
    pub member_count: usize,
    pub signature: String,
}

#[derive(Clone, Debug)]
pub struct DuplicateGroup {
    pub key: String,
    pub kind: GroupKind,
    pub assets: Vec<Asset>,
    pub state: ReviewState,
    pub keeper: Option<PathBuf>,
    pub suggested: PathBuf,
    pub suggested_reason: String,
    pub member_count: usize,
    pub signature: String,
    pub total_bytes: u64,
}

impl DuplicateGroup {
    pub fn keeper_asset(&self) -> Option<&Asset> {
        self.keeper
            .as_ref()
            .and_then(|path| self.assets.iter().find(|asset| &asset.path == path))
    }

    pub fn suggested_asset(&self) -> Option<&Asset> {
        self.assets
            .iter()
            .find(|asset| asset.path == self.suggested)
    }

    pub fn reclaimable(&self) -> u64 {
        self.total_bytes
            .saturating_sub(self.keeper_asset().map(|asset| asset.bytes).unwrap_or(0))
    }

    pub fn title(&self) -> String {
        self.keeper_asset()
            .or_else(|| self.assets.first())
            .map(Asset::title)
            .unwrap_or_else(|| self.key.clone())
    }

    pub fn duplicate_paths(&self) -> Vec<PathBuf> {
        let keeper = self.keeper.as_ref();
        self.assets
            .iter()
            .filter(|asset| Some(&asset.path) != keeper)
            .map(|asset| asset.path.clone())
            .collect()
    }

    pub fn distance_to_keeper(&self, asset: &Asset) -> Option<u32> {
        if self.kind != GroupKind::Similar {
            return None;
        }
        let keeper = self.keeper_asset().or_else(|| self.suggested_asset())?;
        phash_distance(keeper.phash?, asset.phash?)
    }

    pub fn similarity_pct(&self) -> Option<u32> {
        if self.kind != GroupKind::Similar {
            return None;
        }
        let keeper = self.keeper_asset().or_else(|| self.suggested_asset())?;
        let distance = self
            .assets
            .iter()
            .filter(|asset| asset.path != keeper.path)
            .filter_map(|asset| phash_distance(keeper.phash?, asset.phash?))
            .max()?;
        Some(similarity_pct_from_distance(distance))
    }
}

pub fn phash_distance(a: u64, b: u64) -> Option<u32> {
    Some((a ^ b).count_ones())
}

pub fn is_protected(path: &Path, protected: &[PathBuf]) -> bool {
    protected.iter().any(|root| path.starts_with(root))
}

fn pick_protected_keeper(assets: &[Asset], protected: &[PathBuf]) -> Option<PathBuf> {
    let mut candidates = assets
        .iter()
        .filter(|asset| is_protected(&asset.path, protected))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| {
        (b.width as u64 * b.height as u64, b.bytes, b.tags.len())
            .cmp(&(
                a.width as u64 * a.height as u64,
                a.bytes,
                a.tags.len(),
            ))
            .then_with(|| a.mtime.cmp(&b.mtime))
            .then_with(|| a.path.cmp(&b.path))
    });
    Some(candidates[0].path.clone())
}

pub fn similarity_pct_from_distance(distance: u32) -> u32 {
    100 - (distance.min(64) * 100 / 64)
}

/// 把任意 `u8` 阈值收进 0–32 的相似语义；超过 32 的距离对去重没有意义。
pub fn clamp_similar_threshold(threshold: u8) -> u8 {
    threshold.min(MAX_SIMILAR_THRESHOLD)
}

/// 以单张已入库图片为锚点查找相似图。
///
/// 只读取 [`Asset::phash`]（库内已存值，来自扫描阶段的 pHash），不重新解码、
/// 也不重算哈希；返回除锚点自身外汉明距离 `≤ threshold` 的资产，按距离升序、
/// 同距离按路径升序排列，相似度百分比统一走 [`similarity_pct_from_distance`]。
///
/// 语义与全库分组解耦：每条命中只表示「与锚点的直接距离」，不做链式合并，
/// 因此同 `sha1` 的精确副本会以距离 0 出现在结果里（全库分组会把它们归入精确重复组）。
/// `limit` 为 0 时返回空 `hits`；命中数超过 `limit` 时 `truncated` 为真。
pub fn similar_to_anchor(
    assets: &[Asset],
    anchor: &Path,
    threshold: u8,
    limit: usize,
) -> Result<SimilarQueryResult> {
    let threshold = clamp_similar_threshold(threshold);
    let anchor_index = assets
        .iter()
        .position(|asset| asset.path == anchor)
        .with_context(|| format!("anchor not found in library: {}", anchor.display()))?;
    let anchor_phash = assets[anchor_index]
        .phash
        .with_context(|| format!("anchor has no pHash: {}", anchor.display()))?;

    let mut scanned = 0usize;
    let mut matches: Vec<(u32, &Path)> = Vec::new();
    for (index, asset) in assets.iter().enumerate() {
        let Some(phash) = asset.phash else {
            continue;
        };
        scanned += 1;
        if index == anchor_index {
            continue;
        }
        let distance = (anchor_phash ^ phash).count_ones();
        if distance <= u32::from(threshold) {
            matches.push((distance, asset.path.as_path()));
        }
    }
    let truncated = matches.len() > limit;
    let matches = if truncated {
        top_matches(matches, limit)
    } else {
        matches.sort_unstable_by(compare_hits);
        matches
    };
    let hits = matches
        .into_iter()
        .map(|(distance, path)| SimilarHit {
            path: path.to_path_buf(),
            distance,
            similarity_pct: similarity_pct_from_distance(distance),
        })
        .collect();
    Ok(SimilarQueryResult {
        anchor: anchor.to_path_buf(),
        threshold,
        scanned,
        hits,
        truncated,
    })
}

fn compare_hits(a: &(u32, &Path), b: &(u32, &Path)) -> std::cmp::Ordering {
    a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1))
}

/// 只保留按「距离升序、同距离路径升序」排在最前的 `limit` 条。
///
/// 直接对全部命中按路径比较排序在 10 万库阈值 32 下要 0.4–0.6s（6 万条同距离命中，
/// 每次比较都要随机访问路径字符串）；这里先按距离分桶（33 个桶）定位截断距离，
/// 只在截断距离这一桶里用 quickselect 取路径最小的若干条，最后只对 ≤ `limit`
/// 条做完整排序。结果与全量排序再截断逐条一致。
fn top_matches(matches: Vec<(u32, &Path)>, limit: usize) -> Vec<(u32, &Path)> {
    if limit == 0 {
        return Vec::new();
    }
    let mut counts = [0usize; MAX_SIMILAR_THRESHOLD as usize + 1];
    for (distance, _) in &matches {
        let bucket = (*distance as usize).min(MAX_SIMILAR_THRESHOLD as usize);
        counts[bucket] += 1;
    }
    let mut remaining = limit;
    let mut cutoff = 0usize;
    for (distance, count) in counts.iter().enumerate() {
        if *count >= remaining {
            cutoff = distance;
            break;
        }
        remaining -= *count;
    }
    let mut below = Vec::with_capacity(limit);
    let mut ties = Vec::new();
    for entry in matches {
        match entry.0 as usize {
            distance if distance < cutoff => below.push(entry),
            distance if distance == cutoff => ties.push(entry),
            _ => {}
        }
    }
    if ties.len() > remaining {
        ties.select_nth_unstable_by(remaining, |a, b| a.1.cmp(b.1));
        ties.truncate(remaining);
    }
    below.extend(ties);
    below.sort_unstable_by(compare_hits);
    below
}

/// 预览某个候选阈值下全库相似去重会得到多少组、多少成员。
///
/// 阈值语义与 [`compute_similar_groups`] 完全一致（同为「汉明距离 ≤ 阈值」的
/// 单链接连通分量，且跳过同 `sha1` 的配对）；只读估算，不改阈值、分组与评审状态。
///
/// - 已存 pHash 数 `≤ PREVIEW_EXACT_LIMIT`：精确计算，与全扫结果逐项一致。
/// - 更大的图库：按 `PREVIEW_SAMPLE_LIMIT` 个种子做**星形抽样**（每个种子与全库比较，
///   只保留种子发出的边），抽样比例记录在 `sampled_assets / total_assets`：
///   - 组数用 Horvitz–Thompson 加权，权重 `1 / (1 - (1 - f)^组内观测成员数)`；
///   - 成员数按「有邻居的种子占比 / f」做元素级估计（在 10 万库上与全扫偏差 < 1%）。
///   - `chain_risk` 在样本内判定：同一组件里存在两个成员的直接距离仍大于阈值。
/// - **密集链式合并回退**：若样本里既有链式合并、又是多个组、且 ≥25% 的资产落在
///   相似组内，说明真实组件被抽样切碎、组数估计不可信（10 万级合成库上曾高估 20 倍），
///   此时直接跑一次精确全扫，保证「预览数量与实扫一致」；此时 `sampled` 为假，
///   代价与现有全库相似扫描同量级。只剩一个巨型组时抽样本身是准的，不回退。
///   稀疏库（重复占比 < 25%）仍走抽样，偏差见 `benchmark_preview_synthetic_100k`。
pub fn preview_similar_threshold(assets: &[Asset], threshold: u8) -> SimilarThresholdPreview {
    let threshold = clamp_similar_threshold(threshold);
    let threshold_bits = u32::from(threshold);
    let items = assets
        .iter()
        .filter(|asset| asset.phash.is_some())
        .collect::<Vec<_>>();
    let total = items.len();
    if total < 2 {
        return SimilarThresholdPreview {
            threshold,
            group_count: 0,
            member_count: 0,
            chain_risk: false,
            chain_groups: 0,
            sampled_assets: total,
            total_assets: total,
            sampled: false,
        };
    }
    let hashes = items
        .iter()
        .map(|asset| asset.phash.unwrap_or_default())
        .collect::<Vec<_>>();

    if total <= PREVIEW_EXACT_LIMIT {
        return exact_preview(&items, &hashes, threshold);
    }

    let seeds = preview_seed_indices(total, PREVIEW_SAMPLE_LIMIT);
    let fraction = seeds.len() as f64 / total as f64;
    let sha_ids = intern_sha_ids(&items);
    let (mut union, captured) = preview_union(&hashes, &sha_ids, &seeds, threshold_bits);
    let seed_roots = seeds
        .iter()
        .map(|&index| union.find(index))
        .collect::<HashSet<_>>();
    let components = preview_components(&hashes, &mut union, Some(&seed_roots), threshold_bits);

    let mut group_estimate = 0.0f64;
    for component in &components {
        let capture = 1.0 - (1.0 - fraction).powi(component.size as i32);
        group_estimate += 1.0 / capture.max(f64::MIN_POSITIVE);
    }
    let grouped_seeds = captured.iter().filter(|captured| **captured).count();
    let member_estimate = (grouped_seeds as f64 / fraction).round() as usize;
    let chain_groups = components
        .iter()
        .filter(|component| component.chain)
        .count();

    if chain_groups > 0 && group_estimate > 2.0 && member_estimate.saturating_mul(4) >= total {
        return exact_preview(&items, &hashes, threshold);
    }

    SimilarThresholdPreview {
        threshold,
        group_count: (group_estimate.round() as usize).min(total / 2),
        member_count: member_estimate.min(total),
        chain_risk: chain_groups > 0,
        chain_groups,
        sampled_assets: seeds.len(),
        total_assets: total,
        sampled: true,
    }
}

/// 精确预览：与 [`compute_similar_groups`] 的组数/成员数逐项一致。
fn exact_preview(items: &[&Asset], hashes: &[u64], threshold: u8) -> SimilarThresholdPreview {
    let total = hashes.len();
    let mut union = similar_union(items, u32::from(threshold));
    let components = preview_components(hashes, &mut union, None, u32::from(threshold));
    let group_count = components.len();
    let member_count = components.iter().map(|component| component.size).sum();
    let chain_groups = components
        .iter()
        .filter(|component| component.chain)
        .count();
    SimilarThresholdPreview {
        threshold,
        group_count,
        member_count,
        chain_risk: chain_groups > 0,
        chain_groups,
        sampled_assets: total,
        total_assets: total,
        sampled: false,
    }
}

struct PreviewComponent {
    size: usize,
    chain: bool,
}

/// 按连通分量汇总；`root_filter` 只保留包含抽样的根，用于星形抽样的样本内统计。
fn preview_components(
    hashes: &[u64],
    union: &mut UnionFind,
    root_filter: Option<&HashSet<usize>>,
    threshold: u32,
) -> Vec<PreviewComponent> {
    let mut components: HashMap<usize, (usize, Vec<usize>)> = HashMap::new();
    for index in 0..hashes.len() {
        let root = union.find(index);
        if root_filter.is_some_and(|roots| !roots.contains(&root)) {
            continue;
        }
        let component = components.entry(root).or_insert_with(|| (0, Vec::new()));
        component.0 += 1;
        if component.1.len() < PREVIEW_CHAIN_MEMBERS {
            component.1.push(index);
        }
    }
    components
        .into_values()
        .filter(|(size, _)| *size >= 2)
        .map(|(size, members)| PreviewComponent {
            size,
            chain: component_has_chain(&members, hashes, threshold),
        })
        .collect()
}

/// 组件里是否存在「两端直接距离仍大于阈值」的成员对（即发生了链式合并）。
/// 只看前 [`PREVIEW_CHAIN_MEMBERS`] 个成员，超大组件按样本判定。
fn component_has_chain(members: &[usize], hashes: &[u64], threshold: u32) -> bool {
    for (position, &left) in members.iter().enumerate() {
        for &right in &members[position + 1..] {
            if (hashes[left] ^ hashes[right]).count_ones() > threshold {
                return true;
            }
        }
    }
    false
}

/// 按 `splitmix64(index ^ salt)` 排序取前 `sample` 个，得到与路径无关、可复现的均匀抽样。
fn preview_seed_indices(count: usize, sample: usize) -> Vec<usize> {
    let sample = sample.min(count);
    if sample == count {
        return (0..count).collect();
    }
    let mut scored = (0..count)
        .map(|index| (splitmix64(index as u64 ^ PREVIEW_SAMPLE_SALT), index))
        .collect::<Vec<_>>();
    scored.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    scored.truncate(sample);
    scored.sort_unstable_by_key(|entry| entry.1);
    scored.into_iter().map(|entry| entry.1).collect()
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

/// 星形抽样的两两比较：每个种子与全库比一遍，只保留种子发出的边。
/// 各线程持独立 UnionFind，最后按行合并；样本图是真实图的子图，不会跨组误连。
/// 返回值第二项与 `seeds` 一一对应：该种子是否至少有一个邻居（用于估计成员数）。
fn preview_union(
    hashes: &[u64],
    sha_ids: &[u32],
    seeds: &[usize],
    threshold: u32,
) -> (UnionFind, Vec<bool>) {
    let count = hashes.len();
    let scan = |start: usize, end: usize| {
        let mut union = UnionFind::new(count);
        let mut captured = vec![false; end - start];
        for (offset, &seed) in seeds[start..end].iter().enumerate() {
            let hash = hashes[seed];
            let sha = sha_ids[seed];
            for node in 0..count {
                if node == seed || (hash ^ hashes[node]).count_ones() > threshold {
                    continue;
                }
                if sha == sha_ids[node] {
                    continue;
                }
                union.union(seed, node);
                captured[offset] = true;
            }
        }
        (union, captured)
    };
    let workers = similar_workers(seeds.len(), 256);
    if workers <= 1 {
        return scan(0, seeds.len());
    }
    let chunk = seeds.len().div_ceil(workers);
    let partials = std::thread::scope(|scope| {
        (0..workers)
            .map(|worker| {
                let start = worker * chunk;
                let end = ((worker + 1) * chunk).min(seeds.len());
                let scan = &scan;
                scope.spawn(move || scan(start, end))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().expect("preview worker panicked"))
            .collect::<Vec<_>>()
    });
    let mut merged = UnionFind::new(count);
    let mut captured = Vec::with_capacity(seeds.len());
    for (mut partial, flags) in partials {
        for index in 0..count {
            let root = partial.find(index);
            if root != index {
                merged.union(index, root);
            }
        }
        captured.extend(flags);
    }
    (merged, captured)
}

#[derive(Clone, Debug, Default)]
pub struct DedupSummary {
    pub total: usize,
    pub pending: usize,
    pub pending_assets: usize,
    pub ignored: usize,
    pub related: usize,
    pub done: usize,
    pub pending_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct DedupState {
    pub groups: Arc<Vec<DuplicateGroup>>,
    pub similar_groups: Arc<Vec<DuplicateGroup>>,
    pub similar_scanned: bool,
    pub similar_scanning: bool,
    pub similar_threshold: u8,
    pub protected_paths: Vec<PathBuf>,
    reviews: HashMap<String, DuplicateReview>,
}

impl Default for DedupState {
    fn default() -> Self {
        Self {
            groups: Arc::new(Vec::new()),
            similar_groups: Arc::new(Vec::new()),
            similar_scanned: false,
            similar_scanning: false,
            similar_threshold: DEFAULT_SIMILAR_THRESHOLD,
            protected_paths: Vec::new(),
            reviews: HashMap::new(),
        }
    }
}

impl DedupState {
    pub fn with_reviews_and_threshold(
        reviews: HashMap<String, DuplicateReview>,
        threshold: u8,
    ) -> Self {
        Self {
            similar_threshold: threshold,
            reviews,
            ..Default::default()
        }
    }

    pub fn rebuild(&mut self, assets: &[Asset]) {
        self.rebuild_exact(assets);
        self.clear_similar();
    }

    pub fn rebuild_exact(&mut self, assets: &[Asset]) {
        let groups = build_exact_groups(
            assets,
            &self.reviews,
            &self.groups,
            &self.protected_paths,
        );
        self.groups = Arc::new(groups);
    }

    pub fn set_protected_paths(&mut self, paths: Vec<PathBuf>) {
        self.protected_paths = paths;
    }

    pub fn clear_groups(&mut self) {
        self.groups = Arc::new(Vec::new());
        self.clear_similar();
    }

    pub fn clear_similar(&mut self) {
        self.similar_groups = Arc::new(Vec::new());
        self.similar_scanned = false;
        self.similar_scanning = false;
    }

    pub fn prune_similar(&mut self, removed: &HashSet<PathBuf>) {
        let groups = Arc::make_mut(&mut self.similar_groups);
        groups.retain(|group| {
            !group
                .assets
                .iter()
                .any(|asset| removed.contains(&asset.path))
        });
    }

    pub fn set_similar_groups(&mut self, groups: Vec<DuplicateGroup>, threshold: u8) {
        self.similar_groups = Arc::new(groups);
        self.similar_threshold = threshold;
        self.similar_scanned = true;
        self.similar_scanning = false;
    }

    pub fn group(&self, key: &str) -> Option<&DuplicateGroup> {
        self.groups
            .iter()
            .chain(self.similar_groups.iter())
            .find(|group| group.key == key)
    }

    pub fn review_snapshot(&self) -> HashMap<String, DuplicateReview> {
        self.reviews.clone()
    }

    fn groups_mut(&mut self, kind: GroupKind) -> &mut Arc<Vec<DuplicateGroup>> {
        match kind {
            GroupKind::Exact => &mut self.groups,
            GroupKind::Similar => &mut self.similar_groups,
        }
    }

    pub fn set_keeper(
        &mut self,
        key: &str,
        path: &Path,
        kind: GroupKind,
    ) -> Option<DuplicateReview> {
        let groups = Arc::make_mut(self.groups_mut(kind));
        let group = groups.iter_mut().find(|group| group.key == key)?;
        if !group.assets.iter().any(|asset| asset.path == path) {
            return None;
        }
        group.keeper = Some(path.to_path_buf());
        let review = DuplicateReview {
            group_key: group.key.clone(),
            state: group.state,
            keeper_path: group.keeper.clone(),
            member_count: group.member_count,
            signature: group.signature.clone(),
        };
        self.reviews.insert(review.group_key.clone(), review.clone());
        Some(review)
    }

    pub fn set_state(
        &mut self,
        key: &str,
        state: ReviewState,
        kind: GroupKind,
    ) -> Option<DuplicateReview> {
        let groups = Arc::make_mut(self.groups_mut(kind));
        let group = groups.iter_mut().find(|group| group.key == key)?;
        group.state = state;
        let review = DuplicateReview {
            group_key: group.key.clone(),
            state,
            keeper_path: group.keeper.clone(),
            member_count: group.member_count,
            signature: group.signature.clone(),
        };
        self.reviews.insert(review.group_key.clone(), review.clone());
        Some(review)
    }

    pub fn preselect(&mut self, kind: GroupKind) -> usize {
        let groups = Arc::make_mut(self.groups_mut(kind));
        let mut selected = 0;
        for group in groups
            .iter_mut()
            .filter(|group| group.state == ReviewState::Pending)
        {
            group.keeper = Some(group.suggested.clone());
            selected += 1;
        }
        selected
    }

    pub fn summary(&self, kind: GroupKind) -> DedupSummary {
        let groups = match kind {
            GroupKind::Exact => &self.groups,
            GroupKind::Similar => &self.similar_groups,
        };
        let mut summary = DedupSummary {
            total: groups.len(),
            ..Default::default()
        };
        for group in groups.iter() {
            match group.state {
                ReviewState::Pending => {
                    summary.pending += 1;
                    summary.pending_assets += group.assets.len();
                    summary.pending_bytes =
                        summary.pending_bytes.saturating_add(group.reclaimable());
                }
                ReviewState::Ignored => summary.ignored += 1,
                ReviewState::Related => summary.related += 1,
                ReviewState::Done => summary.done += 1,
            }
        }
        summary
    }
}

fn build_exact_groups(
    assets: &[Asset],
    reviews: &HashMap<String, DuplicateReview>,
    old_groups: &[DuplicateGroup],
    protected: &[PathBuf],
) -> Vec<DuplicateGroup> {
    let mut by_hash: HashMap<&str, Vec<Asset>> = HashMap::new();
    for asset in assets {
        let Some(sha1) = asset.sha1.as_deref().filter(|value| !value.is_empty()) else {
            continue;
        };
        by_hash.entry(sha1).or_default().push(asset.clone());
    }

    let old_keepers = keeper_map(old_groups);
    let mut groups = Vec::new();
    for (sha1, mut members) in by_hash {
        if members.len() < 2 {
            continue;
        }
        members.sort_by(|a, b| a.path.cmp(&b.path));
        let (suggested, suggested_reason) = suggest_exact_keeper(&members, protected);
        groups.push(make_group(
            GroupKind::Exact,
            sha1.to_string(),
            members,
            reviews,
            &old_keepers,
            suggested,
            suggested_reason,
            protected,
        ));
    }
    sort_groups(&mut groups);
    groups
}

pub fn compute_similar_groups(
    assets: &[Asset],
    threshold: u8,
    reviews: &HashMap<String, DuplicateReview>,
    old_groups: &[DuplicateGroup],
    protected: &[PathBuf],
) -> Vec<DuplicateGroup> {
    let items = assets
        .iter()
        .filter(|asset| asset.phash.is_some())
        .collect::<Vec<_>>();
    let threshold = clamp_similar_threshold(threshold) as u32;
    let mut union = similar_union(&items, threshold);

    let mut components: HashMap<usize, Vec<Asset>> = HashMap::new();
    for (index, asset) in items.iter().enumerate() {
        components
            .entry(union.find(index))
            .or_default()
            .push((*asset).clone());
    }

    let old_keepers = keeper_map(old_groups);
    let mut groups = Vec::new();
    for (_, mut members) in components {
        if members.len() < 2 {
            continue;
        }
        members.sort_by(|a, b| a.path.cmp(&b.path));
        let signature = similar_signature(&members);
        let key = format!("sim:{signature}");
        let (suggested, suggested_reason) = suggest_similar_keeper(&members, protected);
        groups.push(make_group(
            GroupKind::Similar,
            key,
            members,
            reviews,
            &old_keepers,
            suggested,
            suggested_reason,
            protected,
        ));
    }
    sort_groups(&mut groups);
    groups
}

fn make_group(
    kind: GroupKind,
    key: String,
    members: Vec<Asset>,
    reviews: &HashMap<String, DuplicateReview>,
    old_keepers: &HashMap<String, PathBuf>,
    suggested: PathBuf,
    suggested_reason: String,
    protected: &[PathBuf],
) -> DuplicateGroup {
    let signature = group_signature(kind, &members);
    let member_count = members.len();
    let review = reviews.get(&key);
    let mut state = review.map(|review| review.state).unwrap_or_default();
    if state == ReviewState::Done
        && review.is_some_and(|review| {
            review.member_count != member_count || review.signature != signature
        })
    {
        state = ReviewState::Pending;
    }
    let old_keeper = old_keepers
        .get(&key)
        .cloned()
        .or_else(|| review.and_then(|review| review.keeper_path.clone()))
        .filter(|path| members.iter().any(|asset| &asset.path == path));
    let has_protected = members
        .iter()
        .any(|asset| is_protected(&asset.path, protected));
    let keeper = if has_protected {
        old_keeper
            .filter(|path| is_protected(path, protected))
            .unwrap_or_else(|| suggested.clone())
    } else {
        old_keeper.unwrap_or_else(|| suggested.clone())
    };
    let total_bytes = members.iter().map(|asset| asset.bytes).sum();
    DuplicateGroup {
        key,
        kind,
        assets: members,
        state,
        keeper: Some(keeper),
        suggested,
        suggested_reason,
        member_count,
        signature,
        total_bytes,
    }
}

fn keeper_map(groups: &[DuplicateGroup]) -> HashMap<String, PathBuf> {
    groups
        .iter()
        .filter_map(|group| {
            group
                .keeper
                .as_ref()
                .map(|path| (group.key.clone(), path.clone()))
        })
        .collect()
}

fn sort_groups(groups: &mut Vec<DuplicateGroup>) {
    groups.sort_by(|a, b| {
        b.reclaimable()
            .cmp(&a.reclaimable())
            .then_with(|| a.title().cmp(&b.title()))
    });
}

fn member_signature(assets: &[Asset]) -> String {
    let mut hasher = blake3::Hasher::new();
    for asset in assets {
        hasher.update(asset.path.to_string_lossy().as_bytes());
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex()[..16].to_string()
}

fn similar_signature(assets: &[Asset]) -> String {
    let mut hasher = blake3::Hasher::new();
    for asset in assets {
        hasher.update(asset.path.to_string_lossy().as_bytes());
        hasher.update(&asset.phash.unwrap_or(0).to_le_bytes());
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex()[..16].to_string()
}

fn group_signature(kind: GroupKind, assets: &[Asset]) -> String {
    match kind {
        GroupKind::Exact => member_signature(assets),
        GroupKind::Similar => similar_signature(assets),
    }
}

fn suggest_exact_keeper(assets: &[Asset], protected: &[PathBuf]) -> (PathBuf, String) {
    if let Some(path) = pick_protected_keeper(assets, protected) {
        return (path, "保护路径".to_string());
    }
    let max_tags = assets.iter().map(|asset| asset.tags.len()).max().unwrap_or(0);
    let min_tags = assets.iter().map(|asset| asset.tags.len()).min().unwrap_or(0);
    if max_tags > 0 && max_tags != min_tags {
        let mut tagged = assets
            .iter()
            .filter(|asset| asset.tags.len() == max_tags)
            .collect::<Vec<_>>();
        if tagged.len() == 1 {
            return (tagged[0].path.clone(), format!("标签最多（{max_tags} 个）"));
        }
        tagged.sort_by(|a, b| a.path.cmp(&b.path));
        return (
            tagged[0].path.clone(),
            format!("标签最多（并列 {max_tags} 个）"),
        );
    }
    fallback_keeper(assets)
}

fn suggest_similar_keeper(assets: &[Asset], protected: &[PathBuf]) -> (PathBuf, String) {
    if let Some(path) = pick_protected_keeper(assets, protected) {
        return (path, "保护路径".to_string());
    }
    let max_area = assets
        .iter()
        .map(|asset| asset.width as u64 * asset.height as u64)
        .max()
        .unwrap_or(0);
    let min_area = assets
        .iter()
        .map(|asset| asset.width as u64 * asset.height as u64)
        .min()
        .unwrap_or(0);
    if max_area > 0 && max_area != min_area {
        let candidates = assets
            .iter()
            .filter(|asset| asset.width as u64 * asset.height as u64 == max_area)
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            return (candidates[0].path.clone(), "分辨率最高".to_string());
        }
    }

    let max_bytes = assets.iter().map(|asset| asset.bytes).max().unwrap_or(0);
    let min_bytes = assets.iter().map(|asset| asset.bytes).min().unwrap_or(0);
    if max_bytes > 0 && max_bytes != min_bytes {
        let candidates = assets
            .iter()
            .filter(|asset| asset.bytes == max_bytes)
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            return (candidates[0].path.clone(), "文件更大".to_string());
        }
    }

    let max_tags = assets.iter().map(|asset| asset.tags.len()).max().unwrap_or(0);
    let min_tags = assets.iter().map(|asset| asset.tags.len()).min().unwrap_or(0);
    if max_tags > 0 && max_tags != min_tags {
        let candidates = assets
            .iter()
            .filter(|asset| asset.tags.len() == max_tags)
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            return (candidates[0].path.clone(), format!("标签最多（{max_tags} 个）"));
        }
    }

    fallback_keeper(assets)
}

fn fallback_keeper(assets: &[Asset]) -> (PathBuf, String) {
    let oldest = assets
        .iter()
        .map(|asset| asset.mtime)
        .filter(|mtime| *mtime > 0)
        .min();
    if let Some(oldest) = oldest {
        let candidates = assets
            .iter()
            .filter(|asset| asset.mtime == oldest)
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            return (candidates[0].path.clone(), "最早导入".to_string());
        }
    }

    let mut shortest = assets.iter().collect::<Vec<_>>();
    shortest.sort_by(|a, b| {
        a.path
            .as_os_str()
            .len()
            .cmp(&b.path.as_os_str().len())
            .then_with(|| a.path.cmp(&b.path))
    });
    (shortest[0].path.clone(), "路径最短".to_string())
}

struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

fn intern_sha_ids(items: &[&Asset]) -> Vec<u32> {
    let mut interned: HashMap<&str, u32> = HashMap::new();
    items
        .iter()
        .map(|asset| match asset.sha1.as_deref() {
            None => u32::MAX,
            Some(sha1) => {
                let next = interned.len() as u32;
                *interned.entry(sha1).or_insert(next)
            }
        })
        .collect()
}

fn similar_workers(count: usize, per_worker: usize) -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .min(16)
        .min(count / per_worker + 1)
}

fn merge_partials(partials: &mut [UnionFind], count: usize) -> UnionFind {
    let mut merged = UnionFind::new(count);
    for partial in partials.iter_mut() {
        for i in 0..count {
            let root = partial.find(i);
            if root != i {
                merged.union(i, root);
            }
        }
    }
    merged
}

/// 相似判定是「64 位 pHash 汉明距离 ≤ 阈值」的配对：10k 档 5,000 万对、10 万档 50 亿对。
/// 按 i 区间切给多个线程、各持一份 UnionFind，最后按每行根节点合并；
/// sha1 先驻留成 u32，内层循环只剩异或+popcount，单对约 1–2ns。
/// 语义与全量两两一致（含「sha1 相同则跳过」）。
/// 曾按 czkawka 的方案试过 BK-tree（`bk-tree` crate）：100k 合成哈希、阈值 10/14 下
/// 38.7s / 66.5s，比本实现（1.8s）慢 20–37 倍，故不引入该依赖。
fn similar_union(items: &[&Asset], threshold: u32) -> UnionFind {
    let count = items.len();
    if count < 2 {
        return UnionFind::new(count);
    }
    let hashes = items
        .iter()
        .map(|asset| asset.phash.unwrap_or_default())
        .collect::<Vec<_>>();
    let sha_ids = intern_sha_ids(items);
    let scan = |start: usize, end: usize| {
        let mut union = UnionFind::new(count);
        for i in start..end {
            let a = hashes[i];
            let sha_a = sha_ids[i];
            for j in (i + 1)..count {
                if sha_a == sha_ids[j] {
                    continue;
                }
                if (a ^ hashes[j]).count_ones() <= threshold {
                    union.union(i, j);
                }
            }
        }
        union
    };
    let workers = similar_workers(count, 2048);
    if workers <= 1 {
        return scan(0, count);
    }
    let chunk = count.div_ceil(workers);
    let mut partials = std::thread::scope(|scope| {
        (0..workers)
            .map(|worker| {
                let start = worker * chunk;
                let end = ((worker + 1) * chunk).min(count);
                let scan = &scan;
                scope.spawn(move || scan(start, end))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().expect("similar scan worker panicked"))
            .collect::<Vec<_>>()
    });
    merge_partials(&mut partials, count)
}

impl UnionFind {
    fn new(size: usize) -> Self {
        Self {
            parent: (0..size).collect(),
            rank: vec![0; size],
        }
    }

    fn find(&mut self, index: usize) -> usize {
        if self.parent[index] != index {
            self.parent[index] = self.find(self.parent[index]);
        }
        self.parent[index]
    }

    fn union(&mut self, a: usize, b: usize) {
        let root_a = self.find(a);
        let root_b = self.find(b);
        if root_a == root_b {
            return;
        }
        match self.rank[root_a].cmp(&self.rank[root_b]) {
            std::cmp::Ordering::Less => self.parent[root_a] = root_b,
            std::cmp::Ordering::Greater => self.parent[root_b] = root_a,
            std::cmp::Ordering::Equal => {
                self.parent[root_b] = root_a;
                self.rank[root_a] += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TagHit;

    fn asset(path: &str, tags: usize, mtime: u64) -> Asset {
        asset_with(path, "aaa", 1, 100, mtime, 100, 100, tags)
    }

    #[allow(clippy::too_many_arguments)]
    fn asset_with(
        path: &str,
        sha1: &str,
        phash: u64,
        bytes: u64,
        mtime: u64,
        width: u32,
        height: u32,
        tags: usize,
    ) -> Asset {
        Asset {
            path: PathBuf::from(path),
            library_root: PathBuf::from("lib"),
            thumb: None,
            tile_thumb: None,
            width,
            height,
            group: String::new(),
            rating: String::new(),
            favorite: false,
            star: 0,
            manual_tags: Vec::new(),
            tags: (0..tags)
                .map(|index| TagHit {
                    tag: format!("tag{index}"),
                    category: "general".to_string(),
                    score: 1.0,
                    vocab_index: None,
                })
                .collect(),
            tagged: tags > 0,
            tag_attempts: 0,
            error: None,
            damage: None,
            sha1: Some(sha1.to_string()),
            blake3: Some("bbb".to_string()),
            phash: Some(phash),
            bytes,
            mtime,
            captured_at: None,
            metadata_json: String::new(),
        }
    }

    #[test]
    fn groups_exact_duplicates_and_uses_default_keeper() {
        let assets = vec![asset("C:/lib/a.png", 5, 20), asset("C:/lib/b.png", 1, 10)];
        let groups = build_exact_groups(&assets, &HashMap::new(), &[], &[]);
        assert_eq!(groups.len(), 1);
        let group = &groups[0];
        assert_eq!(group.kind, GroupKind::Exact);
        assert_eq!(group.member_count, 2);
        assert_eq!(group.keeper.as_deref(), Some(Path::new("C:/lib/a.png")));
        assert_eq!(group.suggested_reason, "标签最多（5 个）");
        assert_eq!(group.reclaimable(), 100);
    }

    #[test]
    fn done_group_reopens_when_membership_changes() {
        let assets = vec![asset("C:/lib/a.png", 5, 20), asset("C:/lib/b.png", 1, 10)];
        let groups = build_exact_groups(&assets, &HashMap::new(), &[], &[]);
        let review = DuplicateReview {
            group_key: "aaa".to_string(),
            state: ReviewState::Done,
            keeper_path: Some(PathBuf::from("C:/lib/a.png")),
            member_count: 1,
            signature: groups[0].signature.clone(),
        };
        let reviews = HashMap::from([("aaa".to_string(), review)]);
        let rebuilt = build_exact_groups(&assets, &reviews, &groups, &[]);
        assert_eq!(rebuilt[0].state, ReviewState::Pending);
    }

    #[test]
    fn ignored_group_stays_ignored_after_rebuild() {
        let assets = vec![asset("C:/lib/a.png", 5, 20), asset("C:/lib/b.png", 1, 10)];
        let groups = build_exact_groups(&assets, &HashMap::new(), &[], &[]);
        let review = DuplicateReview {
            group_key: "aaa".to_string(),
            state: ReviewState::Ignored,
            keeper_path: Some(PathBuf::from("C:/lib/a.png")),
            member_count: 2,
            signature: groups[0].signature.clone(),
        };
        let reviews = HashMap::from([("aaa".to_string(), review)]);
        let rebuilt = build_exact_groups(&assets, &reviews, &groups, &[]);
        assert_eq!(rebuilt[0].state, ReviewState::Ignored);
    }

    #[test]
    fn keeper_choice_is_saved_as_a_pending_review() {
        let assets = vec![asset("C:/lib/a.png", 5, 20), asset("C:/lib/b.png", 1, 10)];
        let mut state = DedupState::default();
        state.rebuild(&assets);
        let key = state.groups[0].key.clone();
        let review = state
            .set_keeper(&key, Path::new("C:/lib/b.png"), GroupKind::Exact)
            .expect("keeper review");
        assert_eq!(review.state, ReviewState::Pending);
        assert_eq!(review.keeper_path.as_deref(), Some(Path::new("C:/lib/b.png")));
        state.rebuild(&assets);
        assert_eq!(
            state.groups[0].keeper.as_deref(),
            Some(Path::new("C:/lib/b.png"))
        );
    }

    #[test]
    fn equal_tag_counts_fall_back_to_the_oldest_file() {
        let assets = vec![asset("C:/lib/a.png", 2, 20), asset("C:/lib/b.png", 2, 10)];
        let groups = build_exact_groups(&assets, &HashMap::new(), &[], &[]);
        assert_eq!(groups[0].keeper.as_deref(), Some(Path::new("C:/lib/b.png")));
        assert_eq!(groups[0].suggested_reason, "最早导入");
    }

    #[test]
    fn similar_groups_use_phash_distance_and_ignore_exact_copies() {
        let assets = vec![
            asset_with("C:/lib/a.png", "a", 0b0000_0000, 100, 30, 100, 100, 1),
            asset_with("C:/lib/a-copy.png", "a", 0b0000_0000, 100, 5, 100, 100, 1),
            asset_with("C:/lib/b.png", "b", 0b1111_0000, 200, 20, 200, 200, 2),
            asset_with("C:/lib/c.png", "c", 0b1111_0001, 300, 10, 300, 300, 3),
        ];
        let groups = compute_similar_groups(&assets, 2, &HashMap::new(), &[], &[]);
        assert_eq!(groups.len(), 1);
        let group = &groups[0];
        assert_eq!(group.kind, GroupKind::Similar);
        assert!(group.key.starts_with("sim:"));
        assert_eq!(group.assets.len(), 2);
        assert!(group.assets.iter().any(|asset| asset.path.ends_with("b.png")));
        assert!(group.assets.iter().any(|asset| asset.path.ends_with("c.png")));
        assert_eq!(group.suggested_reason, "分辨率最高");
        assert!(group.similarity_pct().is_some());
    }

    #[test]
    fn similar_threshold_controls_group_size() {
        let assets = vec![
            asset_with("C:/lib/a.png", "a", 0b0000, 100, 30, 100, 100, 1),
            asset_with("C:/lib/b.png", "b", 0b0011, 100, 20, 100, 100, 1),
            asset_with("C:/lib/c.png", "c", 0b1111, 100, 10, 100, 100, 1),
        ];
        let tight = compute_similar_groups(&assets, 1, &HashMap::new(), &[], &[]);
        assert!(tight.is_empty());
        let loose = compute_similar_groups(&assets, 4, &HashMap::new(), &[], &[]);
        assert_eq!(loose.len(), 1);
        assert_eq!(loose[0].assets.len(), 3);
    }

    #[test]
    fn related_state_survives_similar_rebuild() {
        let assets = vec![
            asset_with("C:/lib/a.png", "a", 0b0000, 100, 30, 100, 100, 1),
            asset_with("C:/lib/b.png", "b", 0b0011, 100, 20, 100, 100, 1),
        ];
        let groups = compute_similar_groups(&assets, 2, &HashMap::new(), &[], &[]);
        let review = DuplicateReview {
            group_key: groups[0].key.clone(),
            state: ReviewState::Related,
            keeper_path: Some(PathBuf::from("C:/lib/a.png")),
            member_count: 2,
            signature: groups[0].signature.clone(),
        };
        let mut state = DedupState::with_reviews_and_threshold(
            HashMap::from([(review.group_key.clone(), review)]),
            DEFAULT_SIMILAR_THRESHOLD,
        );
        let rebuilt = compute_similar_groups(&assets, 2, &state.reviews, &[], &[]);
        state.similar_groups = Arc::new(rebuilt);
        assert_eq!(state.similar_groups[0].state, ReviewState::Related);
    }

    #[test]
    fn protected_path_wins_keeper_selection() {
        let assets = vec![
            asset_with("C:/protected/a.png", "same", 1, 100, 20, 100, 100, 1),
            asset_with("C:/other/b.png", "same", 1, 100, 10, 300, 300, 5),
        ];
        let protected = [PathBuf::from("C:/protected")];
        let groups = build_exact_groups(&assets, &HashMap::new(), &[], &protected);
        assert_eq!(
            groups[0].keeper.as_deref(),
            Some(Path::new("C:/protected/a.png"))
        );
        assert_eq!(groups[0].suggested_reason, "保护路径");
        assert!(is_protected(
            Path::new("C:/protected/a.png"),
            &protected
        ));
        assert!(!is_protected(Path::new("C:/other/b.png"), &protected));
    }

    #[test]
    fn anchor_query_orders_hits_by_distance_and_percentage() {
        let assets = vec![
            asset_with("C:/lib/anchor.png", "anchor", 0, 100, 10, 100, 100, 0),
            asset_with("C:/lib/same.png", "anchor", 0, 100, 10, 100, 100, 0),
            asset_with("C:/lib/near.png", "near", 0b1, 100, 10, 100, 100, 0),
            asset_with("C:/lib/five.png", "five", 0b1_1111, 100, 10, 100, 100, 0),
            asset_with(
                "C:/lib/edge32.png",
                "edge32",
                0xFFFF_FFFF,
                100,
                10,
                100,
                100,
                0,
            ),
            asset_with(
                "C:/lib/far33.png",
                "far33",
                0x1_FFFF_FFFF,
                100,
                10,
                100,
                100,
                0,
            ),
            asset_with("C:/lib/far64.png", "far64", u64::MAX, 100, 10, 100, 100, 0),
        ];
        let result = similar_to_anchor(&assets, Path::new("C:/lib/anchor.png"), 32, 10).unwrap();
        assert_eq!(result.threshold, 32);
        assert_eq!(result.scanned, 7);
        assert!(!result.truncated);
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.distance)
                .collect::<Vec<_>>(),
            vec![0, 1, 5, 32]
        );
        assert!(
            result
                .hits
                .iter()
                .all(|hit| hit.similarity_pct == similarity_pct_from_distance(hit.distance))
        );
        assert_eq!(result.hits[0].similarity_pct, 100);
        assert_eq!(result.hits[3].similarity_pct, 50);
        assert!(
            result.hits.iter().any(|hit| hit.path.ends_with("same.png")),
            "同 sha1 的精确副本应以距离 0 出现在锚点查询里"
        );
        assert!(
            result
                .hits
                .iter()
                .all(|hit| !hit.path.ends_with("anchor.png"))
        );
    }

    #[test]
    fn anchor_query_covers_threshold_zero_and_thirty_two() {
        let assets = vec![
            asset_with("C:/lib/anchor.png", "anchor", 0, 100, 10, 100, 100, 0),
            asset_with("C:/lib/same.png", "same", 0, 100, 10, 100, 100, 0),
            asset_with("C:/lib/near.png", "near", 0b1, 100, 10, 100, 100, 0),
            asset_with(
                "C:/lib/edge32.png",
                "edge32",
                0xFFFF_FFFF,
                100,
                10,
                100,
                100,
                0,
            ),
            asset_with(
                "C:/lib/far33.png",
                "far33",
                0x1_FFFF_FFFF,
                100,
                10,
                100,
                100,
                0,
            ),
        ];

        let zero = similar_to_anchor(&assets, Path::new("C:/lib/anchor.png"), 0, 10).unwrap();
        assert_eq!(zero.threshold, 0);
        assert_eq!(
            zero.hits
                .iter()
                .map(|hit| hit.path.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            vec!["C:/lib/same.png".to_string()]
        );
        assert_eq!(zero.hits[0].similarity_pct, 100);

        let top32 = similar_to_anchor(&assets, Path::new("C:/lib/anchor.png"), 32, 10).unwrap();
        assert_eq!(
            top32
                .hits
                .iter()
                .map(|hit| hit.distance)
                .collect::<Vec<_>>(),
            vec![0, 1, 32],
            "距离 32 是边界内、33 是边界外"
        );
        assert_eq!(
            top32.hits[2].similarity_pct,
            similarity_pct_from_distance(32)
        );

        let clamped = similar_to_anchor(&assets, Path::new("C:/lib/anchor.png"), 255, 10).unwrap();
        assert_eq!(clamped.threshold, MAX_SIMILAR_THRESHOLD);
        assert_eq!(clamped.hits.len(), 3);

        let limited = similar_to_anchor(&assets, Path::new("C:/lib/anchor.png"), 32, 1).unwrap();
        assert_eq!(limited.hits.len(), 1);
        assert_eq!(limited.hits[0].distance, 0);
        assert!(limited.truncated);

        let none = similar_to_anchor(&assets, Path::new("C:/lib/anchor.png"), 32, 0).unwrap();
        assert!(none.hits.is_empty());
        assert!(none.truncated);
    }

    #[test]
    fn anchor_query_rejects_missing_anchor_or_phash() {
        let mut assets = vec![
            asset_with("C:/lib/anchor.png", "anchor", 0, 100, 10, 100, 100, 0),
            asset_with("C:/lib/no-phash.png", "none", 0, 100, 10, 100, 100, 0),
        ];
        assets[1].phash = None;
        assert!(similar_to_anchor(&assets, Path::new("C:/lib/missing.png"), 10, 10).is_err());
        assert!(similar_to_anchor(&assets, Path::new("C:/lib/no-phash.png"), 10, 10).is_err());
    }

    #[test]
    fn preview_exact_counts_match_full_groups() {
        let assets = vec![
            asset_with("C:/lib/a.png", "a", 0b0000, 100, 10, 100, 100, 0),
            asset_with("C:/lib/b.png", "b", 0b0011, 100, 10, 100, 100, 0),
            asset_with("C:/lib/c.png", "c", 0b1111, 100, 10, 100, 100, 0),
            asset_with("C:/lib/d.png", "d", 0b1_0000, 100, 10, 100, 100, 0),
            asset_with("C:/lib/e.png", "e", 0b1_0001, 100, 10, 100, 100, 0),
            asset_with("C:/lib/f.png", "f", 0xFFFF_FFFF, 100, 10, 100, 100, 0),
        ];
        for threshold in [0u8, 1, 2, 4, 6, 10, 14, 32] {
            let preview = preview_similar_threshold(&assets, threshold);
            let groups = compute_similar_groups(&assets, threshold, &HashMap::new(), &[], &[]);
            assert!(!preview.sampled, "threshold={threshold}");
            assert_eq!(preview.sampled_assets, assets.len());
            assert_eq!(preview.total_assets, assets.len());
            assert_eq!(preview.group_count, groups.len(), "threshold={threshold}");
            assert_eq!(
                preview.member_count,
                groups.iter().map(|group| group.assets.len()).sum::<usize>(),
                "threshold={threshold}"
            );
            assert_eq!(
                preview.chain_risk,
                preview.chain_groups > 0,
                "threshold={threshold}"
            );
        }
        let zero = preview_similar_threshold(&assets, 0);
        assert_eq!((zero.group_count, zero.member_count), (0, 0));
        assert!(!zero.chain_risk);
    }

    #[test]
    fn preview_flags_chain_risk_at_low_thresholds() {
        // 0 → 3 → 15：相邻距离 2、两端距离 4。阈值 2 时三者被链式合并成一组。
        let assets = vec![
            asset_with("C:/lib/a.png", "a", 0b0000, 100, 10, 100, 100, 0),
            asset_with("C:/lib/b.png", "b", 0b0011, 100, 10, 100, 100, 0),
            asset_with("C:/lib/c.png", "c", 0b1111, 100, 10, 100, 100, 0),
            asset_with("C:/lib/x.png", "x", 0xFFFF_FFFF, 100, 10, 100, 100, 0),
            asset_with("C:/lib/y.png", "y", 0xFFFF_FFFE, 100, 10, 100, 100, 0),
        ];
        let chained = preview_similar_threshold(&assets, 2);
        let groups = compute_similar_groups(&assets, 2, &HashMap::new(), &[], &[]);
        assert_eq!(chained.group_count, groups.len());
        assert!(chained.chain_risk);
        assert!(chained.chain_groups > 0);

        let direct = preview_similar_threshold(&assets, 4);
        assert!(!direct.chain_risk);
        assert_eq!(direct.chain_groups, 0);
        assert_eq!(direct.group_count, 2);
        assert_eq!(direct.member_count, 5);
    }

    #[test]
    fn preview_samples_large_libraries_and_scales_counts() {
        // 全库同哈希（不同 sha1）：一个巨型组；抽样模式应仍报 1 组、成员数接近全库。
        let count = PREVIEW_EXACT_LIMIT + 500;
        let assets = (0..count)
            .map(|index| {
                asset_with(
                    &format!("C:/lib/{index:05}.png"),
                    &format!("sha{index}"),
                    42,
                    100,
                    10,
                    100,
                    100,
                    0,
                )
            })
            .collect::<Vec<_>>();
        let preview = preview_similar_threshold(&assets, 0);
        assert!(preview.sampled);
        assert_eq!(preview.total_assets, count);
        assert_eq!(preview.sampled_assets, PREVIEW_SAMPLE_LIMIT);
        assert_eq!(preview.group_count, 1);
        assert_eq!(preview.member_count, count);
        assert!(!preview.chain_risk);
    }

    #[test]
    fn preview_matches_full_scan_on_dense_random_library() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut assets: Vec<Asset> = Vec::new();
        for index in 0..1_200usize {
            // 每 9 张复制上一张的哈希（制造大量近距离对），其余随机。
            let phash = if index > 0 && index % 9 == 0 {
                assets[index - 1].phash.unwrap_or_default()
            } else {
                next()
            };
            assets.push(asset_with(
                &format!("C:/lib/p{index}.jpg"),
                &format!("sha{index}"),
                phash,
                100,
                10,
                100,
                100,
                0,
            ));
        }
        for threshold in [0u8, 2, 6, 10, 14, 32] {
            let preview = preview_similar_threshold(&assets, threshold);
            let groups = compute_similar_groups(&assets, threshold, &HashMap::new(), &[], &[]);
            assert!(!preview.sampled);
            assert_eq!(preview.group_count, groups.len(), "threshold={threshold}");
            assert_eq!(
                preview.member_count,
                groups.iter().map(|group| group.assets.len()).sum::<usize>(),
                "threshold={threshold}"
            );
        }
    }

    #[test]
    fn truncated_anchor_query_matches_full_sort_then_truncate() {
        // 三档同距离命中 + 路径乱序，覆盖「截断距离落在同距离大桶里」的取前 N 条。
        let mut assets = vec![asset_with(
            "C:/lib/m-anchor.png",
            "anchor",
            0,
            100,
            10,
            100,
            100,
            0,
        )];
        for index in 0..80usize {
            let phash = match index % 3 {
                0 => 0b11,
                1 => 0b1111,
                _ => 0b1111_1111,
            };
            let name = if index % 2 == 0 { 'z' } else { 'a' };
            assets.push(asset_with(
                &format!("C:/lib/{name}{index:03}.png"),
                &format!("sha{index}"),
                phash,
                100,
                10,
                100,
                100,
                0,
            ));
        }
        let anchor = Path::new("C:/lib/m-anchor.png");
        let full = similar_to_anchor(&assets, anchor, 8, usize::MAX).unwrap();
        assert_eq!(full.hits.len(), 80);
        assert!(
            full.hits
                .windows(2)
                .all(|window| (window[0].distance, window[0].path.as_path())
                    <= (window[1].distance, window[1].path.as_path()))
        );
        for limit in [0usize, 1, 7, 26, 27, 53, 54, 79, 80] {
            let limited = similar_to_anchor(&assets, anchor, 8, limit).unwrap();
            let expected = &full.hits[..limit.min(full.hits.len())];
            assert_eq!(limited.hits, expected, "limit={limit}");
            assert_eq!(limited.truncated, limit < full.hits.len(), "limit={limit}");
        }
    }

    #[test]
    fn threshold_clamp_and_similarity_mapping_cover_boundaries() {
        assert_eq!(MIN_SIMILAR_THRESHOLD, 0);
        assert_eq!(MAX_SIMILAR_THRESHOLD, 32);
        assert_eq!(SIMILAR_THRESHOLD_PRESETS, [6, 10, 14]);
        assert_eq!(clamp_similar_threshold(0), 0);
        assert_eq!(clamp_similar_threshold(6), 6);
        assert_eq!(clamp_similar_threshold(32), 32);
        assert_eq!(clamp_similar_threshold(33), 32);
        assert_eq!(clamp_similar_threshold(u8::MAX), 32);
        assert_eq!(similarity_pct_from_distance(0), 100);
        assert_eq!(similarity_pct_from_distance(32), 50);
        assert_eq!(similarity_pct_from_distance(64), 0);
    }

    #[test]
    fn threshold_change_keeps_review_state() {
        let assets = vec![
            asset_with("C:/lib/a.png", "a", 0b0000, 100, 30, 100, 100, 1),
            asset_with("C:/lib/b.png", "b", 0b0011, 100, 20, 100, 100, 1),
        ];
        let groups = compute_similar_groups(&assets, 2, &HashMap::new(), &[], &[]);
        let review = DuplicateReview {
            group_key: groups[0].key.clone(),
            state: ReviewState::Done,
            keeper_path: Some(PathBuf::from("C:/lib/a.png")),
            member_count: groups[0].member_count,
            signature: groups[0].signature.clone(),
        };
        let mut state = DedupState::with_reviews_and_threshold(
            HashMap::from([(review.group_key.clone(), review)]),
            DEFAULT_SIMILAR_THRESHOLD,
        );
        state.set_similar_groups(groups, DEFAULT_SIMILAR_THRESHOLD);
        // 仅改阈值：评审快照不变，重建后仍是 Done。
        state.similar_threshold = 0;
        state.similar_threshold = 32;
        assert_eq!(state.review_snapshot().len(), 1);
        let rebuilt = compute_similar_groups(&assets, 32, &state.reviews, &[], &[]);
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(rebuilt[0].state, ReviewState::Done);
    }

    #[test]
    #[ignore = "10k similar-scan benchmark"]
    fn benchmark_similar_10k() {
        let mut assets = Vec::with_capacity(10_000);
        for index in 0..10_000u64 {
            let path = format!("C:/lib/{index:05}.png");
            let sha1 = format!("{index:040x}");
            let phash = index
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .rotate_left(17);
            assets.push(asset_with(
                &path, &sha1, phash, 100, 10, 100, 100, 0,
            ));
        }
        for threshold in [6u8, 10, 14] {
            let started = std::time::Instant::now();
            let groups = compute_similar_groups(&assets, threshold, &HashMap::new(), &[], &[]);
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            println!(
                "similar 10k: threshold={threshold} groups={} elapsed_ms={elapsed_ms:.0}",
                groups.len(),
            );
        }
    }

    #[test]
    fn parallel_similar_matches_serial_reference() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        // 5,000 条以覆盖多线程路径（worker 数 = count/2048+1）。
        let mut assets: Vec<Asset> = Vec::new();
        for index in 0..5_000usize {
            let mut phash = next();
            if index > 0 && index % 7 == 0 {
                phash = assets[index - 1].phash.unwrap_or_default() ^ (next() % 8);
            }
            let sha1 = if index % 11 == 0 {
                "dup".to_string()
            } else {
                format!("sha{index}")
            };
            assets.push(asset_with(
                &format!("C:/lib/p{index}.jpg"),
                &sha1,
                phash,
                100,
                100,
                100,
                100,
                0,
            ));
        }
        assets[5].sha1 = None;
        assets[6].sha1 = None;
        for threshold in [0u8, 6, 10, 14, 32] {
            let expected = brute_force_components(&assets, threshold);
            let groups = compute_similar_groups(&assets, threshold, &HashMap::new(), &[], &[]);
            let mut actual = groups
                .iter()
                .map(|group| {
                    let mut paths = group
                        .assets
                        .iter()
                        .map(|asset| asset.path.display().to_string())
                        .collect::<Vec<_>>();
                    paths.sort();
                    paths
                })
                .collect::<Vec<_>>();
            actual.sort();
            assert_eq!(actual, expected, "threshold={threshold}");
        }
    }

    #[test]
    #[ignore = "100k similar-scan benchmark"]
    fn benchmark_similar_100k() {
        let mut assets = Vec::with_capacity(100_000);
        for index in 0..100_000u64 {
            let path = format!("C:/lib/{index:06}.png");
            let sha1 = format!("{index:040x}");
            let phash = index
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .rotate_left(17);
            assets.push(asset_with(&path, &sha1, phash, 100, 10, 100, 100, 0));
        }
        for threshold in [10u8, 14] {
            let started = std::time::Instant::now();
            let groups = compute_similar_groups(&assets, threshold, &HashMap::new(), &[], &[]);
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            println!(
                "similar 100k: threshold={threshold} groups={} elapsed_ms={elapsed_ms:.0}",
                groups.len(),
            );
        }
    }

    fn load_bench_assets(db: &Path) -> Vec<Asset> {
        let connection =
            rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("open benchmark db read-only");
        let mut statement = connection
            .prepare(
                "SELECT path, phash, sha1 FROM assets
                 WHERE phash IS NOT NULL AND deleted_at IS NULL
                 ORDER BY path",
            )
            .expect("prepare benchmark query");
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .expect("query benchmark rows");
        rows.map(|row| {
            let (path, phash, sha1) = row.expect("benchmark row");
            asset_with(
                &path,
                sha1.as_deref().unwrap_or_default(),
                phash as u64,
                100,
                10,
                100,
                100,
                0,
            )
        })
        .collect()
    }

    fn percentile(sorted: &[f64], percent: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let rank = ((percent / 100.0) * sorted.len() as f64).ceil() as usize;
        sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
    }

    /// 单锚点查询基准（默认跳过）：
    /// `KANALIB_BENCH_DB=<db 副本> cargo test -p kanalib-core -- --ignored --nocapture benchmark_anchor_query`
    #[test]
    #[ignore = "100k anchor-query benchmark (KANALIB_BENCH_DB)"]
    fn benchmark_anchor_query_100k() {
        let Some(db) = std::env::var_os("KANALIB_BENCH_DB") else {
            eprintln!("KANALIB_BENCH_DB 未设置，跳过基准");
            return;
        };
        let assets = load_bench_assets(Path::new(&db));
        println!("anchor benchmark assets={}", assets.len());
        let limit = 500usize;
        let sample = 100usize;
        let step = (assets.len() / sample).max(1);
        let anchors = (0..sample)
            .map(|index| assets[index * step].path.clone())
            .collect::<Vec<_>>();
        for threshold in [6u8, 10, 14, 32] {
            for anchor in anchors.iter().take(5) {
                let _ = similar_to_anchor(&assets, anchor, threshold, limit);
            }
            let mut timings = Vec::with_capacity(anchors.len());
            let mut hit_total = 0usize;
            for anchor in &anchors {
                let started = std::time::Instant::now();
                let result =
                    similar_to_anchor(&assets, anchor, threshold, limit).expect("anchor query");
                timings.push(started.elapsed().as_secs_f64() * 1000.0);
                hit_total += result.hits.len();
            }
            timings.sort_by(|a, b| a.partial_cmp(b).unwrap());
            println!(
                "anchor 100k: threshold={threshold} limit={limit} n={} p50={:.2}ms p95={:.2}ms max={:.2}ms avg_hits={:.0}",
                anchors.len(),
                percentile(&timings, 50.0),
                percentile(&timings, 95.0),
                timings.last().copied().unwrap_or(0.0),
                hit_total as f64 / anchors.len() as f64,
            );
        }
    }

    /// 阈值预览与全扫的偏差记录（默认跳过）：
    /// `KANALIB_BENCH_DB=<db 副本> cargo test -p kanalib-core -- --ignored --nocapture benchmark_threshold_preview`
    #[test]
    #[ignore = "100k threshold-preview benchmark (KANALIB_BENCH_DB)"]
    fn benchmark_threshold_preview_100k() {
        let Some(db) = std::env::var_os("KANALIB_BENCH_DB") else {
            eprintln!("KANALIB_BENCH_DB 未设置，跳过基准");
            return;
        };
        let assets = load_bench_assets(Path::new(&db));
        println!(
            "preview benchmark assets={} sample_limit={} exact_limit={}",
            assets.len(),
            PREVIEW_SAMPLE_LIMIT,
            PREVIEW_EXACT_LIMIT
        );
        for threshold in [0u8, 6, 10, 14, 32] {
            let started = std::time::Instant::now();
            let preview = preview_similar_threshold(&assets, threshold);
            let preview_ms = started.elapsed().as_secs_f64() * 1000.0;
            if threshold == 32 {
                println!(
                    "preview 100k: threshold={threshold} sampled={}/{} group={} member={} chain_risk={} chain_groups={} preview_ms={:.1}（32 位全配对过慢，不做全扫对照）",
                    preview.sampled_assets,
                    preview.total_assets,
                    preview.group_count,
                    preview.member_count,
                    preview.chain_risk,
                    preview.chain_groups,
                    preview_ms,
                );
                continue;
            }
            let started = std::time::Instant::now();
            let groups = compute_similar_groups(&assets, threshold, &HashMap::new(), &[], &[]);
            let scan_ms = started.elapsed().as_secs_f64() * 1000.0;
            let actual_groups = groups.len();
            let actual_members = groups.iter().map(|group| group.assets.len()).sum::<usize>();
            let group_delta = preview.group_count as f64 - actual_groups as f64;
            let member_delta = preview.member_count as f64 - actual_members as f64;
            println!(
                "preview 100k: threshold={threshold} sampled={}/{} group={} actual={} delta={:+.1}% member={} actual={} delta={:+.1}% chain_risk={} chain_groups={} preview_ms={:.1} full_scan_ms={:.0}",
                preview.sampled_assets,
                preview.total_assets,
                preview.group_count,
                actual_groups,
                if actual_groups == 0 {
                    0.0
                } else {
                    group_delta * 100.0 / actual_groups as f64
                },
                preview.member_count,
                actual_members,
                if actual_members == 0 {
                    0.0
                } else {
                    member_delta * 100.0 / actual_members as f64
                },
                preview.chain_risk,
                preview.chain_groups,
                preview_ms,
                scan_ms,
            );
        }
    }

    /// 稀疏库（重复占比低、无长链）的抽样偏差：10 万随机 pHash + 2000 组植入近重复对。
    /// `cargo test -p kanalib-core -- --ignored --nocapture benchmark_preview_synthetic_100k`
    #[test]
    #[ignore = "100k synthetic sparse preview benchmark"]
    fn benchmark_preview_synthetic_100k() {
        let mut seed = 0x1234_5678_9ABC_DEF0u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut assets = Vec::with_capacity(100_000);
        let mut index = 0usize;
        while assets.len() < 100_000 {
            let phash = next();
            assets.push(asset_with(
                &format!("C:/lib/{index:06}.png"),
                &format!("sha{index}"),
                phash,
                100,
                10,
                100,
                100,
                0,
            ));
            index += 1;
            if index % 50 == 0 && assets.len() < 100_000 {
                // 每 50 张里植入一张与上一张距离 ≤6 的近似副本。
                let flipped = phash ^ (next() & 0b11_1111);
                assets.push(asset_with(
                    &format!("C:/lib/{index:06}.png"),
                    &format!("sha{index}"),
                    flipped,
                    100,
                    10,
                    100,
                    100,
                    0,
                ));
                index += 1;
            }
        }
        for threshold in [6u8, 10, 14] {
            let started = std::time::Instant::now();
            let preview = preview_similar_threshold(&assets, threshold);
            let preview_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = std::time::Instant::now();
            let groups = compute_similar_groups(&assets, threshold, &HashMap::new(), &[], &[]);
            let scan_ms = started.elapsed().as_secs_f64() * 1000.0;
            let actual_groups = groups.len();
            let actual_members = groups.iter().map(|group| group.assets.len()).sum::<usize>();
            println!(
                "preview synthetic 100k: threshold={threshold} sampled={}/{} group={} actual={} delta={:+.1}% member={} actual={} delta={:+.1}% chain_risk={} preview_ms={:.1} full_scan_ms={:.0}",
                preview.sampled_assets,
                preview.total_assets,
                preview.group_count,
                actual_groups,
                (preview.group_count as f64 - actual_groups as f64) * 100.0
                    / actual_groups.max(1) as f64,
                preview.member_count,
                actual_members,
                (preview.member_count as f64 - actual_members as f64) * 100.0
                    / actual_members.max(1) as f64,
                preview.chain_risk,
                preview_ms,
                scan_ms,
            );
        }
    }

    fn brute_force_components(assets: &[Asset], threshold: u8) -> Vec<Vec<String>> {
        let items = assets
            .iter()
            .filter(|asset| asset.phash.is_some())
            .collect::<Vec<_>>();
        let threshold = threshold.min(32) as u32;
        let mut union = UnionFind::new(items.len());
        for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                if items[i].sha1 == items[j].sha1 {
                    continue;
                }
                if (items[i].phash.unwrap() ^ items[j].phash.unwrap()).count_ones() <= threshold {
                    union.union(i, j);
                }
            }
        }
        let mut components: HashMap<usize, Vec<String>> = HashMap::new();
        for (index, item) in items.iter().enumerate() {
            components
                .entry(union.find(index))
                .or_default()
                .push(item.path.display().to_string());
        }
        let mut groups = components
            .into_values()
            .filter(|members| members.len() >= 2)
            .map(|mut members| {
                members.sort();
                members
            })
            .collect::<Vec<_>>();
        groups.sort();
        groups
    }
}
