# Kanalib 调研与实施方案

> 目标：Rust + GPUI（Ely 组件）的本地图片/媒体管理器，首版接入本地目录、Immich（只读）和 OpenList/WebDAV 网盘，使用 PixAI Tagger v1.0 自动打标与分类。Windows 优先。

## 1. 结论

1. **模型可用，且对原创动漫图效果很好。** 已在 RTX 4060 Laptop / CPU 环境实际加载 `pixai-tagger-v1.0` 并完成推理；两张本地动漫图的标签与画面高度一致（`1girl`、`wings`、`halo`、`cat_ears`、`heart_hands` 等均 >0.98）。已知角色/作品的识别仍需用本库样本抽检。
2. **它是多标签分类器，不是单标签分类器。** 输出 6 个类别、30,877 个标签；适合做标签面（facets）、智能集合和筛选，不应强行把每张图塞进一个文件夹。
3. **CPU 推理不可用于批量，GPU 可用。** CPU 实测 **21.6–24.3 秒/张**；RTX 4060 Laptop 8GB 上 FP32 约 1.1 张/秒，**FP16 权重约 3.9 张/秒（258ms/张，峰值显存 1.6GB）**，标签与 FP32 基本一致。按 3.9 张/秒，10 万张约 7.2 小时机时。
4. **首版推理路线锁定：Python sidecar + PyTorch CUDA，程序内置独立 Python 运行环境。** 不评估 ONNX / TensorRT；推理后端不再作为可选项讨论。
5. **UI 基础复用 Ely GPUI Components。** `floral-notepaper` vendor 的是 Ely `0.1.1`，但 pin 的 `gpui-pre` 是 `0.3.8`；上游当前已是 `0.1.1 / gpui-pre 0.3.9`，需要升级并重新验证 Windows 中文字体/IME。
6. **首版只读。** 不修改 Immich、不写回网盘、不改原图；自动标签只进本地 SQLite。标签写回、XMP 导出放到后续。

## 2. 已确认的模型事实

来源：[HuggingFace 模型卡](https://huggingface.co/pixai-labs/pixai-tagger-v1.0) · [ModelScope 镜像](https://www.modelscope.cn/models/pixai-labs/pixai-tagger-v1.0)（国内下载）

| 项 | 值 |
|---|---|
| 架构 | `ViTDetCls`，32 层 ViTDet，1024 维，16 头，patch 14 |
| 输入 | 1008×1008，保持比例缩放后黑边填充，RGB，[-1,1] 归一化 |
| 参数量 | 486.3M，FP32，1.945 GB |
| 输出 | 30,877 个 sigmoid logits |
| 类别划分 | general 15,043 / character 8,308 / copyright 2,460 / style 4,917 / meta 145 / rating 4 |
| 默认阈值 | general 0.17 / character 0.27 / copyright 0.24 / style 0.15 / meta 0.17 / rating 0.41 |
| 许可证 | Apache-2.0 |
| 仓库内容 | 仅 safetensors + 自定义 `tagger_pipeline.py`，**没有 ONNX** |
| 模型卡速度 | H100，batch 16，48.3 张/秒（不含预处理） |
| 训练域 | 动漫插画；2026-05 截止，之后的角色/作品无法识别 |

模型文件校验（已对齐 ModelScope 的 SHA256）：

```text
model.safetensors          f29e475205cbcbc25b52a075840c7809d6215be15f45375113ba09c49bf90292
config.json                f8a19b38661c37dc5fd519f2137be70f6f897972021b4ebc17b7d35a0e27b9dd
preprocessor_config.json   20451575e627950993de7b00641d5b263eceb3e7436e4ad518fea2411b60e49d
tagger_pipeline.py         3431279fdfff234f62664999fc1fe6bb20f1e8756f7435d0e165ebd4456f7fcd
```

### 2.1 实测结果

下表为 CPU 标签结果（Windows 11，16GB RAM，torch 2.9.1 CPU，FP32）；GPU 精度/吞吐见下节。

| 图片 | 关键标签（置信度） | 类别结果 |
|---|---|---|
| `fixtures/anime_girl.png`（天使少女） | `1girl .998`, `wings .993`, `halo .990`, `solo .988`, `white_dress .937`, `head_wreath .902`, `angel_wings .846` | general 50 个；copyright `original .853`；meta `ai-generated .253`；rating `g .618` |
| `fixtures/anime_chibi.png`（猫耳 Q 版） | `animal_ears .997`, `cat_ears .989`, `heart .986`, `ahoge .984`, `heart_hands .983`, `cat_tail .976`, `white_hair .953` | general 56 个；copyright `indie_virtual_youtuber .388`；rating `g .982` |
| 完整 AI 绘画截图（带 UI） | `1girl .996`, `dress .987`, `wings .978`, `halo .971`, `solo .962`, `angel_wings .928` | general 51 个；copyright `original .905`；rating `g .673`；UI 边框未破坏主体识别 |
| Floral 主窗口截图（纯 UI） | `text_focus .982`, `no_humans .965`, `text-only_page .920`, `book .532`, `fake_screenshot .351` | 合理识别为 UI/文本截图 |
| Windows 抽象壁纸 | `no_humans .920`, `blue_theme .900`, `blue_background .668`, `realistic .395`, `gradient_background .338` | 非动漫仍能给出构图/颜色/写实类标签，但语义有限 |
| 真实风景拼图（海滩/樱花/雪山） | `sky .884`, `cloud .801`, `outdoors .766`, `flower .738`, `mountain .689`, `no_humans .666` | meta `photo_(medium) .769`、`off-topic .630`、`lowres 1.000`；rating `g .893` |

结论：**动漫图可用；带 UI 的截图也能识别主体；截图/实景图不会乱飞，但只适合低层标签，且 meta 会给出 `off-topic`/`photo_(medium)` 提示，不应把 rating 当作内容安全判定。**

需要注意：本次两张动漫样本都是**原创角色**，`character` 类别均为空、`copyright` 只给出 `original`。模型有 8,308 个角色标签，但“已知角色/作品”识别尚未用本库样本验证；阶段 0 需用 20–50 张已知角色图抽检 `character/copyright`，再决定是否把这两类用于智能集合。

GPU 精度对比（RTX 4060 Laptop 8GB，torch 2.9.1+cu130，4 张图）：

| 模式 | 吞吐 | 单张 | 峰值显存 | 标签一致性 |
|---|---:|---:|---:|---|
| FP32 | 1.09–1.18 张/秒 | 848–916ms | 2.1–2.9GB | 基准 |
| FP16 autocast | 3.24–3.44 张/秒 | 291–308ms | 2.9–3.5GB | 基本一致 |
| FP16 权重 (`model.half()`) | **3.84–3.89 张/秒** | **257–261ms** | **1.1–1.6GB** | 仅 0.001 级差异 |

建议 sidecar 默认使用 **FP16 权重 + batch 1**；batch 4 在 sidecar 端到端实测无吞吐收益（见 7.3）。首次启动做一次 FP32/FP16 标签一致性抽检。

### 2.2 预处理与分类语义

- 预处理必须严格复刻官方：RGB；非 RGB 先合成白底；等比缩放到 1008 内；居中补 0；`(x-0.5)/0.5`。
- `rating` 的 4 个标签是互斥语义，但模型仍是多标签输出；取最高分作为展示 rating，并保留全部四个分数。
- 自动分类建议拆成三层：
  1. **rating**：`g/s/q/e`，只做筛选，不做安全承诺。
  2. **copyright + character**：作品/角色分类，对应智能集合。
  3. **general + style + meta**：细标签，对应标签面与搜索。
- 低置信度图片进入 `untagged/low-confidence` 队列，不强行归类。
- 首版不把自动标签写回 Immich/文件；人工标签与自动标签分表/分 origin 存储。

## 3. UI 基础：Ely GPUI Components

- 上游：[ELY-Open-Source/Ely-GPUI-Components](https://github.com/ELY-Open-Source/Ely-GPUI-Components)，当前 HEAD `f3cfbad`（2026-10-23），版本 `0.1.1`，依赖 `gpui-pre =0.3.9`。
- 本地参考：`D:\Code\floral-notepaper\vendor\ely-gpui-components`（0.1.1，`gpui-pre 0.3.8`）。
- `floral-notepaper` 还提供了可直接复用的 Windows 外壳经验：单实例、托盘、自动启动、主题、HarmonyOS Sans SC 字体注册、Win32 AppUserModelID，以及 `vendor/gpui-pre-windows` 的 GDI 提示字体补丁。
- 可复用组件：
  - `files/explorer`、`files/grid`：文件浏览与图标网格模式；
  - `media/thumbnail`、`media/viewer`：缩略图与缩放/旋转查看器；
  - `files/duplicates`、`files/search`：去重与搜索界面；
  - `lists/*`、`data_display/*`、`forms/*`、`overlays/*`：列表、对话框、表单。
- **注意**：Ely 的 `Gallery` 会一次性渲染所有 tile，不适合 10 万级资产；图片网格必须自己用 `uniform_list` + 固定高度行 + `img(PathBuf)` 实现。
- **风险**：GPUI 无内置文本输入，Ely 的输入组件有 composition 测试，但 Windows 原生 IME 仍需实机验证；2026 年的第三方调研仍提到 GPUI 的 IME 崩溃问题。首版必须做“中文输入 + 候选窗 + 焦点切换”冒烟测试。

## 4. 总体架构

```text
 GPUI 前台线程 (Kanalib UI)
   │  view model / commands
   ├── tokio 网络运行时 (Immich / WebDAV / 下载)
   ├── SQLite 单写线程 + WAL 读连接
   ├── 缩略图/哈希工作池 (rayon)
   └── Tagger 队列 ──stdio JSONL──> Python sidecar (PyTorch CUDA)
                                      └── PixAI Tagger v1.0
```

### 4.1 进程与线程

- GPUI 前台只做渲染与交互；所有 IO、解码、数据库写入、推理都在后台。
- SQLite 使用一个写线程 + 多个 WAL 读连接；写操作排队，避免 UI 卡顿。
- 缩略图与哈希用固定线程池；远程源单独限流。
- Python sidecar 由 Rust 启动，stdin/stdout 走换行分隔 JSON；stderr 收日志。模型只加载一次，按 batch 处理；sidecar 崩溃后指数退避重启。
- 任务表持久化，应用重启后未完成任务继续；所有步骤幂等。

### 4.2 建议 crate 结构

```text
Cargo.toml                  workspace
crates/
  kanalib-core/             领域模型、ID、Source/Tagger trait、查询 AST
  kanalib-store/            SQLite schema、迁移、仓库、FTS5
  kanalib-media/            解码、EXIF、缩略图、SHA1/BLAKE3/pHash
  kanalib-sources/          Local / Immich / WebDAV
  kanalib-tagger/           sidecar 客户端、批处理、队列协议
  kanalib-ui/               GPUI + Ely 视图、虚拟网格、查看器
apps/kanalib/               组合根、窗口、单实例、托盘
sidecar/tagger/             Python 推理服务（uv 管理）
vendor/ely-gpui-components/ Ely 上游源码（固定 commit）
vendor/gpui-pre-windows/    Windows 字体补丁（按需）
tools/model_probe/          本次可行性验证脚本（保留作为回归基准）
docs/
```

### 4.3 关键 trait

```rust
trait Source {
    fn capabilities(&self) -> SourceCapabilities;
    async fn probe(&self) -> Result<SourceInfo>;
    async fn scan(&self, cursor: Option<SyncCursor>) -> Result<ScanPage>;
    async fn fetch_preview(&self, item: &SourceItem) -> Result<PathBuf>;
    async fn fetch_thumbnail(&self, item: &SourceItem) -> Result<PathBuf>;
}

trait Tagger {
    fn info(&self) -> TaggerInfo;
    async fn tag_batch(&mut self, paths: &[PathBuf]) -> Result<Vec<TagVector>>;
    async fn cancel(&mut self, ids: &[AssetId]) -> Result<()>;
}
```

`SourceItem` 必须从第一天就带 `media_kind`（image/video/document）、`mime`、尺寸、时长/页数、来源 checksum 与元数据 JSON，为后续视频/小说留位，但 v1 UI 只渲染图片。

## 5. 数据模型

```text
sources            id, kind(local/immich/webdav), name, config_json, read_only, last_sync_at, sync_cursor
assets             id, source_id, locator, media_kind, mime, name, bytes, width, height, duration_ms,
                   page_count, captured_at, modified_at, sha1, blake3, phash, rating, favorite,
                   metadata_json, thumb_state, tag_state, tag_attempts, deleted_at
tags               id, name, category, vocab_index, threshold_override, parent_id, color
asset_tags         asset_id, tag_id, confidence, origin(auto/manual/import), created_at
asset_scores       asset_id, tag_id, logit_f16        -- 只存候选（如 p>0.01 / top100）
collections        id, source_id, locator, name, kind
collection_assets  collection_id, asset_id
jobs               id, kind, asset_id, priority, state, attempts, last_error, run_after
saved_searches     id, name, query_json
settings           key, value
duplicate_reviews  group_key(sha1), state(pending/ignored/done), keeper_path, member_count,
                   signature, reviewed_at
```

- `jobs` 目前承载 `tag` 队列：`pending/running/done/error`、`attempts`、`last_error`，按 `(kind, asset_id)` 唯一；启动时把 `running` 恢复为 `pending` 再继续。
- `tags.vocab_index` 必须保留模型输出顺序；类别范围来自 `config.tags_split`。
- 只存过阈值标签 + 候选中 top100 的 f16 logit，避免每图 30,877 个 float 占 12GB；阈值调整时先重算已存候选，缺失的再重跑。
- 人工标签不覆盖自动标签；查询时可选择只看 manual。
- FTS5 使用 trigram tokenizer，支持中文文件名子串；标签走 `tags` 表前缀/精确匹配。
- 分页用 keyset（排序键 + id），不用大偏移量 OFFSET。

## 6. 索引、缩略图与去重

### 6.1 管道

```text
扫描/增量同步 -> assets
  -> 计算 SHA1/BLAKE3/phash（本地）
  -> 生成 256 / 1024 两档缩略图（缓存按内容哈希命名）
  -> 远程资产按需下载 preview
  -> tagger 队列 -> asset_tags/asset_scores
  -> FTS 索引
```

- 本地：`notify` 文件监听 + 手动重扫；移动文件用 size+mtime+sha1 尝试识别，避免重建。
- Immich：首次分页全量；之后用 `updatedAfter` 增量；定期对账删除/回收站。
- WebDAV：`PROPFIND Depth:1` 递归，ETag/Last-Modified 跳过未变文件；并发 4-8；大文件设上限。
- 打标任务持久化在 `jobs` 表：扫描导入后按 `tag` 入队，4 张一批认领/完成/失败；应用重启后自动把中断的 `running` 恢复为 `pending`，再交给 sidecar 继续。
- 缩略图缓存默认预算 10GB，LRU 淘汰；GPUI 侧再实现解码内存 LRU（约 512MB），只保留可视区域 ±1 屏。

### 6.2 去重

- 精确：SHA1（对齐 Immich 的 base64 SHA1）分组；已实现侧栏“重复项”工作区：组列表、比较卡片、差异面板、未审/已忽略/已保留状态、按规则预选、单组/批量移入系统回收站。
- 近似：64-bit pHash 汉明距离分组，后台按需扫描；阈值预设 6/10/14（约 91%/85%/79% 相似），单组评审，不提供批量删除；支持“不是重复 / 相关变体 / 全部保留 / 逐组移入回收站”。阈值持久化在 `settings.dedup.similar_threshold`。
- 保护路径：设置页可添加/移除文件夹，持久化在 `settings.dedup.protected_paths`；保护路径下的文件优先作为保留项，且永远不会被移入回收站。批量处理会跳过含保护副本的组，单组操作直接禁用/拒绝。
- **绝不自动删除**；本地删除只调用系统回收站（仅允许固定磁盘），执行前列出组数、张数、保留项和可释放空间；只读源以后只允许忽略/保留。
- 评审状态写入 `duplicate_reviews`：`member_count` + `signature` 用于在成员变化时重新打开已保留组；忽略/相关变体状态跨扫描保留。相似组按成员路径签名生成 `sim:<signature>` 键，避免阈值变化后误用旧状态。
- 主资产按“保护路径/来源 → 标签 → 修改时间 → 路径”推荐；相似组优先“分辨率 → 文件大小 → 标签 → 时间 → 路径”，并在界面显示推荐理由。

## 7. 推理 sidecar

### 7.1 运行方式

- `sidecar/tagger` 开发期由 `uv` 管理；发布运行时使用 `runtime/python` 独立 CPython（见 15 节）。
- 依赖安装走国内镜像：Tsinghua PyPI + SJTU/Alibaba `pytorch-wheels/cu128`（或 cu130，按驱动/轮子匹配）；模型从 ModelScope 下载并校验 SHA256。
- sidecar 一次加载模型；preprocess 使用官方 `tagger_pipeline.py` 的逻辑，避免 Rust 侧重复实现导致数值偏差。
- 设备选择：CUDA 可用则 CUDA；否则 CPU 并显式警告“批量打标不可用”。batch size 从 1 起步；端到端瓶颈在解码/预处理，暂不扩大 batch。
- **默认 FP16 权重 + batch 1**（权重推理实测 3.9 张/秒、峰值 1.6GB；sidecar 端到端约 3.1 张/秒）；保留 FP32 作为首次启动抽检与回退。若 FP16 与原模型标签差异超过约定阈值，自动回退 FP32。

### 7.2 协议

```json
{"id":1,"cmd":"load","model":"D:/.../pixai-tagger-v1.0","device":"cuda","batch_size":1}
{"id":2,"cmd":"tag","items":[{"asset_id":"...","path":"..."}],"thresholds":{"general":0.17}}
{"id":2,"ok":true,"items":[{"asset_id":"...","probs":{"1girl":0.998},"top":[{"tag":"1girl","cat":"general","p":0.998}]}]}
{"id":3,"cmd":"shutdown"}
```

- 只传本地缓存路径，不传远程 URL/密钥；sidecar 不接触 Immich/WebDAV 凭据。
- Rust 侧负责队列、重试、去重、完成状态；sidecar 只负责推理。
- 生产打包使用 `runtime/python` 独立 CPython 运行时；首版不引入 PyInstaller。

### 7.3 性能预算

- 实测 CPU：**21.6–24.3 s/张**（batch=1，FP32，16 线程）。
- 实测 GPU（RTX 4060 Laptop 8GB，torch 2.9.1+cu130）：
  - FP32：1.09–1.18 张/秒，峰值 2.1–2.9GB；
  - FP16 autocast：3.24–3.44 张/秒，峰值 2.9–3.5GB；
  - FP16 权重：**3.84–3.89 张/秒，峰值 1.1–1.6GB**，标签与 FP32 仅 0.001 级差异。
  - sidecar 端到端（14 张样本，`dist/models` FP16）：batch 1 325ms/张（3.08 张/秒）；batch 4 321ms/张（3.12 张/秒）。batch 4 无实质收益，默认用 batch 1；按 3.1 张/秒，10 万张约 9 小时机时。
- 模型加载 3–6 秒（本机 dist FP16 冷启动 5.75 秒）；sidecar 常驻，不重复加载。
- 说明：早期 GPU 验证曾借用本机 ComfyUI 的 Python 环境；产品链路已改为独立 `runtime/python`，不依赖 ComfyUI。

## 8. 界面

### 8.1 主界面

```text
┌ 侧栏 ─────────┬ 工具栏/搜索 ────────────────────────┬ 检查器 ──────┐
│ 来源            │ [搜索/筛选 chips]  排序  缩放  选择  │ 1024 预览     │
│ 本地目录        ├─────────────────────────────────────┤ 标签(分类/置信)│
│ Immich          │  虚拟化图片网格（等高行/自适应列）    │ 元数据/来源    │
│ WebDAV/OpenList │                                     │ 操作/复制 prompt│
│ 智能集合        │                                     │              │
│ 重复项 / 未打标  │                                     │              │
└────────────────┴─────────────────────────────────────┴──────────────┘
状态栏：资产数 / 索引进度 / 打标队列 / GPU / 缓存
```

- 网格：`uniform_list`，每行固定高度、按宽高比计算列内宽度；支持键盘方向键、Home/End、Shift 多选、Ctrl+A。
- 查看器：复用 Ely `ImageViewer`（缩放、平移、旋转）；大图按需从 1024 缩略图切换到原图。
- 搜索语法：`tag:1girl`、`-tag:rating:q`、`rating:s`、`character:...`、`copyright:...`、`is:untagged`、`source:immich`、`dup:true`、自由文本。
- 批量动作：加/删标签、复制 Danbooru 风格 prompt、导出 CSV/JSON、加入智能集合。
- 标签管理器：类别阈值、标签屏蔽/别名、低置信度审核。
- 源设置：本地路径、Immich URL + API key（只读）、WebDAV URL + 凭据；测试连接、立即同步、排除规则。

### 8.2 中文与字体

- 复用 `floral-notepaper` 的 HarmonyOS Sans SC 注册与 `gpui-pre-windows` GDI 提示补丁；升级到 `gpui-pre 0.3.9` 后重新截图对比。
- 首版必测：微软拼音输入中文、候选窗位置、失焦/切窗、Ctrl+F 搜索框、标签自动补全。
- 若 IME 仍不可用：保留标签选择器 + 粘贴搜索作为降级路径，并把它作为 GPUI 上游 issue 处理。

## 9. 来源接入

### 9.1 Immich（只读）

- 认证：`x-api-key`；密钥存 Windows Credential Manager（`keyring`），不写配置文件。
- 探测：`GET /api/server/version`、`/api/server/about`；按版本选择兼容路径。
- 列表：`POST /api/search/metadata`，`page/size` 分页，`withExif=true`，`type=IMAGE`；读取 `assets.items/total/nextPage`。
- 缩略图：`GET /api/assets/{id}/thumbnail?size=thumbnail|preview`。
- 增量：`updatedAfter`；定期对账删除/回收站；不下载 original。
- 校验和：Immich `checksum` 是 base64 SHA1，本地也计算 SHA1 以做跨源精确去重。
- 相册/人物/标签只作为集合与筛选来源，不写回。

### 9.2 WebDAV / OpenList

- 只做标准 WebDAV：OpenList/AList 负责夸克等网盘的登录与风控，Kanalib 不接触网盘私有 API。
- `PROPFIND Depth:1` 递归，解析 `getetag/getlastmodified/getcontentlength`；并发限流；缓存目录列表。
- 图片预览按需下载并生成缩略图；超大文件跳过缩略图/打标并标记 `skipped`。
- 凭据同样进 Credential Manager；URL 只允许 http(s)，禁止路径穿越。

## 10. 阶段与验收

> 不写工时；每阶段以“可运行构建 + 指标记录”收口。

### 阶段 0：可行性（模型/运行时已收口；UI 与网络源待验）
- [x] 模型完整下载与 SHA256 校验。
- [x] CPU 推理跑通，动漫/截图/非动漫三类样本输出合理。
- [x] GPU 推理跑通：4060 上 FP32/autocast/FP16 三档吞吐与显存已测，FP16 标签一致。
- [x] 独立运行时安装 CUDA torch（`runtime/python`，不复用 ComfyUI 环境）。
- [x] 在 sidecar 内复测 FP16 端到端吞吐（batch 1 3.08 张/秒；batch 4 3.12 张/秒，无实质收益，默认 batch 1）。
- [x] 用 27 张已知角色/作品图抽检 `character/copyright`（本地 9 + 网络 18；网络身份 18/18，本地身份 6/9、作品 8/9）。
- [ ] Ely 上游 `0.1.1/0.3.9` 在 Windows 编译；中文 IME 冒烟；10k 缩略图滚动测试。当前已在 `0.1.1/0.3.8` 编译运行；10k 加载/滚动已完成（见 15 节），IME 与 0.3.9 仍待验。
- [ ] Immich/WebDAV 连接冒烟。2026-10-24 决定暂缓，先完成本地 MVP。
- 退出条件：GPU 吞吐 ≥2 张/秒 ✅；本地源可读 ✅；IME 可用或有降级方案（待验）；Immich/WebDAV 首图（暂缓）。

### 阶段 1：本地 MVP
- workspace、Ely 外壳、SQLite schema/迁移、本地源、SHA1/BLAKE3/pHash、缩略图、虚拟网格、检查器、sidecar 队列、FTS 搜索、标签面、设置。
- 当前（2026-10-24）：已有本地扫描、缩略图、v5 schema/自动备份迁移、SHA1/BLAKE3/pHash、`tags`/`asset_tags`、FTS5 搜索（trigram + 短词 LIKE 回退）、自动归类/评级、标签 facets、检查器、虚拟网格、RwLock 共享资产列表、Counts/筛选缓存、未变文件快速重扫和失败 3 次封顶；完全重复去重（忽略/全部保留/单组与批量移入回收站）与相似图片（pHash 阈值扫描/逐组评审/相关变体）均已完成；设置页已具备去重保护路径、相似阈值、数据位置和任务队列状态；打标任务已持久化到 jobs 表并支持重启恢复；10k 导入/打标/搜索/滚动已完成。缺网络源凭据；60fps 已在 10k 滚动下实测并达标（p99 8.4ms，10 万资产未测）；网格已用 256 小档 + 有界 LRU 图片缓存，2.2GB 内存不回落已定位并修复（img 绘制阶段回退到全局缓存，见 2026-10-25 条目）；代码仍为单 crate。
- 验收：1 万张本地图片端到端可导入、打标、按标签搜索；重启后任务可恢复；不修改原文件。

### 阶段 2：多源与去重
- Immich 只读同步、WebDAV 同步、统一资产视图、跨源精确/近似去重、来源徽标、离线错误处理。
- 验收：同一张图在本地与 Immich 被识别为同一组；网盘断线不阻塞本地库；删除/回收站对账正确。

### 阶段 3：标签工作流与交付
- 批量标签、阈值调优、智能集合、prompt 复制/导出、全屏查看器、缓存管理、安装包/签名、日志与诊断。
- 验收：10 万资产滚动 60fps 目标；搜索 p95 <150ms；缓存可限量；安装包在干净 Windows 环境可用。

### 阶段 4：扩展
- 视频缩略图/播放（ffmpeg/mpv）、小说（EPUB/PDF）阅读与文本索引、CLIP/向量语义搜索、可选标签写回与 XMP sidecar、Linux/macOS 发布。

## 11. 风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| Python sidecar 打包重 | 安装包大、用户环境依赖 | 首版随包分发 `runtime/python` + FP16 模型；CPU 环境明确提示 |
| CUDA/cuDNN 版本不匹配 | GPU 不可用 | sidecar 启动自检；CPU 降级；固定与 torch 匹配的 CUDA 轮子（cu128/cu130） |
| 8GB 显存 OOM | 批量中断 | batch 1 起步，OOM 降级；FP16 先做标签一致性验证 |
| 动漫模型误用于真人/截图 | 错误自动分类 | 低置信度进审核队列；非动漫只取低层标签；后续接通用模型 |
| 30,877 标签 UI 过重 | 卡顿/难找 | 虚拟化自动补全、按类别/计数过滤、FTS、常用标签置顶 |
| GPUI/Ely 升级破坏 | 编译失败 | vendor 固定 commit；UI 隔离在 `kanalib-ui`；升级单独 PR |
| Windows IME | 中文搜索不可用 | 阶段 0 实测；标签选择器/粘贴降级；跟踪上游 |
| Immich API 变更 | 同步失效 | 版本探测；只用稳定端点；契约测试；只读降级 |
| WebDAV 慢/大目录 | 首扫慢 | 并发限流、ETag、目录缓存、按需加载 |
| 去重误判 | 误删风险 | 只生成待审核组，绝不自动删除；相似组只逐组确认，不提供批量删除 |
| pHash 链式合并 | 同系列图被合进一组 | 组内显示相似度与距离，提示“并非两两完全相同”；保留“不是重复/相关变体” |
| 相似扫描 O(n²) | 10 万级变慢 | 当前 10k 270ms；后续按 pHash 分桶/索引或限制候选数 |
| 磁盘/缓存膨胀 | 占满 D 盘 | 默认 10GB 缓存预算 + LRU；模型/环境体积在设置中展示 |

## 12. 关键指标

- 模型：GPU 吞吐（张/秒）、峰值显存、与 CPU/官方 logits 的标签一致率。
- 索引：本地扫描张/秒、缩略图张/秒、失败率、重启恢复率。
- UI：10 万资产滚动帧时间、搜索 p95、内存占用、缩略图缓存命中率。10k 实测（2026-10-24）：搜索 p95 26.4–76.7ms；滚动 render build p95 0.68ms；10k 滚动合成器实测 164.8fps、present 间隔 p99 8.41ms、0.0% 帧超 16.67ms 预算（小档+缓存修复前为 149.3fps / p99 15.69ms / 0.5%；10 万资产滚动未测）。
- 数据：跨源精确匹配率、pHash 待审核组准确率（人工抽样）。相似扫描 10k 全量两两比较 270ms（测试构建，无命中）；当前 O(n²)，10 万级需分桶/索引。
- 稳定性：sidecar 崩溃重启次数、单源失败不影响其他源、无原文件写入。

## 13. 待确认/后续决策

1. ~~sidecar 交付形态~~：已定为首版随程序分发独立 CPython `runtime/python` + `dist/models` FP16，不引入 PyInstaller。
2. GPU 不可用时的产品行为：仅允许手动选择少量图片打标，还是允许 CPU 慢速后台跑？（建议默认暂停并提示。）
3. 是否需要 XMP sidecar 导出（不改原图，但会在图片旁生成 `.xmp`）？（建议阶段 3 再做。）
4. WebDAV 之外是否还要 S3/对象存储？（首版不做。）

## 14. 参考

- [PixAI Tagger v1.0 · HuggingFace](https://huggingface.co/pixai-labs/pixai-tagger-v1.0)
- [PixAI Tagger v1.0 · ModelScope](https://www.modelscope.cn/models/pixai-labs/pixai-tagger-v1.0)
- [Ely GPUI Components](https://github.com/ELY-Open-Source/Ely-GPUI-Components)
- [Immich API](https://api.immich.app/)
- [Immich OpenAPI（仓库内）](https://github.com/immich-app/immich/blob/main/open-api/immich-openapi-specs.json)
- [GPUI](https://www.gpui.rs/)
- 本地参考工程：`D:\Code\floral-notepaper`（Ely vendor、Windows 外壳、字体补丁）
- 本次探针：`tools/model_probe/probe.py`、`tools/model_probe/README.md`；结果在 `out*/probe.json`，GPU 基准在 `out_half/`、`out_autocast/`（`out_gpu/` 已清理）

## 15. 实现进展（可用示例 + 后端基准）

本地示例库已扩到 14 张（5 张基础样本 + 9 张角色样本），完成扫描、缩略图、PixAI 打标和自动归类；界面见 `.kanalib/arc_final.png`。

- 最初 5 张的自动归类结果：原创 2、`作品: indie_virtual_youtuber` 1、截图/文本 1、非动漫/实景 1；5 张全部评级 G。
- 单张端到端：Python sidecar FP16 约 327ms（3.06 张/秒），与 PyTorch 直接推理一致。
- ONNX / TensorRT 不再评估，相关探针脚本与文档已从仓库移除。
- 推理运行时随程序分发：`runtime/python/` 为独立 CPython（3.13）+ 固定版本依赖，启动时优先使用 exe 同级的 `runtime/python`；`KANALIB_PYTHON` 仅作为开发期覆盖。构建方式见 `scripts/build_runtime.ps1`，版本清单见 `scripts/runtime-requirements.txt`。
- 运行时已裁剪到 2.84GB：删除 `torch/include`、`torch/bin`、`torch/share`、`torch/lib/*.lib`、未使用的 `networkx`，以及 transformers 里未加载的模型实现（只保留 auto / bert / encoder_decoder / speecht5）。`torch/lib` 下的 CUDA DLL 一个都不能删——torch 在 Windows 上通过 `torch_global_deps` 预加载全部 DLL。
- 发布模型用 FP16 权重：`scripts/pack_model.py` 生成 `dist/models/pixai-tagger-v1.0`（0.91GB，源码模型 1.95GB）。sidecar 直接按目标精度加载，5 张样本的标签集合与分数和 FP32 模型完全一致。
- 已构建并验证：sidecar 就绪帧返回 `cuda / half / 30877`，启动日志确认使用 `runtime\python\python.exe`。
- 角色识别抽检（9 张，2026-10-24）：流萤 ×3 / 琪亚娜 / 荧 / 西琳 的角色标签分别为 1.000 / 1.000 / 1.000 / 0.999，全部命中；克拉拉（`clara_(honkai:_star_rail)` 仅 0.048）和格蕾修 ×2（`griseo` 0.15–0.16，阈值 0.27）未过角色阈值。作品/系列 8/9 正确，格蕾修-6.17 被误判为 original。界面见 `.kanalib/char_test.png`。
- 网络抽检（18 张，2026-10-24，星穹铁道 / 崩坏3 / 原神各 6 个角色）：角色身份 top-1 **18/18**，其中 12 张直接命中通用角色标签，6 张命中的是换装/形态标签（`kiana_kaslana_(herrscher_of_finality)`、`griseo_(starry_impression)`、`herrscher_of_sentience` 等）；作品/系列 18/18。样本与联络表见 `.kanalib/web_char_test/`。
- 同一角色在网络图与用户样本上差异明显：克拉拉网络图 0.982 命中，用户图仅 0.048，说明角色识别对画风/构图较敏感。后续做智能集合时需把形态/换装标签归并到同一角色。
- 自动归类规则（2026-10-24 已实现，`sidecar/tagger/server.py`）：
  1. **作品优先具体游戏**：copyright 最高分是 `*_(series)` 时，若同根的具体作品标签（如 `honkai:_star_rail`、`honkai_impact_3rd`）也过阈值，则取具体作品，否则保留 series。
  2. **角色形态归并**：`角色_(形态)` 形式且基础角色标签存在于 character 词表时归并为基础角色（如 `kiana_kaslana_(herrscher_of_finality)` → `kiana_kaslana`、`griseo_(starry_impression)` → `griseo`）。没有基础标签的角色（星铁/原神多数角色）保持全名，避免把不同作品的同名角色错误合并。
  3. 分组结果写入数据库；已有图片需要重新打标才会按新规则更新。
- 发布模型路径（2026-10-24）：`sidecar::model_dir()` 默认优先 `dist/models/pixai-tagger-v1.0`（FP16，930MB）；缺失时回退 `models/pixai-tagger-v1.0`（FP32，1.9GB）；`KANALIB_MODEL_DIR` 仍可覆盖。本机启动日志确认使用 `dist\models`，单张 smoke 图打标成功。
- sidecar 端到端复测（2026-10-24，14 张样本，`dist/models` FP16）：冷启动就绪 5.75 秒；batch 1 325ms/张（3.08 张/秒），batch 4 321ms/张（3.12 张/秒）。batch 4 无实质收益，默认改为 batch 1。
- 本地图片网格（2026-10-24）：已从 `flex_wrap` 全量渲染改为 `uniform_list` 按行虚拟化（固定行高 248，按主区域宽度计算列数），启动和切换筛选时回到顶部；当前只有可视行会创建 tile。
- 资产列表共享（2026-10-24）：`Shared.assets` 最终改为 `Arc<RwLock<Vec<Asset>>>`；UI 只持 Arc 并按需读锁，用 `Vec<usize>` 可见索引筛出结果，tile 只克隆可视资产；后台 upsert 原地写，不再 COW 复制整表。扫描中移除每 12 张的排序，结束统一排序。
- 路径索引与扫描性能（2026-10-24）：v2 的 `path` 原先只有 `UNIQUE(source_id,path)`，按 path 查询会全表扫描；`db.hashes`/`load_asset`/`update_tags` 每张图都要扫全表。补 `idx_assets_path` 后，10k 缓存导入从 61.9s 降到 48.4s，GUI 扫描（跳过打标）33.0s；修复前 GUI 扫描 4 分钟仍未完成。
- Counts 与筛选缓存（2026-10-24）：`Shared` 维护 `CountAccumulator`（总量、已打标、评级、归类、general 标签），`upsert` 只按单个资产增减，资产版本变化时才生成 top 视图；UI 缓存 Counts 和可见索引，缓存键为资产 revision + filter + 搜索文本 + 搜索 revision。选中资产或状态刷新不再重算全表，筛选只在条件或资产版本变化时重建。
- v2–v5 schema 与迁移（2026-10-24）：当前 `PRAGMA user_version=5`；建立 `sources/assets/tags/asset_tags/collections/collection_assets/jobs/saved_searches/settings` 与 FTS5；v3 增加 `assets.tag_attempts`，v4 增加 `duplicate_reviews`，v5 为 `jobs(kind, asset_id)` 建唯一索引与状态索引。迁移前自动把旧库复制为 `.kanalib/kanalib.db.bak-v{旧版本}-<时间戳>`。真实 14 条资产、268 个标签、539 条 `asset_tags` 与 10k 压测库均已迁移；`asset_scores` 尚未建，候选 logit 暂不落库。
- 内容哈希（2026-10-24）：扫描时流式计算 SHA1/BLAKE3，并生成 64-bit pHash；文件 mtime/bytes 未变时复用已有哈希。14 条资产已全部回填，检查器显示 SHA1/B3 前缀与 pHash。完全重复与相似图片去重 UI 均已实现。
- 完全重复去重 UI（2026-10-24）：侧栏新增“重复项”，进入独立工作区：组列表、比较卡片、只显示差异字段、当前保留/推荐理由、未审/已忽略/已保留筛选、上/下一组。评审操作有“不是重复”“稍后”“全部保留”“移入回收站”，单组与批量都先弹确认框；批量只处理未审组。保留项可用鼠标点选，选择立即写入 `duplicate_reviews`。删除调用 Windows 系统回收站，仅允许本地固定磁盘，且目标必须位于当前图库内；已用测试库验证单组 2 张、批量 4 张移入回收站后，文件和 DB 行同步消失，回收站中可查到原文件。测试库为 `.kanalib/dedup_test_lib`（3 组：2 张克拉拉、3 张 anime_girl、2 张 real_landscape）；界面见 `.kanalib/exact_ui_final.png`、`.kanalib/exact_ui_wide.png`、`.kanalib/dedup_ui11_trashed.png`、`.kanalib/dedup_ui14_batch_done.png`。
- 相似图片 UI（2026-10-24）：侧栏“相似图片”进入同一工作区的相似标签；后台计算 64-bit pHash 汉明距离，提供 6/10/14 三档阈值并持久化；组列表显示最低相似度，卡片显示各成员相对保留项的相似度，差异面板新增“相似度（距离）”行并提示链式合并风险。评审状态新增“相关变体”，与“不是重复”分开；相似模式只提供逐组移入回收站，顶部不出现批量按钮。测试库 `.kanalib/similar_test_lib` 6 张：原图与 50% 缩放距离 2、与 70% JPG 距离 0，20px 裁剪距离 20，默认 ≤10 归为 3 张组；另有 2 张无关图未入组。已验证“相关变体”落库、逐组移走 2 张变体（回收站可查到）并保留原图。界面见 `.kanalib/similar_ui_final.png`、`.kanalib/similar_ui5_related.png`。
- 去重工作区自适应布局（2026-10-24）：详情宽度 ≥1050 时，差异面板固定在卡片右侧，充分利用宽而矮的窗口；窄窗口下卡片与差异放在同一纵向滚动区，操作栏固定底部。修正了此前差异表被卡片和操作栏挤压、窗口底部留空的问题。
- 打标任务队列（2026-10-24）：`jobs` 表成为打标队列的唯一事实来源；扫描导入后入队，sidecar 按 4 张批次认领并落库 `running/done/error`，失败同步 `assets.tag_attempts`。启动时 `recover_tag_jobs` 把上次中断的 `running` 恢复为 `pending`，并立即按当前图库重新入队；设置页和侧栏显示等待/运行/失败。真实恢复测试：2 张图先以 `KANALIB_SKIP_TAG=1` 留下 2 个 pending，手动把 jobs 改为 running 模拟崩溃；再正常启动后恢复并完成，jobs 全部 done、2 张图全部 tagged；模型加载 11.86s、打标 1.24s、总 13.31s。v5 迁移前已备份为 `.kanalib/kanalib.db.bak-v4-*`。设置页队列状态见 `.kanalib/settings_ui3.png`。
- 仓库（2026-10-24）：`git init -b main`，初始提交 `7906dca`，1,857 个文件 / 7.97 MiB。忽略策略：`target/`、`runtime/`、`models/`、`dist/`、`.kanalib/`、`sample_library/`、根目录会话日志与 `.qoder-credits/`；`vendor/` 两处补丁依赖（Ely 组件、gpui-pre-windows）随库分发，克隆后按 `scripts/` 重建大件。全局 `core.autocrlf=true` 未改，`.gitattributes` 待补。
- 设置页与保护路径（2026-10-24）：侧栏新增固定“设置”入口，包含去重保护路径、相似阈值、数据位置和安全规则。保护路径持久化并参与去重分组：推荐保留项时优先保护路径；保护副本不能移入回收站，单组按钮禁用、批量自动跳过。已用整个 `.kanalib/dedup_test_lib` 作为保护路径验证：三组均显示“保护”，推荐保留项切换到保护副本，回收站按钮禁用。设置页见 `.kanalib/settings_ui2.png`，保护态见 `.kanalib/protected_ui1.png`。测试后已清空保护路径与评审状态，保留干净演示库。
- 去重测试（2026-10-24）：`cargo test` 19 项，17 通过 + 2 默认忽略（回收站集成测试、10k 相似扫描基准）；回收站测试单独运行通过。覆盖精确分组、相似阈值分组、同 SHA1 不进入相似边、保护路径优先保留、默认保留规则、成员变化重开、忽略/相关变体持久化、keeper 选择落库、v4/v5 迁移、评审读写与 jobs 队列恢复、缺失文件清理，以及 `dup:true` 搜索按去重分组路径过滤。10k 相似扫描基准（无相似命中、全量两两比较）270ms，当前为 O(n²)，10 万级需要分桶/索引。
- FTS 搜索（2026-10-24）：主工具栏增加搜索框；支持自由文本、`tag:`、`#tag`、`rating:`、`is:tagged/untagged`、`source:local` 和 `-tag:`/`-rating:`。自由文本走 FTS5 trigram，1–2 字或 FTS 无结果时回退 LIKE；搜索或扫描状态变化时刷新结果。库内实测：`1girl` 命中 12 张，`克拉拉` 命中 1 张；界面用 `克拉` 验证短词回退命中 `克拉拉.png`。界面见 `.kanalib/local_mvp_search.png`。
- `vocab_index`（2026-10-24）：sidecar 现在对每个标签返回词表下标；启动时从 `config.json` 的 `tags` 列表回填历史标签，268 个已迁移标签全部补齐。
- 当前本地 MVP 边界（2026-10-24）：单 crate 原型；已具备本地递归扫描、512 缩略图缓存、v5 持久化、sidecar 打标（失败 3 次封顶）、持久化打标队列与重启恢复、内容哈希、FTS 搜索、自动归类/评级、评级/归类/标签 facets、检查器、虚拟网格、完全重复去重、相似图片去重，以及设置页（保护路径/相似阈值/数据位置/队列状态）；未具备网络源凭据。合成器帧计时探针已具备（`--features frame-profiler` 取证档），网格缩略图已分 256 小档并走有界 LRU 图片缓存。
- 10k 合成库（2026-10-24）：`scripts/generate_stress_library.py` 生成 10,000 个文件（9,990 图像 + 10 损坏）、10 个目录、99 个精确重复、中文名与 JPG/PNG/WebP，79MB；压测 root 为 `.kanalib/stress_10k`，不触碰本地 14 张库。压测后已删除 `library` 与压测 `.kanalib`（约 0.5GB），仅保留指标 JSONL 与截图。
- 无界面压测（2026-10-24）：`KANALIB_HEADLESS=1`、`KANALIB_SKIP_TAG=1`、`KANALIB_METRICS_PATH`；阶段耗时写入 JSONL。顺带修复 `hash_file` 将 1MB 缓冲放在栈上、无界面主线程栈溢出的问题，改为堆分配。
- 10k 首次导入（2026-10-24，补 path 索引前）：201.3s（49.7 张/秒）；哈希 25.5s（392 张/秒），缩略图 112.6s（88.7 张/秒），其余约 63s 为 DB/写入；10 个损坏文件全部报错并继续；DB 11.3MB，99 个重复文件形成 97 个 SHA1 重复组（196 个资产）。
- 10k 缓存导入（2026-10-24，补 `idx_assets_path` 后）：48.4s（206 张/秒）；哈希全部复用，缩略图缓存 12.8s；GUI 扫描（跳过打标）33.0s。
- 未变文件快速重扫（2026-10-24）：`stored_meta` 命中 `mtime/bytes/thumb/hash` 时跳过哈希、缩略图、DB 写和 UI upsert。10k 重扫 1.98s（GUI 1.73s），9,990 未变；仅 10 个损坏文件继续处理。重启不再重写 10k 行。
- 10k 完整打标（2026-10-24）：模型加载 5.24s；推理 3099.84s（51.7 分钟，3.22 张/秒）；成功 9,990，损坏 10 个；总计 3167.22s（52.8 分钟）。DB 43.2MB，163 个标签、134,134 条 `asset_tags`。
- 10k 搜索（2026-10-24，打标后，100 次/查询，含 DB 打开）：`synthetic` 10,000 命中 p50 51.6ms / p95 76.7ms；`合成` 2,000 命中 p50 40.4ms / p95 48.4ms；`sky` 2,996 命中 p50 26.4ms / p95 29.6ms；`1girl` 6,490 命中 p50 39.7ms / p95 49.3ms。`synthetic` 首次查询冷启动最大 1.6s。
- 10k UI（2026-10-24，未变快速重扫 + 失败封顶后）：正常路径（不跳过打标）窗口出现 1.19s；扫描 1.89s；应用内 400 次滚动跳转，全链 17.3s；侧栏 10 个失败文件不再触发 sidecar。滚动阶段 804 次 render build：p50 0.14ms / p95 0.48ms / max 1.43ms；滚动期间仅 1 次 Counts/可见索引重建（扫描结束排序）。注意这是元素构建耗时，不是完整合成器帧率；完整帧计时见下面「合成器帧率测量」。界面见 `.kanalib/stress_10k/ui6_10k.png`。
- 打标失败重试上限（2026-10-24）：`assets.tag_attempts` 记录失败次数；达到 3 次后不再自动进入打标队列，文件 `mtime/bytes` 变化时在 `upsert_meta` 中重置。v2→v3 迁移把已有 `tag_state='error'` 行设为 3。10k 库正常扫描（不跳过打标）untagged=0、无 sidecar 启动，headless 2.14s，GUI 1.89s。
- 失败筛选与手动重试（2026-10-24）：侧栏增加 `失败` 计数/筛选和 `重试失败` 按钮；重试会清空当前目录失败资产的 attempts/error 并重新扫描。完成状态区分“全部已打标”和“N 个失败待处理”。截图见 `.kanalib/stress_10k/retry_ui2.png`。
- UI 压测开关（2026-10-24）：`KANALIB_UI_METRICS` 记录 Counts/索引重建与 render build；`KANALIB_UI_SCROLL_TEST` 通过应用内 `scroll_to_item` 做 400 次跳转（GPUI 不响应合成滚轮）；`KANALIB_FRAME_METRICS=<秒>` 记录合成器帧间隔，只在 `--features frame-profiler` 构建里生效。
- 合成器帧率测量（2026-10-24，release + `--features frame-profiler` 取证档，1700×1000 物理窗口、1.25 缩放、165Hz 面板）：`gpui-pre` 的 `profiler` feature 提供三个直方图——present 间隔（上一帧提交完成到本帧提交完成）、`Window::draw` 全量耗时、首次脏到提交完成的全链路延迟。探针在扫描结束且网格非空后每帧调用 `request_animation_frame()`：gpui 只有在窗口处于活动状态且持续有动画需求时才记录间隔，所以测量必须保持窗口在前台（`WindowOptions` 默认 `inactive_frame_interval=33.333ms`，失焦就限到 30fps）。
  - 上限档（14 张示例库、无滚动）：162.0fps，present 间隔 p50 5.87ms / p95 11.05ms / p99 12.77ms / max 23.87ms，超 16.67ms 预算 0.1%；draw p50 3.62ms。
  - 10k 静态（暖库、无滚动）：161.2fps，present 间隔 p50 6.10ms / p95 11.12ms / p99 12.43ms / max 14.23ms，超预算 0.0%；draw p50 2.01ms / max 27.21ms。p50 就是 1/165Hz，说明节律由合成器时钟决定而不是本应用。
  - 10k + 应用内 400 次滚动跳转（探针 12s 全程与滚动重叠）：149.3fps（1798 个间隔样本），present 间隔 p50 5.91ms / p95 12.76ms / p99 15.69ms / max 251.53ms，超 16.67ms 预算 0.5%（约 9 帧）。滚动期间 `KanalibApp::render` p50 0.10ms / p95 0.38ms / p99 0.67ms，探针窗口内没有任何一帧的 render 超过 16ms → 251ms 尖峰花在 `Window::draw` 的其它阶段（首次访问行的元素构建 + 512px 缩略图解码与纹理上传），不在本应用的渲染代码里。
  - 插桩不改变被测对象：同库同滚动测试用无 `profiler` 的 release 二进制对照，render build p50 0.12 / p95 0.42 / p99 0.79 / max 2.25ms，与取证档分布一致；对照跑完整 400 步，取证档跑约 12/13.2s。
  - 边界：present 间隔是应用向 DWM 提交的节奏（swapchain 用 `Present(0, DXGI_PRESENT(0))` 不阻塞，节律由 `DwmFlush` 驱动的 vsync 线程给出），不等于 DWM 最终上屏帧数；本档数据量是 10k，验收目标 10 万尚未测量，且可见索引重建是 O(n)。回归同步：`cargo test --features frame-profiler` 17 通过 + 2 默认忽略。
  - 脚本与数据：`scripts/run_frame_probe.ps1`；`.kanalib/frame_metrics.jsonl`（示例库）、`.kanalib/stress_10k/metrics-frame.jsonl`（10k 静态与滚动）、逐帧日志 `.kanalib/frame_rel_*.log`。
- 缩略图小档 + 有界图片缓存（2026-10-24，关掉上面滚动档的尖峰与内存无界）：
  - 机制：`img(path)` 在没有自定义缓存时走 `window.use_asset::<ImgResourceLoader>`，解码后的 `RenderImage`（BGRA）常驻到进程结束；网格用 512 档（`state.rs` 的 `image.thumbnail(512,512)`）意味着每个新 tile 上传约 1MB。
  - 改动 1：新增 256px 网格档 `<hash>-t256.jpg`，由已有 512 档再缩；`Asset.tile_thumb` 只在内存里（不落库、无 schema 变更）。扫描末尾 `backfill_tile_thumbs` 按 256 张一批生成并各 bump 一次 revision（IO 全在锁外，避免卡住 UI 读锁），启动时 `load_tile_thumbs` 只挂已存在的文件。10k 库首次补齐 9,990 张用 43.0s，之后只做存在性检查；inspector 与去重卡片仍走 512 档。
  - 改动 2：实现 `gpui::ImageCache` 的 LRU（`ThumbCache`，默认 128 项，`KANALIB_IMAGE_CACHE` 可调），用 `image_cache(entity)` 包住整棵树，逐出时 `cx.drop_image`；可见 tile 每帧 touch，不会被自己挤掉。
  - 效果（同一 10k 暖库、同 400 步跳距 32、release 取证档、窗口前台）：present 间隔 p99 15.69→8.41ms，max 251.53→14.88ms，超 16.67ms 预算 0.5%→0.0%，fps 149.3→164.8；12 秒峰值工作集 2,588→1,020MB。
  - 长跑（45s、1,200 次跳转、7,420 帧）：164.8fps、p99 8.36ms、max 26.30ms、超预算 0.03% → 尖峰问题关闭。
  - 上限对照：缓存压到 8 项（`cached_mb` 2.2MB）峰值仍 985MB、帧率不变 → 剩余约 1GB 由 tile 翻涌本身驱动（上色板/纹理侧），与缓存保留量无关；但 cap 8 的 dirty→present p50 从 2.31ms 升到 5.61ms，所以默认取 128。（该增长的根因已于 2026-10-25 定位并修复，见下条。）
  - 根因与修复（2026-10-25，内存不回落）：全部增长落在 (128K,256K] 分配档（256×256 BGRA 解码缓冲）。45s 长跑计数 alloc=73,121 / free=63,001 / live=10,120 块，停滚后一条不释放。逐层排除：atlas 页恒定 10 页/41.9MB；`KANALIB_ATLAS_SKIP_UPLOAD` 跳过上传无变化；`drop_image → AtlasState::remove → etagere deallocate` 链完整；`remove_asset` 按原始库路径清缓存是一轮无效探针（实际绘制用的是缩略图路径，键不匹配）；把解码换成自解码后仍复现。最终用「按 Arc 指针登记 + 256K 档分配 backtrace」定位：泄漏块由 gpui 的 `ImageAssetLoader` 在**绘制阶段**解码——`img` 元素绘制时 `image_cache_stack` 为空（`interactivity.paint` 的绘制路径不在 `image_cache` 元素的作用域内），回退到 `window.use_asset` 的全局资产缓存，该缓存永不淘汰。布局阶段走 `ThumbCache`、逐出时 `drop_image` 正常，所以只观察 LRU/Arc 时一切像是干净的，这是此前判断被误导的原因。
  - 改动 3：所有 `img()` 显式挂 `.image_cache(&thumb_cache)`（网格 tile、inspector、去重组卡片/资产卡），不再依赖 `image_cache_stack` 的作用域；`ThumbCache` 的 Path 资源改为自解码（`decode_thumb`：`fs::read` + `image::load_from_memory` + `RenderImage::new([frame])`），去掉 gpui `CachedLoad`/`SmallVec::from_elem` 的克隆路径，余下非 Path 资源仍走 `ImageCacheItem`；解码中的条目延迟到下一轮逐出，避免取消后台解码。
  - 修复后实测（45s、1,200 次跳转、7,422 帧）：164.9fps、present p99 8.33ms、超 16.67ms 预算 0.03%；峰值工作集 132.9MB、停滚后 113.6MB（修复前 2,213.9 / 2,204.8MB）；256K 档全程恒定 28.5MB / 130 块（alloc 28,768 / free 28,638），`cached_images` 128 / `cached_mb` 27.3。
  - 取证工具（只在 `--features frame-profiler` 档编译，默认档零影响）：`src/alloc_stats.rs` 按 2 的幂尺寸档统计存活字节与 alloc/free 次数（`KANALIB_ALLOC_STATS=<秒>`；`KANALIB_ALLOC_TRACES=1` 记录 256K 档存活块分配栈，dbghelp 非线程安全故捕获全程持锁）；vendor `directx_atlas` 的 `KANALIB_ATLAS_STATS=<秒>` 计数与 `KANALIB_ATLAS_SKIP_UPLOAD=1` 开关；`KANALIB_WATCH_IMAGES=1` 按 Arc 指针登记存活解码图；`[profile.probe]`（release + debug=1）用于解析栈符号。探针脚本新增 `-AtlasStatsSecs` 并回显 alloc/atlas/watch 行。
  - 测量有效性：低前台帧率（4–112fps）的几轮出现过同样的 2.2GB 平台，属同一泄漏；任何一轮都必须先确认 `renders>0`、窗口在前台，否则数据作废（曾出现整轮 139MB 的假数据）。
  - 测量前提：gpui 只在窗口活动时记录 present 间隔，失焦会被 `inactive_frame_interval=33.333ms` 限到 30fps 且样本为 0；脚本现在每秒 `AppActivate` 本进程窗口，探针在 `presents==0` 时打 warn。
- 合成库的模型输出只用于管道压力测试：9,990 张中 6,556 归为截图/文本、1,651 非动漫/实景、1,273 未分类，不能作为真实图库的分类质量指标。
- Immich/WebDAV：按 2026-10-24 决定暂缓，先完成本地 MVP；阶段 2 再接入。
