# 图片墙开发状态（2026-11-10 21:00）

> 分支 `feature/cloud-photo-wall` · 上级计划 [cloud-photo-wall.md](cloud-photo-wall.md) · v3 标准 [photo-wall-v3.md](photo-wall-v3.md)
>
> 一句话：**在 W3 锐利档与四入口之后，v4 按用户实机反馈补上「单击原位聚焦 + 300px 信息检查器 + 触控板 deltaX/详情横向 wheel」：单击把卡放大并居中、邻卡压暗，双击才进详情；截图 1 的底部信息条删掉，标题/元数据/标签/相似全部进侧栏；详情返回墙上仍保留聚焦与检查器。**
>
> v5（2026-11-10 下午）：按“成熟、流畅、沉浸、美观”再做一轮体验收尾——渐进模糊加载与骨架扫光、hover 微缩放、聚焦压暗、窄屏字号/角标收敛、详情环境模糊背景、滑动轨道 pane 顺序修正（消除中段黑屏）、邻图预解码、滚轮 rAF 缓动、检查器实色化与复制深链/全屏、P 自动放映 + 4.2s 进度线、有限墙垂直居中。
>
> v5.1（2026-11-10 晚，参考 recent.design）：按 Harry Atkins 加 **单色预览（N）**，按 Meech213 / Cathy Dolle 加**留白视图（M）+ 极简图注**；参考清单见 §8.2。
>
> 之前的“详情水平滑动/排序已实现”只在动画和有限墙层面成立，触摸手势、入口结构、标签分组等并未落地；本轮以代码 + 浏览器实测为准重新核过。

## 1. 当前状态速览

| 项 | 值 |
|---|---|
| 分支 / HEAD | `feature/cloud-photo-wall`；轮次起点 `9c3564e`（功能键收进侧栏） |
| 提交情况 | v5.2 代码/测试已提交 `029edc7`…`39fcf71`；v4 `5699cb5`…`4f5c369`；四入口 `bca6cd4`…`c182557`（清单见 §9；本行数据随文档提交） |
| 类型检查 | `cd web && npx tsc --noEmit` → exit 0 |
| 生产构建 | exit 0：CSS 33.89KB（gzip 7.51KB）/ JS 303.55KB（gzip 98.04KB）；v5 增量来自渐进加载、环境层、放映与新交互，v5.1 单色/留白，v5.2 锚定挤压 + 键盘焦点平移 |
| 数据 | **默认数据 `web/public/data/` 已是真实库**：937 条、t1024 785（83.8%）、t2048 346（36.9%）、2,000 facets 标签 / 73 分组；源 `D:\Pictures\bili` 只读。合成 9899 条与测试图片已按用户要求删除（清单见 `.kanalib/cleanup-manifest-2026-11-10*.json`） |
| 验证产物 | 本轮截图/探针产物已按 2026-11-10 清理指令删除；数值留在 §4.11/§4.12/§5.2.1，`wall_shots`/`wall_visual_probe` 可随时重跑，实库默认数据为 `web/public/data` |
| 改动前备份 | 本轮改动前状态由 `a491b85` 保存，可直接 git 回退；未另建目录备份 |

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

### 4.4 本轮增量实测（20:00–20:45，P1/P2 收尾）

工具：既有 `scripts/wall_shots.mjs`（回归）+ 新增 `scripts/wall_perf.mjs`（长时性能/对比度/帧率）+ 一次性 CDP 探针（390 详情、焦点链、排序波次）。

| 项 | 实测 | 结论 |
|---|---|---|
| 缩放比例提示 | Ctrl+滚轮后出现 `126%`（`pointer-events:none`、`aria-hidden`）；1440 下矩形 1371–1422 / 780–808，与 hint(820–848)、home(856–884)、progress(899–900)、error 四者**相交面积全 0**；390 下同样全 0；停手 1.4s 后节点已卸载（`afterExists: false`） | 通过 |
| 有限世界边界回弹 | `#/t/1girl` 甩惯性撞边：越界峰值 26.8px（≤28 上限），195ms 回到合法位（180–220 区间）；结束时 `world` 位移 = 合法值 `-1508.8`，无残留偏移。无限首页同样操作峰值 0（不触发） | 通过 |
| `prefers-reduced-motion` | 同场景峰值 0 / 回弹 0ms（直接夹住） | 通过 |
| 筛选收窄不重播波次 | 搜索收窄到 33 张：`animationDelay` 全部为 0（`max: 0`）；清空搜索扩宽到 44 张：延迟重新出现 `0/30/60/90/120/150` | 通过 |
| 详情相似条 · 1440 | 6 张 58×58，`scrollWidth = clientWidth = 378`（不溢出） | 通过 |
| 详情相似条 · 390 | 单张 61×61，`scrollWidth 395 / clientWidth 298`（溢出 97px）；4 张整 + 右露半张；可滚到 `scrollLeft = 97`，最后一张完整进框（`fullyIn: true`） | 通过 |
| 详情页 390 横向溢出 | `documentElement.scrollWidth = body.scrollWidth = 390 = innerWidth` | 通过 |
| 标签封面 | 135 张卡、段内相邻重复 39 次，其中来自"候选 ≥2 的标签"的是 **0** 次——39 次全部来自单图标签（135 个标签里 **107 个只有 1 张图**，封面唯一，属数据必然）。段内不同封面数 5（受样例库真实图片数限制） | 通过（策略有效，重复是数据决定的） |
| 焦点归位 | 墙 → 点卡开灯箱 → 点「详情」→ Esc 返回：`document.activeElement` 是 `div.tile` 且 `dataset.id` 与触发卡一致（`restored: true`），未退化到 `.canvas` | 通过 |
| 30 秒连续拖拽（blur 开） | JS 堆 7.59 → 8.76MB（**+15.5%**，GC×3 后保留堆）；DOM 卡片 45–81（上限 220）；帧 p50 6.9 / p95 7.1 / p99 13.9ms，掉帧 **0%**，4241 帧；长任务 0 条；主线程占用 53% | 通过 |
| backdrop-filter 代价 | 开/关各 30s：p95 7.1 vs 7.1、p99 13.9 vs 13.9、掉帧 0% vs 0%、主线程 53% vs 51%。**主线程差值为 0，不等于 GPU 无代价**（headless 走 SwiftShader，结论只到"主线程不受影响"） | 通过（口径有限） |
| 侧栏实际像素对比度 | 最差样本 `bright-1-y120`（底图亮度 0.6394）：次级文字「Kanalib」**5.68:1**（目标 ≥4.5），主文字最差 **12.16:1**（目标 ≥7）；理论下限（纯白图 + 名义色，不含抗锯齿）5.36:1 | 通过（另：计数文字 2.55:1，v3 未立口径，仅记录） |

### 4.5 交互重做（2026-11-09 21:00–22:00，用户反馈驱动）

用户四条反馈（Folia 图 1–4 做参考，图 5 即当时的浮动概要卡=不要的形态）：①详情页顶部黑条遮挡；
②右键概要改成"信息叠在卡片上"；③单击突出、双击展开全屏（不要浮层灯箱）；④详情切换像 niri 平滑左右滑。

| 项 | 实现 | 实测 |
|---|---|---|
| 黑条/裁切 | 根因：`.view__img` 的 `height:100%` 写在 grid 自动行上解析不出，盒子按原图比例撑到 1371×1028（>900 视口）→ 底裁顶留黑。改为 img 绝对定位钉死在 pane、media `padding:0` | padding 0px / imgTop 0 / 无裁切；contain 在窄轴留黑属预期 |
| 双击展开 | 共享元素动画：克隆图从卡片矩形飞到 contain 落位（`lib/expand.ts`，`object-fit:cover` 起点与卡片一致、终点宽高比=原图），详情页窗口期内蒙住再露出 | 双击 → `#/d/<id>`；克隆飞行中存在、落位后移除（body>img 0） |
| 单击/双击 | 鼠标单击=选中高亮，同卡 360ms 二次抬起=展开；触屏单击直接展开、长按概要不变 | 探针双击 ok |
| 详情滑动 | 三格轨道（前/中/后各一屏），CSS 动画 420ms，信息层随图走；动画结束才提交中心，不相邻跳转直接落位；滑动中不接新指令 | →: mid `--next` transform -1694（-1440→-2880 中途）→ after seq 更新；←: mid `--prev` → after seq 回退 |
| 右键概要 | 浮动小卡退役，改叠层：暗 scrim + 右上双圆钮（打开详情/关闭）+ 文件名/meta/标签；`data-summary` 隐藏卡片自己的角标与常驻标题（否则两层文字重影，实测抓到） | 叠层 open / Esc 关闭 / 长按同路径 |
| PhotoSwipe 退役 | 灯箱、lightbox.css、probe.ts、photoswipe 依赖全删 | chunk 60.3KB 消失；CSS 32.8→26.8KB、JS gzip 94.5→89.3KB |

**验收注意**：展开动画的蒙层类（`.view--expanding`）必须在窗口结束后摘除——它的 `animation`
声明特异性更高，不摘会永久压住滑动的 `pane-next/pane-prev`（第一轮实测滑不动就是这个原因）。

### 4.6 功能键收进侧栏（2026-11-09 22:00–22:30，用户指令"把所有的功能键都做到侧栏上"）

| 项 | 说明 |
|---|---|
| 缝新增两组互斥功能键 | 墙页 2×2：缩小/放大/缩放复位/回到起点；详情页一行三钮 + 竖排序号：返回/上一张/下一张（到头置灰）。样式跟页签同源，缝宽不变（规则 2） |
| 退役 | 画布「走远才浮出」的回到起点按钮（§4.3 曾裁定"只在需要时出现"，本条按用户新指令改判为常驻侧栏）；详情页整条顶栏；标签墙标题条返回键（侧栏「标签」页签已覆盖） |
| 实现要点 | 缩放按钮经 `WallControls` ref 句柄直调 Wall，不走 React 状态（缩放不触发全树重渲染）；键盘 +/- 与按钮共用同一路径（同锚点、同瞬态比例提示）；回到起点无限回格子原点、有限回顶部 |
| 坑 | 竖排一列 4 钮会把底部计数挤出 900 高视口（实测 foot 被推出界）→ 压成 2×2 网格；顺带清掉 styles.css 重复定义的 `.view__bar` 旧块 |
| 实测 | 缩放 1→1.25→1.563→复位 1；拖远 e=-1117 → ⌖ 回原点；详情翻页 seq 2311→2312、返回 hash #/；`.view__bar` 已不存在；390 缝 64px、4 钮、无溢出；wall_shots failures 空；构建 exit 0 |

### 4.7 锐利档 t1024/t2048（2026-11-10，W3 落地）

| 项 | 实现 | 实测 |
|---|---|---|
| 发布器生成 | 一次解码同时产出两档：EXIF 方向套用 → ICC/EXIF 广色域转 sRGB → Lanczos3 按长边缩放 → JPEG q90；源图长边不足档位不放大；视频/ffmpeg-only/源缺失只降级 | 隔离根 5 张真实图（含中文文件名）：t1024 新写 3、t2048 新写 1、无需生成 6、失败 0；ui_screenshot 产出 2048×1339，克拉拉 576×1024（原图 1120×1992） |
| 增量与 dry-run | 锐利档走同一账本指纹；`--no-tiers` 可关；`--dry-run` 只计数 | 重导 9899 条 11.3s：缩略图全命中账本（跳过 19798），锐利档源缺失 9899（原图已删）、0 失败 |
| 网格 srcset | `256w/512w` + `sizes = 卡片宽 × 相机缩放`；降级到 t512 后不再带 srcset | 1440×900、DPR1：显示宽 154–218px 的卡选 t256，262–1029px 的卡选 t512 |
| 详情回退链 | 原图 → t2048 → t1024 → t512（`onError` 逐级） | 同一张 ui_screenshot：t2048（naturalWidth 2048）→ 删 t2048 后 t1024（1024）→ 删 t1024 后 t512（512） |
| 契约/校验 | `paths` 增 `t2048/`；契约 §1/§6.2/§7 回写；`wall_manifest_check.py` 增两档覆盖统计 | `python scripts/wall_manifest_check.py web/public/data` → exit 0（8 条既有 rating 类别警告），t1024/t2048 覆盖 0/9899 |
| 回归 | 类型检查 + 生产构建 + 四页脚本 | tsc exit 0；build CSS 25.33KB / JS 278.83KB（gzip 5.84/89.88）；`wall_shots` 74 卡、135 标签、390 窄屏、failures 空 |

**开发数据的限制**：现有 9899 条全部来自已删除的 `stress_10k`，`assets.path` 指向的原图不存在；样例库 14 条的缩略图缓存也已不在（t512 源缺失），所以这批数据既不能补原图也不能补锐利档。t1024/t2048 的清晰度收益要等接入真实图库后复验，**不能拿当前墙的观感当基线**。

### 4.8 锐利档声明 `sharp`（2026-11-10，续）

| 项 | 实现 | 实测 |
|---|---|---|
| 发布器声明 | 每条 item 写 `sharp` = 该条目最高可用锐利档（`0`/`1024`/`2048`），与账本/实际文件一致；`--no-tiers` 时全写 0 | 夹具 6 张真实图（3840×2160、2480×4016、1300×1149、744×988、557×697、280×175）：sharp 依次 2048/2048/1024/0/0/0；14 条契约测试加了 5 处 sharp 断言 |
| 网格选档 | srcset 基础两档，`sharp>=1024/2048` 才追加 `t1024 1024w` / `t2048 2048w`；`sizes` 仍为显示宽×相机缩放 | 1440 视口、DPR2.5：大卡 413px 选 **t2048**、中卡 427px 选 **t1024**、小卡 484px 选 t512；小卡 srcset 只有 `t256/t512` |
| 详情不试探 | 按 `sharp` 只请求存在的档位；`0` 直接 t512 | MutationObserver 记录：大图 srcLog `['t2048']`、中图 `['t1024']`、小图 `['t512']`，各只一次；真实数据（9899 条 sharp=0）详情 srcLog `['t512']` |
| 旧数据兼容 | `sharp` 缺省时退回逐档试探 | 删掉大图的 sharp 与 t2048 文件后：srcLog `['t2048','t1024']`，最终落到 1024，画面正常 |
| 契约校验 | `sharp` 值域 0/1024/2048；声明 t1024/t2048 时文件必须存在 | 重导后 `sharp 声明 9899/9899（t1024 0、t2048 0）`，validator exit 0（8 条既有 rating 警告） |
| 回归 | 类型检查 + 生产构建 + 四页脚本 | tsc exit 0；build CSS 25.33KB / JS 279.23KB（gzip 5.84/90.05）；`wall_shots` 135 标签、390 窄屏，failures 空 |

**口径**：`sharp` 缺失 ≠ 无档位，而是“旧发布器没声明”；新发布器**总是**写（含 0）。这样前端既能对旧数据保持兼容，也不再为无档条目制造 404。

### 4.9 按拍板口径复查并补齐网页层（2026-11-10）

复查方法：逐项对照 v3 的 Q1–Q23/§3 硬规则读代码，再用 headless 浏览器真实点击、触摸、滚轮与截图验证；不再只看旧状态文档。

| 项 | 原状（代码/浏览器） | 本轮实现 | 实测 |
|---|---|---|---|
| Q8 四个入口两级结构 | 只有标签是「封面总览 → 二级墙」；收藏/分级/分组只是侧栏里的筛选面板 | 新增 `#/favs → #/fav`、`#/ratings → #/r/<G\|S\|Q\|E>`、`#/groups → #/g/<group>`；侧栏三个页签改为入口，搜索/排序仍是面板 | 分组总览 2 卡（原创 1 张、截图/文本 1 张）→ 二级墙 1 张；分级总览 4 卡（G 9,899、S/Q/E 0）→ `#/r/G` 墙；收藏总览 1 卡（0 张）→ 空墙提示「还没有收藏的图片」 |
| 详情触摸换图 | 只有键盘/侧栏按钮能触发滑动，触摸横滑无效 | Pane 增加单指横滑（阈值 70px、横向占优）；滑动中不接新指令 | 1440 触摸左滑 → 下一张；右滑 → 回到上一张；390 窄屏左滑同样生效 |
| 详情缩放/平移 | 无 wheel/pinch/pan，图片固定 contain | Ctrl/⌘+滚轮以光标为锚缩放；双指缩放；放大后拖拽平移并夹在边界内；双击复位 | Ctrl+滚轮 1→1.616；双指 1→2.8；双击后 `transform: none`；放大状态横滑改为平移、不换图 |
| 详情标签 | 44 个 chip 平铺，遮住半屏，无类别 | 按类别分组（通用/角色/作品/画师/手动/元信息/分级）；默认只露 18 个、每组最多 6 个，其余 `+N`；「全部 N 个标签」展开后列表自己滚，按钮留在滚动区外 | `anime_chibi.png`：4 组（通用/作品/元信息/分级）、折叠 11 chip、按钮「全部 44 个标签」；展开 44 chip + 「收起标签」 |
| Q7a 强调色分层 | 标签页/总览不取封面，详情色会残留到下一页 | 首页聚合视野；标签/收藏/分级/分组（总览与二级墙）用该入口封面；详情用本图；无封面回中性灰 | 标签总览 `rgb(186,163,87)` = 首个通用封面的黄；`1girl` 同色；`outdoors` `rgb(167,144,210)`；详情用本图色 |
| 排序 | 无限首页的排序只改变随机池，画面仍随机；排序控件等于无效 | `sort !== random` 时首页切为有限有序墙（`infiniteHome` 只在随机且无搜索时成立） | 切「文件名」后可见卡 `data-index` 0–14，名称严格升序；回到随机恢复无限 |
| 搜索范围 | 仅在已加载分片上过滤；筛选后停止拉全库 | 所有列表/总览页 `needAll=true`，分片拉全后再过滤；搜索面板只筛文件名 | 搜 `synthetic_00001`（第二分片）命中 1 张；`#/r/G` 计数 9,899 与库一致 |
| W4 noindex | index.html 写死 `noindex, nofollow`，与「公开只读、默认可收录」相反 | 去掉静态 noindex；需要私密部署时在托管层加 `X-Robots-Tag` 或重新加 meta。**2026-11-10 用户改选「公开但 noindex」，已恢复 meta（见 §5.2.1）** | 浏览器 `meta[name=robots]` 为 `noindex, nofollow` |
| Q4 切页 | 总览/二级墙无淡入 | `.tags` 与 `.wallhead` 加 350ms `page-in` 淡入 | 总览首屏截图正常；无位移 |
| 代码 | `styles.css` 3009 行，二级墙/标签/详情区块重复 8 次 | 删除 6 份完全相同的副本与 1 份重复的二级墙/标签段；保留概要层与最终生效的详情基底 | 3009 → 1288 行；tsc/构建/wall_shots 全过，截图无回归 |

**已知口径**：
- 390 下 `wall_shots.mjs` 的规则 11 行程、拖拽与 Ctrl+滚轮断言是按桌面标定的；用 `--width 390` 跑会报这四项。移动端实际用触摸左滑/长按/横拖单独验过，截图见 `mobile-detail-tagged.png`，不把桌面脚本的 390 结果当功能失败。
- 详情相似条数量由 pHash 距离决定；`anime_chibi.png` 本轮为 0，邻图有 6，属数据分布，不是布局缺失。
- 收藏/分级/分组的第一层计数与封面取原始 mtime 顺序，刷新稳定；收藏当前为 0 张，所以收藏总览是空白封面卡。

### 4.10 用户反馈：单击聚焦 / 触控板横滑 / 信息侧栏（2026-11-10，已实施）

用户四条实机反馈（[photo-wall-focus-mode.md](photo-wall-focus-mode.md) §1）：单击只选中、触控板横滑无反应、详情底部信息条没搬进侧栏、侧栏信息不全。本轮按该文档 §9 的推荐全部落地：

| 反馈 | 实现 | 实测 |
|---|---|---|
| 单击应原位放大、占视觉中心，但不是全屏详情 | 单击把卡片在布局里换成更大的 rect（`layoutSpotlightCell`/`layoutFeaturedWorld`），同格余图重排到右列/底带、相邻格被推开；相机居中；不跳路由；双击/Enter 才进详情。**v5.2 已改判：原位按比例放大、邻卡不重排、相机不动，见 §4.12** | 卡片中心 `(930,450)` = 内容区中心；首页 329×289 → 473×448；`overlap: 0`、`transformTiles: 0`、`zTiles: 0`；Esc 关闭；从详情返回仍保持聚焦（该组数据为 v4 历史口径） |
| 聚焦要“挤压邻图、都在一层” | 去掉 `transform: scale`、聚焦 z-index、邻卡压暗；改为 `left/top/width/height` 320ms 过渡；`.tile:hover` 的 `z-index: 3` 也删除。**v5.2 改判：不重排，只压暗；英雄卡 z-index 3 + 投影，见 §4.12** | 聚焦卡 `z-index: auto`、`transform: none`；所有卡同层；探针见 `.kanalib/v4/reflow.json`（v4 历史口径） |
| 网格图片黑块 | 画布已虚拟化，`loading="lazy"` 在 `.world` 平移下只触发左上角一小撮；改为 `loading="eager"`，聚焦卡 `fetchpriority="high"` | 冷载 45 张可见卡约 6s 全部加载；`reflow-home/finite/mobile.png` 无黑卡 |
| 触控板双指左右划没反应 | 墙 wheel 同时吃 `deltaX/deltaY`；Shift+wheel 兜底；详情 1× 时 deltaX 累积 80px/冷却 450ms 切图，放大后改平移；`ctrlKey/metaKey` 才缩放 | 墙 `deltaX=80` → world e -80；详情 `deltaX=120` → 下一张、`-120` → 上一张；390 触摸横滑仍生效 |
| 截图 1 底部信息条没删、也没进侧栏 | 删除 `Detail` 的 `.view__info/.view__scroll`；标题/尺寸/日期/分级/星级/分组/标签/相似/原图全部进 300px 检查器；`I`/信息钮可收起 | 详情 `oldInfo=0`、stageLeft 420；检查器标签 11 chip、相似 6；关闭 → stageLeft 0、重开回 420 |
| 侧栏信息不全/不美观 | 检查器分组：标题、两列元数据、按类别标签（默认 18、每组 6）、3 列相似、打开详情/原图；95% 纸面 + 投影，无 backdrop-filter | 1440/390 截图见 `.kanalib/v4/`；390 为底部 sheet（`x=64,w=326,h=464`、overflow 0），聚焦卡在 sheet 上方居中 |

实施状态与偏差（墙上相似图统一进详情、详情信息隐藏不跨页保留）见 [photo-wall-focus-mode.md](photo-wall-focus-mode.md) §11。

### 4.11 成熟度与动效收尾（2026-11-10 下午，v5）

用户目标：把展示做到“成熟、有流畅动画特效、沉浸、美观”。参考物：Wix Pro Gallery 的模糊占位、Apple Photos/Unsplash 的环境图、Google 的 scroll/view timeline 与常见 1500ms 骨架扫光（来源见 §8.1）。本轮按“先取证、再改、再复拍”执行，生产预览（`vite preview`）上全部复测。

| 项 | 实现 | 实测 |
|---|---|---|
| 网格渐进加载 | 未就绪时 `blur(14px) + scale(1.035)`，`onLoad` 后 700ms 内回到 1x 清晰；`.tile::before` 1.5s 低对比扫光，仅 `:has(img:not(.is-loaded))` 时出现 | 冷载 `pending 0` 用时 0.6–0.8s；慢网络下骨架可见，图片到帧后无布局抖动 |
| hover / 聚焦层次 | hover 只保留滤镜 + 2px 抬升 + 1.045 图片微缩放；聚焦时非英雄卡 `opacity .52 / saturate .55 / brightness .7`，英雄卡 18px 圆角 + 内描边；`.canvas.is-spotlight` 由 `spotlightId` 驱动 | 既有 hover 断言仍为 `saturate(1.08) brightness(1.05)`，`detailOpacity 1`；聚焦截图邻卡明显压暗、所有卡同层 |
| 窄屏字号/角标 | `titleSizeForWidth` 细分 10/11/12.5/15/18/23/27；≤760 标题两行、9px 角标；≤560 直接隐藏尺寸角标 | 390×844：世界 326、无横向溢出；示例卡 142–191px 宽，标题 10–12.5px，chip `display:none` |
| 详情环境层 | 同图 t512 铺满 view，`blur(52px) brightness(.44)` + 暗角；`contain` 留边不再纯黑 | 详情 stage 1020×900；环境层 `background-image` 指向当前图 t512；截图中心锐利、四周延伸同色 |
| 滑动轨道修正 | 原 pane 顺序把“下一张”放在动画终点前的空格，220ms 时只剩旧图右缘 → 黑屏；改为 next=`[空,旧,新]`、prev=`[新,旧,空]`，目标屏跳过淡入 | 1440 下 → 键：220ms 截图为旧图左缘 + 新图主体；900ms 后 hash 前进、三个 pane 均 `loaded`；此前同一时刻为纯黑 |
| 邻图预取 | 详情 mount 时对前后各一张 t512 `new Image()` + `decode()` | 滑动目标屏 `loaded:true`，无需等 onLoad |
| 滚轮缓动 | 墙 wheel 的 pan/zoom 走 170ms rAF ease-out，离散滚轮目标可连续叠加；拖拽/第二指/按钮会立刻 `stopGlide` | Ctrl+wheel 1→1.528（既有断言）；drag 断言通过，无残留惯性 |
| 检查器 | 背景改实色渐变（消除墙标题透字）；相似项 hover 2px 抬升 + accent 内描边；新增“复制链接 / 全屏（F）”；复制的是 `#/d/<id>` 深链 | 按钮文案 `✓ 已复制链接`；剪贴板回读 `http://127.0.0.1:4273/#/d/<当前 id>`；检查器宽 300 |
| 自动放映 | 侧栏详情组新增 ▶/❚❚（P 键同义），每 4.2s 切下一张、到尾回环；底部 2px accent 进度线随换图重置；离开详情自动停 | 点击 ▶ 后 `bar:true`、`aria-pressed:true`；4.2s 后 hash 前进；暂停后进度线卸载 |
| 单色预览（N） | 参考 Harry Atkins 的 Monochrome：侧栏底部新增半实心圆按钮，`N` 同义；`.app.is-mono` 只给网格/标签封面/相似条/详情图和环境层去色，UI 与动态强调色保留；图片自身的 filter 过渡负责淡入淡出 | `appClass: app is-mono`、`pressed: true`、图片 `grayscale(0.976) contrast(1.039)`；强调色仍为该视野的 `rgb(199,122,145)`；恢复彩色不报错 |
| 留白视图（M） | 参考 Meech213 的 print-style 与 Cathy Dolle 的极简画廊：详情侧栏新增画框按钮，`M` 同义；图片按 `--matte-x/-y/-b` 显式算宽高，`drop-shadow` 跟随实际像素；信息收起时底部出现文件名 + 尺寸的极简图注，环境层压到 22% | `classOn:true`；1440 下图框 `[130,72,1181,729]`（4:3 内容居中）、图注 `[130,848]` 横跨图框；390 下留白 16/18px、图注隐藏 |
| 有限墙居中 | 内容高度小于视口时，`.world` 加纯视觉垂直余量（camera 不变），只有 3 张图的标签墙不再贴顶 | `#/t/1girl` 3 张卡垂直居中；无限首页与可滚动有限墙不受影响 |
| 路由守卫 | 直接改 hash 进 `/tags` 等总览页时清掉残留 spotlight/summary | 总览截图不再被上一页检查器遮挡 |
| 测试脚本 | 右键概要用例优先选“整卡在安全区”的样本；半出屏卡本来就会让贴卡概要在视口外 | `wall_shots` final：failures 空，summary `inViewport:true` |

**性能与回归**（`wall_perf.mjs`，生产预览，headless/SwiftShader）：

| 项 | 实测 | 结论 |
|---|---|---|
| 30s 连续拖拽 | 堆 5.39→6.73MB（+24.7%）；DOM 45–75；帧 p50 7ms / p95 7.3ms / p99 14ms；掉帧 0.05%；长任务 0；主线程 55% | 达标 |
| backdrop-filter 开关 | p95 7.3 vs 7.3、p99 14 vs 14.1、掉帧 0.05% vs 0、占用 55% vs 57% | 主线程无差；GPU 侧仍无结论 |
| 侧栏对比度 | 次级文字最差 5.78:1、主文字 11.49:1 | 达标 |
| 四页 + 交互 + 390 + 双指 | failures 空；两次复跑横走 35,435–36,041px / 竖走 8,777–8,814px，覆盖 89.3–95.3% / 90.6–91.3%；双指 1→1.6 | 达标 |

**口径与风险**：
- 现有 9899 条是合成渐变图，环境层/模糊在真实照片上的观感与 GPU 代价需接入真实库后复验；本轮只证明结构与主线程帧率。
- 大图淡入从 420ms 缩到 160ms；滑动目标屏完全不淡入，避免 520ms 动画中出现半屏空窗。
- 自动放映没有任何持久化，刷新/离开详情即停；这是只读展示墙，不写本地存储。

#### 4.12 聚焦挤压返修：细条、锁中心、跳变（2026-11-10 v5.2）

用户明确要求**保留“图片挤到周边”的效果**，只修三件事：邻图变细条、点邻图/切图后强制锁中心、从小到大过渡不自然。挤压重排保留，算法与动效重写：

| # | 根因 | 修法 |
|---|---|---|
| 1 | 右列/底带固定 1 列或按行数硬分，条目一多单元格就变成 300×90 的横向细条 | `gridColumns()` 按“剩余区域宽高比 × 平均图片比例”选列，并惩罚短边 <88px 的方案；右列不再写死 1 列 |
| 2 | 卡片 `key` 与渲染顺序都跟着 rect/布局顺序变，重排时 React 移动/重挂节点，宽高 transition 被打断 | `key`/排序改成 `item.id + 格子 + index`。重排只改 rect，DOM 节点不移动，挤压与放大都能连续过渡 |
| 3 | 每次聚焦都调 `centerOnWorld`，新英雄卡被强制拉到内容区中心；点邻卡、开关检查器都会重演 | 去掉强制居中，只在英雄卡被缝/检查器/sheet 遮住时做**最小平移** |
| 4 | 重排把英雄固定放在格子左上角：点邻卡后，新图被搬到英雄位、旧英雄被排到邻位 → 视觉上“交换图片” | 点击瞬间记录被点卡片的 world rect，`layoutSpotlightCell` / `layoutFeaturedWorld` 以它为锚；新英雄留在原地放大，邻图围到它周围 |

英雄卡仍占大块，其余图继续挤到英雄四周（右列/底带，按空间方向自动选择）；锚点矩形来自点击时的 DOM rect，已包含 rebase/verticalSlack 修正。展开 420ms `cubic-bezier(.4,0,.2,1)`，基础收回 320ms；相机仅必要时 420ms 补位。

实测（`scripts/wall_visual_probe.mjs`，1440×900，生产预览）：

| 指标 | 实测 | 结论 |
|---|---|---|
| 放大中间帧 | 起始 `[478,126,268,254]` → 150ms `[430,126,363,320]` → 最终 `[444,126,473,397]` | 宽高都有真实中间态 |
| DOM 身份 | `focusMid.sameNode: true`、`spotlight.sameNode: true` | 重排时未卸载重挂 |
| 挤压结果 | `moved: 12`（邻卡确实被挤动）、`strips: 0`、最小邻卡 `133×114` | 保留挤压，没有细条 |
| 图片是否被搬位 | 初始 `anchorDelta: [0,72]`、点邻卡 `[0,-20]`；x 均为 0 | 被点图片留在原处放大，不再交换位置 |
| 相机 | 初始 `camDelta: 69`、点邻卡 `camDelta: 169`、`centerDelta: -249` | 仅避让检查器，不是居中锁定 |
| 点邻卡 | 新英雄 `[444,369,473,397]`、`inSafe: true` | 只切换放大对象 |
| 二级墙 | 英雄 `[515,142,493,616]`、`centerDelta: -169` | 锚定点击位置 + 挤压，非居中 |
| 390 聚焦 | 英雄 `[88,128,318,228]`、sheetTop 380、`aboveSheet: true`、`strips: 0` | 英雄在 sheet 上方 |
| 键盘焦点 | Tab 连续 6 次：6/6 张卡都在视口内（焦点卡触发最小平移） | 不再出现盲焦点 |
| 回归 gate | `wall_shots` 新增聚焦断言：`moved>0`、`strips=0`、`sameNode`、点邻卡 `|anchorDelta.x|≤16`、`|centerDelta|≥40`；failures 空 | 可防回退 |
| 详情返回 | 按 `spotlightAnchor` 把锚点位置放回视野一次 | 相机重置后不丢聚焦卡 |
| 详情/放映/复制/单色/留白 | 既有断言全部保持 | 无回归 |

**口径**：`photo-wall-focus-mode.md` §4/§9/§11 的“相机居中 / 零重叠”作废；“挤压重排”保留。验收看“点哪张哪张原地放大 + 邻卡被挤动 + 无细条 + 不居中”。

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

### 5.2 质量档位（依赖 F-080）——**2026-11-10 已实现**

- 导出已产出 `t1024/t2048`（q90、sRGB、EXIF 方向、Lanczos3、不放大），每条 item 写 `sharp` 声明可用档，前端详情逐档选档、不再 404 试探（§4.7/§4.8）。
- 当前开发数据覆盖 0/9899：源图库已删除，只剩缩略图缓存。真实图库接入后需要重跑一次发布，并用 `wall_manifest_check.py` 看两档覆盖率。
- 原图仍由 `--with-originals` 控制；不发布原图时详情最高用 t2048，发布时优先原图。

#### 5.2.1 实库复验：`D:\Pictures\bili`（2026-11-10，源目录只读）

- 源：952 个文件 / 2.12 GiB（952 个图片扩展名，无 `.kanalib` 侧车）。扫描前后记录 `source-before.json` 并逐文件比对路径、大小、mtime：**完全一致**，未在源目录写任何文件。
- 隔离数据根：`.kanalib/bili-real`（DB/缩略图在这里，55 MiB）；首次 headless 扫描 206s：imported 952、unchanged 0、thumb_errors 1（`bili2/maomi_...png` 是坏 PNG）、exact 重复 13 组；`KANALIB_SKIP_TAG=1` 先跑通视觉链路。
- 导出到默认 `web/public/data`（405 MiB，已加入 `web/.gitignore`）：**937 条**（重复 id 14、宽高 0 一条）；t512/t256 覆盖 937/937，t1024 **785/937（83.8%）**，t2048 **346/937（36.9%）**；`sharp` 声明 937 条；未打标版本 manifest 0 警告，打标后 1,244 条 `rating`/`style` 类别透传警告（非错误）。
- 2026-11-10 用户要求“之后默认真实库，把测试用图片全部删掉”：`web/public/data-bili` 已改名为默认 `data`；删除 `data-synthetic`、`web/dist`、`web/screenshots` 及 `.kanalib` 下 42 个测试图库/证据目录、根目录 211 张测试截图与 4 个测试浏览器 profile，共 **176,657 张图 / 3.27GB**；保留 `bili-real`（真实派生 DB/缩略图，67MB）、`backups`、文档和清理清单。
- 真实库视觉探针（937 条）：首屏 72 卡、pending 0；详情实际加载 **t1024**；聚焦 `strips: 0`、点邻卡 `anchorDelta.x: 0`、相机未居中；Tab 6/6 在视口内；console errors 空。
- 实库暴露的基础格位问题：`pickStrip` 允许第一张竖图只有 60px 宽，标题被切；已改为所有入选卡先过 `min(行宽×0.45, 96)`，整行无宽卡才降级。合成库 gate 复测邻卡最小宽度 62→116px。
- 打标（sidecar，模型/runtime 从项目根只读接入隔离根）：951/952 成功、1 张 6496×18898 报 `image too large`；写入 **40,095** 条标签链接；扫描/打标期间源目录逐文件比对仍完全一致。
- 带标签重导：facets **2,000 标签 / 73 分组**；标签墙真实封面、`1girl` 739 张；详情可见 S 敏感分级与真实标签；契约校验通过（1,244 条 `rating`/`style` 类别透传警告，非错误）。
- **内容风险**：真实库含 S 敏感内容与成人向标签；W4 虽定“公开只读”，部署前需重新确认访问控制（公开 / noindex / Basic Auth / Tailscale），不能默认直接公开。

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
7. ~~详情页相似条密度~~ 已修（§4.4）：390 下单张 61px、4 整 + 右露半张、可滚；1440 保持 6 张 58px 不溢出。
   ~~标签封面策略~~ 已改为段内贪心去重（§7 裁定）：候选 ≥2 的标签**零相邻重复**，39 次重复全部来自 107 个单图标签。
8. ~~性能红线~~ 已量化（§4.4）：30s 拖拽堆 +15.5%、DOM 45–81、帧 p95 7.1ms、掉帧 0%、长任务 0。

### P2

9. ~~缩放比例提示~~ 已实现（§4.4）：瞬态、不占焦点、与四个既有浮层零相交。
   ~~惯性到边界回弹~~ 已实现（§4.4）：有限世界峰值 26.8px / 195ms 归位；无限模式与 reduced-motion 均不触发。
10. ~~触屏长按概要~~ 已实现：按住卡片 500ms 出概要，松手不会误开灯箱（移动 / 第二指 / 抬手都会取消）。
11. ~~lightbox ↔ 详情切换后焦点显式归位到触发卡~~ 已实现并实测（§4.4）。
12. ~~入场波次在"筛选收窄"时重播~~ 已修（§4.4）：收窄只淡入，扩宽/换页签仍播波次。
13. ~~边栏对比度量化~~ 已实测（§4.4）：次级文字最差 5.68:1、主文字 12.16:1。
    ~~`backdrop-filter` 帧率测量~~ 已测（§4.4）：主线程差值 0；**GPU 侧无结论**（headless 软件渲染）。

## 7. 验收口径（裁定记录，避免重开争论）

- **规则 1**：首页画布上不得出现任何计数/页码/脚手架文案；计数只在侧栏底部 `.seam__count`。加载反馈允许一条 1px 进度线（仅 `phase === 'loading'`）与错误时一句提示。
- **规则 4 vs Q13**：§F 已拍板 "Q2–Q23 按建议值"，Q13 建议 A = 常驻底部大字标题 → **标题常驻合规**；需隐藏的是 `.tile__copy small`（分辨率·星级）。`.tile__chip` 角标常显（B1 交付物）。
- **文字越框的判法**：不能凭截图判断——画布可平移，视口右缘会切卡。用 `Range.selectNodeContents().getClientRects()` 量文字矩形与卡片矩形的关系。
- **窄屏世界宽**：≤760px 时世界宽 = 视口宽 − 64px 缝，行高 = min(268, 可见宽×0.55)；桌面保持世界 1843 / 行高 268。
- **缩放比例提示 vs 规则 1**（2026-11-09 裁定）：规则 1 禁的是常驻计数/页码/脚手架文案。缩放提示是**瞬态交互反馈**，合规的前提是四条同时满足：不缩放时不显示（节点直接卸载）、`pointer-events:none`、`aria-hidden`、不进 Tab 序列。以后加同类提示（如"已回到起点"）沿用这四条。
- **入场波次的"收窄"判据**（2026-11-09 裁定）：按 **item id 集合**判定，新列表 ⊆ 旧列表即视为收窄（只淡入，不播波次）。副作用是**排序变化也不播**（集合不变）——这是可接受的：排序后卡片位置全变，逐格波次反而制造无意义的动效；换页签/换标签/扩宽仍照常播。
- **标签封面**（2026-11-09 裁定）：按 categoryKey 分段贪心，先剔除左邻封面、再取本段用量最少者，起点由 `hash(name)` 轮转决定，**全流程无随机数**（同数据刷新结果一致）。107/135 个标签只有 1 张图，封面重复属数据必然，不算 bug；真实库接入后可复跑同一判据复核。

## 8. 复现与工具

```powershell
# 类型检查
cd web; npx tsc --noEmit

# 生产构建（2026-11-10 复测：npm run build 直接可用，含 public/data 拷贝）
cd web; npm run build

# 生产预览（测真实产物，不用 dev server：dev 长时间跑会偶发图片 pending）
cd web; npx vite preview --port 4273


# 四页 + 概要 + 5 条交互 + 390 窄屏 + 双指缩放，一次跑完
node scripts/wall_shots.mjs --base http://127.0.0.1:4273/ --out web/screenshots/final

# v5 视觉探针：等图加载完再截图；聚焦/详情/滑动/pane 状态/放映/复制深链/390 指标
node scripts/wall_visual_probe.mjs http://127.0.0.1:4273/

# 30 秒长时拖拽（堆/DOM/帧/长任务）+ 侧栏真实像素对比度 + backdrop-filter 开关对比
node scripts/wall_perf.mjs --base http://127.0.0.1:4318/ --out web/screenshots --seconds 30

# 重新导出数据（默认生成 t1024/t2048；只要清单与 t512 时加 --no-tiers）
cargo run -p kanalib-publish -- --db .kanalib/kanalib.db --out web/public/data

# 契约校验
python scripts/wall_manifest_check.py web/public/data
```

环境坑：

- `--window-size` 在 headless=new 下不可信，必须用 `Emulation.setDeviceMetricsOverride` 钉视口。
- `--screenshot` 的 exit code 不可信，判据是文件大小（空白页 5–10KB）。
- 首页 world 高约 19 万 px，全页截图不可行，只能多点视口。
- 本机 safe-delete 会把删除送回收站，临时产物用 `find <dir> -depth -delete` 直接删。
- **local 请求必须绕过 env 代理**：本机 `HTTP_PROXY` 指向 127.0.0.1:58614，`curl http://127.0.0.1:4318/` 不加 `--noproxy '*'` 会拿到 502 假故障。
- **headless 的帧率不代表真机**：没有 vsync，rAF 基准约 143–145Hz；`p95 ≤33.3ms` 是"两个 60Hz 帧周期"的卡顿红线，不是刷新率。backdrop-filter 由合成器执行，headless 走 SwiftShader，**GPU 侧成本量不出来**。
- **性能测量必须在无人改代码时跑**：HMR 重载会让探针失效，`wall_perf.mjs` 用 session 令牌判废并自动重试 3 次，报告里出现"重试 N 次"说明当时有人在改文件，数据要重跑。

### 8.1 动效参考（v5 查过并采用的部分）

| 来源 | 采用的点 | 未采用/边界 |
|---|---|---|
| [Wix Pro Gallery — Designing Your Gallery](https://support.wix.com/en/article/wix-pro-gallery-designing-your-gallery) | 加载期模糊占位、hover 缩放/压暗、滚动入场 | 其“悬停模糊/反色”等强效果不符合本墙“一个状态一种通道”的克制基调 |
| [大厂级 APP UI 交互动效指令（素材集市）](https://www.sucaijishi.com/articles-51-2397-1.html) | 骨架扫光 1500ms、模糊揭示 `cubic-bezier(.25,1,.5,1)`、200ms 淡出 | 未引入骨架屏的高度/布局占位（画布本身已锁定卡片矩形，不会 CLS） |
| [Google — 开始使用 CSS 滚动驱动动画](https://codelabs.developers.google.cn/scroll-driven-animations?hl=zh-cn) | 评估了 `view()` 做入场/视差 | 画布是 transform 平移而非滚动容器，`scroll()`/`view()` 不适用；仍用“进入窗口 + 定时收口”的显式控制 |
| [GSAP Vault — 2026 animation examples](https://gsapvault.com/blog/gsap-animation-examples) | 环境色块/视差氛围、ease-out 收尾的节奏参考 | 不引第三方动画库：产物保持只依赖 React，动效用 CSS/rAF |

### 8.2 recent.design 参考（2026-11-10 采集，v5.1）

从 [recent.design/websites](https://recent.design/websites) 的 20 个最新条目里挑出与图片浏览/作品集最相关的 8 个，逐个读落地页与描述；实际采用两项，其余作为方向校准。

| 条目 | 视觉/交互 | 对本墙的取舍 |
|---|---|---|
| [Harry Atkins — harryjatkins.com](https://harryjatkins.com/)（[Recent 条目](https://recent.design/i/9b18jw0-harry-atkins)） | 超大单字母、极小的项目索引、`Text mode / Dark mode / Monochrome (N)` 三态 | **采用 Monochrome（N）**：图片去色、UI 与强调色保留；不引入 Light/Text 全站换肤（会推翻黑白“缝”的既定标准） |
| [Meech213 — meech213.com](https://www.meech213.com/)（[Recent 条目](https://recent.design/i/i1mq7nk-meech213)） | 编辑影像以“漂浮印刷卡”呈现，卡片有留白与投影 | **采用留白视图（M）**：详情图收进画框 + drop-shadow；不做漂浮旋转卡片（破坏画布的精确排布） |
| [Cathy Dolle — cathydolle.com](https://www.cathydolle.com/)（[Recent 条目](https://recent.design/i/sayzd67-cathy-dolle)） | 极度克制的小字导航、超大留白、画廊主体 | **采用**图注的小字/大留白语言；不把主墙改成白底静态网格（失去无限画布） |
| [Gregor Collienne — gregorcollienne.com](https://gregorcollienne.com/)（[Recent 条目](https://recent.design/i/fjoqfqm-gregor-collienne)） | 米白底、大小不一的漂浮照片、居中的超大无衬线名字 | 验证“漂浮拼贴”方向；我们的无限格位已经是拼贴，不额外加居中字标（会遮挡卡片） |
| [Podium — podium.global](https://podium.global/)（[Recent 条目](https://recent.design/i/gxi9ddp-podium)） | 黑白骨架 + 彩色摄影 + 强节奏项目展示 | 校准“黑白 UI + 彩色内容是成立的”；单色模式因此保留彩色强调色 |
| [Nothing to Watch — nothing-to-watch.port80.ch](https://nothing-to-watch.port80.ch/)（[Recent 条目](https://recent.design/i/6wmr7uw-nothing-to-watch)） | 3D 海报球，空间化随机浏览 | 作为“空间化浏览”的旁证；不引入 WebGL/3D 球（与 2D 无限画布的坐标/性能模型冲突） |
| [Belen Jones — belenjones.com](https://www.belenjones.com/)（[Recent 条目](https://recent.design/i/si1eikw-belen-jones)） | 超大字体 + 3D 编辑项目 + 流动的柔和色场 | 动态强调色已有同源效果；不加 3D 变换 |
| [Avara — avara.xyz](https://avara.xyz/)（[Recent 条目](https://recent.design/i/bimlbnk-avara)） | 暗色交互展厅、角色化插画、顺滑动效 | 校准暗色展厅的动效分寸；不引入插画/品牌 IP |

## 9. 本轮改动清单（v4，已提交 `5699cb5`…`4f5c369`）

```text
 M web/src/App.tsx                       （聚焦/检查器状态、详情让位、触控板、诊断开关）
 M web/src/components/Wall.tsx           （单击聚焦、相机锚点/居中、deltaX、Esc、聚焦恢复）
 M web/src/components/WallCard.tsx       （聚焦锐利档、布局重排、即时加载、隐藏卡上信息）
 M web/src/components/Sidebar.tsx        （挂载检查器；详情信息钮）
 M web/src/components/Detail.tsx         （删底部信息层；横向 wheel；I 开关）
 M web/src/components/detail.css         （stage 让位检查器；只留图与手势）
 M web/src/components/Inspector.tsx      （新：300px 信息检查器）
 M web/src/components/inspector.css      （新：检查器/底部 sheet）
 M web/src/components/GestureDebug.tsx   （新：?gestures=1 诊断）
 M web/src/components/gesturedebug.css   （新）
 M web/src/lib/layout.ts                 （新：缝/检查器/断点常量）
 M web/src/lib/itemDetail.ts             （新：标签分组/相似）
 M web/src/lib/expand.ts                 （containRect 支持内容区偏移）
 M web/src/styles.css                    （聚焦尺寸过渡/同层、--inspector-w）
 M scripts/wall_shots.mjs                （390 首帧等待加固）
 M web/README.md、docs/backlog/{photo-wall-focus-mode,photo-wall-status,cloud-photo-wall,registry}.md
```

> 上一轮四入口/详情手势/整库搜索已提交（`bca6cd4`…`c182557`）；发布器锐利档与 `item.sharp` 在 `77e00b8`…`a491b85`。

## 10. 提交切分（历史记录）

v3 及之前已按下列五笔提交完毕（`f691dc4` 为止）。原计划拆成两个 feat（画布 / 缝与详情），实际合并成一个：`App.tsx`、`styles.css` 两个文件同时承载画布与导航两块的改动，按文件切会得到"引用了尚未提交组件"的编译不过中间态，硬拆只为了好看没有价值。

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
- 画布性能命门是虚拟化：30s 连续拖拽实测 DOM 45–81 张、堆 +15.5%、掉帧 0%（§4.4）。1 万条的数据规模已覆盖；十万条需重新评估（无限首页要求全量分片到齐才生成格子）。
- **锐利档单线程解码**：大库首次发布会把时间花在 t1024/t2048 上，增量发布只重编源指纹变化或目标缺失的条目；十万级真实照片库若嫌慢，下一步再上小线程池（本轮未做）。
- **开发数据 t1024/t2048 覆盖 0/9899**：源图库已删，当前墙的详情仍走 t512 回退。真实库接入后必须重跑发布再评价清晰度。
- 动态强调色对纯灰/纯白图会退化成中性灰，属预期行为。
- **标签封面的"5 种封面"是样例库的限制**：9899 条里只有 5 条真实带标签的图，107/135 个标签是单图标签。真实库接入后要复跑一次相邻重复统计，不能把当前数字当基线。
- `.kanalib/folia-ui-spec.md`（36KB）是 Folia 的实测规格，需要细节时继续照抄。
