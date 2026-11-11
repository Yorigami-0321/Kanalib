# 图片墙视觉与信息架构改版（v2 设计任务书）

> 上级：[cloud-photo-wall.md](../../backlog/cloud-photo-wall.md) · 契约：[photo-wall-contract.md](../../backlog/photo-wall-contract.md)
>
> **状态：已实施（2026-11-09）。** 本文是 web/ 改版的唯一视觉依据；代码落点见 §10，验收截图 `web/screenshots/v2-*.png`（本地，已 gitignore）。

## 1. 拍板记录

2026-11-08 用户否掉 v1 的顶部工具条，要求：
1. 顶部横排「Kanabli 图片墙 · 已加载 N / 共 M + 文件名/标签/收藏/分级/星级/分组/缩放/适应屏幕」**不要**；
2. 改**左侧竖向边栏**做唯一导航；
3. **不要中栏**，边栏之外整屏就是墙；
4. 视觉要现代化，脱离工作台的工具感；
5. 详情页按**展示型网页**重做。

2026-11-09 用户给出正式风格参考：[chthollyphile/folia-major](https://github.com/chthollyphile/folia-major)——「就参考这个音乐播放器的风格」。

已稀疏克隆到 `.kanalib/refs/folia`（只取 `src/`，`.kanalib/` 已 gitignore，不入库）。以下取值全部来自仓库源码实测，不是目测。

## 2. 色板与表层

取自 `src/index.css` 与 `src/components/wall/wall.css`。

| Token | 值 | 用途 |
|---|---|---|
| `--bg` | `#070707` | 整页底 |
| `--bg-soft` | `#09090b` | 边栏与浮层底 |
| `--ink` | `#f7f4ee` | 主文字（暖白，不是纯白） |
| `--ink-muted` | `#a1a1aa` | 次级文字 |
| `--accent` | `#fd5c47` | 珊瑚红：选中、焦点环、强调 |
| `--accent-2` | `#6a48ff` | 紫罗兰：第二强调色 |

- **底不是纯色**：`radial-gradient(circle at 14% 12%, accent 16%, transparent 29%)` 与 `radial-gradient(circle at 86% 82%, accent-2 20%, transparent 32%)` 叠在 `--bg` 上——墙因此"有光"，不是死黑一片。
- **噪点**：全屏一层 SVG `feTurbulence`（`baseFrequency .92`，2 octaves），`opacity .13`、`mix-blend-mode: soft-light`，压掉大图的塑料感。
- **暗角**（可选）：`radial-gradient(ellipse at center, transparent 78%, rgb(0 0 0 / 52%) 100%)`，只铺在墙层、chrome 之下。
- **滚动条**：8px，track `#18181b`、thumb `#3f3f46`、hover `#52525b`、圆角 4px。

## 3. 墙的几何

Folia 的墙不是 waterfall，而是**固定格子 + 跨格贴片**（`wall/layout.ts` + `blockTemplates.ts`）：基准 12 列 × 8 行一个 block，一个 block 恰好被 12 个贴片铺满，跨度只取 2/3/4/6 的组合（2x2、2x3、3x2、4x3、4x4、6x2、6x3、6x4…）。贴片之间只有 gap，没有内 padding，所以墙永远密不透风、边缘对齐。

**本项目落法**：CSS grid——
- 列数：桌面 `12`、≤1200px `8`、≤760px `6`；
- `gap: 6px`（墙层左右留白 10px）；行高 = 方形的列宽（`aspect-ratio: 1` 的基准格）；
- 每张图按 `ratio = w / h` 求跨度：

```text
C = ratio >= 1.6 ? 4 : 3
R = clamp(round(C / ratio), 2, 8)
```

- `grid-auto-flow: dense` 让后续小图补洞，保持无空白；
- 加载由 `IntersectionObserver` 监听墙尾哨兵触发（取代 infinitegrid 的 onRequestAppend），顺序追加分片。

## 4. 贴片（WallCard）

- 无边框、无浅灰底：`overflow: hidden`、`border-radius: 3px`、`object-fit: cover`、`contain: layout paint`。
- 默认中性；hover：`filter: saturate(1.08) brightness(1.05)`、`translateY(-1px)`、`z-index` 抬起，过渡 600ms（Folia 的慢过渡）。
- 底部遮罩保文字可读：`linear-gradient(180deg, rgb(0 0 0 / 5%) 34%, rgb(0 0 0 / 24%) 56%, rgb(0 0 0 / 92%) 100%)`，hover/聚焦时整体显影。
- 叠加信息（默认极简）：左下角文件名（单行省略）+ 右侧小徽章（♥ 收藏、★ 星级、R 分级），字号 11px、字重 500、`text-shadow: 0 1px 2px rgb(0 0 0 / 60%)`。
- 选中/键盘焦点：2px `--accent` 内描边 + 内发光 `inset 0 0 15px -7px`，不改变布局。

## 5. 左侧边栏（Sidebar）

- 收起 76px / 展开 232px；底 `--bg-soft`，无右边框，只用 `--ink` 5% 的一条极淡分隔。
- 顶部：品牌字「Kanabli」细字重 + 字距 0.14em；下方一行「图片墙」12px `--ink-muted`。
- 中部入口（竖排图标 + 文字）：**搜索、标签、收藏、分级、分组、排序、缩放**。
- 底部：「关于」+ 展开/收起按钮。
- 选中项 = 圆角胶囊（`border-radius: 14px`），底色 `--ink` 8%、文字 `--accent`；未选中 55% 透明，hover 100%。
- 面板从边栏右侧浮出（`left: calc(rail + 12px)`）：`background: color-mix(in srgb, var(--bg) 92%, var(--ink))`、`backdrop-filter: blur(18px)`、圆角 14px、无边框、`box-shadow: 0 24px 64px rgb(0 0 0 / 55%)`，进入 140ms 位移 + 淡入。
- 面板内容：标签（搜索框 + 54 项计数列表，可多选）、收藏（开关）、分级（全部/G/S/Q/E）、分组（全部 + 分组列表）、排序（发布时间倒序/文件名/星级）、缩放（进入/退出、适应屏幕）。

## 6. 排版

- 字体栈：`Inter, "Segoe UI", var(--cjk)`；`--cjk: "Noto Sans CJK SC", "Source Han Sans SC", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei"`。
- 正文 13px / 1.5；列表项 13px；面板小标题 11px、`letter-spacing: .08em`、`text-transform: uppercase`；详情页标题 30px / 字重 500。
- 数字统一 `font-variant-numeric: tabular-nums`。

## 7. 动效

- 曲线统一 `cubic-bezier(0.22, 1, 0.36, 1)`。
- 时长：hover 180ms、面板 140ms、贴片显影 600ms、路由切换 350ms。
- 只动 `opacity` / `transform` / `filter`，不动布局属性。
- `prefers-reduced-motion: reduce` 时关掉位移与噪点动画。

## 8. 详情页（展示型）

- 布局：主图占左 62%（`max-height: 76vh`、`object-fit: contain`、无边框、无棋盘格），右 38% 信息列；≤1100px 改上图下文。
- 顶部只有一枚返回图标 + 右对齐序号（`0013 / 9899`），没有标题栏、没有工具栏。
- 信息列：作品名（30px/500）→ 一行元数据（`3200 × 2133 · 2026-10-24 · G`）→ 细分割线 → 标签 chips（点击回流到墙的筛选）→ 原图入口（有才显示）。
- 底部「相似」横向条：`scroll-snap-type: x mandatory`，每张 120px 方块，hover 提亮，键盘 ← → 切换。
- 键盘：`Esc` 返回墙；`←`/`→` 在相邻作品间移动（路由 `#/a/<id>`）。
- Lightbox（单击）保持 PhotoSwipe，但按新色板换皮：深底、无边框、计数器右下、工具条透明。

## 9. 不采用 Folia 的部分

- 不用它的 Electron 壳、Tailwind 工程链、i18n/设置体系；本项目仍是 Vite + 手写 CSS token。
- 不搬它的「二维自由画布 + 相机平移」：我们的墙是纵向无限滚动。
- 不引入播放器、歌词、悬浮播放条等音乐专属形态。

## 10. 改造落点（本次实施）

| 文件 | 处理 |
|---|---|
| `web/src/styles.css` | 重写：token + 噪点/光晕层 + 边栏 + 墙 + 详情 + lightbox 换皮 |
| `web/src/components/Toolbar.tsx` | **删除**，拆成 `Sidebar.tsx`（含各筛选面板） |
| `web/src/App.tsx` | 改成 `grid: 边栏 1fr` 外壳；底栏薄条；快捷键迁到边栏 |
| `web/src/components/Wall.tsx` | 弃用 `MasonryInfiniteGrid`，改 CSS grid + IntersectionObserver |
| `web/src/components/WallCard.tsx` | 按 §4 重画（跨度由布局算，卡片只负责渲染） |
| `web/src/components/Detail.tsx` | 按 §8 重排为展示页 |
| `web/src/components/Lightbox.ts` | 换皮（深底、细计数器） |
| `web/package.json` | 去掉 `@egjs/react-infinitegrid` |
| 不动 | `StaticSource`、`source/types`、`state/filters`、`state/useWall`、契约、发布器、路由形状 |

## 11. 验收清单

1. 首屏无顶部横条；左侧边栏可收起/展开，键盘可达。
2. 墙缝隙 6px、无卡片描边，连续滚动 2000 张无跳动、无横向滚动条。
3. hover 叠加信息出现/消失不引起布局位移。
4. 详情页在 1440×900 与 1024 宽下都不出横向滚动，主图 ≤76vh。
5. 筛选从边栏发起，命中数正确，清除后回到全量视图。
6. 正文对背景对比度 ≥4.5:1，chip 文字 ≥4.5:1。
7. 移动端 390px：边栏收为图标条，墙 6 列。
8. `npx tsc --noEmit` 与 `npm run build` 通过；headless 截图确认首屏与详情页。
