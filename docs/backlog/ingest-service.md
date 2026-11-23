# 入库过滤服务（box）设计（v2）

> 上级：[pixiv-integration.md](pixiv-integration.md) · 网页后端：[web-dynamic-backend.md](web-dynamic-backend.md) · 侧栏：[web-rail-horizontal.md](web-rail-horizontal.md) · 三端互联：[easytier-connectivity.md](easytier-connectivity.md)
> 组件选型与可复用项目：[p2-reuse-research.md](p2-reuse-research.md)（Pixiv 客户端、Ugoira、队列、签名）。
>
> 2026-11-21 用户指示：来自 Pixiv 等外部来源的图**必须**经过独立入库过滤服务中转，不能直接接入网页；该服务部署在 box 上，负责拉取、打标、按标签黑名单与规则过滤，只把通过的图交给网页。
>
> 2026-11-22 用户定调（v2）：过滤改为**纯黑名单制**，规则写在 box 的配置文件里，分「按用户屏蔽」「按作品标签屏蔽」两类；**不屏蔽 AI 生成，不屏蔽 R-18**，**含 R-18G 的作品完全屏蔽**；作品标签不足以判断类别时，交打标模型补标签，合并后再跑黑名单。§7 按此重写。
>
> 代码现状与本文档的差距见 **§17 待改动项**（按文件列出，含行号）；那一节是 P3 的施工清单，不要按本文档的"目标态"误以为已实现。
>
> 状态：**设计稿，未实现**。本文件只定职责、契约、数据模型与验收，不含代码。

## 0. 一句话

`kanalib-ingest` 是**唯一**持有外部来源凭据、**唯一**决定 `allow / block / review` 的边界服务：它把来源图片变成「已过滤、可发布」的条目，再推给 2c2g 云上的网页后端。

## 1. 边界

### 1.1 必须做

1. 持有并保护来源凭据（Pixiv refresh token 等）；凭据不出 box。
2. 拉取元数据、下载原图、维护下载队列与失败重试。
3. 调用打标模型，产出标签与分数。
4. 按配置里的用户黑名单、标签黑名单与 R-18G 硬拦做决策；记录命中原因与规则版本。
5. 提供 `review` 队列，允许人工放行/拒绝。
6. 只把 `decision=allow` 的条目推送给网页后端；推送幂等、可重试、可重放。
7. 为单条目提供短期签名媒体响应，供网页/桌面按需回源原图；不提供目录、搜索或页面。
8. 只通过 EasyTier 私网提供管理页、原图与桌面拉取入口；默认零公网入站。

**v2 过滤口径（覆盖任何旧默认值）**

1. 规则是**黑名单制**：没命中就是放行，不存在「默认拒绝某分级」。
2. 黑名单只有两类：**黑名单用户的作品**、**命中黑名单标签的作品**；另加一条与黑名单无关的分级硬拦（R-18G）。
3. **R-18 放行、AI 生成放行**；只有 **R-18G 完全屏蔽**。
4. 标签数量不足以判断类别时，先交打标模型补标签，再对合并后的标签跑黑名单。
5. 阈值、词表、用户 ID 全部在配置文件里，改规则 = 改文件 + 重载，不改代码。

### 1.2 绝对不做

- **不提供公开目录**：没有可浏览页面、搜索或批量只读 API；管理页只回环/EasyTier 私网。默认**没有任何公网入站**；签名媒体也优先只走私网。
- **不写网页数据库**：只通过网页后端的内部 ingest 接口推送，不直连云上 SQLite。
- **不扫描桌面图库**：桌面端是独立工作台，入库服务不读桌面 DB、不监听本地目录。
- **不接收桌面全库同步**：桌面可选回传只走网页后端的设备接口，不经过本服务。
- **不把未过滤条目外发**：任何失败路径都不得产生 `allow`。
- **不做数据迁移**：不导入 Pixiv-Archive 或旧版 Kanalib 的数据包、缩略图包、元数据包。

### 1.3 信任边界

```text
Pixiv / 其他来源
      │  凭据只在这里
      ▼
┌─────────────────────────────┐
│ box: kanalib-ingest         │
│  - 来源适配 / 下载 / 打标    │
│  - 规则 / 审核 / 推送        │
│  - 原图签名回源              │
└──────────┬──────────────────┘
           │ EasyTier 私网：只发 allow（HMAC 批次 + 缩略图）
           ▼
┌─────────────────────────────┐
│ 2c2g: kanalib-web           │
│  - 独立 SQLite / 只读 API    │
│  - 不持有来源令牌            │
└─────────────────────────────┘
```

## 2. 架构

### 2.1 管线

```text
Scheduler
  └─ SourceAdapter (pixiv)
       ├─ 收藏订阅 / 作者作品 / 单作品补拉
       └─ 元数据归一
            ▼
       DownloadQueue
         ├─ .part + Range
         ├─ 多页并发
         └─ ugoira → GIF
              ▼
         Tagger
           ├─ sidecar 或 HTTP 模型
           ├─ 超时 / 重试 / 批处理
           └─ 失败 → review
                ▼
           RuleEngine
             ├─ 来源开关 / 作者黑名单
             ├─ R-18G 硬拦（x_restrict / 标签 / restriction_attributes）
             ├─ 标签黑名单（原名 + 译名，归一化精确匹配）
             ├─ 标签数 < 阈值 → 回 Tagger 补标签 → 再跑一次黑名单
             ├─ 重复哈希（直接丢）/ 观察词（转 review）/ 尺寸（review 兜底）
             └─ allow / block / review
                  ▼
             ReviewQueue（人工）
                  ▼
             PushClient
               ├─ HMAC 批次
               ├─ 幂等键
               ├─ 重试 / 重放
               └─ deleted 事件
```

### 2.2 进程与 worker

| 组件 | 职责 | 并发 |
|---|---|---|
| `sync` worker | 来源同步、订阅追平、元数据补全 | 1 个调度循环；来源请求共享 `ApiGate` |
| `download` worker | 下载队列、分页、ugoira | 可配置 2–4 个；受 `ImageSemaphore` 限制 |
| `tag+filter` worker | 打标、规则、决策、入 review；标签不足时再调一次模型补标 | 1–2 个；受 `ModelSemaphore` 限制 |
| `push` worker | 批次推送、重试、删除同步 | 1 个；失败退避 |
| 管理 HTTP | 健康、审核、队列、规则、手动触发 | 绑 EasyTier 虚拟 IP，只私网可达 |
| 媒体 HTTP | 单条目原图、Range、签名校验 | 只私网；桌面可直连 |

**共享资源**

- `ApiGate`：按来源做令牌桶/并发上限；429 时统一退避，避免多个 worker 一起打爆。
- `ImageSemaphore`：限制同时下载的图片数，保护 box 磁盘与带宽。
- `ModelSemaphore`：限制同时打标的图片数；模型不可用时整批暂停并告警，不空转。
- SQLite WAL：所有 worker 短事务；下载/打标的大文件 IO 不持有写事务。

### 2.3 崩溃恢复

- 队列状态持久化；启动时把 `running` 重置为 `queued`，并保留 `attempts`。
- 下载中的 `.part` 保留，按 Range 续传；校验失败才删除重下。
- 推送批次持久化；未确认批次重启后重试，靠幂等键防重复。
- 打标中的条目重启后重新入队；已产出但未决策的标签保留。

## 3. 数据模型（服务自己的 SQLite）

> `SCHEMA_VERSION` 从 1 开始；与桌面、网页的库完全独立。迁移前自动备份。

```sql
CREATE TABLE IF NOT EXISTS sources (
    id TEXT PRIMARY KEY,                 -- pixiv
    kind TEXT NOT NULL,                  -- pixiv | other
    enabled INTEGER NOT NULL DEFAULT 1,
    config_json TEXT NOT NULL DEFAULT '{}',
    credential_ref TEXT,                 -- keyring/文件引用，不存明文
    last_sync_at INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS authors (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    external_id TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    avatar_url TEXT,
    avatar_path TEXT,
    avatar_updated_at INTEGER,
    deleted INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER,
    UNIQUE(source, external_id)
);

CREATE TABLE IF NOT EXISTS works (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    external_id TEXT NOT NULL,
    author_id TEXT REFERENCES authors(id) ON DELETE SET NULL,
    title TEXT NOT NULL DEFAULT '',
    page_count INTEGER NOT NULL DEFAULT 1,
    posted_at INTEGER,
    rating TEXT NOT NULL DEFAULT 'all',    -- all | r18 | r18g；来自 x_restrict 归一
    x_restrict INTEGER,                    -- 原始值 0/1/2；缺失为 NULL
    sanity_level INTEGER,                  -- 原始值 0/2/4/6；仅用于三路全缺时的 review 兜底
    restriction_attributes TEXT NOT NULL DEFAULT '[]', -- 原始 JSON 数组，如 ["restricted_mode"]
    ai_marked INTEGER NOT NULL DEFAULT 0,  -- illust_ai_type == 2
    tag_count INTEGER NOT NULL DEFAULT 0,  -- 来源标签数（不含模型补的）
    status TEXT NOT NULL DEFAULT 'active', -- active | deleted
    metadata_synced_at INTEGER,
    next_retry_at INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(source, external_id)
);

CREATE TABLE IF NOT EXISTS items (
    id TEXT PRIMARY KEY,                 -- 40/64 位小写 hex，与网页契约一致
    work_id TEXT NOT NULL REFERENCES works(id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    page INTEGER NOT NULL DEFAULT 0,
    original_url TEXT,
    original_path TEXT,                  -- box 本地原图；不外发
    ext TEXT,
    width INTEGER NOT NULL DEFAULT 0,
    height INTEGER NOT NULL DEFAULT 0,
    bytes INTEGER NOT NULL DEFAULT 0,
    sha1 TEXT,
    phash TEXT,
    thumb_256 TEXT,
    thumb_512 TEXT,
    decision TEXT NOT NULL DEFAULT 'pending', -- pending | allow | block | review
    decision_reason TEXT,
    rules_version TEXT,
    reviewed_by TEXT,
    reviewed_at INTEGER,
    revision INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(work_id, page)
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
    origin TEXT NOT NULL DEFAULT 'model', -- model | pixiv | manual
    PRIMARY KEY(item_id, tag_id, origin)
);

CREATE TABLE IF NOT EXISTS subscriptions (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL,
    user_external_id TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_seen_work_id TEXT,
    last_checked_at INTEGER,
    next_check_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(source, user_external_id)
);

CREATE TABLE IF NOT EXISTS download_queue (
    id INTEGER PRIMARY KEY,
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    state TEXT NOT NULL DEFAULT 'queued', -- queued | running | done | failed | paused
    priority INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_retry_at INTEGER,
    last_error TEXT,
    started_at INTEGER,
    finished_at INTEGER,
    UNIQUE(item_id)
);

CREATE TABLE IF NOT EXISTS rule_snapshots (
    version TEXT PRIMARY KEY,            -- 配置文件里的 rules.version，只增不减
    config_hash TEXT NOT NULL,           -- 规范化后的 SHA-256，用于发现"改了但没升版本"
    config_json TEXT NOT NULL,           -- 生效时的规则快照，便于回溯判定依据
    loaded_at INTEGER NOT NULL DEFAULT (unixepoch()),
    loaded_by TEXT
);

CREATE TABLE IF NOT EXISTS rule_proposals (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,                  -- add_user | add_tag | remove_user | remove_tag
    payload_json TEXT NOT NULL,          -- {"user_id": 1234567} 或 {"tag": "guro"}
    source_item_id TEXT,                 -- 从哪条 review 触发的；手动提交为 NULL
    note TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT 'pending', -- pending | applied | rejected
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    decided_at INTEGER,
    decided_by TEXT,
    apply_error TEXT                     -- 应用失败原因（文件权限、并发修改、校验不过）
);

CREATE TABLE IF NOT EXISTS decisions (
    id INTEGER PRIMARY KEY,
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    decision TEXT NOT NULL,              -- allow | block | review
    reason TEXT NOT NULL DEFAULT '',
    rules_version TEXT NOT NULL,
    matched_rules_json TEXT NOT NULL DEFAULT '[]', -- 命中的黑名单条目（用户 ID / 标签 / 规则名）
    tag_source TEXT NOT NULL DEFAULT 'source',     -- source | model | merged：本次判定用的是哪套标签
    tag_count_used INTEGER NOT NULL DEFAULT 0,     -- 参与判定的标签总数
    tags_json TEXT NOT NULL DEFAULT '[]',
    decided_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS push_batches (
    id TEXT PRIMARY KEY,                 -- 幂等键
    item_count INTEGER NOT NULL DEFAULT 0,
    payload_hash TEXT NOT NULL,
    payload_json TEXT NOT NULL DEFAULT '', -- 原样重放，重启后内容不变
    kind TEXT NOT NULL DEFAULT 'allow',    -- allow | delete
    state TEXT NOT NULL DEFAULT 'pending', -- pending | acked | rejected | failed
    attempts INTEGER NOT NULL DEFAULT 0,
    next_retry_at INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    acked_at INTEGER
);
```

### 3.1 索引

```sql
CREATE INDEX IF NOT EXISTS idx_items_decision ON items(decision, updated_at);
CREATE INDEX IF NOT EXISTS idx_items_work ON items(work_id, page);
CREATE INDEX IF NOT EXISTS idx_works_author ON works(author_id);
CREATE INDEX IF NOT EXISTS idx_queue_state ON download_queue(state, priority DESC, next_retry_at);
CREATE INDEX IF NOT EXISTS idx_decisions_item ON decisions(item_id, decided_at DESC);
CREATE INDEX IF NOT EXISTS idx_push_state ON push_batches(state, next_retry_at);
```

### 3.2 不变量

1. `items.decision` 初始必须是 `pending`；只有规则引擎或人工审核能写 `allow/block/review`。
2. `decision=allow` 必须有对应的 `decisions` 行；无决策记录的条目不得推送。
3. `block` 永不进入推送；`review` 只留在 box。
4. `rules_version` 只增不减；每次决策记录完整命中原因。
5. `original_path` 只在本机使用，任何 API 响应都不返回绝对路径。
6. 同一 `item_id` 的推送幂等；`revision` 只增不减。
7. 删除只置 `works.status='deleted'`；原图与证据保留，便于回溯。
8. `rules_version` 必须存在于 `rule_snapshots`；重跑旧版本时用快照，不读当前文件。
9. `works.tag_count` 是**来源标签数**，不含模型补的标签；判定时用 `decisions.tag_count_used`，两者不可混用。
10. `rating='r18g'` 的条目只能是 `block`，人工审核也不能改成 `allow`。
11. **规则文件是唯一权威**：`rule_snapshots` 只是它某一版本的只读副本，`rule_proposals` 只是待应用草稿；任何判定都不得读取这两张表代替文件。
12. 不存在可写的 `rules` 表。`db.rs` 里已有的 `rules` 表（`kind`/`payload_json`/`priority`/`enabled`）必须删除，否则会出现「文件说放行、表说拒绝」的双源。

## 4. 来源适配

### 4.1 `SourceAdapter` trait

```rust
pub trait SourceAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn capabilities(&self) -> SourceCapabilities;
    async fn fetch_work(&self, external_id: &str) -> Result<SourceWork, SourceError>;
    async fn fetch_author(&self, external_id: &str) -> Result<SourceAuthor, SourceError>;
    async fn list_author_works(&self, external_id: &str, cursor: Option<&str>) -> Result<Page<SourceWork>, SourceError>;
    async fn list_public_bookmarks(&self, user_id: &str, cursor: Option<&str>) -> Result<Page<SourceWork>, SourceError>;
    async fn resolve_image(&self, work: &SourceWork, page: u32) -> Result<ImageRef, SourceError>;
}
```

- 新增来源只实现 trait 与注册，不改下载/打标/规则/推送主流程。
- 适配器只负责「取到什么」，不负责「能不能发布」。

### 4.2 Pixiv 适配

| 关注点 | 做法 |
|---|---|
| 认证 | OAuth refresh token；存 keyring 或 0600 文件；不写日志、不进 DB 明文 |
| 节流 | 共享 `ApiGate`；按响应头与错误类型退避；订阅与元数据同步共享配额 |
| 错误分类 | `auth` / `not_found` / `rate_limited` / `network` / `internal` |
| 404 | 保留本地元数据，`works.status='deleted'`；不删原图与审核记录 |
| 429 | 指数退避 + 抖动；写 `next_retry_at`；不忙等 |
| 收藏订阅 | 按**用户** ID；翻公开收藏页；整页都已入库则提前停（快速追平） |
| 作者 | 改名/头像 URL 变化才更新；头像缓存到 box |
| 多页作品 | `page_count` 与页序来自作品元数据；缺页单独补 |
| 动图 | ugoira zip → 逐帧 → 无限循环 GIF；帧数上限 500 |
| 分级归一 | 读 `x_restrict`（`0/1/2` → `all/r18/r18g`，缺失留 `NULL`）、`sanity_level`、`restriction_attributes` 三个原始字段并存，不在适配层做拒绝 |
| AI 标记 | 读 `illust_ai_type`：`2 → ai_marked=1`，`1 → 0`，`0`/缺失 → `NULL`（未知），**不猜成 0** |

> 现状差异（2026-11-22 核对 `crates/kanalib-ingest/src/source/pixiv.rs`）：骨架里 `rating` 默认值是字符串 `"G"`、`ai_marked` 直接读一个不存在的布尔字段。P3 落地时必须改成上表口径——`rating` 用 `all/r18/r18g`，`x_restrict` 缺失时存 `NULL` 而不是猜 `all`，`illust_ai_type` 存三态。否则「字段缺失」会被静默当成「全年龄」，与 §7.4 的三路判定直接冲突。

### 4.3 未来来源

- 其他图站/本地导入包只作为新 adapter；**不允许**绕过规则引擎。
- 若来源没有稳定作品 ID，适配器负责生成稳定 `external_id`，不得用随机值。

## 5. 下载

### 5.1 队列状态机

```text
queued ──► running ──► done
  ▲           │
  │           ├──► failed（可重试，写 next_retry_at）
  │           └──► paused（人工/配额）
  └───────────┘
```

- `attempts` 有上限；超过后保持 `failed` 并在管理页可见，不静默丢弃。
- 重启时 `running → queued`；`.part` 保留。

### 5.2 下载规则

1. 先写 `<id>.part`，成功后 `rename`；失败不覆盖已有完整文件。
2. 支持 Range 续传；服务端不支持 Range 时从头下载。
3. 校验尺寸/字节/内容类型；HTML 错误页不得当图片。
4. 多页作品并发下载，但受 `ImageSemaphore` 限制；单页失败不影响其他页。
5. 部分成功先入库，缺页进队列；管理页显示「3/5 页」。
6. 下载完成才算 `done`；缩略图生成失败不回滚原图，但条目进 `review`。

### 5.3 动图

- 下载 ugoira zip → 校验帧清单 → 逐帧解码 → 合成无限循环 GIF。
- 帧数上限 500；超限截断并在管理页标注。
- 首帧作为缩略图；GIF 作为原图回源文件。

## 6. 打标

### 6.1 模型后端

| 模式 | 说明 |
|---|---|
| sidecar | 本机进程/Unix socket/HTTP 回环；适合 box 上常驻小模型 |
| HTTP | 远程或容器内模型服务；需要超时、并发上限、鉴权 |

### 6.2 约束

- 批量大小可配置；单张超时（建议 30s）后重试 1 次，仍失败 → `review`。
- 模型不可用（连接失败/连续 5 次超时）→ 暂停 `tag+filter` worker 并告警；不把条目判 `allow`。
- 标签归一化按 §7.3：`NFKC → trim → 去内部多余空白 → lowercase`；保留原始标签与翻译标签两份原文。
- 分数保留浮点，但 v2 不用分数做自动拒绝：分数只用于 review 排序与「可疑度」展示（§7.1）。
- 打标结果写 `item_tags(origin='model')`；Pixiv 自带标签写 `origin='pixiv'`，两者不互相覆盖。
- 打标有两条调用路径：**全量打标**（配置开启，用于建标签库）与**补标**（§7.5，仅在标签不足时触发）。两条路径共用同一个 `TaggingBackend`，但补标只关心标签集合，不需要分数。
- 补标是同步阻塞决策的：调模型期间该条目停在 `pending`，不并发重复调用；同一 `item_id` 用行级锁保证只补一次。

### 6.3 失败策略

| 情况 | 决策 |
|---|---|
| 模型返回空标签 | `review`（可配置为 `block`） |
| 补标后标签数仍 < `min_tags` | 照常判定；若黑名单无命中则 `allow`，只记录 `tag_budget_short_total` |
| 超时/连接失败 | `review` |
| 图片损坏/无法解码 | `review` |
| 规则引擎异常 | `review` |
| 数据库写失败 | 不决策，重试；不得默认放行 |

## 7. 规则与决策

### 7.1 规则模型：黑名单制

规则全部来自配置文件，服务启动时读入并快照；**没有数据库规则表**。只有三类阻断源：

| 类型 | 配置键 | 语义 | 备注 |
|---|---|---|---|
| 用户黑名单 | `rules.blacklist.users` | 该作者（Pixiv user id）的**所有**作品一律 `block` | 精确匹配 `author.external_id`，不做昵称/模糊匹配 |
| 标签黑名单 | `rules.blacklist.tags` | 作品标签命中任一词即 `block` | 归一化后**精确**匹配，同时查 `name` 与 `translated_name` |
| 观察词表 | `rules.blacklist.watch` | 命中**不拦截**，只写 `decisions.reason` 并进 `review` 供人工判断 | 用于高误杀风险的泛词；人工可一键升级到 `tags` |
| 分级硬拦 | `rules.rating.block` | 仅 `r18g` | R-18 与全年龄放行，见 §7.4 |

其余规则只是**兜底与质量门**，不做内容判断：

| 类型 | 作用 | 结果 |
|---|---|---|
| `source.enabled` | 来源总开关 | 关闭 → `block` |
| `dedup` | 与已发布条目同哈希 | `block`，保留最早一条 |
| `size` | 宽/高/字节低于下限 | `review`，不自动拒绝 |
| `tag_budget` | 标签数不足 | 转打标模型，见 §7.5 |

明确**不做**的事：不按 AI 标记过滤（`illust_ai_type=2` 正常入库）、不按 R-18 过滤、不做分数阈值自动拒绝（模型分数只用于 review 排序）。

**`tags` 与 `watch` 的区别**：`tags` 是终态拒绝，不进 review；`watch` 只标记可疑，条目仍进 review 等人工决定。`watch` 的价值是让你先看到「这个词会杀多少图」，再决定要不要把它变成硬规则。两表都做归一化，且同一个词不能同时出现在两处（启动时报语义冲突）。

### 7.2 决策顺序

```text
0. 读配置快照 + 归一化来源字段
1. 来源禁用                       → block
2. 作者在黑名单                   → block
3. R-18G（三路任一命中）           → block，终态
4. 标签黑名单（来源标签）          → block
5. 参与判定的标签数 < 阈值         → 打标模型补标签 → 合并 → 回到第 4 步一次
6. 重复哈希                        → block（保留最早）
7. 命中观察词表                    → review（不拦截，标记原因）
8. 尺寸/字节不达标                 → review
9. 以上全过                        → allow
10. 规则解析/模型/DB 异常           → review
```

- 顺序固定，先命中先返回；`decisions.matched_rules_json` 记录**全部**命中项，不只第一条。
- 第 5 步只补一次标签，不循环；补完仍不足阈值也照常判定（见 §7.5）。
- 去重放在 `watch` 之前：重复图直接丢，不占审核队列。若同一作品既命中硬黑名单又命中观察词，按 `block` 处理，不必再看观察结果。
- allow 永远是最后一步：只有黑名单、R-18G、重复、观察词、尺寸全部过完才会走到它。
- 规则变更必须同时改 `rules.version`；服务检测到 `config_hash` 变化但版本未变时**拒绝热重载**并告警。
- 旧条目不会自动重算；手动「按当前规则重跑」只影响 `pending/review` 与显式选中的条目，已 `allow` 的条目需二次确认。

### 7.3 标签归一化

黑名单是精确匹配，归一化必须确定且可复现，否则「同一个词时中时不中」。

```text
raw        → NFKC → trim → 去内部多余空白 → lowercase
比较对象    → tags[].name 与 tags[].translated_name 各算一次
```

- 全角/半角、大小写、首尾空格差异都吃掉；**不做**同义词、罗马字、分词、模糊匹配。
- 同义词靠词表自己列全（例如 `loli` 与 `ロリ` 各写一条），不引入隐式映射。
- 黑名单词条同样按上面规则归一化后再比较。归一化后**重复**（如 `QOS` 与 `qos`、`ＱＯＳ`）→ **自动去重 + 启动告警**，不拒绝启动：同义重复无害，写两遍只是啰嗦。
- 只有**语义冲突**才拒绝启动：同一个词同时出现在 `rules.rating.block` 与 `rules.rating.allow`，同一个词同时出现在 `tags` 与 `watch`，或同一 user id 既在 `users` 又被白名单引用（当前无白名单，预留）。
- 只匹配标签，不匹配标题/简介/作者名。标题里的词是弱信号，容易误杀。

### 7.4 R-18G 的三路判定

单一字段都不可靠，必须三路任一命中即 block：

| 来源 | 判定 |
|---|---|
| `x_restrict` | `== 2` |
| 标签 | 归一化后等于 `r-18g`（含 `r18g` 写法） |
| `restriction_attributes` | 数组内含 `r18g` |

依据与取舍：

- `x_restrict` 取值 `0=全年龄 / 1=R-18 / 2=R-18G`，已由独立实现交叉确认（见 §16.1）；但它在部分响应里缺失或为 `null`，不能当唯一依据。
- Pixiv 的投稿规范要求这类作品标注 `R-18G`，标签也因此是最稳的一路（Pixiv-Archive 就只用标签判分级）。注意这是平台规则约束，不是 API 强制字段，所以仍不能只靠它。
- `restriction_attributes` 是新字段，实测样本里只出现过 `restricted_mode`，**未观察到 `r18g` 取值**，因此只作加分项，不作必要条件。
- 三路全部拿不到时不算安全：若 `sanity_level >= 6` 则转 `review`，不直接 allow。
- 反过来，`sanity_level` **不能**用来放行：它高不代表是 R-18G，低也不代表干净（样本里大量全年龄作品是 2，但也有 4）。它只在这一条兜底规则里出现。

**R-18 与 AI 的显式放行**：配置写 `rules.rating.allow = ["all", "r18"]`、`rules.ai.allow = true`。这不是「漏配所以放行」，而是有开关、可审计的决策；要改成屏蔽只改配置，不改代码。

### 7.5 标签不足时的模型补标

用户口径：**标签数量不足以判断类别时，交打标模型补标签，合并后仍按黑名单判定。**

阈值与统计口径：

- 配置项 `tag_budget.min_tags = 5`（暂定），只统计**内容标签**。
- 计入：Pixiv 标签的 `name` / `translated_name` 去重后的集合，加模型补的标签。
- 不计入：`オリジナル`、`漫画`、`イラスト` 这类体裁词，以及 `1000users入り` / `5000users入り` 这类收藏数系统标签。系统标签占比很高，算进去会让阈值形同虚设。
- 判定用「去重后的标签集合大小」，不是原始数组长度；`name` 与 `translated_name` 是同一标签的两种写法，只算一个。
- 因为先剔了体裁词，「标签数量够但全是泛词」的情况会自动落进不足区间。例如 5 个标签全是 `オリジナル / 漫画 / イラスト / 女の子 / 風景`，内容标签只有 2 个 → 触发补标。这正是「数量够但不能明确判断类别」的情形。

实测依据（`pixivpy` 仓库 151 条真实响应样本，2026-11-22；按 `tags[].name` 计数，未计译名重复）：

| 口径 | 平均标签数 | `< 5` 占比 | `< 6` 占比 |
|---|---|---|---|
| 原始数组长度 | 8.30 | 7.9% | 12.6% |
| 剔除 `Nusers入り` 后 | 7.34 | 10.6% | 16.6% |
| 再剔除体裁词（本方案的判定口径） | 6.56 | **13.9%** | 23.8% |

样本里标签上限稳定在 10（41% 的作品正好 10 个），说明 5 是「中位数偏下」而不是极端值。按本方案口径约**七分之一**的作品会进模型队列——比只看原始数组长度高出一倍，这是剔掉体裁词与系统标签的代价，也是必须留指标的原因。

补标流程：

```text
tags = 来源标签（归一化去重）
if count(tags) < min_tags:
    model_tags = tagger.tag(image)          # 单张，超时 30s，重试 1 次
    tags = dedupe(tags ∪ model_tags)
    decisions.tag_source = "merged"
else:
    decisions.tag_source = "source"
decisions.tag_count_used = count(tags)
再跑一次标签黑名单（只跑一次，不递归）
```

- 模型补的标签与来源标签**合并后一起判**；不能因为「模型判过了」就放行。
- 模型失败/超时 → `review`，不是 `allow`，也不退回「只看来源标签」。
- 补标结果写 `item_tags(origin='model')`，与 `origin='pixiv'` 并存，不覆盖。
- **不采信模型输出的 `rating` 分类做过滤**。现有 sidecar 的 rating 是 `g/s/q/e` 四档艺术性分级，不是内容安全等级（见 [PLAN.md](../PLAN.md) 的实测结论）；本服务只用它补标签集合，不用它决定 allow/block。
- 必须记录 `tag_budget_hit_total` 指标，按来源分维度；上线两周后按真实分布决定阈值调 4 还是 6（按 13.9% 的基线，调到 6 会让约四分之一作品进模型，需要先确认 box 的推理吞吐）。

### 7.6 fail-closed

- 规则文件解析失败 → 服务**拒绝启动**，不是降级为「全部放行」；已运行的实例保持旧快照并告警。
- 单条目标签缺失、模型异常、DB 异常、签名失败 → `review`。
- 不存在「默认 allow」的兜底配置；第 9 步的 allow 是「黑名单、R-18G、重复、观察词、尺寸全部跑完且没命中」的结果，不是异常路径。
- `rating='r18g'` 一旦判定，人工操作也不能改成 `allow`；要放行只能改规则文件并升版本。

### 7.7 规则文件是唯一权威

结论：**纯配置文件，不建可写的规则表。** 理由是这套规则属于声明式配置（一组 user id、一组标签），不是运行时业务数据。

| 维度 | 配置文件（采用） | 数据库规则表（不采用） |
|---|---|---|
| 改规则 | 编辑文本 + 重载 | 要写管理页，或手写 SQL |
| 审查/回滚 | `git diff` / `git revert` | 得自己实现版本 diff 与回滚 UI |
| 灾难恢复 | 拷一个文件，或从仓库检出 | 要恢复 DB 或导出表 |
| 首次配置 | 复制示例，填两个数组 | 先跑迁移，再灌数据 |
| 并发 | 单操作员，无冲突 | 要处理并发写与冲突合并 |

什么时候才该换成表：规则破千条、需要按规则单独统计命中率并在线启停、或要支持多人各自的黑名单。到那时文件格式已是结构化 TOML，一条命令即可导入，迁移成本低。

**DB 的两个正当用途，都不替代文件：**

1. `rule_snapshots` —— 每次重载存一份只读副本。一年后回查「这张图当时为什么被拒」，不依赖 box 上有没有 git 仓库。
2. `rule_proposals` —— 管理页的待应用草稿箱。UI **不直接改写 TOML**。

**为什么 UI 不能直接写文件**：会丢注释、会和手工编辑冲突、会和 git 工作区打架、systemd 用户的文件权限也可能不对。所以走「草稿 → 人工确认 → 原子改写」。

### 7.8 规则变更流程

```text
管理页点「拉黑作者 / 拉黑标签」
  → 写 rule_proposals(kind, payload, source_item_id, state='pending')
  → 页面显示「待应用：+user 1234567（来自 review #4412）」
  → 人工点「应用」
      → 读当前规则文件，算 hash
      → 与「当前生效快照的 config_hash」比对；不一致 → 说明有人手工改过，中止并提示先重载
      → toml_edit 只做最小改动（保留注释、键序、空行）
      → 校验新内容（词条归一化后不重复、rules.version 必须变大）
      → 写 <file>.tmp（同目录）→ fsync → rename 覆盖
      → rename 前再算一次 hash；若期间文件又被改动 → 中止
      → SIGHUP 或内部触发重载
      → 重载成功：proposal.state='applied'，写 rule_snapshots
      → 重载失败：proposal.state 保持 pending，写 apply_error，旧快照继续生效
```

硬性要求：

- **程序应用草稿时自动递增版本号**（`2026-11-22.1` → `.2`）；手工编辑时由人自己改，改漏了会被 `config_hash_check` 拦住。
- 写文件用 `toml_edit`，不用 `serde` 反序列化再序列化——后者会丢掉全部注释和格式。
- 先写临时文件再 `rename`，不允许原地截断写；断电时要么是旧文件，要么是新文件。
- 同一时刻只允许一个应用操作（文件锁）；检测到并发修改直接中止，不自动合并。
- 应用前在管理页展示 diff 预览（新增/删除哪几行），确认后才落盘。
- 手工编辑文件仍然是一等公民：改完 `SIGHUP` 即可，不经过 proposal 流程，但同样要过校验。

### 7.9 为什么不用数据库规则表

如果真建一张 `rules(kind, payload_json, enabled, priority)` 表并让引擎读它，会出现三个具体问题：

1. **双源**：文件和表各说一套，排查时无法确定哪条生效。必须再引入「以谁为准」的规则，等于自己造一个配置系统。
2. **丢失历史**：表里的行更新是覆盖式的，`git diff` 那套审查能力没有了；要补就得自己写审计表，成本高于直接用文件。
3. **UI 与 CLI 打架**：你在服务器上用编辑器改文件，管理页又写表，两边不同步，最后还是要人工核对。

`priority` 字段尤其危险：一旦规则有顺序，判定结果就依赖排序稳定性，而 TOML 数组顺序 + 固定代码顺序已经足够表达，不需要再叠一层运行时优先级。

## 8. 审核队列

### 8.1 管理页

- 只监听 `127.0.0.1` 或 VPN 地址；默认不暴露公网。
- 列表：缩略图、来源、作者、标签、命中规则、分数、时间。
- 操作：放行、拒绝、拉黑标签/作者、按当前规则重跑、打开原图。
- 批量操作必须二次确认；所有操作写 `decisions` 与操作者标识。
- 「拉黑作者」把 user id 写进 `rules.blacklist.users`；「拉黑标签」把归一化后的词写进 `rules.blacklist.tags`。两者都先落 `rule_proposals`（草稿），**不直接改文件**；点「应用」后走 §7.8 的原子改写，自动升 `rules.version`，并在页面上先给 diff 预览。
- 规则页要显示：当前生效的 `rules.version`、`config_hash`、文件路径、上次重载时间、待应用草稿数、上次重载失败原因。
- R-18G 条目在列表里显示为「终态拒绝」，不提供放行按钮（见 §7.6）。

### 8.2 审核语义

- 放行 → `decision=allow`，进入下一批推送。
- 拒绝 → `decision=block`，不推送；保留证据。
- 放行不改变 `rules_version`；审核记录单独可追溯。
- 人工放行只能作用于 `review` 条目；命中用户黑名单、标签黑名单或 R-18G 的条目不允许人工放行（要放行先改规则文件并升版本）。
- 审核页不提供「永久删除原图」；清理走独立运维任务。

## 9. 推送

### 9.1 批次

实际 wire shape 与 `kanalib-wall-model::IngestBatch` 一致：`batch_id` 走
`X-Kanalib-Batch-Id` 头；条目平铺 `IngestItem`，`decision` 与 `revision` 在条目层。

```json
{
  "source": "pixiv",
  "rules_version": "2026-11-22.1",
  "items": [
    {
      "revision": 3,
      "decision": "allow",
      "source_id": "200",
      "id": "…",
      "width": 1200, "height": 1600, "name": "001.jpg",
      "rating": "S", "star": 4, "favorite": true,
      "tags": [{"name": "tag", "category": "general", "score": 0.92}],
      "mtime": 1700000000, "group": "原创", "orig": "jpg", "sharp": 512,
      "work_id": "work-1", "page_count": 2, "page": 0,
      "author": "作者", "author_id": "author-1", "title": "作品标题"
    }
  ]
}
```

- 只包含 `decision=allow`；网页后端收到非 allow 整批 400。
- `build_allow_batch()` 还要求条目在 `decisions` 里有对应的 `allow` 记录；没有决策记录不推送。
- 当前 box schema 没有 `rating/star/favorite/group` 列，构建批次时暂用 `G/0/false`；P3 增加字段后必须改为真实值，再进入 allow 批次。
- `r18g` 不会出现在 allow 批次里；`tag_source` 等 P3 字段尚未进入 wire shape。
- `batch_id` 是幂等键；重试不改变内容。
- 批次大小可配置（建议 50–200 条）；失败整批重试，不做半批提交。

### 9.2 签名

- `X-Kanalib-Timestamp`：Unix 秒；与网页后端时间差 ≤ 5 分钟。
- `X-Kanalib-Signature`：`HMAC-SHA256(body, shared_secret)`。
- `X-Kanalib-Batch-Id`：幂等键。
- 共享密钥只存 box 与网页后端的密钥文件（0600）；轮换时支持新旧双密钥过渡。

### 9.3 删除事件

- 来源 404/下架 → 推送 `POST /internal/ingest/delete`，只带 `id` 与 `revision`。
- 网页后端只置 `deleted=1`，不物理删行；前端隐藏。
- 删除事件同样幂等；乱序时以更高 `revision` 为准。

### 9.4 原图回源

- 网页后端用共享的媒体签名密钥自行生成短期签名 URL（默认 5 分钟），302 到 box；不额外调用 box 的签发接口。
- box 只校验签名、过期时间与条目存在性；不提供目录列表。
- 签名 URL 只对该 `item_id` 有效；不暴露 `original_path`。
- **桌面优先直连 box** 拉原图/批量下载，不经过 2c2g 云；这是主要的流量节省点。
- 网页代理原图只作为私网内的兜底；不让公网浏览器走 `box → 云 → 浏览器` 的双重出口。

### 9.5 私网入口

```bash
# box：EasyTier 节点（管理页 + 签名媒体只绑虚拟 IP）
easytier-core --hostname kanalib-box -i 10.126.126.3 \
  --network-name kanalib --network-secret '<强随机>' \
  --secure-mode --accept-dns --dev-name et0 \
  -p https://<公网域名>/easytier/peers.txt
```

- 只监听 `10.126.126.3:8790`；不绑 `0.0.0.0`，公网扫描不到。
- 桌面/网页用 `kanalib-box.et.net` 访问，不写死 `10.126.126.3`。
- 私网用 secure mode 加密 + 明文 HTTP；不依赖私有 CA。
- 公网不开放 box 端口；如需公网原图，单独评估签名媒体端口，不作为默认。

## 10. 配置

拆成两份文件：`ingest.toml`（服务参数，改动要重启）与 `rules.toml`（规则，支持热重载）。理由见 §7.7——规则要能 `git diff`、能回滚、能被 UI 原子改写；服务参数不需要。

### 10.1 主配置 `ingest.toml`

```toml
[service]
bind = "10.126.126.3:8790"
db = "/var/lib/kanalib-ingest/ingest.db"
media_dir = "/var/lib/kanalib-ingest/media"
public_base = "http://kanalib-box.et.net:8790"
# 规则文件与主配置分离：主配置改完要重启，规则文件支持热重载
rules_file = "/etc/kanalib/rules.toml"
sync_interval_secs = 60

[source.pixiv]
enabled = true
bookmark_users = []
# 二选一：access token 直填，或 refresh token + OAuth 客户端凭据（推荐）
# access_token_file = "/etc/kanalib/pixiv-access-token"
refresh_token_file = "/etc/kanalib/pixiv-refresh-token"
client_id = "pixiv-oauth-client-id"
client_secret = "pixiv-oauth-client-secret"
# keyring 尚未实现；填 credential_ref 会在启动时显式报错
# credential_ref = "keyring:kanalib-pixiv"
user_agent = "Kanalib/0.1 (Pixiv ingest)"

[download]
concurrency = 3
retry_max = 5
timeout_secs = 60

[tagging]
backend = "http"
endpoint = "http://127.0.0.1:8791/tag"
concurrency = 2
timeout_secs = 30

[push]
web_base = "http://kanalib-web.et.net:8788"
secret_file = "/etc/kanalib/ingest-hmac"
batch_size = 100
max_attempts = 5
backoff_base_secs = 5
backoff_max_secs = 600

[media]
signing_key_file = "/etc/kanalib/media-signing-key"
# 默认复用 [service].bind；需要拆端口时才覆盖
# bind = "10.126.126.3:8790"
```

仓库根部的可直接复制版本见 [`deploy/ingest.toml.example`](../../deploy/ingest.toml.example)。当前版本只支持令牌文件：`credential_ref` 会显式报错，不会静默回退。

### 10.2 规则文件 `rules.toml`

```toml
# 唯一规则源。改完 SIGHUP 重载；管理页「应用草稿」也写这个文件。
# 校验：词条归一化后重复 → 自动去重 + 告警；同词跨 tags/watch → 拒绝启动；
# version 必须比当前生效值大；rating.block 只允许 r18g。

[rules]
version = "2026-11-22.1"
# 内容变了但版本没变 → 拒绝重载并告警，防止"改了文件以为生效了"
config_hash_check = true

[rules.blacklist]
# 作者黑名单：Pixiv user id（数字，字符串也接受）。命中即整作者 block。
users = []
# 标签黑名单：归一化后精确匹配 name / translated_name。
# 词表自己列全同义词（loli / ロリ 各写一条），服务不做隐式映射。
# 归一化后重复 → 自动去重 + 告警；同一个词同时出现在 tags 与 watch → 拒绝启动。
tags = [
  # "guro",
  # "loli",
  # "ロリ",
]
# 观察词表：命中不拦截，只标记原因并进 review。
# 用于误杀风险高的泛词（触手 / 拘束 / 腹筋 等），先看命中量再决定是否升级到 tags。
watch = [
  # "おっぱい",
  # "tentacle",
]

[rules.rating]
# 只拦 R-18G；R-18 与全年龄显式放行。
block = ["r18g"]
allow = ["all", "r18"]
# 三路字段全缺且 sanity_level >= 该值时转 review，而不是 allow。
sanity_review_threshold = 6

[rules.ai]
# AI 生成（illust_ai_type = 2）正常入库。
allow = true

[rules.tag_budget]
# 内容标签（去重、剔除体裁词与 Nusers入り）少于该值 → 交打标模型补标签。
min_tags = 5
# 剔除词：体裁/形式词与收藏数系统标签，不计入内容标签数。
ignore = ["オリジナル", "漫画", "イラスト", "original", "manga", "illustration"]
ignore_suffixes = ["users入り"]

[rules.dedup]
enabled = true
on_conflict = "block"     # block | review

[rules.size]
min_width = 512
min_height = 512
on_fail = "review"
```

### 10.3 约定

- 密钥不写配置文件；只写路径。
- 地址用魔法 DNS `*.et.net`，不写 `10.126.126.x`；迁移时只换节点。
- 权限：`rules.toml` 由服务运行用户可读写（UI 要能应用草稿），`ingest.toml` 只读；两者都不进仓库的真实值，仓库只放 `.example`。
- 规则文件是唯一规则源：改完 `SIGHUP` 重载（或管理页点「重载」），不需要重启服务；重载时先校验再原子替换快照。
- 重载失败（语法错、语义冲突、版本未升）→ 保持旧快照继续跑，管理页显示错误，不进入「全部放行」。
- 非规则配置（bind、路径、并发、密钥路径）改动需要重启。
- 两份文件都建议纳入备份；`rules.toml` 还应进私有 git 仓库，这样每次改词表都有 diff 可查。

### 10.4 初始词表

用户 2026-11-22 给定 27 条，归一化后 26 条（`QOS` 与 `qos` 合并）。补充变体 118 条，再按 2026-11-22 二次定调把 `おっぱい`、`巨乳` 两个泛词移到 `watch`，当前 `tags` **142 条**、`watch` **18 条**，归一化后无重复、两表无交集。分组只为可读，服务侧是一张平表，顺序无意义。

二次定调的边界：只降 `おっぱい`、`巨乳` 这两个**光秃秃的泛词**。它们的组合变体照旧硬拦——`おっぱい星人`、`おっぱい飲み`、`おっぱい巨大化`、`巨乳娘`、`巨乳フェラ`、`巨乳フェチ`、`巨乳女子`、`超巨乳` 全部仍在 `tags`。

分层原则：

- **`tags`（硬黑名单）**：用户明确列出的 27 条（`おっぱい`、`巨乳` 两个泛词除外，见下），加上「同一概念的确定变体」（其他语言、片假名、下划线/空格写法）。命中即 `block`。
- **`watch`（观察）**：我补充的、但会出现在正常作品里的泛词。命中只进 `review`，不拦截。

#### 词表内容不在本文档维护

初始词表（用户原词 27 条 + 补充变体 118 条，归一化后 `tags` 142 条 / `watch` 18 条）与 9 个作者黑名单 ID，全部落在 `deploy/rules.toml.example`，分组注释也在那里。本文档只保留分层原则和决策口径，不再抄一份词条列表——两个副本必然漂移，而且整段词表塞进设计文档会让任何读到该段的会话上下文被上游内容审核拦掉（词条密度过高即命中，与用途无关）。

- 要看或改词表：直接编辑 `deploy/rules.toml.example`，或线上 `/etc/kanalib/rules.toml`。
- 归一化只做 NFKC / 去空白 / 小写，**不折叠下划线与连字符**，所以 `big_breasts` 与 `bigbreasts`、`nipple_fuck` 与 `nipplefuck` 这类要各写一条，不算是重复。
- `crates/kanalib-ingest/src/rules.rs` 的 `shipped_rules_template_parses` 会加载模板并断言条数（9 / 142 / 18），改词表时同步改这个断言。
- 别对本文档跑整份 `git diff` / `git log -p`：删除行里还留着旧词表，词条密度一高就被上游内容审核拦掉（`data_inspection_failed`），跟改动内容无关。看改动用 `git diff --stat` 或 `--name-only`；要读词表当前状态就读 `deploy/rules.toml.example`。

#### 观察词的取舍口径

进 `watch` 而不是 `tags` 的判断标准只有一个：**这个词会不会大量出现在正常作品里**。会，就只观察不拦截。泛词与它的明确组合分开处理——泛词本体进 `watch`，指向确定的组合变体留在 `tags`（`おっぱい` 在 `watch`，`おっぱい星人` / `おっぱい飲み` / `おっぱい巨大化` 在 `tags`；`巨乳` 同理）。角色名与作品名只用精确组合标签拦截，`sonic` / `tails` / `スターレイル` / `オレグ` 单独出现一律放行。

**另外提醒**：`爆乳`、`獣人`、`ケモノ`、`furry` 这类词在 Pixiv 上覆盖范围也大于「违规」，目前按「一律拒绝」留在 `tags`。上线两周后看 `decision_reason_total{kind="tag_blacklist"}` 的分词命中量，杀量最大的几个可以照 `おっぱい` / `巨乳` 的先例降到 `watch`。

### 10.5 继续扩词的方法

`deploy/rules.toml.example` 里 `tags` 的 142 条是「能凭语言知识确定」的部分。精确匹配意味着**没写进去的写法就是漏网**，所以扩词要走数据，不靠猜：

1. **从种子词拉相关标签**：对每个概念组取 1–2 个种子词（如 `爆乳`、`獣人`、`ボテ腹`、`猿轡`、`アヘ顔`、`皮モノ`），调 Pixiv 的相关标签接口（`/v1/illust/related` 返回同作品的相关标签）与搜索补全（`/v1/search/autocomplete`），把返回标签收进候选池。
2. **候选池去噪**：按归一化去重；剔除作品名、角色名、画师名、纯体裁词；只保留「指向同一概念」的词。判断不了的一律先放 `watch`，不放 `tags`。
3. **命中量预演**：用已入库数据对候选词跑 dry-run，输出「命中条数 / 命中样例 / 其中会被其他规则先拦掉的比例」。命中量异常高的词先降级到 `watch`——这类词通常是泛词，不是漏词。
4. **落盘走草稿**：候选词写进 `rule_proposals`（§7.8），人工确认后才 `toml_edit` 写 `rules.toml` 并升版本；不要在管理页直接改文件。
5. **别指望同义词自动合并**：服务不做词干化、罗马字转换、翻译映射。`ケモノ` / `kemono` / `furry` 是三条，`獣姦` / `獸姦` / `兽奸` 也是三条。少写一条就少拦一类写法。

需要继续盯的高风险缺口（这些概念在 Pixiv 上写法特别散，建议优先补）：

| 概念组 | 现有条数 | 还可能漏的方向 |
|---|---|---|
| 巨乳系 | 32 | 英文 `huge tits` / `big boobs` 一类口语写法；中文「大奶」类口语；`爆乳` 的繁简与异体 |
| 兽人 / 兽交系 | 32 | 具体物种组合（`狼` / `狐` / `豚` 已有部分）；英文 `feral` 类；`ケモノ` 的罗马字变体 |
| 妊娠系 | 16 | `孕み` 等动词变形；英文 `impregnation`；`出産` 的异体 |
| 乳首 / 授乳系 | 10 | 英文 `suckling` / `breastfeeding`；具体授乳场景词 |
| 肌肉 / 体格系 | 15 | 英文 `buff` / `jacked` 类口语；`腹パン` 的异体 |
| 表情 / 面部系 | 9 | `アヘ顔` 的罗马字与英文变体（`ahegao` 已有）；其他表情词 |
| 束缚 / 口塞系 | 8 | 具体器具名（`スパイダーギャグ`、`ハーネスギャグ` 等） |
| 皮物 / 变身系 | 7 | 英文 `skinsuit` 的复数/连写；`スーツ` 类泛词不要碰 |
| QOS | 7 | 黑桃系的其他语言写法 |
| 丸呑み | 4 | `丸呑み` 的更多异体 |

`watch` 里的词每轮复盘只看一件事：命中量里有多少最终被人工放行。放行率高的说明是泛词，留在 `watch`；几乎全部被拒的，升级进 `tags`。

## 11. box 部署

### 11.1 形态

- 单个二进制 + SQLite + 媒体目录；systemd 或容器。
- 管理口只绑 EasyTier 虚拟 IP；不直接开公网。
- 媒体目录单独挂盘；原图与缩略图分目录。
- 出站只允许 Pixiv API、模型服务与网页后端；不做通用代理。
- 入站零公网端口；桌面/网页只通过 EasyTier 私网访问。

### 11.2 资源

- 下载与打标是主要开销；`ImageSemaphore`/`ModelSemaphore` 防止打满。
- SQLite WAL；每日备份 DB；媒体按条目可重建（原图除外）。
- 现成脚本：`deploy/backup/kanalib-ingest-backup.sh`（DB + 媒体硬链接快照 + `rules.toml`），`deploy/monitor/kanalib-health.sh` 检查推送/下载/review 积压。
- 日志结构化；记录 `item_id`、`batch_id`、`rules_version`，不记录令牌与绝对路径。

### 11.3 可观测性

| 指标 | 用途 |
|---|---|
| `queue_depth{state}` | 发现下载/打标积压 |
| `decision_total{decision}` | allow/block/review 比例 |
| `decision_reason_total{kind}` | 按 `author_blacklist` / `tag_blacklist` / `r18g` / `dedup` / `watch` / `size` 分桶，看谁在杀量；`watch` 桶是决定「泛词该不该留在观察表」的依据 |
| `tag_budget_hit_total{source}` | 因标签不足转模型的次数；调阈值的唯一依据 |
| `tag_budget_short_total{source}` | 补标后仍不足阈值的次数；过高说明模型标签质量或阈值有问题 |
| `rules_version_info{version}` | 当前生效版本；与 `config_hash` 一起做漂移告警 |
| `model_errors_total` | 模型健康 |
| `push_failures_total` | 网页联通 |
| `review_age_seconds` | 审核积压 |
| `source_errors_total{kind}` | 来源限流/认证 |

告警：模型连续失败、推送连续失败、review 超过阈值、磁盘不足。

## 12. 测试

### 12.1 单元/集成

- `SourceAdapter` mock：分页、404、429、auth 失败、快速追平。
- 下载：`.part` 续传、Range、部分成功、重启恢复、HTML 错误页拒绝。
- ugoira：帧数上限、无限循环、首帧缩略图。
- 打标：超时 → review、空标签 → review、模型不可用 → 暂停。
- 规则：用户黑名单、标签黑名单（归一化精确匹配）、R-18G 三路、R-18 与 AI 放行、尺寸、哈希、顺序、版本记录。
- 观察词：命中 `watch` → `review` 且 `reason` 含 `watch:<词>`；不得拦截；同一词同时在 `tags` 与 `watch` → 启动报错。
- 标签不足：`< min_tags` 触发补标一次；合并后仍不足不阻塞；模型失败 → review。
- 配置校验：`version` 未升但 `config_hash` 变化、`rules.rating.block` 出现非 `r18g` 值、同一词同时在 `rating.block` 与 `rating.allow` → 拒绝加载并报错；词条归一化后重复 → 自动去重 + 告警（不拒绝启动）。
- 规则写入：`toml_edit` 保留注释与键序；并发修改（mtime/hash 变化）→ 中止且不落盘；`rename` 后内容完整；版本号自动 +1。
- proposal：pending → applied / rejected 状态流转；应用失败保留 pending 并写 `apply_error`；重复应用同一 proposal 幂等。
- 决策：任何异常不得产生 allow；`decisions` 与 `items.decision` 一致。
- 推送：签名、过期、幂等、重试、删除事件、乱序 revision。
- 回源：签名有效/过期/错误 id；不泄露路径。
- 私网：管理口只绑虚拟 IP；公网扫描不到 8790；私网内可访问。

### 12.2 端到端（mock）

1. mock Pixiv 返回 2 页收藏，第 2 页全已入库 → 只拉第 1 页。
2. 下载中断 → 重启续传 → 完成。
3. 黑名单标签 → block，网页批次中不存在。
4. 模型超时 → review，网页批次中不存在。
5. 人工放行 → 下一批 allow → 网页出现。
6. 来源 404 → deleted 事件 → 网页隐藏。
7. R-18 作品（`x_restrict=1` + `R-18` 标签）→ allow，且不被 AI 标记影响。
8. R-18G 三种形态各一条（`x_restrict=2` / 仅标签 / 仅 `restriction_attributes`）→ 全部 block。
9. 标签 3 个且无黑名单命中 → 触发补标 → 补出黑名单标签 → block；补出干净标签 → allow。
10. 标签 3 个但模型不可用 → review，不是 allow。
11. 作者在黑名单 → 无论标签多干净都 block。
12. 管理页拉黑一个标签 → 生成 pending 草稿 → 应用 → 文件新增该行、`rules.version` 加一、重载后新条目被 block、旧 `allow` 条目不变。
13. 手工改文件但忘升版本 → 重载被拒绝，旧快照继续生效，管理页报错。
14. 命中 `watch` 词 → `review`，`reason` 含 `watch:<词>`，网页批次里不出现；人工放行后下一批出现。
15. 同一词同时写进 `tags` 与 `watch` → 启动失败并指出该词。
16. `tags` 里写 `QOS` 与 `qos` → 启动成功，归一化后只保留一条，告警一次。

### 12.3 手动

- 真实 Pixiv 令牌只做手动冒烟，不写入仓库/CI。
- box 重启后队列恢复；网页断网时推送重试；恢复后不重复。
- 管理页审核放行/拒绝各一条，网页结果符合预期。
- 桌面从 box 拉 100MB 原图：`easytier-cli route` 的 `next_hop` 为 `DIRECT`，云出口流量接近 0。

## 13. 阶段

| 阶段 | 内容 | 退出条件 |
|---|---|---|
| I0 | 契约与 schema、`SourceAdapter`、mock 来源 | 单测通过；无外发 |
| I1 | Pixiv 元数据 + 下载队列 + ugoira | 中断恢复；多页正确 |
| I2 | 打标后端 + 规则加载/校验 + 决策 + 标签不足补标 + review | fail-closed；黑名单必 block；R-18/AI 放行；R-18G 三路全拦 |
| I3 | 推送 + 删除 + 原图签名 | 幂等/重试/签名测试通过 |
| I4 | 管理页 + 指标 + 备份 | 审核闭环；告警可达 |
| I5 | 部署到 box + 与网页联调 | 只出现 allow；可回滚；零公网入站 |

> 依赖：I0 的 item 契约与 `web-dynamic-backend.md` 的 DTO 一致；I3 依赖网页后端 W4 的内部 ingest 接口。

## 14. 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| 参考项目 GPL-3.0 | 许可污染 | 只借鉴流程/字段；代码独立重写 |
| Pixiv 条款/版权 | 账号或再分发风险 | 令牌只在 box；默认只处理公开收藏；下载需显式开启 |
| 模型误判 | 违规内容进网页 | 黑名单（用户 + 标签）+ R-18G 三路 + review；fail-closed；审计 |
| box 被攻破 | 令牌/原图泄露 | 最小权限、磁盘加密、管理口回环/VPN；默认零公网入站 |
| 推送重复/乱序 | 网页目录不一致 | 幂等键 + revision + 删除事件 |
| 2c2g 容量 | 网页慢 | 只推缩略图；原图回源；批次限流 |
| EasyTier 不可用 | 私网推送/原图中断 | 公网只读不受影响；重试；必要时 frp/公网 HMAC 回退 |
| 磁盘增长 | box 打满 | 原图保留策略、缩略图可重建、容量告警 |
| 规则漂移 | 同图不同结果 | `rules_version` + 决策记录 + 手动重跑 |
| UI 写规则文件冲突 | 手工编辑被覆盖 / 注释丢失 | `toml_edit` 最小改动 + 写入前比对 hash/mtime + 文件锁；不一致即中止 |
| 规则文件被误删/写坏 | 服务无法启动 | 每次重载留 `rule_snapshots`；启动失败时管理页可导出上一快照，人工恢复 |
| 版本号忘记递增 | 改了规则却不生效 | 程序自动 +1；`config_hash` 变化但版本未变直接拒绝重载 |
| 黑名单词表写偏 | 误杀正常作品 | 只精确匹配标签；词表走审核页「先预览命中量再启用」；每次改词表升版本，可一键回滚 |
| 阈值 5 过严 | 模型调用量暴涨、review 积压 | 阈值配置化 + `tag_budget_hit_total` 指标；必要时只对「可疑体裁」补标 |
| 只靠 `x_restrict` 判分级 | 字段缺失时漏掉 R-18G | 三路判定；三路全缺且 `sanity_level >= 6` 转 review |

## 15. 待确认

| # | 问题 | 推荐 |
|---|---|---|
| I-Q1 | 打标模型放 box 本机还是远程 | box 本机 sidecar；远程仅作备选 |
| I-Q2 | 分级与 AI 策略 | **R-18 放行、AI 生成放行、只拦 R-18G**；配置里 `rules.rating.allow = ["all","r18"]`、`rules.ai.allow = true` 显式声明 |
| I-Q3 | review 默认保留多久 | 90 天；原图按磁盘策略清理，决策记录保留 |
| I-Q4 | 推送失败是否阻塞后续批次 | 不阻塞；按条目/批次重试，网页按 revision 合并 |
| I-Q5 | 是否允许来源自带标签直接 allow | 不允许；仍需过规则引擎 |
| I-Q6 | 作者头像是否进网页 | 进；只推最终头像 URL/缩略图，不推原始响应 |
| I-Q7 | 是否支持多 box 实例 | v1 不支持；单实例 + 备份恢复 |
| I-Q8 | box 是否开公网入站 | **否**；默认零公网端口，只走 EasyTier 私网 |
| I-Q9 | 桌面拉原图走哪 | **直连 box**；不经过 2c2g 云 |
| I-Q10 | 标签黑名单初始词表 | 用户 2026-11-22 给定 27 条，补变体后 `tags` 142 条 + `watch` 18 条（§10.4，`おっぱい`/`巨乳` 按二次定调在 `watch`）；空表时启动告警（不是拒绝启动），管理页显示「黑名单为空」 |
| I-Q11 | 用户黑名单初始 ID | 用户给定 9 个：`108664166, 88232335, 101499615, 80373205, 49157160, 92724137, 3006351, 24518360, 18512565`（§10.4）；后续靠审核页「拉黑作者」一键写入草稿，人工确认后落盘 |
| I-Q12 | `min_tags = 5` 是否长期成立 | 暂定 5；按 §7.5 实测约 13.9% 进模型队列，上线后看 `tag_budget_hit_total` 与 review 积压再调 |
| I-Q13 | 规则存文件还是存 DB | **文件**（§7.7）；DB 只存只读快照与待应用草稿，不存可写规则 |
| I-Q14 | 管理页能否直接改规则文件 | **不直接改**：写 `rule_proposals` → 人工确认 → `toml_edit` 原子改写（§7.8） |

## 16. 调研依据（v2 新增）

### 16.1 Pixiv 字段事实

| 字段 | 类型 | 结论 | 证据强度 |
|---|---|---|---|
| `x_restrict` | `int`，可缺失/`null` | `0=全年龄 / 1=R-18 / 2=R-18G` | 强：gallery-dl（MIT，活跃）与 PixivBatchDownloader（GPL-3.0）各自维护同映射；本项目只作三路之一 |
| `sanity_level` | `int` | 取值 `0/2/4/6`，越大越敏感；**不是** R-18/R-18G 的等价物 | 强：PixivBatchDownloader 明确列出取值集合；样本中 `x_restrict=0` 的作品也有 `sanity_level=4`，说明两者不同轴 |
| `illust_ai_type` | `int` | `1=非 AI`、`2=AI`；**`0` 也存在**（字段缺失/未标注的默认值） | 强：`pixivpy` 151 条样本里 `0` 出现 80 次、`1` 出现 71 次；Pixiv-Archive 也按 `COALESCE(ai_type,1)` 兜底 |
| `restriction_attributes` | `string[]` | 实测只见过 `restricted_mode`（出现在 `x_restrict=0` 的作品上），**未见过 `r18g`/`ai`** | 弱：只能作加分项，不能作判据；也说明它和分级不是一回事 |
| `tags[].name` / `translated_name` | `string` / `string?` | 原名必有，译名可能为空；同一标签两种写法 | 强：样本普遍如此 |

由此得出两条硬结论：

1. **不能只用 `x_restrict`**。字段缺失时它会变成 `null`，只看它等于给缺失字段开绿灯。
2. **不能只用 `illust_ai_type` 判 AI**。`0` 既可能是「未标注」也可能是旧客户端默认值，语义不唯一；本项目反正不拦 AI，所以不依赖它做拒绝，只把它作为元数据透传。

还有一条值得记录的坑：网上流传过一份把 `1` 写成 `R-18G`、`2` 写成 `R-18` 的说明（某第三方搜索服务的 README），与两个主流实现的映射相反。那类代码实际只判 `x_restrict != 0`，并不依赖 1/2 的语义，所以错误一直没被发现。**结论：单一来源不可信；分级必须多路交叉，且不要用「等于某个具体值」做唯一放行条件。**

### 16.2 标签数量

- 投稿侧标签上限为 **10**（多个第三方说明一致，未见官方一手页面）；样本中 41.1% 的作品正好 10 个，`> 10` 一次都没出现，与该上限吻合。
- 只剔 `Nusers入り`：平均 7.34，`< 5` 占 10.6%，`< 6` 占 16.6%。
- 再剔体裁词（本方案口径）：平均 6.56，`< 5` 占 **13.9%**，`< 6` 占 23.8%。
- 结论：阈值 5 会让约七分之一的作品进模型，是可接受的默认值；调到 6 会跳到约四分之一，需要先确认 box 的推理吞吐；调到 4 则几乎只覆盖极端少标签作品。**先按 5 上线，用指标说话。**
- 样本偏差提醒：这批样本来自推荐/排行/相关接口，偏热门作品，真实收藏流里少标签作品的比例可能更高；因此 `tag_budget_hit_total` 必须按来源分别统计，不能只看总量。

### 16.3 参考实现的取舍

| 参考 | 做法 | 本项目是否采用 |
|---|---|---|
| Pixiv-Archive（GPL-3.0） | 分级完全靠标签 `IN ('R-18','R-18G')`，不用 `x_restrict` | 部分采用：标签作为三路之一，另加 `x_restrict` 与 `restriction_attributes` |
| PixivBatchDownloader（GPL-3.0） | `xRestrictMap` 显式 `0/1/2` 映射；AI 用 `1/2/未知` 三态 | 采用映射与三态思路，代码独立实现 |
| `pixivpy`（MIT） | 只声明字段类型，不做语义映射 | 用作字段事实与样本来源 |
| gallery-dl（MIT） | `ratings = {0:"General", 1:"R-18", 2:"R-18G"}` | 作为 `x_restrict` 映射的第二个独立证据 |
| PixivUtil2（MIT） | 登录态下用 `xRestrict` 表示「网站浏览限制」而非单作品分级 | 不作为单作品分级依据；说明同名概念有歧义 |

许可边界不变：以上项目**只读参考**，不复制代码、注释、结构体命名；本项目 MIT。

## 17. 待改动项（代码现状 → 目标态）

> 2026-11-22 核对 `crates/kanalib-ingest` 当前实现。本文件其余章节描述的是**目标态**；下表是把它落地必须做的改动，按依赖顺序排列。行号对应核对时的版本，改动后可能偏移，以符号名为准。

### 17.1 阻断性：分级与 AI 字段口径错误

现状是「字段缺失 = 全年龄 = 放行」，与 §7.4 的三路判定直接冲突。这是唯一会导致**漏放 R-18G** 的一类问题，必须最先修。

| # | 文件 | 现状 | 目标 |
|---|---|---|---|
| B1 | `src/source/mod.rs:40-41` | `rating: String`、`ai_marked: bool` | `rating: Rating`（`All/R18/R18G/Unknown` 四态枚举）、`ai: AiType`（`No/Ai/Unknown` 三态），`Unknown` 不得折叠成 `All` |
| B2 | `src/source/pixiv.rs:278` | `rating: Option<String>`，读一个 Pixiv 不存在的字段 | 删掉；改为读 `x_restrict`（`Option<i64>`） |
| B3 | `src/source/pixiv.rs:342` | `rating: wire.rating.unwrap_or_else(\|\| "G")` | `x_restrict: 0→All / 1→R18 / 2→R18G / 缺失→Unknown`，**不得** `unwrap_or(All)` |
| B4 | `src/source/pixiv.rs:280,343` | `ai_marked: Option<bool>` + `unwrap_or(false)` | 读 `illust_ai_type`：`2→Ai`、`1→No`、`0`/缺失→`Unknown` |
| B5 | `src/source/pixiv.rs:264-311` | `WireWork` 无 `sanity_level`、`restriction_attributes` | 补这两个字段并透传；`WireTag` 补 `translated_name` |
| B6 | `src/source/mock.rs:44-45` | 固定 `"G"` / `false` | 按新枚举给默认值，并补 `R18` / `R18G` / `Unknown` 三种 mock 样本 |
| B7 | `src/ingest.rs:118-119` | 直接透传旧字段 | 透传新枚举；`Unknown` 时保留 `NULL` 语义 |

### 17.2 阻断性：数据库 schema 缺列

现在 `works` 表存不下三路判定所需的原始值，`decisions` 表也存不下「用了哪套标签判定」。这两处不补，§7.2 的第 3、5 步无法实现，也无法回溯。

| # | 文件 | 现状 | 目标 |
|---|---|---|---|
| B8 | `src/db.rs:49` | `rating TEXT NOT NULL DEFAULT 'G'` | `rating TEXT NOT NULL DEFAULT 'all'`（`all/r18/r18g/unknown`）+ `x_restrict INTEGER`（可空）+ `sanity_level INTEGER`（可空）+ `restriction_attributes TEXT NOT NULL DEFAULT '[]'` |
| B9 | `src/db.rs:50` | `ai_marked INTEGER NOT NULL DEFAULT 0` | 改为三态：`ai_type TEXT NOT NULL DEFAULT 'unknown'`（`ai/no/unknown`），或保留整数但允许 `NULL` |
| B10 | `src/db.rs:47` 后 | 无 `tag_count` | 加 `tag_count INTEGER NOT NULL DEFAULT 0`（来源内容标签数，§3.2 不变量 9） |
| B11 | `src/db.rs:138-147` | `decisions` 只有 `reason` / `matched_rules_json` / `tags_json` | 补 `tag_source TEXT NOT NULL DEFAULT 'source'`、`tag_count_used INTEGER NOT NULL DEFAULT 0` |
| B12 | `src/db.rs:626-634` | `SourceWorkRow { rating: String, ai_marked: bool }` | 跟随 B1/B8 改类型 |
| B13 | `src/db.rs:327-351` | `upsert_work` 只写 `rating` / `ai_marked` | 写入新增的 5 个字段 |

**迁移方式**：`SCHEMA_VERSION` 从 1 升到 2，新增 `MIGRATION_V2`（`ALTER TABLE ... ADD COLUMN`）。这是 box 服务自己的库，与桌面/网页无关，不涉及用户数据迁移；但**已入库的旧行**必须回填：`rating='G'` → `rating='unknown'`，`ai_marked=0` → `ai_type='unknown'`，避免把"当年没记"当成"当年确认全年龄"。

### 17.3 阻断性：规则层基线审计（2026-11-21）

本节记录规则层落地前的基线；`rules`/`tagging`/`review`/`pipeline`/`rules_state`/`rules_edit`/`admin`/`metrics` 现已落地，现状与剩余项以 [`pending/ingest-i2-handoff.md`](pending/ingest-i2-handoff.md) 为准。

| # | 文件 | 现状 | 目标 |
|---|---|---|---|
| B14 | `src/db.rs:129-136` | 有可写的 `rules` 表（`kind`/`payload_json`/`priority`/`enabled`） | **删除**；换成 `rule_snapshots`（只读快照）+ `rule_proposals`（草稿），见 §3 |
| B15 | `src/db.rs` | **已完成**：schema v7 含只读 `rule_snapshots` 与 `rule_proposals`；草稿→应用→重载链路已接通 | 按 §3 建表 |
| B16 | `src/lib.rs` | 只导出 4 个模块 | 增加 `pub mod rules; pub mod tagging; pub mod review; pub mod rules_edit;` |
| B17 | 新建 `src/rules.rs` | 不存在 | `RuleConfig` 反序列化 + 校验（`rating.block` 只允许 `r18g`、版本必须递增、词条归一化后重复→告警去重、`tags`/`watch` 同词→拒绝启动）+ 十步决策（含 `watch`） |
| B18 | 新建 `src/rules_edit.rs` | 不存在 | proposal → `toml_edit` 原子改写 → 版本自增 → hash 比对（§7.8） |
| B19 | 新建 `src/tagging.rs` | 不存在 | `TaggingBackend` trait + sidecar/HTTP 实现 + 超时重试 |
| B20 | 新建 `src/review.rs` | 不存在 | review 队列、放行/拒绝、禁止放行 R-18G |
| B21 | `Cargo.toml:21-36` | 无 TOML 解析依赖 | 加 `toml`（读）、`toml_edit`（写）；文件锁按需加 `fs2` |
| B22 | `deploy/rules.toml.example` | **已完成**：按 §10.2 的形态给出可直接复制的示例，词表（`tags` 142 / `watch` 18）与 9 个 `users` ID 都在这个文件里，由 §10.4 指向 | 后续改词表只改这个文件并升 `rules.version`；`rules.rs` 的 `shipped_rules_template_parses` 会断言条数 |
| B22a | `src/rules.rs` | — | `blacklist.watch` 是新配置项，`RuleConfig` 必须有对应字段；漏掉会被 `serde` 静默忽略（无 `deny_unknown_fields`），表现为「观察词表不生效」 |

### 17.4 非阻断：配置与启动

| # | 文件 | 现状 | 目标 |
|---|---|---|---|
| B23 | `src/main.rs` | **已完成**：`--config ingest.toml` + `clap` 覆盖项；未知键由 `deny_unknown_fields` 拒绝 | `rules_file` 指向 `rules.toml`（规则层未落地前只告警） |
| B24 | `src/main.rs` | **已完成**：`mock` / `pixiv` 均可启动；`--source` 缺省由 `source.pixiv.enabled` 决定 | 真实来源只在显式开启时启动 |
| B25 | `src/main.rs` | **已完成**：Unix `SIGHUP` + 每轮 mtime 变更检测触发重载；失败保持旧快照并记日志 | 保持 fail-closed |
| B26 | `src/main.rs` | **已完成**：Pixiv 走 `reqwest` 图片传输（Range、Referer、状态/头透传）；mock 只在 `--source mock` 下使用 | 保持两者按来源选择 |

### 17.5 非阻断：推送与对外接口

`src/push.rs` 已实现队列、签名、重试、删除事件与原图签名校验，并已接进主循环：每轮从库里挑 `decision='allow'` 且 `revision > pushed_revision` 的条目生成批次，ack 后回写追踪位；删除事件按 `works.status='deleted'` 生成。规则层落地后 `allow` 会正常产生；批次 `rules_version` 默认取当前生效版本，`config.push.rules_version` 仅用于显式覆盖或回放历史批次。

| # | 文件 | 现状 | 目标 |
|---|---|---|---|
| B27 | `src/push.rs` | **已完成**：`enqueue_decisions` 从 DB 生成 allow/delete 批次；`flush` ack 后回写 `pushed_revision` / `delete_pushed` | 批次大小与重试参数来自 `[push]` |
| B28 | `src/admin.rs` | **已完成**：审核放行/拒绝、规则草稿/预览 diff/应用、状态、指标、健康检查；默认只绑回环，部署时绑 EasyTier 虚拟 IP | 审核页 + 规则页 + 健康检查；只绑 EasyTier 虚拟 IP |
| B29 | `src/media.rs` | **已完成**：`GET/HEAD /original/{id}` 校验签名/过期/条目存活，单区间 Range，流式返回；只绑配置地址 | 路径不进入响应；EasyTier 未就绪时 30s 重试绑定 |

### 17.6 已完成、不需改动

核对时确认这些已经符合设计，别在 P3 里重复造：

- `src/download.rs`：`.part` + Range + 续传 + 重启恢复 + 缩略图生成，已实现。
- `src/db.rs`：`items.decision` 有 `CHECK (pending/allow/block/review)` 约束；`item_tags` 已有 `origin` 列，能区分 `pixiv` / `model`。
- `src/source/pixiv.rs`：错误分类（`auth`/`not_found`/`rate_limited`/`network`/`internal`）、429 退避、`ApiGate` 已实现。
- `tests/ingest_flow.rs`：mock 全链路、续传、多页、429 退避已有覆盖。

### 17.7 顺序建议

```text
B14 → B8/B9/B10/B11（schema 一次改完，升 v3；v2 已被 I3 的 push 列占用）
  → B1..B7（字段口径，含 mock 与测试夹具）
  → B21/B22/B15（依赖与示例配置）
  → B17/B16（规则加载与决策，先 mock 后端）
  → B19（打标后端）
  → B18（规则编辑）
  → B23..B26（配置与重载）
  → B20（review 页）
  → B27..B29（推送与对外）
```

理由：schema 与字段口径是所有下游的地基，先改完再写规则，能避免「规则按旧字段写、schema 一改全废」。B14 放在最前，是因为留着可写 `rules` 表会诱导后来的人直接读它，等于把双源问题固化进代码。
