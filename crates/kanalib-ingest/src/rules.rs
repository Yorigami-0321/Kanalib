//! I2 规则层：`rules.toml` 是唯一规则源，这里负责加载校验与十步决策。
//!
//! 文件解析失败必须拒绝启动；决策异常一律 review，不允许 fail-open。

use std::{collections::HashSet, path::Path, sync::Arc};

use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;
use unicode_normalization::UnicodeNormalization;

use crate::source::{AiType, Rating};

pub const R18G_TAGS: [&str; 2] = ["r-18g", "r18g"];
pub const CONTENT_TAG_EXCLUDES: [&str; 6] = [
    "オリジナル",
    "漫画",
    "イラスト",
    "original",
    "manga",
    "illustration",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
#[derive(Default)]
pub struct RuleConfig {
    pub rules: RulesSection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RulesSection {
    pub version: String,
    pub config_hash_check: bool,
    pub blacklist: Blacklist,
    pub rating: RatingRules,
    pub ai: AiRules,
    pub tag_budget: TagBudget,
    pub dedup: DedupRules,
    pub size: SizeRules,
}

impl Default for RulesSection {
    fn default() -> Self {
        Self {
            version: "2026-11-22.1".to_string(),
            config_hash_check: true,
            blacklist: Blacklist::default(),
            rating: RatingRules::default(),
            ai: AiRules::default(),
            tag_budget: TagBudget::default(),
            dedup: DedupRules::default(),
            size: SizeRules::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Blacklist {
    pub users: Vec<String>,
    pub tags: Vec<String>,
    pub watch: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RatingRules {
    pub block: Vec<String>,
    pub allow: Vec<String>,
    pub sanity_review_threshold: i64,
}

impl Default for RatingRules {
    fn default() -> Self {
        Self {
            block: vec!["r18g".to_string()],
            allow: vec!["all".to_string(), "r18".to_string()],
            sanity_review_threshold: 6,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AiRules {
    pub allow: bool,
}

impl Default for AiRules {
    fn default() -> Self {
        Self { allow: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct TagBudget {
    pub min_tags: usize,
    pub ignore: Vec<String>,
    pub ignore_suffixes: Vec<String>,
}

impl Default for TagBudget {
    fn default() -> Self {
        Self {
            min_tags: 5,
            ignore: CONTENT_TAG_EXCLUDES
                .iter()
                .map(|tag| tag.to_string())
                .collect(),
            ignore_suffixes: vec!["users入り".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct DedupRules {
    pub enabled: bool,
    pub on_conflict: String,
}

impl Default for DedupRules {
    fn default() -> Self {
        Self {
            enabled: true,
            on_conflict: "block".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SizeRules {
    pub min_width: u32,
    pub min_height: u32,
    pub on_fail: String,
}

impl Default for SizeRules {
    fn default() -> Self {
        Self {
            min_width: 512,
            min_height: 512,
            on_fail: "review".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedRules {
    pub config: Arc<RuleConfig>,
    pub config_hash: String,
    pub version: String,
}

pub fn load_file(path: &Path) -> Result<LoadedRules> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading rules file {}", path.display()))?;
    load_str(&text)
}

pub fn load_str(text: &str) -> Result<LoadedRules> {
    let config: RuleConfig = toml::from_str(text).context("parse rules file")?;
    let config = normalize_and_validate(config)?;
    let version = config.rules.version.clone();
    Ok(LoadedRules {
        config_hash: config_hash(&config),
        version,
        config: Arc::new(config),
    })
}

pub fn config_hash(config: &RuleConfig) -> String {
    let canonical = serde_json::to_string(config).expect("canonical rule json");
    let digest = Sha256::digest(canonical.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl RuleConfig {
    /// 标签黑名单为空时没有任何硬拦词：启动/重载要告警，管理页要显示。
    pub fn tag_blacklist_is_empty(&self) -> bool {
        self.rules.blacklist.tags.is_empty()
    }
}

pub fn normalize_tag(raw: &str) -> String {
    UnicodeNormalization::nfkc(raw.chars())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn normalize_unique(tags: Vec<String>, label: &str) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for raw in tags {
        let normalized = normalize_tag(&raw);
        if normalized.is_empty() {
            anyhow::bail!("{label} contains an empty tag");
        }
        if seen.insert(normalized) {
            result.push(raw.trim().to_string());
        } else {
            warn!(table = label, tag = %raw, "duplicate rule tag normalized and ignored");
        }
    }
    Ok(result)
}

fn normalize_and_validate(mut config: RuleConfig) -> Result<RuleConfig> {
    if config.rules.version.trim().is_empty() {
        bail!("rules.version must not be empty");
    }
    config.rules.version = config.rules.version.trim().to_string();
    config.rules.blacklist.users = normalize_unique(config.rules.blacklist.users, "users")?;
    config.rules.blacklist.tags = normalize_unique(config.rules.blacklist.tags, "tags")?;
    config.rules.blacklist.watch = normalize_unique(config.rules.blacklist.watch, "watch")?;
    config.rules.rating.block = normalize_unique(config.rules.rating.block, "rating.block")?;
    config.rules.rating.allow = normalize_unique(config.rules.rating.allow, "rating.allow")?;
    config.rules.tag_budget.ignore =
        normalize_unique(config.rules.tag_budget.ignore, "tag_budget.ignore")?;

    if config
        .rules
        .rating
        .block
        .iter()
        .any(|rating| rating != "r18g")
    {
        bail!("rules.rating.block only permits r18g");
    }
    if config
        .rules
        .tag_budget
        .ignore_suffixes
        .iter()
        .any(|suffix| suffix.trim().is_empty())
    {
        bail!("rules.tag_budget.ignore_suffixes contains an empty suffix");
    }
    if config.rules.dedup.on_conflict != "block" && config.rules.dedup.on_conflict != "review" {
        bail!("rules.dedup.on_conflict must be block or review");
    }
    if config.rules.size.on_fail != "review" && config.rules.size.on_fail != "block" {
        bail!("rules.size.on_fail must be review or block");
    }
    let watch: HashSet<_> = config
        .rules
        .blacklist
        .watch
        .iter()
        .map(|tag| normalize_tag(tag))
        .collect();
    for tag in &config.rules.blacklist.tags {
        if watch.contains(&normalize_tag(tag)) {
            bail!("tag {tag:?} is both in tags and watch");
        }
    }
    for tag in &config.rules.rating.block {
        if config.rules.rating.allow.contains(tag) {
            bail!("rating {tag:?} is both blocked and allowed");
        }
    }
    Ok(config)
}

pub fn content_tag_values(name: &str, translated_name: Option<&str>) -> Vec<String> {
    let mut values = vec![normalize_tag(name)];
    if let Some(translated) = translated_name {
        values.push(normalize_tag(translated));
    }
    values.retain(|tag| !tag.is_empty());
    values
}

pub fn is_content_tag(config: &RuleConfig, candidates: &[String]) -> bool {
    candidates.iter().any(|tag| {
        !config
            .rules
            .tag_budget
            .ignore
            .iter()
            .any(|ignore| normalize_tag(ignore) == *tag)
            && !config
                .rules
                .tag_budget
                .ignore_suffixes
                .iter()
                .any(|suffix| tag.ends_with(&normalize_tag(suffix)))
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleSubject {
    pub source: String,
    pub source_enabled: bool,
    pub author_external_id: Option<String>,
    pub rating: Rating,
    pub x_restrict: Option<i64>,
    pub sanity_level: Option<i64>,
    pub restriction_attributes: Vec<String>,
    pub ai: AiType,
    pub tags: Vec<(String, Option<String>)>,
    pub tag_count: usize,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub sha1: Option<String>,
    pub item_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Block,
    Review,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Block => "block",
            Self::Review => "review",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleOutcome {
    pub decision: Decision,
    pub reason: String,
    pub matched: Vec<String>,
    pub needs_tagging: bool,
}

impl RuleOutcome {
    fn block(reason: impl Into<String>, matched: Vec<String>) -> Self {
        Self {
            decision: Decision::Block,
            reason: reason.into(),
            matched,
            needs_tagging: false,
        }
    }

    fn review(reason: impl Into<String>, matched: Vec<String>) -> Self {
        Self {
            decision: Decision::Review,
            reason: reason.into(),
            matched,
            needs_tagging: false,
        }
    }

    pub fn review_public(reason: impl Into<String>, matched: Vec<String>) -> Self {
        Self::review(reason, matched)
    }

    fn allow(reason: impl Into<String>) -> Self {
        Self {
            decision: Decision::Allow,
            reason: reason.into(),
            matched: Vec::new(),
            needs_tagging: false,
        }
    }
}

pub struct DecisionInput<'a> {
    pub db: &'a crate::db::Db,
    pub config: &'a RuleConfig,
    pub subject: &'a RuleSubject,
}

pub fn evaluate(input: DecisionInput<'_>) -> Result<RuleOutcome> {
    let config = input.config;
    let subject = input.subject;
    let mut matched = Vec::new();

    if !subject.source_enabled {
        return Ok(RuleOutcome::block("source disabled", matched));
    }
    if let Some(author) = &subject.author_external_id {
        let normalized_author = normalize_tag(author);
        if config
            .rules
            .blacklist
            .users
            .iter()
            .any(|user| normalize_tag(user) == normalized_author)
        {
            matched.push(format!("user:{author}"));
            return Ok(RuleOutcome::block("author blacklist", matched));
        }
    }

    let tag_names = || {
        subject
            .tags
            .iter()
            .flat_map(|(name, translated)| content_tag_values(name, translated.as_deref()))
            .collect::<HashSet<_>>()
    };
    let blacklist_hits = |tags: &HashSet<String>| -> Vec<String> {
        config
            .rules
            .blacklist
            .tags
            .iter()
            .filter(|rule| tags.contains(&normalize_tag(rule)))
            .map(|rule| format!("tag:{rule}"))
            .collect()
    };
    let watch_hits = |tags: &HashSet<String>| -> Vec<String> {
        config
            .rules
            .blacklist
            .watch
            .iter()
            .filter(|rule| tags.contains(&normalize_tag(rule)))
            .map(|rule| format!("watch:{rule}"))
            .collect()
    };

    let mut tags = tag_names();
    if let Some(reason) = hard_r18g_reason(subject, &tags) {
        matched.push(reason);
        return Ok(RuleOutcome::block("r18g", matched));
    }
    // 三路字段全缺时不能当全年龄放行；sanity 只是兜底信号，转 review。
    if subject.rating == Rating::Unknown
        && subject.x_restrict.is_none()
        && subject.sanity_level.unwrap_or(0) >= config.rules.rating.sanity_review_threshold
    {
        matched.push("rating:sanity".to_string());
        return Ok(RuleOutcome::review("rating sanity review", matched));
    }

    let mut hits = blacklist_hits(&tags);
    if !hits.is_empty() {
        matched.extend(hits);
        return Ok(RuleOutcome::block("tag blacklist", matched));
    }

    let content_count = content_tag_count(config, subject);
    if content_count < config.rules.tag_budget.min_tags {
        return Ok(RuleOutcome {
            decision: Decision::Review,
            reason: "tag budget short".to_string(),
            matched,
            needs_tagging: true,
        });
    }

    if config.rules.dedup.enabled && duplicate_exists(input.db, subject)? {
        matched.push("dedup:sha1".to_string());
        return Ok(policy_outcome(
            &config.rules.dedup.on_conflict,
            "duplicate hash",
            matched,
        ));
    }

    tags = tag_names();
    hits = watch_hits(&tags);
    if !hits.is_empty() {
        let reason = hits.join(", ");
        matched.extend(hits);
        return Ok(RuleOutcome::review(reason, matched));
    }
    if subject.width < config.rules.size.min_width || subject.height < config.rules.size.min_height
    {
        matched.push("size:low".to_string());
        return Ok(policy_outcome(
            &config.rules.size.on_fail,
            "size below minimum",
            matched,
        ));
    }
    if subject.bytes == 0 {
        matched.push("size:empty".to_string());
        return Ok(policy_outcome(
            &config.rules.size.on_fail,
            "empty image",
            matched,
        ));
    }
    matched.clear();
    Ok(RuleOutcome::allow("all rules passed"))
}

/// `dedup.on_conflict` / `size.on_fail` 在校验层只允许 block|review。
fn policy_outcome(policy: &str, reason: &str, matched: Vec<String>) -> RuleOutcome {
    if policy == "review" {
        RuleOutcome::review(reason, matched)
    } else {
        RuleOutcome::block(reason, matched)
    }
}

fn content_tag_count(config: &RuleConfig, subject: &RuleSubject) -> usize {
    let mut unique = HashSet::new();
    // 一个标签算一个：译名只是同一标签的另一写法，不能再加一次计数，
    // 否则双语标签会凭空翻倍、把 < min_tags 的条目错判为足量。
    for (name, _translated) in &subject.tags {
        let normalized = normalize_tag(name);
        if normalized.is_empty() {
            continue;
        }
        if is_content_tag(config, std::slice::from_ref(&normalized)) {
            unique.insert(normalized);
        }
    }
    unique.len()
}

fn hard_r18g_reason(subject: &RuleSubject, tags: &HashSet<String>) -> Option<String> {
    if subject.rating == Rating::R18G || subject.x_restrict == Some(2) {
        return Some("rating:x_restrict".to_string());
    }
    if R18G_TAGS.iter().any(|tag| tags.contains(&tag.to_string())) {
        return Some("rating:tag".to_string());
    }
    if subject
        .restriction_attributes
        .iter()
        .any(|attribute| normalize_tag(attribute) == "r18g")
    {
        return Some("rating:restriction".to_string());
    }
    None
}

fn duplicate_exists(db: &crate::db::Db, subject: &RuleSubject) -> Result<bool> {
    let Some(sha1) = subject.sha1.as_deref().filter(|value| !value.is_empty()) else {
        return Ok(false);
    };
    // 只在“已有更早的同哈希条目”时判重；否则同一批 pending 的两条会互相判重、双双丢弃。
    let existing: Option<String> = db
        .connection()
        .query_row(
            "SELECT earlier.id
               FROM items earlier
               JOIN items subject ON subject.id = ?2
              WHERE earlier.sha1 = ?1
                AND earlier.id <> subject.id
                AND (earlier.created_at < subject.created_at
                     OR (earlier.created_at = subject.created_at AND earlier.id < subject.id))
              LIMIT 1",
            params![sha1, subject.item_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(existing.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(tags: &[&str], watch: &[&str]) -> RuleConfig {
        RuleConfig {
            rules: RulesSection {
                blacklist: Blacklist {
                    users: vec!["blocked-author".to_string()],
                    tags: tags.iter().map(|tag| tag.to_string()).collect(),
                    watch: watch.iter().map(|tag| tag.to_string()).collect(),
                },
                ..RulesSection::default()
            },
        }
    }

    fn subject() -> RuleSubject {
        RuleSubject {
            source: "pixiv".to_string(),
            source_enabled: true,
            author_external_id: Some("author".to_string()),
            rating: Rating::All,
            x_restrict: Some(0),
            sanity_level: Some(2),
            restriction_attributes: Vec::new(),
            ai: AiType::No,
            tags: (0..5).map(|index| (format!("tag{index}"), None)).collect(),
            tag_count: 5,
            width: 800,
            height: 800,
            bytes: 100,
            sha1: Some("hash".to_string()),
            item_id: "item".to_string(),
        }
    }

    #[test]
    fn normalization_and_conflicts() {
        let parsed = load_str(
            r#"
[rules]
version = "v1"
[rules.blacklist]
users = ["A", "ａ"]
tags = ["QOS", "qos"]
watch = ["tentacle"]
"#,
        )
        .unwrap();
        assert_eq!(parsed.config.rules.blacklist.users, vec!["A"]);
        assert_eq!(parsed.config.rules.blacklist.tags, vec!["QOS"]);

        let error = load_str(
            r#"
[rules]
version = "v1"
[rules.blacklist]
tags = ["same"]
watch = ["SAME"]
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("both in tags and watch"));
    }

    #[test]
    fn r18g_is_blocked_by_each_route() {
        let rules = config(&[], &[]);
        let mut subject = subject();
        let db = crate::db::Db::open_in_memory().unwrap();
        assert_eq!(
            evaluate(DecisionInput {
                db: &db,
                config: &rules,
                subject: &subject,
            })
            .unwrap()
            .decision,
            Decision::Allow
        );

        subject.x_restrict = Some(2);
        assert_eq!(
            evaluate(DecisionInput {
                db: &db,
                config: &rules,
                subject: &subject,
            })
            .unwrap()
            .decision,
            Decision::Block
        );
        subject.x_restrict = None;
        subject.tags.push(("R-18G".to_string(), None));
        assert_eq!(
            evaluate(DecisionInput {
                db: &db,
                config: &rules,
                subject: &subject,
            })
            .unwrap()
            .decision,
            Decision::Block
        );
        subject.tags.clear();
        subject.restriction_attributes.push("R18G".to_string());
        assert_eq!(
            evaluate(DecisionInput {
                db: &db,
                config: &rules,
                subject: &subject,
            })
            .unwrap()
            .decision,
            Decision::Block
        );
    }

    #[test]
    fn watch_goes_to_review_not_block() {
        let rules = config(&[], &["tentacle"]);
        let mut subject = subject();
        subject.tags.push(("Tentacle".to_string(), None));
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(outcome.decision, Decision::Review);
        assert_eq!(outcome.reason, "watch:tentacle");
        assert_eq!(outcome.matched, vec!["watch:tentacle"]);
    }

    #[test]
    fn tag_budget_requests_one_model_pass() {
        let rules = config(&["blocked"], &[]);
        let mut subject = subject();
        subject.tags = vec![
            ("オリジナル".to_string(), None),
            ("1000users入り".to_string(), None),
            ("art".to_string(), None),
        ];
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert!(outcome.needs_tagging);
        assert_eq!(outcome.decision, Decision::Review);
    }

    #[test]
    fn tag_blacklist_blocks_even_when_tag_budget_is_short() {
        let rules = config(&["blocked"], &[]);
        let mut subject = subject();
        subject.tags = vec![
            ("blocked".to_string(), None),
            ("オリジナル".to_string(), None),
        ];
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(outcome.decision, Decision::Block);
        assert_eq!(outcome.reason, "tag blacklist");
        assert!(!outcome.needs_tagging);
    }

    #[test]
    fn translated_tags_do_not_double_count_toward_budget() {
        let rules = config(&[], &[]);
        let mut subject = subject();
        subject.tags = vec![
            ("オリジナル".to_string(), Some("original".to_string())),
            ("女の子".to_string(), Some("Girl".to_string())),
            ("百合".to_string(), Some("yuri".to_string())),
            ("好都合セミフレンド".to_string(), None),
            (
                "オリジナル1000users入り".to_string(),
                Some("original 1000+ bookmarks".to_string()),
            ),
        ];
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert!(outcome.needs_tagging, "3 个内容标签应触发补标");
        assert_eq!(outcome.decision, Decision::Review);
    }

    #[test]
    fn five_bilingual_content_tags_meet_budget() {
        let rules = config(&[], &[]);
        let mut subject = subject();
        subject.tags = (0..5)
            .map(|index| (format!("内容{index}"), Some(format!("content {index}"))))
            .collect();
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert!(!outcome.needs_tagging);
        assert_eq!(outcome.decision, Decision::Allow);
    }

    #[test]
    fn sanity_fallback_reviews_instead_of_blocking() {
        let rules = config(&[], &[]);
        let mut subject = subject();
        subject.rating = Rating::Unknown;
        subject.x_restrict = None;
        subject.sanity_level = Some(6);
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(outcome.decision, Decision::Review);
        assert_eq!(outcome.matched, vec!["rating:sanity"]);
    }

    #[test]
    fn ignored_tags_are_compared_after_normalization() {
        let parsed = load_str(
            r#"
[rules]
version = "v1"
[rules.tag_budget]
min_tags = 2
ignore = ["Original"]
"#,
        )
        .unwrap();
        let mut subject = subject();
        subject.tags = vec![("  ORIGINAL ".to_string(), None)];
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &parsed.config,
            subject: &subject,
        })
        .unwrap();
        assert!(outcome.needs_tagging);
        assert_eq!(outcome.decision, Decision::Review);
    }

    #[test]
    fn shipped_rules_template_parses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/rules.toml.example");
        let parsed = load_file(&path).unwrap_or_else(|error| {
            panic!("deploy/rules.toml.example must load: {error}");
        });
        let blacklist = &parsed.config.rules.blacklist;
        assert_eq!(blacklist.users.len(), 9);
        assert_eq!(blacklist.tags.len(), 142);
        assert_eq!(blacklist.watch.len(), 18);
        assert_eq!(parsed.version, "2026-11-22.1");
    }

    fn seed_item(db: &crate::db::Db, id: &str, sha1: &str, created_at: i64) {
        db.connection()
            .execute(
                "INSERT INTO works(id, source, external_id, title)
                 VALUES (?1, 'pixiv', ?1, 'title')",
                [id],
            )
            .unwrap();
        db.connection()
            .execute(
                "INSERT INTO items(id, work_id, source, original_path, width, height, bytes, sha1,
                                   created_at, updated_at)
                 VALUES (?1, ?1, 'pixiv', 'x.png', 800, 800, 10, ?2, ?3, ?3)",
                rusqlite::params![id, sha1, created_at],
            )
            .unwrap();
    }

    #[test]
    fn dedup_on_conflict_review_is_honored() {
        let db = crate::db::Db::open_in_memory().unwrap();
        seed_item(&db, "earlier", "same", 1);
        seed_item(&db, "subject", "same", 2);

        let mut subject = subject();
        subject.item_id = "subject".to_string();
        subject.sha1 = Some("same".to_string());

        let mut rules = config(&[], &[]);
        let block = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(block.decision, Decision::Block);
        assert_eq!(block.reason, "duplicate hash");

        rules.rules.dedup.on_conflict = "review".to_string();
        let review = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(review.decision, Decision::Review);
        assert_eq!(review.matched, vec!["dedup:sha1"]);
    }

    #[test]
    fn size_on_fail_block_is_honored() {
        let db = crate::db::Db::open_in_memory().unwrap();
        let mut subject = subject();
        subject.width = 100;
        let mut rules = config(&[], &[]);
        assert_eq!(
            evaluate(DecisionInput {
                db: &db,
                config: &rules,
                subject: &subject,
            })
            .unwrap()
            .decision,
            Decision::Review
        );

        rules.rules.size.on_fail = "block".to_string();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(outcome.decision, Decision::Block);
        assert_eq!(outcome.reason, "size below minimum");
    }

    #[test]
    fn empty_tag_blacklist_is_reported() {
        let empty = load_str("[rules]\nversion = \"v1\"\n").unwrap();
        assert!(empty.config.tag_blacklist_is_empty());
        let populated =
            load_str("[rules]\nversion = \"v1\"\n[rules.blacklist]\ntags = [\"blocked\"]\n")
                .unwrap();
        assert!(!populated.config.tag_blacklist_is_empty());
    }
}
