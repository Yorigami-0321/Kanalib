//! F-080 云端图片墙发布器：把本地 Kanalib 库导出成静态站点数据目录。
//!
//! 契约唯一来源：`docs/backlog/photo-wall-contract.md`（v1）。本 crate 只读本地库，
//! 产出 `index.json` + `m/NNNN.json` + `t256/t512` 缩略图 + 可选 `o/` 原图 +
//! `publish-ledger.json`，不依赖 gpui / kanalib-ui。
//!
//! 约束（与任务书一致）：
//! - 导出目录内任何文件都不含本地绝对路径，也不写日志/临时文件；
//! - 日志只走 stderr；
//! - 不新增 schema 迁移（沿用 `kanalib_core::db::Db::open` 的既有打开流程）。

pub mod fsutil;
pub mod ledger;
pub mod manifest;
pub mod publish;
pub mod timefmt;

pub use ledger::{Ledger, LedgerEntry};
pub use manifest::{
    DEFAULT_SHARD_SIZE, Item, MAX_FACET_GROUPS, MAX_FACET_TAGS, SCHEMA_VERSION, Shard, ShardRef,
};
pub use publish::{Options, Stats, run};
