# 第二轮任务包（2026-10-27）

> 上级索引：[../FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md) · 文档地图：[README](./README.md) · 第一轮：[parallel-plan.md](parallel-plan.md)

> 第一轮状态：任务 0/A/B/C/E/D 已全部落地并合入（`5fb45f4` 依赖骨架、`5672b97` 备份、`cf6943c` 日志、`fe18b60` 扫描/排除/监听、`f11eec6` 重命名修复、`6b1b88b` 监听恢复、`dbd6f35` 便携校验、`5226eee` UI 集成）。第二轮核心线可以并行。
>
> 本轮目标：打标质量/队列、搜索性能、媒体格式与 ffmpeg 阶段 2。浏览增强、相册/集合、网络源、截屏/小交互留第三轮。

## 任务 0：第一轮收尾（已完成）

- **结果**：D 已合入（`5226eee`）；F-031/F-062 已归档，F-030/F-032/F-061 保留 pending 并记录剩余边界；F-065 便携校验部分完成、MSI/签名/自动更新仍远期；`cargo test --workspace` core 61 + UI 8 全绿。详见 [parallel-plan.md](parallel-plan.md) 的「第一轮执行结果」。

- **文件**：`crates/kanalib-ui/src/lib.rs`；文档 `docs/backlog/registry.md`、`implemented.md`、`pending/*`。
- **内容**：
  - 完成 D：备份/诊断/来源状态/排除规则 UI，隔离数据根截图与 UIA 验证。
  - 把 F-030/031/032、F-061、F-062、F-065（便携校验部分）从 pending 详情文件移到 implemented，更新 registry 状态；F-065 的 MSI/签名/自动更新仍保留远期。
  - 关闭第一轮任务表中的遗留项，确认 `cargo test --workspace` 全绿。
- **验收**：D 的 UI 截图/UIA 通过；文档状态与 git log 一致；第二轮核心线可以只依赖已合入 API。
- **边界**：不做第二轮功能；不改 core/schema/sidecar。

## Wave 1（可与 D 收尾并行；均不碰 `lib.rs`）

### 任务 A：F-021/024/023 打标质量与队列（核心线，Round 2 唯一 schema owner）

- **文件**：`db.rs`（schema v8 迁移；当前为 v7）、`model.rs`（候选/阈值/队列状态）、`state.rs`（打标队列与重打标）、`sidecar/tagger/server.py`、测试；不碰 `lib.rs`。
- **内容**：
  - F-021：general/character/copyright/style/meta/rating 分类阈值 + 单标签覆盖；sidecar 返回阈值下 top-K 候选；存 `asset_scores`/候选表；低置信度审核 API；改阈值不重跑模型。
  - F-024：按选中/当前视图重打标；保留 manual tags；阈值重算优先用候选，缺失才调模型。
  - F-023：`jobs` 优先级、暂停/继续/取消、当前批错误、吞吐/ETA；GPU 不可用默认暂停。
  - 唯一 schema 迁移负责人；迁移前自动备份；sidecar 协议保持向后兼容。
- **不做**：阈值/队列 UI（任务 D2）、网络源、媒体格式。
- **验收**：阈值变更后标签/计数正确且不重跑模型；低置信度可审核/忽略；重打标只处理选中范围且 manual 不丢；暂停/取消不丢已完成结果；单测 + 临时 DB 迁移测试。
- **冲突**：`state.rs` 打标路径由本任务独占；任务 C 只改 `ensure_thumb`/媒体，不改 tag 队列；任务 B 等本任务迁移完成后再加搜索索引。

### 任务 C：F-033 + F-040 阶段 2 媒体管线（核心线）

- **文件**：`model.rs`（扩展名/`media_kind`）、`media.rs`、`ffmpeg.rs`、`state.rs` 的 `ensure_thumb`/pHash 集成点、`sidecar/tagger/repair.py`（阶段 2）；不碰 `lib.rs`。
- **内容**：
  - F-033：先接 `image` 的 tiff/avif/ico/qoi/exr；HEIC/RAW/PSD 按许可与真实样本评估 `libheif-rs`/`rawloader`/`psd`，不承诺未实测格式。
  - F-040 阶段 2：pHash 与损坏修复迁移评估；动画首帧；保留 ffmpeg 批处理池、`v3` 缓存、GPL 不随包；更新 `docs/FFMPEG.md`。
  - 不新增 DB schema；不引入 `ffmpeg-sidecar`（现有批处理 CLI 已达标，libav 仅评估）。
- **不做**：格式筛选/徽标 UI（任务 D2）、视频/小说。
- **验收**：新格式样本可扫描/缩略图/尺寸/哈希；方向、alpha、损坏文件不回归；10k/100k 缩略图指标不劣于当前基线（10k 52.2s、100k 738.7s、总 42.3min）；许可/原生依赖记录；单测 + 样本清单。
- **冲突**：与任务 A 共享 `state.rs`，只允许改缩略图/媒体函数；不得改 tag 队列或 DB 迁移；与任务 B 无 schema 交集。

### 任务 D1：第一轮 D 收尾（UI 车道）

- 见任务 0；必须早于 D2 合入。

## Wave 2（A 的 schema 合入后）

### 任务 B：F-012 搜索性能核心（DB/查询线）

- **文件**：`db.rs` 搜索查询/索引、必要时 `state.rs` 搜索入口；不碰 `lib.rs`。
- **内容**：保持 FTS5，不引入 Tantivy；为评级/星级/收藏/来源/时间加索引；筛选/排序下推 SQL；keyset/分页；短词回退策略；提供 10 万基准脚本/指标。
- **不做**：UI 防抖/懒加载/可见索引缓存（D2）。
- **验收**：10 万查询 p50/p95 较 284ms 明显下降，目标 p95 <150ms；结果与现有内存过滤逐条一致；无 schema 冲突（等待 A 的 v8）。
- **冲突**：`db.rs` 在 A 之后合；不要改 UI。

### 任务 D2：第二轮 UI 集成（唯一 UI 车道，D1 后）

- **文件**：只改 `crates/kanalib-ui/src/lib.rs`。
- **顺序**：
  1. F-021/023/024：阈值编辑器、低置信度审核、队列暂停/优先级/ETA/错误、选中重打标。
  2. F-033：格式能力/失败提示（不做解码 UI）。
  3. F-012：搜索输入防抖、`dup:true` 时才构造去重路径、可见索引缓存；与任务 B 的查询结果对齐。
- **不做**：F-003/004/005/045 浏览、F-013/14 相册、F-036/37 拖拽/移除；这些留第三轮。
- **验收**：隔离数据根截图/UIA；`cargo test --workspace`；搜索 p95 与无回归；不混入其它 UI。

## 可选研究线（不占核心）

- **F-035 Immich/WebDAV 方案/骨架**：新增 `sources/` 模块与 `reqwest`/`reqwest_dav`/`keyring` 最小客户端，不接 UI/不迁移 schema；交付接口、许可、凭据与离线策略。只有核心线之外还有人力时启动。
- **F-016 CLIP 原型**：独立 sidecar/向量索引，不改现有 tagger；仅做可行性/许可/基准。

## 合并顺序与禁止事项

1. 任务 0（D 收尾）先合；Wave 1 的 A/C 可在 D 收尾期间并行，但都不得改 `lib.rs`。
2. A 先合 schema v8；B 在 A 之后合；C 只改媒体函数并 rebase。
3. D2 在 D1 与 A/B/C API 稳定后合；D2 独占 UI。
4. 本轮禁止：改 `db.rs` schema 的第二个任务、改 sidecar 协议的第二个人、多个代理同时改 `lib.rs`、启动 F-003/004/005/013/014/035 的实现。
5. 交付附：改动文件、接口签名、迁移/备份、测试命令/结果、未完成边界；UI 附截图。

## 第二轮执行结果（2026-10-27）

| 任务 | 状态 | 提交 |
|---|---|---|
| A 打标质量与队列核心（F-021/023/024，schema v8） | 已合入 | `8eb97b2` |
| C 媒体管线（F-033 + F-040 阶段 2） | 已合入 | `74d65be` |
| B 搜索性能核心（F-012） | 已合入 | `f5f8ed4` |
| D2a 打标 UI 集成（F-021/023/024） | 已合入 | `38d4d1c` |
| D2b 低置信度审核 / 搜索防抖 / 格式提示 | 已合入 | `7f75bbc` |

- **D2b 交付**：只改 `crates/kanalib-ui/src/lib.rs`。低置信度审核接到设置页（读 `pending_tag_reviews`，按资产分组显示候选标签、类别、分数与阈值，逐条或整张接受/忽略）；搜索输入 220ms 防抖，只在 `dup:true` 时构造去重路径集，查询走任务 B 的 `search_page`（`MAX_SEARCH_PAGE`），结果超分页上限时回退到不截断的内存过滤；检查器加格式友好名与解码能力提示（AVIF 需要 FFmpeg），`friendly_media_error` 补 AVIF/FFmpeg/不支持编码等提示。
- **验证**：`cargo test --workspace` core 82（7 项按需忽略）+ UI 15 + bin 3 全绿；`cargo check --workspace` 无警告。隔离数据根 `.kanalib/r2_ui_root`（sample_library，15 资产）实测：AVIF 资产检查器显示「格式 · AVIF · 需要 FFmpeg 解码」、PNG 显示「内置解码」；待审候选 5 条，接受 `#chibi` 后提示「接受候选 #chibi · 处理 1 条」，DB 中 `asset_scores.review_state='accepted'` 且标签写入；搜索 `girl`→1 张、`tag:chibi`→仅 anime_chibi.png、`dup:true`→0 张、清空→15 张。截图 `.kanalib/r2b_*.png`。所有 UI 冒烟使用隔离数据根，未触碰真实图库。
- **基准**：release、`KANALIB_BENCH_ROWS=100000` 重新生成的 10 万库，名称排序 p95 133ms（达标），命中全库的非名称排序 276–304ms（未达标）。明细见 [pending/search-organization.md](pending/search-organization.md)。
- **未完成边界**：F-012 非名称排序 p95 仍超 150ms（临时 B 树排序）；F-033 还差 HEIC/RAW/PSD 与动画播放入口；F-040 阶段 2 还差 pHash/损坏修复迁移、ffprobe 与许可落盘；F-012/F-033/F-040 保留在 pending。浏览增强、相册/集合、网络源仍在第三轮。
- **禁止回退**：schema 仍为 v8（A 是唯一 schema owner）；sidecar 协议保持向后兼容；UI 仍只有一条车道；标签别名由调用方 `SearchQuery::resolve_tag_aliases` 解析（SQL 侧按标签名精确匹配，不解析别名），改搜索路径时不能漏掉这一步。

## 第三轮预览（不在本轮）

- 浏览增强：F-003/004/005/045。
- 组织：F-013/014（saved_searches/collections、侧栏/搜索 UI）。
- 来源/小交互：F-035（如未作为研究线启动）、F-036/037。
- 进度与扩展：F-063、F-016/026/064/066。
