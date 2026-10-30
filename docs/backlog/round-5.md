# 第五轮任务包（2026-10-28）

> 上级索引：[../FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md) · 文档地图：[README](./README.md) · 第四轮：[round-4.md](round-4.md)
>
> 第四轮已完成（F-061/F-033/F-040/F-012/F-030/E1），但留下真实拖放、Ctrl 多选、真实网络盘、10 万真实缩略图键入延迟、大库备份进度等边界。本轮先做桌面基础与验收，再启动 F-035 网络源主线和媒体扩展；所有 UI 仍只有一条车道。

## 任务 0：第四轮边界验收（QA/数据，不写新功能）

- **文件**：`docs/backlog/*`；仅在发现缺陷时改对应模块。
- **内容**：
  - F-036 真实 Windows 资源管理器拖放：文件夹/来源内图片/来源外图片/取消/Unicode 日文/长路径/重复来源；用用户手动操作验收，不能再用 UIA 模拟替代。
  - F-003 Ctrl 多选 2–4 张后进入对比；F-005 10 万真实缩略图库分组滚动（需可访问的缩略图库，不再用离线占位库）。
  - F-030 真实 OneDrive/网络盘事件、断线重放与恢复；F-012 10 万真实缩略图库的 UI 键入延迟；F-061 大库备份进度/VACUUM 评估。
  - 文档：把 F-038/F-039/F-035 从候选/远期提前立项并更新 registry/pending；F-046/F-047/F-015/F-025/F-068 在 D 完成后再标记。
- **验收**：每项写明「通过/未通过/环境不可用」；证据放 `.kanalib/`；通过项归档，未通过项保留在 pending 并写清剩余边界。
- **边界**：不启动 F-035/F-040/F-064；不把未复现项写成已通过。

## 任务 A：F-039 单实例 + F-038 窗口状态（前置，可与 QA 并行）

- **文件**：`src/main.rs`、`crates/kanalib-ui/src/platform.rs`、`app.rs` 的窗口状态少量接入；不碰 DB 搜索/媒体。
- **内容**：
  - F-039 单实例：第二实例把已有窗口置前而不是再开一个；Windows mutex/命名事件；headless 与便携版兼容；无全局键鼠自动化。
  - F-038 窗口状态：保存窗口位置/大小/最大化与屏幕；启动时恢复并夹取到可见工作区；设置页/配置文件持久化。
  - F-039 是 F-061 恢复独占的前提，必须先于大库恢复验收。
- **不做**：托盘/开机启动（F-039 的扩展部分）、多窗口、自动更新。
- **验收**：双开只保留一个主窗口并置前；窗口状态重启恢复；多显示器/125% DPI 不越界；便携包/headless 正常；UIA/窗口截图。

## 任务 B：F-035 网络源（主线，唯一 schema/state 负责人）

- **文件**：新增 `crates/kanalib-core/src/sources/`（`mod.rs`、`local.rs`、`immich.rs`、`webdav.rs`）、`db.rs` sources 表/迁移、`state.rs` 源编排、`settings.rs`/`app.rs` 源设置 UI（UI 部分由任务 D 收口）。
- **顺序（同一车道，不可拆并行）**：
  1. **Source 抽象**：把现有本地 roots 包成 `Source` trait；行为与数据不变；先不动 UI。
  2. **Immich 只读 MVP**：版本探测、分页搜索、`updatedAfter` 增量、缩略图缓存、只读约束；凭据进 `keyring`；断线不阻塞本地库。
  3. **WebDAV 只读**：PROPFIND、ETag/Last-Modified、并发限流、目录缓存；凭据同上。
  4. **跨源精确去重**：按 SHA1 归组，复用现有去重 UI/评审状态；跨源只读。
- **不做**：写回、删除远端、上传、下载 original、跨源相似去重（后续）。
- **验收**：同一张图本地/Immich 进同一 SHA1 组；断线/密钥失效不阻塞本地；远端零写入；本地 Source 行为不回归；`sources` 表迁移前备份；网络源测试用隔离数据根/测试容器，不碰真实图库。
- **冲突**：本任务独占 `sources` schema 与 `state.rs` 源编排；任务 A/C 不改 schema；任务 D 只做 UI 集成。

## 任务 C：F-040 视频 + F-033 格式门（媒体线，可与 B 并行）

- **文件**：`ffmpeg.rs`、`media.rs`、`model.rs`、`state.rs` 缩略图入口、`viewer.rs`/`format.rs`（播放 UI 由 D 收口）、`docs/FFMPEG.md`。
- **内容**：
  - F-040 视频：首帧缩略图、时长/分辨率元数据、系统默认播放器打开；不实现内置解码器播放。
  - F-033：HEIC/HEIF、RAW 仅在真实样本 + 可分发解码链 + 许可审查通过后接入；PSD 已接 ffmpeg-only，不重复。
  - 保持批处理池、LGPL 运行时、`v3` 缓存和 `image` 回退；视频仅作为新 media_kind，不改现有图片路径。
- **不做**：应用内视频播放器、转码、批量视频打标、小说/EPUB。
- **验收**：视频样本首帧/时长/系统播放；图片格式不回归；10k/100k 图片缩略图指标不劣化；HEIC/RAW 未通过门槛时明确写“不支持”，不伪装支持；许可/SHA256 记录。
- **冲突**：不改 `sources` schema；如必须改 `state.rs`，只加 media_kind 分支，与 B 的源编排合并时由核心集成人 rebase。

## 任务 D：UI 集成与小项包（唯一 UI 车道）

- **文件**：`app.rs`、`settings.rs`、`viewer.rs`、`format.rs`、`sidebar.rs`、`dedup.rs`、`status_bar.rs`；`app.rs` 仅本任务负责人写。
- **顺序**：
  1. F-035 源设置：添加/测试/同步 Immich/WebDAV、凭据提示、来源状态/错误、只读标识。
  2. F-040 视频/格式：时长/首帧、系统播放入口、HEIC/RAW 不支持提示。
  3. 小项包（按余量，独立提交）：F-047 回收站入口/审计、F-046 去重组搜索排序、F-015 搜索历史/语法帮助、F-025 低频标签清理、F-068 复制图片。
- **不做**：F-066 视频/小说完整页、F-016 CLIP、F-064 ICC、F-065 安装器。
- **验收**：隔离数据根截图/UIA；网络源连接/断线/空结果；视频/格式提示；小项各自截图；`cargo check --workspace`；不混入 F-035 核心逻辑。

## 独立可选线（不占核心/UI）

- **F-065 安装器/签名/自动更新**：仅 scripts/CI；需要签名证书与更新通道，证书未就绪只出方案/校验脚本。
- **F-064 ICC**：评估 `lcms2`，不改现有显示链；排在媒体线后。
- **F-016 CLIP**：只做独立 sidecar/向量索引原型，不接 UI、不改 schema。

## 合并顺序与禁止事项

1. 任务 0 QA 与 A 桌面基础可先并行；A 先合，供 F-061 恢复验收复用。
2. 任务 B（F-035）是唯一 schema/state 源编排负责人；任务 C 媒体线可并行，但不得改 sources schema/源 UI。
3. 任务 D 在 B/C API 稳定后合；D 独占 `app.rs`。任务 0 的手动验收不阻塞 B/C 核心开发，但必须在归档前完成。
4. 禁止：两个代理同时改 `app.rs`；第二份 sources 迁移；远端写操作；把 HEIC/RAW/视频未实测写成支持；用 UIA 模拟代替真实资源管理器拖放。

## 任务 0 执行结果（2026-10-29）

> 范围：只复核、归档与文档立项，不改功能代码、不新增实测。证据均来自 2026-10-29 已有目录 `.kanalib/r4_task0/`、`.kanalib/r4_e1/`（`.kanalib/` 已 gitignore，不提交）。没有真实证据的项一律不写通过；UIA 模拟拖放、离线占位库、去重组入口均不作为通过依据。

| 验收项 | 状态 | 证据 / 缺口 |
|---|---|---|
| F-036 真实 Windows 资源管理器拖放 | 待用户手动 | 无真实拖放 E2E；窗口内 UIA/消息模拟不算。手动清单 1–7 项（文件夹、来源内图片、来源外图片、取消、Unicode 日文/长路径、重复来源、自绘标题栏兼容）见 [pending/library-sources-formats.md](pending/library-sources-formats.md) 的 F-036。缺少环境：人工从资源管理器发起拖放。 |
| F-003 Ctrl 多选 2–4 张进入对比 | 未通过 | 去重组「对比此组」3 张对比通过（`.kanalib/r4_task0/screens/10k-compare-3up.png`、`10k-compare-zoomin.png`、`10k-compare-pan-left.png`、`10k-compare-esc-return.png`：并排 3 栏、同步缩放/平移、差异表、保留标记、Esc 返回），但不能当作多选入口；Ctrl 多选尝试 `.kanalib/r4_task0/screens/10k-multiselect-ctrl-attempt.png` 显示仅 1 张（`synthetic_00002_city_合成.jpg`）被选中。缺口：真实 Ctrl/Shift 选中 2–4 张后经「对比选中」进入对比未复现，待用户手动复核。 |
| F-005 10 万真实缩略图库分组滚动 | 环境不可用 | 分组菜单（修改/拍摄时间、来源、评级）、组头吸顶、折叠、连续滚动可用；600s 采样工作集约 145MB、私有约 190MB 无增长（`.kanalib/r4_task0/100k-scroll-memory.csv`、`screens/100k-*.png`、`uia-100k-grouped.txt`）。但 `.kanalib/r4_task0/logs/100k-group.log` 记录 `source inaccessible: C:\bench` 与 `thumb missing and could not rebuild`，网格只有占位符。缺少环境：可访问的 10 万真实缩略图库。 |
| F-030 真实 OneDrive/网络盘事件与断线恢复 | 环境不可用 | 只有临时目录的离线/恢复/目录重命名模拟（第四轮 D）；`.kanalib/` 下无 OneDrive/SMB/断线重放证据。缺少环境：真实 OneDrive 同步目录（占位/已水合文件）、SMB 共享与断线重连。 |
| F-012 10 万真实缩略图库 UI 键入延迟 | 环境不可用 | 仅有 `C:\bench` 离线的缺图库：`synthetic` visible 931ms、`sky` 657ms、`synthetic_0004` 修复后 360ms（`.kanalib/r4_e1/logs/search100k-limit512c.log`、`search100k-fixed.log`）。缺少环境：缩略图可访问的 10 万库；现有库上 `p95≤150ms` 仍未达成。 |
| F-061 大库备份进度 / VACUUM | 未通过 | 备份进度/取消与 VACUUM 维护入口未实现，`.kanalib/` 无大库备份采样或 VACUUM 记录；见 [pending/ops.md](pending/ops.md) 的 F-061。 |

**文档立项（同日）**

- F-035 由「远期」改为「实现中」（第五轮任务 B）；范围限定为 Source 抽象、Immich 只读、WebDAV 只读、跨源 SHA1 精确去重与只读边界，详见 [pending/library-sources-formats.md](pending/library-sources-formats.md)。
- F-038/F-039 由「候选」改为「实现中」（第五轮任务 A）；F-038 只做窗口位置/大小/最大化/屏幕恢复，F-039 只做单实例置前，托盘/开机启动仍不做，详见 [pending/ops.md](pending/ops.md)。
- F-046/F-047/F-015/F-025/F-068 保持「候选」，仅标记为「待任务 D」；未实现前不得写成已实现。
- `implemented.md` 的 F-040 阶段 1 行改为“部分实现条目”说明，避免与 registry「实现中」及脚本的状态互斥检查冲突。
- 文档自检 `bash scripts/check_backlog_docs.sh` 通过：registry 50 条无重复/丢失（已实现 30、候选 8、实现中 7、远期 5）；「已实现但无归档行」「implemented 有行但状态非已实现」「overview 缺口」「详情文件未覆盖」四项均为空。

**待用户手动清单**

1. F-036：按 [pending/library-sources-formats.md](pending/library-sources-formats.md) F-036 的 1–7 项逐一拖放（文件夹、来源内、来源外、取消、`D:\测试\日本語\写真\...` 与 >260 字符长路径、重复来源、自绘标题栏边缘）。
2. F-003：真实 Ctrl/Shift 选中 2–4 张后点「对比选中」，确认栏数与选中数一致、Esc 返回后选择与滚动保持、仅 1 张时提示不可对比。
3. F-030：若手头有 OneDrive 同步目录或网络共享，验证新增/修改/删除/目录重命名/离线重连，并确认监听批次结束后无过期去重组；没有该环境则保持「环境不可用」。
4. F-005/F-012：需要可访问的 10 万真实缩略图库后才能补测分组滚动与 UI 键入延迟；不能再用离线占位库替代。
5. F-061：大库备份进度/取消与 VACUUM 属未实现功能，需回到实现线，不是本轮验收可关闭项。

**未完成边界**

- 真实资源管理器拖放（F-036）、Ctrl 多选对比入口（F-003）仍待手动验收；F-005/F-012 的真实 10 万缩略图证据缺失；F-030 的真实 OneDrive/网络盘语义缺失；F-061 大库进度/VACUUM 未实现。
- 任务 0 不启动 F-035/F-040/F-064 实现，不改任何 Rust 代码，也不改 `docs/FFMPEG.md`（当时任务 C 在另一 worktree 修改）。

## 任务 A 执行结果（2026-10-30）

| 项 | 结果 |
|---|---|
| 分支提交 | `cd5e1a5 feat: 单实例置前与窗口状态恢复 (F-039/F-038)`，合并 `8842a78` |
| F-039 单实例 | 数据根规范化哈希 → Windows 命名 mutex/激活事件；第二实例 `AllowSetForegroundWindow` + `SetEvent` 后 0.11s 退出且不建窗口，首实例轮询置前；headless 在加锁前返回；不同数据根可共存 |
| F-038 窗口状态 | `.kanalib/window_state.json` 保存位置/大小/最大化/屏幕；500ms 节流 + 关闭/退出补写；启动按 display_id→窗口中心→主屏回退并夹取；10 个单测覆盖离屏/125% DPI/显示器回退 |
| 验证 | `cargo check --workspace`；`cargo test -p kanalib-ui` 46 项（基线 36+10）；`cargo test --bin kanalib` 6 项；实机窗口化恢复像素差 0、最大化重启 `IsZoomed=true`、双开仅一个主窗口 |
| 证据 | worktree `.kanalib/r5a/summary.txt`、`01-07*.png`、`window_state-root*.json`、`verify_r5a.ps1` |
| 未验证 | 真实多显示器目视、真实 125% DPI 目视（仅单测覆盖逻辑夹取）；托盘/开机启动按范围不做 |

## 任务 B 执行结果（2026-10-30）

| 项 | 结果 |
|---|---|
| 分支提交 | `8340f71 feat: 网络源只读与跨源精确去重 (F-035)`，合并 `18a0155` |
| Source 抽象 | 新增 `crates/kanalib-core/src/sources/{mod,local,immich,webdav,tests}.rs`；本地 roots 行为/清理语义不变；`Shared.sources` 与统一 folder 列表供加载/搜索/标签使用 |
| Immich 只读 | 版本探测、分页搜索、`updatedAfter` 增量、缩略图缓存；仅 `GET /api/server/*`、只读 `POST /api/search/metadata`、`GET /api/assets/{id}/thumbnail`；断线/密钥失效不阻塞本地 |
| WebDAV 只读 | PROPFIND Depth 0/1、ETag/Last-Modified、4 并发限流 + 退避、目录缓存；无 PUT/DELETE/MOVE/COPY，不下载 original |
| 跨源去重 | 按 SHA1 归组并复用现有评审状态；远端资产用 `immich://`/`webdav://` 虚拟路径，回收站/移动/删除三重拦截 |
| 凭据 | Windows 凭据管理器（keyring），Debug 脱敏，不落 DB/settings/日志；测试用内存后端 |
| 验证 | 分支 `cargo test -p kanalib-core` 162/15（基线 150+12）；合并后全量 177/16；测试均为回环 mock，覆盖分页/增量/断线/凭据/WebDAV 解析/限流/跨源归组 |
| 未验证 | 未联真实 Immich/WebDAV；WebDAV 无 SHA1 不参与精确归组且不下载 original；WebDAV 远端删除暂不清理旧资产行 |

## 任务 C 执行结果（2026-10-30）

| 项 | 结果 |
|---|---|
| 分支提交 | `40baa4f feat: 视频首帧与格式门 (F-040/F-033)`，合并 `d1df29a` |
| 数据模型 | `SCHEMA_VERSION` 9→10；`assets.media_kind TEXT NOT NULL DEFAULT 'image'`，`migrate()` 按 `PRAGMA table_info` 幂等补列并先备份，旧 v9 行读作 image；未知值回落 image |
| 视频 | 首帧走既有 ffmpeg 批处理池（512、120s、`-frames:v 1`）；ffprobe 取时长/宽高/编码（20s 超时、可关）；视频不进 pHash/打标队列/不标损坏；系统默认播放器只 `spawn` 不等待 |
| 格式门 | HEIC/HEIF、相机 RAW 明确不支持：不扫描/不入库/不解码，统一 `UNSUPPORTED_FORMAT_NOTE` 提示；PSD 维持 ffmpeg-only；未新增原生库 |
| 文档 | `docs/FFMPEG.md` 第五轮章节记录探测/首帧/系统播放/许可与 ffmpeg/ffprobe SHA256 |
| 验证 | 分支 `cargo test -p kanalib-core` 165/16（基线 150/15）；合并后全量 177/16；10k 图片冷导入 213.7s / 0 失败（同机基线 284.5s，无回退）；Unicode+空格视频样本首帧 512×384、时长 2.0s |
| 未验证 | 100k 未重跑（缺可访问大库）；HEIC/RAW 无真实样本/许可链，维持不支持；UI 入口由任务 D 接入 |

**A/B/C 合并后基线**：`cargo check --workspace` 通过；core 177 passed / 16 ignored、UI 46、bin 6；任务 D（UI 集成）进行中。

## 任务 D 执行结果（2026-10-30）

| 项 | 结果 |
|---|---|
| 分支提交 | `3d9bb40 feat: 网络源设置 UI (F-035)`、`582d9e2 feat: 视频与格式门 UI (F-040/F-033)`，合并 `3025605` |
| 网络源设置 | 设置页新增「网络来源」：Immich/WebDAV 添加（名称 + base URL 校验）、只读徽标、上次同步/凭据状态、测试连接、立即同步、设置/清除凭据、移除确认；凭据只存 keyring 且不回显，测试/同步走后台 executor，不阻塞 GPUI 线程 |
| 视频/格式 UI | 网格视频瓦片首帧 + 「视频 · 时长」角标；查看器固定首帧、隐藏「原图」、显示时长/分辨率/编码并提供系统播放入口；详情页视频信息行；远端虚拟路径在查看器/右键/详情/去重组/侧栏统一只读拦截；HEIC/RAW 明确「不支持」 |
| 验证 | `cargo check --workspace`；合并后 core 177/16、UI **52**（46+3+3）、bin 6；隔离数据根实机：视频瓦片首帧与 `视频 · 00:02`（`.kanalib/r5d/grid2.png`）、网络来源区块（`settings3.png`）、添加 WebDAV 对话框（`dialog.png`） |
| 未验证 | 添加/测试/同步的完整 UI 提交链路未用真实键入跑通（GPUI TextInput 不接受 UIA `ValuePattern`，只验证到对话框）；未联真实 Immich/WebDAV；远端资产在网格/去重组的只读禁用为单测覆盖；小项包（F-015/F-025/F-046/F-047/F-068）未做 |

**第五轮合并后基线**：`cargo check --workspace` 通过；core 177 passed / 16 ignored、UI 52、bin 6。A/B/C/D 全部合入；F-035/F-038/F-039/F-040 归档为已实现（边界见 [implemented.md](implemented.md)），F-033 保留在未完成项，等待 HEIC/RAW 样本与许可链。

## 第六轮预览（不在本轮）

- F-035 WebDAV/跨源相似（若本轮只完成 Immich/WebDAV 精确去重，则剩余项顺延）。
- F-066 视频/小说、F-016 CLIP、F-064 ICC、F-065 完整签名/更新。
- F-038/F-039 的托盘/开机启动扩展、F-067 撤销历史、F-048 相似扫描分桶。
