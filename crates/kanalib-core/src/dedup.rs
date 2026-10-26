use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::model::Asset;

pub const DEFAULT_SIMILAR_THRESHOLD: u8 = 10;

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
    let threshold = threshold.min(32) as u32;
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
