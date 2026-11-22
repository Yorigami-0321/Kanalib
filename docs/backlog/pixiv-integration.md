# Pixiv 归档能力合并计划（v2，调研稿）

> 登记：[F-084–F-090](registry.md) · 侧栏：[web-rail-horizontal.md](web-rail-horizontal.md) · 网页轻量后端：[web-dynamic-backend.md](web-dynamic-backend.md) · 入库过滤服务：[ingest-service.md](ingest-service.md) · 三端互联：[easytier-connectivity.md](easytier-connectivity.md)
>
> 并行拆分：[execution-workstreams.md](execution-workstreams.md)（P0 契约闸门 → P1–P5 车道、文件所有权与派发顺序）。
>
> 组件选型：[p2-reuse-research.md](p2-reuse-research.md)（P2 之后的 Pixiv 客户端、网页后端、打标、签名与队列复用结论）。
>
> 状态：**只做调研与计划，不改代码**。2026-11-21 用户指示：
> 1. 网页侧栏文字全部改横排，不要竖排；其余按推荐方案。
> 2. 保留 Pixiv API 接口，但**所有外部来源图片必须经独立「入库过滤服务」中转，不能直接接入网页**。
> 3. **不做统一后端**：网页在 2c2g 轻量云上使用自己的轻量后端；桌面端现有功能不拆分，继续作为工作台。
> 4. 新服务部署在 box 上，负责拉取 Pixiv 等来源、调用打标模型、按标签黑名单与规则过滤，只把通过的图交给网页。
> 5. 桌面端可选从网页拉取图片到本地处理，再把处理后的图片、标签等数据回传网页。
>
> 用户明确排除：**不做数据迁移**（不导入旧版/Pixiv-Archive 的数据库、缩略图包或元数据包）。

## 0. 结论摘要

| 问题 | 结论 |
|---|---|
| 参考项目能不能直接抄代码 | **不能**。参考仓库是 **GPL-3.0**，Kanalib 是 MIT；只借鉴流程与字段设计，代码全部按本项目接口重写 |
| 整体架构 | **三块，不合并**：box「入库过滤服务」→ 2c2g 云「网页轻量后端」→ 浏览器；桌面工作台独立，可选双向桥接。三台机器之间走 EasyTier 私网 |
| 外部来源怎么进网页 | **只能经入库过滤服务**。网页后端不持有 Pixiv 令牌、不直连 Pixiv、不接收未过滤条目 |
| 桌面端怎么变 | **不变**。现有扫描、打标、相册、回收站、发布器全部保留；本计划不要求桌面 schema 升级，也不拆分现有功能 |
| 网页后端怎么变 | 从静态 `index.json` 改为**自己的轻量只读 API + SQLite**；只存过滤通过的条目与缩略图，不读桌面数据库 |
| 过滤怎么做 | 打标模型产出标签 → 黑名单/规则引擎 → `allow / block / review`；**fail-closed**：打标失败进 review，不进网页 |
| 原图放哪 | 缩略图（t256/t512）放网页云；原图留在 box，网页按需用签名 URL 回源，避免 2c2g 磁盘/带宽爆掉 |
| 网页侧栏 | 120px 竖排「缝」改为 **208px 横向文字导航**（图标 + 横排文字，可折叠 60px）；≤760 底部横向条 |
| 三端怎么连 | **公网只读走 HTTPS；写接口/同步/原图走 EasyTier 私网**；不要求访客装客户端 |
| 先做哪一步 | P1 侧栏可独立先落；P2 入库服务骨架；P3 打标过滤；P4 网页轻量后端；P5 桌面桥接可选 |

## 1. 调研

### 1.1 参考项目事实（2026-11-21，`main`）

- 形态：Tauri 2 + React 19 + TypeScript + SQLite；Rust 侧按 `scan/sync/subs/download/thumbnails/artworks/browse/favorites` 拆分。
- 许可：**GPL-3.0**。本项目 MIT，**禁止粘贴其源码**；字段名、流程、状态机属于设计事实，可独立重写。
- 数据模型：`authors`、`artworks`、`tags`、`artwork_tags`、`images`、`download_queue`、`favorites`、`favorite_artworks`、`bookmark_subs`。
- 作品 ID：`artworks.pixiv_id` 为正数；认不出 Pixiv ID 的本地文件用哈希派生负数 ID。
- 扫描识别四种命名：`[标题] 12345678_p3.jpg`、`12345678p2.jpg`、`[作者] 112290978.png`、`[作者] 138838560/164.jpg`；只认 **7–10 位**连续数字。
- 元数据同步：增量补「没同步过/上次失败/到重试时间」，全量跳过已删除；404 保留本地元数据只标 `deleted`；错误分类 `auth/not_found/rate_limited/network/internal` 并按退避写 `next_retry_at`。
- 收藏订阅：按用户 ID 存 `last_pid`；翻公开收藏页，**整页都已入库就提前停**；新作品入下载队列。
- 下载可靠：`.part`、Range 续传、成功才 rename；多页并发；部分成功先入库，下次补缺页；重启恢复。
- 动图：ugoira zip → 逐帧解码 → 无限循环 GIF，帧数上限 500。
- 前端：224px 可折叠侧栏，图标 + **横排文字**；六个页面（画廊/作者/标签/收藏夹/收藏订阅/本地作品）。

### 1.2 本项目现状

| 层 | 现状 | 本次怎么处理 |
|---|---|---|
| 桌面 | GPUI + `kanalib-ui`；扫描、打标、相似、去重、相册、回收站、发布器 | **原样保留**，不拆分、不强制升级 schema |
| 核心 | `kanalib-core` schema v11；`assets/tags/asset_tags/collections/...` | 现有能力继续服务桌面；不把网页需求塞进桌面库 |
| 发布 | `crates/kanalib-publish`：静态 `index.json` + 分片 + 四档缩略图 + 原图 | 保留为静态兜底；网页动态后端复用同一 item 形状 |
| 网页 | `web/` Vite + React 19，`StaticSource` 拉分片 | 新增 `ApiSource`；侧栏改横排；搜索/筛选走网页后端 |
| 部署 | 静态站已在 2c2g 轻量云 | 改为同机轻量后端 + SPA；不把桌面 DB 搬上云；公网 listener 只读 |
| 新服务 | 无 | box 上新增「入库过滤服务」，唯一持有 Pixiv 凭据 |
| 互联 | 无 | EasyTier 虚拟网：web/box/桌面；私网名字用魔法 DNS `*.et.net`，迁移不写死 IP |

### 1.3 可复用 / 需新建 / 冲突

**可直接复用**

- 桌面的扫描、哈希、EXIF、缩略图、监听、忽略规则、回收站、撤销、FTS。
- `kanalib-publish` 的 item 字段、id 规则、四档缩略图口径；网页后端只换数据来源，不改视觉契约。
- `collections`/`collection_assets` 继续承载桌面相册；网页收藏夹用自己的表，不跨库复用。

**需要新建**

- `crates/kanalib-ingest`：box 上的入库过滤服务（Pixiv 适配、下载队列、作品/作者、打标、规则、审核、推送）。
- `crates/kanalib-web`：2c2g 云上的网页轻量后端（独立 SQLite、只读 API、媒体、SPA 托管、桌面桥接）。
- `crates/kanalib-wall-model`：box、网页、桌面三端共用的 DTO/契约，无 IO；不依赖桌面 `kanalib-core` 的 DB。
- 网页端 `ApiSource`、作者/作品/收藏夹页、横向导航。
- 桌面桥接客户端（可选）：拉取、回传；不改现有桌面功能。

**语义冲突（已定）**

| 冲突 | 处理 |
|---|---|
| 参考项目「作品」是主表，Kanalib 桌面「资产」是主表 | 网页/入库服务用作品层；桌面仍是资产层，不强制引入作品表 |
| 桌面 schema v11 vs 新服务 schema | **各自独立**：桌面不动；box/网页各自迁移，不共享 DB 文件 |
| 桌面打标 vs box 打标 | 桌面现有打标继续用；box 打标是**独立管线**，只服务外部来源入库 |
| 网页收藏夹 vs 桌面相册 | 两套数据；桌面桥接只回传显式选择的收藏/标签，不自动双向同步 |
| `#/a/<id>` 已是详情别名 | 作者路由用 `#/u/<id>`，不能占用 `#/a/` |
| 数据迁移 | **不做**；不导入旧库、不解析 Pixiv-Archive 数据包 |

## 2. 合并原则

1. **外部来源只有一个入口**：Pixiv 等网络来源的图片、元数据、标签只能先进 box 入库过滤服务；网页后端不直连来源。
2. **网页只收过滤结果**：网页后端只接受 `decision=allow` 的条目；`block` 永不出现，`review` 留在 box 审核队列。
3. **桌面不拆分**：现有桌面功能、数据目录、schema、快捷键、发布器全部保留；桥接是可选增量。
4. **各自的数据主权**：box 管来源/队列/规则/审核；网页管已发布目录/标签/收藏；桌面管本地工作副本。
5. **一个条目契约**：box → 网页、网页 → 前端、静态导出共用同一 item 形状；新增字段全部可选。
6. **fail-closed**：打标超时、模型不可用、规则异常、签名失败都不得把条目放进网页。
7. **可回退**：静态导出与 `StaticSource` 保留；网页后端故障时可切回静态数据。
8. **只借鉴设计**：参考项目 GPL-3.0，代码重写，不复制实现。

## 3. 架构与数据流

```text
┌──────────────┐     Pixiv API / 未来来源
│  box         │◄──────────────────────────
│  kanalib-    │  1. 拉取元数据 / 下载原图
│  ingest      │  2. 调用打标模型
│              │  3. 标签黑名单 + 规则
│              │  4. allow / block / review
└──────┬───────┘
       │ 5. 只推送 allow（签名批次 + 缩略图）
       ▼
┌──────────────────────┐
│  2c2g 轻量云          │
│  kanalib-web         │  6. 独立 SQLite 目录
│  + web/dist          │  7. 只读 API / 媒体 / SPA
└──────┬───────────────┘
       │ 8. HTTPS
       ▼
┌──────────────┐        ┌────────────────────┐
│  浏览器       │        │  桌面工作台（本地）  │
│  React SPA   │        │  GPUI，现有功能不变  │
└──────────────┘        └─────────┬──────────┘
                                  │ 9. 可选拉取 / 回传
                                  └──────────────► 网页后端
```

**信任边界**

- Pixiv refresh token 只存在 box；网页后端和浏览器都没有来源凭据。
- box → 网页用 HMAC 签名 + 时间戳 + 幂等键；网页不提供「让服务器代拉任意 URL」的接口。
- 桌面 → 网页用设备令牌（可撤销、最小权限）；网页 → 桌面不主动连接，桌面轮询或手动拉取。
- **公网 listener 只读**；写接口只挂在 EasyTier 私网 listener 上，公网直接 404。
- 桌面从 box 拉原图走 P2P 直连，不经过 2c2g 云；原图默认不出私网。
- 浏览器访问主站不要求安装 EasyTier；EasyTier 只服务机器间链路。

> 拓扑、ACL、流量账与迁移：见 [easytier-connectivity.md](easytier-connectivity.md)。

**数据归属**

| 数据 | box ingest | 网页后端 | 桌面 |
|---|---|---|---|
| 来源账号/令牌 | 唯一持有 | 无 | 无 |
| 原始元数据 / 审核证据 | 保留 | 只存通过项的必要字段 | 可选 |
| 下载队列 / 失败重试 | 保留 | 无 | 无 |
| 作品/作者/标签（已发布） | 可保留 | 权威 | 可选导入 |
| 原图 | 保留 | 不默认存 | 拉取后本地 |
| 缩略图 | 生成 | 存 t256/t512 | 本地生成 |
| 桌面本地编辑 | 不感知 | 接收回传 | 权威 |
| 对外地址 | 不适用 | 公网域名（只读）+ `*.et.net`（私网） | 魔法 DNS |

## 4. 入库过滤服务（box）

> 详细设计：[ingest-service.md](ingest-service.md)。本节只定职责与边界。

### 4.1 职责

1. **来源适配**：Pixiv 收藏订阅、单作品补拉、作者作品；未来可加其他来源。接口固定为 `SourceAdapter`，新增来源不改过滤/推送主流程。
2. **可靠下载**：`.part`、Range、多页并发、失败退避、重启恢复；ugoira → GIF。
3. **元数据归一**：作品、页、作者、标签、发布日期、AI 标记、原文/翻译标签。
4. **打标**：把图片交给模型（本地 sidecar 或 HTTP 模型服务），拿到标签与分数。
5. **过滤**：**黑名单制**——用户黑名单、标签黑名单（归一化精确匹配）、R-18G 硬拦；**R-18 与 AI 生成放行**；标签不足 5 个交打标模型补标后再判；输出 `allow / block / review`。规则写在 box 配置文件里，见 [ingest-service.md](ingest-service.md) §7。
6. **审核**：`review` 队列可在 box 管理页逐条放行/拒绝；规则命中原因必须可见。
7. **推送**：只把 `allow` 批量推给网页后端；失败重试、幂等、可重放。
8. **私网出口**：管理页与签名媒体只走 EasyTier 虚拟网；不默认开公网入站。

### 4.2 关键约束

- **不得**把未过滤条目直接写网页，也不得把 Pixiv 令牌下发给网页。
- 打标失败/超时 = `review`，不是 `allow`；模型不可用时整批暂停并告警。
- 规则版本化：每次决策记录 `rules_version` 与命中规则，便于回溯。
- 分级口径：**只拦 R-18G**（`x_restrict=2` / 标签 `R-18G` / `restriction_attributes` 三路任一）；R-18 与 AI 生成不拦，配置里显式写 `allow`。
- 删除/下架：来源 404 只标记 `deleted`，网页收到 `deleted` 后隐藏但不丢本地审核记录。
- 媒体：原图留 box；推送缩略图与必要元数据；网页需要原图时用短期签名 URL 回源。**桌面拉原图优先直连 box，不走云。**

### 4.3 与桌面的关系

- 桌面**不参与**这条外部来源管线；桌面现有 Pixiv 无关功能不受影响。
- 桌面可选「从网页拉取」：把网页已发布条目下载到本地来源，走现有扫描/打标/整理。
- 桌面可选「回传」：把本地处理后的标签、星级、收藏、相册成员回传网页；默认关闭，需显式开启。
- 桌面 ↔ 网页、桌面 ↔ box 的链路走 EasyTier 私网；设备离线时显示明确错误，不静默丢队列。

## 5. 网页轻量后端（2c2g）

> 详细设计：[web-dynamic-backend.md](web-dynamic-backend.md)。

### 5.1 形态

- 新增 `crates/kanalib-web`，独立二进制；**不读桌面 DB**，不依赖 GPUI，不跑打标模型。
- `axum + tokio + rusqlite`；SQLite WAL；单机部署，内存目标 <150MB，启动 <1s。
- 同时托管 `web/dist` 静态文件；`/api/*` 与 `/media/*` 同源，避免 CORS。
- **两个 listener**：public 只读（绑回环给 Nginx）；private 绑 EasyTier 虚拟 IP，暴露写接口与桌面桥接。

### 5.2 数据

- 只存 `allow` 条目：`items`、`works`、`authors`、`tags`、`item_tags`、`collections`、`ingest_batches`、`desktop_devices`。
- 不存 Pixiv 令牌、不存下载队列、不存未审核原图。
- 缩略图存 `t256/t512`；`t1024/t2048` 可选；原图默认不落云，按需签名回源 box。

### 5.3 接口

- 公开只读（public listener）：`/api/health`、`/api/index`、`/api/items`、`/api/items/{id}`、`/api/works`、`/api/authors`、`/api/collections`、`/api/tags`、`/media/*`。
- 内部写入（private listener）：`POST /internal/ingest/batch`（HMAC + 时间戳 + 幂等键）；`POST /internal/ingest/delete`。
- 桌面桥接（private listener）：`GET /api/desktop/pull`、`POST /api/desktop/push`（设备令牌）；上传图片/数据也走 private。
- 动态 `schema` 仍为 **1**；能力放 `capabilities`，不靠升 schema 区分静态/动态。

## 6. 网页前端

- 侧栏：120px 竖排「缝」→ 208px 横排文字；折叠 60px；≤760 底部横向条。详见 [web-rail-horizontal.md](web-rail-horizontal.md)。
- 数据源：`StaticSource` 保留；新增 `ApiSource`，`VITE_WALL_SOURCE=api|static` 切换；默认 static，云上部署用 api。
- 搜索/筛选/排序/计数：api 模式走网页后端；static 模式保持现有浏览器侧逻辑。
- 新路由：`#/authors`、`#/u/<id>`、`#/works/<id>`、`#/collections`、`#/c/<id>`；**不使用 `#/a/`**。
- 作品详情：页条、翻页、作者/标签跳转；原图按需回源 box。

## 7. 桌面工作台桥接（可选，默认关闭）

- **不改现有功能**：扫描、打标、相似、去重、相册、回收站、发布器、快捷键全部保持。
- **拉取**：网页 `GET /api/desktop/pull?since=` 返回条目与媒体 URL；桌面下载到独立 `bridge_root`，再走现有导入/扫描。
- **原图直连**：桌面需要原图时优先从 box 的私网媒体入口拉取，不经过 2c2g 云。
- **回传**：桌面把选中的标签、星级、收藏、相册成员 `POST /api/desktop/push`；网页按 `item_id + revision` 合并。
- **上传**：桌面可把本地图片/元数据 `POST /api/desktop/upload` 到网页；走 private listener，设备令牌 + ACL。
- **冲突**：默认 last-write-wins + `updated_at`；同一字段双向都改过则进网页「冲突」列表，不静默覆盖。
- **不做**：不自动同步整个桌面库、不把桌面 schema 改成网页 schema、不把桌面变成网页从属。

## 8. 截图功能逐条映射

| 截图能力 | 落在哪 | 做法 |
|---|---|---|
| 扫描入库：选目录、识别 Pixiv ID、建库、缩略图 | 桌面（本地目录） + box（外部来源） | 外部来源由 box 适配器入库；桌面现有扫描不变，不新增 Pixiv 导入管线，本地文件仍按普通资产入库 |
| 作品详情：翻页、缩放、拖拽、收藏、删除、作者/标签跳转 | 网页 | 网页作品页/详情；删除只对网页下架，不物理删源；桌面不新增作品详情层 |
| 作者/标签/收藏夹：聚合、空作者消失、自由归类 | 网页 | 网页 `works/authors/collections`；标签黑名单在入库服务先过滤 |
| 元数据同步：标题/作者/标签/日期、改名/头像、已删除标记 | box 入库服务 | Pixiv 客户端只在 box；404 保留元数据标 deleted；网页只收结果 |
| 收藏订阅：用户 ID、自动下载、入库/缩略图/元数据、GIF | box 入库服务 | 公开收藏分页 + 快速追平 + 下载队列 + ugoira；过滤通过才推送 |
| 下载可靠：断点续传、失败重试、队列持久化、多页并发 | box 入库服务 | `.part`/Range/退避/重启恢复；状态在 box 管理页可见 |
| 并行任务：同步与订阅同时跑 | box 入库服务 | 两个 worker + 共享 API 节流器/图片信号量；DB 短事务 WAL |
| 作者头像：自动下载、批量更新 | box 入库服务 | 头像缓存；URL 变化才重下；网页只收最终头像 |
| 数据迁移：打包/导入旧版 | **不做** | 不导入旧库/第三方包；只做各服务自己的 schema 升级 |

## 9. 模块与文件落点（计划）

### 9.1 box 入库服务

| 文件 | 计划 |
|---|---|
| `crates/kanalib-ingest/src/main.rs` | 启动、配置、优雅退出、管理端口 |
| `crates/kanalib-ingest/src/source/mod.rs` | `SourceAdapter` trait、来源注册 |
| `crates/kanalib-ingest/src/source/pixiv.rs` | OAuth、签名、节流、错误分类、收藏/作品/作者 |
| `crates/kanalib-ingest/src/download.rs` | 队列、`.part`、Range、多页并发、重启恢复 |
| `crates/kanalib-ingest/src/ugoira.rs` | zip → GIF |
| `crates/kanalib-ingest/src/tagging.rs` | 模型后端（sidecar/HTTP）、超时、重试、批量 |
| `crates/kanalib-ingest/src/rules.rs` | 黑名单、规则 DSL、决策、版本 |
| `crates/kanalib-ingest/src/review.rs` | review 队列、放行/拒绝 |
| `crates/kanalib-ingest/src/push.rs` | HMAC 批次、幂等、重试、删除同步 |
| `crates/kanalib-ingest/src/db.rs` | 服务自己的 SQLite schema |

### 9.2 网页轻量后端

| 文件 | 计划 |
|---|---|
| `crates/kanalib-web/src/main.rs` | 配置、public/private 双 listener、静态托管、优雅退出 |
| `crates/kanalib-web/src/routes/read.rs` | index/items/works/authors/collections/tags |
| `crates/kanalib-web/src/routes/ingest.rs` | 内部批次/删除，HMAC 校验 |
| `crates/kanalib-web/src/routes/desktop.rs` | pull/push/upload、设备令牌 |
| `crates/kanalib-web/src/media.rs` | t256/t512、ETag、Range、签名回源 |
| `crates/kanalib-web/src/db.rs` | 网页目录 schema、迁移、备份 |

### 9.3 共用契约

| 文件 | 计划 |
|---|---|
| `crates/kanalib-wall-model/src/lib.rs` | `WallItem`、`Work`、`Author`、`Collection`、`Page<T>`、`Capabilities` |
| `crates/kanalib-wall-model/src/convert.rs` | 内部记录 → DTO；无 IO |

### 9.4 网页

| 文件 | 计划 |
|---|---|
| `web/src/components/Sidebar.tsx` | 横排文字导航；保留 `.seam` 兼容类直到探针改完 |
| `web/src/source/ApiSource.ts` | 实现 `WallSource`；分页/详情/媒体 |
| `web/src/source/index.ts` | `createSource()` 按 `VITE_WALL_SOURCE` 选择 |
| `web/src/state/useWall.ts` | 增加 `reset()`；api 模式先 `setQuery` 再拉第一页 |
| `web/src/lib/router.ts` | 作者/作品/收藏夹路由 |

### 9.5 桌面桥接（可选）

| 文件 | 计划 |
|---|---|
| `crates/kanalib-bridge`（独立 crate）或桌面内独立模块 | pull/push 客户端、令牌、冲突处理；不改现有扫描/打标路径 |
| `crates/kanalib-ui/src/settings.rs` | 桥接开关、服务器地址、设备令牌、拉取目录 |

## 10. 阶段计划

> 顺序按依赖排，不写工时。P1 可独立先落；P2 的条目契约必须先于 P3/P4。

### P0 决策与文档（本任务）

- 产出：本文 + 侧栏 + 网页后端 + 入库服务 + registry/overview 登记。
- 退出：用户确认 §13 决策；文档自检通过。

### P1 网页侧栏横向化

- 范围：只改 `web/` 导航层，不动数据与后端。
- 任务：`Sidebar.tsx`、`sidebar.css`、`styles.css`、`layout.ts`；保留 `.seam`/`--seam-w` 兼容。
- 退出：1440/1280/1024/900/760/390 六档截图；折叠持久化；现有 `wall_*` 探针通过；无横向溢出。

### P2 条目契约与入库服务骨架

- 范围：`kanalib-wall-model`、`kanalib-ingest` 骨架、Pixiv 适配、下载队列、作品/作者。
- 任务：来源接口、OAuth、节流、`.part`/Range、ugoira、服务 schema、健康检查。
- 退出：mock Pixiv 全链路；下载中断恢复；作品/页/作者正确；**暂不推送网页**。

### P3 打标与过滤

- 范围：`tagging.rs`、`rules.rs`、`review.rs`、管理页。
- 任务：模型后端、超时、黑名单/规则、**标签不足补标**、决策记录、review 放行/拒绝、fail-closed。
- 退出：黑名单命中必 block；模型失败必 review；规则版本可追溯；无未过滤条目外发。

### P4 网页轻量后端与动态化

- 范围：`kanalib-web`、内部 ingest 接口、`ApiSource`、作者/作品/收藏夹页、媒体。
- 任务：独立 SQLite、只读 API、HMAC 批次、缩略图、签名回源、SPA 托管。
- 退出：2c2g 压测达标；只出现 allow 条目；静态/动态同形状；网页 build + 探针通过。

### P5 桌面桥接（可选）

- 范围：pull/push、设备令牌、冲突列表；不改现有桌面功能。
- 退出：拉取到独立目录并可扫描；回传标签/星级/收藏；冲突可见；关闭开关后零影响。

### P6 运维与硬化

- 范围：备份、日志、限流、监控、部署脚本、回滚、EasyTier 接入。
- 退出：box/云两侧可独立重启；推送重放不重复；密钥轮换；告警可到达；公网写路由 404；私网内 `DIRECT`。

## 11. 验收与测试

### 11.1 入库服务

- `cargo test -p kanalib-ingest`：解析、队列状态机、Range、ugoira、规则、决策、幂等推送。
- mock 来源：收藏 2 页、第 2 页全已入库 → 快速追平；404 → deleted；429 → 退避；auth → 整批暂停。
- 过滤：黑名单命中 block；模型超时 review；规则异常 review；任何失败都不得产生 allow。
- 推送：断网重试、重复批次不重复入库、删除同步、签名过期拒绝。

### 11.2 网页后端

- `cargo test -p kanalib-web`：只读 API、分页/游标、facets、媒体 200/206/304/404/416、路径穿越。
- 内部接口：无签名 401、时间戳过期 401、重复幂等键 200 且不重复。
- 桌面接口：无令牌 401、撤销令牌 401、push 冲突可见。
- 网络边界：public listener 对 `/internal/*`、`/api/desktop/*` 返回 404；private listener 只绑回环。
- 容量：2c2g 上 32 并发读、10k 条目 p95 目标 ≤120ms；内存 <150MB。

### 11.3 网页

- `npm run typecheck && npm run build`（api/static 各一次）。
- 现有 `wall_*` 探针 + 新 `wall_probe_rail.mjs`、`wall_api_check.mjs`。
- 断网/后端 503：错误条可重试；不清空已加载；可切回静态。

### 11.4 不做

- 不联真实 Pixiv 做 CI；真实令牌只做手动冒烟，不写入仓库。
- 不在本任务跑构建/测试（只允许文档改动）。
- 不用合成事件宣称真实触控板/键盘通过。

## 12. 风险与合规

| 风险 | 影响 | 缓解 |
|---|---|---|
| 参考项目 GPL-3.0 | 污染 MIT 仓库 | 只读调研；独立重写；不复制注释/结构体/函数体 |
| Pixiv 服务条款与版权 | 账号/再分发风险 | 令牌只在 box；默认只处理公开收藏；下载需显式开启；文档标注自担 |
| 未过滤内容进网页 | 合规/信任事故 | 网页只收 allow；fail-closed；内部接口 HMAC；审计批次 |
| box 被攻破 | 令牌/原图泄露 | 最小权限、磁盘加密、令牌 keyring、管理口只回环/VPN |
| 2c2g 容量不足 | 网页慢/挂 | 只存缩略图；原图回源；SQLite WAL；静态资源 CDN；限流 |
| 推送重复/乱序 | 目录不一致 | 幂等键 + `revision` + 批次游标；删除单独事件 |
| 桌面桥接冲突 | 本地/网页互相覆盖 | 默认关闭；last-write-wins + 冲突列表；不自动全库同步 |
| 媒体路径穿越 | 任意文件读取 | id 正则 + DB 反查 + canonicalize + 根前缀 |
| 备份缺失 | 数据丢失 | 两侧每日备份；推送批次可重放；原图在 box 有副本 |
| 动图 GIF 体积 | 带宽 | 帧数上限；尺寸取首帧；后续评估 WebP/MP4 |
| EasyTier 不可用 | 私网同步/原图中断 | 公网只读不受影响；重试；必要时 frp/公网 HMAC 回退 |
| 误把写接口暴露公网 | 未授权写入 | public/private 两个 listener；公网 404 + 反代拒绝 |

## 13. 已定决策（全部按推荐）

| # | 问题 | 结论 |
|---|---|---|
| D1 | 侧栏形态 | **浮层 208px + 76% 玻璃**，不推挤画布 |
| D2 | 侧栏展开宽度 | **208px**，折叠 60px |
| D3 | 桌面是否升级 schema | **不升级、不拆分**；桥接用现有 tags/collections |
| D4 | 网页数据来源 | **只接收 box 过滤后的 allow 条目** |
| D5 | Pixiv 凭据放哪 | **只在 box**；网页/浏览器都没有 |
| D6 | 过滤失败策略 | **fail-closed**：review，不进网页 |
| D6a | 过滤规则形态 | **纯黑名单制**，配置文件驱动；用户黑名单 + 标签黑名单 + R-18G 硬拦；R-18 与 AI 放行 |
| D6b | 标签不足阈值 | `min_tags = 5`（只算内容标签，剔除体裁词与 `Nusers入り`）；不足则补标一次后再判 |
| D6c | 规则存哪 | **独立 `rules.toml` 文件**，不建可写规则表；DB 只存只读快照 `rule_snapshots` 与待应用草稿 `rule_proposals` |
| D6d | 管理页改规则 | **不直接改文件**：写草稿 → 人工确认 → `toml_edit` 原子改写 + 版本自增 + hash 比对（[ingest-service.md](ingest-service.md) §7.8） |
| D7 | 媒体存放 | 缩略图在云；原图在 box，签名回源 |
| D8 | 网页后端技术 | **Rust axum + SQLite WAL**，独立二进制 |
| D9 | 静态导出 | **保留**，作为 CDN/离线兜底 |
| D10 | 动图格式 | **GIF** |
| D11 | 作者路由 | **`#/u/<id>`** |
| D12 | 桌面桥接 | **可选、默认关闭**，不改现有功能 |
| D13 | 冲突策略 | last-write-wins + 冲突列表，不静默覆盖 |
| D14 | 部署 | 网页在 2c2g 云；入库服务在 box |
| D15 | 三端互联 | **EasyTier 私网**：写接口/同步/原图；公网只读不要求访客安装 |
| D16 | 原图默认范围 | **仅私网**；公网最多 t2048 |
| D17 | 迁移期私网名字 | **魔法 DNS `*.et.net`**；不写死 `10.126.126.x` |

## 14. 参考

- 参考项目：<https://github.com/Tumber-ZM/Pixiv-Archive>（GPL-3.0，2026-11-21 `main`）。
- 静态契约：[photo-wall-contract.md](photo-wall-contract.md) v1。
- 视觉规格：[photo-wall-v3.md](photo-wall-v3.md) 十一条硬规则。
- 侧栏：[web-rail-horizontal.md](web-rail-horizontal.md)。
- 网页轻量后端：[web-dynamic-backend.md](web-dynamic-backend.md)。
- 入库过滤服务：[ingest-service.md](ingest-service.md)。
- 三端互联：[easytier-connectivity.md](easytier-connectivity.md)。
- 调研准则：[README.md](README.md)。
