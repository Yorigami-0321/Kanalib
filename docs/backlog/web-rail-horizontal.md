# 网页导航横向化规格（v1 草案）

> 上级：[pixiv-integration.md](pixiv-integration.md) · 视觉基线：[photo-wall-v3.md](photo-wall-v3.md) · 契约：[photo-wall-contract.md](photo-wall-contract.md)
>
> 2026-11-21 用户指示：把网页端侧边栏上的 tab 改成和 Pixiv-Archive 类似的**文字横向分布**布局。
>
> 状态：**已实现（2026-11-23 复核）**。`Sidebar.tsx` 已落地 208px / 60px 折叠、`localStorage`、`[`/`]` 与窄屏底部条；剩窄屏/折叠态视觉复验。

## 0. 一句话

把 120px 竖排「缝」换成 **208px 横向文字导航**：图标在左、文字横排，分组标题横排；折叠后 60px 纯图标；窄屏退化为横向滚动条。导航仍是**浮层**，不推挤画布。

## 1. 为什么不是照抄

参考项目 `Sidebar.tsx` 是 224px 可折叠侧栏 + 图标/横排文字 + 操作区 + 底部工具。可直接借鉴的是**信息层级**（导航 / 操作 / 底部状态三段），不能照抄的是：

- 它是 Tauri 应用内的固定布局，会推挤内容；本项目硬规则 2 要求「导航不推挤内容」。
- 它用 Tailwind + lucide；本项目是手写 CSS + 内联 SVG，且已有一套「缝」的对比度/动效基线。
- 它的六个页面（画廊/作者/标签/收藏夹/收藏订阅/本地作品）与当前路由不完全一致。

所以：**保留浮层与玻璃，改内部排版**。

## 2. 结构

```text
┌──────────────────────────┐
│ ●  Kanalib          [«]  │  头部：强调点 + 字标 + 折叠
├──────────────────────────┤
│ 浏览                     │  分组标题（11px / .18em / 62%）
│ [⌂] 首页                 │
│ [▣] 作品                 │  ← 新增（P2 后出现）
│ [◈] 标签                 │
│ [♡] 收藏                 │
│ [◷] 分级                 │
│ [▤] 分组                 │
├──────────────────────────┤
│ 整理                     │
│ [◫] 作者                 │  ← 新增（P3 后出现）
│ [▦] 收藏夹               │  ← 新增（P3 后出现）
│ [☰] 订阅                 │  ← 新增（P4 后出现）
├──────────────────────────┤
│ 工具                     │
│ [⌕] 搜索                 │  打开浮层
│ [⇅] 排序                 │  打开浮层
├──────────────────────────┤
│ [−][+]  [⤢] [⌖]          │  画布/详情操作组
├──────────────────────────┤
│ 1,234 / 9,999            │  计数（横排、等宽数字）
│ [◐] 单色      [i] 关于    │
└──────────────────────────┘
```

### 2.1 尺寸

| token | 展开 | 折叠 |
|---|---|---|
| `--rail-w` | `208px` | `60px` |
| 左右内边距 | `12px` | `0` |
| 头部高 | `52px` | `52px` |
| 导航项高 | `34px` | `34px` |
| 导航项圆角 | `8px` | `8px` |
| 图标 | `16px` | `16px` |
| 图标-文字间距 | `10px` | — |
| 文字 | `13px / 500 / 0` | — |
| 分组标题 | `11px / 500 / .14em` | 隐藏（用 1px 分隔线替代） |
| 分组间距 | `14px` | `8px` |

- `--seam-w` **保留为别名**（`--seam-w: var(--rail-w)`），避免 `Wall.tsx`、`inspector.css`、`detail.css` 一次性全改；P1 只改定义处。
- **DOM 类名也要保留**：`Sidebar.tsx` 自己的外部点击判断用 `target.closest('.seam')`，`App.tsx` 用 `#seam-panel-trigger-search` 打开搜索，现有 `wall_*` 探针也按 `.seam` 取元素；`Wall.tsx` 另读 `--seam-w` 做缝区手势拦截。P1 只加 `.rail` 作为并列类（`className="seam rail"`），不要删除 `.seam`；P2 确认探针改完后才能移除。
- JS 侧 `layout.ts` 不再硬编码 120/64，改为 `getComputedStyle(document.documentElement).getPropertyValue('--rail-w')`，读不到再回退常量。
- **断点必须和现有 CSS 对齐**：现有 `styles.css` 在 `max-width: 900px` 就把 `--seam-w` 降到 64px、隐藏文字；`max-width: 760px` 已是底部条。P1 不要另造 1100px 断点，否则 901–1100 区间会出现「JS 认为展开、CSS 已折叠」。先按现有 900/760/560 三档实现，1100 档留给后续单独评估。

### 2.2 断点

| 宽度 | 行为 |
|---|---|
| `> 900` | 208px 展开；折叠可用（与现有 `max-width: 900px` 分界一致） |
| `761–900` | 60px 折叠态（纯图标）；`title`/`aria-label` 保留；悬停显示 160px 浮出标签（可选） |
| `≤760` | **底部横向导航条**（高 56px），横向滚动；文字保留，图标 14px；计数/单色/关于收进「更多」浮层 |
| `≤560` | 底部导航只留 5 个主入口，其余进「更多」；文字 10px |

### 2.3 与画布的关系

- 导航 `position: fixed; left: 0; top: 0; bottom: 0; z-index: 40`；画布仍铺满全宽，图垫在导航下（保留现有 76% 半透明玻璃）。
- `.stage { padding-left: var(--rail-w) }` 不变；`Wall.tsx` 的缝区命中拦截仍读 `--seam-w`（别名生效）。
- 折叠动画只改 `width`，`transition: width 200ms var(--ease)`；`prefers-reduced-motion` 下改为无动画。
- 折叠状态存 `localStorage['kanalib.rail.collapsed']`；默认展开。

## 3. 状态

### 3.1 选中

- 底色：`color-mix(in srgb, var(--ink) 9%, transparent)`；文字 `var(--ink)`；图标同色。
- 左侧 2px 强调色竖条：`top: 8px; bottom: 8px; left: 0`，`background: var(--accent)`，240ms 入场。
- 折叠态：选中只保留底色 + 2px 竖条，不显示文字。

### 3.2 悬停

- 底色 `color-mix(in srgb, var(--ink) 6%, transparent)`，160ms；不改变文字亮度以外的任何属性（硬规则 3）。
- 折叠态悬停：图标提亮到 `var(--ink)`，并浮出 160px 的标签气泡（`left: calc(100% + 8px)`，`pointer-events: none`）。

### 3.3 焦点

- `:focus-visible`：`outline: 2px solid var(--accent); outline-offset: -2px;`（内描边，硬规则 6）。
- 折叠/展开按钮、导航项、操作项、底部按钮全部可聚焦；顺序 = DOM 顺序。

### 3.4 禁用/进行中

- 进行中的操作项（同步/订阅）显示 14px 旋转指示器，`aria-busy="true"`，仍可聚焦但不重复触发。
- 禁用只降透明度到 0.4，不加删除线、不改字号。

### 3.5 计数

- `1,234 / 9,999` 横排，`font-variant-numeric: tabular-nums`；颜色 `rgba(247,244,238,.62)`。
- 折叠态隐藏；底部导航条隐藏。

## 4. 对比度与可读性

- 沿用现有 `--surface` 76% + `blur(10px)`；文字 `--ink-soft` 提到 `72%`（现有 seam 已如此）。
- 目标：正文 ≥ 7:1，次级 ≥ 4.5:1，分组标题 ≥ 3:1（大字/小字按 WCAG）。
- 分组标题用 `letter-spacing: .14em` + `text-transform: none`（中文不加字距过大）；英文 `uppercase`。
- 不使用 `writing-mode`；删除所有 `vertical-rl` 规则。

## 5. 动效

| 事件 | 动效 |
|---|---|
| 展开/折叠 | `width 200ms var(--ease)`；文字 `opacity 120ms`；折叠时不位移图标 |
| 切换页签 | 选中竖条 `scaleY(0→1)` 240ms；底色 160ms |
| 打开面板 | 现有 `panel-in` 160ms；面板 `left: calc(var(--rail-w) + 12px)` |
| 底部导航切换 | 无位移动画，仅底色/文字 160ms |
| `prefers-reduced-motion` | 全部改为 0ms，仅保留状态色 |

## 6. 交互与键盘

- 导航项是 `<button>`，不是 `<a>`；路由跳转仍由 `onClick` 调 `navigate`（保持现有 hash 路由与 `useRoute`）。
- 键盘：`Tab` 遍历、`Enter/Space` 激活；`[` 折叠、`]` 展开（新增全局快捷键，避开输入框）。
- 折叠后 `title` 与 `aria-label` 必须同时存在；不使用 `aria-hidden` 隐藏文字（用 CSS `display:none` + `aria-label`）。
- 面板触发按钮保留 `aria-expanded`/`aria-controls`；关闭后焦点回到触发按钮（现有逻辑）。

## 7. 与现有代码的改动面

| 文件 | 改动 |
|---|---|
| `web/src/components/Sidebar.tsx` | 结构重排：头部 / 分组导航 / 操作组 / 底部；新增 `railCollapsed` 状态与持久化；新增作者/收藏夹/订阅入口（按 `capabilities` 显示）；根节点 `className="seam rail"`，内部 `.seam__*` 类名先保留 |
| `web/src/components/sidebar.css` | 用 `.rail` 系列新增布局；`.seam` 继续作为兼容类（P1 过渡），P2 探针改完再删 |
| `web/src/styles.css` | `:root` 的 `--seam-w: 120px` → `--rail-w: 208px; --seam-w: var(--rail-w)`；`@media (max-width: 900px)` 里改成只覆盖 `--rail-w: 60px`（不要同时写 `--seam-w: 64px`，否则别名被写死）；`.panel { left: calc(var(--rail-w) + 12px) }` |
| `web/src/lib/layout.ts` | `SEAM_WIDE/SEAM_NARROW` → `RAIL_WIDE=208`/`RAIL_COLLAPSED=60`；`readLayoutMetrics` 读 computed style |
| `web/src/App.tsx` | `<Sidebar>` 新增 `collapsed/onToggleCollapsed`、`capabilities`；`layout.seam` 语义不变 |
| `web/src/components/Wall.tsx` | 不改逻辑；确认 `--seam-w` 别名仍能读到宽度 |
| `web/src/components/detail.css` / `inspector.css` | 不改（继续读 `--seam-w`） |

## 8. 验收

### 8.1 视觉

- 1440 / 1280 / 1024 / 900 / 760 / 390 六档截图；无横向溢出、无文字截断。
- 展开 208px、折叠 60px、底部条 56px 三态像素测量与规格一致。
- 选中/悬停/焦点/禁用四态各 1 张；对比度脚本（可复用 `scripts/check_theme_contrast.py` 的思路）通过。
- 折叠后仅图标，`title` 可见；展开后文字横排，无 `vertical-rl`。
- 900px 边界两侧实测 `getComputedStyle` 的 `--rail-w`/`--seam-w` 同步变化；不允许出现 rail 208 / seam 64 的错位。

### 8.2 行为

- 点击五个主入口路由正确；刷新后保持；浏览器前进/后退高亮正确。
- 折叠状态刷新后保持；`[`/`]` 可切换；面板打开时 `left` 跟随宽度。
- 窄屏底部条可横向滚动；`≤560` 主入口 5 个，「更多」可开。
- 现有 `wall_shots.mjs`、`wall_probe_tag360.mjs`、`wall_probe_inspector_sheet.mjs`、`wall_probe_edge.mjs` 全绿。

### 8.3 回归

- `Wall.tsx` 的缝区拦截仍生效（拖拽/滚轮在导航上不穿透）。
- 详情页检查器 `left` 正确；`has-inspector` 时 stage 让位不变。
- 单色预览 `N`、留白 `M`、放映 `P` 等快捷键不受影响。

## 9. 不做

- 不做可拖拽调宽（参考项目也没有；本项目桌面端已有宽度记忆，网页端先固定两档）。
- 不做图标库替换（继续内联 SVG）。
- 不做第二套主题（沿用现有 token）。
- 不做导航内嵌搜索框（搜索仍是浮层，避免在 208px 里挤输入框）。

## 10. 迁移顺序

1. CSS token + `.rail` 骨架（不改 JSX）→ 截图确认浮层/宽度。
2. `Sidebar.tsx` 根节点加 `rail` 并列类，结构重排 + 折叠状态；`.seam` 与 `#seam-panel-trigger-*` 保留。
3. 新增入口（先按 `capabilities=false` 隐藏）。
4. 改现有探针选择器（`.seam` → `.rail`，或同时匹配），再删 `.seam` 与旧竖排规则。
5. 六档探针 + 现有探针回归。

> 回退点：第 2 步完成后先跑 `wall_probe_state` / `wall_probe_tag360` / `wall_probe_inspector_sheet` / `wall_probe_edge`；全绿才继续第 3 步。折叠宽度用 `--rail-w`，但 `Wall.tsx` 仍读 `--seam-w`，两者必须由同一变量派生，不能各写一份。
