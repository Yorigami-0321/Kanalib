# 图片墙现状与路线图

> 分支 `feature/cloud-photo-wall` · 计划 [cloud-photo-wall.md](cloud-photo-wall.md) · 契约 [photo-wall-contract.md](photo-wall-contract.md) · 视觉规格 [photo-wall-v3.md](photo-wall-v3.md)
>
> **口径：代码是事实来源。** 本文只写当前可验证的功能、基线、已知问题与下一批任务；历史过程与已关闭议题在 [archive/2026-11/](../archive/2026-11/)（非权威）。产品上线不属于当前目标，先把手感和完整度做扎实。

## 1. 功能面（2026-11-11 代码核对）

### 1.1 路由

| 地址 | 页面 |
|---|---|
| `#/` | 无限随机墙（默认） |
| `#/tags` · `#/t/<tag>` | 标签总览 · 单标签墙（图片数 ≥8 且随机、无搜索时无界，否则有限） |
| `#/favs` · `#/fav` | 收藏总览 · 收藏墙 |
| `#/ratings` · `#/r/<G\|S\|Q\|E>` | 分级总览 · 分级墙 |
| `#/groups` · `#/g/<group>` | 分组总览 · 分组墙 |
| `#/d/<id>` | 详情（`#/a/<id>` 为兼容别名） |
| 其他非空路径 | 缺失页：说明 + 「回首页」「去搜索」，不再静默回落首页 |

> 世界形态：首页随机墙恒为无界；单标签墙在「随机排序 + 无搜索 + 图片数 ≥ 8」时无界（`INFINITE_MIN_TAG_ITEMS = 8`，计数优先取 `index.facets.tags`），其余组合——显式排序、搜索、收藏/分级/分组墙——均为有限有序墙。

### 1.2 交互

| 面 | 已实现 |
|---|---|
| 画布 | 左键拖拽、滚轮/Ctrl+滚轮/双指缩放、方向键/PageUp/PageDown/Home/缩放键、360° 无限格位、有限墙边界回弹、缩放比例瞬态提示；画布全幅铺满（图垫在左侧 76% 半透明缝下，玻璃可见），缝区在画布 pointer/wheel 层硬拦截（不拖动、不缩放、不选中）；标签墙点顶部卡片时四周自动补图 |
| 卡片 | 单击原位聚焦（邻图挤压、不居中）、双击/Enter 进详情、长按概要、hover 微缩放、渐进模糊加载、入场波次 |
| 详情 | 键盘/侧栏翻页、触摸横滑、Ctrl/双指缩放与放大后平移、双击复位、自动放映（P）、单色（N）、留白视图（M）、复制深链、全屏；停留 800ms 后预取前后 ±2/±3 的 t512（弱网跳过，同 URL 只取一次） |
| 信息 | 侧栏 300px 检查器：元数据、按类别折叠标签、相似条、原图；窄屏为底部 sheet |
| 列表 | 文件名 + 全库搜索、排序（随机/文件名/时间等）、标签/分级/分组筛选、四个入口两级结构；各类空态带可执行动作（清空搜索 / 回对应总览 / 回首页） |

### 1.3 数据与发布管线

| 项 | 现状 |
|---|---|
| 默认数据 | `web/public/data`：**937 条**，1 个分片 `m/0000.json`（1.385 MB，约 1.48 KB/条），2,000 标签 / 73 分组 |
| 图片档 | t256/t512 937、t1024 785（83.8%）、t2048 346（36.9%），未发布原图 |
| 发布器 | `crates/kanalib-publish`：index + 分片 + 四档 + 可选原图 + ledger 增量 + `--dry-run` + `--no-tiers` |
| 导出修正 | 源图纯白底带裁剪、透明图 alpha 合成白底、GIF 明确取首帧；账本记录渲染版本，渲染逻辑变更会触发对应条目重编 |
| 增量 | ledger 指纹 = 源文件指纹 + 渲染版本；首次升级账本会全量重建一次 |
| 契约校验 | `python scripts/wall_manifest_check.py web/public/data` |

## 2. 验证基线

```bash
cd web && npx tsc --noEmit            # 类型检查
cd web && npm run build               # 生产构建（含 public/data 拷贝）
cd web && npx vite preview --port 4273
node scripts/wall_shots.mjs --base http://127.0.0.1:4273/ --out web/screenshots/final
node scripts/wall_visual_probe.mjs http://127.0.0.1:4273/
node scripts/wall_perf.mjs --base http://127.0.0.1:4273/ --seconds 30
node scripts/wall_probe_wheel.mjs http://127.0.0.1:4273/
node scripts/wall_probe_recover.mjs --base http://127.0.0.1:4273/   # 断片注入 → 重试 → 恢复
node scripts/wall_probe_state.mjs --base http://127.0.0.1:4273/     # 无效深链 / 搜索 / URL / 小集合
node scripts/wall_probe_cold_search.mjs --base http://127.0.0.1:4273/ # 冷开带 q / 刷新 / 新标签 / 索引 404
node scripts/wall_probe_detail_nav.mjs --base http://127.0.0.1:4273/  # 详情 ←/→/侧栏/放映/相似与路由同步
node scripts/wall_probe_tag360.mjs --base http://127.0.0.1:4273/     # 标签墙 360° 补图 / 缝不穿透 / 移动端
node scripts/wall_probe_tag_threshold.mjs --base http://127.0.0.1:4273/  # 1/7/8/9/大标签的世界形态边界
node scripts/wall_probe_edge.mjs --base http://127.0.0.1:4273/      # 未知 hash 缺失页 / 空态动作 / 历史
node scripts/wall_probe_mask_edge.mjs --base http://127.0.0.1:4273/ --out web/screenshots/mask-edge  # 概要遮罩边缘像素 + 反馈环形态
node scripts/wall_probe_detail_preload.mjs --serve-dist web/dist    # 详情预取窗口与网络计数（自带缓存静态服务）
cargo test -p kanalib-publish         # 25 单元 + 23 集成契约测试
```

已知测量边界：headless 走 SwiftShader，帧率与 GPU 合成成本不代表真机；手势探针用合成事件，**真实触控板/触屏手感仍需实机确认**。

### 2.1 输入手感（用户问“有没有改进”）

已改进，且有量化证据：

| 指标 | 改前 | 改后 |
|---|---|---|
| 触控板连续平移跟随误差 | 72.7 px | **11.8 px** |
| 快速滑动跟随误差 | 159.9 px | **23.7 px** |
| 松手收敛时间 | 298 ms | **11 ms** |
| 滚轮回滚帧数 | 12–36 帧 | **0** |
| 活跃期失速 | 0–12 ms | **0–12 ms** |

做法：鼠标离散滚轮仍走 80 ms 缓动 glide；触控板/小步长增量改为逐帧 1:1 直跟（`classifyWheel` 分类 + 持久 rAF 积分泵）。回归闸门已写进 `wall_shots.mjs`（跟随误差 ≤24px、收敛 ≤80ms、鼠标 glide ≥60ms）。**合成事件只能证明轨迹，最终以真实设备手感为准。**

## 3. 当前数据规模下的表现

| 场景 | 结果 |
|---|---|
| 无限墙 30 s 连续拖拽 | DOM 45–81 张、掉帧约 0%、长任务 0、主线程约 53% |
| 远走 | 横 4.3 万 px / 竖 1 万 px 仍满屏，DOM 恒定 |
| 侧栏对比度 | 次级文字最差 5.68:1、主文字 12.16:1（达标）；`seam__count` 计数 0.62 后 7.14:1 |
| 实库探针 | 首屏 72 卡、pending 0、详情 t1024、聚焦细条 0、Tab 6/6 可见、console 0 错误 |
| 标签墙 360°（1440×900） | 画布 left=0 / w=1440 并盖住 120px 缝；DOM 46–67 张；点最顶卡片后四周无空缺、上方仍可拖 21–23 行；缝内拖拽/真实+合成滚轮均不动 transform、不换 hash |
| 移动端（390×844） | 画布 left=0 / w=390，横向溢出 0，缝宽 64，canvasCoversSeam=true |
| 详情预取（4g / 弱网） | 4g 打开 7 张 t512（3 邻屏 + ±2/±3 共 4 张预取，276 KB）；弱网预取 0、t2048 0，首屏体积 +0% |
| 概要遮罩边缘（1440×900） | zoom 1 / 1.2635 / 1.6 / DPR 2.5：四边最外 2px 与内缩 4–6px 亮度差 −2.2…+1.4（≤3/255），遮罩四边外扩 2.00px，内容内缩 12.00px；`wall_probe_mask_edge` passed |

## 4. 已知问题（代码审计，按影响排序）

| # | 问题 | 代码位置 | 影响 | 计划 |
|---|---|---|---|---|
| 1 | ~~搜索只匹配文件名~~ **已完成**：`search-index.json` 全库检索（名称/标签/分组/分级；空格 AND、`-词` 排除、`tag:` 限定；索引缺失时退回文件名） | `state/filters.ts`、`source/StaticSource.ts` | — | 回归：`wall_probe_state`、`wall_probe_cold_search` |
| 2 | 所有列表路由都串行拉全部分片；无限首页要等全量到齐才生成格位 | `App.tsx` `needAll` / `infinite && !ready` | 分片一多首屏即被拖住，单分片失败全网卡死 | P0：首页按需渲染 + 全量改为后台渐进；搜索索引独立 |
| 3 | ~~分片失败后没有就地重试~~ **已修复（2026-11-11）**：游标改为请求成功后才推进 + generation 作废旧请求；失败自动补拉 800/2000ms；空态与“已有卡片”错误条均可重试/续拉，不清空已加载 | `StaticSource.ts`、`useWall.ts`、`Wall.tsx` | — | 回归：`wall_probe_recover.mjs` 连续 3 次失败 → 真实坐标点击 → 84 张恢复 |
| 4 | 标签总览一次渲染全部 2,000 张卡 | `TagGallery.tsx` | 大库首屏 DOM 与封面请求过重 | P1：分类目录 + 分组展开/窗口化 + 搜索置顶 |
| 5 | ~~筛选/排序/单色/留白只在内存~~ **已完成**：`q/sort/mono/matte` 写入 hash query，刷新/深链可恢复 | `App.tsx`、`lib/router.ts` | — | 遗留：跨入口静默延续 query 无可视 chip（见 §5 第二批 B） |
| 6 | 详情深链最多补拉 8 个分片（约 4 万条），更靠后的 id 找不到 | `useWall.ts` `MAX_PROBE_SHARDS` | 大库深链可能“图片不存在” | P1：按 id 前缀定位分片，或提供 id→分片索引 |
| 7 | ~~未知 hash 静默回落首页~~ **已完成**：未知非空路径解析为 `notfound`，渲染缺失页（说明 + 回首页 / 去搜索），不进全量拉取；有效路由与历史前进后退正常 | `lib/router.ts`、`App.tsx` | — | 回归：`wall_probe_edge` |
| 8 | ~~标签/分组墙为空时动作不足~~ **已完成**：五类空态各带动作（清空搜索 / 回标签·分级·分组总览 / 回首页） | `App.tsx`、`Wall.tsx`、`BucketGallery.tsx` | — | 回归：`wall_probe_edge` |
| 9 | 移动端检查器为底部 sheet，遮挡较多画面 | `inspector.css` | 390 宽下内容区被压 | P2：改底部 tab 或可折叠把手（待产品决策） |
| 10 | 发布非原子：中断可能留下半套产物 | `crates/kanalib-publish` | 部署时可能出现版本不一致 | P2：写临时目录后原子替换，或产物带版本号目录 |
| 11 | 语义搜索缺失 | — | 只能按文件名/标签找图 | 候选：CLIP（F-016），先不做 |

### 4.1 2026-11-11 已闭环（明细归档）

失败路径与兜底、第二批 A 复验、功能与旅程审计的完整证据已归档：[photo-wall-audit-2026-11-11.md](../archive/2026-11/photo-wall-audit-2026-11-11.md)。当前结论：

| 批次 | 结论 |
|---|---|
| 失败路径与兜底 | 详情全档失败错误卡与重试、打开详情焦点接管、ErrorBoundary、计数对比度、双错误条分离，全部闭环 |
| 第二批 A | 无效深链、全库搜索、URL 状态、小集合墙、弱网骨架、索引体积门限，全部闭环 |
| 功能与旅程审计 | 19 项中 12 项闭环；剩余为分片渐进、标签目录、移动端布局、排序面板不收起、分级双真源、长图/GIF、内容闸门 |

**残余风险**：部分分片先到会先渲染一轮、后续分片到达再整体换一批选图（无限首页）。当前接受；若实机观感闪烁，再改成“只追加不重排”。

### 4.2 画布与边界态收口（2026-11-11 夜）

| 项 | 实现 | 复验 |
|---|---|---|
| 画布全幅 + 缝区拦截（回退让位方案） | `.canvas` 恢复 `inset:0`，图垫到缝下让 76% + `blur(10px)` 玻璃重新可见；`Wall.tsx` 在 `pointerdown` / `wheel` 首行按 `clientX < 缝宽` 提前 return（在 `preventDefault`、`setPointerCapture` 之前），缝区不拖拽/不缩放/不选中 | `wall_probe_tag360`：canvas `left 0 / w 1440`、`canvasCoversSeam:true`；缝内拖 220px 与真实/合成滚轮均不动 transform/hash/spotlight，右侧滚轮平移、Ctrl+滚轮缩放正常；`Sidebar.tsx` / `sidebar.css` git diff 为空 |
| 标签墙 360° 无界 | 单标签「随机 + 无搜索」进入无界格子世界，点顶卡四周自动补图；坐标换算以画布左上角为原点（画布全幅，含缝下） | `wall_probe_tag360`：桌面画布 left=0/w=1440、移动端 left=0/w=390，上方可拖 21–23 行，rows/cols 无缺 |
| 小标签阈值 | `INFINITE_MIN_TAG_ITEMS = 8`；计数优先 `index.facets.tags`，缺失时等分片全量到位再兜底；世界模式并入 `resetKey` | `wall_probe_tag_threshold`：1/7 张有限、8/9/735 张无界，0 console error |
| 无界标签页标题可读 | `.wallhead::before` 纸面衬底（顶部压暗 + blur 14px + 径向遮罩淡出），不参与命中测试；无界世界没有固定边缘，故不做顶部安全区 | `wall_probe_detail_preload`：标题可见，`wallheadBlocksClick:false` |
| 详情预加载窗口 | 停留 800ms 后预取前后 ±2/±3 的 t512（`PRELOAD_RADIUS=3`，URL 去重、上限 120、弱网跳过）；邻屏固定 t512、居中才升档不变 | `wall_probe_detail_preload`：4g 打开 7 张 t512 / 276 KB，窗口外 0；弱网预取 0、首屏体积 +0% |
| 未知 hash 缺失页 | 未知非空路径 → `notfound`，渲染「回首页 / 去搜索」，不进全量拉取 | `wall_probe_edge`：两动作齐全、tiles 0、console 0 |
| 空态动作 | 无匹配 → 清空搜索；空标签/分级/分组 → 回对应总览；空收藏 → 回首页；`#/favs` 桶空态 → 回首页 | `wall_probe_edge`：五类空态点击后到达目标路由 |

**残余风险**：① 8 张标签按 8 张循环，属阈值语义的预期观感；② 未进 facets 前 2000 的小标签依赖全量分片兜底，加载极慢时会先无界占位再切有限（当前单分片无感）；③ 详情预取在快网会多约 4 张 t512（~160 KB，low priority），弱网跳过；④ 缺失页初始焦点仍在 body，无障碍收口时再处理。

### 4.3 功能与旅程审计（明细归档）

前端二号 9 轮 CDP 审计的逐条证据已归档：[photo-wall-audit-2026-11-11.md](../archive/2026-11/photo-wall-audit-2026-11-11.md) §3。仍未闭环的条目已并入 §4 已知问题表与 §5 第二批 B。

### 4.4 概要遮罩边缘亮边（2026-11-11 深夜，已闭环）

用户口径：右键/hover 时图上的遮罩没有铺满整张图，卡片边缘留一条很细的亮边。复现（1440×900、zoom 1.2635 / 1.6）：概要卡 rect 为分数坐标，`.tile__info` / `.tile__shade` 的 rect 与卡片一致，但隐藏卡内 `<img>` 后四边仍见紫色细线（右缘 x=913/914/915/916 亮度 6.8/20.3/29.1/38.9，内部约 6.8）；zoom 1.6 最底行 (55,48,46) vs 上一行 (5,5,6)。

根因：`.tile` 自身 `overflow:hidden + contain:paint` 在分数坐标下裁切边界只有部分像素覆盖，`inset:0` 遮罩填不满最外设备像素；下层 `data-current` 的 2px accent 内描边与 hover 的 1px 高光从边界透出。只把遮罩外扩不够（外扩部分仍被父级裁掉；-3px 仅消底边、右缘仍 63.9）。

修法（`styles.css`、`WallCard.tsx`）：
- `.tile` 取消裁切（移除 `overflow:hidden`，`contain: layout paint` → `layout`）；新增 `.tile__media` 承载图片 / 底部压暗 / 骨架的圆角裁切。
- 概要挂载期间（`.tile:has(.tile__info)`）`.tile__media` 外扩 1px；`.tile__info` 外扩 2px + `border-radius:inherit`，在图片抗锯齿之外收边；内边距 14px，内容距卡边仍 12px。
- 概要态压掉卡片自身 `box-shadow`，并隐藏 current / hover / spotlight 装饰环；键盘 `:focus-visible` 环保留。
- 新增 `.tile__ring`（z6 > 遮罩 z5、`pointer-events:none`、圆角继承）：hover 2px 白环、当前项 2px accent 环、聚焦卡 2px 环，均为遮罩之上的整圈。

复验：`wall_probe_mask_edge` passed（zoom 1 / 1.2635 / 1.6 / DPR 2.5 四边 Δ ≤3/255；遮罩外扩 2.00px；内容内缩 12.00px；概要态 ring=none）；`tag360` / `tag_threshold` / `edge` / `detail_nav` / `detail_preload` 全 passed；tsc/build 通过。独立抽查 zoom 0.5 概要边缘：隐藏 img 后四边亮度 5.5–6.7，无透光亮线。

**残余风险**：① 卡片外 1px 仍可见图片自身抗锯齿透过半透明遮罩，亮度低于内部（采样在卡片内，不参与判定），要压平只能把遮罩边缘做成不透明；② 概要挂载瞬间图片裁切层外扩 1px（被 160ms 淡入盖住）；③ 概要态不显示 current/hover 装饰环（键盘焦点环保留），属有意的识别性取舍；④ `.tile:has(.tile__info)` 依赖 `:has`（Chrome 105+）。

## 5. 下一批任务（不与上线绑定）

### 第一批（已完成，未提交）

| 任务 | 写域 | 状态 |
|---|---|---|
| 失败分片退避重试、详情全档失败降级、对话框焦点、ErrorBoundary | `StaticSource.ts`、`useWall.ts`、`Detail.tsx`、`main.tsx` | 完成 |
| 计数对比度、错误条重试、空态复用同一路径 | `styles.css`、`Wall.tsx`（仅错误块与渲染门槛） | 完成 |
| 白带 alpha/GIF 修复、账本渲染版本 | `crates/kanalib-publish/**`、`wall_manifest_check.py` | 完成（20 项契约测试通过） |

### 第二批 A（已开工，不依赖产品决策）

1. `search-index.json`：发布器侧仍待交付；前端已按元组契约接入并复验（索引缺失时退回文件名搜索）。
2. URL 状态持久化：已完成并复验。**遗留**：跨入口会静默延续 query（URL 可见但画布无 chip），可清除筛选条进 2B。
3. 稳定性与信任小修：无效深链、弱网骨架、小集合有限墙、0 收藏空态，均已完成并复验。
4. 详情体验（前端二号）：邻屏 t512 占位不重复拉档；相似为空回退同标签/分组；去掉不可达的“打开原图”承诺。**已完成并复核**（正常网首屏 64–196KB，Fast 3G 50–368KB；t512 ≤1.03s、锐利层 ≤2.04s；B2 抽样 10 页回退 10/10；原图入口 0）。
5. 详情翻页路由同步（前端一号）：A4 引入的 `replaceState` 无事件回归，**已完成并复核**；`wall_probe_detail_nav` 固化“→ 后 500ms 画面必须变”。
6. 冷启动带 q 深链（前端一号）：搜索索引请求与 `init()` 的 generation 竞态，**已完成并复核**；`wall_probe_cold_search` 固化冷开/刷新/新标签/404 四种场景。

### 第二批 B（部分已完成，其余待决策）

3. 首页按需分片与无限格位渐进生成；后台补全其余分片。（待决策）
4. 标签页目录化：默认分类目录、每组限量、搜索置顶。（待决策）
5. 移动端布局调整（底部 tab 或可折叠 sheet）。（待决策）
6. 卡面信息与打开方式（聚焦保留名称+主标签+“打开”按钮 / 双击窗口）。（待决策）
7. ~~空态动作与 404 提示~~ **已完成并复验**：未知非空 hash → 缺失页（回首页 / 去搜索）；无匹配 / 空标签 / 空分级 / 空分组 / 空收藏 / `#/favs` 桶空态均带可执行动作。回归：`wall_probe_edge`（1440，console 0，历史前进后退正常）。

### 第三批（体验打磨）

- ~~详情左右切换的预加载窗口扩大~~ **已完成并复核**：停留 800ms 后预取前后 ±2/±3 的 t512（`PRELOAD_RADIUS=3`，URL 去重、上限 120、弱网跳过）；4g 打开 7 张 t512 / 276 KB，窗口外 0；弱网预取 0、首屏体积 +0%。回归：`wall_probe_detail_preload`。
- ~~无界标签页标题被首行卡片压住~~ **已完成并复核**：`.wallhead::before` 纸面衬底（顶部压暗 + blur 14px + 径向遮罩淡出），不拦指针，相机与 360° 补图不受影响。回归：`wall_probe_tag360`。
- ~~概要遮罩边缘 1px 亮边~~ **已完成并复核**：遮罩外扩 2px 越出卡片裁切边界，`.tile__media` 承担圆角裁切，装饰环改为遮罩之上的 `.tile__ring`（概要态关闭/键盘焦点保留）。回归：`wall_probe_mask_edge`（zoom 1 / 1.2635 / 1.6 / DPR 2.5 四边 Δ ≤3/255）、`wall_probe_tag360`、`wall_probe_tag_threshold`、`wall_probe_edge`、`wall_probe_detail_nav`、`wall_probe_detail_preload`。明细见 §4.4。
- 检查器信息密度与标签分组交互（默认组、展开记忆）。
- 发布器线程池（大库首编耗时）与原子发布。
- 真实设备手感复验（触控板/触屏/高分屏 DPR）。

## 6. 待产品决策

| # | 问题 | 选项 |
|---|---|---|
| 1 | 桌面单击聚焦后是否保留名称+主标签并加“打开”按钮 | A 保留并加按钮；B 维持纯图 |
| 2 | 移动端导航形态 | A 底部 tab（内容区 326→≥346px）；B 继续 64px 侧缝 |
| 3 | 标签页默认形态 | A 分类目录 + 每组 60 + 搜索置顶；B 维持全量平铺 |
| 4 | 内容分级闸门（2026-11-11 新增） | A 默认只显示 G，可手动切换；B 记住上次选择；C 进入前确认。全库 S 504/937，当前首屏可能含受限内容 |

## 7. 历史记录

| 内容 | 位置 |
|---|---|
| 2026-11-11 功能与旅程审计、失败路径、第二批 A 明细 | [archive/2026-11/photo-wall-audit-2026-11-11.md](../archive/2026-11/photo-wall-audit-2026-11-11.md) |
| 2026-11-10 21:00 前的完整开发日志（v3/v4/v5 逐轮实测） | [archive/2026-11/photo-wall-status-2026-11-10.md](../archive/2026-11/photo-wall-status-2026-11-10.md) |
| 两轮实机反馈根因与修复明细 | [archive/2026-11/photo-wall-feedback-2026-11-10.md](../archive/2026-11/photo-wall-feedback-2026-11-10.md) |
| v4 聚焦与手势调研推演 | [archive/2026-11/photo-wall-focus-mode.md](../archive/2026-11/photo-wall-focus-mode.md) |
| v2 视觉设计（已被 v3 取代） | [archive/2026-11/photo-wall-design.md](../archive/2026-11/photo-wall-design.md) |
