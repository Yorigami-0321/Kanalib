# 三服务缺陷审查（2026-11-25）

对象：网页端、筛选服务（kanalib-ingest）、打标服务（sidecar/tagger）。
方式：只读静态审查，逐条核对当前工作区代码行号；未编译、未运行、未修改任何受跟踪文件。

## 0. 基线与仓库

三个服务已就地建立嵌套 Git 仓库（无 remote、未推送），提交只包含各自目录：

| 服务 | 仓库根 | 首次提交 | 文件数 |
| --- | --- | --- | --- |
| 网页端前端 | `web/` | `7da1479` | 42 |
| 筛选服务 | `crates/kanalib-ingest/` | `2c23080` | 25 |
| 打标服务 | `sidecar/tagger/` | `cda4d15` | 3 |

说明：

- 网页端后端 `crates/kanalib-web/` 与契约 crate `crates/kanalib-wall-model/` 仍留在主仓；在不移动文件的前提下，它们无法与 `web/` 放进同一个嵌套仓库。
- 提交时的主工作区有另一个进程正在开发 AVIF 支持（11 个 `M` 文件，含 `media.rs`、`download.rs`、`push.rs`、`routes/ingest.rs`）。本审查的行号对应提交当时的工作区快照，若该进程继续改动，行号可能漂移。
- 构建产物未入库：`web/` 使用自身 `.gitignore`，另两个仓库通过 `.git/info/exclude` 排除 `target/`、`.kanalib/`、`__pycache__/`、`*.pyc`。

## 1. 严重度统计

| 级别 | 数量 | 含义 |
| --- | --- | --- |
| P0 | 6 | 可直接造成越权、注入、内存/IO 打爆或功能整体失效 |
| P1 | 14 | 明确的功能缺陷、安全弱化或明显的资源风险 |
| P2 | 14 | 边界条件下的正确性/可用性问题 |
| P3 | 7 | 低风险信息泄露、健壮性或体验问题 |

## 2. 网页端（web + kanalib-web 后端）

### 2.1 后端

| ID | 级别 | 位置 | 缺陷与影响 | 修复方向 |
| --- | --- | --- | --- | --- |
| W-01 | P0 | `crates/kanalib-web/src/db.rs:920`、`src/routes/read.rs:79-88`、`src/query.rs:55` | `SortKind::Random` 直接映射为 `i.id`，`seed` 被解析后丢弃。所有用户看到同一"随机"顺序且刷新不变；与 `docs/web-dynamic-backend.md` 的稳定 seed 承诺冲突。 | 用 `hash(id, seed)` 之类的确定性洗牌生成排序键。 |
| W-02 | P0 | `crates/kanalib-web/src/db.rs:162-175`（调用点 `db.rs:991`、`1041`、`1423`） | `rebuild_search_index` 每次先 `DELETE` 再全表 `INSERT…SELECT`。每个 allow 批次、删除、desktop push 都是 O(N) 写放大并长时间持有写锁，2c2g 上用合法小批次即可拖垮。 | 按受影响 item_id 增量更新 FTS，或改为触发器/定期合并。 |
| W-03 | P1 | `crates/kanalib-web/src/routes/read.rs:111-128` | 游标只校验 `sort`/`ascending`，不绑定 `q`/`tag`/`group`/`rating`/`fav`/`seed`。同一游标换过滤条件复用会静默跳行或重复。 | 把过滤条件哈希写入游标并在解码时校验。 |
| W-04 | P1 | `crates/kanalib-web/src/db.rs:897-907` | `escape_like` 只转义 `\` 和 `%`，但 SQL 使用 `ESCAPE '\'`。搜索 `a_b` 会命中 `axb`。 | 同时转义 `_`。 |
| W-05 | P1 | `crates/kanalib-ingest/src/push.rs:786`、`push.rs:460`、`web/src/source/StaticSource.ts:87-88`、`web/src/lib/assetUrl.ts:40-49`、`crates/kanalib-web/src/routes/read.rs:282-305` | `sharp` 契约三处不一致：ingest 硬编码 `Some(0)` 且只上传 t256/t512；前端只接受 0/1024/2048，`detailSharpSources` 对 0 返回空数组；后端仍宣告四档。详情页永远拿不到 t1024/t2048，放大必糊。 | 统一档位契约，并真正生成、上传大档缩略图。 |
| W-06 | P1 | `crates/kanalib-web/src/routes/mod.rs:68-92`、`src/response.rs:12-18`、`deploy/nginx/kanalib.conf.example` | SPA fallback 与静态响应只有 `no-cache` 和 `nosniff`，没有 CSP、X-Frame-Options、Referrer-Policy、HSTS，nginx 示例也没有。点击劫持与注入的危害被放大。 | 在统一中间件/反代层补齐安全头。 |
| W-07 | P1 | `crates/kanalib-web/src/routes/desktop.rs:308-371` | 私网 `upload` 写入 `<media>/uploads/`，不写库、不被 `/media/{tier}` 服务，形成只增不减的磁盘死区。 | 入库并提供读取路径，或加大小/数量上限与清理。 |
| W-08 | P2 | `crates/kanalib-web/src/routes/original.rs:46-56` | 原图 302 的 `Location` 直接拼 `{box_origin}/original/{id}`，把 `10.126.x.x` 内网地址暴露给任何持设备 token 的客户端。 | 改为反代相对路径，或只回跳转路径不回源站地址。 |
| W-09 | P2 | `crates/kanalib-web/src/media.rs:23`、`99`、`322` | 缩略图返回 `public, max-age=31536000, immutable`，而 ETag 取自 `updated_at`。同 id 换图但 `updated_at` 未变时，客户端会永久使用旧图。 | 用内容哈希做 ETag，或缩短缓存时间。 |
| W-10 | P2 | `crates/kanalib-web/src/db.rs:1227-1237` | `upsert_tag` 在 category 非空时直接覆盖，任意一个批次的标签分类会改写全局 facet 分组。 | 仅首次写入，或引入人工锁定/来源优先级。 |
| W-11 | P3 | `crates/kanalib-web/src/routes/read.rs:245`、`255` | `/api/health` 公开返回版本号，便于指纹识别。 | 移除版本或仅对私网监听返回。 |

### 2.2 前端

| ID | 级别 | 位置 | 缺陷与影响 | 修复方向 |
| --- | --- | --- | --- | --- |
| W-12 | P1 | `web/src/App.tsx:532-553` | 静态模式为了筛选，对每个非详情路由逐片 `loadMore()` 直到全库加载；`allLoaded` 仅在 `loadedShards === 0` 时重置（`App.tsx:559-561`）。大库首屏即被拖死，内存持续上涨。 | 改成本地索引按需查询，或对筛选态分页 + 虚拟化。 |
| W-13 | P2 | `web/src/App.tsx:336-339` | 深链详情页的浏览序列回退到 `orderedItems`，只含已加载页。深链条目不在首屏时 prev/next 不可用、放映中断。 | 按详情 id 单独请求前后条目。 |
| W-14 | P2 | `web/src/components/WallCard.tsx:148-152` | `handleError` 依赖闭包中的 `tierIndex`，同一图片连续多帧 error 可能一次跳两档，跳过可用降级源。 | 用函数式更新 `setTierIndex(i => i + 1)` 并去重。 |
| W-15 | P3 | `web/src/state/filters.ts:187-191` | 无搜索索引时只匹配文件名；且筛选态关闭虚拟化，每次按键全量重排。 | 保留轻量索引或对输入做防抖，筛选态维持虚拟化。 |

## 3. 筛选服务（kanalib-ingest）

| ID | 级别 | 位置 | 缺陷与影响 | 修复方向 |
| --- | --- | --- | --- | --- |
| I-01 | P0 | `crates/kanalib-ingest/src/admin.rs:63-85`、`src/main.rs:493-533` | 管理面路由没有任何中间件或 token，只按配置绑定（默认 `127.0.0.1:8791`，生产示例 `10.126.126.3:8792`）。EasyTier 内任意节点可审核 allow/reject、创建并应用规则草稿（直接改黑名单并热重载）、recheck、读取原图。 | 加共享 token/HMAC 鉴权 + 来源 IP allowlist，管理面只绑私网。 |
| I-02 | P0 | `crates/kanalib-ingest/src/admin.rs:803`、`839-869` | 管理页用 `innerHTML` 拼 HTML，`esc` 把 `'` 转成 `&#39;`，但属性值经 HTML 解码后仍进入内联 JS 字符串；`author_id`、`tags[0]` 来自远端作品元数据。恶意作者名/标签可在管理员浏览器执行脚本，等同管理员权限。 | 用 DOM API/`textContent` 构建，交互参数放 `data-*`，不拼接内联 `onclick`。 |
| I-03 | P1 | `crates/kanalib-ingest/src/media.rs:184-215`、`crates/kanalib-web/src/routes/original.rs:28-44` | 原图回源只校验 `works.status != 'deleted'` / `item_is_live`，不看 `items.decision`。review/block 条目仍可拉原图。 | 同时要求 `decision = 'allow'`，两服务保持一致。 |
| I-04 | P1 | `crates/kanalib-ingest/src/admin.rs:385` | 管理端原图用 `tokio::fs::read` 整文件读入内存，无 Range、无大小上限。点开大图即抬高峰值内存。 | 流式响应 + 大小上限 + 支持 Range。 |
| I-05 | P1 | `crates/kanalib-ingest/src/pipeline.rs:43`、`src/tagging.rs:80-97`、`src/config.rs:138` | 打标串行遍历全部 pending；每次重试新建 blocking Client；默认超时 1800 秒。单条最坏阻塞整条流水线数小时。 | 并发打标、缩短超时、失败快速降级到 review。 |
| I-06 | P1 | `crates/kanalib-ingest/src/config.rs:128`、`137` | `tagging.concurrency` 有定义、有默认值，但全仓从未使用，与 `deploy/tagger/README.md` 的说明矛盾。 | 真正接入并发执行，或删除该配置并改文档。 |
| I-07 | P1 | `crates/kanalib-ingest/src/ingest.rs:60`、`129`、`src/db.rs:369-382` | 每条发现的作品都调用 `persist_bookmark_image`，该函数无条件写 `favorite=1`。public/feed 来源的条目全部被标成收藏，收藏视图失真。 | 由来源携带 favorited 标志，或拆分 URL 持久化与收藏语义。 |
| I-08 | P2 | `crates/kanalib-ingest/src/download.rs:196-199` | 目标文件已存在时直接 finalize，不比对大小/sha。崩溃后残留的坏文件会被永久接受。 | 命中已有文件也校验大小/哈希，不匹配则重下。 |
| I-09 | P2 | `crates/kanalib-ingest/src/download.rs:235-244`、`268-282` | `expected_bytes` 只在 200 响应校验；206 续传只比较 `Content-Range` 总量，上游返回错误字节数时静默接受。 | 续传完成后再按 `expected_bytes` 做一次总校验。 |
| I-10 | P2 | `crates/kanalib-ingest/src/download.rs:78-110`、`src/main.rs:229`、`272` | 下载队列循环到 pending 为空才返回，决策与推送在其后执行。积压时决策/推送被饿死。 | 每轮设配额（条数/时长），让流水线阶段公平调度。 |
| I-11 | P2 | `crates/kanalib-ingest/src/push.rs:460-520`、`588` | 缩略图逐条逐档串行上传，任一条失败走 `mark_retry` 导致整批复传。 | 限并发上传，失败按条目重试。 |
| I-12 | P2 | `crates/kanalib-ingest/src/admin.rs:176-265`（`:240`、`:244`） | 审核列表每行再查 `item_tag_names` 和 `is_r18g_item`，200 行产生 400+ 次查询并占用阻塞线程池。 | 批量查询或在 SQL 中一次性 join。 |
| I-13 | P2 | `crates/kanalib-ingest/src/config.rs:205-206` | 密钥文件 0600 权限检查只在 `#[cfg(unix)]` 下生效，Windows 不校验。 | 增加 Windows ACL 校验或启动时显式告警。 |
| I-14 | P3 | `crates/kanalib-ingest/src/pipeline.rs:170` | `tag_count` 取 `max(tags.len())`，可能高于实际打标数。 | 以实际来源计数为准。 |

已复核通过：规则层 R-18G 四路硬拦（rating / `x_restrict=2` / tag / restriction_attributes）、黑名单先于标签预算、watch 转 review、异常一律 fail-closed；未发现 fail-open 路径。

## 4. 打标服务（sidecar/tagger）

| ID | 级别 | 位置 | 缺陷与影响 | 修复方向 |
| --- | --- | --- | --- | --- |
| T-01 | P0 | `sidecar/tagger/http_server.py:63-72`、`97-104` | 按 `Content-Length` 直接 `rfile.read()`，没有最大值；服务端是 `ThreadingHTTPServer`。任意对端并发提交大 body 可打爆 Windows 主机内存。 | 设 body 上限，超限直接 413；限制并发连接数。 |
| T-02 | P0 | `sidecar/tagger/http_server.py:50-94`、`deploy/tagger/kanalib-tagger-proxy.py:18-23` | `/tag` 与 `/health` 无鉴权；代理默认监听 `10.126.126.2:8793` 并转发 `10.126.126.1:8793`，也没有 allowlist。网内任意节点可白嫖 GPU、注入标签。 | 共享密钥/签名 + 来源 IP allowlist + 限流。 |
| T-03 | P1 | `sidecar/tagger/http_server.py:23-32`、`63` | multipart 用 `split`/`strip` 手工解析；`boundary=x; charset=` 时会取到脏值。合法客户端也可能 400 或解析出错误图片。 | 使用标准 multipart 解析库。 |
| T-04 | P1 | `sidecar/tagger/http_server.py:92`、`117-123` | 错误响应回显 `类型: 异常信息`，`/health` 泄露模型绝对路径。 | 只回稳定错误码，内部细节写日志。 |
| T-05 | P1 | `sidecar/tagger/server.py:287`、`41` | 批量推理先把所有图片 `load_rgb` 进内存再推理；`MAX_TAG_PIXELS=1e8` 单张 RGB 可达数百 MB。 | 逐张加载/推理，或限制批次总像素与并发。 |
| T-06 | P2 | `sidecar/tagger/server.py:65-81`、`308` | 进程级修改 `ImageFile.LOAD_TRUNCATED_IMAGES`，多线程互相影响；tolerant 路径接受截断图并继续打标。 | 用局部解码参数，避免改全局状态。 |
| T-07 | P2 | `sidecar/tagger/server.py:136` | `trust_remote_code=True`，模型目录被替换即任意代码执行。 | 关闭远程代码，或对模型目录做哈希校验。 |
| T-08 | P2 | `deploy/tagger/kanalib-tagger-proxy.py:46-49` | 建连 10 秒超时后 `settimeout(None)`，无空闲超时；`request_queue_size` 默认 5。慢连接可长期占用线程。 | 设置读写/空闲超时并提高 backlog。 |
| T-09 | P3 | `sidecar/tagger/server.py:118` | 关键一致性用 `assert`，`python -O` 下会被移除。 | 改为显式抛错。 |

## 5. 跨服务

| ID | 级别 | 位置 | 缺陷与影响 | 修复方向 |
| --- | --- | --- | --- | --- |
| X-01 | P1 | ingest → tagger 调用链 | ingest 与 tagger 之间没有共享密钥，打标结果可伪造；良性标签可满足 `min_tags` 把条目推向 allow（`x_restrict==2`/rating 仍硬拦，防线被削弱）。 | 打标请求/响应加签名，服务端校验来源。 |
| X-02 | P3 | `crates/kanalib-ingest/src/push.rs:660`、`crates/kanalib-web/src/routes/ingest.rs:211` | 缩略图上传签名使用空 `batch_id`，300 秒窗口内可重放覆盖同路径文件（同内容幂等，风险低）。 | 签名纳入批次 id 或 nonce 并做去重。 |
| X-03 | P2 | `docs/web-dynamic-backend.md`、`deploy/tagger/README.md` | 三处文档漂移：random seed、`tagging.concurrency`、`sharp` 档位，均为文档承诺、代码未实现。 | 以代码为准修订文档，或补齐实现。 |

## 6. 优先修复顺序

| 顺序 | 编号 | 理由 |
| --- | --- | --- |
| 1 | I-01 | 管理面零鉴权，可直接改黑名单并热重载，影响所有入库内容。 |
| 2 | I-02 | 管理页存储型 XSS，等价于管理员权限。 |
| 3 | T-01 | 无 body 上限 + 多线程，单机内存可被远程打爆。 |
| 4 | T-02 | 打标服务零鉴权，GPU 与标签可被网内任意节点滥用。 |
| 5 | W-02 | 每批次全量重建 FTS，小批次即可打满磁盘 IO 与写锁。 |

## 7. 审查方法与限制

- 逐文件阅读三个服务的核心路径：HTTP 入口、鉴权/签名、DB 写入、下载与推送、打标调用、规则判定、前端数据流。
- 所有行号已在提交时的工作区中复核；`db.rs`、`push.rs`、`download.rs`、`media.rs`、`routes/ingest.rs` 在另一进程的 AVIF 改动后仍有漂移可能。
- 未执行编译、测试或压测；结论为静态分析，P0/P1 的资源类判断基于代码结构（全表扫描、无上限读取、串行阻塞），未做实测数据。
- 未检查运行时配置与部署环境（证书、反代、EasyTier ACL、Windows 服务账户），实际风险以部署形态为准。
