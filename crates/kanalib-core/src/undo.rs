//! 会话内撤销栈（F-067）。
//!
//! 这里只保存可逆编辑的“前值”，不持久化到数据库，也不记录删除/回收站、
//! 去重评审、来源移除或文件系统操作。`AppState` 在四个 mutation 入口成功后
//! 自动写入；`undo_last` 先克隆栈顶条目，执行成功后才按 id 移除，失败时
//! 原样保留，避免一次失败吞掉整段历史。

use std::collections::VecDeque;
use std::path::PathBuf;

use anyhow::{Result, bail};

/// 默认最多保留 50 条；溢出时丢最旧的一条。
pub const DEFAULT_UNDO_LIMIT: usize = 50;

/// `asset_tags.origin`；恢复时按原 origin 精确回写，避免把手工标签变成自动标签。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagOrigin {
    Auto,
    Manual,
}

impl TagOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }
}

/// 移除标签前保存的一行 `asset_tags`；类别/分数/词表索引用于精确恢复。
#[derive(Clone, Debug, PartialEq)]
pub struct TagSnapshot {
    pub path: PathBuf,
    pub tag: String,
    pub category: String,
    pub score: f32,
    pub vocab_index: Option<u32>,
    pub origin: TagOrigin,
}

/// 一类用户编辑的旧值。撤销只恢复旧值，不再产生新的撤销条目。
#[derive(Clone, Debug, PartialEq)]
pub enum UndoAction {
    /// 收藏前状态；只包含实际发生变化的路径。
    Favorite { changes: Vec<(PathBuf, bool)> },
    /// 星级前状态；只包含实际发生变化的路径。
    Star { changes: Vec<(PathBuf, u8)> },
    /// 本次实际新增了 manual 行的路径；撤销只删除 `origin = 'manual'`。
    ManualTagsAdded { paths: Vec<PathBuf>, name: String },
    /// 本次删除的 auto/manual 行；撤销用精确恢复辅助写回。
    TagsRemoved {
        name: String,
        rows: Vec<TagSnapshot>,
    },
}

/// 栈内一条操作记录；`id` 自增，用于执行成功后定位并移除同一条目。
#[derive(Clone, Debug, PartialEq)]
pub struct UndoEntry {
    pub id: u64,
    pub label: String,
    pub action: UndoAction,
}

/// 撤销结果；`affected` 是实际恢复/变化的资产数，具体口径由执行方决定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndoOutcome {
    pub label: String,
    pub affected: usize,
}

/// 有界后进先出栈；`undo_last` 接收执行闭包，保证“成功才移除”。
#[derive(Debug)]
pub struct UndoStack {
    entries: VecDeque<UndoEntry>,
    limit: usize,
    next_id: u64,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new(DEFAULT_UNDO_LIMIT)
    }
}

impl UndoStack {
    /// 上限至少为 1；0 会被当作 1，避免记录后立刻被淘汰。
    pub fn new(limit: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            limit: limit.max(1),
            next_id: 0,
        }
    }

    /// 记录一条操作并返回自增 id；超过上限时丢最旧条目。
    pub fn record(&mut self, label: impl Into<String>, action: UndoAction) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.entries.push_back(UndoEntry {
            id,
            label: label.into(),
            action,
        });
        while self.entries.len() > self.limit {
            self.entries.pop_front();
        }
        id
    }

    pub fn can_undo(&self) -> bool {
        !self.entries.is_empty()
    }

    pub fn depth(&self) -> usize {
        self.entries.len()
    }

    pub fn peek_label(&self) -> Option<&str> {
        self.entries.back().map(|entry| entry.label.as_str())
    }

    /// 克隆栈顶 → 执行 → 按 id 移除；执行失败时不修改栈。
    pub fn undo_last<F>(&mut self, apply: F) -> Result<UndoOutcome>
    where
        F: FnOnce(&UndoEntry) -> Result<usize>,
    {
        let Some(entry) = self.entries.back().cloned() else {
            bail!("没有可撤销的操作");
        };
        let affected = apply(&entry)?;
        if let Some(index) = self.entries.iter().position(|item| item.id == entry.id) {
            self.entries.remove(index);
        }
        Ok(UndoOutcome {
            label: entry.label,
            affected,
        })
    }

    #[cfg(test)]
    fn peek_id(&self) -> Option<u64> {
        self.entries.back().map(|entry| entry.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(stack: &mut UndoStack, label: &str) {
        stack.record(
            label,
            UndoAction::Favorite {
                changes: Vec::new(),
            },
        );
    }

    #[test]
    fn undo_is_lifo_and_removes_only_after_success() {
        let mut stack = UndoStack::new(10);
        push(&mut stack, "first");
        push(&mut stack, "second");
        assert!(stack.can_undo());
        assert_eq!(stack.depth(), 2);
        assert_eq!(stack.peek_label(), Some("second"));

        let outcome = stack
            .undo_last(|entry| {
                assert_eq!(entry.label, "second");
                Ok(3)
            })
            .unwrap();
        assert_eq!(
            outcome,
            UndoOutcome {
                label: "second".into(),
                affected: 3
            }
        );
        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.peek_label(), Some("first"));
        assert!(stack.undo_last(|_| Ok(0)).is_ok());
        assert!(!stack.can_undo());
    }

    #[test]
    fn empty_stack_returns_chinese_error() {
        let mut stack = UndoStack::default();
        let error = stack.undo_last(|_| Ok(0)).unwrap_err();
        assert!(error.to_string().contains("没有可撤销的操作"));
        assert!(!stack.can_undo());
    }

    #[test]
    fn failed_undo_keeps_entry_and_reports_error() {
        let mut stack = UndoStack::new(10);
        push(&mut stack, "keep");
        let id = stack.peek_id();
        let error = stack.undo_last(|_| bail!("数据库恢复失败")).unwrap_err();
        assert!(error.to_string().contains("数据库恢复失败"));
        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.peek_label(), Some("keep"));
        assert_eq!(stack.peek_id(), id, "失败不能替换或丢弃栈顶条目");

        assert!(stack.undo_last(|_| Ok(1)).is_ok());
        assert_eq!(stack.depth(), 0);
    }

    #[test]
    fn default_limit_is_fifty_and_drops_oldest() {
        let mut stack = UndoStack::default();
        for index in 0..51 {
            push(&mut stack, &index.to_string());
        }
        assert_eq!(stack.depth(), DEFAULT_UNDO_LIMIT);
        assert_eq!(stack.peek_label(), Some("50"));

        let mut labels = Vec::new();
        while stack.can_undo() {
            labels.push(stack.undo_last(|_| Ok(0)).unwrap().label);
        }
        assert_eq!(labels.len(), DEFAULT_UNDO_LIMIT);
        assert_eq!(labels.first().map(String::as_str), Some("50"));
        assert_eq!(labels.last().map(String::as_str), Some("1"));
        assert!(!labels.iter().any(|label| label == "0"));
    }
}
