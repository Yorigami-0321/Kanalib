# 第四轮任务包（2026-10-28）

> 上级索引：[../FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md) · 文档地图：[README](./README.md) · 第三轮：[round-3.md](round-3.md)
>
> 第三轮 G2b2 已完成；当前剩余：F-012 非名称排序上限、F-030 网络盘/去重刷新收尾、F-033/F-040 媒体评估、F-036 真实拖放验收、F-061 自动备份/恢复、F-065 安装器，以及候选池。
>
> 本轮目标：收口三条顺延任务 + 一个 UI 小项包；仍不启动大型网络源（F-035 只做可选研究）。

## 任务 0：第三轮验收收尾（QA/文档，不写功能）

- **文件**：`docs/backlog/*`；只有在发现缺陷时才改 UI 模块。
- **内容**：
  - F-036：真实 Windows 资源管理器拖放 E2E（Unicode/日文/长路径、文件夹/图片、取消）；记录自绘标题栏兼容边界。
  - F-005：10 万库分组浏览滚动实测（吸顶/折叠/内存），补记录；F-003：多选 2–4 张进入对比的自动化点击复现。
  - 把通过项从 `pending/` 移入 `implemented.md`，更新 `registry.md`；未通过项写清剩余边界。
- **验收**：截图/指标归档；`scripts/check_backlog_docs.sh`、`cargo check --workspace` 通过；无功能代码改动（除修复验证发现的缺陷）。
- **边界**：不启动 F-012/F-033/F-040/F-061 的实现。

## Wave 1：三条核心线（可并行；不碰 `app.rs`）

### 任务 A：F-061 自动备份与恢复（核心+设置模块）

- **文件**：`crates/kanalib-core/src/backup.rs`、`db.rs` 的备份/保留辅助、`state.rs` 的备份互斥；`crates/kanalib-ui/src/settings.rs`；不碰 `app.rs`。
- **内容**：
  - 定时备份（日/周可配）、保留 N 份/按时间清理；设置存 `settings`，不加 schema。
  - 恢复：独占检查、扫描/打标中拒绝；恢复前自动做 safety backup；校验 `integrity_check` 与关键表计数；恢复后提示重启。
  - UI 在 `settings.rs` 增加计划/保留/恢复入口；命令/状态字段交给任务 E1 统一接入。
- **不做**：文件级备份、云备份、自动更新数据库 schema。
- **验收**：临时数据根完成定时/保留/恢复；恢复失败不破坏当前库；扫描中拒绝；单测覆盖 WAL、失败、恢复。

### 任务 B：F-033/F-040 媒体评估与阶段 2（核心线）

- **文件**：`model.rs`、`media.rs`、`ffmpeg.rs`、`state.rs` 的缩略图/哈希入口、`sidecar/tagger/repair.py`、`docs/FFMPEG.md`；不碰 UI。
- **内容**：
  - F-033：HEIC/HEIF、RAW、PSD 逐格式做“样本 + 许可 + 原生依赖”门槛；通过才接入，不承诺未实测格式；动画 GIF/WebP 首帧/播放能力结论。
  - F-040 阶段 2：pHash 与损坏修复迁移的结论/实施；视频首帧/时长评估；ffprobe 打包与 GPL/LGPL 许可落盘；保持 `image` 回退与 `v3` 缓存。
- **不做**：格式 UI/动画播放器（任务 E1）；视频/小说完整功能。
- **验收**：样本清单与许可记录；方向/alpha/损坏/Unicode 不回归；10k/100k 指标不劣于 52.2s/738.7s/42.3min；无进程/临时文件泄漏；单测 + `docs/FFMPEG.md` 更新。

### 任务 C：F-012 非名称排序与端到端延迟（DB/搜索线）

- **文件**：`db.rs` 的搜索/索引、`crates/kanalib-ui/src/search.rs` 的只读查询入口；不碰 `app.rs`。
- **内容**：
  - 复测 10 万非名称排序全命中 p95 276–304ms；定位 B 树临时排序瓶颈，尝试索引/排序键/keyset 或改为记录上限。
  - 测 UI 端到端键入延迟（防抖、可见索引重建、渲染）；保持 FTS5，不引入 Tantivy。
- **不做**：改变搜索语义、引入第二套索引。
- **验收**：给出可复现 p50/p95 报告；若无法稳定 <150ms，明确记录“名称排序达标、非名称上限”的取舍；结果与内存过滤逐条一致。

## Wave 2：F-030 监听收尾（核心线，Wave 1 后）

- **文件**：`watch.rs`、`state.rs` 监听/对账、`dedup.rs` 刷新入口。
- **内容**：OneDrive/网络盘事件语义与离线恢复；监听批次结束后的去重组刷新；不引入 UI/schema。
- **验收**：临时目录/离线/重命名/跨来源移动测试；监听批次不遗留过期去重组；10 万事件风暴不阻塞 UI。

## Wave 3：UI 集成（唯一 UI 车道，Wave 1/2 API 稳定后）

### 任务 E1：F-061/F-033/F-012 UI 收口

- **文件**：`settings.rs`、`search.rs`、`viewer.rs`/`format.rs`；`app.rs` 只由本任务负责人接命令/状态。
- **内容**：备份计划/保留/恢复；格式能力/失败提示/动画入口；搜索防抖与端到端延迟；不新增浏览/相册功能。
- **验收**：隔离数据根截图/UIA；`cargo check --workspace`；搜索/恢复/格式提示实测。

### 任务 E2（可选，UI 有余量时）：小项包

- 候选：F-015 搜索历史/语法帮助、F-025 低频标签清理、F-046 去重组搜索排序、F-068 复制图片到剪贴板。
- 只由 E1 同一 UI 负责人按顺序做，不并行；每项独立提交，附截图。

## 独立可选线（不占核心/UI）

- **F-035 研究/骨架**：`sources/` 模块 + `reqwest`/`reqwest_dav`/`keyring`，只做接口/许可/凭据/离线策略，不接 UI、不迁移 schema；本轮不承诺完整网络源。
- **F-065 安装器/签名/更新**：仅 scripts/CI；需要代码签名证书与更新通道，证书未就绪时只做方案与校验脚本。
- **F-064 ICC**：评估 `lcms2`，不改现有显示链；排在媒体线之后。

## 合并顺序与禁止事项

1. 任务 0 可先做；Wave 1 A/B/C 并行，A 与 C 的 `db.rs` 改动由同一 DB 负责人合并，B 只碰媒体/缩略图入口。
2. Wave 2 D 在 Wave 1 后合；Wave 3 E1 在 A/B/C API 稳定后合，E2 再排。
3. 禁止：多个代理同时改 `app.rs`；同时改两份 schema；在 F-035 未冻结 Source 接口前接 UI；把 HEIC/RAW/PSD 未实测格式写成已支持。
4. 每项交付附：改动文件、接口、迁移/备份、测试/指标、未完成边界；UI 附截图。

## 第四轮执行结果（总览）

| 任务 | 状态 | 提交 |
|---|---|---|
| 任务 0 第三轮验收 | 完成（F-003 多选/F-036 真实拖放待手动验收） | `f838b52`、`f73c006` |
| A F-061 自动备份/恢复 | 完成（大库进度/VACUUM 未做） | `1dcaef0` |
| B F-033/F-040 媒体评估 | 完成（PSD 接入；HEIC/RAW 明确不支持；视频未做） | `1ae110b`、`f75b0a6` |
| C F-012 搜索排序 | 完成（150ms 未达成，记录同库上限；UI 探测上限 512） | `b6dd327`、`15c804b` |
| D F-030 监听收尾 | 完成（真实 OneDrive/网络盘未实测） | `481171d`、`94a2394` |
| E1 UI 集成 | 完成（动画仅系统应用播放；100k 搜索异常见边界） | `02e4960`、`fb090fe` |
| E2 可选小项 | 未开工 | — |
| F-035/F-064/F-065 独立可选线 | 未开工 | — |

- **最终验证**：`cargo test --workspace` core **150 passed / 15 ignored**、UI **36 passed**、bin **3**；`cargo check --workspace` 无代码警告；`scripts/check_backlog_docs.sh` 无新增矛盾。
- **归档**：Task 0 验收证据 `.kanalib/r4_task0/`；E1 备份/格式截图与搜索日志 `.kanalib/r4_e1/`。
- **剩余边界**：F-012 的 150ms 目标（`synthetic_0004` 窄 FTS 异常已修复，核心 3.5ms、UI page 55ms/visible 360ms；真实 10 万缩略图库仍待实测）、F-030 真实网络盘、F-033 HEIC/RAW、F-040 视频、F-036 真实拖放、F-003 Ctrl 多选均未完成/未实测；F-039 多实例互斥仍是 F-061 恢复独占的前提。

## 任务 0 执行结果（2026-10-30）

> 范围：只做验收、证据归档与文档，不改功能代码；本轮未重跑 UI 自动化，截图/UIA/日志/内存采样均来自 2026-10-29 的同一轮实测。原始目录 `target/r4-task0-data/`（不入库），本地归档 `.kanalib/r4_task0/`（`.kanalib/` 已在 `.gitignore`，图片与日志不提交）。

- **F-036 拖拽导入：未通过 E2E，待用户手动验收。** 真实 Windows 资源管理器拖放无法在窗口内自动化中可靠复现（`ExternalPaths` 由系统拖放目标注入，UIA/消息模拟不可靠），本轮未做，不写成已通过；手动验收清单（文件夹、来源内图片、来源外图片、取消、Unicode/日文/长路径、重复来源、自绘标题栏兼容）已写入 [pending/library-sources-formats.md](pending/library-sources-formats.md)，registry 保持「实现中」。
- **F-005 分组浏览：通过（带边界）。** 100000 张合成库上，修改时间/拍摄时间/来源/评级四种分组菜单可用，组头吸顶、折叠展开、连续滚动正常；600s 滚动采样工作集约 145MB、私有约 190MB，无增长（`.kanalib/r4_task0/100k-scroll-memory.csv`，无表头 30 行，第一列 20–600s）。边界：该库来源目录 `C:\bench` 已离线（`logs/100k-group.log` 记录 `source inaccessible: C:\bench` 与 `thumb missing and could not rebuild`），网格只有占位符，因此只证明分组布局/行级虚拟化/内存，不证明 10 万真实缩略图滚动；10k 副本（缩略图缓存可用、源目录离线）分组/滚动视觉通过。
- **F-003 图片对比：3 张去重组对比通过，多选入口未复现。** 从 10k 去重组「对比此组」进入 3 张并排对比：同步缩放 ×1.95、同步平移、差异表（差异 1 项/相同 8 项）、保留标记、Esc 返回去重组均通过。边界：Ctrl 多选 2–4 张后进入对比仍未复现——`.kanalib/r4_task0/screens/10k-multiselect-ctrl-attempt.png` 显示 Ctrl 点击多张后仅 1 张（`synthetic_00002_city_合成.jpg`）被选中；不能把去重组入口当作多选复现，多选对比入口待手动验收。
- **证据**：`.kanalib/r4_task0/screens/`（28 张：100k 分组/折叠/连续滚动/菜单、10k 分组/滚动、4 张对比截图（并排/缩放/平移/Esc 返回）、多选未复现、命令面板）；`.kanalib/r4_task0/100k-scroll-memory.csv`；`.kanalib/r4_task0/uia-100k-grouped.txt`、`.kanalib/r4_task0/uia-100k-menu.txt`、`.kanalib/r4_task0/uia-10k-dedup.txt`、`.kanalib/r4_task0/uia-10k-palette.txt`；`.kanalib/r4_task0/logs/100k-group.log`、`.kanalib/r4_task0/logs/10k-group.log`。
- **未完成边界**：10 万真实缩略图下的分组滚动（需可访问的 10 万缩略图库）；Ctrl 多选 2–4 张发起对比；真实资源管理器拖放。registry 状态不改（F-003/F-005 已实现、F-036 实现中）。

## 任务 E1 执行结果（2026-10-30）

> 唯一 UI 车道；提交 `02e4960`（本地，未推送），改动 `app.rs`/`settings.rs`/`search.rs`/`viewer.rs`/`format.rs`/`inspector.rs`/`status_bar.rs` 与 `lib.rs` 的 import 收尾。截图归档 `.kanalib/r4_e1/`（`.kanalib/` 已忽略，不提交）。

- **F-061 备份/恢复 UI**：命令面板新增「备份与恢复」（立即备份、恢复备份…、取消待恢复、打开设置·数据库备份）；启动时 `pending_restore` 通过 `pending_restore_hint` 提示“重启后生效，可在设置页取消”，不自动替换运行中的库；设置页备份计划/保留/恢复/损坏候选拒绝均已截图。恢复仍走“校验 + safety backup + 下次启动应用”，运行期不换库。
- **F-033 格式 UI**：PSD 检查器显示「需要 FFmpeg 解码 · 仅合成图」及图层/Alpha/pHash/`.psb` 边界；GIF/WebP 显示「内置解码 · 动画仅首帧」，查看器提供「用系统应用播放」出口，未实现应用内逐帧播放；HEIC/RAW 仍不支持。
- **F-012 搜索 UI**：单次 SQL 探测上限从 10000 降为 `UI_SEARCH_PAGE_LIMIT=512`（超过回退内存过滤，结果语义不变）；`KANALIB_SEARCH_TIMING` 记录防抖/page/run/visible/frame。100k 缺图最坏库上 `synthetic` 热查 0.93s、`sky` 0.66s（原约 1.6/1.45s），但仍未达 150ms。`synthetic_0004` 10 命中 14s 异常已定位：`search_extent` 普通 `JOIN` 在 SQLite 3.46 下逐行探 FTS；改 `CROSS JOIN` 后核心 `search_page` 3.5ms、UI page 55ms/visible 360ms，回归测试 `fts_extent_query_plan_prefers_fts_driver`（修复提交 `fed7857`）。完整表见 [pending/search-organization.md](pending/search-organization.md)。
- **验证**：`cargo test --workspace` core **150 passed / 15 ignored**、UI **36 passed**、bin **3**；`cargo check --workspace` 无警告（仅 vendor profile 提示）。
- **截图**：`.kanalib/r4_e1/screens/settings-backup-section.png`、`settings-backup-plan-enabled.png`、`settings-backup-now.png`、`restore-corrupt-failure.png`、`psd-inspector.png`、`gif-inspector.png`、`gif-viewer.png`；搜索日志 `.kanalib/r4_e1/logs/search100k-limit512c.log`。
- **未完成边界**：应用内动画逐帧播放、100k 真实缩略图库的键入延迟、备份大库进度/VACUUM、真实 OneDrive/网络盘事件语义、F-036 真实拖放与 F-003 Ctrl 多选仍待手动验收。

## 第五轮预览（不在本轮）

- F-035 完整 Immich/WebDAV 与跨源去重（若研究线通过）。
- F-038 窗口状态、F-039 单实例/托盘、F-047 回收站入口、F-067 撤销历史。
- F-016 CLIP、F-064 ICC 正式接入、F-066 视频/小说。
- F-065 签名/自动更新完整链路。
