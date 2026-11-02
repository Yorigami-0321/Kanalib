# Kanalib UI 重构调研与设计基线

> 关联条目：F-073（总纲）、F-070（侧栏/标签）、F-071（设置/主题位置）、F-072（搜索入口/状态）。
>
> 登记日期：2026-11-02。状态：调研中；阶段 0/1 首批（token、F-071、F-072A、F-070C + 侧栏分组重排）与阶段 2 首批（空状态组件、页头规格、筛选 chip、去重分段控件）已落地，其余方案待确认。
>
> 用户边界（2026-11-02）：主图库网格与查看器/检查器这两张截图的布局不需要特别大的改动；重点放在 UI 组件、视觉体系、状态、外围界面与交互一致性。
>
> 方向修正（2026-11-02，用户指示）：**不以 VS Code 为主参考对象**。主参考改为图片/素材管理器（Lightroom Classic、digiKam、Eagle、Immich、PhotoPrism、Capture One）；桌面系统惯例（Windows 11 / Fluent、Apple HIG）作为次参考；VS Code 只保留命令面板、键盘/焦点、菜单行为等局部模式参考，不作为整体信息架构、视觉或搜索视图的目标。

## 1. 目标与边界

### 目标

1. **功能性**：每个能力一个主入口；搜索、标签管理、主题、设置不再重复；键盘路径完整；空/加载/错误/禁用状态明确。
2. **美观度**：统一间距、字号、控件高度、圆角、边框、图标与动效；用层级、留白、字重建立秩序，而不是堆装饰。
3. **一致性**：侧栏、标题栏、内容区、检查器、底栏、对话框、菜单、toast 使用同一套 token 与状态规则。
4. **可维护**：新增界面直接复用 token/组件，不在业务模块里散落颜色、`px()`、字号和一次性交互。
5. **可访问**：键盘可达、焦点可见、对比度达标、减少动效可用、状态不只靠颜色表达。

### 不改变

- 数据模型、schema、标签语义、别名/合并规则、搜索语法、缓存预算。
- 主图库网格的视图模式、行高、虚拟化与缩略图策略；查看器内联布局与检查器分区（用户明确不需要大改）。
- 100k 图库的性能基线；不得以视觉重构为由引入全量查询或 UI 线程解码。

### 允许的结构调整

- 侧栏/标签工作区的入口与上下文（F-070）。
- 活动栏底部与标题栏控件（F-071）。
- 搜索入口与搜索态（F-072）。
- 标签页、设置页、重复项页、对话框、菜单、空/加载/错误状态的组件与排版。
- 上述调整必须以“减少重复、提升可读性”为前提，不做换肤式重排。

## 2. 当前审计（2026-11-02）

### 2.1 已有基础

- 现有五区壳层：标题栏 48px、活动栏 48px、侧栏 260px（200–420 可调）、内容区、检查器、底栏 26px；方向正确，用户认可主布局，本次不重排。
- 已使用 Ely 的 `ControlSize`、`TextSize`、`IconSize`、`Theme` 与 Lucide 图标；组件基础不是从零开始。
- 暗色调色板、细边框、选中态、虚拟列表、100k 图库滚动与搜索已稳定，应保留。
- 已有命令面板、底栏、toast、快捷键、标签面、设置页等能力。

### 2.2 主要问题

| 区域 | 问题 | 关联 |
|---|---|---|
| 项目级 token | `theme.rs` 主要提供颜色与 3 个壳层尺寸；间距、字号、圆角、控件高度、动效没有集中规范，组件内仍有大量一次性数值 | F-073 |
| 活动栏/标题栏 | 设置与主题位置不符合最新要求；标题栏与活动栏各有一套主题入口 | F-071 |
| 搜索 | 活动栏搜索只聚焦标题栏输入框，不是真正的搜索视图；Esc/点击空白不能退出，焦点高亮残留；全局搜索与局部筛选文案混用 | F-072 |
| 侧栏/标签 | 标签管理与侧栏标签区重复；标签页打开时侧栏仍是资源管理器；两套搜索与计数口径 | F-070 |
| 外围页面 | 设置页、标签页、重复项页的标题/工具条/筛选条/列表行密度与交互不完全一致 | F-073 |
| 状态 | 空/加载/失败/禁用/加载中的视觉与文案不统一；部分列表缺少 hover/focus/selected 规则 | F-073 |
| 组件 | 按钮/图标按钮/输入/菜单/页签/徽章/提示的尺寸与状态不统一；对话框/toast 的层级与动作位置不统一 | F-073 |
| 可访问性 | 焦点环、键盘顺序、图标按钮名称、对比度与目标尺寸缺少统一验收 | F-073 |
| 验证 | 缺少亮/暗、不同宽度/DPI、空库/100k 的截图基线与视觉回归清单 | F-073 |

### 2.3 用户明确保留的部分

- 图库网格缩略图卡片、文件名/评级/标签数信息行、检查器预览与元数据分区、查看器内联舞台。
- 这些区域只做组件级修正（间距/状态/文案/对比度），不做结构重排。

## 3. 主流参考矩阵

> 参考的是信息架构、交互原则与视觉规则，不复制品牌资产、图标、文案或专有布局；开源参考遵守其许可证。
>
> **参考层级（2026-11-02 用户方向修正）**
>
> 1. **主参考**：图片/素材管理器——Lightroom Classic、digiKam、Eagle、Immich、PhotoPrism、Capture One。本产品是本地图片库，信息架构、标签/集合/检查器关系优先按这类产品校准。
> 2. **次参考**：桌面系统惯例——Windows 11 / Fluent、Apple HIG、Microsoft NavigationView。只借用侧栏、导航、设置位置、菜单与键盘习惯。
> 3. **局部参考**：VS Code 等通用工具——只用命令面板、键盘焦点、菜单行为等具体模式；不采用它的整体布局、视觉语言或独立搜索视图作为本项目目标。

### 3.1 主参考：图片/素材管理器

| 参考 | 关键做法 | 对本项目的启示 |
|---|---|---|
| [Lightroom Classic Keywords](https://helpx.adobe.com/lightroom-classic/help/keywords.html) | 左侧目录/文件夹/集合；右侧 Keywords 分配 + Keyword List 全词表（计数、点击筛选、增删改） | 导航侧不放标签管理；标签列表只保留一个权威面板；分配与列表分开 |
| [digiKam 9.0](https://www.digikam.org/news/2026-03-08-9.0.0_release_announcement/) | 左侧 Tags 树筛选；右侧 Caption > Tags 分配；Tools > Tags Manager 维护 | 三面分离：侧栏筛选、检查器分配、管理器维护（F-070） |
| [Eagle](https://en.eagle.cool/) | 文件夹/智能文件夹 + 标签面板（全部/未分类/常用/标签群组）；检查器显示标签 | 标签群组/常用标签适合数万标签；标签与文件夹树分离 |
| [PhotoPrism Labels](https://docs.photoprism.app/user-guide/organize/labels/) | Labels 是唯一标签目的地；`label:` 参与搜索；管理在 Labels 页 | 一个标签目的地，避免主导航再复制一棵标签树 |
| [Immich Tags](https://docs.immich.app/features/tags) | Tags 页 + 侧栏导航；Info 面板添加；层级标签 | 标签页 + 检查器分配；层级化是规模化方向 |
| [Capture One](https://www.captureone.com/) | Keywords 工具分配；Keyword Library 管理；过滤器/智能相册浏览 | 关键字库独立于过滤器；分配与管理不共用同一列表 |
| Lightroom / Eagle / Finder | 搜索框单一位置；Esc/清除/点击外部结束编辑；不会长期高亮 | 全局搜索只保留标题栏一处；搜索焦点只在编辑态出现；点击侧栏/空白必须失焦 |

### 3.2 次参考：桌面壳层与导航

| 参考 | 关键做法 | 对本项目的启示 |
|---|---|---|
| [Apple HIG Sidebars](https://developer.apple.com/design/human-interface-guidelines/sidebars) | 侧栏位于前置边缘；可折叠；最多两级层级；分组用 disclosure；避免把关键操作放底部 | 侧栏保持可折叠与两级结构；底部只放主题/设置等低频入口（F-071） |
| [Material 3 Navigation Rail](https://m3.material.io/components/navigation-rail/overview) | 3–8 个顶层目的地；窄屏用 rail，宽屏用 drawer；选中态明确 | 活动栏保持少量主目的地 + 底部工具；不再塞入重复入口 |
| [Microsoft NavigationView](https://learn.microsoft.com/en-us/windows/apps/design/controls/navigationview) | 左侧导航适合 5+ 项目；设置/账户固定在底部；导航项简短 | 设置移到底部符合桌面习惯；导航文案用名词，保持短 |
| [Obsidian UI](https://docs.obsidian.md/User+interface) | Ribbon + 左侧栏 + 编辑区 + 右侧面板 + 状态栏；命令面板；设置入口在底部 | 底部设置与命令面板的模式可借鉴；避免把管理器塞进侧栏 |
| [Figma UI3](https://help.figma.com/hc/en-us/articles/15297425105303-Explore-design-files) | 左右面板可收起；属性面板按选择上下文变化；工具栏下移让出内容空间 | 检查器按选择显示内容、可折叠；外围面板不应长期挤压主内容 |

### 3.3 局部参考：搜索、命令与键盘

| 参考 | 关键做法 | 对本项目的启示 |
|---|---|---|
| [Apple HIG Search Fields](https://developer.apple.com/design/human-interface-guidelines/search-fields) | 搜索框含搜索图标、清除按钮、占位文本；作用域栏/Token；单一主搜索位置；局部搜索要能看出作用域 | 全局搜索只保留一处；清除/Esc/失焦必须有；局部筛选明确标注作用域（F-072） |
| [Apple HIG Searching](https://developer.apple.com/design/human-interface-guidelines/searching) | 搜索重要时给主位置；尽量一个位置搜全部；清楚显示当前搜索范围 | 标题栏搜索是主入口；相似结果/标签页搜索是局部筛选，不另起全局入口 |
| [VS Code Search](https://code.visualstudio.com/docs/getstarted/userinterface) | 活动栏搜索是独立视图；`Ctrl+F` 查找当前视图；Esc 关闭并返回原焦点 | **仅作局部反例**：本项目采用图片管理器的单一搜索框 + 网格结果，不把活动栏搜索做成独立视图（F-072） |
| [Linear](https://linear.app/method) / Notion / Obsidian | 命令面板是独立浮层；Esc 关闭；键盘优先；与筛选搜索分开 | 命令面板与搜索不要共用一个入口或一个焦点状态 |

### 3.4 视觉与状态参考

| 参考 | 关键做法 | 对本项目的启示 |
|---|---|---|
| [Linear](https://linear.app/method) | 深色、克制 accent、键盘优先、每个界面一个主操作、动效服务于状态 | 暗色克制方向正确；用留白/层级而非装饰；动效只表达状态变化 |
| [GitHub Primer](https://primer.style/) | token 化颜色/间距/字号；组件状态规范；可访问性内建 | 先建 token 与状态矩阵，再改页面 |
| [Radix Primitives](https://www.radix-ui.com/primitives/docs/overview/introduction) | 无障碍交互原语；焦点/键盘/弹层行为明确 | 对话框、菜单、下拉的焦点与 Esc 规则参考其行为模型（不复制代码） |
| [IBM Carbon](https://carbondesignsystem.com/) | 数据密集界面；表格/筛选/空状态/加载模式完整 | 100k 图库的表格、筛选、批量操作可参考其密度与状态 |
| [WCAG 2.2](https://www.w3.org/TR/WCAG22/) | 文本对比 4.5:1、UI 组件 3:1、焦点可见、键盘可达、目标尺寸 | 作为验收项，不放到最后再做 |
| [Material 3 Color](https://m3.material.io/styles/color/overview) | 语义颜色角色；亮暗主题成对 | 颜色按语义命名，不在组件里写死 hex |

## 4. 设计原则（建议写入后续 UI 规范）

1. **内容优先**：主图库、查看器、检查器保持现有布局；控件让位内容，不增加常驻装饰。
2. **一个能力一个入口**：搜索、标签管理、主题、设置各一个主入口；局部筛选必须标注作用域。
3. **上下文侧栏**：侧栏内容跟随活动栏目的地；不在标签/设置/去重页继续显示资源管理器筛选。
4. **单一搜索**：全局搜索一个位置；相似结果/标签页的搜索是局部筛选，不另起全局入口。
5. **渐进披露**：高频动作直接可见；低频动作进菜单/更多；每屏只有一个主按钮。
6. **键盘优先**：高频路径可全键盘完成；焦点可见；Esc/Enter/点击外部有明确、可预测的语义。
7. **状态完整**：每个可交互组件都有 rest/hover/focus/pressed/selected/disabled/loading/error/success 状态；空/加载/失败有下一步。
8. **克制视觉**：暗色底、细边框、少量 accent；用表面层级、字重和留白建立层级；少用大圆角、大阴影、渐变。
9. **可访问与性能是验收项**：对比度、焦点、键盘、减少动效、100k 性能与内存都纳入完成定义。

## 5. 设计 token 草案

> 优先扩展现有 `vendor/ely-gpui-components` 的 `Theme`/`ControlSize`/`TextSize`/`IconSize`；Kanalib 只补业务语义 token，不另建第二套设计系统。

### 5.1 间距

| Token | 值 | 用途 |
|---|---|---|
| `space-0.5` | 2px | 图标与文字、紧凑行内 |
| `space-1` | 4px | 控件内部、图标按钮 |
| `space-1.5` | 6px | 行内分组、标签 |
| `space-2` | 8px | 标准内边距、行间距 |
| `space-3` | 12px | 卡片/面板内边距 |
| `space-4` | 16px | 区块间距、页面边距 |
| `space-6` | 24px | 大区块、空状态 |
| `space-8` | 32px | 页面级留白 |

规则：4px 为主节奏，2px 只用于紧凑图标/文字对齐；同一层级内不混用 5/7/9/10/14 等非 token 值。

### 5.2 字号与行高

| Token | 字号/行高 | 用途 |
|---|---|---|
| `text-meta` | 11 / 16 | 计数、快捷键、辅助说明 |
| `text-secondary` | 12 / 18 | 次级信息、标签、状态 |
| `text-body` | 13 / 20 | 主要 UI 文本、列表 |
| `text-title` | 15 / 22 | 面板标题、对话框标题 |
| `text-page` | 18 / 26 | 页面标题 |
| `text-display` | 22 / 30 | 设置/标签页大标题 |

字重只用 400 / 500 / 600；标题最多 600，避免全界面粗体。行高与字号绑定，不单独写 `line_height` 魔法数。

### 5.3 控件与图标

| Token | 值 | 用途 |
|---|---|---|
| `control-sm` | 24px | 紧凑工具条、表格行内 |
| `control-md` | 28px | 默认按钮/输入 |
| `control-lg` | 32px | 主操作、页面级按钮 |
| `icon-sm` | 16px | 行内、菜单 |
| `icon-md` | 18px | 默认工具条 |
| `icon-lg` | 20px | 活动栏/主操作 |
| `row-sm` | 28px | 紧凑列表 |
| `row-md` | 36px | 默认列表 |
| `row-lg` | 44px | 检查器/设置分组 |

### 5.4 圆角、边框、层级

| Token | 值 | 用途 |
|---|---|---|
| `radius-sm` | 4px | 标签、徽章、紧凑控件 |
| `radius-md` | 6px | 按钮、输入、菜单项 |
| `radius-lg` | 8px | 卡片、面板、对话框 |
| `border-hairline` | 1px | 分隔线、输入边框、焦点底 |
| `surface-0/1/2/3` | bg/surface/sunken/overlay | 用表面层级区分区域；阴影只用于浮层 |

规则：优先用 1px border + surface 层级，不用大阴影；选中态用 accent/selection，不用高饱和填充大面积。

### 5.5 颜色语义

- `bg` / `surface` / `sunken` / `overlay`：背景层级。
- `fg` / `muted` / `subtle`：文本层级；正文对比至少 4.5:1，弱化文本不低于 4.5:1（小字号）。
- `accent`：主操作/选中；一屏不要超过一个主 accent 区域。
- `selection` / `hover`：交互态；hover 不要比 selection 更抢眼。
- `danger` / `warning` / `success`：状态；必须同时有图标/文案，不只靠颜色。
- 不使用组件内硬编码 hex；亮暗主题使用同一语义 token。

### 5.6 动效

| Token | 值 | 用途 |
|---|---|---|
| `motion-micro` | 120ms | hover、按下、图标切换 |
| `motion-state` | 180ms | 选中、展开、页签 |
| `motion-overlay` | 240ms | 对话框、抽屉、toast |
| easing | ease-out | 进入/展开；退出可更快 |

尊重减少动效：位移/缩放降为即时或 1ms；不改状态语义。动效只表达“发生了什么”，不做装饰性循环。

## 6. 组件与状态规范

### 6.1 组件清单

按钮（primary/secondary/ghost/danger，sm/md/lg）、图标按钮、输入、搜索输入、下拉/选择、菜单、页签/分段控件、列表行、树/分组、卡片、徽章/计数、提示/tooltip、对话框、确认框、toast、空/加载/错误状态、底栏状态项。

### 6.2 必须覆盖的状态

| 状态 | 要求 |
|---|---|
| rest | 清晰的层级与对比度；图标按钮有 tooltip/aria-label |
| hover | 背景/边框变化，不动布局；不抢过 selection |
| focus | 键盘可见焦点环；鼠标点击不残留焦点环 |
| pressed | 明确按下反馈；不改变最终布局 |
| selected | 与 hover 可区分；列表行/页签/活动栏一致 |
| disabled | 降对比度 + 禁止交互；不伪装成可点击；tooltip 说明原因（可选） |
| loading | 保留布局尺寸，显示 spinner/skeleton；按钮显示进行中且不重复触发 |
| error | 就地说明 + 可重试；不只靠 toast |
| empty | 说明为什么为空 + 一个明确下一步（清除筛选/导入/重试） |

### 6.3 具体规范

- **按钮**：一屏一个 primary；图标+文字优先，纯图标必须 tooltip + aria-label；危险操作必须确认，且默认焦点不落在危险按钮。
- **输入/搜索**：label/placeholder/help/error 四层信息按需出现；搜索框带清除；Esc/失焦规则见第 7 节。
- **列表行**：hover/selected/focus 三态清晰；行内操作在 hover/focus 时出现，但键盘可到达；长文本 truncate + tooltip；计数右对齐。
- **菜单**：分组、快捷键提示、check/radio 状态、禁用原因；键盘上下/左右/Enter/Esc 完整。
- **对话框**：标题/正文/动作三段；Esc 关闭、点击遮罩关闭需按危险级别决定；焦点陷阱与返回焦点。
- **toast**：成功/信息可自动消失；错误保留更久或提供详情；不用于必须确认的操作。
- **空/加载/错误**：统一图标、标题、说明、主操作；加载用 skeleton 保持布局，不用整屏 spinner（除非首次启动）。
- **底栏**：只放状态与进度，不放导航；数字与单位统一；长任务可展开详情。

## 7. 搜索与键盘规范

### 7.1 搜索

- 全局搜索只在标题栏一个位置；活动栏搜索图标移除，不另做独立 Search 视图（与图片管理器的单一搜索框 + 网格结果一致）。
- `Ctrl+F` 聚焦并全选；`Esc`：有文本先清空并保留结果，再按退出编辑态；清除按钮清空并保持焦点。
- 点击侧栏、内容区、空白、标签页、设置页、对话框时搜索框失焦；命令面板/查看器打开时强制失焦。
- 局部筛选（相似结果、标签工作区）使用独立 placeholder/标题，明确“只筛当前视图”。
- 搜索历史下拉在失焦/Esc/提交后关闭；不抢占对话框焦点。

### 7.2 键盘与焦点

- Tab 顺序与视觉顺序一致；活动栏/侧栏/内容/检查器/底栏各自可预期。
- 焦点环只在键盘操作时明显；鼠标点击后不残留。
- Esc 按上下文分层：先关闭浮层/编辑态，再清选择，再退出视图；一次 Esc 只做一件事。
- Enter 用于主操作/提交；Space 用于切换/选择；避免与文本输入冲突。
- 所有图标按钮有可读名称；焦点在滚动容器内自动可见。

## 8. 分阶段实施

> 只表达依赖，不表达排期。每阶段结束都应有截图基线与回归清单。

### 阶段 0：审计与基线

- 截图：亮/暗、1024/1440/1920 宽度、125% DPI、280px 侧栏、空库/10k/100k、查看器/标签/设置/去重。
- 盘点现有颜色/间距/字号/圆角/图标尺寸；建立 token 映射表。
- 输出组件状态矩阵与缺口清单。

### 阶段 1：壳层与导航收敛

- F-071：设置移到主题下方，移除标题栏主题按钮。
- F-072：搜索入口与搜索态收敛。
- F-070：侧栏/标签入口与上下文收敛（按确认的方案 A/B/C）。
- 退出条件：重复入口消失；所有主界面导航可预测；无布局回归。

### 阶段 2：组件库统一

- 用 Ely token 扩展现有组件；不再新增一次性按钮/输入/菜单样式。
- 完成按钮、输入/搜索、菜单、页签、列表行、对话框、toast、空/加载/错误的状态矩阵。
- 退出条件：组件清单全部有状态示例；键盘/焦点/对比度检查通过。

### 阶段 3：外围页面重构

- 标签工作区、设置页、重复项页、命令面板、对话框、检查器分组。
- 主图库/查看器只做组件级微调，不重排。
- 退出条件：外围页面与主界面使用同一套 token/组件；无重复入口。

### 阶段 4：可访问性、动效与性能收口

- WCAG 2.2 AA、减少动效、键盘全路径、目标尺寸。
- 100k 首屏/滚动/搜索/切图性能与内存回归。
- 截图基线归档；视觉 QA 清单通过。

## 9. 验收与视觉 QA

### 功能

- [ ] 搜索、标签管理、主题、设置各只有一个主入口；所有入口文案一致。
- [ ] 搜索 Esc/清除/点击外部/命令面板切换行为符合第 7 节。
- [ ] 侧栏/标签/设置/去重页的上下文与返回路径明确。
- [ ] 组件状态完整；危险操作确认；加载中不重复触发。

### 视觉

- [ ] token 唯一来源；无新增硬编码颜色/字号/间距。
- [ ] 亮/暗主题层级一致；对比度达标；选中/hover/focus 可区分。
- [ ] 1024/1440/1920 宽度、125% DPI、280px 侧栏无溢出/遮挡/裁切。
- [ ] 空/加载/错误/长文本/多选/批量操作截图归档。

### 性能与安全

- [ ] 100k 图库首屏、滚动、搜索、切图不劣于既有基线；内存不增长。
- [ ] 不改变 schema、标签语义、搜索语法、缓存预算。
- [ ] 不复制 GPL 代码/品牌资产；新增依赖先过许可审查。

## 10. 风险与待确认

- **视觉方向**：继续暗色克制风（推荐）还是做更明显的品牌化？先统一再谈品牌。
- **重构边界**：主图库/查看器布局冻结；侧栏、标签、设置、去重允许结构级调整。若侧栏也只做视觉微调，F-070/F-073 范围需缩小。
- **组件策略**：优先扩展现有 Ely 组件（推荐），不另建第二套组件库；Kanalib 只保留业务组合层。
- **大模块冲突**：`app.rs`、`settings.rs` 体量大，重构应按模块切片、小步提交，避免并行改动同一文件。
- **验证成本**：没有截图基线就没有“变好”的证据；阶段 0 必须先完成基线。
- **GPUI 限制**：焦点、弹层、DPI、阴影/动画能力需以实际组件验证；不假设 Web CSS 行为。
- **范围蔓延**：F-073 是体系重构，不是把所有页面推倒重来；主布局冻结是硬边界。

## 11. 参考资料

### 图片 / 素材管理器（主参考）

- [Lightroom Classic — Keywords](https://helpx.adobe.com/lightroom-classic/help/keywords.html)
- [digiKam 9.0 release](https://www.digikam.org/news/2026-03-08-9.0.0_release_announcement/)
- [digiKam](https://apps.kde.org/digikam/)
- [Eagle](https://en.eagle.cool/)
- [Eagle — tags](https://cn.eagle.cool/blog/post/streamline-file-management-how-eagle-tags-boost-design-assets-organization)
- [PhotoPrism — Labels](https://docs.photoprism.app/user-guide/organize/labels/)
- [Immich — Tags](https://docs.immich.app/features/tags)
- [Capture One](https://www.captureone.com/)

### 桌面壳层 / 导航（次参考）

- [Apple HIG — Sidebars](https://developer.apple.com/design/human-interface-guidelines/sidebars)
- [Material 3 — Navigation Rail](https://m3.material.io/components/navigation-rail/overview)
- [Microsoft — NavigationView](https://learn.microsoft.com/en-us/windows/apps/design/controls/navigationview)
- [Microsoft — Command bar](https://learn.microsoft.com/en-us/windows/apps/design/controls/command-bar)
- [Obsidian — User interface](https://docs.obsidian.md/User+interface)
- [Figma — Explore design files](https://help.figma.com/hc/en-us/articles/15297425105303-Explore-design-files)
- [Windows File Explorer](https://support.microsoft.com/en-US/Windows/Experience/FileExplorer/file-explorer-in-windows)

### 搜索 / 命令 / 键盘（局部参考）

- [Apple HIG — Search fields](https://developer.apple.com/design/human-interface-guidelines/search-fields)
- [Apple HIG — Searching](https://developer.apple.com/design/human-interface-guidelines/searching)
- [Material 3 — Search](https://m3.material.io/components/search/overview)
- [Linear — Method](https://linear.app/method)
- [VS Code User Interface](https://code.visualstudio.com/docs/getstarted/userinterface) — 仅局部参考命令面板/键盘/菜单行为，不作为整体布局、视觉或搜索视图目标
- [VS Code Keyboard Shortcuts](https://code.visualstudio.com/docs/getstarted/keybindings) — 仅参考快捷键习惯

### 视觉 / 设计系统 / 可访问性

- [GitHub Primer](https://primer.style/)
- [Radix Primitives](https://www.radix-ui.com/primitives/docs/overview/introduction)
- [IBM Carbon](https://carbondesignsystem.com/)
- [Material 3 — Color](https://m3.material.io/styles/color/overview)
- [Apple HIG — Color](https://developer.apple.com/design/human-interface-guidelines/color)
- [WCAG 2.2](https://www.w3.org/TR/WCAG22/)

### 本地约束

- [constraints.md](constraints.md)：性能、缓存、许可、UI 固定规格。
- [browse.md](pending/browse.md)：F-069/F-070/F-071/F-072/F-073 的验收与风险。
- [component-research.md](component-research.md)：非 UI 现成组件调研，避免重复引入。
