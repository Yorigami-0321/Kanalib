# 非 UI 现成组件调研

> 上级索引：[../FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md) · 文档地图：[README](./README.md)

> 范围：只覆盖后端、文件处理、来源、搜索、缓存、日志、交付等非 UI 条目；F-003/F-005/F-006/F-007/F-008/F-009/F-041/F-042 等界面交互不在本表。版本为 2026-10-26 在 crates.io 查询到的结果，落库前需再核对当前版本与 MSRV。
>
> 原则：**能用成熟 crate/二进制解决的不自己写**；已经在工作区稳定运行、且经过实测的实现（标签面、搜索语法、EXIF、缓存预算）不为了“用库”而重写。

### 结论速览

- **直接采用**：`notify`/`notify-debouncer-mini`（文件监听）、`ignore`/`globset`（排除规则）、`reqwest` + `reqwest_dav` + `keyring`（网络源与凭据）、`rusqlite` 的 `backup` feature（数据库备份）、`flexi_logger`（滚动日志）、`image` 的 tiff/avif/ico/qoi/exr feature、`lcms2`（ICC）、`ffmpeg-sidecar`（ffmpeg 进程管理）、`rbook`/`pdfium-render`/`lopdf`（小说/PDF，阶段 4）。
- **先评估**：`ffmpeg-next`（libav 进程内解码）、`image_hasher`（替换自研 pHash）、`libheif-rs`/`rawloader`/`psd`（HEIC/RAW/PSD）、`usearch`/`sqlite-vec`/`fastembed`（向量检索）、`moka`/`lru`（缓存重写）。
- **保持自研**：打标阈值/低置信度、jobs 队列、单来源扫描、从图库移除、扫描进度、磁盘缓存预算、当前 pHash 与 ThumbCache；这些没有合适的现成件，或现有实现已实测稳定。
- **不建议引入**：`tantivy`（当前 SQLite FTS5 足够，双索引成本高）、通用任务队列框架、GPL 许可的 `epub` crate（项目 MIT，许可冲突）。

### 逐条对照

| 条目 | 可直接复用 | 许可/状态 | 建议 | 备注 |
|---|---|---|---|---|
| F-040 | 未引入 `ffmpeg-sidecar`；改为多输入批处理 CLI（`std::process`），需要进程内解码时再评估 `ffmpeg-next` 9.0.0 | LGPL shared BtbN 构建已落 `runtime/ffmpeg`；`ffmpeg-next` WTFPL 包装 | 10k 基准已通过门禁，保留批处理 CLI；100k 结果接近上限时再评估 libav | 不自写 libav 绑定；GPL 构建默认不进包 |
| F-030 | `notify` 8.2.1 + `notify-debouncer-mini` 0.7.0（稳定）；需要重命名配对时评估 `notify-debouncer-full` 0.8 RC | CC0 / MIT OR Apache-2.0 | 直接采用，替换手写轮询/防抖 | 与 F-032 的 `ignore` 过滤一起用 |
| F-032 | `ignore` 0.4.33（WalkBuilder，支持 gitignore/glob 覆盖）；需要单独匹配时用 `globset` | Unlicense OR MIT | 直接采用；用它替换纯 `walkdir` 的排除逻辑 | 自定义规则顺序、隐藏文件、最小/最大字节数仍由业务层决定 |
| F-033 | `image` 0.25 的 `tiff`/`avif`/`ico`/`qoi`/`exr` feature；`libheif-rs` 3.0.0（HEIC）；`rawloader` 0.37.2（RAW）；`psd` 0.3.5（PSD） | image MIT/Apache；libheif-rs MIT + 原生 libheif LGPL；rawloader LGPL-2.1；psd MIT/Apache | 先开 image feature（TIFF/AVIF 等），HEIC/RAW/PSD 按许可与真实需求再评估 | ffmpeg 可覆盖部分格式，但不能替代全部；解码器安全/体积要测 |
| F-035 | `reqwest` 0.13.5（HTTP）；`reqwest_dav` 0.3.3（WebDAV）；`keyring` 4.2.0（Windows Credential Manager） | MIT OR Apache-2.0 | 直接采用；Immich 没有维护良好的 Rust SDK，按官方 OpenAPI 生成薄客户端或手写少量端点 | 不自写 HTTP/WebDAV/凭据存储；只读约束仍在业务层 |
| F-061 | `rusqlite` 0.32（项目当前）的 `backup` feature（SQLite Backup API；crates.io 最新 0.41） | MIT | 直接采用，替代手写复制 DB/WAL | 与定期备份/恢复 UI 一起做；恢复前重启/独占仍由应用控制 |
| F-062 | `flexi_logger` 0.31.10（滚动文件、保留策略）；或 `tracing` + `tracing-appender` | MIT OR Apache-2.0 | 直接采用，替换当前仅 env/headless 才落盘的 `env_logger` 配置 | 不上自研日志轮转；敏感路径/凭据需脱敏 |
| F-045 | `image_hasher` 3.1.1（pHash/平均哈希等） | MIT OR Apache-2.0 | 先评估：换库会改变哈希值，现有去重评审状态会失效；新实现或重写时直接用 | 当前自研 DCT pHash 已稳定，不必为用库而换 |
| F-060 | `lru`（已被传递依赖）/`moka` 0.12.16（并发、权重） | MIT/Apache | 保持现有 `ThumbCache` 与磁盘预算；仅内存缓存重写时考虑 `moka` | 当前 10 万实测已达标，重写风险大于收益 |
| F-012 | SQLite FTS5（已有）+ 索引/去重路径懒加载/输入防抖；`tantivy` 0.27 可用但不建议 | MIT | 保持 FTS5，不引入 Tantivy；先把现成 SQLite 能力用满 | 双索引会带来一致性与磁盘成本 |
| F-016 | Python `open_clip_torch`（sidecar）+ `usearch` 2.26.6（ANN）或 `sqlite-vec` 0.1 alpha；Rust 侧 `fastembed` 7.5.0 | open_clip MIT；usearch Apache-2.0；sqlite-vec MIT/Apache（alpha）；fastembed Apache-2.0 | 评估：优先 open_clip + usearch；sqlite-vec 仍 alpha，不作为唯一方案 | 不自己写向量索引；后端仍受 PyTorch sidecar 约束 |
| F-064 | `lcms2` 6.2.0（LittleCMS 包装） | MIT | 直接采用（若做 ICC/广色域） | 当前 ffmpeg 构建已带 lcms2；不要在 Rust 里手写色彩变换 |
| F-026 | `quick-xml` 0.42（解析/生成 XMP）；或外部 ExifTool；Exempi 的 Rust 绑定已弃用 | MIT；ExifTool 需额外分发 | 暂缓：没有维护良好的 Rust XMP 绑定，先明确写回范围 | 不自己实现完整 XMP 模型；优先 sidecar 导出 |
| F-066 | 视频用 ffmpeg；EPUB 用 `rbook` 0.7.12（Apache-2.0），不要用 GPL 的 `epub` 2.1.5；PDF 用 `pdfium-render` 0.9.4 或 `lopdf` 0.46 | Apache/MIT；pdfium 需原生二进制 | 阶段 4 评估；先把 ffmpeg 接入复用到视频 | 许可与二进制约束多，别在阶段 1 开工 |
| F-021 | 无合适现成件 | — | 保持 sidecar 自研：阈值/低置信度是模型语义 | 可复用现有 jobs/标签表，不引入新框架 |
| F-023 | 无合适现成件 | — | 保持 jobs 表；不要引入通用 job queue | 现有 SQLite 队列已持久化/可恢复 |
| F-024 | 复用现有 sidecar + jobs | — | 不新增组件 | 选择范围与覆盖策略是业务逻辑 |
| F-031 | 无合适现成件 | — | 保持自研来源模型 | 需要先把 local roots 统一到 sources |
| F-037 | 现有 `Db::delete_assets` | — | 直接复用 | 只删 DB、不碰磁盘；忽略列表另做 |
| F-063 | 无合适现成件 | — | 自研结构化 ScanProgress | `indicatif` 只适合 CLI，不适合 GPUI 状态栏 |
| F-013/F-014 | SQLite `saved_searches` / `collections` | — | 复用现有 schema | 不需要 ORM/集合框架 |
| F-065 | Inno Setup / WiX / MSIX + Windows 签名工具 | 各自许可 | 评估；不自研安装器 | 便携版已可用，安装器属阶段 3 |

### 采用边界

- **先复用、后重写**：F-030/F-032/F-035/F-061/F-062 是明确的“不要自己写”项；直接引入 crate 能减少大量边界 bug。
- **不要为了库而换算法**：F-045 的 pHash、F-060 的 ThumbCache 已经过实测；换库必须单独评估评审状态/内存/缓存迁移，不与 UI 改版捆绑。
- **许可先过门**：`ffmpeg` GPL full build、`rawloader` LGPL-2.1、`libheif` LGPL、`epub` GPL-3.0 都需要在分发前确认；项目当前是 MIT，不能默认把 GPL 依赖随包发布。
- **格式支持不要夸大**：`image` feature 与 ffmpeg 构建能力不同；HEIC/AVIF/RAW 以实际样本测试为准，不写“支持所有格式”。
- **已有实现不返工**：F-010/F-011/F-020/F-022/F-034/F-060 已在工作区实现并通过测试；除非新增需求，不重写为第三方库。
