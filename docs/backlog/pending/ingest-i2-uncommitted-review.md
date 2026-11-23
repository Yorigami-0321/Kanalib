# F-089 / I2 未提交改动评审记录（2026-11-22）

> 上级文档：[ingest-service.md](../ingest-service.md) · 交接：[ingest-i2-handoff.md](ingest-i2-handoff.md)
>
> 本文记录对 `main` 工作树中未提交改动的一次代码评审。当前工作树仍在变化（评审过程中 `db.rs` 从 schema v6 升到 v7，并新增了 `rules_edit.rs` / `metrics.rs`），因此结论按当时读到的代码落笔；修复后要同步更新本文。
>
> **2026-11-22 状态更新**：2.2（`dedup.on_conflict`）与 2.3（`size.on_fail`）已真正分流；2.4（单条异常阻断整轮）已改为单条转 `review` 后继续；2.1 采用“空词表启动/重载告警 + 管理页提示”，仍允许启动。第 4 节修复清单 1–3 已完成，当前实现以 [ingest-i2-handoff.md](ingest-i2-handoff.md) 为准。

## 1. 评审范围

- 已跟踪修改：`Cargo.toml`、`Cargo.lock`、`crates/kanalib-ingest/{Cargo.toml,src/db.rs,src/ingest.rs,src/lib.rs,src/main.rs,tests/push_flow.rs}`、`docs/backlog/{ingest-service.md,registry.md}`。
- 新增文件：`crates/kanalib-ingest/src/{pipeline.rs,rules.rs,rules_state.rs,tagging.rs,review.rs,rules_edit.rs,metrics.rs}`、`crates/kanalib-ingest/tests/decision_flow.rs`、`deploy/rules.toml.example`、`docs/backlog/pending/ingest-i2-handoff.md`。
- 未执行测试：评审环境中 `cargo` 不在 PATH；交接文档中记录的上一次全绿结果不能直接覆盖当前仍在变化的工作树。

## 2. 发现

### 2.1 [P1] 未配置 `rules_file` 时空黑名单静默放行

`RulesState::load()` 在未配置 `service.rules_file` 时加载 `DEFAULT_RULES`，日志写的是 `using safe default rules`；但 `RulesSection::default()` 的 `Blacklist` 三个数组全空。于是除 R-18G、去重、尺寸外，所有条目都会走到 `allow`。

这与 [ingest-service.md](../ingest-service.md) §7.6「不存在默认 allow 的兜底配置」相反。生产环境漏配规则文件会变成静默放行，建议缺文件时拒绝启动，或让默认规则 fail-closed。

相关位置：`crates/kanalib-ingest/src/rules_state.rs:57`、`crates/kanalib-ingest/src/rules.rs:38`。

**2026-11-23 状态更新**：已改为缺文件即拒绝启动。`RulesState::load()` 在 `path` 为 `None` 时直接 `bail!`，不再加载 `DEFAULT_RULES`；内置默认规则拆到 `RulesState::load_without_file()`，仅 `main.rs` 在来源为 `mock` 且未配置规则文件时调用。日志措辞同步改为写明「因为来源是 mock 才用默认规则」。§7.6 补了缺文件与空表的区别。

### 2.2 [P2] `dedup.on_conflict` 被静默忽略

规则校验允许 `dedup.on_conflict = "block|review"`，`deploy/rules.toml.example` 也把它作为可配开关；但 `evaluate()` 硬编码 `RuleOutcome::block("duplicate hash", matched)`。配成 `review` 不会生效，管理员会以为冲突项进了人工队列。

相关位置：`crates/kanalib-ingest/src/rules.rs:241`、`crates/kanalib-ingest/src/rules.rs:458`、`deploy/rules.toml.example:113`。

### 2.3 [P2] `size.on_fail` 被静默忽略

同类问题：校验允许 `size.on_fail = "review|block"`，但实现只返回 `RuleOutcome::review("size below minimum", matched)`。配成 `block` 不会生效。

相关位置：`crates/kanalib-ingest/src/rules.rs:244`、`crates/kanalib-ingest/src/rules.rs:470`。

### 2.4 [P2] 单条条目异常会中止整轮决策

`pipeline::run_decisions()` 中 `record_model_tags(...)?` 和补标后复判的 `rules::evaluate(...)?` 会把错误直接抛给主循环。主循环只记一条 `decision pass failed; keeping items pending`，本轮后续所有 pending 条目都被跳过。

设计要求单条模型/规则异常进 `review`（数据库写失败可保留 pending 重试），不应让一个条目阻断整批。建议这两处把错误转换成 `review_public(...)` 并继续下一条；真正的 DB 写失败按规范不决策重试。

相关位置：`crates/kanalib-ingest/src/pipeline.rs:94`、`crates/kanalib-ingest/src/pipeline.rs:120`、`docs/backlog/ingest-service.md:424`、`docs/backlog/ingest-service.md:466`。

## 3. 仓库卫生

工作树里有若干看起来与本任务无关的未跟踪文件，提交前逐个确认：

- `.kanalib-hook-log-check.mjs`、`package.json`、`package-lock.json`；
- `key-acl.txt`（含 `kanalib_deploy` ACL，注意敏感信息）；
- `probe-rail-inspector.json`、`probe-rail-state.json`、`web/box-preview.html`（约 2.7 MB）；
- 名为 `SELECT COUNT() FROM t;` 的空文件。

## 4. 修复清单

按当前状态重排，已完成项标注日期与落点。

| # | 项 | 状态 | 落点 |
|---|---|---|---|
| 1 | `rules_file` 缺省策略 | **已完成**（2026-11-23） | `RulesState::load()` 未配置即 `bail!`；内置默认规则改走 `RulesState::load_without_file()`，只在 `main.rs` 判定来源为 `mock` 且未配置规则文件时调用。真实来源（`pixiv`）没有这条路。§7.6 补了「缺规则文件 ≠ 规则为空」：空表仍只告警（I-Q10 不变），缺文件拒绝启动。测试 `missing_rules_file_refuses_to_start`、`mock_only_default_still_records_snapshot` |
| 2 | `dedup.on_conflict` / `size.on_fail` 真正分流 | **已完成**（2026-11-22） | 见 2.2 / 2.3 状态更新，`rules.rs::policy_outcome()` |
| 3 | `pipeline::run_decisions()` 单条异常隔离 | **已完成**（2026-11-22） | 见 2.4 状态更新；测试 `one_item_error_is_reviewed_without_blocking_the_rest` |
| 4 | 清理无关未跟踪文件 | **已完成**（2026-11-23） | I2 提交 `253fd09` 只纳入规则层相关文件；`key-acl.txt`、`probe-rail-*.json`、`web/box-preview.html`、`package*.json`、`.kanalib-hook-log-check.mjs`、`SELECT COUNT( ) FROM t;` 均未入库 |
| 5 | 工作树稳定后重跑测试与静态检查 | **已完成**（2026-11-23） | 77 passed（43 单元 + 8 admin + 6 decision + 8 ingest + 12 push）；`clippy -D warnings` 与 `fmt --check` 通过 |
