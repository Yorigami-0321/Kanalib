# 并行执行工作流（v1）

> 上级：[pixiv-integration.md](pixiv-integration.md) · 网页后端：[web-dynamic-backend.md](web-dynamic-backend.md) · 入库服务：[ingest-service.md](ingest-service.md) · 侧栏：[web-rail-horizontal.md](web-rail-horizontal.md) · 私网：[easytier-connectivity.md](easytier-connectivity.md) / [private-networking-options.md](private-networking-options.md)
>
> 状态：P0（`W0` 契约）、P1（侧栏横向化、私网部署准备）、P2（`W1` 只读后端、`I0`→`I1` 入库骨架）、P3（`W2` 搜索/游标/facets、`W3a` ApiSource、`W3b` setQuery）、P4（`W4` 私网写入/桌面桥接、`I3` 推送队列与原图签名）、P5（`W5`+`I5` 部署/备份/监控模板）已落地；P3 打标与 P5 桌面桥接未开始，P5 实机安装与告警未验收。本文件只回答“哪些能同时做、按什么优先级做、每个工作流怎么落地”，不含工时与排期。
>
> 组件选型与可复用项目：[p2-reuse-research.md](p2-reuse-research.md)（P2 之后的 crate/项目、许可、版本冲突与复用级别）。

## 0. 阅读方式

- 本文不重述需求，只把 [pixiv-integration.md](pixiv-integration.md) §10 的阶段拆成**可并行的工作流**。
- 每个工作流给出：目标、落点、执行步骤、退出条件、禁止事项。
- 优先级是**依赖顺序**，不是日历顺序：P0 不做完，P2/P3 会返工。
- 并发上限按 4 条车道估算；超过 4 条时按 §5 的顺序排队。

## 1. 唯一的共享闸门

所有并行都卡在同一个东西上：**条目契约**。

| 闸门 | 内容 | 为什么必须先做 |
|---|---|---|
| `W0` | `crates/kanalib-wall-model`：`WallItemDto`、`WorkDto`、`WorkPageDto`、`AuthorDto`、`CollectionDto`、`PageDto<T>`、`Capabilities` + 转换 + 单测 | 入库服务 I0、网页后端 W1、网页 `ApiSource`、静态发布器共用同一形状；先各写一份必然返工 |

冻结标准：

- 静态 `WallItem` 字段（`id/w/h/n/rt/st/fav/tags/g/ph/mtime/orig/sharp`）原样保留，动态只加可选字段。
- `schema` 保持 1；静态/动态用 `api_version` + `capabilities` 区分。
- 字段命名、可空性、时间单位（秒）、id 规则（40/64 位小写 hex）一次定死。
- `kanalib-publish` 现有测试不回归。

`W0` 由**一个人**完成并合并；合并前其他人只读契约文档，不新建 DTO。

## 2. 优先级总览

| 优先级 | 工作流 | 对应阶段 | 依赖 | 可并行对象 |
|---|---|---|---|---|
| **P0** | 契约冻结 | `W0` | 无 | 只此一件，先做完 |
| **P1** | 网页侧栏横向化 | `P1`（F-087） | 无 | 与 P0 并行；与 P2/P3 并行 |
| **P1** | 私网方案实测与部署准备 | `F-091` / `F-090` | 无 | 与 P0/P1/P2/P3 全部并行 |
| **P2** | 网页后端只读骨架 | `W1`（F-088） | P0 | 与 P3 并行 |
| **P2** | 入库服务骨架 + Pixiv 适配 | `I0`→`I1`（F-084/085/086） | P0 | 与 P2 网页后端并行 |
| **P3** | 打标与规则 | `I2`（F-089） | P2 入库骨架 | 与 P4 并行 |
| **P3** | 网页后端搜索/游标/facets | `W2`（F-088） | P2 网页后端 | 与 P3 打标并行 |
| **P3** | 网页 `ApiSource` | `W3a`（F-088） | P2 网页后端只读接口 | 与 P3 其余并行 |
| **P4** | 内部 ingest 接口 + 桌面桥接 | `W4`（F-088） | P3 网页后端、P3 打标 | 集成点，单点收口 |
| **P4** | 推送 / 删除 / 原图签名 | `I3`（F-089） | P4 `W4` | 与 P4 桥接并行（不同文件） |
| **P5** | 桌面桥接客户端 | `P5`（F-090 可选） | P4 `W4` | 与 P5 部署并行 |
| **P5** | 部署、反代、备份、监控 | `W5` + `I5` | P4 全部 | 最后收口 |

## 3. 各工作流怎么做

### P0 · 契约冻结（`W0`）

**目标**：产出唯一一份三端共用的 DTO 与转换，之后所有服务只依赖它。

**落点**

| 文件 | 内容 |
|---|---|
| `crates/kanalib-wall-model/src/lib.rs` | DTO 定义、`Capabilities`、分页 |
| `crates/kanalib-wall-model/src/convert.rs` | 内部记录 → DTO；无 IO、无 DB |
| `crates/kanalib-wall-model/src/lib.rs` 单测 | 序列化形状、可选字段缺省、id 规则 |

**步骤**

1. 从 [web-dynamic-backend.md](web-dynamic-backend.md) §4.1 原样落 DTO，不改字段名。
2. 写 `serde` 属性：可选字段 `skip_serializing_if = "Option::is_none"`，与静态 JSON 对齐。
3. 加转换函数：静态 `index.json` 条目 → `WallItemDto`；入库记录 → `WorkDto`/`AuthorDto`。
4. 在 `Cargo.toml` 的 workspace members 里**一次性**加入新 crate；不要留给后续工作流各自改。
5. 加契约测试：同一份样本分别走静态转换与动态转换，序列化结果字段集合一致。

**退出**

- `cargo test -p kanalib-wall-model` 通过。
- `kanalib-publish` 现有测试不回归。
- 静态/动态样本的 JSON 字段集合一致；网页 `normalizeItem` 不需要改。

**禁止**

- 不在这个 crate 里放 HTTP、SQLite、文件 IO。
- 不引入桌面 `kanalib-core` 的 DB 类型。
- 不为了让某个服务方便而临时加字段；加字段必须同时更新本文与契约文档。

### P1 · 网页侧栏横向化（`P1` / F-087）

**目标**：把 120px 竖排「缝」改成 208px 横排文字导航，折叠 60px，窄屏底部条。

**落点**

| 文件 | 改动 |
|---|---|
| `web/src/components/Sidebar.tsx` | 根节点加 `rail` 类；结构重排；折叠状态 |
| `web/src/components/sidebar.css` | 横排布局、选中/悬停/焦点、计数 |
| `web/src/styles.css` | 只加 token 与布局变量，不重排画布 |
| `web/src/lib/layout.ts` | 折叠宽度与断点常量 |
| 探针 | `.seam` → `.rail` 选择器迁移 |

**步骤**

1. 先加 CSS token 与 `.rail` 骨架，不改 JSX；截图确认浮层与宽度。
2. `Sidebar.tsx` 加 `rail` 并列类，保留 `.seam` 与 `#seam-panel-trigger-*`。
3. 新增入口先按 `capabilities=false` 隐藏，避免空页面。
4. 改探针选择器，让 `.seam` 与 `.rail` 同时匹配。
5. 六档（1440/1280/1024/900/760/390）截图；确认无横向溢出。
6. 全绿后再删旧竖排规则与 `.seam`。

**退出**

- 折叠状态可持久化；刷新后保持。
- `--rail-w` 与 `--seam-w` 由同一变量派生，`Wall.tsx` 不改读法。
- `wall_probe_state`、`wall_probe_tag360`、`wall_probe_inspector_sheet`、`wall_probe_edge` 全绿。
- 1440/1280/1024/900/760/390 六档无横向溢出。

**禁止**

- 不改 `web/src/App.tsx` 的数据源初始化。
- 不动 `web/src/source/*`、`web/src/state/useWall.ts`。
- 不推挤画布；侧栏仍是浮层。

### P1 · 私网方案实测与部署准备（`F-091` / `F-090`）

**目标**：确认三端链路可用，并锁定“连接层可替换”的配置形状。

**落点**：部署配置、反代配置、验证脚本；**不碰业务代码**。

**步骤**

1. 按 [easytier-connectivity.md](easytier-connectivity.md) §12 做三端连通测试：桌面 → web private、桌面 → box、box → web private。
2. 记录 `easytier-cli route` 的 `DIRECT` / relay 状态；测一次 100MB 传输，对比 2c2g 网卡计数。
3. 强制 UDP 不可用，验证 TCP 11010 回退（WSS 待验证；必要时 frp）。
4. 确认公网 `/internal/*`、`/api/desktop/*` 返回 404。
5. 把验证结果写回 [easytier-connectivity.md](easytier-connectivity.md) §14 的待实测表。

**退出**

- 三条私网链路可达；退出组网后立即失败。
- 桌面 ↔ box 的 `next_hop` 为 `DIRECT`，2c2g 出口增量接近 0。
- UDP 不可用时有明确回退路径，且不需要改业务代码。

**禁止**

- 不把任何组网产品写进业务配置；只写稳定服务名 + 端口。
- 不为测试改 `crates/` 或 `web/`。

### P2 · 网页后端只读骨架（`W1` / F-088）

**目标**：`kanalib-web` 能独立启动，用临时库提供只读 API 与媒体。

**落点**

| 文件 | 内容 |
|---|---|
| `crates/kanalib-web/src/main.rs` | 配置、public/private 双 listener、静态托管、优雅退出 |
| `crates/kanalib-web/src/routes/read.rs` | `health`、`index`、`items`、`works`、`authors`、`collections`、`tags` |
| `crates/kanalib-web/src/media.rs` | t256/t512、ETag、Range、路径校验 |
| `crates/kanalib-web/src/db.rs` | 网页目录 schema、迁移、备份 |

**步骤**

1. 建 crate，依赖 `kanalib-wall-model`，不依赖 `kanalib-core`。
2. 先只开 public listener：`/api/health`、`/api/index`、`/api/items`、`/media/*`。
3. 用临时 SQLite + 固定样本跑集成测试；媒体走 `tower-http` 的 ETag/Range。
4. 再加 `works/authors/collections/tags` 只读路由。
5. private listener 先只绑回环、只放 health，写路由留到 `W4`。

**退出**

- 临时库集成测试覆盖 200/206/304/404/416。
- 媒体路径穿越测试失败即拒绝；id 正则 + DB 反查 + canonicalize 前缀。
- public listener 不注册任何写路由；请求 `/internal/*` 返回 404。
- 2c2g 上内存 <150MB，启动 <1s。

**禁止**

- 不读桌面 DB、不引入 ORM、不做 SSR。
- 不在 `W1` 做搜索/facets（留给 `W2`）。
- 不把 `search-index.json` 带进 api 模式。

### P2 · 入库服务骨架 + Pixiv 适配（`I0`→`I1` / F-084/085/086）

**目标**：box 上能跑通“mock 来源 → 作品/页/作者 → 下载队列 → 缩略图”，**暂不推送网页**。

**落点**

| 文件 | 内容 |
|---|---|
| `crates/kanalib-ingest/src/main.rs` | 启动、配置、优雅退出 |
| `crates/kanalib-ingest/src/db.rs` | 服务自己的 SQLite schema |
| `crates/kanalib-ingest/src/source/mod.rs` | `SourceAdapter` trait |
| `crates/kanalib-ingest/src/source/pixiv.rs` | OAuth、节流、错误分类、收藏/作品/作者 |
| `crates/kanalib-ingest/src/download.rs` | 队列、`.part`、Range、多页并发、重启恢复 |
| `crates/kanalib-ingest/src/ugoira.rs` | zip → GIF |

**步骤**

1. 按 [ingest-service.md](ingest-service.md) §3 落 schema；`SCHEMA_VERSION` 从 1 开始。
2. 先做 mock `SourceAdapter`，把队列状态机跑通。
3. 再接 Pixiv：OAuth、429/404/auth/network 分类、`next_retry_at` 退避。
4. 下载用 `.part` + Range；成功才 rename；重启恢复未完成项。
5. 多页并发 + 部分成功入库；缺页下次补。
6. ugoira 最后做：帧数上限 500，无限循环 GIF。

**退出**

- mock 来源全链路通过：收藏 2 页、第 2 页全已入库 → 快速追平。
- 下载中断可恢复；多页作品页序正确；404 标 `deleted` 不删元数据。
- 429 退避、auth 整批暂停有测试。
- **没有任何外发请求**到网页后端。

**禁止**

- 不把 Pixiv 令牌写进仓库或日志。
- 不在 `I0/I1` 做打标、规则、推送。
- 不修改桌面 schema 或 `kanalib-core`。

### P3 · 打标与规则（`I2` / F-089）

**目标**：把图片变成标签，再按**配置文件里的黑名单**输出 `allow / block / review`，且 fail-closed。

> **先读**：[ingest-service.md](ingest-service.md) §17 待改动项。当前 `crates/kanalib-ingest` 只实现了来源适配与下载，规则层完全不存在；schema 也缺三路判定所需字段。§17 按依赖排好了顺序（B14 → schema → 字段口径 → 规则），不要跳过前置项直接写规则。

**落点**

| 文件 | 内容 |
|---|---|
| `crates/kanalib-ingest/src/tagging.rs` | 模型后端、超时、重试、批量 |
| `crates/kanalib-ingest/src/rules.rs` | `rules.toml` 解析/校验、用户与标签黑名单、R-18G 三路、决策、版本快照 |
| `crates/kanalib-ingest/src/rules_edit.rs` | proposal → `toml_edit` 原子改写、版本自增、hash/mtime 并发检测、diff 预览 |
| `crates/kanalib-ingest/src/review.rs` | review 队列、放行/拒绝 |
| box 管理页 | review 列表、命中原因 |

**步骤**

1. 先定 `TaggingBackend` 接口，mock 后端返回固定标签。
2. 规则加载器先做：读 `rules.toml` → 归一化词条 → 校验（重复词、版本未升、`rules.rating.block` 只允许 `r18g`）→ 快照入库。校验失败拒绝启动。
3. 规则引擎先做「来源开关 + 用户黑名单 + R-18G 三路 + 标签黑名单」；再做标签预算补标、重复哈希与尺寸兜底。不做分数阈值自动拒绝。
4. 补标只跑一次：`< min_tags` → 调模型 → 合并去重 → 再跑黑名单；模型失败直接 `review`。
5. 决策顺序按 [ingest-service.md](ingest-service.md) §7.2；每次记录 `rules_version`、`tag_source`、`tag_count_used` 与命中规则。
6. 失败策略：模型超时/不可用/规则异常 → `review`，绝不 `allow`。
7. review 管理页只走 EasyTier 私网；放行/拒绝写审计；R-18G 条目在页面上不提供「放行」按钮。
8. 规则编辑走 proposal：管理页只写 `rule_proposals`，不碰文件；「应用」时用 `toml_edit` 最小改动 + 版本自增 + hash 比对，失败保留 pending 并显示原因。**禁止**引入可写的 `rules` 表。

**退出**

- 黑名单命中必 `block`；模型失败必 `review`。
- R-18 与 AI 生成能正常走到 `allow`；R-18G 三条路径都能 `block`。
- 标签不足的条目必定留下一次补标记录，或明确记为「模型失败 → review」。
- 管理页拉黑 → 草稿 → 应用 → 文件 diff 只增一行、版本 +1、重载生效；并发改文件时应用被安全中止。
- 规则版本可追溯；同一图重跑结果可解释。
- 没有任何未过滤条目进入外发队列。

**禁止**

- 不把“模型没跑完”当成 `allow`。
- 不在规则里硬编码 Pixiv 特有字段；来源差异放适配器。

### P3 · 网页后端搜索/游标/facets（`W2` / F-088）

**目标**：把搜索、排序、计数切到服务端，支持 keyset 分页。**状态：已实现（2026-11-22）**。

**落点**：`crates/kanalib-web/src/routes/read.rs`、`db.rs` 查询层、`/api/index` 的 facets。

**步骤**

1. 先定游标格式（排序键 + id），保证稳定分页。
2. 搜索走 SQLite FTS 或 `LIKE` + 索引；与静态搜索语法保持兼容。
3. facets 与筛选同源，避免“筛选后计数不对”。
4. 加 `limit` 上限与慢查询日志。

**退出**

- 10k 条目、32 并发读：列表/排序 p95 ≤120ms，搜索 p95 ≤200ms（与 web-dynamic-backend §9 对齐）。
- 翻页不重不漏；筛选 + 计数一致。
- 与静态模式的查询语义有对照测试。

**实现备注**

- 搜索用 FTS5 trigram（≥3 字符）与 `LIKE` 兜底；标签筛选是 `EXISTS` 半连接，不物化去重候选集。
- `/api/items` 只翻页不 `COUNT`；`total/counts/facets` 统一由 `/api/index` 返回，避免每页全量计数。
- `/api/index` 额外返回 `counts.favorite`、`counts.ratings` 与带 `cover` 的标签/分组 facets，供总览页直接使用服务端聚合。
- 游标是 base64url 的 `{sort,order,key,id}`；换排序/顺序或损坏游标回 `400 invalid_cursor`。
- ignored 压测：`w2_10k_32_concurrent_p95_under_120ms`、`w2_10k_db_only_concurrency_profile`、`w2_component_profile`。

**禁止**

- 不在前端做“只过滤已加载页”的假服务端搜索。
- 不引入连接池或重型缓存；先压测再决定。

### P3 · 网页 `ApiSource`（`W3a` / F-088）

**目标**：网页能在 `VITE_WALL_SOURCE=api` 下用真实 API，默认仍是 static。**状态：W3a/W3b 已实现（2026-11-22）。**

**落点**

| 文件 | 内容 |
|---|---|
| `web/src/source/ApiSource.ts` | 实现 `WallSource`；分页/详情/媒体 |
| `web/src/source/index.ts` | `createSource()` 按环境变量选择 |
| `web/src/state/useWall.ts` | 增加 `reset()`；api 模式先 `setQuery` 再拉第一页 |
| `web/src/App.tsx` | `new StaticSource` → `createSource()`（仅此一处） |

**步骤**

1. 先只换数据源（W3a）：页面仍用现有内存筛选，保证探针通过。
2. `reset()` 处理切换来源/查询时的状态清理。
3. 媒体 URL 按 `item.sharp` 选档；原图走 302，不在前端拼 box 地址。
4. W3b 再把筛选/排序/计数切到 `setQuery`，单独提交。

**退出**

- `VITE_WALL_SOURCE=static` 行为不变。
- `VITE_WALL_SOURCE=api` 下首页、分页、详情、媒体可用。
- `npm run typecheck && npm run build` 两种模式各通过一次。
- 现有 `wall_*` 探针在 static 模式全绿。

**W3a 验收记录**

- `npm run typecheck` 通过；`VITE_WALL_SOURCE=static` 与 `api` 两次 `npm run build` 均通过。
- 静态模式：`wall_probe_state`、`wall_probe_detail_nav` 全绿（0 console error）。
- API 模式：Rust 服务 + 样例库实测首页渲染 2 个条目、`#/d/<id>` 深链走 `fetchItem` 直接命中，无数据源错误。
- `ApiSource` 的分页游标只在成功响应后推进；`fetchItem` 404 返回 `undefined`，其余错误照常抛出。

**W3b 实现备注**

- `App.tsx` 用 `server_filter` 分支：路由（tag/fav/rating/group）、搜索 `q`、排序在变化时先 `setQuery`，再 `useWall.reset()` 清空并拉第一页；详情页不重置。
- API 模式 `needAll=false`，有限墙滚到墙尾才翻页；`infinite` 只在静态模式开启，避免为无限画布拉完整库。
- 服务端已排序/筛选，`shown` 不再走 `filterItems`/`sortItems`；`random` 只做客户端稳定洗牌。
- 计数：`total` 用 `/api/index.count`；收藏/分级用 `counts.favorite`/`counts.ratings`；分组/标签封面用 facets 的 `cover`。
- 验收：`wall_api_check.mjs`（endpoint + `--ui` 请求断言）通过；2 条目样例库的首页、4 个总览页与深链在 API 模式实测正常；static 的 `wall_probe_state`/`wall_probe_detail_nav` 仍全绿。

**禁止**

- 不在 `ApiSource` 里写死域名；用同源 `/api`、`/media`。
- 不与 P1 侧栏同人同文件；`App.tsx` 只改数据源那一行。

### P4 · 内部 ingest 接口 + 桌面桥接（`W4` / F-088）

**目标**：网页后端能安全接收 box 的 allow 批次，并提供桌面 pull/push/upload。**状态：已实现（2026-11-22）。**

**落点**

| 文件 | 内容 |
|---|---|
| `crates/kanalib-web/src/routes/ingest.rs` | `POST /internal/ingest/batch`、`/delete`；HMAC + 时间戳 + 幂等键 |
| `crates/kanalib-web/src/routes/desktop.rs` | `GET /api/desktop/pull`、`POST /push`、`/upload`；设备令牌 |
| `crates/kanalib-web/src/main.rs` | private listener 挂写路由；public 不注册 |

**步骤**

1. 先做 HMAC 校验中间件：无签名 401、时间戳过期 401、重复幂等键 200 且不重复入库。
2. 批次只接受 `decision=allow`；出现非 allow 整批 400。
3. 删除事件单独接口；只置 `deleted=1`，不物理删。
4. 设备令牌只存哈希、可撤销；pull 用 `since` 游标。
5. 最后接 EasyTier 私网监听，确认公网写路由 404。

**退出**

- 签名/幂等/令牌测试全过。
- public listener 对 `/internal/*`、`/api/desktop/*` 返回 404。
- private listener 只绑 EasyTier 虚拟 IP（`10.126.126.2:8788`），不绑 `0.0.0.0`。

**禁止**

- 不提供“代拉任意 URL”的接口。
- 不让网页后端持有 Pixiv 令牌。
- 不在 public listener 上做任何写操作。

**实现备注**

- HMAC 校验在 JSON 解析之前；签名覆盖原始 body，时间戳窗口 ±300s；`X-Kanalib-Timestamp` 缺失/非法直接 401。
- 幂等键先查 `ingest_batches`，重复批次 200 `duplicate=true` 且不写；批次内先校验全部 `decision=allow`，任一非 allow 整批 400，事务未开始。
- 批次写入按 `revision` 合并且 fail-closed：低于网页当前 revision 的条目计入 `stale`，不覆盖；allow 重新入库会清 `deleted`。
- 删除只置 `deleted=1`，`revision` 不低于当前值才生效；`rebuild_search_index` 同步把 tombstone 移出 FTS。
- 设备令牌只存 SHA-256 哈希；`--issue-device NAME` 生成一次并打印，撤销走 `desktop_devices.revoked`。
- pull 用 `(updated_at, id)` keyset 游标（base64url），包含已删除条目供桌面下架；push 以 `base_revision` 做乐观并发，冲突返回服务端当前值。
- upload 是原始 body 流式写入 `media/uploads/{id}.{ext}.part`，超限立刻删除并 413；成功后 rename，不整文件读入内存。
- 原图回源只在配置了 `--box-origin` 与 `--media-signing-key` 时启用；未配置 404，公网仍 404。
- 公开 listener 不注册 `/internal/*` 与 `/api/desktop/*`；测试断言它们是 404 而不是 401。

### P4 · 推送 / 删除 / 原图签名（`I3` / F-089）

**目标**：box 把 allow 批次可靠推给网页，并能签发短期原图 URL。**状态：已实现（2026-11-22）；尚未接进主循环，等待 P3 产生 `allow` 决策。**

**落点**：`crates/kanalib-ingest/src/push.rs`、`crates/kanalib-ingest/src/media.rs`（或等价模块）。

**步骤**

1. 批次持久化；未确认批次重启后重试。
2. HMAC 签名 + 时间戳 + 幂等键；失败分类退避。
3. 删除事件单独发；404 只标 `deleted`。
4. 原图签名 URL 短 TTL；桌面优先直连 box，不经过 2c2g。

**退出**

- 断网重试、重复批次不重复入库、签名过期拒绝。
- 删除同步后网页隐藏但保留审核记录。
- 原图 URL 只能被授权设备换取。

**禁止**

- 不把原图上传到 2c2g。
- 不在日志里打印签名密钥或完整 URL。

**实现备注**

- `crates/kanalib-ingest/src/push.rs`：批次 JSON 持久化在 `push_batches.payload_json`，`kind=allow|delete`；同 id 入队幂等。
- `flush()` 只取到期批次；2xx 置 `acked`，4xx（除 408/425/429）置 `rejected` 不再重试，429/5xx/网络错误按指数退避重试，超过 `max_attempts` 置 `failed`。
- `build_allow_batch()` 从 box 库显式挑条目；发现任一非 `allow` 直接失败，不生成半批。
- `sign_original()` / `verify_original()` 与网页 `routes::original` 共用规范串 `GET\n/original/{id}\n{expires}`，默认 5 分钟。
- 测试：`crates/kanalib-ingest/tests/push_flow.rs`（7 个）覆盖幂等、5xx 重试、4xx 拒绝、最大次数、网络错误、签名与过期。

### P5 · 桌面桥接客户端（`P5` / 可选）

**目标**：桌面可选拉取/回传，默认关闭，关闭时零影响。

**落点**

| 文件 | 内容 |
|---|---|
| `crates/kanalib-bridge`（独立 crate）或桌面内独立模块 | pull/push 客户端、令牌、冲突处理 |
| `crates/kanalib-ui/src/settings.rs` | 开关、服务器地址、设备令牌、拉取目录 |

**步骤**

1. 先做只读 pull：下载到独立 `bridge_root`，再走现有扫描。
2. 再做 push：只回传显式选择的标签/星级/收藏/相册成员。
3. 冲突用 last-write-wins + `updated_at`；双向都改过进冲突列表。
4. 开关默认关闭；关闭时不启动线程、不建目录、不发请求。

**退出**

- 拉取目录可被现有扫描识别；回传字段正确。
- 冲突可见，不静默覆盖。
- 关闭开关后现有桌面功能与性能无变化。

**禁止**

- 不改现有扫描/打标/相册/回收站/发布器路径。
- 不自动同步整个桌面库。
- 不升级桌面 schema。

### P5 · 部署、反代、备份、监控（`W5` + `I5`）

**目标**：两侧可独立重启、可回滚、可告警。**状态：部署/备份/监控模板已落地（2026-11-22）；实机安装与告警通道未验收。**

**落点**：部署脚本、Nginx 配置、systemd/服务单元、备份任务、监控项；不改业务逻辑。

**步骤**

1. 网页：Nginx 只反代 public listener；显式拒绝 `/internal/*`、`/api/desktop/*`。
2. box：默认零公网入站；管理页与媒体只走 EasyTier 私网。
3. 备份：网页 SQLite 每日备份；box 原图/规则/审核记录备份；推送批次可重放。
4. 监控：健康检查、磁盘、队列积压、推送失败、证书到期。
5. 回滚：保留上一版二进制与静态包；数据库迁移前自动备份。

**退出**

- box/云两侧可独立重启，互不阻塞。
- 推送重放不重复；密钥可轮换。
- 公网写路由 404；私网内链路 `DIRECT` 或明确记录 relay。
- 告警可到达。

**禁止**

- 不在部署脚本里硬编码私网 IP。
- 不把密钥写进仓库或镜像层。

**实现备注**

- `deploy/nginx/kanalib.conf.example`：只反代 public listener；`/internal/`、`/api/desktop/`、公开原图全部 404。
- `deploy/systemd/kanalib-web.service.example`：`StateDirectory=kanalib-web` + `UMask=0077` + `EnvironmentFile`，密钥只从 `/etc/kanalib/web.env` 读；private 地址仍是占位符。
- public/private 解耦：private 绑定失败只打 warning 并每 30s 重试，public 只读照常服务；unit 用 `Wants=easytier.service` 而不是 `Requires`。已在 Windows 实测：private 地址不可绑时 `/api/health` 仍 200。
- `deploy/systemd/kanalib-ingest.service.example`：改成当前真实 CLI（`--db/--media-dir/--source`），并加 `StateDirectory`；仍用 `--source mock`，真实 Pixiv 属于 P3。
- `deploy/backup/`：网页/box SQLite 在线备份（`.backup` + `integrity_check` + gzip + 保留天数）；box 媒体用 `rsync --link-dest` 硬链接快照；`rules.toml` 一并复制。
- 推送重放：`kanalib-ingest --replay-push <BATCH_ID>` 把批次重置为 pending；内容来自 `push_batches.payload_json`，重放不改内容。
- `deploy/monitor/kanalib-health.sh`：HTTP、磁盘、推送积压/失败/拒绝、review 与下载队列；非 0 退出。systemd timer 每 5 分钟。
- 密钥轮换：`KANALIB_INGEST_SECRET` 支持 `new,old` 双密钥重叠；先加新密钥、切 box、再删旧密钥。
- 证书检查：配 `KANALIB_TLS_HOST` 时用 `openssl s_client` 检查剩余天数；`KANALIB_ALERT_WEBHOOK` 在失败时 POST 报告（地址只放 `/etc/kanalib/health.env`）。
- 脚本已在 WSL 用临时 SQLite 实测备份/恢复；shell 语法 `bash -n` 通过。Nginx/systemd 未在真实 2c2g + EasyTier 环境安装，告警通道也未接入。

## 4. 文件所有权与冲突

同一时刻只允许一个人写下列文件/目录：

| 资源 | 所有者 | 说明 |
|---|---|---|
| `Cargo.toml`（workspace members） | P0 契约负责人 | 三个新 crate 一次加完 |
| `crates/kanalib-wall-model/` | P0 契约负责人 | 冻结后只接受 additive 变更 |
| `crates/kanalib-web/src/routes/read.rs`、`db.rs` | P2 网页后端 | W2 同人续写，避免路由冲突 |
| `crates/kanalib-web/src/routes/ingest.rs`、`desktop.rs` | P4 集成负责人 | 与 read 分离，可并行 |
| `crates/kanalib-ingest/src/source/`、`download.rs` | P2 入库 | 与 tagging/rules 分离 |
| `crates/kanalib-ingest/src/tagging.rs`、`rules.rs`、`review.rs` | P3 打标 | 不与 source/download 同人 |
| `web/src/components/Sidebar.tsx`、`sidebar.css` | P1 侧栏 | 独占 |
| `web/src/source/`、`state/useWall.ts` | P3 `ApiSource` | 独占；`App.tsx` 只改一行 |
| `web/src/styles.css` | P1 侧栏 | 其他人只加组件级 CSS |
| 部署脚本/反代 | P1 私网 + P5 部署 | 私网实测先产出配置草案 |

冲突规则：

1. 一个文件同一时刻只有一个写入者；需要跨工作流改动时，由所有者代改或先拆接口。
2. 新 crate 的 `Cargo.toml` 注册、workspace 依赖提升，统一由 P0 负责人做。
3. `App.tsx`、`styles.css`、`Cargo.toml` 是三个高频冲突点，改动前先在任务里声明。

## 5. 派发顺序

按优先级派发，不按时间：

1. **P0 契约**：单独派 1 人，先合并。
2. **P1 侧栏 + P1 私网实测**：契约进行中即可并行；两条互不碰文件。
3. **P2 网页后端 + P2 入库骨架**：契约合并后同时开；两条各自新建 crate。
4. **P3 打标 + P3 搜索 + P3 ApiSource**：P2 各自接口稳定后开；三条文件不重叠。
5. **P4 内部接口 + P4 推送**：P3 完成后开；`W4` 单点收口。
6. **P5 桥接 + P5 部署**：P4 完成后开；桥接默认关闭，可最后验收。

并发位建议：

- 4 条车道时：P0/P1 侧栏/P1 私网 + 1 条预留；P2 起固定 2 条后端 + 1 条前端 + 1 条私网/部署。
- 只有 2 条车道时：先 P0 → P1 侧栏 → P2 网页后端 → P2 入库 → P3；私网实测穿插在等待编译/评审时做。

## 6. 验收与回退

### 6.1 每个工作流的最低验收

| 工作流 | 最低验收 |
|---|---|
| P0 契约 | `cargo test -p kanalib-wall-model`；publish 不回归；字段集合一致 |
| P1 侧栏 | 六档截图；折叠持久化；四个 `wall_*` 探针全绿 |
| P1 私网 | 三条链路可达；`DIRECT`；UDP 封时 TCP 11010 回退；公网写路由 404 |
| P2 网页后端 | 临时库集成测试；200/206/304/404/416；public 无写路由 |
| P2 入库 | mock 全链路；中断恢复；多页；404/429/auth 分类 |
| P3 打标 | 黑名单必 block；失败必 review；规则版本可追溯 |
| P3 搜索 | 10k/32 并发：列表/排序 ≤120ms、搜索 ≤200ms；翻页不重不漏 |
| P3 ApiSource | static 行为不变；api 首页/分页/详情/总览可用；`wall_api_check` 通过；两种模式 build 通过 |
| P4 内部接口 | 签名/幂等/令牌测试；公网 404；private 只绑回环 |
| P4 推送 | 断网重试；重复批次幂等；删除同步；签名过期拒绝 |
| P5 桥接 | 拉取可扫描；回传正确；关闭零影响 |
| P5 部署 | 两侧独立重启；重放不重复；告警可达 |

### 6.2 回退点

- 侧栏：第 2 步完成后先跑四个探针，全绿才继续；失败回退 `.seam` 保留态。
- 网页后端：`VITE_WALL_SOURCE=static` 始终可用；api 故障切回静态。
- 入库：推送关闭时服务仍可采集/打标/审核，不阻塞桌面。
- 桥接：开关关闭即回到现状，无后台线程。
- 私网：EasyTier 不可用时公网只读不受影响；私网操作明确报错并重试。

## 7. 不做

- 不做数据迁移；不导入旧库、Pixiv-Archive 数据包或缩略图包。
- 不做统一后端；网页、box、桌面各自独立库。
- 不拆分桌面现有功能；不升级桌面 schema。
- 不让外部来源直连网页；唯一入口是 box 入库过滤服务。
- 不把原图放上 2c2g；公网最多 t2048。
- 不在本文件写工时或排期。
