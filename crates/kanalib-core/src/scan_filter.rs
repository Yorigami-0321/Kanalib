//! F-032 扫描排除规则：按来源根目录应用 glob/隐藏/大小过滤。
//!
//! 配置结构见 [`crate::model::ScanFilterConfig`]，按来源存于 `settings` 的
//! `scan.filters`。扫描时若无任何启用规则，仍走原来的 `walkdir` 快路径，
//! 保证 10k/100k 扫描性能与改造前一致；有规则时才使用 `ignore::WalkBuilder`，
//! 并显式关闭 gitignore/global gitignore/parents/.ignore 语义，避免用户仓库里的
//! `.gitignore` 悄悄改变图库扫描结果。

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use ignore::overrides::{Override, OverrideBuilder};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::db::Db;
use crate::model::{ScanFilterConfig, is_media};

/// settings 表中的配置键；值为 `{ root: ScanFilterConfig }` 的 JSON 对象。
pub const SCAN_FILTERS_SETTING_KEY: &str = "scan.filters";

/// F-037 持久忽略列表；路径/哈希/glob 三种粒度共用同一份 settings JSON。
pub const IGNORE_RULES_SETTING_KEY: &str = "library.ignore";

/// 预览最多返回多少个被排除样例，避免为 UI 保留大列表。
pub const PREVIEW_SAMPLE_LIMIT: usize = 20;

/// 排除规则预览结果；计数按文件数统计，样例路径为绝对路径。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanFilterPreview {
    /// 会被扫描入库的图片数。
    pub matched: usize,
    /// 会被规则排除的图片数。
    pub excluded: usize,
    /// 被排除样例（最多 [`PREVIEW_SAMPLE_LIMIT`] 个）。
    pub sample_excluded: Vec<PathBuf>,
}

/// 一次过滤遍历的结果。
#[derive(Clone, Debug, Default)]
pub struct ScanWalk {
    /// 过滤后的图片文件（已排序）。
    pub files: Vec<PathBuf>,
    /// 遍历是否完整；出现任何目录读取错误时为 false，调用方不得据此清理记录。
    pub walk_complete: bool,
}

/// 编译后的来源过滤规则；构建一次即可用于扫描、预览与监听事件判断。
#[derive(Clone, Debug)]
pub struct CompiledScanFilter {
    root: PathBuf,
    config: ScanFilterConfig,
    overrides: Option<Override>,
}

impl CompiledScanFilter {
    /// 校验并编译排除规则；非法 glob 返回可读错误而不是 panic。
    pub fn new(root: &Path, config: &ScanFilterConfig) -> Result<Self> {
        let overrides = if config.exclude_globs.is_empty() {
            None
        } else {
            let mut builder = OverrideBuilder::new(root);
            for pattern in &config.exclude_globs {
                let pattern = pattern.trim();
                if pattern.is_empty() {
                    bail!("排除规则不能为空");
                }
                // Override 的 `!` 语义与 gitignore 相反：带 `!` 才是排除。
                // 配置项本身就叫 exclude_globs，因此统一归一化为排除模式。
                let pattern = pattern.trim_start_matches('!');
                builder
                    .add(&format!("!{pattern}"))
                    .with_context(|| format!("无效的排除规则：{pattern}"))?;
            }
            Some(builder.build().context("构建排除规则失败")?)
        };
        Ok(Self {
            root: root.to_path_buf(),
            config: config.clone(),
            overrides,
        })
    }

    pub fn config(&self) -> &ScanFilterConfig {
        &self.config
    }

    pub(crate) fn overrides(&self) -> Option<&Override> {
        self.overrides.as_ref()
    }

    /// 路径是否被规则排除。`bytes` 未知时只检查隐藏与 glob。
    pub fn excludes(&self, path: &Path, is_dir: bool, bytes: Option<u64>) -> bool {
        if !self.config.include_hidden && is_hidden(&self.root, path) {
            return true;
        }
        if self
            .overrides
            .as_ref()
            .is_some_and(|overrides| overrides.matched(path, is_dir).is_ignore())
        {
            return true;
        }
        if let Some(bytes) = bytes {
            if self.config.min_bytes.is_some_and(|min| bytes < min) {
                return true;
            }
            if self.config.max_bytes.is_some_and(|max| bytes > max) {
                return true;
            }
        }
        false
    }
}

fn is_hidden(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .map(|relative| {
            relative.components().any(|component| match component {
                Component::Normal(name) => name.to_string_lossy().starts_with('.'),
                _ => false,
            })
        })
        .unwrap_or(false)
}

/// 收集来源下应入库的媒体（图片与视频，F-040）。
///
/// 未启用任何规则时保持原有 `walkdir` 行为（包含隐藏文件、不读 ignore 文件）；
/// 启用规则时使用 `ignore`，但显式关闭所有 gitignore 类规则。
pub fn collect_images(root: &Path, config: &ScanFilterConfig) -> Result<ScanWalk> {
    if !config.is_active() {
        return Ok(collect_images_fast(root));
    }
    let compiled = CompiledScanFilter::new(root, config)?;
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(!config.include_hidden)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .parents(false)
        .follow_links(false);
    if let Some(overrides) = compiled.overrides() {
        builder.overrides(overrides.clone());
    }
    let mut files = Vec::new();
    let mut walk_complete = true;
    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                walk_complete = false;
                log::warn!("walk error under {}: {error}", root.display());
                continue;
            }
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) || !is_media(entry.path()) {
            continue;
        }
        let bytes = entry.metadata().ok().map(|metadata| metadata.len());
        if compiled.excludes(entry.path(), false, bytes) {
            continue;
        }
        files.push(entry.path().to_path_buf());
    }
    files.sort();
    Ok(ScanWalk {
        files,
        walk_complete,
    })
}

/// 在 F-032 排除规则之上叠加 F-037 持久忽略项。
///
/// 路径忽略在遍历后统一剔除；glob 合并进过滤配置，仍可在遍历时剪枝目录。
pub fn collect_images_with_ignores(
    root: &Path,
    config: &ScanFilterConfig,
    rules: &IgnoreRules,
) -> Result<ScanWalk> {
    let ignores = CompiledIgnoreRules::new(root, rules);
    let merged = ignores.merged_config(config);
    let mut walk = collect_images(root, &merged)?;
    walk.files
        .retain(|path| !ignores.path_ignored(path));
    Ok(walk)
}

/// 无规则快路径：与改造前的遍历逻辑保持一致。
fn collect_images_fast(root: &Path) -> ScanWalk {
    let mut files = Vec::new();
    let mut walk_complete = true;
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                walk_complete = false;
                log::warn!("walk error: {error}");
                continue;
            }
        };
        if entry.file_type().is_file() && is_media(entry.path()) {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    ScanWalk {
        files,
        walk_complete,
    }
}

/// 预览规则效果：遍历来源并逐个判断媒体（图片/视频）是否会被排除。
pub fn preview_scan_filter(root: &Path, config: &ScanFilterConfig) -> Result<ScanFilterPreview> {
    if !root.is_dir() {
        bail!("来源不可访问：{}", root.display());
    }
    let compiled = CompiledScanFilter::new(root, config)?;
    let mut preview = ScanFilterPreview::default();
    for entry in WalkDir::new(root).follow_links(false) {
        let Ok(entry) = entry else {
            continue;
        };
        if !entry.file_type().is_file() || !is_media(entry.path()) {
            continue;
        }
        let bytes = entry.metadata().ok().map(|metadata| metadata.len());
        let excluded = compiled.excludes(entry.path(), false, bytes);
        if excluded {
            preview.excluded += 1;
            if preview.sample_excluded.len() < PREVIEW_SAMPLE_LIMIT {
                preview.sample_excluded.push(entry.path().to_path_buf());
            }
        } else {
            preview.matched += 1;
        }
    }
    Ok(preview)
}

/// F-037 持久忽略列表。
///
/// 语义：
/// - `paths`：绝对文件路径；扫描遍历、监听事件与哈希前都会跳过，恢复后重新参与扫描。
/// - `sha1`：内容哈希（小写）；扫描/监听在算出哈希后跳过，避免同内容副本重新入库。
/// - `globs`：按来源根目录保存的 glob，与 F-032 排除规则使用同一 `Override` 语义；
///   区别是它属于“忽略列表”，可逐条查看与恢复，而 `scan.filters` 是来源级扫描配置。
///
/// glob 只影响索引，不删除磁盘文件。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IgnoreRules {
    pub paths: Vec<String>,
    pub sha1: Vec<String>,
    pub globs: BTreeMap<String, Vec<String>>,
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn normalized_hash(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

impl IgnoreRules {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.sha1.is_empty() && self.globs.is_empty()
    }

    pub fn contains_path(&self, path: &Path) -> bool {
        let key = path_key(path);
        self.paths
            .iter()
            .any(|item| path_key(Path::new(item)) == key)
    }

    /// 新增路径忽略项；已存在时返回 false。
    pub fn insert_path(&mut self, path: &Path) -> bool {
        if self.contains_path(path) {
            return false;
        }
        self.paths.push(path.to_string_lossy().to_string());
        self.paths.sort();
        true
    }

    /// 移除路径忽略项；返回是否实际移除。
    pub fn remove_path(&mut self, path: &Path) -> bool {
        let key = path_key(path);
        let before = self.paths.len();
        self.paths.retain(|item| path_key(Path::new(item)) != key);
        self.paths.len() != before
    }

    pub fn contains_hash(&self, sha1: &str) -> bool {
        let hash = normalized_hash(sha1);
        !hash.is_empty() && self.sha1.iter().any(|item| item == &hash)
    }

    pub fn insert_hash(&mut self, sha1: &str) -> bool {
        let hash = normalized_hash(sha1);
        if hash.is_empty() || self.contains_hash(&hash) {
            return false;
        }
        self.sha1.push(hash);
        self.sha1.sort();
        true
    }

    pub fn remove_hash(&mut self, sha1: &str) -> bool {
        let hash = normalized_hash(sha1);
        let before = self.sha1.len();
        self.sha1.retain(|item| item != &hash);
        self.sha1.len() != before
    }

    /// 某来源的忽略 glob 列表（根目录大小写不敏感）。
    pub fn globs_for(&self, root: &Path) -> Vec<String> {
        let key = path_key(root);
        self.globs
            .iter()
            .find(|(stored, _)| path_key(Path::new(stored)) == key)
            .map(|(_, globs)| globs.clone())
            .unwrap_or_default()
    }

    /// 校验并新增一条来源 glob；非法 glob 返回错误。
    pub fn insert_glob(&mut self, root: &Path, pattern: &str) -> Result<bool> {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            bail!("忽略规则不能为空");
        }
        validate_exclude_glob(root, pattern)?;
        let key = path_key(root);
        let stored = self
            .globs
            .keys()
            .find(|stored| path_key(Path::new(stored)) == key)
            .cloned()
            .unwrap_or_else(|| {
                let value = root.to_string_lossy().to_string();
                self.globs.insert(value.clone(), Vec::new());
                value
            });
        let globs = self.globs.get_mut(&stored).expect("ignore glob root present");
        if globs.iter().any(|item| item == pattern) {
            return Ok(false);
        }
        globs.push(pattern.to_string());
        globs.sort();
        Ok(true)
    }

    pub fn remove_glob(&mut self, root: &Path, pattern: &str) -> bool {
        let pattern = pattern.trim();
        let key = path_key(root);
        let Some((_, globs)) = self
            .globs
            .iter_mut()
            .find(|(stored, _)| path_key(Path::new(stored)) == key)
        else {
            return false;
        };
        let before = globs.len();
        globs.retain(|item| item != pattern);
        let changed = globs.len() != before;
        if globs.is_empty() {
            self.globs
                .retain(|stored, _| path_key(Path::new(stored)) != key);
        }
        changed
    }

    /// 来源被移除时同步清掉它的 glob，避免 settings 里留下无主规则。
    pub fn prune_root(&mut self, root: &Path) {
        let key = path_key(root);
        self.globs
            .retain(|stored, _| path_key(Path::new(stored)) != key);
    }
}

fn validate_exclude_glob(root: &Path, pattern: &str) -> Result<()> {
    let config = ScanFilterConfig {
        exclude_globs: vec![pattern.to_string()],
        ..ScanFilterConfig::default()
    };
    CompiledScanFilter::new(root, &config).map(|_| ())
}

/// 单次扫描/监听使用的编译后忽略规则；路径与哈希用集合查找，glob 合并进过滤配置。
#[derive(Clone, Debug)]
pub struct CompiledIgnoreRules {
    root: PathBuf,
    paths: std::collections::HashSet<String>,
    sha1: std::collections::HashSet<String>,
    globs: Vec<String>,
}

impl CompiledIgnoreRules {
    pub fn new(root: &Path, rules: &IgnoreRules) -> Self {
        Self {
            root: root.to_path_buf(),
            paths: rules.paths.iter().map(|path| path_key(Path::new(path))).collect(),
            sha1: rules
                .sha1
                .iter()
                .map(|hash| normalized_hash(hash))
                .filter(|hash| !hash.is_empty())
                .collect(),
            globs: rules.globs_for(root),
        }
    }

    pub fn path_ignored(&self, path: &Path) -> bool {
        self.paths.contains(&path_key(path))
    }

    pub fn hash_ignored(&self, sha1: &str) -> bool {
        let hash = normalized_hash(sha1);
        !hash.is_empty() && self.sha1.contains(&hash)
    }

    pub fn merged_config(&self, base: &ScanFilterConfig) -> ScanFilterConfig {
        let mut merged = base.clone();
        merged.exclude_globs.extend(self.globs.iter().cloned());
        merged
    }

    /// 统一忽略判定：F-037 路径/glob + F-032 来源排除规则。
    pub fn ignores(
        &self,
        filter: &CompiledScanFilter,
        path: &Path,
        is_dir: bool,
        bytes: Option<u64>,
    ) -> bool {
        self.path_ignored(path) || filter.excludes(path, is_dir, bytes)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// 从 settings 读取持久忽略列表；缺失时返回空列表。
pub fn load_ignore_rules(db: &Db) -> Result<IgnoreRules> {
    let Some(value) = db.setting(IGNORE_RULES_SETTING_KEY)? else {
        return Ok(IgnoreRules::default());
    };
    serde_json::from_str(&value).context("解析 library.ignore 失败")
}

/// 写回持久忽略列表；路径/哈希去重排序，便于比对与测试。
pub fn save_ignore_rules(db: &Db, rules: &IgnoreRules) -> Result<()> {
    let mut normalized = rules.clone();
    normalized.paths.sort();
    normalized.paths.dedup();
    normalized.sha1 = normalized
        .sha1
        .iter()
        .map(|hash| normalized_hash(hash))
        .filter(|hash| !hash.is_empty())
        .collect();
    normalized.sha1.sort();
    normalized.sha1.dedup();
    for globs in normalized.globs.values_mut() {
        globs.sort();
        globs.dedup();
    }
    db.set_setting(
        IGNORE_RULES_SETTING_KEY,
        &serde_json::to_string(&normalized).context("序列化 library.ignore 失败")?,
    )
}

/// 从 settings 读取所有来源的排除规则。
pub fn load_filters(db: &Db) -> Result<HashMap<PathBuf, ScanFilterConfig>> {    let Some(value) = db.setting(SCAN_FILTERS_SETTING_KEY)? else {
        return Ok(HashMap::new());
    };
    let parsed = serde_json::from_str::<HashMap<String, ScanFilterConfig>>(&value)
        .context("解析 scan.filters 失败")?;
    Ok(parsed
        .into_iter()
        .map(|(root, config)| (PathBuf::from(root), config))
        .collect())
}

/// 把所有来源的排除规则写回 settings（键排序，便于比对与测试）。
pub fn save_filters(db: &Db, filters: &HashMap<PathBuf, ScanFilterConfig>) -> Result<()> {
    let values = filters
        .iter()
        .map(|(root, config)| (root.to_string_lossy().to_string(), config))
        .collect::<BTreeMap<_, _>>();
    db.set_setting(
        SCAN_FILTERS_SETTING_KEY,
        &serde_json::to_string(&values).context("序列化 scan.filters 失败")?,
    )
}

/// 监听事件使用的路径判断；配置非法时返回错误。
pub fn path_excluded_by(
    root: &Path,
    compiled: &CompiledScanFilter,
    path: &Path,
    is_dir: bool,
    bytes: Option<u64>,
) -> bool {
    !path.starts_with(root) || compiled.excludes(path, is_dir, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbImage;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kanalib-filter-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_image(path: &Path, bytes: u32) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]))
            .save(path)
            .unwrap();
        if bytes > 0 {
            // 用完整字节数补齐文件，便于测试 min/max 大小。
            let mut data = std::fs::read(path).unwrap();
            data.resize(bytes as usize, 0);
            std::fs::write(path, data).unwrap();
        }
    }

    fn config() -> ScanFilterConfig {
        ScanFilterConfig::default()
    }

    #[test]
    fn default_filter_keeps_hidden_and_inactive() {
        let dir = temp_dir("default");
        write_image(&dir.join("keep.png"), 0);
        write_image(&dir.join(".hidden.png"), 0);
        write_image(&dir.join("cache").join("nested.png"), 0);

        let config = config();
        assert!(config.include_hidden);
        assert!(!config.is_active());

        let walk = collect_images(&dir, &config).unwrap();
        assert!(walk.walk_complete);
        assert_eq!(walk.files.len(), 3);

        let preview = preview_scan_filter(&dir, &config).unwrap();
        assert_eq!(preview.matched, 3);
        assert_eq!(preview.excluded, 0);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn globs_hidden_and_size_rules_match_and_preview() {
        let dir = temp_dir("rules");
        write_image(&dir.join("keep.png"), 100);
        write_image(&dir.join(".hidden.png"), 0);
        write_image(&dir.join("cache").join("nested.png"), 0);
        write_image(&dir.join("small.png"), 32);
        write_image(&dir.join("big.png"), 4096);

        let config = ScanFilterConfig {
            exclude_globs: vec!["cache/**".into(), "*.tmp".into()],
            include_hidden: false,
            min_bytes: Some(64),
            max_bytes: Some(2048),
        };
        assert!(config.is_active());

        let walk = collect_images(&dir, &config).unwrap();
        assert!(walk.walk_complete);
        let names = walk
            .files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["keep.png".to_string()]);

        let preview = preview_scan_filter(&dir, &config).unwrap();
        assert_eq!(preview.matched, 1);
        assert_eq!(preview.excluded, 4);
        assert_eq!(preview.sample_excluded.len(), 4);
        assert!(
            preview
                .sample_excluded
                .iter()
                .any(|path| path.ends_with(".hidden.png"))
        );
        assert!(
            preview
                .sample_excluded
                .iter()
                .any(|path| path.ends_with("nested.png"))
        );
        assert!(
            preview
                .sample_excluded
                .iter()
                .any(|path| path.ends_with("small.png"))
        );
        assert!(
            preview
                .sample_excluded
                .iter()
                .any(|path| path.ends_with("big.png"))
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_glob_returns_error_instead_of_panicking() {
        let dir = temp_dir("invalid");
        write_image(&dir.join("a.png"), 0);
        let config = ScanFilterConfig {
            exclude_globs: vec!["[".into()],
            ..ScanFilterConfig::default()
        };
        let error = collect_images(&dir, &config).unwrap_err();
        assert!(format!("{error:#}").contains("排除规则"));
        assert!(preview_scan_filter(&dir, &config).is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn gitignore_files_are_never_consulted() {
        let dir = temp_dir("gitignore");
        write_image(&dir.join("keep.png"), 0);
        write_image(&dir.join("ignored.png"), 0);
        std::fs::write(dir.join(".gitignore"), "ignored.png\n").unwrap();

        // 仅开启隐藏过滤，让 ignore walker 生效，但 .gitignore 必须被忽略。
        let config = ScanFilterConfig {
            include_hidden: false,
            ..ScanFilterConfig::default()
        };
        let walk = collect_images(&dir, &config).unwrap();
        assert_eq!(walk.files.len(), 2);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ignore_rules_round_trip_and_merge_with_scan_filters() {
        let dir = temp_dir("ignore-rules");
        let db = Db::open(&dir.join("kanalib.db")).unwrap();
        let root = dir.join("lib");
        std::fs::create_dir_all(root.join("cache")).unwrap();
        write_image(&root.join("keep.png"), 0);
        write_image(&root.join("gone.png"), 0);
        write_image(&root.join("cache").join("skip.png"), 0);

        let ignored_path = root.join("gone.png");
        let mut rules = IgnoreRules::default();
        assert!(rules.insert_path(&ignored_path));
        assert!(!rules.insert_path(&ignored_path));
        assert!(rules.insert_hash("ABCDEF"));
        assert!(!rules.insert_hash("abcdef"));
        assert!(rules.insert_glob(&root, "cache/**").unwrap());
        assert!(!rules.insert_glob(&root, "cache/**").unwrap());

        save_ignore_rules(&db, &rules).unwrap();
        let loaded = load_ignore_rules(&db).unwrap();
        assert_eq!(loaded, rules);
        assert!(loaded.contains_hash("abcdef"));
        assert!(loaded.contains_path(&ignored_path));

        let compiled = CompiledIgnoreRules::new(&root, &loaded);
        assert!(compiled.path_ignored(&ignored_path));
        assert!(compiled.hash_ignored("abcdef"));
        assert!(compiled.hash_ignored("ABCDEF"));
        assert!(!compiled.hash_ignored("other"));

        let config = ScanFilterConfig {
            exclude_globs: vec!["*.tmp".into()],
            ..ScanFilterConfig::default()
        };
        let merged = compiled.merged_config(&config);
        assert!(merged.exclude_globs.contains(&"*.tmp".to_string()));
        assert!(merged.exclude_globs.contains(&"cache/**".to_string()));

        let walk = collect_images_with_ignores(&root, &config, &loaded).unwrap();
        let names = walk
            .files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["keep.png".to_string()]);

        let mut restored = loaded.clone();
        assert!(restored.remove_path(&ignored_path));
        assert!(restored.remove_hash("abcdef"));
        assert!(restored.remove_glob(&root, "cache/**"));
        assert!(restored.is_empty());
        save_ignore_rules(&db, &restored).unwrap();
        assert!(load_ignore_rules(&db).unwrap().is_empty());

        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_ignore_glob_returns_error() {
        let dir = temp_dir("ignore-invalid");
        let mut rules = IgnoreRules::default();
        let error = rules.insert_glob(&dir, "[").unwrap_err();
        assert!(format!("{error:#}").contains("排除规则"));
        assert!(rules.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn filters_round_trip_through_settings() {
        let dir = temp_dir("settings");
        let db = Db::open(&dir.join("kanalib.db")).unwrap();
        let root = dir.join("lib");
        let mut filters = HashMap::new();
        filters.insert(
            root.clone(),
            ScanFilterConfig {
                exclude_globs: vec!["cache/**".into()],
                include_hidden: false,
                min_bytes: Some(10),
                max_bytes: Some(100),
            },
        );
        save_filters(&db, &filters).unwrap();
        let loaded = load_filters(&db).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get(&root), filters.get(&root));

        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
