# 云端无界图片墙计划（F-080–F-083）

> 上级索引：[README.md](README.md) · 登记：[registry.md](registry.md) · 速览：[pending/overview.md](pending/overview.md)
>
> 2026-11-08 登记。分支 `feature/cloud-photo-wall`；本地工作台作为图片墙的后台管理工具；在做的本地条目适当精简（提案见 §6，已拍板）。
>
> 2026-11-08 用户指示改为**在本分支直接开发代码**（原「本分支只写文档」红线作废，见 §8）。接口契约冻结在 [photo-wall-contract.md](photo-wall-contract.md)，实施状态见 §9。
>
> 参考截图（音乐应用的不规则拼贴墙，仅作交互参照）：当时存于本机临时目录（`<临时目录>/images/<uuid>.png`，未入库），构图要点见 [photo-wall-design.md](photo-wall-design.md)。

## 1. 产品形态与边界

| 面 | 形态 |
|---|---|
| 墙（web） | 整屏不规则拼贴/瀑布流图片墙，无限追加加载；尺寸档位混排（大图突出、小图填充），暗色沉浸风格参照截图 |
| 单击 | 选中卡片（竖线 + 提亮）；同一张再点一次 / 双击展开全屏详情（2026-11-09 交互重做后生效） |
| 双击 / 触屏单击 | 打开详情页（标签、相似推荐、元数据），整屏大图；移动端没有 hover，单击直接展开 |
| 管理（本地工作台） | 扫描/打标/去重/标签/收藏仍是唯一写入口；新增「发布/同步到云」入口；桌面继续当后台管理工具 |
| 云端 | **只读展示**：不做上传、不做远端写回、不引入账号体系（v1 可选访问令牌，见 W4） |

边界沿用本地优先决策：多来源 = 多个本地文件夹；Immich/WebDAV 只读来源的资产 v1 不上云（本地无原图字节，只有 512 档缩略图），发布范围默认仅本地来源。

## 2. 本软件可复用为网页后端的部分

调研口径：2026-11-08 只读盘点 `crates/kanalib-core`（函数名为准，行号会漂移）。

### 2.1 直接复用

| 模块 | 入口 | 网页用途 |
|---|---|---|
| core 整库 | `kanalib-core` 不依赖 gpui/ely，单向被 UI 依赖；`KANALIB_HEADLESS` 已验证脱离 GUI 运行 | 可作为独立后端库编译，无需剥离 |
| 分页搜索 | `Db::search_page` / `AppState::search_page`（keyset 分页、SQL 下推、`MAX_SEARCH_PAGE=10_000`） | 无限滚动的分页 API 原型；keyset 天然适配 append |
| 查询语法 | `SearchQuery::parse`（`tag:`/类别/`is:`/`date:`/数值/否定）、`Filter`（Serialize/Deserialize） | 网页搜索框直接复用语法 |
| 详情 | `Db::load_asset`；字段集参照 `export::to_json`（`Asset` 无 Serialize，需写 DTO） | 详情页数据 |
| 相似推荐 | `dedup::similar_to_anchor`（100k p95 15–18ms，只读 phash 不解码） | 详情页「相似」栏 |
| 缩略图 | `.kanalib/thumbs`：512 主档 + `{stem}-t256.jpg` 网格档；`thumb_sources` 表支持淘汰后重建 | 云端图片资产直接上传这两档 |
| 标签/集合 | `list_tags_in`、`load_collections`/`collection_assets`、`load_saved_searches` | 网页筛选 chips 与「相册」入口 |
| 写操作（管理侧） | `set_favorite`/`set_star`/`add_tags`/`remove_tags`/`create_collection`/`undo_last` 等 | 桌面管理工具继续写库；发布器只读 |
| 导出 | `export::to_json`/`to_csv`（含 CSV 注入防护） | manifest 字段集参照 |
| DB 并发 | WAL + busy_timeout 5s；`AppState` 每操作新开连接 | 发布器/未来 web 进程可与桌面并行读同一库 |

### 2.2 需新建（仓库零基础，不重复造轮子处见 §3）

- HTTP/发布层：无任何 axum/actix/hyper 服务端代码；v1 走静态导出则**只需发布器 CLI**，不需要 HTTP 框架。
- `SearchCursor` 不透明、无 Serde：静态导出按排序全量分片 manifest，绕开游标；若将来上动态后端需服务端会话化。
- `Asset` → JSON DTO；鉴权（core 无用户概念）；原图 serving（本地资产有原图路径，远端资产没有）。
- Linux 部署注意（仅动态后端路线相关）：`keyring` 仅 windows-native（`CredentialBackend` 接口已留好可注入）；`trash.rs` 非 Windows 直接 bail（云端只读不需要）。

### 2.3 性能事实（引用须带库标识）

100k 合成库：名称排序全命中 p95 133ms、非名称排序 276–304ms（同库实测上限口径，150ms 不再是硬门禁）；锚点相似 p95 15–18ms；全库相似扫描 ~2.7–3.0s。静态导出后网页查询在浏览器侧，需分片控制首包体积（W2）。

## 3. 开源调研与复用结论

准则见 [README.md](README.md)「调研与实现准则」。star/许可证/推送时间为 2026-11-08 GitHub API 实测。

### 3.1 图库后端（A 类）

| 项目 | star/许可/活跃 | 复用结论 |
|---|---|---|
| [Immich](https://github.com/immich-app/immich) | 118k/AGPL-3.0/极活跃 | 备选托底：API 最全但 PG+Redis+ML 重、AGPL、API 频繁 breaking；人脸/CLIP 对动漫标签无增益，与现有工作台重复 |
| [HomeGallery](https://github.com/xemle/home-gallery) | 1.2k/MIT/活跃 | **重点借鉴**：静态导出 + 整库 JSON 进浏览器 + endless stream，正是「本地真相→云端只读」形态 |
| [LibrePhotos](https://github.com/LibrePhotos/librephotos) | 8.1k/MIT/活跃 | 借鉴前端/DRF 接口设计；ML 取向不符 |
| [Photoview](https://github.com/photoview/photoview) / [PiGallery2](https://github.com/bpatrik/PiGallery2) | 6.6k AGPL / 2.3k MIT | 借鉴只读轻量后端；不能写入自定义元数据 |
| [Lychee](https://github.com/LycheeOrg/Lychee) | 4.3k/MIT/活跃 | 借鉴 justified 墙 UI 颜值 |
| [PhotoPrism](https://github.com/photoprism/photoprism) | 40k/AGPL+品牌附加条款 | 淘汰：商标限制+会员制功能 |
| [Piwigo](https://github.com/Piwigo/Piwigo) / [Damselfly](https://github.com/webreaper/damselfly) / [Zenphoto](https://github.com/zenphoto/zenphoto) / [Ente](https://github.com/ente-io/ente) | — | 淘汰：老栈/GPL 异构/Blazor 难拆/E2EE 与只读展示冲突 |

**结论：不引入第二个图库后端。** 元数据大脑（SQLite v11、Danbooru 系标签、pHash 相似组）已建好，市售后端的 schema 装不下也不想装；轮子用在墙布局/lightbox/缩放/同步这些没做的环节。

### 3.2 前端墙与交互（B 类）

| 项目 | star/许可 | 复用结论 |
|---|---|---|
| [@egjs/infinitegrid](https://github.com/naver/egjs-infinitegrid) | 2.4k/MIT/活跃 | **直接用**：Masonry/Justified/Packing + 无限 append + DOM 回收；item 事件完全自管 |
| [react-photo-album](https://github.com/igordanchenko/react-photo-album) | 789/MIT/活跃 | 直接用（React 栈布局备选，rows/columns/masonry、srcset） |
| [react-virtuoso](https://github.com/petyosi/react-virtuoso) / [masonic](https://github.com/jaredLunde/masonic) | 6.5k/1.4k MIT | 备选：虚拟化海量图 |
| [PhotoSwipe](https://github.com/dimsemenov/PhotoSwipe) | 25.3k/MIT | **直接用**：框架无关 lightbox，手势/缩放强，自定义 UI 元素可放「详情」按钮 |
| [yet-another-react-lightbox](https://github.com/igordanchenko/yet-another-react-lightbox) | 1.3k/MIT/活跃 | 直接用（React 栈，与 react-photo-album 同作者有配套示例） |
| [react-zoom-pan-pinch](https://github.com/BetterTyped/react-zoom-pan-pinch) | 2.0k/MIT/活跃 | **直接用**：包整墙即得可缩放拼贴（「无界」缩放模式） |
| [pixi-viewport](https://github.com/pixijs-userland/pixi-viewport) | 1.2k/MIT/活跃 | 按需：十万级 sprite 墙改 WebGL 相机 |
| [OpenSeadragon](https://github.com/openseadragon/openseadragon) | 3.5k/BSD-3 | 按需：详情页超大图深缩放 |
| [lightGallery](https://github.com/sachinchoolur/lightGallery) / [fancybox](https://github.com/fancyapps/fancybox) | GPL+商业双授权 / archived | 淘汰：许可/已归档 |
| [justified-layout](https://github.com/flickr/justified-layout) / [masonry-layout](https://github.com/desandro/masonry-layout) | MIT/停滞 | 借鉴算法，优先用封装好的 infinitegrid |

**无现成「无限缩放图片墙」成品 demo**（GitHub 高 star 命中为零，商用 iZoom 非开源）→ 用「布局组件 + 缩放容器」两块现成轮子拼装，不算造轮子。

单击/双击接法（2026-11-09 交互重做后）：单击只选中高亮，同一张 360ms 内再次抬起或双击展开全屏详情；触屏单击直接展开，原 PhotoSwipe 方案已退役。

### 3.3 同步桥（C 类）

| 方案 | 复用结论 |
|---|---|
| [rclone](https://github.com/rclone/rclone)（60.9k/MIT） | **直接用**：`rclone sync 导出目录 remote:bucket --checksum`，先 `--dry-run`；单向使 dest=source，契合云端只读 |
| [Syncthing](https://github.com/syncthing/syncthing)（90k/MPL-2.0） | 备选：云端是自有 VPS 且要近实时时 receive-only |
| [Litestream](https://github.com/benbjohnson/litestream)（14.6k/Apache-2.0，beta） | 按需：确需云端可查 SQLite 再上；v1 导出 JSON 更简单 |
| [immich-go](https://github.com/simulot/immich-go) | 条件：仅当选 Immich 托底时才需要 |
| [LiteFS](https://github.com/superfly/litefs) | 淘汰：分布式活副本过重 |

### 3.4 推荐组合

**HomeGallery 式静态导出架构 + @egjs/infinitegrid 墙 + PhotoSwipe lightbox + react-zoom-pan-pinch 整墙缩放 + rclone 单向同步**。全 MIT/Apache/BSD，无 AGPL 传染。云端零数据库、零后端代码、天然只读；SQLite 不上云，导出 manifest 分片即可。

备选组合（仅当需要手机备份/多端/现成搜索时）：Immich 托底 + immich-go 上传 + 同款自定义前端；代价是 PG+Redis 运维与 AGPL、API breaking（锁版本+契约测试）。

## 4. 架构与条目分解

```
本地工作台（唯一写入口，AppState/Db）
  └─ 发布器（F-080，core 只读：search_page/load_asset/相似/缩略图档）
       └─ 导出目录：manifest 分片 JSON + t256/t512 + 可选原图
            └─ rclone sync（F-083，--checksum、先 dry-run）
                 └─ 对象存储+CDN / Nginx VPS（只读静态）
                      └─ 墙前端 SPA（F-081/F-082）
```

| ID | 条目 | 内容 | 依赖 |
|---|---|---|---|
| F-080 | 发布导出与同步 | 发布器 CLI/工作台入口：读 core 生成 manifest 分片（id/宽高/标签/评级/收藏/相似签名/缩略图路径）+ 复制 t256/t512/t1024/t2048 + 原图（W3 无失真管线）；rclone 单向推送；增量按 mtime/sha1；**不改 schema**（发布状态走 settings/导出目录清单） | 无（W1–W5 已定） |
| F-081 | 墙前端与交互 | 二维画布墙 + 无限追加；单击选中、双击/触屏单击展开全屏详情；拖拽/滚轮/Ctrl 缩放/双指缩放；左侧缝收功能键 | F-080 manifest 契约 |
| F-082 | 详情页 | 标签 chips/类别、相似推荐（manifest 内嵌相似组或锚点距离表）、元数据；超大图按需 OpenSeadragon | F-081 |
| F-083 | 云端部署与同步运维 | 托管选型落地（R2+Pages/Workers 或 VPS Nginx）；rclone 配置、dry-run/校验和/限速；公开只读（W4=A，可选 noindex）；发布审计与回滚（保留上一版 manifest） | 无（W1–W5 已定） |

manifest 契约要点（F-080 首产物）：分片按排序键切（每片 ≤5000 条，首包只载首片+索引）；字段集参照 `export::to_json` 增 `thumb256/thumb512/thumb1024/thumb2048/original?/similar_group`；path 用 `path_key::encode` 的编码串做 id，不泄漏本地绝对路径（W4 隐私口径）。

清晰度管线（W3 已定）：现有 t256/t512 为等比缩放 + EXIF 旋转 + ICC→sRGB，缩放本身无失真，失真只来自小图拉伸放大；故墙按 DPR 与 tile 尺寸选档（srcset），详情默认原图。发布器新增 t1024/t2048 档（高质量 JPEG q≈90、正确 sRGB 转换），原图随发布增量上云（checksum 去重）；存储受限回退时以最高档兜底并在 UI 显式标注「非原图」。

## 4.1 静态→VPS 迁移路径（契约为先）

前期静态不是死胡同：迁移成本集中在数据契约与 URL 形状，二者前期固定则后期只加适配器。

| 层 | 前期固定 | 迁移时变化 |
|---|---|---|
| DTO | manifest 字段集 = `export::to_json` 字段集 | API 返回同形状，前端零改动 |
| URL | `/m/<shard>.json`、`/t256/<id>.jpg`、`/t512/<id>.jpg`、`/o/<id>.jpg` | axum 同路径 serve（原图可 302 回对象存储），分享链接不破 |
| 前端数据层 | source 接口：`StaticSource`（fetch 分片） | 换 `ApiSource`；墙/lightbox/详情/交互不动 |
| 搜索 | 浏览器侧只做常用筛选子集（标签 chips/收藏/排序/名称） | 完整语法切服务端 `SearchQuery::parse`，不复刻全量 JS 轮子 |
| 云端新增 | 无 | core+axum 薄层、DB/缩略图同步桥（Litestream 或持续发布）、鉴权、Linux 适配（keyring 注入/trash 绕过） |

纪律：静态期不写将来要扔的完整语法 JS 实现；不随意定 DTO/URL（否则后期才是真重写）。

## 5. 验收与风险

- 验收：墙首屏（10k 资产分片）LCP/滚动帧率抽样；单击/双击/移动端兜底三链路；rclone dry-run 清单与实推 checksum 一致；发布增量不重传未变缩略图；桌面写库与发布器并行读无锁冲突（WAL 实测）。
- 风险：① 100k manifest 首包体积 → 分片+索引（W2 小基准定片大小）；② 原图上云带宽/存储 → W3 已定原图随发布上云（增量 checksum），存储受限回退最高档并显式标注；③ 远端来源资产无原图 → v1 排除发布范围；④ 静态站无服务端搜索 → 查询语法在浏览器侧对分片索引执行，长尾查询按需拉片；⑤ 并发代理在 main 推进本地条目 → 本分支只动文档，合并前复核（见 §6 与红线）。

## 6. 本地在做条目精简（2026-11-08 用户拍板按表冻结，状态已回写）

原则：桌面收敛为「管理工具」，展示面 polish 让位给 web 墙；已实现的保留，未验收的展示面尾巴冻结。registry/overview 已按下列结论回写；冻结项不再投入验收，web 墙承接后如需再解冻。

| ID | 原剩余 | 结论（已回写） | 理由 |
|---|---|---|---|
| F-073 | tooltip/loading 视觉、渲染后 WCAG、帧率 p99、100k 滚动内存 | 实现中（已落地部分收尾；剩余展示面验收冻结） | 展示体验主战场转 web 墙；管理工具保留现有可用性 |
| F-069 | 0/1 快捷键 | 实现中（剩余冻结） | 查看器缩放交互 web 端承接 |
| F-074 | 宽色域样本/连续切图内存 | 实现中（剩余冻结） | 同上 |
| F-070 | 1440/1920 与 F-020 GUI 提交链路 | 实现中（宽屏基线冻结；只保留 F-020 提交链路） | 侧栏是管理面，宽屏截图基线收益低 |
| F-071 | 标题栏拖拽区验收 | 实现中（剩余冻结） | 外壳 polish |
| F-072 | 待补 UI 验收 | 保留一次轻量验收后归档（不冻） | 搜索是管理核心入口 |
| F-030/F-033/F-036 | 等真实环境/手动 | 维持现状挂起，不新增投入 | 与 web 无直接关系 |
| F-075/F-076 | 待移除 | 保留移除任务 | 产品已决定下线 |
| F-016/F-026/F-048/F-066 | 远期 | 维持远期不启动 | 候选池不变 |

精简后本地车道只保留：管理核心写链路验收（F-020/F-072）、F-075/F-076 移除、发布器 F-080；其余展示面尾巴冻结等 web 墙承接。

## 7. 决策点（W1–W5 全部已定，2026-11-08）

| # | 问题 | 推荐 | 备选 |
|---|---|---|---|
| W1 | 云端形态 | **已定（2026-11-08 用户方向）：前期静态导出+CDN，后期按需迁 VPS 动态后端**；迁移成本见 §4.1 | 若新鲜度/私密性需求提前出现，直接跳动态后端 |
| W2 | 墙布局 | **已定（2026-11-08）：混排拼贴**（infinitegrid Packing/Justified + 尺寸档） | 纯 masonry；WebGL 无限缩放（pixi-viewport，十万级再上） |
| W3 | 发布范围与清晰度 | **已定（2026-11-08）：无失真管线**——仅本地来源；新增 t1024/t2048 锐利档（高质量 JPEG、正确 sRGB、按档 srcset）；详情默认原图（原图随发布上云，增量 checksum），存储受限需回退时最高档兜底并显式标注 | 仅两档缩略图（不满足清晰度要求，否） |
| W4 | 访问控制 | **已定（2026-11-08）：A 公开只读**——不锁门、零鉴权代码；id 用编码串不泄漏本地路径；可选 noindex 挡搜索引擎（默认不挡） | B Cloudflare Access / E Tailscale（用户未选）；C Basic Auth、D 签名 URL 否决 |
| W5 | §6 精简提案 | **已定（2026-11-08）：按表冻结**，状态已回写 registry/overview | 维持现状继续验收（否） |

## 8. 红线

- （2026-11-08 更新）用户指示本分支直接开发：F-080 发布器与 F-081 墙前端在本分支落地，不再另开分支；新增代码只落在 `crates/kanalib-publish/` 与 `web/` 两个新目录，schema/sidecar 单一负责人的约束不变。
- 不引入 schema 迁移；发布状态走 settings/导出清单。
- 不引入 AGPL 依赖进推荐组合；借鉴遵守许可与署名。
- 不写工时/排期；规模用分片数/资产量/牵连面表达。
- 未验收不写成通过；main 合并需用户正文确认。

## 9. 实施状态（2026-11-08 起，末次更新 2026-11-09 18:35）

> 契约：[photo-wall-contract.md](photo-wall-contract.md)（v1 冻结）。代码只落在两个新目录 + 脚本，不改 DB schema、不动 `crates/kanalib-ui`。
>
> v3 的完整状态、复验数据与待办见 [photo-wall-status.md](photo-wall-status.md)。

| 层 | 落点 | 状态 | 备注 |
|---|---|---|---|
| 契约 | `docs/backlog/photo-wall-contract.md` | 已冻结 v1 | item 字段 id/w/h/n/rt/st/fav/tags/ph/mtime/orig；目录 index.json + m/*.json + t256/t512/o |
| F-080 发布器 | `crates/kanalib-publish/`（新 workspace 成员，仅依赖 core） | 实现中 | 只读导出：manifest 分片 + 缩略图 + 可选原图 + publish-ledger 增量；`--dry-run`。2026-11-09 修「重复副本丢标签」（同内容多份时用有标签的那份补位，见 status §5.1）。**2026-11-10**：新增 t1024/t2048 锐利档（EXIF 方向、ICC→sRGB、Lanczos3、q90、不放大、账本增量、`--no-tiers` 可关），每条 item 写 `sharp` 声明最高可用档，rclone 仍未接入 |
| F-081 墙前端 | `web/`（Vite + React + TS，包名 kanalib-wall） | 实现中（v3 已落地并复验并提交） | v1：infinitegrid + PhotoSwipe + 详情路由 + 整墙缩放；v2：Folia 风格左侧边栏 + 密集拼贴；**v3（2026-11-09）**：二维画布相机（左键拖拽/滚轮/Ctrl 缩放/双指缩放）、120px 竖排"缝"（常驻半透明）、动态强调色（OKLCH 取样）、标签两层、全屏详情、入场动效；**同日二期**：L0 改 360° 全向无限（格子按需生成、相机不夹取、rebase、回到起点）；**2026-11-10**：网格 srcset（基础两档 + 按 `sharp` 追加 t1024/t2048）、详情按 `sharp` 选档且旧数据逐档试探；**同日晚**按拍板口径补齐收藏/分级/分组两级入口（`#/favs→#/fav`、`#/ratings→#/r/…`、`#/groups→#/g/…`）、详情触摸横滑/缩放平移/标签分组折叠、强调色分层、无限首页排序与整库搜索；清掉 8 份重复 CSS。标准见 [photo-wall-v3.md](photo-wall-v3.md) |
| 验收脚手架 | `scripts/wall_manifest_check.py`、`scripts/wall_verify.ps1`、`scripts/wall_shots.mjs` | 前两个已提交；`wall_shots.mjs` 待提交 | 契约不变量校验 + 单页 headless + **四页/概要/五条交互/390 窄屏/双指缩放一次跑完**（零依赖 CDP） |
| 视觉改版 v2 | [photo-wall-design.md](photo-wall-design.md) | **已实施（2026-11-09）** | 珊瑚红 `#fd5c47` + 光晕/噪点、左侧 76/232px 边栏、6px 密集拼贴、展示型详情页；顶部工具条已删除。**已被 v3 取代**（强调色改动态取样、边栏改 120px 竖排缝） |
| 视觉改版 v3 | [photo-wall-v3.md](photo-wall-v3.md) | **已实施（2026-11-09）** | 空间画布 + 黑白竖排缝 + 动态强调色 + 标签两层 + 全屏详情；硬规则 1–10 全部通过，第 11 条（L0 360° 全向无界）待做（status §4.3） |

- 开发数据：发布器可导出到 `web/public/data/`（已 gitignore），`npm run dev` 直接以 `/data/` 读取；构建产物 `web/dist` 与 `data/` 一起上传。
- v2 自测（2026-11-09，headless Edge + CDP，1440×900 与 390×844）：9899 条全量数据下墙 17 列、贴片跨度随宽高比、滚动到墙尾继续追加；边栏展开 232px 后墙重排为 12 列点位；标签筛选生效（筛后计数与边栏计数一致）；单击 PhotoSwipe、双击进详情、Esc 返回。截图见 `web/screenshots/v2-*.png`。
- v3 自测（2026-11-09 18:20，`scripts/wall_shots.mjs`）：首页 25–38 张可见卡、动态强调色生效；详情大图铺满视口；`#/tags` 135 卡、第四段标题「分级」；右键概要 320×120 且在视口内；拖拽平移 / Ctrl+滚轮 1→1.528 / 键盘 Tab→Enter 开灯箱 / 双指 1→1.6 / hover 滤镜与信息行；390 下横拖仍可继续；控制台 0 错误。截图 `web/screenshots/v3f-*.png`。
- v3 二期自测（2026-11-09 19:00，规则 11）：横拖 42,899px、竖拖 9,799px 后画面仍满（覆盖率 87% / 86%），DOM 恒定 25–38 张，`Home` 回起点；标签墙/筛选仍是有限世界（3 张图不会无限重复）。
- 数据重导（2026-11-09）：facets 标签 54 → 135，带标签条目 2 → 5，`#/t/1girl` 由 1 张变 3 张；`wall_manifest_check.py` 退出 0（8 条警告）。
- 锐利档自测（2026-11-10）：隔离根 6 张真实图（3840×2160 到 280×175）跑完 headless 扫描 + 发布，sharp 依次 2048/2048/1024/0/0/0；浏览器实测网格在 DPR2.5 下按显示宽选到 t2048/t1024/t512，详情 srcLog 分别为 `['t2048']`/`['t1024']`/`['t512']`（各只请求一次，无 404 试探）；删掉 sharp 与 t2048 后旧数据仍能 `['t2048','t1024']` 回退；`cargo test -p kanalib-publish` 14 条契约测试 + 11 条单测全过。当前 9899 条开发数据 `sharp: 0`（源图库已删除），真实库接入后需重跑发布再评价。
- 网页复查与补齐（2026-11-10）：按 v3 Q1–Q23/硬规则逐条读代码 + headless 浏览器实测，补 Q8 四入口、详情触摸换图/缩放平移、标签分组折叠、强调色分层、排序与整库搜索；详情放大 1→2.8（双指）与 1→1.616（Ctrl+滚轮）、双击复位；390 触摸左滑换图；去掉与 W4 相反的 noindex；styles.css 3009→1288 行。实测与截图见 status §4.9、`.kanalib/audit-final/`。
- 未做（后续）：rclone 推送与 F-083 部署、动态后端迁移（§4.1）；锐利档并行解码（大库首次发布性能，按需）；桌面端「发布/同步到云」入口（用户 2026-11-10 指示先不做发布同步）。
