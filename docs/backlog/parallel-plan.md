# 并行开发与第一轮任务包

> 上级索引：[../FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md) · 文档地图：[README](./README.md)

> 2026-10-27 状态：第一轮任务 0/A/B/C/E/D 全部完成并合入；第二轮任务 0/A/B/C/D1/D2 全部完成并合入，见 [round-2.md](round-2.md)。第三轮 G2b2 已完成，剩余项见 [round-3.md](round-3.md)；第四轮任务包见 [round-4.md](round-4.md)。
>
> 冲突约束延续：`db.rs` schema 迁移、`sidecar` 协议各只有一个负责人；UI 只有一个写入车道（第三轮任务 0 的模块拆分合入后，`app.rs`/命令面板/底栏仍是单点写入）。

## 第一轮执行结果（2026-10-27）

| 任务 | 状态 | 提交 |
|---|---|---|
| 任务 0 基础接口/依赖 | 已合入 | `5fb45f4` |
| A 备份核心（F-061） | 已合入 | `5672b97` |
| B 日志/诊断核心（F-062） | 已合入 | `cf6943c` |
| C 扫描/来源/监听（F-030/31/32） | 已合入 | `fe18b60`、`f11eec6`、`6b1b88b` |
| E 便携包校验（F-065 部分） | 已合入 | `dbd6f35` |
| D UI 集成 | 已合入 | `5226eee` |

- **验证**：`cargo test --workspace` core 61 + UI 8 通过（4 项按需忽略）；`cargo check --workspace` 通过；便携包 `-SkipBuild` 完整打包 12,700 文件/3.93GB/36s，清单 SHA256 可复现（zip 2.8GB/5m24s）；UI 隔离冒烟 `.kanalib/round1_settings.png`/`round1_sources.png`，UIA 实测备份 160KB 且完整性通过。
- **未完成边界**：F-030 目录重命名/跨源移动、启动离线后自恢复、真实 notify 集成测试；F-032 规则编辑器与已入库排除项处理；F-061 自动定时与恢复流程；F-065 MSI/签名/自动更新。F-031、F-062 已归档；F-030/032/061 保留在 pending。
- **禁止回退**：第一轮未改 DB schema（v7）、未改 sidecar 协议、UI 只有一条车道；后续任务仍按此约束。

## 并行开发矩阵（2026-10-27）

> 结论：真正低冲突的并行单元是「新模块 + 明确接口」。当前 UI 全部在 `crates/kanalib-ui/src/lib.rs`，核心扫描/打标/缩略图集中在 `crates/kanalib-core/src/state.rs`，schema 在 `db.rs`；没有先解耦时，不要同时派两个 UI 或两个 state/schema 任务。

| 工作流 | 组内顺序（不可再拆并行） | 主要落点 | 与其他工作的冲突 |
|---|---|---|---|
| 扫描/来源 | F-030 → F-031 → F-032；F-035 在 Source 接口冻结后 | `state.rs`、`model.rs`、`db.rs`、新增 watch/ignore 模块、来源 UI | 与打标/媒体共同改 `state.rs`；与 F-013/21/35 共同改 schema；与 UI 侧栏冲突 |
| 打标流水线 | F-021 → F-024 → F-023 | `sidecar/tagger/server.py`、`repair.py`、`state.rs` 队列、`db.rs` schema/jobs、队列 UI | 与扫描/媒体共同改 `state.rs`；schema 迁移；sidecar 协议 |
| 媒体管线 | F-033 → F-040 阶段 2 → F-064 | `media.rs`、`ffmpeg.rs`、`state.rs` 缩略图、Cargo features | 与扫描（缩略图）、打标（修复）共享 state/sidecar；F-033 与 F-040 同媒体层 |
| 浏览增强 | F-045 → F-004 → F-003 → F-005 | `dedup.rs`、pHash、`lib.rs` 的 viewer/dedup/grid | 组内共享 pHash/去重/查看器；与任何 UI 工作冲突 |
| 搜索/组织 | F-013 → F-014 → F-012 | `db.rs`（saved_searches/collections）、`lib.rs` 搜索/侧栏/设置 | schema 迁移；UI 单体；F-012 与 F-013/14 都改搜索/列表 |
| 运维基础设施 | F-061 → F-062 → F-063 | 新增 backup/logging 模块、`main.rs`、`state.rs` 进度、设置/状态栏 UI | F-061/62 较独立；F-063 依赖扫描进度接口；都需 UI 集成 |
| 小交互 | F-036 → F-037；候选 F-046/47/67/68 | `lib.rs`、`trash.rs`、`state.rs` | 只能占用同一条 UI 车道；不要与其他 UI 任务并行 |

**可以安全并行（低冲突）**

- F-061 备份 + F-062 日志：新增 `backup.rs`/日志模块，核心文件不同；设置/诊断 UI 交给同一个 UI 集成人。
- F-065 打包脚本 + F-064 ICC + F-016 CLIP 原型：新脚本/新模块/独立 sidecar 模型，不改现有 state/schema；先做接口与许可验证。
- F-030/31/32（扫描）与 F-033/40阶段2（媒体）在“state.rs 集成点先约定、由同一核心集成人合并”的前提下可并行；否则会冲突。
- F-061/F-062/F-065 可与上述两条核心线并行；不要让它们各自再改 `lib.rs`。

**不要并行**

- 任意两个 UI 重任务：F-012、F-013/14、F-003/04/05/45、F-036/37、F-063 的状态栏/诊断 UI；当前只有一个 `lib.rs`。
- F-021/23/24 与 F-030/31/32：同时改 `state.rs` 扫描/打标路径与 `db.rs` 迁移。
- F-013/14 与 F-021/030/031/035：都要动 schema 或侧栏数据模型，必须由单一迁移负责人串行。
- F-003/004/005/045：共享 pHash、去重组与查看器/比较 UI。
- F-033 与 F-040 阶段 2：同一媒体/ffmpeg 层，先定接口再拆。
- F-035 与 F-030/31：Source 模型未冻结前不要同时开工。

**并行前置解耦**

1. 把 `crates/kanalib-ui/src/lib.rs` 拆成 library/viewer/dedup/sidebar/settings/status 模块，或指定唯一 UI 集成人；否则 UI 并行没有意义。
2. 把 `state.rs` 的扫描、打标队列、缩略图、搜索拆成可调用模块，state 只做编排；否则核心任务互相踩。
3. 指定一个 schema 迁移负责人；F-013/14、F-021、F-030/31、F-035、F-061 的迁移按序合并。
4. 冻结 `Source`/`ScanProgress`/`TagJob`/`MediaPipeline` 四个接口；F-035/F-063/F-033/F-040 都依赖它们。
5. sidecar 协议（阈值、候选、修复）由同一负责人维护；F-021/23/24 与 F-040 阶段 2 不要各自改协议。

**建议轮次**

- 第 1 轮（低冲突）：F-061、F-062、F-065 并行；UI 集成人只做它们的设置/诊断入口。另开一条核心线做 F-030/31/32（或 F-033/40 阶段 2，二选一），不要两条核心线同时改 `state.rs`。
- 第 2 轮：完成扫描接口后做 F-035；同时可让媒体线做 F-033/40 阶段 2，UI 集成人做 F-012 或 F-013/14 中的一条。
- 第 3 轮：F-021/23/24 打标线；UI 线做 F-003/04/05/45；之后 F-036/37 与候选小项。
- 远期：F-016/F-026/F-064/F-066 在独立模块/许可评估通过后并行，不占用当前 UI/state 热点。

## 第一轮并行任务包（可直接派发）

> 目标：先不动 schema、不拆 UI 单体，跑通「3 条低冲突核心线 + 1 条唯一 UI 集成线」。任务 0 必须先合；任务 D 只由同一个 UI 负责人改 `lib.rs`。

### 任务 0：基础接口与依赖（1 人，先合）

- **文件**：`crates/kanalib-core/Cargo.toml`、根 `Cargo.toml`、新增 `watch.rs`/`scan_filter.rs`/`backup.rs` 模块骨架、`model.rs` 的接口定义。
- **内容**：
  - 加依赖：`notify` 8、`notify-debouncer-mini` 0.7、`ignore` 0.4、`rusqlite` 的 `backup` feature、`flexi_logger` 0.31。
  - 定义 `SourceScanStatus`（root、last_scan、counts、error、online）、`ScanProgress`（phase、done/total、current、source、throughput、eta）、`BackupReport`、`DiagnosticSnapshot`、`ScanFilterConfig`。
  - 建模块骨架并导出；**不改 DB schema、不改 UI**。
- **验收**：`cargo check --workspace` 通过；接口有字段注释；后续任务不再改 Cargo 依赖（除紧急修复）。
- **边界**：只做依赖/接口/骨架；不实现扫描、备份、日志逻辑。

### 任务 A：F-061 数据库备份核心（核心线）

- **文件**：`crates/kanalib-core/src/backup.rs`、`db.rs` 增加一个 `backup_to` 方法、`backup.rs` 测试；不碰 `lib.rs`。
- **内容**：用 `rusqlite` 的 `backup` API；备份前 WAL checkpoint；输出 `BackupReport`；打开备份校验 `PRAGMA integrity_check`/关键表计数；扫描中拒绝或排队。
- **不做**：设置页 UI、自动定时、恢复流程（交给 UI 线/下一轮）。
- **验收**：临时 DB 备份后可在另一路径打开且计数一致；失败不破坏源库；单测覆盖 WAL 与失败路径。
- **冲突**：只改 `db.rs` 一个小方法；若任务 C 先改 `db.rs`，本任务 rebase。

### 任务 B：F-062 日志与诊断核心（核心线）

- **文件**：新增 `src/logging.rs`、改 `src/main.rs` 的初始化；新增 `crates/kanalib-core/src/diagnostics.rs`；不碰 `lib.rs`。
- **内容**：正常 GUI 启动即写数据目录滚动日志（大小/保留天数可配）；保留 `KANALIB_LOG_FILE` 覆盖；实现 `DiagnosticSnapshot`（版本、数据目录、DB/缓存大小、计数、队列、设备、最近扫描/错误、日志路径）；凭据/路径按需脱敏。
- **不做**：诊断页 UI（交给任务 D）。
- **验收**：正常启动生成日志；滚动/保留生效；快照函数单测；无敏感信息；`cargo test --workspace` 通过。
- **冲突**：仅 `main.rs` 与新增模块；不要改 `state.rs`/DB。

### 任务 C：F-030/31/32 扫描与来源核心（核心线）

- **文件**：`state.rs`、`model.rs`、新增 `watch.rs`/`scan_filter.rs`；`settings` JSON；**第一轮不改 DB schema**。不碰 `lib.rs`。
- **顺序**：
  1. **F-032 排除**：用 `ignore::WalkBuilder` 按 root 应用排除 glob/隐藏/大小过滤；配置存 `settings`，提供预览命中数 API。
  2. **F-031 单来源**：`rescan_root(root)`、运行时 `SourceScanStatus` 映射（上次扫描/计数/错误/在线）；不改 `sources` 表；离线来源不参与清理。
  3. **F-030 监听**：`notify` + `notify-debouncer-mini` 按 root 监听；防抖/批量、新增/修改/删除/重命名走增量索引；离线暂停、失败回退手动重扫；事件风暴限流。
- **不做**：来源侧栏 UI、排除规则 UI（任务 D）；F-035 网络源；schema 迁移。
- **验收**：临时目录新增/删除/重命名/排除规则测试；离线不误删；10k 扫描时间与内存不明显劣化；无 UI 编辑。
- **冲突**：第一轮独占 `state.rs`/`model.rs`；任务 A 的 `db.rs` 方法后合；任务 B 不改 state。

### 任务 D：UI 集成（唯一 UI 车道）

- **文件**：只改 `crates/kanalib-ui/src/lib.rs`（必要时设置页局部）。
- **内容**（按核心线就绪顺序）：
  - F-061：设置页「立即备份/打开备份目录」+ 报告/错误提示；恢复入口可留按钮但提示后续。
  - F-062：设置/诊断页展示 `DiagnosticSnapshot`、日志路径、复制诊断信息。
  - F-031：来源行显示上次扫描/计数/错误/离线；“重新扫描此来源”加入右键菜单。
  - F-032（可选）：来源级排除规则编辑；若排期紧，先只读展示，编辑放第二轮。
- **不做**：F-012/F-013/14/F-003/04/05/F-021/23/24 等其它 UI；不写核心逻辑；不改 DB。
- **验收**：UI 冒烟截图；隔离数据根；UIA 可识别新按钮/状态；无回归；`cargo check --workspace`。
- **冲突**：全项目唯一 UI 车道；核心线必须先把 API 合入。

### 可选任务 E：F-065 打包脚本（独立线，不占核心）

- **文件**：仅 `scripts/build_portable.ps1`、`scripts/portable_readme.txt`，必要时新增校验脚本。
- **内容**：便携包构建校验、许可证/SHA256 清单、`runtime/ffmpeg` 与模型路径检查；**不做 MSI/签名/自动更新**（后续单独任务）。
- **执行环境**：本机有 PowerShell 7.6.7，所有 `.ps1` 用 `pwsh -NoLogo -NoProfile` 执行/验证；只有没有 `pwsh` 时才回退 5.1，并注意 LF/here-string 坑。
- **验收**：`-SkipBuild` 打包可复现；清单包含 ffmpeg/runtime/models；无 Rust 代码改动。

### 合并顺序与禁止事项

1. 任务 0 先合；任务 C 先合共享的 `state.rs`/`model.rs` 变更，A/B rebase 后合并；D 在 A/B/C API 稳定后合。
2. 第一轮禁止：改 DB schema、改 sidecar 协议、让多个代理同时改 `lib.rs`、启动 F-012/F-021/F-003 等热点任务。
3. 每个任务交付时附：改动文件、接口签名、测试命令/结果、未完成边界；UI 任务附截图路径。
4. 如任务 C 发现必须迁移 `sources` 表，暂停并与任务 A/后续 F-035 协调，不在本轮临时加迁移。
