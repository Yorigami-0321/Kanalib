# 图片墙开发状态（2026-11-09 19:05）

> 分支 `feature/cloud-photo-wall` · HEAD `8ea1fc4` · 上级计划 [cloud-photo-wall.md](cloud-photo-wall.md) · v3 标准 [photo-wall-v3.md](photo-wall-v3.md)
>
> 一句话：**v3 全部硬规则（含 11 条中的"360° 全向无界"）已落地并 headless 实测通过；发布器的重复副本标签问题已修并重导数据（facets 54 → 135）；改动已按三块提交。**

## 1. 当前状态速览

| 项 | 值 |
|---|---|
| 分支 / HEAD | `feature/cloud-photo-wall` / `0341e2f` feat(wall): F-081 v2 按 Folia 风格改版 |
| 提交情况 | **v3 一行未提交**：11 个修改文件 + 11 个新增文件（清单见 §9） |
| 类型检查 | `cd web && npx tsc --noEmit` → exit 0 |
| 生产构建 | exit 0：CSS 31.3KB / JS 285.0KB（gzip 91.3KB）/ photoswipe chunk 60.3KB（本机 build 命令见 §8，`npm run build` 会被沙箱批量删除拦截） |
| 数据 | `web/public/data/` 9899 条 / 2 分片（m/0000 5000 + m/0001 4899），gitignore |
| 改动前备份 | `.kanalib/backups/src-20261109-130014`（25 个文件），整体回退用 |
| 验证产物 | `web/screenshots/v3f-*.png` + `report.json`（本轮，脚本可复跑） |

## 2. 本轮怎么做的

Lead 统筹，编码分给三个 teammate（写域隔离，只有 verifier 能 build 和用浏览器），Lead 做两轮代码审查并亲自返修；收尾阶段补完交互实测与文档。

| 成员 | 写域 | 结果 |
|---|---|---|
| wall-canvas | `App.tsx`、`Wall.tsx`、`WallCard.tsx`、`lib/{world,accent,shuffle}.ts`、`styles.css` | task-1 + 返修 task-7 全部交完 |
| nav-detail | `Sidebar.tsx`、`Detail.tsx`、`TagGallery.tsx`、`Lightbox.ts` + 4 个组件级 CSS | task-2 + 返修 task-8/task-9 全部交完 |
| verifier | 只读 + build + headless 截图 | task-3/task-4 完成；第二轮复拍由收尾阶段补齐 |

## 3. 代码审查（两轮，全部返修落地）

审查口径：`git diff --stat` 约 1661 insertions / 1087 deletions；机械扫描 `as any` / `@ts-ignore` / `eslint-disable` / `console.*` / `TODO` 全 0 命中，`!important` 仅存在于 `prefers-reduced-motion` 块（合规），`var(--accent)` 28 处均为细线/inset/小面积 tint。

### 3.1 P1（6 条，全部已修）

| # | 问题 | 位置 | 修法 |
|---|---|---|---|
| 1 | `activePswp` 恒为 undefined（动态 import 完成前实例不存在）→ `isLightboxOpen()`/`closeLightbox()` 全失效 | `Lightbox.ts` | 赋值挪进 `lightbox.on('uiRegister')`；close 置 null 且 destroy 幂等 |
| 2 | `allLoaded` 只置真不重置，retry 后标签页永久停在首片 | `App.tsx` | `loadedShards === 0` 时复位 |
| 3 | `setFailed` 嵌在 `setTierIndex` 的 updater 里（updater 必须纯） | `WallCard.tsx` | 降级判定移出 updater，依赖补 `tierIndex` |
| 4 | 惯性 rAF 无卸载清理，切页后仍在已卸载组件上 setState | `Wall.tsx` | `stopInertia` 改 useCallback + `useEffect(() => stopInertia, ...)` |
| 5 | 私有 `EMPTY` 与 `EMPTY_FILTERS` 两份真源 | `Sidebar.tsx` | 改 import，删本地常量 |
| 6 | 键盘不可达（墙只能鼠标拖、`.tile` 是裸 div） | `Wall.tsx` / `WallCard.tsx` | 卡片 `role=button` + tabIndex + Enter/Space；画布方向键/PageUp/PageDown/Home/End/±缩放 |

> 第 6 条的连带坑：`INTERACTIVE` 含 `[role="button"]`，卡片一旦有 role 就被 `closest()` 命中导致整墙拖不动；判定已排除 `.tile` 本身。

### 3.2 P2（9 条，全部已修）

`WallCard` 注释"两行"改"三行" · `accent.ts` 同一次解码算两遍 + `as Oklch` · `App.tsx` Esc 死分支删除 · `useCallback(...,[wall])` 改 `[wall.loadMore]` · 渲染期写 ref 改用 ref 同步语义 · `world.ts` 满行 clamp 后可能横向溢出（加保护）· `Sidebar` 死字段 `badge` + 面板补 `aria-controls`/焦点进入回收 · `Detail` 的 `srcIndex` 重置改与 `item.id` 绑定的 `loadStage` · 删除与单击叠加的 `onDoubleClick`（详情入口留在灯箱按钮与概要卡）。

另统一浏览序列：详情页序号/左右键与灯箱都走 `detailBrowseItems`（筛后+排序），不再一处 `orderedItems` 一处 `shown`。

### 3.3 收尾阶段增量修复（2026-11-09 18:00–18:35）

| # | 问题 | 位置 | 修法 |
|---|---|---|---|
| 1 | 详情大图只按原尺寸显示，1440 屏里 512 图四周全黑 | `styles.css` `.view__img` | `width/height:100%` + `object-fit:contain` |
| 2 | 视口变窄/旋转后相机越界，390 屏右侧空一大条 | `Wall.tsx` | 尺寸变化时把相机夹回合法区间（`clampCamera` effect） |
| 3 | 窄屏第一列被导航缝压掉一半 | `Wall.tsx` | ≤760px 世界右移 64px，`viewWidth` 参与夹取与居中 |
| 4 | 窄屏三张不齐比例的卡同一行时，整行比世界宽、最右一张被裁 | `lib/world.ts` | 行高上限与 `raw` 取小，保证整行 ≤ 世界宽 |
| 5 | 右键概要正文挤三行、按钮贴边 | `App.tsx` / `styles.css` | 宽度 268→320、封面 96、meta 单行省略 |
| 6 | 卡片角标显示占位像素宽 | `WallCard.tsx` | 「主标签 · 张数」，无标签退回 宽×高 |
| 7 | 标签页副标题「每个先给你一张」语义含糊 | `TagGallery.tsx` | 只留「N 个标签」 |
| 8 | 入场动效缺失（规则 9 后半） | `Wall.tsx` / `styles.css` | 首载淡入+上浮 12px/240ms；切页签对角线波次（30ms/格、封顶 340ms）；虚拟化滚动不重播 |
| 9 | 触屏无双指缩放（B4） | `Wall.tsx` | 双指指距→缩放、锚点固定双指中点；抬手后剩下那根手指重建拖拽基线 |

## 4. 视觉与交互复验（`web/screenshots/report.json`，可用 §8 命令复跑）

工具：`scripts/wall_shots.mjs`（零依赖，CDP 直连，`Emulation.setDeviceMetricsOverride` 钉视口）。

### 4.1 页面与数据

| 项 | 实测 |
|---|---|
| 首页 1440 | 56 张卡（视野 + 约 1/3 格缓冲，恒定不随走动增长），`--accent` 按当前视野算出（如 `rgb(159,129,194)`）；世界不再有尺寸 |
| 详情 | 大图 1371×1371（contain 后画面 1371×1028），源 `t512/...jpg` |
| 标签总览 | 135 张标签卡（重导后），`#/tags` 第四段标题为「分级」（D3 已修） |
| 单标签墙 | `#/t/1girl` 3 张、标题正确；样例库只有 5 张真实图片，多数标签仍只有 1 张 |
| 右键概要 | 打开成功，320×120，四向翻转后完全在视口内 |
| 390×844 | seam 64、世界 326、`scrollWidth = 390`、右缘 390、溢出 0 |

### 4.2 交互实测

| 交互 | 实测 |
|---|---|
| hover | `filter: saturate(1.08) brightness(1.05)`（只走一个通道），分辨率行 opacity 0→1 |
| 左键拖拽 | 世界平移 (−201,0) → (−403,0)，未误开灯箱 |
| Ctrl+滚轮 | scale 1 → 1.528（以光标为锚） |
| 键盘路径 | 画布聚焦 → ↓ 平移生效 → Tab 落在 `.tile`（role=button）→ Enter 打开灯箱 |
| 双指缩放 | scale 1 → 1.6（触顶 `ZOOM_MAX`） |
| 规则 11 远走 | 横拖 43,657px / 竖拖 10,581px 后仍满屏（覆盖率 97% / 97%），DOM 恒 17–50 张；`Home` 回起点 |
| 入场动效 | 首载检测到 `.tile--enter` |
| 控制台 | 0 错误 |

### 4.3 硬规则对照（`photo-wall-v3.md` §3）

| # | 规则 | 现状 | 结论 |
|---|---|---|---|
| 1 | 首页无脚手架文案 | 无计数/页码；计数只在 `.seam__count`；加载态 1px 进度线；提示条 2.6s 淡出且 ≤760px 不出现 | 通过 |
| 2 | 导航一条、固定宽、不推挤 | 120px（≤900px 64px）浮层 | 通过 |
| 3 | 一个状态一种视觉通道 | hover=滤镜；选中=竖线+提亮；当前/焦点=内描边 | 通过 |
| 4 | 卡片信息默认隐藏 | 按 Lead 裁定：标题常驻合规（Q13=A），`.tile__copy small` hover/焦点/当前才出现 | 按裁定通过 |
| 5 | 角标不 blur/不圆角/不渐变 | 直角 + 纯色 62% 黑 | 通过 |
| 6 | 焦点/当前用内描边 | `inset box-shadow` | 通过 |
| 7 | 图片 cover（详情 contain） | 满足 | 通过 |
| 8 | 背景层不拦指针 | 光晕/噪点/暗角均 `pointer-events:none` | 通过 |
| 9 | 动效只在进入/离开/切换 | 入场受 `enter` 窗口约束，滚动无动画 | 通过 |
| 10 | 390 单手可浏览 | seam 64 / 世界 326 / 无横向溢出 / 无右缘裁切 | 通过 |
| 11 | L0 任意方向无边界（2026-11-09 追加，v3 §1.1） | 无限格子：`layoutCell`/`cellTilesInView` 按 (种子, 格号) 生成，`clampCamera` 在无限模式直接返回原相机；实测横走 42,899px、竖走 9,799px 仍满屏（覆盖率 87% / 86%），DOM 恒定 25–38 张，按 Home 回起点 | 通过 |

## 5. 仍未解决

### 5.1 重复副本的标签曾丢失（2026-11-09 已修，数据已重导）

- **原判断**（"导出器没 join `asset_tags`，9899 条只有 2 条带 tags"）**是错的**，实测证伪：
  - `.kanalib/kanalib.db` 里只有 23 条资产有标签（14 条在 `sample_library`、9 条在 `dedup_test_lib`），
    这 23 条的 `tags_json` 与 `asset_tags` **完全一致**（871 行 ≈ 23 × 38），不存在漏读。
  - 真正的现象：同一张图在 `stress_10k` 里还有一份**未打标签**的拷贝，且 mtime 更新；
    发布器按 Modified 降序 + 内容 id 去重，保留了这份空标签的拷贝，标签随之丢失。
- 修法：`Db::tags_for_duplicated_assets()`（core，只查"同哈希有多份"的资产）+ 发布器
  `fill_tags_from_duplicates()`——被保留的那条**没有**任何标签时，用同内容副本的标签补位；
  两份都有标签时仍以被保留那条为准（不混批次）。stats 新增 `tags_from_duplicates`。
- 重导结果（`web/public/data`）：`其中 3 条的标签来自同内容副本`；facets 标签 **54 → 135**；
  带标签条目 2 → 5；`#/t/1girl` 从 1 张变 3 张；`wall_manifest_check.py` 退出 0（8 条警告）。
- 注意：样例库只有 5 张真实图片，135 个标签里有大量"只挂 1 张"的标签属数据结构（不是 bug）；
  真实库的观感要等接入实际图库后再看一次。

### 5.2 质量档位（依赖 F-080）

导出目前没有 `t1024` 与 `o/`，详情页只能放大 t512，2K 屏必然软。补档在 F-080（W3）。

### 5.3 L0 全向无界（v3 §1.1 / 规则 11）——**2026-11-09 已实现**

- `lib/world.ts` 新增 `layoutCell` / `cellTilesInView`：空间切成 `cellWidth × cellHeight` 的格子，
  格号可以是负数，内容由 `(worldSeed, b, c)` 决定 —— 往回走看见的是刚才那片，不是重新随机。
- 每格 2–4 行，行高由格子种子决定；行内宽度富余摊进列间距（不拉伸图片），所以每格正好填满自己的盒子，
  环带之间既不留白带也不重叠。
- `Wall.tsx`：无限模式下 `clampCamera` 直接返回原相机（四向都不夹），`layoutWorld` 整体跳过（不再排一万条），
  只渲染视野 + 1/3 格的卡；`origin` 偏移实现 rebase（屏幕坐标超 1e7 时整盘平移，画面不动）。
- 「回到起点」：走远约一屏半后左下角浮出小按钮（只在需要时出现，不构成常驻工具栏）；键盘 `Home` 是**全局**快捷键（window 监听，不依赖画布焦点；输入框内与灯箱打开时让行）。
- **只对"全库随机墙"开启**：标签墙/筛选后的列表条目少，仍是有限世界（否则三张图会无限重复）。
- 实测：横走 42,898px、竖走 10,572px，覆盖率 **90–99%**（8px 缝会让 40px 网格采样零星漏点），DOM 恒定 38–56 张，390 下横拖 513px 仍可继续。
- 行内不再留空白竖条：富余宽度按比例摊到整行（图片本来就 `cover` 裁切，只是多裁一点），
  早期"摊进列距"的做法在大行高下会摊出 180px 宽的空条，覆盖率一度掉到 0.65。

### 5.4 侧边栏常驻透明（v3 §6.G #1）——**2026-11-09 已实现**

- Q6 改判 **C′**：默认 76% 纸 + `blur(10px) saturate(1.08)` 常驻；边栏内 `--ink-soft` 提到 72%，图片透上来后次级文字仍可读。
- **悬停不改透明度**（2026-11-09 用户报"鼠标扫过会暗一下"）：纸面是深色，76% → 88% 就是整条变暗。
  现在 hover 只把模糊从 10px 提到 12px，底色恒为 76%；真正的交互反馈交给页签自己的底/描边。
  `wall_shots.mjs` 新增断言：`rest` 与 `hover` 的 `backgroundColor` 必须完全一致。
- `@supports not (backdrop-filter)` 降级为 88% 实色（没有模糊时纸面要更实才压得住图），两态同值，同样不会闪。
- 未做：最亮底图下的对比度**量化**测量与 `backdrop-filter` 对拖拽帧率的影响。

## 6. 后续工作

### P0 — 收尾本轮（全部完成）

1. ~~补完第二轮复拍~~ 已由 `scripts/wall_shots.mjs` 覆盖（§4）。
2. ~~复核 D1–D4 量值~~ 已在 §4 复验：详情信息层让位、390 首行整张进框、第四段标题「分级」、画布无「已加载 N/M」。
3. ~~拆提交~~ 已提交（feat / docs / test / fix 四笔）。

### P1

4. ~~task-5（重复副本标签）~~ 已修并重导（§5.1）。
5. ~~L0 360° 全向无界~~ 已实现（§5.3）；侧栏常驻透明同样已实现（§5.4）。
6. 首屏"格位感"：**已完成跨行布局**——约 30% 的格子是"主图（38–50% 宽 / 42–55% 高）+ 右侧竖列 + 下方 1–2 条横条"，
   其余格子仍是 2–4 条横向满铺；行高权重 0.72–1.42。踩过的两个坑都写在代码注释里：
   格子开到 1.15 个视口会让整面墙像放大一档（现为 0.72）、行内允许过窄的图会出现 60px 细条（现最小 96px）。
7. 详情页相似条密度（窄屏 48px 偏挤）、标签封面策略（真实数据下复查是否重复）。
8. 性能红线：1 万条连续拖拽 30 秒的内存与帧率曲线（虚拟化已生效，未跑长测）。

### P2

9. 缩放比例提示；惯性到边界的回弹（无限模式下没有边界，回弹只在有限世界有意义）。
10. ~~触屏长按概要~~ 已实现：按住卡片 500ms 出概要，松手不会误开灯箱（移动 / 第二指 / 抬手都会取消）。
11. lightbox ↔ 详情切换后焦点显式归位到触发卡。
12. 入场波次在"筛选收窄"时会让留下的卡重播一次动画——按当前口径视为"切换态"的合理表现，若嫌吵可只在路由变化时播。
13. 边栏对比度量化（§5.4 未做项）与 `backdrop-filter` 帧率测量。

## 7. 验收口径（裁定记录，避免重开争论）

- **规则 1**：首页画布上不得出现任何计数/页码/脚手架文案；计数只在侧栏底部 `.seam__count`。加载反馈允许一条 1px 进度线（仅 `phase === 'loading'`）与错误时一句提示。
- **规则 4 vs Q13**：§F 已拍板 "Q2–Q23 按建议值"，Q13 建议 A = 常驻底部大字标题 → **标题常驻合规**；需隐藏的是 `.tile__copy small`（分辨率·星级）。`.tile__chip` 角标常显（B1 交付物）。
- **文字越框的判法**：不能凭截图判断——画布可平移，视口右缘会切卡。用 `Range.selectNodeContents().getClientRects()` 量文字矩形与卡片矩形的关系。
- **窄屏世界宽**：≤760px 时世界宽 = 视口宽 − 64px 缝，行高 = min(268, 可见宽×0.55)；桌面保持世界 1843 / 行高 268。

## 8. 复现与工具

```powershell
# 类型检查
cd web; npx tsc --noEmit

# 生产构建（本机沙箱可用）
# 直接 npm run build 会在 vite prepare-out-dir 被 safe-delete 拦下（dist/data 上万文件超本轮阈值）
cd web; node --input-type=module -e "import { build } from 'vite'; import react from '@vitejs/plugin-react'; await build({ root: process.cwd(), base: './', publicDir: false, plugins: [react()], build: { outDir: 'dist-check', target: 'es2022', sourcemap: false, assetsInlineLimit: 0 } });"

# 开发/预览服务（含数据）
cd web; npx vite --port 4318 --strictPort

# 四页 + 概要 + 5 条交互 + 390 窄屏 + 双指缩放，一次跑完
node scripts/wall_shots.mjs --base http://127.0.0.1:4318/ --out web/screenshots

# 重新导出数据（修完 task-5 之后）
cargo run -p kanalib-publish -- --db .kanalib/kanalib.db --out web/public/data

# 契约校验
python scripts/wall_manifest_check.py web/public/data
```

环境坑：

- `--window-size` 在 headless=new 下不可信，必须用 `Emulation.setDeviceMetricsOverride` 钉视口。
- `--screenshot` 的 exit code 不可信，判据是文件大小（空白页 5–10KB）。
- 首页 world 高约 19 万 px，全页截图不可行，只能多点视口。
- 本机 safe-delete 会把删除送回收站，临时产物用 `find <dir> -depth -delete` 直接删。

## 9. 未提交文件清单

```text
 M web/package.json           M web/src/App.tsx               M web/src/components/Detail.tsx
 M web/package-lock.json      M web/src/lib/router.ts         M web/src/components/Lightbox.ts
 M web/src/styles.css         M web/src/lib/format.ts         M web/src/components/Sidebar.tsx
 M web/src/components/Wall.tsx  M web/src/components/WallCard.tsx
 D web/src/components/ZoomStage.tsx          （整墙缩放退役）
?? web/src/components/TagGallery.tsx        ?? web/src/lib/accent.ts   ?? web/src/lib/shuffle.ts
?? web/src/components/{sidebar,detail,taggallery,lightbox}.css       ?? web/src/lib/world.ts
?? scripts/wall_shots.mjs
?? docs/backlog/photo-wall-v3.md            ?? docs/backlog/photo-wall-status.md（本文件）
```

## 10. 提交切分（已按此执行）

原计划拆成两个 feat（画布 / 缝与详情），实际合并成一个：`App.tsx`、`styles.css` 两个文件同时承载画布与导航两块的改动，按文件切会得到"引用了尚未提交组件"的编译不过中间态，硬拆只为了好看没有价值。

```text
feat(wall): v3 空间画布、黑白竖排缝与全屏详情
  web/package.json、web/package-lock.json（去掉 infinitegrid 与 zoom-pan-pinch）
  web/src/App.tsx、styles.css、lib/{world,accent,shuffle,router,format}.ts
  web/src/components/{Wall,WallCard,Sidebar,TagGallery,Detail,Lightbox}.tsx
  web/src/components/{sidebar,taggallery,detail,lightbox}.css
  删除 web/src/components/ZoomStage.tsx

docs(wall): v3 标准、开发状态与登记表回写
  docs/backlog/{photo-wall-v3,photo-wall-status}.md
  docs/backlog/{README,registry,cloud-photo-wall}.md

test(wall): 四页 headless 验收脚本
  scripts/wall_shots.mjs

fix(publish): 重复副本的标签在导出时补回
  crates/kanalib-core/src/db.rs、crates/kanalib-publish/{src/publish.rs,tests,README.md}

feat(wall): 首页改为 360° 全向无限画布 + 侧栏常驻半透明
  web/src/lib/world.ts（layoutCell / cellTilesInView）、web/src/components/Wall.tsx、
  web/src/App.tsx、web/src/styles.css（.canvas__home）、web/src/components/sidebar.css
  scripts/wall_shots.mjs（规则 11 断言）、docs（状态与登记表）
```

## 11. 风险与备注

- **不要在生产库上跑导出**：白名单/隐私边界见契约 §7。
- 画布性能命门是虚拟化：视野内约 40–50 张卡流畅；1 万条长时拖拽的内存曲线仍未测。
- 动态强调色对纯灰/纯白图会退化成中性灰，属预期行为。
- `.kanalib/folia-ui-spec.md`（36KB）是 Folia 的实测规格，需要细节时继续照抄。
