//! 规则草稿：管理页只写 `rule_proposals`，人工确认后原子改写 `rules.toml`。
//!
//! 改写用 `toml_edit` 保留注释与键序；版本号自动递增，写入前做漂移与并发检查。

use std::{io::Write, path::Path, sync::Mutex};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use toml_edit::{Array, DocumentMut, Item, Table, Value as TomlValue};

use crate::{db::Db, rules, rules_state::compare_versions};

use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalKind {
    AddUser,
    RemoveUser,
    AddTag,
    RemoveTag,
}

impl ProposalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AddUser => "add_user",
            Self::RemoveUser => "remove_user",
            Self::AddTag => "add_tag",
            Self::RemoveTag => "remove_tag",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "add_user" => Ok(Self::AddUser),
            "remove_user" => Ok(Self::RemoveUser),
            "add_tag" => Ok(Self::AddTag),
            "remove_tag" => Ok(Self::RemoveTag),
            other => bail!("unknown proposal kind {other}"),
        }
    }

    pub fn is_add(self) -> bool {
        matches!(self, Self::AddUser | Self::AddTag)
    }

    fn is_user(self) -> bool {
        matches!(self, Self::AddUser | Self::RemoveUser)
    }

    pub fn table_key(self) -> &'static str {
        if self.is_user() { "users" } else { "tags" }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied { version: String },
    AlreadySatisfied { version: String },
}

impl ApplyOutcome {
    pub fn version(&self) -> &str {
        match self {
            Self::Applied { version } | Self::AlreadySatisfied { version } => version,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffLine {
    pub kind: &'static str,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ProposalPreview {
    pub value: String,
    pub current_version: String,
    pub next_version: String,
    pub already_satisfied: bool,
    pub diff: Vec<DiffLine>,
}

struct EditPlan {
    current_version: String,
    next_version: String,
    already_satisfied: bool,
    original: String,
    edited: Option<String>,
}

/// 串行化同一进程内的规则写入；跨进程并发由写入前后的 hash 检查兜底。
#[derive(Debug, Default)]
pub struct RulesEditor {
    lock: Mutex<()>,
}

impl RulesEditor {
    pub fn create_proposal(
        &self,
        db: &Db,
        kind: ProposalKind,
        value: &str,
        source_item_id: Option<&str>,
        note: &str,
    ) -> Result<i64> {
        let normalized = normalize_value(kind, value)?;
        let payload = payload_json(kind, &normalized);
        db.insert_rule_proposal(kind.as_str(), &payload.to_string(), source_item_id, note)
    }

    /// 只计算改动与逐行 diff，不写文件；apply 复用同一份编辑计划。
    pub fn preview(&self, db: &Db, rules_path: &Path, proposal_id: i64) -> Result<ProposalPreview> {
        let proposal = db
            .rule_proposal(proposal_id)?
            .ok_or_else(|| anyhow::anyhow!("proposal {proposal_id} not found"))?;
        if proposal.state != "pending" {
            bail!(
                "proposal {proposal_id} is {}; only pending proposals can be previewed",
                proposal.state
            );
        }
        let kind = ProposalKind::parse(&proposal.kind)?;
        let value = payload_value(kind, &proposal.payload_json)?;
        let plan = plan_edit(db, rules_path, kind, &value)?;
        let diff = plan
            .edited
            .as_deref()
            .map(|edited| line_diff(&plan.original, edited))
            .unwrap_or_default();
        Ok(ProposalPreview {
            value,
            current_version: plan.current_version,
            next_version: plan.next_version,
            already_satisfied: plan.already_satisfied,
            diff,
        })
    }

    pub fn apply(
        &self,
        db: &Db,
        rules_path: &Path,
        proposal_id: i64,
        decided_by: &str,
    ) -> Result<ApplyOutcome> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let proposal = db
            .rule_proposal(proposal_id)?
            .ok_or_else(|| anyhow::anyhow!("proposal {proposal_id} not found"))?;
        match proposal.state.as_str() {
            "applied" => {
                return Ok(ApplyOutcome::AlreadySatisfied {
                    version: current_version(db),
                });
            }
            "pending" => {}
            other => {
                bail!("proposal {proposal_id} is {other}; only pending proposals can be applied")
            }
        }
        let kind = ProposalKind::parse(&proposal.kind)?;
        let value = payload_value(kind, &proposal.payload_json)?;
        let plan = plan_edit(db, rules_path, kind, &value)?;
        if plan.already_satisfied {
            db.set_rule_proposal_state(proposal_id, "applied", decided_by, None)?;
            return Ok(ApplyOutcome::AlreadySatisfied {
                version: plan.current_version,
            });
        }
        let edited = plan
            .edited
            .ok_or_else(|| anyhow::anyhow!("edit plan is missing content"))?;
        let latest = std::fs::read_to_string(rules_path)
            .with_context(|| format!("re-reading rules file {}", rules_path.display()))?;
        if latest != plan.original {
            bail!("rules file changed while applying proposal; nothing was written");
        }
        atomic_write(rules_path, &edited)?;
        db.set_rule_proposal_state(proposal_id, "applied", decided_by, None)?;
        Ok(ApplyOutcome::Applied {
            version: plan.next_version,
        })
    }

    pub fn reject(&self, db: &Db, proposal_id: i64, decided_by: &str) -> Result<()> {
        let proposal = db
            .rule_proposal(proposal_id)?
            .ok_or_else(|| anyhow::anyhow!("proposal {proposal_id} not found"))?;
        if proposal.state != "pending" {
            bail!(
                "proposal {proposal_id} is {}; only pending proposals can be rejected",
                proposal.state
            );
        }
        db.set_rule_proposal_state(proposal_id, "rejected", decided_by, None)
    }

    pub fn record_apply_error(&self, db: &Db, proposal_id: i64, error: &str) -> Result<()> {
        db.set_rule_proposal_state(proposal_id, "pending", "system", Some(error))
    }
}

fn plan_edit(db: &Db, rules_path: &Path, kind: ProposalKind, value: &str) -> Result<EditPlan> {
    let text = std::fs::read_to_string(rules_path)
        .with_context(|| format!("reading rules file {}", rules_path.display()))?;
    let current = rules::load_str(&text).context("validating current rules file")?;
    if let Some(active) = db.latest_rule_snapshot()?
        && active.config_hash != current.config_hash
    {
        bail!(
            "rules file differs from active snapshot {}; reload before applying proposals",
            active.version
        );
    }

    let mut document: DocumentMut = text.parse().context("parsing rules file for edit")?;
    let array = blacklist_array_mut(&mut document, kind)?;
    let present = array
        .iter()
        .filter_map(TomlValue::as_str)
        .any(|existing| rules::normalize_tag(existing) == rules::normalize_tag(value));
    if kind.is_add() == present {
        return Ok(EditPlan {
            current_version: current.version.clone(),
            next_version: current.version,
            already_satisfied: true,
            original: text,
            edited: None,
        });
    }
    if kind.is_add() {
        array.push(value.to_string());
    } else {
        let index = array
            .iter()
            .position(|existing| {
                existing
                    .as_str()
                    .is_some_and(|s| rules::normalize_tag(s) == rules::normalize_tag(value))
            })
            .expect("entry checked above");
        array.remove(index);
    }

    let next_version = bump_version(&current.version)?;
    set_version(&mut document, &next_version)?;
    let edited = document.to_string();
    let validated = rules::load_str(&edited).context("validating edited rules")?;
    if compare_versions(&validated.version, &current.version) != Ordering::Greater {
        bail!(
            "edited rules.version {} is not newer than {}",
            validated.version,
            current.version
        );
    }
    Ok(EditPlan {
        current_version: current.version,
        next_version: validated.version,
        already_satisfied: false,
        original: text,
        edited: Some(edited),
    })
}

/// 无依赖的行级 LCS diff：足够预览规则文件，不引入额外 crate。
pub fn line_diff(before: &str, after: &str) -> Vec<DiffLine> {
    let before = before.lines().collect::<Vec<_>>();
    let after = after.lines().collect::<Vec<_>>();
    let mut lcs = vec![vec![0usize; after.len() + 1]; before.len() + 1];
    for i in (0..before.len()).rev() {
        for j in (0..after.len()).rev() {
            lcs[i][j] = if before[i] == after[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut diff = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < before.len() && j < after.len() {
        if before[i] == after[j] {
            diff.push(DiffLine {
                kind: "context",
                text: before[i].to_string(),
            });
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            diff.push(DiffLine {
                kind: "remove",
                text: before[i].to_string(),
            });
            i += 1;
        } else {
            diff.push(DiffLine {
                kind: "add",
                text: after[j].to_string(),
            });
            j += 1;
        }
    }
    for line in &before[i..] {
        diff.push(DiffLine {
            kind: "remove",
            text: (*line).to_string(),
        });
    }
    for line in &after[j..] {
        diff.push(DiffLine {
            kind: "add",
            text: (*line).to_string(),
        });
    }
    diff
}

fn current_version(db: &Db) -> String {
    db.latest_rule_snapshot()
        .ok()
        .flatten()
        .map(|snapshot| snapshot.version)
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn normalize_value(kind: ProposalKind, raw: &str) -> Result<String> {
    let value = if kind.is_user() {
        raw.trim().to_string()
    } else {
        let normalized = rules::normalize_tag(raw);
        if normalized.is_empty() {
            bail!("tag must not be empty");
        }
        normalized
    };
    if value.is_empty() {
        bail!("value must not be empty");
    }
    Ok(value)
}

pub fn payload_json(kind: ProposalKind, value: &str) -> Value {
    if kind.is_user() {
        match value.parse::<i64>() {
            Ok(id) => json!({ "user_id": id }),
            Err(_) => json!({ "user_id": value }),
        }
    } else {
        json!({ "tag": value })
    }
}

pub fn payload_value(kind: ProposalKind, payload: &str) -> Result<String> {
    let payload: Value = serde_json::from_str(payload).context("parsing proposal payload")?;
    let key = if kind.is_user() { "user_id" } else { "tag" };
    let value = match payload.get(key) {
        Some(Value::String(value)) => value.trim().to_string(),
        Some(Value::Number(value)) => value.to_string(),
        _ => bail!("proposal payload is missing {key}"),
    };
    if value.is_empty() {
        bail!("proposal payload {key} is empty");
    }
    Ok(value)
}

pub fn bump_version(version: &str) -> Result<String> {
    let mut parts = version.split('.').map(str::to_string).collect::<Vec<_>>();
    if let Some(last) = parts.last_mut()
        && let Ok(number) = last.parse::<u64>()
    {
        *last = number.saturating_add(1).to_string();
        return Ok(parts.join("."));
    }
    if version.trim().is_empty() {
        bail!("rules.version must not be empty");
    }
    Ok(format!("{version}.1"))
}

fn blacklist_array_mut(document: &mut DocumentMut, kind: ProposalKind) -> Result<&mut Array> {
    let rules = document.entry("rules").or_insert(Item::Table(Table::new()));
    let rules = rules.as_table_mut().context("rules must be a table")?;
    let blacklist = rules
        .entry("blacklist")
        .or_insert(Item::Table(Table::new()));
    let blacklist = blacklist
        .as_table_mut()
        .context("rules.blacklist must be a table")?;
    let array = blacklist
        .entry(kind.table_key())
        .or_insert(Item::Value(TomlValue::Array(Array::new())));
    array
        .as_array_mut()
        .with_context(|| format!("rules.blacklist.{} must be an array", kind.table_key()))
}

fn set_version(document: &mut DocumentMut, version: &str) -> Result<()> {
    let rules = document
        .get_mut("rules")
        .and_then(Item::as_table_mut)
        .context("rules must be a table")?;
    if let Some(existing) = rules.get_mut("version").and_then(Item::as_value_mut) {
        let decor = existing.decor().clone();
        *existing = TomlValue::from(version);
        *existing.decor_mut() = decor;
    } else {
        rules.insert("version", toml_edit::value(version));
    }
    Ok(())
}

fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .context("rules path has no file name")?
        .to_string_lossy();
    let temp = directory.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file =
            std::fs::File::create(&temp).with_context(|| format!("creating {}", temp.display()))?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
            .with_context(|| format!("renaming {} to {}", temp.display(), path.display()))?;
        #[cfg(unix)]
        if let Ok(directory) = std::fs::File::open(directory) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules_state::{ReloadOutcome, RulesState};

    const BASE: &str = r#"# keep this comment
[rules]
version = "2026-11-22.1" # version comment

[rules.blacklist]
users = ["111"]
tags = ["guro"]
watch = ["tentacle"]
"#;

    fn setup() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        Db,
        RulesState,
        RulesEditor,
    ) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, BASE).unwrap();
        let db = Db::open_in_memory().unwrap();
        let state = RulesState::load(&db, Some(&path)).unwrap();
        (dir, path, db, state, RulesEditor::default())
    }

    #[test]
    fn add_tag_preserves_comments_and_bumps_version() {
        let (_dir, path, db, state, editor) = setup();
        let id = editor
            .create_proposal(&db, ProposalKind::AddTag, "  GROO ", None, "test")
            .unwrap();
        let outcome = editor.apply(&db, &path, id, "admin").unwrap();
        assert_eq!(
            outcome,
            ApplyOutcome::Applied {
                version: "2026-11-22.2".to_string()
            }
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep this comment"));
        assert!(text.contains("# version comment"));
        assert!(text.contains("\"groo\""));
        assert_eq!(state.reload(&db, "test").unwrap(), ReloadOutcome::Reloaded);
        assert_eq!(state.current().version, "2026-11-22.2");
        assert!(
            state
                .current()
                .config
                .rules
                .blacklist
                .tags
                .contains(&"groo".to_string())
        );
        assert_eq!(db.rule_proposal(id).unwrap().unwrap().state, "applied");
    }

    #[test]
    fn duplicate_add_and_missing_remove_are_idempotent() {
        let (_dir, path, db, _state, editor) = setup();
        let add = editor
            .create_proposal(&db, ProposalKind::AddTag, "guro", None, "")
            .unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(matches!(
            editor.apply(&db, &path, add, "admin").unwrap(),
            ApplyOutcome::AlreadySatisfied { .. }
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        let remove = editor
            .create_proposal(&db, ProposalKind::RemoveTag, "absent", None, "")
            .unwrap();
        assert!(matches!(
            editor.apply(&db, &path, remove, "admin").unwrap(),
            ApplyOutcome::AlreadySatisfied { .. }
        ));
    }

    #[test]
    fn remove_tag_rewrites_array_and_bumps_version() {
        let (_dir, path, db, _state, editor) = setup();
        let id = editor
            .create_proposal(&db, ProposalKind::RemoveTag, "guro", None, "")
            .unwrap();
        assert!(matches!(
            editor.apply(&db, &path, id, "admin").unwrap(),
            ApplyOutcome::Applied { .. }
        ));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("guro"));
        let reloaded = rules::load_str(&text).unwrap();
        assert_eq!(reloaded.version, "2026-11-22.2");
        assert!(reloaded.config.rules.blacklist.tags.is_empty());
    }

    #[test]
    fn apply_aborts_when_file_drifted_from_snapshot() {
        let (_dir, path, db, _state, editor) = setup();
        let id = editor
            .create_proposal(&db, ProposalKind::AddTag, "newtag", None, "")
            .unwrap();
        std::fs::write(
            &path,
            BASE.replace("version = \"2026-11-22.1\"", "version = \"2026-11-22.9\""),
        )
        .unwrap();
        let error = editor.apply(&db, &path, id, "admin").unwrap_err();
        assert!(error.to_string().contains("differs from active snapshot"));
    }

    #[test]
    fn version_bump_handles_numeric_and_plain_versions() {
        assert_eq!(bump_version("2026-11-22.1").unwrap(), "2026-11-22.2");
        assert_eq!(bump_version("2026-11-22").unwrap(), "2026-11-22.1");
        assert_eq!(bump_version("v1").unwrap(), "v1.1");
    }

    #[test]
    fn line_diff_marks_added_and_removed_lines() {
        let diff = line_diff("a\nb\n", "a\nc\n");
        assert_eq!(
            diff,
            vec![
                DiffLine {
                    kind: "context",
                    text: "a".to_string(),
                },
                DiffLine {
                    kind: "remove",
                    text: "b".to_string(),
                },
                DiffLine {
                    kind: "add",
                    text: "c".to_string(),
                },
            ]
        );
    }

    #[test]
    fn preview_returns_diff_without_writing() {
        let (_dir, path, db, _state, editor) = setup();
        let id = editor
            .create_proposal(&db, ProposalKind::AddTag, "new-tag", None, "")
            .unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let preview = editor.preview(&db, &path, id).unwrap();
        assert_eq!(preview.current_version, "2026-11-22.1");
        assert_eq!(preview.next_version, "2026-11-22.2");
        assert!(!preview.already_satisfied);
        assert!(
            preview
                .diff
                .iter()
                .any(|line| line.kind == "add" && line.text.contains("new-tag"))
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "preview must not write"
        );
    }
}
