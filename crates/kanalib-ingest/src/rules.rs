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
            version: "2026-11-25.1".to_string(),
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
    pub combos: Vec<ComboRule>,
}

/// 「只有 X、没有 Y」这类单个词表达不了的条件。
///
/// `any` 命中任一即成立，`none` 命中任一即否决；两个表都按标签归一化后精确匹配。
/// 例：`any = ["1boy", "2boys"]` + `none = ["1girl", "2girls"]` 表达「只有男性」。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ComboRule {
    pub name: String,
    pub any: Vec<String>,
    pub none: Vec<String>,
    /// 两组信号都没出现时，先补一次打标再判；补完仍无信号则照常往下走。
    pub needs_tags: bool,
    /// block | review
    pub action: String,
}

impl Default for ComboRule {
    fn default() -> Self {
        Self {
            name: String::new(),
            any: Vec::new(),
            none: Vec::new(),
            needs_tags: true,
            action: "block".to_string(),
        }
    }
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
    for combo in &mut config.rules.blacklist.combos {
        combo.name = combo.name.trim().to_string();
        if combo.name.is_empty() {
            bail!("rules.blacklist.combos contains an empty name");
        }
        if combo.any.is_empty() {
            bail!(
                "rules.blacklist.combos {:?} has an empty any list",
                combo.name
            );
        }
        if combo.action != "block" && combo.action != "review" {
            bail!(
                "rules.blacklist.combos {:?} action must be block or review",
                combo.name
            );
        }
        let label = format!("combos.{}", combo.name);
        combo.any = normalize_unique(std::mem::take(&mut combo.any), &label)?;
        combo.none = normalize_unique(std::mem::take(&mut combo.none), &label)?;
        let any: HashSet<_> = combo.any.iter().map(|tag| normalize_tag(tag)).collect();
        for tag in &combo.none {
            if any.contains(&normalize_tag(tag)) {
                bail!(
                    "rules.blacklist.combos {:?} has {tag:?} in both any and none",
                    combo.name
                );
            }
        }
    }
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
    /// 本轮判定前是否已经跑过模型补标；组合条件只在补标前要求补一次，避免反复触发。
    pub tagged: bool,
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
    if subject.ai == AiType::Ai && !config.rules.ai.allow {
        matched.push("ai:generated".to_string());
        return Ok(RuleOutcome::block("ai generation disabled", matched));
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
    // 组合条件先算：它比单标签更具体，命中后不必再看后面的兜底规则。
    let combo_hits = |tags: &HashSet<String>| -> Vec<(&ComboRule, String)> {
        config
            .rules
            .blacklist
            .combos
            .iter()
            .filter_map(|combo| {
                // 只要 `none` 出现，这条组合就整体不成立；哪怕 `any` 也命中。
                if combo
                    .none
                    .iter()
                    .map(|rule| normalize_tag(rule))
                    .any(|rule| tags.contains(&rule))
                {
                    return None;
                }
                let hit = combo
                    .any
                    .iter()
                    .map(|rule| normalize_tag(rule))
                    .find(|rule| tags.contains(rule))?;
                Some((combo, format!("combo:{}:{hit}", combo.name)))
            })
            .collect()
    };
    // 两边的词都没出现时，组合条件无从判断：先补一次标签，补完仍无信号才放弃。
    let combo_needs_tags = |tags: &HashSet<String>| -> bool {
        !subject.tagged
            && config.rules.blacklist.combos.iter().any(|combo| {
                combo.needs_tags
                    && !combo
                        .any
                        .iter()
                        .chain(combo.none.iter())
                        .map(|rule| normalize_tag(rule))
                        .any(|rule| tags.contains(&rule))
            })
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

    let rating = match (subject.rating, subject.x_restrict) {
        (Rating::Unknown, Some(0)) => "all",
        (Rating::Unknown, Some(1)) => "r18",
        (Rating::Unknown, Some(2)) => "r18g",
        (Rating::All, _) => "all",
        (Rating::R18, _) => "r18",
        (Rating::R18G, _) => "r18g",
        (Rating::Unknown, _) => "unknown",
    };
    if config
        .rules
        .rating
        .block
        .iter()
        .any(|value| value == rating)
    {
        matched.push(format!("rating:{rating}"));
        return Ok(RuleOutcome::block("rating blocked", matched));
    }
    if rating == "unknown" {
        matched.push("rating:unknown".to_string());
        return Ok(RuleOutcome::review("rating unknown", matched));
    }
    if !config
        .rules
        .rating
        .allow
        .iter()
        .any(|value| value == rating)
    {
        matched.push(format!("rating:{rating}"));
        return Ok(RuleOutcome::block("rating not allowed", matched));
    }

    let mut hits = blacklist_hits(&tags);
    if !hits.is_empty() {
        matched.extend(hits);
        return Ok(RuleOutcome::block("tag blacklist", matched));
    }

    let combos = combo_hits(&tags);
    if !combos.is_empty() {
        // 同一作品可能同时命中 block 与 review 的组合；取更严的那个。
        let action = if combos.iter().any(|(combo, _)| combo.action == "block") {
            "block"
        } else {
            "review"
        };
        matched.extend(combos.iter().map(|(_, hit)| hit.clone()));
        return Ok(policy_outcome(action, "combo rule", matched));
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

    // 组合条件依赖标签，排在去重/观察词/尺寸之后：这几条已有终态，没必要为它们白跑一次模型。
    if combo_needs_tags(&tags) {
        return Ok(RuleOutcome {
            decision: Decision::Review,
            reason: "combo rule needs tags".to_string(),
            matched,
            needs_tagging: true,
        });
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
                    ..Blacklist::default()
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
            tagged: false,
            tag_count: 5,
            width: 800,
            height: 800,
            bytes: 100,
            sha1: Some("hash".to_string()),
            item_id: "item".to_string(),
        }
    }

    fn combo_config(combos: &str) -> std::sync::Arc<RuleConfig> {
        load_str(&format!(
            r#"
[rules]
version = "v1"
{combos}
"#
        ))
        .unwrap()
        .config
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
        subject.tags = vec![("Tentacle".to_string(), None)];
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
        assert!(!outcome.needs_tagging);
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
    fn creature_only_combo_blocks_the_raichu_screenshot() {
        let rules = combo_config(
            r#"
[[rules.blacklist.combos]]
name = "creature_only"
any = ["pokemon", "pokemon_(creature)", "creature", "monster"]
none = ["1girl", "1boy", "multiple_boys", "multiple_girls", "people"]
action = "block"
"#,
        );
        let mut subject = subject();
        // 打标模型对第一张截图的真实输出（节选）。
        subject.tags = [
            "raichu",
            "highres",
            "pokemon",
            "rating:g",
            "can",
            "no_humans",
            "canned_food",
            "fish",
            "chibi",
            "pokemon_(creature)",
        ]
        .iter()
        .map(|tag| (tag.to_string(), None))
        .collect();
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(outcome.decision, Decision::Block);
        assert_eq!(outcome.reason, "combo rule");
        assert_eq!(outcome.matched, vec!["combo:creature_only:pokemon"]);
    }

    #[test]
    fn male_only_combo_blocks_the_two_boys_screenshot() {
        let rules = combo_config(
            r#"
[[rules.blacklist.combos]]
name = "male_only"
any = ["1boy", "2boys", "multiple_boys", "male_focus"]
none = ["1girl", "2girls", "multiple_girls", "female", "girl"]
action = "block"
"#,
        );
        let mut subject = subject();
        // 打标模型对第二张截图的真实输出（节选）。
        subject.tags = [
            "highres",
            "mask",
            "persona_5",
            "rating:g",
            "multiple_boys",
            "2boys",
            "eye_mask",
            "male_focus",
            "short_hair",
            "gloves",
        ]
        .iter()
        .map(|tag| (tag.to_string(), None))
        .collect();
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(outcome.decision, Decision::Block);
        assert_eq!(outcome.reason, "combo rule");
        assert_eq!(outcome.matched, vec!["combo:male_only:2boys"]);
    }

    #[test]
    fn combo_none_vetoes_even_when_any_matches() {
        let rules = combo_config(
            r#"
[[rules.blacklist.combos]]
name = "male_only"
any = ["1boy", "2boys"]
none = ["1girl"]
action = "block"
"#,
        );
        let mut subject = subject();
        subject.tags = vec![
            ("1boy".to_string(), None),
            ("1girl".to_string(), None),
            ("tag2".to_string(), None),
            ("tag3".to_string(), None),
            ("tag4".to_string(), None),
        ];
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(outcome.decision, Decision::Allow);
    }

    #[test]
    fn combo_without_signal_asks_for_one_tagging_pass_only() {
        let rules = combo_config(
            r#"
[[rules.blacklist.combos]]
name = "male_only"
any = ["1boy", "2boys"]
none = ["1girl"]
needs_tags = true
action = "block"
"#,
        );
        let db = crate::db::Db::open_in_memory().unwrap();
        let mut subject = subject();
        let first = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert!(first.needs_tagging);
        assert_eq!(first.reason, "combo rule needs tags");

        // 补标后仍无信号：不再要求第二次补标。
        subject.tagged = true;
        let second = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert!(!second.needs_tagging);
        assert_eq!(second.decision, Decision::Allow);
    }

    #[test]
    fn combo_rejects_conflicting_any_and_none_entries() {
        let error = load_str(
            r#"
[rules]
version = "v1"
[[rules.blacklist.combos]]
name = "bad"
any = ["1boy"]
none = ["1BOY"]
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("in both any and none"));
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
    fn ai_generation_can_be_disabled_by_config() {
        let mut rules = config(&[], &[]);
        rules.rules.ai.allow = false;
        let mut subject = subject();
        subject.ai = AiType::Ai;
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(outcome.decision, Decision::Block);
        assert_eq!(outcome.reason, "ai generation disabled");
        assert_eq!(outcome.matched, vec!["ai:generated"]);

        subject.ai = AiType::Unknown;
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(
            outcome.decision,
            Decision::Allow,
            "unknown AI state must not be treated as AI"
        );
    }

    #[test]
    fn rating_allow_list_is_enforced() {
        let mut rules = config(&[], &[]);
        rules.rules.rating.allow = vec!["all".to_string()];
        let mut subject = subject();
        subject.rating = Rating::R18;
        subject.x_restrict = Some(1);
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(outcome.decision, Decision::Block);
        assert_eq!(outcome.reason, "rating not allowed");
        assert_eq!(outcome.matched, vec!["rating:r18"]);
    }

    #[test]
    fn unknown_rating_goes_to_review() {
        let rules = config(&[], &[]);
        let mut subject = subject();
        subject.rating = Rating::Unknown;
        subject.x_restrict = None;
        subject.sanity_level = Some(1);
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(outcome.decision, Decision::Review);
        assert_eq!(outcome.reason, "rating unknown");
        assert_eq!(outcome.matched, vec!["rating:unknown"]);
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
        assert_eq!(blacklist.combos.len(), 2);
        let names = blacklist
            .combos
            .iter()
            .map(|combo| combo.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["creature_only", "male_only"]);
        assert_eq!(parsed.version, "2026-11-25.1");
    }

    /// 用发版规则模板 + 打标模型对两张截图的真实输出，确认组合条件确实生效。
    #[test]
    fn shipped_template_blocks_both_reported_screenshots() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/rules.toml.example");
        let rules = load_file(&path).unwrap().config;
        let db = crate::db::Db::open_in_memory().unwrap();

        let cases = [
            (
                "raichu",
                vec![
                    "raichu",
                    "highres",
                    "pokemon",
                    "rating:g",
                    ":3",
                    "can",
                    "no_humans",
                    "canned_food",
                    "fish",
                    "chibi",
                    "open_mouth",
                    "blush",
                    "pokemon_(creature)",
                    "drink_can",
                    "tears",
                    "smile",
                    "pillarboxed",
                    "sparkle",
                    "animal_focus",
                    "crying",
                    "solo",
                ],
            ),
            (
                "persona5",
                vec![
                    "highres",
                    "mask",
                    "persona_5",
                    "rating:g",
                    "multiple_boys",
                    "2boys",
                    "eye_mask",
                    "epaulettes",
                    "red_eyes",
                    "male_focus",
                    "short_hair",
                    "open_mouth",
                    "pants",
                    "smile",
                    "persona",
                    "looking_at_viewer",
                    "gloves",
                    "black_hair",
                    "masquerade_mask",
                    "long_sleeves",
                    "jacket",
                    "absurdres",
                    "amamiya_ren",
                    "red_nails",
                    "utaite",
                    "red_hair",
                    ":d",
                    "blue_eyes",
                    "white_pants",
                    "art_program_in_frame",
                ],
            ),
        ];
        for (label, tags) in cases {
            let mut subject = subject();
            subject.tags = tags.iter().map(|tag| (tag.to_string(), None)).collect();
            let outcome = evaluate(DecisionInput {
                db: &db,
                config: &rules,
                subject: &subject,
            })
            .unwrap_or_else(|error| panic!("{label} must evaluate: {error}"));
            assert_eq!(
                outcome.decision,
                Decision::Block,
                "{label} should be blocked by a combo rule, got {outcome:?}"
            );
            assert_eq!(outcome.reason, "combo rule", "{label}");
        }
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
        subject.tags.truncate(1);

        let mut rules = config(&[], &[]);
        let block = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(block.decision, Decision::Block);
        assert_eq!(block.reason, "duplicate hash");
        assert!(!block.needs_tagging);

        rules.rules.dedup.on_conflict = "review".to_string();
        let review = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(review.decision, Decision::Review);
        assert_eq!(review.reason, "duplicate hash");
        assert_eq!(review.matched, vec!["dedup:sha1"]);
        assert!(!review.needs_tagging);
    }

    #[test]
    fn size_on_fail_block_is_honored() {
        let db = crate::db::Db::open_in_memory().unwrap();
        let mut subject = subject();
        subject.width = 100;
        subject.tags.truncate(1);
        let mut rules = config(&[], &[]);
        let review = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(review.decision, Decision::Review);
        assert!(!review.needs_tagging);

        rules.rules.size.on_fail = "block".to_string();
        let outcome = evaluate(DecisionInput {
            db: &db,
            config: &rules,
            subject: &subject,
        })
        .unwrap();
        assert_eq!(outcome.decision, Decision::Block);
        assert_eq!(outcome.reason, "size below minimum");
        assert!(!outcome.needs_tagging);
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
