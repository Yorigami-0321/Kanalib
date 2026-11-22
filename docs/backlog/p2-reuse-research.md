# P2 之后可复用组件调研（v1）

> 上级：[execution-workstreams.md](execution-workstreams.md) · 入库：[ingest-service.md](ingest-service.md) · 网页后端：[web-dynamic-backend.md](web-dynamic-backend.md) · 桌面组件：[component-research.md](component-research.md)
>
> 2026-11-22 用户要求：继续详细调研 P2 之后，找参考，找能直接复用的项目。
>
> 状态：**调研稿，未实现**。版本与许可来自 crates.io / PyPI 官方 API（2026-11-22 查询）；GitHub 原始页面受本机代理影响未能直读，落库前须在仓库核对 `LICENSE` 与最新版本。

## 0. 结论

| 结论 | 内容 |
|---|---|
| 最值得直接复用 | `pixiv-client` 2.0.0（MIT，35+ 端点 + 并发下载器 + SNI 绕过）、`axum` + `tower-http`、`rusqlite`、`governor`、`hmac`、`gif`、`zip` |
| 已有一半，别重写 | 打标 sidecar（`sidecar/tagger/server.py`，JSON-lines + 分类标签 + 分数）；P3 只加一个 `HttpTaggingBackend` 或复用现有进程 |
| 只能参考设计 | `gallery-dl`（GPL-2.0）、Pixiv-Archive（GPL-3.0）；字段/流程可借鉴，代码不可复制 |
| 需要适配层 | `pixiv-client` 用 `reqwest 0.13`，工作区锁 `reqwest 0.12.28`；必须隔离在独立 crate，不能让版本渗透到桌面 |
| 不建议引入 | 通用任务队列（`apalis`）、`ort` 推理栈、`rhai` 规则引擎；现有 SQLite 队列 + Rust 规则已够 |

一句话：**P2 之后真正缺的是“编排与边界”，不是“组件”。** 绝大多数底层能力已有成熟 crate 或本地实现。

## 1. 调研方法

| 项 | 做法 |
|---|---|
| 许可 | 以 crates.io `version.license` / PyPI `classifiers` 为准；GPL 一律只读设计 |
| 活跃度 | 看 `updated_at`、`recent_downloads`、`num_versions`；不看 star |
| 复用级别 | **A 直接引入** / **B 适配后引入** / **C 只参考设计** / **D 不引入** |
| 冲突 | 先查工作区已锁版本（`Cargo.toml`、`crates/kanalib-core/Cargo.toml`） |

工作区现状（会影响选型）：

- `image = 0.25`（已开 jpeg/png/webp/gif/bmp/tiff/ico/qoi/exr）。
- `rusqlite = 0.32`（bundled + backup + functions）。
- `reqwest = 0.12.28`（blocking + rustls-tls-webpki-roots + json）。
- `serde 1` / `serde_json 1` / `anyhow 1`。
- 桌面没有 async runtime；新服务可以独立用 `tokio`。

## 2. P2 · 网页后端只读骨架（`W1`）

### 2.1 可直接用

| 组件 | 版本/许可 | 用途 | 备注 |
|---|---|---|---|
| `axum` | 0.8.9 / MIT | 路由、提取器、状态 | 与 `tower-http` 同生态；0.8 起 `Router` API 稳定 |
| `tower-http` | 0.7.1 / MIT | `ServeDir`、`CompressionLayer`、`TraceLayer`、`SetResponseHeader` | ETag/Range 用 `ServeFile` 或自写 `media.rs` |
| `tokio` | 1.x / MIT | 运行时 | 新服务独立 runtime，不影响 GPUI |
| `rusqlite` | 0.32 / MIT | 网页独立 SQLite | 与桌面同大版本，但**不共享 DB 文件** |
| `mime_guess` | 2.0.5 / MIT | 媒体 Content-Type | 只按扩展名，仍需 DB 反查 + 路径校验 |

### 2.2 需要适配

| 项 | 说明 |
|---|---|
| 双 listener | `axum` 没有内置“两个 Router 两个端口”；用两个 `tokio::spawn` + 两个 `axum::serve`，共享 `AppState` |
| 静态 SPA 托管 | `ServeDir::fallback(ServeFile::new("index.html"))`；注意 `/api/*` 不能落到 fallback |
| ETag/Range | `tower-http` 的 `ServeFile` 已支持；自定义 `media.rs` 要自己算 ETag（`blake3` 或 size+mtime） |
| 2c2g 内存 | 不开 `CompressionLayer` 对 JPEG 无意义；只对 JSON 开 gzip/br |

### 2.3 不建议

- 不引入 ORM（`sqlx`/`diesel`/`sea-orm`）：网页 schema 小，`rusqlite` 手写 SQL 更可控，也避免 async 驱动额外依赖。
- 不引入 `tower-sessions`/账号体系：v1 公开只读，写接口用 HMAC + 设备令牌。

### 2.4 可参考的现成实现

| 项目 | 许可 | 参考点 |
|---|---|---|
| PhotoPrism | AGPL-3.0 | 只读 API 形状、媒体缓存头；**代码不可复制** |
| Immich | AGPL-3.0 | 分页/游标、媒体 tier；**代码不可复制** |
| HomeGallery | MIT | 静态导出与索引结构；可读代码，但本项目已有契约 |

> 结论：`W1` 没有“整段可抄”的项目，但 `axum` + `tower-http` 的组合足够，剩余是业务代码。

## 3. P2 · 入库服务骨架 + Pixiv 适配（`I0`→`I1`）

### 3.1 Pixiv 客户端（关键决策）

| 候选 | 版本 | 许可 | 活跃度 | 能力 | 结论 |
|---|---|---|---|---|---|
| **`pixiv-client`** | 2.0.0 | **MIT** | 2026-08 更新；333 总下载 / 81 近期 | 35+ 端点、typed + raw fallback、tokio、并发下载器、`gfw-bypass`（DoH） | **A：首选** |
| `pixiv3-rs` | 0.1.0 | Unlicense | 2026-02；331 下载 | pixivpy3 的 Rust 移植，流式迭代 | B：备选；端点覆盖不如前者明确 |
| `pixivcrab` | 0.5.2 | MIT | 2025-07；473 下载 | AppAPI 薄封装 | B：只做参考 |
| `pixiv`（rustpixiv） | 0.2.0 | 未核实 | 2018 停更 | 旧 AJAX API | D：不用 |
| `gallery-dl` | 1.32.x | **GPL-2.0** | 活跃 | 站点提取器最全，含 Pixiv | **C：只读设计**，不可链接/复制 |

`pixiv-client` 的能力与本项目需求对照：

| 需求 | `pixiv-client` | 结论 |
|---|---|---|
| refresh token 认证 | `auth(refresh_token)`、`refresh_token()` | 直接满足 |
| 收藏分页 | `user_bookmarks_illust(user_id, restrict, max_id, tag)` | 直接满足 |
| 作品详情/多页 | `illust_detail`、`illust_pages`（按 README 端点表） | 直接满足 |
| 作者信息/头像 | `user_detail` | 直接满足 |
| 标签翻译 | `set_accept_lang("zh-CN")` | 直接满足 |
| 并发下载 | 内置 semaphore 下载器 | 可直接用，但仍需 `.part`/Range 包装 |
| 429/404/auth 分类 | `is_auth_error()` 等 | 需在适配层补齐 429/404 分类 |
| Ugoira | README 未列 | **需自己实现**（`zip` + `gif`） |

**关键冲突：`reqwest` 版本。**

| 项 | 版本 |
|---|---|
| 工作区 `kanalib-core` | `reqwest = 0.12.28`（精确锁） |
| `pixiv-client` 2.0.0 | `reqwest ^0.13`（`json/form/rustls/stream`） |

处理方式（按推荐顺序）：

1. **隔离 crate**：`kanalib-ingest` 独立依赖 `pixiv-client`，不让 `reqwest 0.13` 出现在桌面依赖图。
2. 如果 Cargo 报 links/feature 冲突，再评估把桌面 `reqwest` 升到 0.13；这属于独立任务，不与 P2 捆绑。
3. 不建议 fork `pixiv-client` 改 0.12：维护成本高于隔离。

### 3.2 下载与续传

| 组件 | 版本/许可 | 用途 | 结论 |
|---|---|---|---|
| `reqwest` 0.13（随 `pixiv-client`） | MIT/Apache-2.0 | HTTP、stream | 用它的 `bytes_stream()` 写 `.part` |
| `tokio::fs` | MIT | 异步文件写入 | 直接 |
| `tokio-util`（`io`） | MIT | 流适配 | 可选 |
| `governor` | 0.10.4 / MIT | 全局限流（Pixiv API + 图片） | **A：推荐**；比手写令牌桶稳 |

`.part` + Range + rename 没有“现成库”可抄；这是 20 行状态机，按 [ingest-service.md](ingest-service.md) §5 自写。

### 3.3 Ugoira → GIF

| 组件 | 版本/许可 | 用途 | 结论 |
|---|---|---|---|
| `zip` | 9.8.1 / MIT | 解压 ugoira zip | **A：直接用** |
| `gif` | 0.14.2 / MIT OR Apache-2.0 | 编码动画 GIF、帧延迟 | **A：直接用** |
| `image` 0.25 | 已在工作区 | 帧解码/缩放 | 复用 |

注意：

- Ugoira zip 里是 `000000.jpg` 等帧 + `animation.json` 的 `delay`；`gif` 的 `Frame::from_rgba_speed` + `delay` 足够。
- 帧数上限 500、单帧尺寸取首帧，按 [ingest-service.md](ingest-service.md) §5.3。
- `gif` 的 `color_quant` 默认开启；大图要限制尺寸后再量化，避免 CPU 抖动。

### 3.4 服务内队列

| 候选 | 许可 | 结论 |
|---|---|---|
| `apalis` 1.2.0 | MIT | **D：不引入**；它面向 Redis/Postgres 后端，三节点场景过重 |
| 自写 SQLite 队列 | — | **A：沿用**；[ingest-service.md](ingest-service.md) §5.1 状态机已定 |
| `tokio::sync::Semaphore` | MIT | 控制多页并发；直接使用 |

### 3.5 凭据

| 组件 | 版本/许可 | 用途 | 结论 |
|---|---|---|---|
| `keyring` | 桌面已锁 `=3.6.3`（windows-native） | Windows 凭据管理器 | 桌面复用；box 是 Linux，改用文件 + 权限 600 或环境变量 |
| `secrecy` / `zeroize` | MIT/Apache | 内存中令牌不落日志 | 可选；至少保证 `Debug` 不打印 |

box 上不要把 refresh token 写进 SQLite 明文；`credential_ref` 只存引用。

## 4. P3 · 打标与规则（`I2`）

### 4.1 打标后端：优先复用现有 sidecar

本地已有 `sidecar/tagger/server.py`：

- JSON-lines stdin/stdout 协议；`{"cmd": ...}` + `{"id", "ok"}` 回包。
- `transformers.AutoModel` 加载本地模型目录；输出按 `general/character/copyright/style/meta/rating` 分类，带阈值与候选上限。
- 448 输入、显式像素上限、容错读取。

**结论：P3 不新建推理栈。** 两种接法：

| 接法 | 做法 | 适用 |
|---|---|---|
| **进程复用** | `kanalib-ingest` 以子进程启动现有 sidecar，按 JSON-lines 收发 | 首选；零协议改动 |
| HTTP 包装 | 在 sidecar 外加 10 行 FastAPI/axum 代理 | 需要多客户端/远程时 |

`TaggingBackend` trait 仍要定义，但默认实现是 `SidecarTaggingBackend`，不是新模型。

### 4.2 如果未来要 Rust 原生推理

| 组件 | 版本/许可 | 结论 |
|---|---|---|
| `ort` | 2.0.0-rc.15 / MIT OR Apache-2.0 | **B：仅评估**；仍是 rc，且要分发 ONNX Runtime 动态库 |
| `tract-onnx` | 0.21 / MIT OR Apache-2.0 | B：纯 Rust，但算子覆盖/性能需实测 |
| Python `onnxruntime` | 1.32.0 / MIT | 已可用；比 Rust 绑定成熟 |

不建议为了“去 Python”而重写；sidecar 已在桌面验证。

### 4.3 模型与许可

| 项 | 说明 |
|---|---|
| 模型 | 现有 sidecar 用本地 `transformers` 目录；具体权重许可需在模型卡核对 |
| WD14 系（SmilingWolf） | 常见于动漫图打标；HuggingFace 页面本轮因 fake-IP 无法直读，**许可未核实**，不作为结论 |
| 结论 | 继续用现有模型目录；新增模型前必须单独核许可，不默认随包分发 |

### 4.4 规则引擎

| 候选 | 许可 | 结论 |
|---|---|---|
| `rhai` 1.27.0 | MIT OR Apache-2.0 | **D：不引入**；规则是固定谓词组合，嵌入式脚本会带来沙箱/审计成本 |
| 自写 Rust 规则 | — | **A：推荐**；`RuleConfig { version, blacklist{users,tags}, rating, ai, tag_budget }` + 命中记录；不做表达式求值 |
| `regex` | MIT OR Apache-2.0 | **D：v1 不用**；黑名单是归一化后精确匹配，正则会把「精确」变成「像」，误杀不可控 |
| `globset` | 桌面已用 0.4 | **D：v1 不用**；用户 ID 是数字精确匹配，标签是字符串精确匹配 |
| `toml` / `serde` | 已用 | 规则配置文件解析；`serde` 反序列化后手动校验不变量 |
| `toml_edit` | MIT OR Apache-2.0 | **A：推荐**；管理页应用草稿时只改目标行，保留注释/键序/空行。不能用 `toml` 反序列化再序列化，那会洗掉全部格式 |
| `tempfile` | MIT OR Apache-2.0 | 写 `<file>.tmp` + `fsync` + `rename` 的原子落盘 |
| `fs2` / `fd-lock` | MIT / MIT OR Apache-2.0 | 单实例文件锁，防止两个应用操作同时改文件 |

规则顺序按 [ingest-service.md](ingest-service.md) §7.2；每次决策落 `rules_version` + 命中条目。规则文件是唯一来源，没有数据库规则表。管理页**不直接**改文件，而是写 `rule_proposals` 草稿，再走 [ingest-service.md](ingest-service.md) §7.8 的 `toml_edit` 原子改写流程。

### 4.5 审核队列 UI

| 候选 | 结论 |
|---|---|
| 复用 `axum` + 一个极简 HTML/HTMX 页 | **A：推荐**；不引入前端框架 |
| 复用 `web/` React | 不建议；审核页只走 EasyTier 私网，独立小页面更快 |

## 5. P3 · 网页后端搜索/游标/facets（`W2`）

| 组件 | 版本/许可 | 用途 | 结论 |
|---|---|---|---|
| SQLite FTS5 | 随 `rusqlite` bundled | 标签/标题搜索 | **A：直接用** |
| `rusqlite` `functions` | 已开 | 自定义排序/归一化 | 直接 |
| `tantivy` | MIT | 全文索引 | **D：不引入**；双索引一致性成本高 |
| keyset 游标 | 自写 | `(sort_key, id)` base64 | 直接；不要 offset 分页 |

facets 用一条 `GROUP BY` 聚合 + 索引；不要为 facets 建物化表，除非压测证明需要。

## 6. P3 · 网页 `ApiSource`（`W3a`）

| 项 | 结论 |
|---|---|
| 现有 `StaticSource` | 保留；`WallSource` 接口已存在，`ApiSource` 实现同一接口 |
| 新增依赖 | **零**；用 `fetch` + 现有类型 |
| 分页 | `PageDto<T>` 的 `next` 游标；`reset()` 清状态 |
| 媒体 | 同源 `/media/t256` 与 `/media/t512`；原图 302 到 box 签名 URL |
| 错误 | 503 时保留已加载、显示可重试条；不清空 |

不要引入 `react-query`/`swr`：现有 `useWall` 已是手写状态机，换库会牵动全部探针。

## 7. P4 · 内部 ingest 接口 + 桌面桥接（`W4`）

### 7.1 签名与鉴权

| 组件 | 版本/许可 | 用途 | 结论 |
|---|---|---|---|
| `hmac` | 0.13.0 / MIT OR Apache-2.0 | HMAC-SHA256 批次签名 | **A：直接用** |
| `sha2` | 0.11 / MIT OR Apache-2.0 | 哈希 | 直接 |
| `subtle` 或 `constant_time_eq` | 0.6.1 / CC0/MIT-0/Apache-2.0 | 常量时间比较 | **A：直接用**，不要 `==` |
| `base64` | 0.22 / MIT OR Apache-2.0 | 签名/游标编码 | 直接 |
| `rand` | 0.9 / MIT OR Apache-2.0 | 设备令牌生成 | 直接 |
| `argon2` | 0.6.0 / MIT OR Apache-2.0 | 设备令牌哈希 | **B：可选**；令牌是高熵随机串，`blake3` 哈希足够，argon2 反而增加 CPU |

签名规范按 [ingest-service.md](ingest-service.md) §9.2：

- 覆盖 `timestamp + nonce + body_hash`，不要只签 body。
- 时间窗 ≤5 分钟；nonce 落库去重。
- 幂等键与签名分开：幂等键防重复入库，签名防伪造。

### 7.2 幂等与批次

| 项 | 结论 |
|---|---|
| 幂等表 | 网页 SQLite `ingest_batches(batch_id PRIMARY KEY, ...)` |
| 重复批次 | 返回 200 + 已处理计数，不重复写 |
| 乱序 | 用 `revision`/`updated_at` 比较；删除事件单独接口 |

不需要分布式锁；单机 SQLite `BEGIN IMMEDIATE` 足够。

### 7.3 设备令牌

| 项 | 结论 |
|---|---|
| 存储 | 只存 `blake3(token)` + `device_id` + `created_at` + `revoked_at` |
| 传输 | `Authorization: Bearer <token>`，只走 private listener |
| 撤销 | 置 `revoked_at`；中间件每次查库或短 TTL 内存缓存 |

## 8. P4 · 推送 / 删除 / 原图签名（`I3`）

| 项 | 结论 |
|---|---|
| HTTP 客户端 | 复用 `pixiv-client` 带来的 `reqwest 0.13`，或 `kanalib-ingest` 自建 client |
| 重试 | 自写指数退避 + `next_retry_at`；不引入 `backoff`/`tower-retry` 也可 |
| 签名 URL | `hmac(secret, item_id + exp)` + base64url；box 侧校验 |
| 原图 | 桌面优先直连 box；网页只 302，不代理字节 |

## 9. P5 · 桌面桥接（可选）

| 项 | 结论 |
|---|---|
| 新 crate | `kanalib-bridge`，只依赖 `reqwest`（与桌面同 0.12）或直接用 `ureq` 同步客户端 |
| 线程模型 | 桌面是 GPUI，不要引入 tokio runtime；用后台线程 + channel 回 UI |
| 冲突 | last-write-wins + `updated_at`；双向都改过进冲突列表 |
| 开关 | 默认关闭；关闭时不建线程、不建目录、不发请求 |

`ureq`（MIT/Apache-2.0）比 `reqwest` 更轻，但缺少流式下载；桥接需要下载图片，建议复用桌面现有 `reqwest 0.12` blocking。

## 10. P5 · 部署、反代、备份、监控

| 项 | 结论 |
|---|---|
| 反代 | Nginx 或 Caddy；只反代 public listener，显式 404 写路由 |
| 进程管理 | systemd（box/云）；`Restart=always` + `LimitNOFILE` |
| 备份 | `rusqlite` backup API（桌面已用）+ 文件快照；不要 `cp` 正在写的 DB |
| 日志 | `tracing` + `tracing-subscriber`（0.3.23 / MIT）；JSON 输出便于采集 |
| 监控 | 先 `/api/health` + 磁盘/队列指标；不引入 Prometheus 栈，除非已有 |

## 11. 复用级别总表

| 工作流 | A 直接引入 | B 适配后引入 | C 只参考 | D 不引入 |
|---|---|---|---|---|
| `W1` 网页后端 | `axum`、`tower-http`、`tokio`、`rusqlite`、`mime_guess` | 双 listener、ETag | PhotoPrism/Immich API 形状 | ORM、账号体系 |
| `I0/I1` 入库 | `pixiv-client`、`zip`、`gif`、`governor`、`tokio` | `.part`/Range 包装、429 分类 | `gallery-dl`、`pixiv3-rs` | `apalis`、旧 `pixiv` crate |
| `I2` 打标规则 | 现有 sidecar、`regex`、`globset` | `SidecarTaggingBackend` 适配 | ComfyUI WD14 节点 | `rhai`、`ort`（暂） |
| `W2` 搜索 | SQLite FTS5、keyset 游标 | facets 聚合 | PhotoPrism 搜索语法 | `tantivy` |
| `W3a` ApiSource | 现有 `WallSource` | `ApiSource` 实现 | — | `react-query`/`swr` |
| `W4` 内部接口 | `hmac`、`sha2`、`constant_time_eq`、`base64`、`rand` | 设备令牌哈希 | — | `argon2`（非必需） |
| `I3` 推送 | 复用 ingest HTTP client | 签名 URL | — | 分布式队列 |
| `P5` 桥接 | 桌面现有 `reqwest 0.12` | 后台线程 + channel | — | 新 async runtime |
| `W5/I5` 部署 | Nginx/Caddy、systemd、`tracing` | 备份脚本 | — | Prometheus 全家桶 |

## 12. 必须解决的冲突

| # | 冲突 | 影响 | 处理 |
|---|---|---|---|
| C1 | `reqwest` 0.12（工作区） vs 0.13（`pixiv-client`） | 依赖图膨胀/编译冲突 | `kanalib-ingest` 隔离；不让 0.13 进桌面 |
| C2 | `pixiv-client` 2.0.0 很新（2026-08），下载量低 | API 稳定性未知 | 适配层包一层 `SourceAdapter`；锁定精确版本；保留 raw JSON fallback |
| C3 | `gallery-dl` GPL-2.0 | 许可污染 | 只读设计；不复制提取器代码 |
| C4 | Pixiv-Archive GPL-3.0 | 许可污染 | 只借鉴字段/流程；代码重写 |
| C5 | WD14 模型许可未核实 | 分发风险 | 继续用现有本地模型；新增模型单独核 |
| C6 | `ort` 仍是 rc | 运行时/ABI 风险 | 暂不用；sidecar 已够 |
| C7 | `rusqlite` 0.32 vs 最新 0.41 | 版本落后 | 新服务可独立用 0.32 与桌面一致；升级是独立任务 |

## 13. 落地顺序建议

1. **`I0` 先接 `pixiv-client` 的 mock/只读端点**：验证 refresh token、收藏分页、`illust_detail` 字段。
2. **`I1` 再写 `.part`/Range/ugoira**：`zip` + `gif` 先跑通单作品。
3. **`W1` 与 `I0` 并行**：`axum` 骨架 + 临时 SQLite，不依赖 Pixiv。
4. **`I2` 复用 sidecar**：先 mock 标签，再接真实模型；规则用 Rust 谓词。
5. **`W4`/`I3` 最后接签名**：`hmac` + 幂等表 + 设备令牌。

## 14. 待确认

| # | 问题 | 推荐 |
|---|---|---|
| R-Q1 | 是否接受 `kanalib-ingest` 引入 `reqwest 0.13` | 接受；隔离在独立 crate |
| R-Q2 | `pixiv-client` 是否覆盖收藏订阅的 `max_id` 翻页 | 先做 1 天 spike 验证；不覆盖则用 raw HTTP 补 |
| R-Q3 | 是否继续用现有 sidecar 模型 | 是；先跑通再接规则 |
| R-Q4 | 审核页用 HTMX 还是 React | HTMX/纯 HTML；只走 EasyTier 私网 |
| R-Q5 | 设备令牌是否需要 argon2 | 不需要；高熵随机串 + `blake3` 足够 |

## 15. 参考

- `pixiv-client` 2.0.0：<https://crates.io/crates/pixiv-client>
- `pixiv3-rs` 0.1.0：<https://crates.io/crates/pixiv3-rs>
- `pixivcrab` 0.5.2：<https://crates.io/crates/pixivcrab>
- `gallery-dl`（GPL-2.0）：<https://pypi.org/project/gallery-dl/>
- `axum`：<https://crates.io/crates/axum>
- `tower-http`：<https://crates.io/crates/tower-http>
- `rusqlite`：<https://crates.io/crates/rusqlite>
- `governor`：<https://crates.io/crates/governor>
- `hmac`：<https://crates.io/crates/hmac>
- `constant_time_eq`：<https://crates.io/crates/constant_time_eq>
- `gif`：<https://crates.io/crates/gif>
- `zip`：<https://crates.io/crates/zip>
- `tracing-subscriber`：<https://crates.io/crates/tracing-subscriber>
- `mime_guess`：<https://crates.io/crates/mime_guess>
- `argon2`：<https://crates.io/crates/argon2>
- `rhai`：<https://crates.io/crates/rhai>
- `ort`：<https://crates.io/crates/ort>
- Python `onnxruntime`：<https://pypi.org/project/onnxruntime/>
- 本地打标 sidecar：`sidecar/tagger/server.py`
