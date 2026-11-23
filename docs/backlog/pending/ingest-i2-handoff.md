# F-089 / I2 交接文档（2026-11-23，更新）

> 上级文档：[ingest-service.md](../ingest-service.md) · 登记：[registry.md](../registry.md#f-089) · 分支：`main` @ `086cfcb`（I2 已提交；领先 `origin/main` 4 个提交，未推送）
>
> 本文档交接 **I2 当前实现进度**，不是最终设计文档。设计仍以 [../ingest-service.md](../ingest-service.md) 为准；有冲突时先查设计，再改文档。

## 1. 当前状态

`crates/kanalib-ingest` 的 I2 主体已经闭环：下载完成后按 `rules.toml` 做黑名单/R-18G/标签预算/观察词/尺寸/去重判定；标签不足时调用 HTTP 打标后端补标一次再复判；结果写入 `items.decision` 与 `decisions` 审计表。规则加载写入只读 `rule_snapshots`，支持 mtime 变更检测与 Unix `SIGHUP` 重载；管理页提供审核、规则草稿、逐行 diff 预览、重载与指标。

本轮补完了上一份交接里的 1–8 项（第 9 项 I5 仍需 box 实机）。2026-11-23 另完成三件事：I2 改动入库、评审 P1（`rules_file` 缺省策略）改为 fail-closed、告警补上打标后端连续失败（见第 6 节）。对应的未提交改动评审稿已闭环并归档到 [`../../archive/2026-11/ingest-i2-uncommitted-review-2026-11-22.md`](../../archive/2026-11/ingest-i2-uncommitted-review-2026-11-22.md)。

| 范围 | 进度 | 结论 |
|---|---:|---|
| I2：打标后端 + 规则加载/校验 + 决策 + 补标 + review + 快照/热重载 | 100% | 主循环、规则状态、管理页、指标、规则编辑均已接通；I2 测试全绿 |
| F-089 整体 | ~75% | I0/I1/I2/I3 主体完成；I4 管理页、规则层与告警代码完成；**I5 box 部署 + 网页联调未做** |

## 2. 本轮完成

1. **规则开关真正分流**：`dedup.on_conflict` 支持 `block|review`，`size.on_fail` 支持 `review|block`；不再只校验不生效。位置：`rules.rs` 的 `policy_outcome()`。
2. **watch 原因可追溯**：命中观察词时 `reason` 为 `watch:<词>`（多词用 `, ` 连接），`matched` 仍保留全部 `watch:<词>`；指标按 `watch:` 前缀归入 `watch` 桶。
3. **I-Q10 空词表告警**：`rules.blacklist.tags` 为空时，启动/重载写 warning；`/admin/api/status` 返回 `rules.tag_blacklist_empty`，管理页显示红色提示「标签黑名单为空：不会按标签拦截」。
4. **指标漂移标签**：`rules_version_info{version,config_hash}` 同时带版本与 hash，便于做漂移告警。
5. **草稿逐行 diff**：`rules_edit::line_diff()` 使用无依赖 LCS；`RulesEditor::preview()` 与 `apply()` 共用同一份编辑计划；`/admin/api/proposals/{id}/preview` 返回 `diff`/`already_satisfied`，管理页用面板渲染增删行，确认后才写文件。
6. **推送版本默认值**：`push::enqueue_decisions()` 默认使用当前生效的 `rules.version`；`config.push.rules_version` 仅作为回放历史批次的显式覆盖。
7. **单条异常隔离**：`pipeline` 对每个条目独立执行读取/判定/补标/落库；任一条异常转 `review`（reason `decision error`）后继续下一条，不阻断整轮；DB 写失败保留 pending 并记日志。
8. **测试补齐**：规则策略、watch 原因、空词表、diff/预览、管理页状态与指标、单条异常隔离均有覆盖。

## 3. 代码地图

| 文件 | 作用 | 状态 |
|---|---|---|
| `crates/kanalib-ingest/src/rules.rs` | `RuleConfig` 反序列化、校验、NFKC 归一化、十步决策、R-18G 三路、标签预算、dedup/size 策略 | 完成 |
| `crates/kanalib-ingest/src/tagging.rs` | `TaggingBackend` trait 与 HTTP multipart 打标，超时 + 最多 3 次尝试 | 完成 |
| `crates/kanalib-ingest/src/review.rs` | review 审核边界；仅 `review` 可放行/拒绝；R-18G 禁止人工放行 | 完成 |
| `crates/kanalib-ingest/src/rules_state.rs` | 启动加载 + 快照落库 + mtime 检测 + `SIGHUP` 强制重载 + 空词表告警；失败保留旧快照 | 完成 |
| `crates/kanalib-ingest/src/rules_edit.rs` | proposal → `toml_edit` 原子改写、版本自增、漂移检查、预演/逐行 diff | 完成 |
| `crates/kanalib-ingest/src/admin.rs` | 审核页、规则草稿/预览/应用、状态、指标、原图/缩略图、重跑 | 完成 |
| `crates/kanalib-ingest/src/metrics.rs` | Prometheus 文本指标；决策/原因/预算/模型错误/推送失败/来源错误 | 完成 |
| `crates/kanalib-ingest/src/pipeline.rs` | 逐条决策主循环；补标一次后复判；单条异常转 review | 完成 |
| `crates/kanalib-ingest/src/db.rs` | ingest SQLite schema、下载/标签/决策/快照/草稿辅助查询 | schema v7 |
| `crates/kanalib-ingest/src/main.rs` | 配置/规则/打标/下载/决策/推送装配；推送版本取当前生效规则 | 完成 |
| `crates/kanalib-ingest/tests/decision_flow.rs` | 端到端决策、补标、热重载、单条异常隔离 | 完成 |
| `crates/kanalib-ingest/tests/admin_flow.rs` | 审核、草稿应用、预览 diff、状态、指标 | 完成 |

## 4. 已实现语义

### 4.1 规则加载与校验

- 规则源是 `service.rules_file` 指向的 `rules.toml`；未配置时使用内置默认规则并告警。
- `RuleConfig` 使用 `serde(default, deny_unknown_fields)`，未知键拒绝加载。
- 标签归一化：NFKC → 折叠空白 → lowercase；不折叠下划线/连字符，不做同义词映射。
- 用户、tags、watch、rating allow/block、tag budget ignore 归一化后自动去重；重复项记录 warning。
- 硬校验：`rules.version` 非空；`rating.block` 只允许 `r18g`；同词不得同时在 `tags` 与 `watch`；rating 不得同时 block/allow；`dedup.on_conflict` 只允许 `block|review`；`size.on_fail` 只允许 `review|block`。
- `blacklist.tags` 为空不拒绝启动，但启动、重载与管理页都会显式告警。

### 4.2 决策顺序

`rules::evaluate()` 当前顺序：

1. 来源禁用 → `block`
2. 作者黑名单 → `block`
3. R-18G：`rating == R18G`、`x_restrict == 2`、标签 `r18g/r-18g`、`restriction_attributes` 含 `r18g` 任一命中 → `block`；三路字段缺失且 `sanity_level >= threshold` → `review`
4. 标签黑名单（匹配 `name` 与 `translated_name`）→ `block`
5. 内容标签数 `< min_tags` → `needs_tagging=true`，主循环补标一次后复判
6. 重复 SHA-1 → 按 `dedup.on_conflict` 返回 `block` 或 `review`（只与更早的 `(created_at, id)` 条目判重）
7. 观察词命中 → `review`，`reason` 为 `watch:<词>`
8. 宽/高低于阈值 → 按 `size.on_fail` 返回 `review` 或 `block`
9. bytes 为 0 → 按 `size.on_fail` 返回 `review` 或 `block`
10. 全部通过 → `allow`

R-18、AI 生成显式放行；当前不因 AI 标记拦截。

### 4.3 补标流程

- 阈值来自 `rules.tag_budget.min_tags`，默认 5。
- 只统计内容标签；忽略 `オリジナル`、`漫画`、`イラスト`、`original`、`manga`、`illustration` 与 `users入り` 后缀。
- 补标调用 `tagging.backend = "http"` + `tagging.endpoint`；超时来自 `tagging.timeout_secs`，固定重试 2 次。
- 补出的标签写入 `item_tags(origin='model')`，合并后重新决策。
- 无后端、打标失败、返回空标签 → `review`，不允许退回“只看来源标签”后放行。

### 4.4 数据落库

- `db::SCHEMA_VERSION = 7`：v5 删除旧可写 `rules` 表并给 `decisions` 加 `tag_source`/`tag_count_used`；v6 新增 `rule_snapshots`；v7 新增 `rule_proposals`。
- `works` 有 `rating`、`ai_type`、`x_restrict`、`sanity_level`、`restriction_attributes`、`tag_count`。
- `item_tags.origin` 区分 `pixiv` / `model`。
- 每条决策更新 `items.decision/decision_reason/rules_version/revision`，并插入 `decisions` 审计行（matched rules、tag source、tag count）。
- 单条异常转 review 时写 `reason='decision error'`；若该行也写不进去，条目保持 pending，下一轮重试。

### 4.5 规则快照与热重载

- 启动加载后立即写 `rule_snapshots`；解析/校验失败拒绝启动。
- 每轮主循环 `reload_if_changed()`：mtime/长度变化才重读；Unix `SIGHUP` 强制重读。
- 同版本同 hash → `Unchanged`；同版本不同 hash → 默认拒绝（`config_hash_check = true`）；版本回退 → 拒绝；版本升级 → 校验后原子换内存快照并写快照。
- 版本比较按点分/连字符分段，数字段按数值比较（`2026-11-22.10 > 2026-11-22.9`）。
- 重载失败保留旧快照，并记录 `last_error` 供管理页显示。

### 4.6 管理页与规则编辑

- 只应绑回环或 EasyTier 虚拟 IP；默认 `127.0.0.1:8791`。
- 审核列表显示缩略图、来源、作者、标签、命中原因、分数、时间；R-18G 条目标「终态拒绝」且禁用放行。
- 放行/拒绝/重跑都写 `decisions` 与操作者；R-18G 不能人工放行。
- 规则草稿支持拉黑/移除标签与作者；草稿先落 `rule_proposals`，预览返回逐行 diff 与版本变更，确认后 `toml_edit` 原子改写 `rules.toml` 并自动升版本，再触发重载。
- 管理页不直接写文件；手工编辑文件仍是合法路径，但要自己升版本并 `SIGHUP`/重载。

### 4.7 指标

- `decision_total{decision}`、`decision_reason_total{kind}`（含 `watch`、`dedup`、`size`、`decision_error` 等）。
- `tag_budget_hit_total{source}`、`tag_budget_short_total{source}`、`model_errors_total`。
- `queue_depth{state}`、`review_queue_depth`、`review_age_seconds`、`push_pending`。
- `rules_version_info{version,config_hash}`：用于版本漂移告警。

## 5. 测试与验证

最近一次本机通过（2026-11-23，`086cfcb`）：

```powershell
$cargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
& $cargo test -p kanalib-ingest
& $cargo check -p kanalib-ingest --all-targets
& $cargo clippy -p kanalib-ingest --all-targets -- -D warnings
& $cargo fmt -p kanalib-ingest -- --check
```

结果：

- `kanalib-ingest` 单元测试：44 passed。
- `tests/admin_flow.rs`：8 passed。
- `tests/decision_flow.rs`：6 passed。
- `tests/ingest_flow.rs`：8 passed。
- `tests/push_flow.rs`：12 passed。
- 合计：78 passed。
- `cargo check` / `clippy -D warnings` / `fmt --check`：通过。

覆盖重点：

- allow / 标签黑名单 block / watch review（reason `watch:<词>`）/ 作者黑名单 block。
- 标签不足 → 补标一次 → 黑名单 block 或干净 allow；失败/空标签/无后端 → review。
- R-18G 四路命中均 block；同哈希最早一条 allow、后一条按 `dedup.on_conflict` 处理。
- `dedup.on_conflict=review`、`size.on_fail=block` 均生效。
- 单条模型补标落库异常 → 该条 review，后一条仍正常 allow。
- 规则热重载：未升版本改内容被拒绝；升版本后新决策写新 `rules_version`，两版快照留档。
- 管理页：状态含 `tag_blacklist_empty`；指标含 `config_hash`；草稿预览返回 diff 且不写文件；应用后文件/版本/内存规则一致。
- 下载续传、多页同步、429/auth、推送签名与幂等等既有链路未回归。

## 6. 未完成 / 下一步

1. **I5：box 部署 + 网页联调（需实机，代码侧不算完成）**
   - 按 `deploy/` 部署二进制、SQLite、媒体目录与 `rules.toml`。
   - 验证管理口只绑 EasyTier 虚拟 IP、公网不可达。
   - 审核放行/拒绝各一条，网页批次结果符合预期。
   - 断网重试、box 重启恢复、桌面拉原图走直连。
2. **真实 Pixiv 令牌冒烟**：只在 box 手动执行，不写入仓库/CI。
3. **告警接线**：`deploy/monitor/kanalib-health.sh` 已覆盖磁盘、推送积压、推送失败/被拒、review 积压、HTTP 健康、TLS 与**打标后端连续失败**（`model_error_streak` gauge，2026-11-23 补，提交 `086cfcb`）。剩下的是按部署环境调阈值与 webhook，属于 I5 现场工作。
4. **草稿类型**：当前只支持 tags/users 的增删；`watch`、rating、tag_budget 等仍走手工编辑文件。

## 7. 重要实现细节与坑

- `normalize_unique()` 返回 `Result<Vec<String>>`；调用处必须保留 `?`。
- `HttpTaggingBackend` 在 `spawn_blocking` 中创建 blocking reqwest client；不要把 `reqwest::blocking::Form` 放进 async reqwest body。
- `HttpTaggingBackend` 不能 `Clone`；共享时用 `Arc<HttpTaggingBackend>`。
- 端到端测试的本地 tagger 必须读完请求体（handler 收 `Bytes`），否则 hyper 提前关连接，`reqwest::blocking` 报 `send failed because receiver is gone`。
- 去重只看更早的 `(created_at, id)` 条目；不要退回 `id != self`，否则同批 pending 会互相判重。
- `Decision` 实现了 `Copy`，`as_str(self)` 按值接收。
- 热重载只在 mtime/长度变化时触发；编辑后时间戳没变时用管理页「重载」或 `SIGHUP`。
- `rules.config_hash_check = false` 是显式逃生口：允许同版本覆盖快照，但会告警；正常流程必须升 `rules.version`。
- 预览与真正应用共用 `plan_edit()`；不要在预览里另写一套编辑逻辑，否则 diff 会和落盘结果不一致。
- 规则文件是多行数组时 diff 才有意义；单行超长数组会表现为整行替换。
- 工作区还有若干与本任务无关的未跟踪文件（如 `probe-rail-*.json`、`web/box-preview.html`、`key-acl.txt`、`package*.json`）；提交前逐个确认，不要一并打包。

## 8. 当前工作区

- 分支：`main`
- 当前 commit：`086cfcb`（`feat(monitor): 告警补上打标后端连续失败`）
- 状态：**I2 相关改动已提交，领先 `origin/main` 4 个提交，未推送**；`781d447` 为词表移出文档那次提交
- 工作区无已跟踪改动；仍有与本任务无关的未跟踪文件（`key-acl.txt`、`probe-rail-*.json`、`web/box-preview.html`、`web/dist-api/`、`package*.json`、`.kanalib-hook-log-check.mjs`、`SELECT COUNT( ) FROM t;`）。`key-acl.txt` 含部署 ACL，入库前要单独定夺
- 约束：继续遵循“不推送，除非用户明确要求”。

## 9. 快速接手命令

```powershell
# 测试 + 静态检查
$cargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
& $cargo test -p kanalib-ingest
& $cargo clippy -p kanalib-ingest --all-targets -- -D warnings
& $cargo fmt -p kanalib-ingest -- --check

# 只跑决策端到端
& $cargo test -p kanalib-ingest --test decision_flow

# 最小 mock 启动（不接真实 Pixiv）
& $cargo run -p kanalib-ingest -- --source mock --once --db "$env:TEMP\kanalib-ingest-handoff.db" --media-dir "$env:TEMP\kanalib-ingest-handoff-media"
```

接手后先做 I5 box 部署与网页联调；代码侧剩余项见第 6 节。
