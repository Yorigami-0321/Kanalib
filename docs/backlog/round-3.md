# 第三轮任务包（2026-10-27）

> 上级索引：[../FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md) · 文档地图：[README](./README.md) · 第一轮：[parallel-plan.md](parallel-plan.md) · 第二轮：[round-2.md](round-2.md)

> 本轮目标：补齐整理与浏览工作流（智能集合/相册、以图找相似、相似阈值预览、分组浏览），补齐扫描可观测性与小交互（进度/ETA、拖拽导入、从图库移除）。备份恢复、媒体评估与搜索性能三条留作顺延；网络来源实现仍不进入本轮。

## 第二轮落地复核（2026-10-27，以当前代码与实测为准）

| 任务 | 提交 | 代码证据 | 结论 |
|---|---|---|---|
| A 打标质量与队列（F-021/023/024，schema v8） | `8eb97b2` | `crates/kanalib-core/src/db.rs:23` `SCHEMA_VERSION = 8`；`crates/kanalib-core/src/state.rs:1183` `tag_thresholds`、`crates/kanalib-core/src/state.rs:1264` `retag_paths`、`crates/kanalib-core/src/state.rs:1297` `tag_job_errors`、`crates/kanalib-core/src/state.rs:1371` `tag_queue_status` | 已落地，无遗留收尾项 |
| C 媒体管线（F-033 + F-040 阶段 2） | `74d65be` | `IMAGE_EXTENSIONS` 含 tif/tiff/ico/qoi/exr；AVIF 走 `FFMPEG_ONLY_EXTENSIONS`；`crates/kanalib-core/src/media.rs`、`crates/kanalib-core/src/ffmpeg.rs` | 已落地；HEIC/RAW/PSD 与动画播放入口仍在 pending |
| B 搜索性能核心（F-012） | `f5f8ed4` | `crates/kanalib-core/src/db.rs:2320` `search_page`、`crates/kanalib-core/src/db.rs:3844` `ensure_search_indexes`、`SearchSql.force_index` + `INDEXED BY` | 已落地；非名称排序 p95 276–304ms 为已知上限（2026-10-27 起不再当硬门禁，见下方范围决定 3） |
| D2a/D2b 第二轮 UI 集成 | `38d4d1c`、`7f75bbc` | `crates/kanalib-ui/src/lib.rs:1126` `search_debounce_task`、`crates/kanalib-ui/src/lib.rs:2200` 仅 `dup:true` 构造去重集、`crates/kanalib-ui/src/lib.rs:2321` `pending_tag_reviews`、`crates/kanalib-ui/src/lib.rs:12993` `format_capability_note` | 已落地；源码内无 `TODO`/`FIXME`/「第三轮」遗留标记，D2 计划项全部覆盖，无并入第三轮的收尾项 |

- **测试基线**：本轮复核重跑 `cargo test --workspace`，core 82 通过（7 项按需忽略）+ UI 15 通过，退出码 0；与 [round-2.md](round-2.md) 记录的交付数字一致。
- **文档一致性**：新增校验脚本 `scripts/check_backlog_docs.sh`。当前结果：registry 50 条、无重复 ID、无「文档引用了但 registry 没有」的条目、pending 详情文件覆盖 overview 全部 ID；状态唯一偏差是 F-040（registry 标「实现中」而 implemented.md 归档阶段 1），属预期。
- **接口现状（决定本轮拆分的依据）**：`collections`/`collection_assets` 建表在 `crates/kanalib-core/src/db.rs:381` 起，但全项目零读写；`saved_searches` 只有 `load/save/delete`（`crates/kanalib-core/src/db.rs:1816` 起、`crates/kanalib-core/src/state.rs:1093` 起），没有重命名/更新/排序/置顶。`ScanProgress`/`ScanPhase` 已在 `crates/kanalib-core/src/model.rs:1576` 定义，但 `state.rs` 扫描路径仍只写字符串 `shared.status`，结构化进度从未接入。`dedup.rs` 只有全库分组（`crates/kanalib-core/src/dedup.rs:390` `compute_similar_groups`）与距离函数（`crates/kanalib-core/src/dedup.rs:125`），没有单锚点查询。
- **拖放可行性**：`gpui-pre =0.3.9` 已提供 `FileDropEvent`（Entered/Pending/Submit/Exited/Ended）与 `on_drop`/`ExternalPaths`，`kanalib-ui` 目前没有任何 drop 处理；F-036 不需要新依赖，但需实测 Windows 侧拖入的路径载荷与自绘标题栏的兼容。
- **依赖现状**：`reqwest`/`reqwest_dav`/`keyring` 仍未引入（`crates/kanalib-core/Cargo.toml`），与「本地优先」的决定一致，本轮仍不引入。

## 任务 0：UI 单体拆分（工程前置，独占 `lib.rs`）

- **文件**：`crates/kanalib-ui/src/lib.rs`（当前 13,779 行）拆为 `app.rs`、`views/`（网格/列表/详情/内容）、`sidebar.rs`、`inspector.rs`、`viewer.rs`、`dedup.rs`、`search.rs`、`settings.rs`、`status_bar.rs`、`theme.rs`；模块命名可按实际调整。
- **内容**：只移动代码与调整可见性，不改行为、不改文案、不加功能；`AppState` 字段、命令面板注册、底栏接线集中保留在 `app.rs` 作为单点写入位置；测试随代码迁到对应模块，数量不减。
- **验收**：`cargo test --workspace` 与拆分前逐项一致（core 82 + UI 15）；同一隔离数据根下拆分前后截图逐张比对无差异；UIA 节点树的名称与顺序不变；`cargo check --workspace` 无警告。
- **边界**：不实现任何第三轮功能；不动 core/schema/sidecar。
- **冲突**：本任务合入前，其它任务一律不得改 `lib.rs`。合入后，Wave 2 的三条 UI 线可按模块并行，但 `app.rs`/命令面板/底栏仍由唯一 UI 集成人写入。
- **决定**（2026-10-27 确认）：执行拆分；接受这一条不产出用户可见功能，换取 Wave 2 的并行度。

## Wave 1（本轮只开 A/B/C 三条核心线；均不碰 `lib.rs`）

### 任务 A：F-013/F-014 组织核心（本轮唯一 schema owner，v8 → v9）

- **文件**：`db.rs`（`saved_searches` 增列、`collections`/`collection_assets` 读写）、`model.rs`（`SavedSearch`/`Collection` 结构）、`state.rs` 的组织接口、测试；不碰 `lib.rs`。
- **内容**：
  - F-013：`saved_searches` 增加排序键、视图模式、窗格/显示开关、置顶与手动顺序、`updated_at`；`query_json` 新字段必须向后兼容（旧行按现语义读取）；提供重命名、以当前视图更新、排序/置顶 API；同名冲突返回可判定的错误而不是静默覆盖。
  - F-014：`collections`/`collection_assets` 落地新建/重命名/删除、批量加入/移出、按相册计数、按相册列举资产；同一资产可属多个相册；删除相册只删关系不删资产；资产删除与来源移除时级联清理关系行；导出相册清单（路径列表）不复制文件。
  - 迁移前自动备份沿用第二轮既有流程；`docs/backlog/constraints.md` 的「移除来源/资产只动 DB」不变。
- **不做**：侧栏/菜单 UI（任务 G1）；不改搜索 SQL（顺延 3）；不改打标队列。
- **验收**：临时 DB 迁移测试覆盖 v8→v9 与旧行读取；更新集合后内容与当前筛选一致；重命名不改 ID；相册加入/移出后计数即时、重启保持；删除相册资产仍在；级联删除无孤儿关系行。
- **冲突**：`db.rs` 迁移由本任务独占；顺延 3 在 v9 合入后 rebase；任务 C 与顺延任务若需要持久化只能写 `settings` JSON，不得加迁移。

### 任务 B：F-004/F-045 相似核心

- **文件**：`dedup.rs`、`state.rs` 的相似查询入口、`model.rs`（阈值/结果结构）、测试；不碰 `lib.rs`、不改 schema。
- **内容**：
  - F-004：`similar_to_anchor(path, threshold, limit)`——直接复用库里已存的 `phash`，不重新解码；按汉明距离升序返回距离/相似度百分比；与全库分组解耦（不做链式合并）。
  - F-045：阈值预览 API——给定候选阈值返回预估组数/成员数（可用采样，需说明采样比例与误差）；阈值语义与去重 6/10/14 预设统一，支持 0–32 连续值；低阈值时返回可判定的「链式合并风险」标记。
  - 阈值改变只影响展示与预览，不清空既有评审状态（沿用现有签名策略）。
- **不做**：滑杆/结果视图 UI（任务 G2）；不换 `image_hasher`（换库会使评审状态失效，见 [component-research.md](component-research.md)）。
- **验收**：10 万暖库单张锚点查询 ≤300ms，实测数字写回 [pending/browse.md](pending/browse.md)；距离排序与 `similarity_pct_from_distance` 一致；预览组数与实扫组数偏差有记录；阈值/距离相关单测覆盖边界 0 与 32。
- **冲突**：`dedup.rs` 由本任务独占；任务 C 不改去重分组语义；UI 结果视图只消费本任务返回的路径序列。

### 任务 C：F-063 + F-037 + F-032 剩余 + F-030 剩余（扫描/来源运维核心，`state.rs` 扫描路径唯一负责人）

- **文件**：`state.rs` 扫描/对账路径、`scan_filter.rs`、`watch.rs`、`model.rs`（已有 `ScanProgress`）、`settings` JSON、测试；不碰 `lib.rs`、不加迁移。
- **内容**：
  - F-063：把 `ScanProgress`（阶段/done/total/当前文件/来源/吞吐/ETA）真正接入遍历、哈希、缩略图、打标、对账各阶段；按节流刷新（避免每文件写 UI 状态）；扫描结束给出结构化摘要（新增/更新/跳过/失败/耗时）；多来源可区分当前来源。
  - F-037：「从图库移除（不删磁盘）」+ 持久忽略列表（按路径/哈希/glob 三种粒度先定语义再实现）；复用现有 `Db::delete_assets`；重扫与监听都跳过已忽略项；提供查看与恢复入口的数据接口；与 F-032 的排除规则收敛成同一套「忽略规则」判定函数。
  - F-032 剩余：排除/忽略规则的编辑与执行接口（校验、命中预览已有，需要「对已入库但被规则排除的资产」提供保留/移出的批量结果），任何移出都只动 DB。
  - F-030 剩余：目录级重命名/跨来源移动的识别策略结论；来源在启动即离线时的自恢复监听；真实 `notify` 集成测试（临时目录事件风暴不阻塞、不产生重复条目）。
- **不做**：进度条/状态栏/设置页 UI（任务 G3）；不改搜索 SQL、不改打标队列内部逻辑。
- **验收**：10 万冷导入可看到阶段切换与 ETA（数字记录到 [pending/ops.md](pending/ops.md)）；停止后状态正确；节流不降低扫描吞吐（与 42.3min 基线对比不劣化）；移除后磁盘文件不变、重扫不重新导入、可恢复；忽略规则不误删磁盘文件；监听测试在无 UI 环境可跑。
- **冲突**：`state.rs` 扫描/对账路径由本任务独占；任务 A 只加组织接口、任务 B 只加相似查询函数，都要避开扫描函数；后续与顺延 1 共享 `main.rs` 时按 rebase 处理。

## 顺延任务（本轮不派；方案与门禁保留，等 A/B/C 收口后再排）

> 2026-10-27 决定：Wave 1 只开 A/B/C 三条核心线。下面三条保留已确认的范围、验收与冲突边界，不在本轮开工。

### 顺延 1：F-061 剩余（备份计划与恢复核心）

- **文件**：`backup.rs`、`db.rs` 只读小方法、`main.rs` 启动接线、`settings` JSON、测试；不碰 `lib.rs`、不加迁移。
- **内容**：按日/周自动备份与保留 N 份；恢复流程（独占校验、恢复前先把当前库另存、失败不破坏现有库、提示重启）；大库备份进度复用任务 C 的 `ScanProgress` 风格或独立 `BackupProgress`；VACUUM/维护入口；设置导出/导入。
- **不做**：设置页 UI 随本条一起顺延（第三轮的 G3 不含它）；不做 MSI/签名/自动更新（F-065 仍远期）。
- **验收**：备份在另一数据目录可恢复并正常打开；保留策略删除的是自己生成的备份文件而非用户文件；恢复失败时原库完整性不变；自动备份不阻塞扫描（扫描中拒绝或排队，沿用现有约束）。
- **冲突**：与任务 C 都可能碰 `main.rs`/设置读写；后合者 rebase；不得改 `state.rs` 扫描路径。

### 顺延 2：F-033/F-040 媒体评估线（不承诺未实测格式）

- **文件**：`media.rs`、`ffmpeg.rs`、新解码模块（若实测通过）、`sidecar/tagger/repair.py`（仅评估，不改协议）、`docs/FFMPEG.md`；不碰 `lib.rs`、不加迁移。
- **内容**：
  - F-033 剩余：HEIC/HEIF（`libheif-rs`，原生 libheif LGPL）、RAW（`rawloader` LGPL-2.1 / LibRaw）、PSD（`psd`）——每项先按真实样本做解码成功率、体积、解压炸弹与许可落盘审查；通过才接进扫描，不通过就把结论与样本失败原因写回文档。
  - 动画 GIF/WebP 的首帧之外是否需要播放入口，先给结论再接（与 F-066 视频扩展区分开）。
  - F-040 剩余：pHash 迁移 `image_hasher` 的结论（会失效既有评审状态，默认不换）、损坏修复迁 ffmpeg 还是保留 Python、是否随包 `ffprobe`、LGPL 打包范围与许可落盘。
- **不做**：格式徽标/解码 UI（本轮不做，格式提示已在第二轮落地）；不改 512/t256 缓存键 `v3` 与批处理池参数。
- **验收**：新格式若接入必须给出真实样本的扫描/缩略图/尺寸/哈希数据与安装包体积增量；10k/100k 缩略图指标不劣于 52.2s/738.7s/42.3min 基线；未接格式在文档里明确「不支持」及原因；许可与原生依赖记录进 `docs/FFMPEG.md`/`constraints.md`。
- **冲突**：与任务 C 共享 `state.rs` 的缩略图/媒体函数时，媒体函数改动优先合，扫描进度后合并 rebase；sidecar 协议本轮冻结，任何协议变更需单一负责人。

### 顺延 3：F-012 剩余（非名称排序性能）

- **决定**（2026-10-27 确认）：`p95 ≤ 150ms` 不再当硬门禁，改为「先小基准，取实测上限并写回文档」；A/B/C 收口后再排这条线。
- **文件**：`db.rs` 的排序/分页路径与 `ensure_search_indexes`；不碰 `lib.rs`。
- **内容**：先做小基准再拍板，候选方案至少覆盖：每排序键的覆盖索引 + `INDEXED BY`、把 FTS 命中 rowid 物化到临时表再排序、denormalized 预排序键列（若要加列须并入任务 A 的 v9，不得另起迁移）、以及「接受当前上限 + UI 侧异步分页缓解」的兜底结论。短词 `LIKE` 回退保持现状。
- **不做**：不引入 Tantivy（沿用 [component-research.md](component-research.md) 结论）；不改 UI 防抖。
- **验收**：同一份 10 万合成库上重测，给出方案对照表、实测 p50/p95 与选定理由；把 F-012 的验收写成记录到的上限（当前记录：名称排序 p95 133ms、非名称排序 p95 276–304ms）；结果与内存过滤逐条一致的单测继续通过。
- **冲突**：`db.rs` 在任务 A 之后合；`SearchQuery::resolve_tag_aliases` 的别名解析仍在调用方，改排序路径不能漏。

## Wave 2（UI 车道；A/B/C 接口稳定后）

> 任务 0 已确认执行，G1/G2/G3 按模块并行，`app.rs`/命令面板/底栏由唯一 UI 集成人写入；每段单独合入并附隔离数据根截图。

### 任务 G1：F-013/F-014 组织 UI

- **命名冲突（复核发现）**：侧栏现有分区 id 就是 `"collections"`、标题是「智能集合」（`crates/kanalib-ui/src/lib.rs:5335` 附近），而 `"collections"` 同时是数据库里的相册表名。新增手动相册分区必须换一个新的 section id（如 `"albums"`），并在代码里把「智能集合=saved_searches」「相册=collections 表」的命名固定下来，否则搜索该字符串时两套概念会互相误导。
- **内容**：侧栏「智能集合」提供保存/应用/重命名/用当前视图更新/删除/置顶与拖动排序；应用集合时恢复筛选、搜索、排序、视图模式与窗格状态；「相册」独立分区，支持新建/重命名/删除、把选中资产加入/移出、显示计数与所属相册；同名冲突与批量操作有明确提示。
- **验收**：隔离数据根 UIA 实测重命名后 ID 不变、应用集合恢复排序；重启后顺序与置顶保持；相册增删即时刷新计数；不移动磁盘文件。

### 任务 G2：F-004/F-045/F-005/F-003 相似与浏览 UI

- **内容**：检查器与右键菜单「查找相似」入口，结果为可继续筛选/多选/快速查看的临时视图并显示距离与相似度；设置页与去重页的 0–32 阈值滑杆 + 预估组数/成员数预览，保留 6/10/14 快捷档与临时值/默认值区分；分组浏览与时间线在现有 6 种视图与固定格尺寸（大 288/268、中 160/150、小 128/112，行高 +68）之上加组头，不改动格内布局；对比模式支持当前去重组或选中 2–4 张，同步缩放/平移、差异高亮、可标记保留项。
- **验收**：不违反 [constraints.md](constraints.md) 的 UI 固定规格（6 视图、`Contain` 不裁切、无平铺/幻灯片）；10 万库分组滚动不劣于现有基线；对比中只加载当前对比图、复用 `ThumbCache`；Esc 返回原视图与滚动位置；截图与 UIA 记录。
- **待确认**：F-005 组头吸顶与 F-003 并排布局属新版式，需要用户先确认视觉方向再实现。

### 任务 G3：F-036/F-037/F-063/F-032 交互与运维 UI

- **内容**：拖入文件夹提示「添加为来源」、拖入图片定位或询问添加父目录、拖拽高亮遮罩、取消无副作用；选中资产「从图库移除（不删磁盘）」与忽略列表查看/恢复入口；底栏/侧栏结构化进度条（阶段、吞吐、ETA、当前来源）与扫描结束摘要；排除/忽略规则编辑器（录入、校验、命中预览、执行保留或移出）。
- **不做**：自动备份计划与恢复入口的 UI（属顺延 1 的 F-061 剩余，本轮不派）。
- **验收**：拖拽在真实 Windows 资源管理器拖放路径下实测（含日文/Unicode 与长路径）；移除后网格与计数即时更新、重扫不重新导入；进度刷新不影响扫描吞吐；所有 UI 冒烟使用隔离数据根，不触碰真实图库。

## 可选研究线（不占核心，不写实现代码）

- **F-035 Immich/WebDAV**：只交付方案文档——`Source` trait 边界、凭据进 Windows 凭据管理器、离线不阻塞本地库、只读约束、跨源精确去重语义、`reqwest`/`reqwest_dav`/`keyring` 许可与体积评估。延续「本地优先，多来源=多本地文件夹」的决定，实现仍留远期。
- **F-016 CLIP 语义搜索**：独立 sidecar 可行性 + 许可 + 基准（`open_clip` + `usearch`/`sqlite-vec`），不改现有 tagger、不进主库 schema。
- **F-064 ICC/颜色管理**：`lcms2` 在现有 ffmpeg 构建里的可用性核查，以及广色域样本的偏色量化；先测再决定是否立项。

## 合并顺序与禁止事项

1. 任务 0（UI 拆分）最先合；在它合入前任何任务不得改 `lib.rs`。
2. Wave 1 内 A/B/C 并行：schema 迁移只有 A 一个负责人，`state.rs` 扫描路径只有 C，`dedup.rs` 只有 B；sidecar 协议本轮冻结。顺延任务不派发，任何代理不得顺手开工。
3. C 的忽略/排除规则持久化与 A 的组织迁移分文件落地：A 只加 `db.rs` 迁移，C 只写 `settings` JSON；后合者 rebase。
4. Wave 2 在 Wave 1 对应接口合入后开工；G2 的 F-005/F-003 需等用户确认视觉方向才动。
5. 本轮禁止：第二个 schema 负责人、多个代理同时写 `app.rs`/命令面板/底栏、引入 Tantivy 或 `image_hasher` 替换、把 F-035/065/066 或三条顺延线当实现线开工、自动删除任何磁盘文件。
6. 每个交付附：改动文件、接口签名、迁移/备份说明、测试命令与结果、未完成边界；UI 附隔离数据根截图与 UIA 记录。

## 验收与验证约定

- 每轮收尾重跑 `cargo test --workspace`（当前基线 core 82 + UI 15，7 项按需忽略）与 `cargo check --workspace`。
- 每轮收尾重跑 `bash scripts/check_backlog_docs.sh`：registry ID 不重复、不丢失，implemented/pending 状态互不矛盾。
- 基准数字必须说明用的是哪份库（第二轮已出现同一查询在不同合成库上 276–304ms 与 350–380ms 的差异）。
- `.ps1` 一律 `pwsh -NoLogo -NoProfile`；UI 自动化只针对本应用窗口，用窗口内 UIA/消息，不用全局键鼠。

## 范围决定（2026-10-27 与用户确认）

1. **任务 0 拆分 `lib.rs`：执行。** 用一条不产出用户可见功能的前置任务，换取 Wave 2 三条 UI 线的并行度。
2. **Wave 1 只开 A（F-013/014）、B（F-004/045）、C（F-063/037/032/030 剩余）。** 备份恢复（F-061）、媒体评估（F-033/F-040）、搜索性能（F-012）转为本文件的「顺延任务」，本轮不派发。
3. **F-012 非名称排序：`p95 ≤ 150ms` 不再是硬门禁。** 改为「先小基准，取实测上限并写回文档」，已同步到 [constraints.md](constraints.md) 与 [pending/search-organization.md](pending/search-organization.md)。
4. **HEIC/RAW/PSD：按「实测样本 + 许可门」处理。** 只有真实样本解码成功率、安装包体积增量与 LGPL 原生依赖审查都过关才接进扫描，否则在文档里明确写「不支持」及原因。

## 视觉方向（2026-10-28 用户确认）

- **F-005 分组浏览**：采用 **吸顶分组头**（Windows Explorer 风格）——分组标题滚动时吸顶、可折叠、显示组内数量；按修改时间/拍摄时间/来源/评级分组。必须保留现有行级虚拟化，不得让 10 万库滚动退化。
- **F-003 图片对比**：采用 **并排 2–4 张 + 同步缩放/平移**（digiKam Light Table 风格），元数据差异高亮、可标记保留项；从当前去重组或选中 2–4 张发起。
- 这两项已不再阻塞 G2；G2a（F-004/F-045 UI）已在做，F-005/F-003 随后派发。

## 第三轮执行结果（任务 0 + Wave 1 + Wave 2 部分，2026-10-28）

| 任务 | 状态 | 提交 |
|---|---|---|
| 任务 0 UI 单体拆分（18 个模块） | 已合入 | `d9494cb` |
| A F-013/F-014 组织核心（schema v9） | 已合入 | `6c461c2` |
| C F-063/F-037/F-032/F-030 扫描与来源运维核心 | 已合入 | `fec35bf`、`7c24aef`、`ab4070b`、`4dfdf39`、`be110cc` |
| B F-004/F-045 相似核心 | 已合入 | `f25a1b0` |
| G1 F-013/F-014 组织 UI | 已合入 | `aeb581f` |
| G3 F-036/F-037/F-063/F-032 UI | 已合入 | `f4e72b2`、`521f261` |
| G2a F-004/F-045 相似与阈值 UI | 已合入 | `4a7f539`、`982a078` |
| G2b1 F-005 分组浏览（吸顶组头） | 已合入 | `50e66ea`、`982a078` |
| G2b2 F-003 对比视图 | 已合入 | `926f9a4` |

- **验证**：`cargo test --workspace` core **110 passed / 11 ignored** + UI **21** + bin 3；`cargo check --workspace` 无代码警告；`scripts/check_backlog_docs.sh` 通过。测试基线从 82/7 增至 110/11（core）与 21（UI）。
- **任务 0**：`lib.rs` 13,779 行拆成 18 个文件（`app`/`settings`/`dedup`/`sidebar`/`inspector`/`views/*`/`tags`/`thumb_cache`/`viewer`/`status_bar`/`search`/`theme`/`format`/`platform`），对外导出与 `src/main.rs` 不变；拆分前后截图 PSNR `inf`、UIA dump 50 个节点逐字节相同。
- **A**：`saved_searches` 增 9 列（排序键/升序/视图/窗格/侧栏与扩展面板开关/置顶/手动顺序/`updated_at`），新增 `SavedSearchView`、`Collection`、`OrganizationKind`、`DuplicateNameError`；集合重命名/更新/置顶/排序、相册 CRUD 与批量加入移出/计数/导出路径清单、资产删除与来源移除级联；`SCHEMA_VERSION = 9`，迁移前自动备份 `kanalib.db.bak-v8-<ts>`，旧行按默认视图读取。
- **C**：`ScanProgress`/`ScanSummary` 接入遍历/哈希/缩略图/打标/对账（节流刷新、结束摘要、多来源可区分）；`remove_from_library` 与持久忽略列表（路径/哈希/glob）落地，重扫与监听共用同一判定；排除规则编辑与已入库批量保留/移出；监听自恢复、跨来源移动按 `idx_assets_sha1` 识别。忽略规则只写 `settings` JSON，未加迁移。
- **B**：`similar_to_anchor` 与 `preview_similar_threshold`；阈值统一 0–32（保留 6/10/14 快捷档），低阈值返回链式合并风险标记。10 万库单锚点查询 p50 6.8–9.7ms、p95 15.3–18.1ms（目标 ≤300ms）；预览对 ≤4096 张精确，更大库抽 4096 个种子（10 万约 4.1%，组数偏差 +3.0% @0 / 0% @6/10/14），检测到链式合并时回退精确全扫（1.8–2.6s）。
- **G1**：侧栏「智能集合」支持重命名、用当前视图更新、删除、置顶与上下移动，应用时恢复排序/视图/窗格；新增「相册」分区（section id `albums`，不复用 `collections`），可新建/重命名/删除、加入/移出选中资产、显示计数；检查器显示「所属相册」。同名冲突用 `DuplicateNameError` 提示。顺带修正 PromptDialog 先清空状态再提交导致读不到目标的问题。
- **G3**：拖拽导入（文件夹→添加来源、来源内图片→定位、来源外→询问父目录、高亮遮罩）；「从图库移除（不删磁盘）」与忽略列表查看/恢复；底栏阶段/吞吐/ETA/当前来源与扫描结束摘要；设置页排除/忽略规则编辑器（校验、命中预览、保存·保留记录 / 保存·移出命中、二次确认）。冒烟时发现并修复底栏文字重叠（状态项 `min_w(0)`/`overflow_hidden` + 错误时紧凑摘要 + 错误首行 56 字截断）。
- **G2a**：检查器「查找相似」进入独立结果视图（距离/相似度、本地筛选、全选/清除/快速查看）；无 pHash 的资产给出明确失败提示。去重页与设置页提供 0–32 滑杆（保留 6/10/14 档）、临时值与默认值区分、预估组数/成员数与链式合并风险标记；预览在后台线程执行。
- **G2b1**：按修改时间/拍摄时间/来源/评级分组，组头显示名称与数量、滚动吸顶、可折叠；`ui.group_by` 持久化，「查看 → 分组」菜单与命令面板均可切换；保持行级虚拟化。
- **合并修复**：相似视图不再回写图库的 `view.visible_indices`（否则返回图库变空列表）；补「查看 → 分组」菜单入口。
- **G2b2**：并排 2–4 栏对比（`compare.rs`），同步缩放/平移、适应/原图切换；差异表区分差异项与相同项，可标记保留项（只标记，不删不移）；Esc 返回原视图与滚动位置；只加载当前对比图。入口：命令面板 `compare:selected`、检查器/右键「对比选中」、去重组「对比此组 / 对比前 4 张」。
- **未完成边界**：F-036 的真实资源管理器拖放未做端到端验证（仅单测覆盖）；F-005 的 10 万库分组滚动性能未实测；F-003 的「多选后进入对比」未做自动化点击复现（对比视图本身已截图验证，多选交互依赖 Ctrl/Shift 点击）；三条顺延任务（F-061、F-033/040、F-012）未开工。
- **禁止回退**：忽略/排除判定只有一套（`scan_filter.rs` 编译规则 + `state.rs` 统一入口）；忽略规则持久化在 `settings`，不得另加迁移；UI 字段、命令面板与底栏仍只在 `app.rs` 写；`state.rs` 扫描/对账路径不再有第二个写入点；相册分区 id 固定为 `albums`。

## 第四轮预览（不在本轮）

> 已形成任务包：[round-4.md](round-4.md)。

- 本轮顺延的三条：F-061 自动备份与恢复（顺延 1）、F-033/F-040 媒体评估（顺延 2，按实测样本 + 许可门）、F-012 非名称排序基准与上限复测（顺延 3）。
- G2 中因视觉方向未定而没动的 F-003/F-005，若本轮仍未确认则继续顺延。
- F-035 若方案通过则开工（Source 接口冻结后）。
- 候选池小项：F-015 搜索历史、F-025 低频标签清理、F-038 窗口状态记忆、F-039 单实例/托盘、F-046/047 去重与回收站入口、F-067 撤销、F-068 复制到剪贴板。
- F-065 MSI/签名/自动更新、F-066 视频/小说扩展、F-016/F-064 在可行性与许可通过后立项。
