# 网页轻量后端与动态化设计（v2）

> 上级：[pixiv-integration.md](pixiv-integration.md) · 侧栏：[web-rail-horizontal.md](web-rail-horizontal.md) · 入库过滤服务：[ingest-service.md](ingest-service.md) · 三端互联：[easytier-connectivity.md](easytier-connectivity.md) · 静态契约：[photo-wall-contract.md](photo-wall-contract.md)
> 组件选型与可复用项目：[p2-reuse-research.md](p2-reuse-research.md)（axum/tower-http、SQLite、FTS5、签名与令牌）。
>
> 2026-11-21 用户指示：**不做统一后端**。网页在 2c2g 轻量云上用自己的轻量后端；外部来源图片只能经 box 入库过滤服务中转；桌面端现有功能不拆分。
>
> 原则：**静态导出不废弃**。动态后端是 `WallSource` 的第二种实现，视觉组件零改动；`App.tsx` 的数据装配层按 `capabilities` 分叉。静态站仍可作为 CDN/离线兜底。
>
> 实现状态（2026-11-22）：`crates/kanalib-wall-model`（W0）、`crates/kanalib-web` W1/W2 与前端 W3a/W3b 已落地。W1 覆盖 `health/index/items/works/authors/collections/tags` 与 `/media/*`（ETag、单区间 Range、档位回退）；W2 覆盖服务端搜索（FTS5 trigram）、筛选、排序、keyset 游标与过滤后 facets；W3a 增加 `ApiSource`/`createSource`，W3b 把页面筛选/排序/计数切到 `setQuery`，`/api/index` 提供 favorite/rating 计数与带封面的标签/分组 facets。public listener 不注册写路由，private listener 仅暴露 `/api/health`。集成测试见 `crates/kanalib-web/tests/web_flow.rs`，验收脚本见 `scripts/wall_api_check.mjs`。

## 0. 结论

| 决策 | 结论 |
|---|---|
| 进程形态 | 独立 `crates/kanalib-web` 二进制，部署在 2c2g 轻量云；**不读桌面 DB**、不依赖 GPUI |
| 数据来源 | 只接收 box 入库过滤服务推送的 `allow` 条目；不直连 Pixiv、不持有来源令牌 |
| 存储 | 自己的 SQLite（WAL）；只存已发布目录、标签、作品/作者、收藏夹、批次、桌面设备 |
| HTTP 栈 | `axum` + `tower-http`（ETag/Range/压缩/静态文件）+ `tokio`；`tokio` 已在锁文件（1.54.1），`axum` 需新增 |
| DTO | 新增 `crates/kanalib-wall-model`，网页后端、入库服务、静态发布器共用；网页端只认这一种形状 |
| 前端 | 新增 `ApiSource`，用 `VITE_WALL_SOURCE=api` 切换；`StaticSource` 保留 |
| 搜索 | api 模式服务端搜索 + keyset 游标；static 模式保留浏览器侧子集 |
| 媒体 | 缩略图同源 `/media/{t256,t512,t1024,t2048}/<id>.jpg`；原图默认不落云，按需签名回源 box |
| 写入 | **public listener 只读**；内部 ingest（HMAC）与桌面设备（设备令牌）只挂在 private listener（EasyTier 私网） |
| 网络 | 公网 HTTPS 只读；写接口/桌面桥接/原图入口走 EasyTier 虚拟网；公网写路由 404 |
| 兼容 | 动态 `/api/index` 的 `schema` 仍为 **1**，能力放 `capabilities`；新增字段全部可选 |

## 1. 为什么独立于桌面

- 桌面端 `AppState` 持有 GPUI 生命周期、watcher、打标线程、缩略图内存缓存；网页后端不应依赖这些。
- 2c2g 云上不能跑桌面全量库：桌面库可能有 10 万条、原图和缓存几十 GB；网页只需要**过滤后的一小部分**。
- 桌面和网页的写入节奏不同：桌面是本地工作台，网页是发布端；共用一个 DB 会把两边的迁移、锁、备份绑死。
- 代价：多一套目录 schema、多一条推送链路；用 `kanalib-wall-model` 固定契约、用幂等批次降低风险。

## 2. 目录与依赖

```text
crates/
  kanalib-wall-model/   # 纯 DTO + 转换，无 IO；web/ingest/publish 共用
  kanalib-web/          # axum 路由 + 媒体 + SQLite + SPA 托管
web/                    # 增加 ApiSource 与动态页面
```

```toml
# crates/kanalib-wall-model/Cargo.toml
[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }

# crates/kanalib-web/Cargo.toml
[dependencies]
anyhow = { workspace = true }
axum = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "signal", "fs"] }
tower-http = { version = "0.6", features = ["cors", "compression-gzip", "etag", "trace", "set-header", "fs"] }
kanalib-wall-model = { path = "../kanalib-wall-model" }
rusqlite = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
clap = { version = "4", features = ["derive"] }
hmac = "0.12"
sha2 = "0.10"
```

- `axum 0.8` 与 `tower-http 0.6` 是新增依赖；`tokio`/`hyper`/`tower` 已在 `Cargo.lock`（传递依赖），版本兼容。
- 不引入 ORM；继续 `rusqlite`，v1 每请求短连接 + WAL，压测后再评估池。
- 不引入 OpenAPI 生成器；契约用本文 + 单测固定。

## 3. 数据模型（网页自己的库）

> 与桌面 schema 无关；`SCHEMA_VERSION` 从 1 开始，迁移前自动备份。所有 `ALTER TABLE` 用 `PRAGMA table_info` 守卫。

### 3.1 表

```sql
CREATE TABLE IF NOT EXISTS items (
    id TEXT PRIMARY KEY,                 -- 40/64 位小写 hex，与静态契约一致
    source TEXT NOT NULL,                -- pixiv | other
    source_id TEXT,                      -- 来源作品 ID
    work_id TEXT,
    title TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL DEFAULT '',
    width INTEGER NOT NULL DEFAULT 0,
    height INTEGER NOT NULL DEFAULT 0,
    rating TEXT NOT NULL DEFAULT 'G',
    star INTEGER NOT NULL DEFAULT 0,
    favorite INTEGER NOT NULL DEFAULT 0,
    mtime INTEGER NOT NULL DEFAULT 0,
    posted_at INTEGER,
    orig_ext TEXT,
    sharp INTEGER NOT NULL DEFAULT 0,
    phash TEXT,
    group_name TEXT NOT NULL DEFAULT '',
    decision TEXT NOT NULL DEFAULT 'allow',  -- 只应出现 allow；保留字段便于审计
    rules_version TEXT,
    revision INTEGER NOT NULL DEFAULT 1,
    deleted INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS works (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    external_id TEXT,
    title TEXT NOT NULL DEFAULT '',
    author_id TEXT,
    page_count INTEGER NOT NULL DEFAULT 1,
    posted_at INTEGER,
    status TEXT NOT NULL DEFAULT 'active',
    cover_item_id TEXT REFERENCES items(id) ON DELETE SET NULL,
    UNIQUE(source, external_id)
);

CREATE TABLE IF NOT EXISTS work_items (
    work_id TEXT NOT NULL REFERENCES works(id) ON DELETE CASCADE,
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    page INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(work_id, page),
    UNIQUE(item_id)
);

CREATE TABLE IF NOT EXISTS authors (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    external_id TEXT,
    name TEXT NOT NULL DEFAULT '',
    avatar_url TEXT,
    updated_at INTEGER,
    UNIQUE(source, external_id)
);

CREATE TABLE IF NOT EXISTS tags (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT '',
    translated_name TEXT,
    UNIQUE(name)
);

CREATE TABLE IF NOT EXISTS item_tags (
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    score REAL NOT NULL DEFAULT 0,
    origin TEXT NOT NULL DEFAULT 'ingest',  -- ingest | manual | desktop
    PRIMARY KEY(item_id, tag_id, origin)
);

CREATE TABLE IF NOT EXISTS collections (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'favorite', -- favorite | manual
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS collection_items (
    collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    PRIMARY KEY(collection_id, item_id)
);

CREATE TABLE IF NOT EXISTS ingest_batches (
    id TEXT PRIMARY KEY,                 -- 幂等键
    source TEXT NOT NULL,
    item_count INTEGER NOT NULL DEFAULT 0,
    rules_version TEXT,
    received_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS desktop_devices (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    token_hash TEXT NOT NULL,
    can_pull INTEGER NOT NULL DEFAULT 1,
    can_push INTEGER NOT NULL DEFAULT 1,
    revoked INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    last_seen_at INTEGER
);
```

### 3.2 索引

```sql
CREATE INDEX IF NOT EXISTS idx_items_updated ON items(deleted, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_items_work ON items(work_id);
CREATE INDEX IF NOT EXISTS idx_works_author ON works(author_id);
CREATE INDEX IF NOT EXISTS idx_item_tags_tag ON item_tags(tag_id);
CREATE INDEX IF NOT EXISTS idx_collection_items_item ON collection_items(item_id);
```

### 3.3 不变量

1. `items.decision` 只写 `allow`；`block`/`review` 不进入网页库。收到非 `allow` 批次直接 400。
2. 同一 `id` 的推送必须幂等；`revision` 只增不减。
3. `works.cover_item_id` 必须是成员；为空时取最小 `page`。
4. 删除只置 `deleted=1`，不物理删行；媒体文件可延迟清理。
5. 标签按 `name` 唯一；`item_tags` 主键含 `origin`，桌面回传不覆盖 ingest 标签。

## 4. DTO（`kanalib-wall-model`）

### 4.1 条目形状

静态 `WallItem` 字段（`id/w/h/n/rt/st/fav/tags/g/ph/mtime/orig/sharp`）**原样保留**；动态 DTO 只增加可选字段。网页端 `normalizeItem` 已忽略未知字段，因此静态数据不需要改。

```rust
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WallItemDto {
    pub id: String,
    pub w: u32,
    pub h: u32,
    pub n: String,
    pub rt: String,
    pub st: u8,
    pub fav: bool,
    pub tags: Vec<(String, String, f32)>,
    pub mtime: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub g: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ph: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orig: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sharp: Option<u32>,

    // 可选扩展
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pc: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pg: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub au: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub del: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}
```

```rust
pub struct WorkDto {
    pub id: String,
    pub source: String,
    pub external_id: Option<String>,
    pub title: String,
    pub author: Option<AuthorDto>,
    pub page_count: u32,
    pub posted_at: Option<u64>,
    pub status: String,
    pub tags: Vec<(String, String, f32)>,
    pub pages: Vec<WorkPageDto>,
}

pub struct WorkPageDto {
    pub id: String,
    pub page: u32,
    pub w: u32,
    pub h: u32,
    pub sharp: u32,
    pub orig: Option<String>,
}

pub struct AuthorDto {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    pub work_count: u32,
    pub item_count: u32,
    pub covers: Vec<String>,
}

pub struct CollectionDto {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub item_count: u32,
    pub work_count: u32,
    pub covers: Vec<String>,
}

pub struct PageDto<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
    pub total: Option<u32>,
}

pub struct Capabilities {
    pub source: String,          // static | api
    pub server_search: bool,
    pub server_filter: bool,
    pub can_write: bool,
    pub has_works: bool,
    pub has_authors: bool,
    pub has_collections: bool,
    pub media_tiers: Vec<String>,
    pub original_proxy: bool,    // 原图是否走签名回源
    pub generated_at: Option<String>,
}
```

### 4.2 与静态文件的映射

| 静态 | 动态 |
|---|---|
| `index.json` | `GET /api/index` → `{ schema: 1, api_version: 1, count, facets, paths, capabilities }` |
| `m/NNNN.json` | `GET /api/items?cursor=&limit=` → `PageDto<WallItemDto>` |
| `search-index.json` | 不返回；服务端搜索替代 |
| `t256/<id>.jpg` | `GET /media/t256/<id>.jpg` |
| `o/<id>.<ext>` | 不直接提供；`GET /api/items/{id}/original` 返回 302 到 box 的短期签名 URL |

`schema` 是条目/目录契约版本，不是「静态/动态」开关；动态也返回 1。`api_version` 只描述 HTTP 层。

## 5. HTTP API

### 5.1 公开只读

> 以下全部挂在 **public listener**（`127.0.0.1:8787`，由 Nginx 反代），公网可读；不含任何写路由。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/health` | `{ ok, db, media_root, version }`；DB 不可读返回 503 |
| GET | `/api/index` | 计数（含 favorite/ratings）、facets（标签/分组带封面）、档位、capabilities |
| GET | `/api/items?cursor=&limit=` | 默认 `updated_at` 降序；`limit ≤ 200` |
| GET | `/api/items?q=&tag=&group=&rating=&fav=&sort=&order=&cursor=` | 服务端搜索/筛选 |
| GET | `/api/items/{id}` | 单条；404 明确 |
| GET | `/api/works` | 作品列表（分页） |
| GET | `/api/works/{wid}` | 作品详情 + 页 |
| GET | `/api/authors` | 作者聚合（分页 + `q`） |
| GET | `/api/authors/{auid}` | 作者详情 |
| GET | `/api/authors/{auid}/works` | 作者作品（分页） |
| GET | `/api/collections` | 收藏夹 + 相册 |
| GET | `/api/collections/{id}` | 成员（分页） |
| GET | `/api/collections/{id}/works` | 收藏夹内作品（分页） |
| GET | `/api/tags` | 标签 facets（`limit`） |
| GET | `/media/{tier}/{file}` | 缩略图；`tier ∈ t256,t512,t1024,t2048` |

`/api/items/{id}/original` **不在 public listener**：它只挂 private listener；公网访问返回 404，前端回退到 t2048。

### 5.2 内部写入

> 以下只挂在 **private listener**（`10.126.126.2:8788`），只允许 EasyTier 虚拟网访问。

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/internal/ingest/batch` | box 推送 allow 批次；HMAC + 时间戳 + 幂等键 |
| POST | `/internal/ingest/delete` | box 通知下架/删除；只置 `deleted=1` |
| GET | `/api/items/{id}/original` | 302 到 box 签名 URL；只对 EasyTier 私网可用 |

约束：

1. `X-Kanalib-Timestamp` 与当前时间差 ≤ 5 分钟；`X-Kanalib-Signature` = HMAC-SHA256(body, shared_secret)。
   `shared_secret` 支持逗号分隔的新旧双密钥；轮换时先加新密钥、切 box、再删旧密钥。
2. `X-Kanalib-Batch-Id` 幂等；重复批次返回 200，不重复写。
3. 批次内任何条目 `decision != allow` → 400，整批拒绝。
4. 只允许配置的 box 出口 IP（可选）或共享密钥；不做账号体系。
5. 公网 listener 不注册本组路由；公网请求是 404，不是 401。

批次 body（`revision` 与 `decision` 在条目层，其余字段与 `IngestItem` 平铺）：

```json
{
  "source": "pixiv",
  "rules_version": "2026-11-22.1",
  "items": [
    {
      "revision": 3,
      "decision": "allow",
      "source_id": "200",
      "id": "0123456789abcdef0123456789abcdef01234567",
      "width": 1200, "height": 1600, "name": "001.jpg",
      "rating": "S", "star": 4, "favorite": true,
      "tags": [{"name": "1girl", "category": "general", "score": 0.99}],
      "mtime": 1700000000, "group": "原创", "orig": "jpg", "sharp": 512,
      "work_id": "work-1", "page_count": 2, "page": 0,
      "author": "作者", "author_id": "author-1", "title": "作品标题"
    }
  ]
}
```

- 删除 body：`{ "source": "pixiv", "items": [{ "id": "…", "revision": 4 }] }`；只置 `deleted=1`，`revision` 低于当前值计 `stale`。
- 响应：`{ ok, duplicate, inserted, updated, accepted, stale }`；删除为 `{ ok, duplicate, deleted, stale, missing }`。
- 错误码补充：`missing_batch_id`、`invalid_batch`、`invalid_item`、`non_allow_batch`；未配置共享密钥时 503 `not_configured`。

### 5.3 桌面桥接

> 以下只挂在 **private listener**；桌面通过 EasyTier 虚拟网访问。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/desktop/pull?since=&limit=` | 返回条目 + 媒体 URL；设备令牌 |
| POST | `/api/desktop/push` | 回传标签/星级/收藏/相册成员；设备令牌 |
| POST | `/api/desktop/upload` | 上传本地图片/元数据到网页；设备令牌 |

约束：

1. `Authorization: Bearer <device_token>`；只存哈希，可撤销。
2. push 按 `item_id + revision` 合并；同一字段双向都改过 → 进冲突列表，不静默覆盖。
3. 设备令牌由网页后端生成，桌面手动粘贴；不在公网开放注册。
4. 上传走 multipart/流式；单文件上限可配置；只写网页媒体目录，不碰桌面库。

实现形状：

- `pull?since=<opaque|unix>&limit=≤200`：`{ items: [{ ...WallItemDto, revision, updated_at, media }], next, has_more }`；游标是 `(updated_at, id)` 的 base64url，包含 `deleted=1` 条目。
- `push`：`{ items: [{ id, base_revision, star?, favorite?, rating?, tags?, collections? }] }`；`base_revision` 与服务端当前值不一致即冲突，返回 `{ applied, conflicts: [{ id, base_revision, server }], missing }`。
- `upload`：原始 body 流式写入 `media/uploads/{id}.{ext}.part`，`X-Kanalib-Item-Id` 必填，`X-Kanalib-Ext` 可选；超限 413 `upload_too_large` 并删除 `.part`。
- 设备令牌生成：`kanalib-web --db <path> --issue-device <name>` 打印一次；只存 SHA-256。

### 5.4 查询参数

```text
q         自由词（空格 AND，-词 排除，tag: 限定）
tag       标签名（精确）
group     分组名
rating    G/S/Q/E
fav       1/0
sort      updated|posted|name|star|favorite|rating|random
order     asc|desc
seed      随机种子（sort=random 时）
cursor    不透明游标（keyset）
limit     1–200，默认 60
```

- 未知参数忽略；非法 `sort` 回退 `updated`；`limit` 夹取。
- 游标内容：`{ sort, ascending, key... }` 的 base64url；换排序必须从第一页开始（400）。
- `q` 支持 `tag:`/`name:`/`group:`/`rating:` 字段、引号短语与 `-` 排除；≥3 字符走 FTS5 trigram 子串匹配，短词与排除项退回 `LIKE`。
- 标签筛选用 `EXISTS` 半连接（`item_tags` 允许同一条目多 origin），不物化去重候选集；FTS 是唯一会物化候选 ID 的筛选。
- `/api/items` 只返回 `items` + `next`，**不再逐页 `COUNT`**；`total` 由 `/api/index` 的 `count/counts` 提供，分页端点保持 keyset 的 O(limit) 成本。
- `limit`/`offset` 非整数回 `400 invalid_limit`；游标损坏或与当前 `sort/order` 不一致回 `400 invalid_cursor`。
- `/api/index?<filters>` 返回同一筛选下的 `count`、`counts` 与 `facets`；`capabilities.server_search/server_filter` 为 true。
- `counts = { items, works, authors, collections, favorite, ratings: { g, s, q, e } }`；overview 页的收藏/分级计数直接用它，不再依赖已加载集合。
- `facets.tags = [{ name, category, count, cover }]`，`facets.groups = [{ name, count, cover }]`；`cover` 是该标签/分组下一张代表图 id。静态 `index.json` 的 `groups` 仍是字符串数组，`ApiSource` 会兼容两种形状。

### 5.5 响应头

| 头 | 说明 |
|---|---|
| `ETag` | `W/"<id>-<updated_at>-<tier>"`；媒体与 `/api/items/{id}` 都带 |
| `Cache-Control` | 媒体 `public, max-age=31536000, immutable`；API `no-cache` |
| `Accept-Ranges: bytes` | 媒体 |
| `Content-Range` | 206 时 |
| `X-Content-Type-Options: nosniff` | 全部 |

### 5.6 错误

```json
{ "error": { "code": "not_found", "message": "…" } }
```

- `400 invalid_cursor|invalid_limit|non_allow_batch`；`401 bad_signature|bad_device_token`；`404 not_found`；`416 range_not_satisfiable`；`503 db_unavailable|media_offline`；`500 internal`。
- 不返回绝对路径；日志里才记录。

## 6. 媒体服务

### 6.1 缩略图

1. 校验 `tier` 白名单与 `file` 形状：`^[0-9a-f]{40,64}\.jpg$`。
2. 用 id 反查 DB（`items`），不拼接用户输入路径。
3. `canonicalize` 目标，校验以媒体根为前缀（防 `..`/符号链接逃逸）。
4. 不存在时按档位回退：`t256→t512`、`t1024→t512`、`t2048→t1024→t512`。
5. 支持 `Range`（单区间）与 `HEAD`。

### 6.2 原图回源

- 网页默认不存原图；`GET /api/items/{id}/original` 只挂 private listener，校验条目存在后，用共享媒体签名密钥生成短期签名 URL（默认 5 分钟），返回 302 到 box。
- box 侧只接受签名 + 过期时间；不暴露目录列表。
- 未配置 box 回源时返回 404，前端回退到最高可用缩略图。
- 公网不提供原图；详情页在公网模式最多取 t2048。

签名规范串（网页签、box 验，均用 HMAC-SHA256 + hex）：

```text
GET\n/original/{item_id}\n{expires}
```

- 302 目标：`{box_origin}/original/{item_id}?expires={unix}&sig={hex}`。
- 网页只在 `--box-origin` 与 `--media-signing-key` 同时配置时启用；否则 404。

### 6.3 性能

- `tokio::fs::File` + `ReaderStream`；不整文件读入内存。
- ETag 命中直接 304，不打开文件。
- `compression-gzip` 只用于 JSON，不压缩图片。
- 2c2g 上只存 t256/t512，t1024/t2048 可选；媒体目录单独挂盘。

## 7. 网页端 `ApiSource`

### 7.1 接口实现

现有 `WallSource` 方法：`init/loadMore/items/shardsLoaded/skipped/getItem/index/hasMore/paths`。API 模式需要三个**可选**扩展；不实现时 `StaticSource` 完全不受影响。

```ts
export interface SourceCapabilities {
  source: 'static' | 'api';
  server_search: boolean;
  server_filter: boolean;
  can_write: boolean;
  has_works: boolean;
  has_authors: boolean;
  has_collections: boolean;
  media_tiers: string[];
  original_proxy: boolean;
}

export interface WallQuery {
  q?: string;
  tag?: string;
  group?: string;
  rating?: string;
  fav?: boolean;
  sort?: 'random' | 'time' | 'name' | 'stars';
  order?: 'asc' | 'desc';
  seed?: number;
}

export interface WallSource {
  // …现有方法不变…
  readonly capabilities?: SourceCapabilities;
  loadSearchIndex?(): Promise<SearchIndexData | null>;
  fetchItem?(id: string): Promise<WallItem | undefined>;
  setQuery?(query: WallQuery): Promise<void>;
}
```

```ts
export class ApiSource implements WallSource {
  readonly dataRoot: string;
  capabilities: SourceCapabilities;
  private next: string | null = null;
  private loaded: WallItem[] = [];
  private byId = new Map<string, WallItem>();
  private query: WallQuery = {};

  async init(): Promise<WallIndex> { /* GET /api/index → 映射为 WallIndex，读 capabilities */ }
  async setQuery(query: WallQuery): Promise<void> { /* 清空游标/列表/去重表，记录 query */ }
  async loadMore(): Promise<WallItem[]> { /* GET /api/items?<query>&cursor=… */ }
  async fetchItem(id: string): Promise<WallItem | undefined> { /* GET /api/items/{id} */ }
  getItem(id: string) { return this.byId.get(id); }
  hasMore() { return this.next !== null; }
  paths(): TierPaths { /* /media/t256/ 等 */ }
}
```

- `init` 把 `facets/paths/count/capabilities` 映射到现有 `WallIndex`；`shards` 用虚拟分片（`shards: []` + `hasMore` 由 `next` 决定）。`useWall.totalShards` 在 API 模式会是 0，只用于状态提示。
- `loadMore` 去重、保序、`skipped` 计数；失败不推进游标。
- `fetchItem` 未命中时单条补取；只有 static 模式才继续顺序翻分片。`fetchItem` 内部不得调用 `setQuery` 或 `init`。
- `paths()` 返回媒体前缀，`tierUrl` 无需改。

### 7.2 切换

```ts
// web/src/source/config.ts
const KIND = import.meta.env.VITE_WALL_SOURCE ?? 'static';
export const DATA_ROOT = ...;
export const API_ROOT = import.meta.env.VITE_WALL_API ?? '/api/';
```

```ts
// web/src/source/index.ts
export function createSource(): WallSource {
  return KIND === 'api' ? new ApiSource(API_ROOT) : new StaticSource(DATA_ROOT);
}
```

- 默认仍是 `static`，现有部署不变；云上构建用 `VITE_WALL_SOURCE=api`。
- `api` 模式构建时不需要 `public/data`；`vite build` 产出纯前端，由 `kanalib-web` 托管 `dist/`。
- `ApiSource` 失败时显示错误条 + 「切回静态」提示（仅当同源存在 `./data/index.json`）。

### 7.3 页面层改动

视觉组件（`Wall`/`Detail`/`Inspector`/`TagGallery`/`BucketGallery`）不改；改的是 `App.tsx` 的**数据装配层**。现有实现把 `StaticSource` 直接实例化，并在浏览器里对 `wall.items` 做 `baseItems → filterItems → sortItems`，API 模式不能沿用这条路径。

| 关注点 | static（现状） | api |
|---|---|---|
| 数据源创建 | `new StaticSource(DATA_ROOT)` | `createSource()` 按 `VITE_WALL_SOURCE` 返回 Static/Api |
| 搜索 | 按需 `loadSearchIndex()` + `filterItems` | `source.setQuery({q})` + `useWall.reset()`；不加载 `search-index.json` |
| 标签/收藏/分级/分组 | 对 `wall.items` 做 `baseItems` 过滤 | 路由变化时 `setQuery({tag/fav/rating/group})`，服务端返回完整结果 |
| 排序 | `sortItems` 内存排序 | `setQuery({sort,order,seed})`；`random` 用服务端稳定 seed |
| 分页 | `loadMore()` 追加分片 | `loadMore()` 追加服务端游标页 |
| 详情深链 | `findItem` 顺序翻分片 | `fetchItem(id)` 单条补取；未命中直接缺失页 |
| 计数/封面 | `index.facets` + 已加载集合 | `/api/index` facets + 服务端聚合 |

- `useWall` 增加 `reset()`：API 模式下筛选/排序变化先 `setQuery`，再清空 items/游标并拉第一页。
- `App.tsx` 用 `source.capabilities?.server_filter` 分支；没有 capabilities 的旧 source 一律走 static 逻辑。
- `requestSearchIndex()` 在 API 模式直接跳过；调用处改为 `source.loadSearchIndex?.()`。
- API 模式不再后台拉全库（`needAll=false`），有限墙滚动到墙尾才 `loadMore()`；无限世界只在静态模式开启，避免为了无限画布把整库拉完。
- API 模式的服务端排序直接使用返回顺序；`random` 仍在客户端做一次稳定洗牌（服务端按 id 稳定排序，游标不跳条）。

### 7.4 搜索语法兼容

静态站 `web/src/state/filters.ts` 只支持 `name:`/`tag:`/`group:`/`rating:`、空格 AND、`-词` 排除；服务端查询是超集。两种模式的结果不能承诺逐条一致，必须写进帮助与测试：

| 输入 | static | api |
|---|---|---|
| `tag:xxx`、`name:xxx`、`group:xxx`、`rating:xxx` | 支持 | 支持 |
| `-词`、空格 AND | 支持 | 支持 |
| `tag:"two words"`（引号保留空格） | 支持 | **不支持**：服务端按空白切词；需预分词或禁用并写进帮助 |
| `is:failed`、`star:>=3`、`date:`、`size:` 等 | 不支持（当普通词） | 支持 |
| 未知前缀/非法比较符 | 当普通词 | 记入 warnings，不报错 |

- 引号短语要么在网页后端先做与前端一致的预分词，要么在 API 模式禁用并写进搜索帮助；**不能两种模式静默给出不同结果**。
- `warnings` 非空时通过响应字段回传，网页显示一次性提示。
- 游标/参数错误才显示可关闭错误条，保留上一页结果，不清空墙。

## 8. 部署

### 8.1 本地

```powershell
cargo run -p kanalib-web -- --db .kanalib-web/kanalib-web.db --media .kanalib-web/media --web web/dist --bind-public 127.0.0.1:8787 --bind-private 127.0.0.1:8788
```

首次冒烟可加 `--seed-sample` 写入两条固定样本（含作品/作者/收藏夹），媒体文件需自行放到 `<media>/t512/<id>.jpg`。

### 8.2 2c2g 轻量云

- 二进制 + `web/dist` + SQLite + 媒体目录；媒体单独挂盘，避免系统盘打满。
- 前置 Nginx/Caddy：TLS、`/api` 与 `/media` 反代、gzip/brotli；静态资源可加 CDN。
- **两个 listener**：public `127.0.0.1:8787` 只读（给 Nginx）；private `10.126.126.2:8788` 只给 EasyTier 虚拟网。
- Nginx 显式拒绝 `/internal/*`、`/api/desktop/*`、`/api/items/*/original`，作为第二道保险。
- SQLite 开 WAL；每日备份到对象存储；保留最近 7 天。
- 现成脚本：`deploy/backup/kanalib-web-backup.sh`（`.backup` + `integrity_check` + gzip + 保留天数），timer 模板在 `deploy/systemd/`。
- 内存目标 <150MB；如超，先降 `limit` 上限与并发，不引入重型缓存。
- 公网只读；写接口只在 EasyTier 私网；管理口只回环/VPN。

### 8.3 与 box 的联通

- box → 网页：出站 HTTP 到 **EasyTier 私网地址**（`kanalib-web.et.net:8788`），带 HMAC 批次；网页不需要主动访问 box。
- 网页 → box（原图回源）：只发签名请求，box 校验签名与过期时间；网页不持有 box 的管理密钥。
- 桌面 → 网页（pull/push/upload）：走 EasyTier 私网地址；桌面离线时明确报错并重试。
- 桌面 → box（原图/批量下载）：优先直连 box，不经过 2c2g 云；这是主要的流量节省点。
- 断网：网页保留已有目录；box 推送重试，幂等键防重复。
- 公网主站不依赖 EasyTier：EasyTier 故障时只读仍可用。

## 9. 性能目标

| 场景 | 目标 |
|---|---|
| `/api/items?limit=60`（10k 条目） | p95 ≤ 120ms（2c2g 本地 SSD） |
| 搜索 `q`（10k） | p95 ≤ 200ms |
| `/api/authors`（1k 作者） | p95 ≤ 80ms |
| 媒体 t512 | TTFB ≤ 50ms（本地）；304 ≤ 10ms |
| 并发 | 32 并发无 5xx；内存 <150MB |

测量口径：`wrk`/`oha` 本地回环；记录库标识与机器；不把 headless 数字当公网 SLA。

2026-11-22 开发机实测（Windows / 16 逻辑核，10k 条目，32 并发 × 5 轮，`tests/web_flow.rs` 的 ignored 压测）：plain/star/tag+name/random 的 p95 为 35–42ms，`q=alpha`（命中约 1/3 库）p95 为 183ms；测试按文档预算分别断言 120ms 与 200ms。2c2g 目标机仍以 `w2_10k_32_concurrent_p95_under_120ms` 复测为准。

## 10. 安全

- 路径穿越：id 正则 + DB 反查 + canonicalize 前缀；拒绝 `..`、绝对路径、UNC。
- 目录遍历：`/media` 不列目录；未知 id 404。
- SSRF：网页后端不代用户请求任意 URL；原图回源只访问配置的 box 地址。
- 令牌：box 共享密钥、桌面设备令牌都不写日志；只存哈希/密钥文件 0600。
- CORS：同源部署，默认不允许跨域；需要时显式列出。
- 速率限制：`/internal/*` 与 `/api/desktop/*` 单独限流；公开读走反代限流。
- 隐私：媒体 URL 用 id，不含本地路径；`/api/index` 不返回根目录。
- 网络：public/private listener 分离；public 不注册写路由；EasyTier ACL 只开必要端口。

## 11. 测试

### 11.1 单元/集成

- `kanalib-wall-model`：DTO 序列化、可选字段缺省、静态/动态同形状、游标编解码。
- `kanalib-web`：`tower::ServiceExt::oneshot` + 临时 SQLite + 临时媒体目录。
  - `/api/index` 计数/facets；`/api/items` 分页/游标/排序/搜索；`/api/works` 页序；`/api/authors` 聚合；`/api/collections` 计数。
  - 媒体：200/206/304/404/416；`..`、非法 id、超大 Range。
  - 内部：无签名 401、过期 401、重复幂等键 200 不重复、非 allow 批次 400。
  - 桌面：无令牌 401、撤销令牌 401、push 冲突可见。
  - 网络边界：public listener 对写路由返回 404；private listener 只绑 `127.0.0.1`。
- `kanalib-publish`：静态契约不回归；新增可选字段仍通过 `wall_manifest_check.py`。

### 11.2 网页

- `npm run typecheck && npm run build`（`VITE_WALL_SOURCE=api` 与 `static` 各一次）。
- `wall_api_check.mjs`：endpoint 形状、游标、ETag、Range、`schema=1`、`capabilities.server_filter=true`；`--ui` 时还会断言页面真的发出 `q/tag/sort` 查询、不拉 `search-index.json`、不整库翻页。
- 现有 `wall_probe_state.mjs`、`wall_probe_detail_nav.mjs` 在 static 模式回归；API 模式的首页/总览/深链用 `wall_api_check --ui` 与 headless DOM 检查。

### 11.3 手动

- box 推送 100 条，网页只出现 allow；block/review 不出现。
- 断网/DB 锁定：`/api/health` 503，网页错误条可重试。
- 原图回源：签名 URL 5 分钟内可用，过期 403；未配置时 404。
- 公网原图：`/api/items/{id}/original` 公网 404；EasyTier 私网内可用。
- 桌面直连 box：`easytier-cli route` 的 `next_hop` 为 `DIRECT`；云出口流量接近 0。
- 移动端：底部导航 + 详情 sheet 不回归。

## 12. 阶段

| 阶段 | 内容 | 退出条件 |
|---|---|---|
| W0 | `kanalib-wall-model` DTO + 转换 + 单测（已实现） | 静态/动态同形状；publish 不回归 |
| W1 | `kanalib-web` 只读：health/index/items/媒体（已实现） | 临时库集成测试；ETag/Range |
| W2 | 搜索/排序/游标 + works/authors/collections（已实现） | 2c2g 压测达标；分页稳定 |
| W3a | `ApiSource` + `createSource` + `fetchItem`（已实现） | 静态/API 双 bundle；现有探针 |
| W3b | 筛选/排序/计数切到 `setQuery`（已实现） | `wall_api_check.mjs` |
| W4 | 内部 ingest 接口 + 桌面桥接 + EasyTier 私网 | 签名/幂等/令牌测试；公网写路由 404 |
| W5 | 部署脚本/反代/备份/监控 | 文档 + 冒烟 |

> W3 拆成两步：W3a 只换数据源（`createSource` + `ApiSource` + `useWall.reset` + `fetchItem`），页面仍用现有内存筛选；W3b 再把筛选/排序/计数切到 `setQuery`。W3a 通过现有探针，W3b 通过 `wall_api_check.mjs`（含 `--ui` 请求断言）。

## 13. 待确认

| # | 问题 | 推荐 |
|---|---|---|
| B-Q1 | 网页后端是否与桌面同库 | **否**；完全独立 SQLite |
| B-Q2 | 媒体由 Rust 还是 Nginx 服务 | 缩略图本地 Rust；公网 Nginx/CDN |
| B-Q3 | 动态模式是否保留 `search-index.json` | 不保留；服务端搜索 |
| B-Q4 | 是否做服务端渲染 | 不做；继续 SPA |
| B-Q5 | 公开读是否需要登录 | v1 不需要；写接口必须鉴权 |
| B-Q6 | 是否引入连接池 | v1 不引入；压测后评估 |
| B-Q7 | 动态 `/api/index` 的 `schema` | 保持 1，用 `api_version` + `capabilities` 区分 |
| B-Q8 | API 模式筛选/计数 | 服务端 `setQuery` + facets；不能只过滤已加载页 |
| B-Q9 | 原图是否上云 | 默认不上；签名回源 box |
| B-Q10 | 写接口是否暴露公网 | **否**；只挂 private listener，绑 EasyTier 虚拟 IP |
| B-Q11 | 公网是否要求 EasyTier | **否**；公网 HTTPS 只读 |
| B-Q12 | 桌面上传走哪 | **EasyTier private listener**；不开放公网上传 |
