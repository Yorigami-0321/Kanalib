# Web 端体验审计与优化计划（2026-11-27）

## 1. 结论

桌面端已经形成稳定的视觉语言：黑底、缝式导航、无界画布、详情 `contain` 加环境层、检查器纸面。当前不需要重做视觉方向，短板集中在移动详情层叠、首次使用可发现性、卡面信息价值、搜索与标签发现性、无障碍和图片预算。

本计划按 `P0 移动详情 → P1 发现性与可读性 → P2 沉浸与打磨` 推进。实施时保留以下既定约束：

- 首页不出现工具栏、分页或常驻脚手架文案。
- 导航浮层不推挤画布。
- 图片墙使用 `cover`，详情使用 `contain`。
- 390px 宽度可单手浏览，360° 无界空间成立。
- 标签总览维持全量平铺，不做目录化或窗口化；只增加过滤、类别锚点和动画裁剪。

## 2. 审计口径

审计对象为真实运行的 `preview` 生产包（`http://127.0.0.1:4273/`），并用 CDP 在桌面与移动视口读取 DOM 矩形、资源传输、LCP 和 CLS。截图与原始数据位于 `web/screenshots/audit-2026-11-27/`。

性能数据以 `preview` 生产包为准。`audit-report.json` 中的 dev 依赖不计入预算；`perf-report.json` 的 `desktopDetail` 是同一 SPA 会话的累计值，详情冷启动以独立复测结果为准。

### 2.1 实测基线

| 场景 | 实测 |
| --- | --- |
| 桌面首页 1440×900 | 61 张卡、31 张可见；74 请求 / 2.70 MB；图片 2.27 MB；FCP 96 ms、LCP 676 ms、CLS 约 0.000016 |
| 移动首页 390×844 DPR2 | 两次冷启动 37 / 43 张图；图片 2.67 MB / 8.65 MB，总传输 3.09 MB / 9.07 MB；档位含 `t256/t512/t1024`，偶发 `t2048` 552 KB |
| 桌面详情独立冷启动 | 20 请求 / 900 KB；主图 `t1024` 119.6 KB；LCP 756 ms |
| 标签总览 | 2,000 张卡、DOM 12,173 节点；首屏约 40 张封面 / 617 KB；总资源约 1.14 MB；JS heap 约 16 MB |
| 移动详情 390×844 | sheet 390×464，起点 y 约 380；底栏 390×56，起点 y 约 788；16:9 主图仅露出约 60 px |

生产构建通过：JS 343.85 kB（gzip 111.5 kB）、CSS 46.10 kB（gzip 9.7 kB），无 console error。

## 3. P0：移动详情

### P0-1 默认 55vh sheet 遮挡主图

证据：

- `Inspector.tsx:62` 的 `sheetOpen` 初始值为 `true`。
- `inspector.css:356` 将移动端 sheet 最大高度设为 `55vh`。
- `mobile-detail-info.png` 中 sheet 从约 y=380 开始，16:9 主图只剩约 60px 可见。

影响：用户进入详情的第一眼看不到图，信息面板抢走了浏览图片的主任务。

方案：

- 移动端首次进入详情默认折叠信息 sheet，保留标题行和把手。
- 展开态最大高度降到 `38–42vh`。
- 展开时让详情 stage 为 sheet 让出底部空间，或至少保证主图可见比例不低于 50%。
- 桌面端保持现有左侧 300px 检查器。

### P0-2 底栏压住 sheet 操作区

证据：

- `.seam.rail` 的 `z-index` 为 40（`styles.css:216`），`.inspector` 为 38（`inspector.css:9`）。
- sheet 固定 `bottom: 0`，底栏覆盖“返回列表”和复制/全屏操作。

方案：

- sheet 底边抬到 `var(--seam-h)` 之上，操作区不再被底栏覆盖。
- 详情视图底边同样避开 `--seam-h`。
- 目标态可把移动底栏改成详情专用操作条，或详情页自动隐藏全局导航；最小修复先解决层叠与安全区。

### P0-3 换图重置信息状态

证据：

- `App.tsx:1047` 在 `handleDetail()` 中无条件 `setDetailInfoHidden(false)`。
- `Sidebar.tsx:749` 以 `key={inspectorShown.item.id}` 重挂 `Inspector`，`sheetOpen` 回到 `true`。

影响：用户收起信息后切到下一张，信息又自动展开，连续浏览时反复打断。

方案：

- `handleOpen()` 从墙面进入详情时重置为用户默认态。
- `handleDetail()` 仅用于详情内翻页/相似图跳转，不重置 `detailInfoHidden`。
- 把 `sheetOpen` 提升到 `App` 或共享状态，换图时保留用户选择。

### P0-4 视口与安全区

证据：

- `styles.css:141` 使用 `height: 100vh`。
- 移动端 `--seam-h` 固定为 `56px`（`styles.css:1587`），没有包含 `env(safe-area-inset-bottom)`。
- 底栏没有安全区内边距；`index.html` 已设置 `viewport-fit=cover`，但 CSS 未接上。

方案：

- 增加 `100dvh` fallback。
- `--seam-h: calc(56px + env(safe-area-inset-bottom, 0px))`。
- 同步 `.stage`、`.view`、`.inspector` 的底部偏移。
- 在 iPhone Safari 和 Android Chrome 上验证地址栏收起/展开。

## 4. P1：发现性与可读性

### P1-1 首次使用只能靠猜

证据：

- 桌面提示只写“拖拽平移 · 滚轮滚动 · Ctrl+滚轮缩放 · 右键看概要”。
- 移动端 `styles.css:1546` 直接隐藏提示。
- 完整键位藏在“关于”面板的 `Sidebar.tsx:721`。

方案：

- 桌面提示补充“单击聚焦、双击/Enter 打开详情、长按概要”。
- 移动端显示一次短暂提示：“轻点聚焦 · 双击打开 · 长按概要 · 双指缩放”。
- 提供 `?` 帮助入口或让提示条可点击打开“关于”面板。
- 使用 `localStorage` 记录是否已展示过，不常驻。

### P1-2 卡面信息价值低

证据：

- `WallCard.tsx:48` 的分类优先级为 `general → character → copyright → artist → manual`。
- `chipTextOf()` 在同类里取最高计数，导致大量卡片显示 `1GIRL · 726`。
- 标签墙上每张卡仍重复当前标签。
- 移动端 `styles.css:1691` 隐藏 chip，只剩时间戳/哈希文件名。

方案：

- 优先级改为 `character / copyright / artist > 稀有 general`。
- general 选择低频、有区分度的标签，而不是最热标签。
- 标签墙传入当前 active tag 并排除。
- 移动端保留一行标签 caption，不直接隐藏 chip。
- 时间戳/哈希文件名降级为次要信息，或改为日期 + 尺寸。

### P1-3 搜索不是一等公民

证据：

- 搜索藏在“工具”组（`Sidebar.tsx:506`）。
- 没有 `/` 或 `Ctrl+K`。
- 面板是 300px 浮层，提示为技术语法，无结果数、无“查看结果/关闭”。
- 移动端没有 scrim。
- 进入标签/收藏等总览会静默清空 query（`App.tsx:1129` 起）。

方案：

- 增加 `/`、`Ctrl+K` 快捷键，搜索按钮标注快捷键。
- 面板显示“找到 N 张”和“查看结果”。
- 简化主提示，把 `tag:` 等语法收进“高级语法”。
- 移动端改为带 scrim 的 bottom sheet。
- 若切换总览要清空 query，给轻提示；否则保留 query 并在页面显示 chip。

### P1-4 标签总览没有检索

证据：

- `TagGallery.tsx:80` 全量平铺 2,000 张卡。
- `TagGallery.tsx:88` 每张卡都挂入场动画。
- 1,729 个“通用”标签无法快速定位。

方案：

- 增加 sticky 标签过滤框。
- 增加类别锚点，平滑滚动到对应 section。
- 显示过滤后数量。
- 只给首屏前 60–100 张播放入场动画。
- 保留平铺结构；`content-visibility` 或虚拟化只作为 P2 评估项。

### P1-5 图片预算不稳定

证据：

- `assetUrl.ts:55` 的网格 `srcset` 会按尺寸选到 `t1024`。
- `assetUrl.ts:77` 的聚焦卡直接从 `t2048` 开始。
- `WallCard.tsx:197` 用 `rect.w * displayScale` 计算 `sizes`。
- 移动冷启动图片量在 2.67–8.65 MB 之间波动。

方案：

- 普通网格上限 `t512`；宽卡/聚焦卡才允许 `t1024`。
- `t2048` 只在详情放大或空闲升级时加载。
- 补 `saveData` / `effectiveType` 分支。
- 按最坏随机布局验收，而不是只看一次平均值。
- 目标：移动首屏图片 ≤2.5 MB，桌面 ≤3 MB，详情 ≤1 MB。

### P1-6 对比度与触控

证据：

- 在 `#0c0c0c` 上，`rgba(247,244,238,0.42)` 约 3.8:1，`0.38` 约 3.3:1，`0.32` 约 2.7:1。
- 失败选择器包括 `.tags__group-count`、`.inspector__meta dt`、`.inspector__section h3`、`.wallhead__count`、`.panel__keys dd`、`.view__caption`。
- 移动端 15 个按钮约 30–31×34，chips 高约 23px，复制/全屏文字链接高约 18px。

方案：

- 文本 token 最低 `0.52`（约 5.3:1），小字用 `0.55–0.62`。
- `<0.52` 只用于非文本装饰。
- 主操作 44×44，次操作 ≥36×36。
- 移动底栏减少主项，或详情页只保留详情操作。

### P1-7 无障碍语义

证据：

- 首页默认没有 `h1`，只有标签/分类页有。
- 61 张卡全部 `tabIndex=0`，`aria-label` 直接是文件名。
- 详情 `role="dialog"`（`Detail.tsx:610`）但没有 `aria-modal` 和 focus trap。
- 标签页 2,000 个 button 默认全是 Tab 停靠点。

方案：

- 增加页面 `h1` 和 skip link。
- 卡片采用 roving tabindex，方向键移动，Enter 打开。
- `aria-label` 改为“标题 + 分级 + 主标签”。
- 详情按路由页改成 `article/region + h1`，或真正做成 modal（`aria-modal` + inert 背景 + focus trap）。

### P1-8 面板焦点收口

证据：

- 路由变化时 `Sidebar.tsx:348` 执行 `setPanel(null)`，清理函数把焦点还给触发按钮，导致标签页仍残留排序按钮 focus ring。
- 排序选项点击后面板不自动收起（`Sidebar.tsx:696`）。
- “清除全部筛选”只清 query，不重置 sort（`Sidebar.tsx:739`）。

方案：

- 路由变化关闭面板时抑制焦点恢复。
- 选择排序后自动关闭。
- 按钮改名为“清除搜索”或真正重置 sort。

## 5. P2：沉浸与打磨

- **沉浸模式**：桌面 1440 下 nav 208 + inspector 300，图片实际约 932×524。增加 `H` 或按钮隐藏全部 UI，保留轻量退出提示。
- **检查器降噪**：默认只展示 general/character/copyright/artist/manual，style/meta 折叠到“更多标签”；“同标签”改为“相关作品”。
- **搜索/排序产品化**：最近搜索、热门标签建议、当前排序摘要。
- **大列表动画**：2,000 张标签卡默认同时入场；只给首屏前 60–100 张播放，其余直接显示。
- **错误/空态文案**：`App.tsx:1513` 暴露数据根路径；收藏空态写“工作台”。改为访客可理解的文案，技术细节放 console 或“详细信息”。
- **监控**：补 Web 对比度检查、390/1440 截图回归、随机布局下的图片预算探针。

## 6. 实施顺序与验收

### 第 1 批 P0

范围：移动 sheet 默认折叠/高度、底栏层叠、状态持久化、`dvh` 与 safe-area。

退出标准：

- 390×844 下主图可见 ≥50%。
- sheet footer 不被底栏覆盖。
- 换图保留信息展开状态。
- iPhone 安全区正常。

#### 本轮实施状态（2026-11-27）

P0 已完成；P1 / P2 本轮未实施，相关改动已撤回，留待后续批次。

改动文件：

- `web/src/App.tsx`：`detailSheetOpen` / `detailInfoHidden` 提升到应用层；`handleOpen()` 重置默认态，`handleDetail()` 保留用户选择；深链/前进后退进入详情时恢复默认信息态；详情路由加 `is-detail`。
- `web/src/components/Inspector.tsx`：sheet 展开态改为受控属性，不再在组件内初始化。
- `web/src/components/inspector.css`：移动 sheet 默认折叠、最大高度 38vh、底边抬到 `--seam-h` 之上、把手 28px。
- `web/src/styles.css`：`100dvh`、`--seam-h` 接入安全区、`.view` 让出底栏、详情页移动底栏隐藏全局导航。
- `scripts/wall_probe_inspector_sheet.mjs`：P0 回归探针，覆盖折叠/展开、主图可见比例、底栏层叠、换图状态保持和安全区解析。

验证结果（`preview` 生产包，390×844 / 1440×900）：

- `npm --prefix web run typecheck` 通过。
- `npm --prefix web run build` 通过：JS 344.18 kB（gzip 111.53 kB）、CSS 46.26 kB（gzip 9.69 kB）。
- 移动详情：默认折叠 94px；展开 321px（38vh）；主图可见 69%；sheet footer 底边 788px，与底栏顶边对齐；`--seam-h` 解析为 56px；详情底栏隐藏全局导航。
- 换图后 sheet 仍折叠；隐藏信息后换图仍保持隐藏，重新显示信息时 sheet 仍折叠。
- 桌面检查器仍为 300px 左侧栏，把手隐藏。

### 第 2 批 P1

范围：首次引导、卡面标签算法、搜索入口/结果数、标签过滤/锚点、对比度 token、触控目标、roving tabindex。

退出标准：

- 主要路径可发现。
- 文本 ≥4.5:1。
- 主操作 ≥44×44。
- 移动首屏图片 ≤2.5 MB。

### 第 3 批 P2

范围：沉浸模式、检查器降噪、搜索建议、动画裁剪、错误/空态文案、性能监控。

退出标准：

- 桌面详情沉浸可用。
- 标签列表首屏更轻。
- 文案面向访客。

### 通用验收

- 视口：360×640、390×844、768×1024、1440×900、1920×1080；DPR1/2。
- 键盘：`/` 或 `Ctrl+K` 搜索，Enter 查看结果；详情方向键换图、`I` 收信息、`Esc` 返回。
- 读屏：页面有 `h1`；详情打开时背景不被误聚焦；焦点回到原卡片。
- `prefers-reduced-motion`、慢网、无 console error；不新增明显 CLS。

## 7. 边界

- 不改后端/API 契约。
- 不引入新 UI 库。
- 不做遥测。
- 不推翻标签总览“全量平铺”决策。
- 不重做首页视觉，不把首页变成工具栏/分页页面。
- 代码实施前确认没有其他会话同时修改对应文件。
