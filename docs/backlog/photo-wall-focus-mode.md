# 图片墙 v4：单击聚焦、触控板手势与侧栏信息（调研与设计）

> 2026-11-10 · 用户反馈驱动的下一轮 · **本文件只做调研与设计，代码未动**
>
> 关联：[photo-wall-v3.md](photo-wall-v3.md)（现行交互标准）· [photo-wall-status.md](photo-wall-status.md)（§4.9 的本轮实测）· [photo-wall-contract.md](photo-wall-contract.md)（数据契约）· `.kanalib/folia-ui-spec.md`（Folia 实测规格）

## 0. 结论先行

1. **单击不是“选中”，而是“聚焦卡”**：卡片在原位放大到明显大于邻卡、被相机移到内容区中心，其余卡片压暗；不跳路由、不开全屏详情。双击/Enter 才进详情。
2. **详情页不再有底部信息层**：截图 1 里的标题/元数据/相似条从画面上删掉，全部搬进左侧信息栏；详情画面只留图。
3. **左侧信息栏是“300px 检查器”**：120px 缝保留导航；聚焦/详情时在缝右侧展开 300px 信息面板（Folia 的 full info seam 同量级），相机给信息栏让位。
4. **触控板双指横滑必须有响应**：墙处理 `WheelEvent.deltaX` 做二维平移；详情在 1× 时用横向 wheel 累积切换上/下一张，放大后改为平移图；`ctrlKey/metaKey` 的 wheel 才当捏合缩放。
5. **不做新的第三方重型依赖**：焦点卡用现有相机 + CSS transform；信息栏用现有 React 组件；触控板用原生 wheel 归一化。

## 1. 用户反馈与现状（代码级）

| # | 反馈 | 现状（代码/浏览器） | 结论 |
|---|---|---|---|
| 1 | 单击应把图片放大到比其他图大、占据视觉中心，但不是全屏详情 | `Wall.tsx` 的 `onPointerUp`（约 746–757 行）单击只 `setActiveId`，卡片只出现 2px 内描边；没有缩放、没有相机居中 | 缺少“聚焦卡”状态 |
| 2 | 电脑触控板双指左右划没反应，只能上下滚动 | `Wall.tsx` 的原生 wheel（约 776–805 行）只读 `deltaY`，`deltaX` 完全没用；详情 wheel 只处理 `ctrlKey/metaKey` 缩放 | 横向 wheel 被丢弃 |
| 3 | 截图 1 内容没删，也没做到侧栏 | 详情仍在 `Detail.tsx` 的 `.view__info` 里渲染标题/元数据/标签/相似条；左侧只有返回/翻页/序号 | 信息层位置错误 |
| 4 | 侧边栏信息不全、不美观 | 缝里只有页签/搜索/排序/关于；没有当前图信息、没有标签分组、没有相似图、没有操作 | 需要信息检查器 |
| 5 | 单击放大 ≠ 详情全屏 | 目前单击选中、双击详情；中间缺“聚焦”一档 | 需要三档：浏览 → 聚焦 → 详情 |

补充：用户截图 1 是详情页底部那条（标题 `synthetic_...`、尺寸/日期/分级、右侧“相似”缩略图）。它对应的 DOM 是 `.view__info`，不是右键概要 `.tile__info`。右键概要属于另一条已拍板功能，本轮先不动，见 §9 的开放问题。

## 2. 调研：同类产品怎么做

### 2.1 “原位放大”而不是弹窗

| 参考 | 做法 | 可借的点 | 不照搬 |
|---|---|---|---|
| **Folia / Bravais**（本地 `.kanalib/folia-ui-spec.md`） | 点开一张，块内 12 张就地重排，展开卡占 6×6 格；展开卡标题 48–82px；层序：磁贴 0 → 噪点 2 → 暗角 3 → 展开卡 20 | “点开=在原空间里变大”；相机让位；展开卡与普通卡同一套视觉语言 | 我们的无限格子没有固定 12×8 块，重排整格会把远邻也牵动；先做“原位缩放 + 相机居中”，不做整墙重排 |
| **macOS Finder 画廊视图 + 预览栏**（Apple 官方支持） | 网格里选中文件，右侧 Preview 栏显示大预览与完整元数据；`Shift+Cmd+P` 开关预览栏；双击才用默认应用打开 | “选择 + 检查器”是成熟范式；信息栏可开关；元数据完整 | Finder 是列表/画廊，不是无限画布；相机让位要另做 |
| **Figma / Miro 画布检查器** | 单击选中，右侧属性面板显示所选对象；双击进入对象编辑；画布不跳页 | 选择驱动的信息栏；画布保持上下文 | Figma 的属性是“可编辑”的；我们只读，信息栏以展示为主 |
| **GSAP Infinite Draggable Gallery** | 卡片在靠近视口中心时变大（鱼眼式），拖动即浏览 | “中心权重”让视线有落点 | 它按位置连续缩放；我们按点击离散聚焦，避免拖动时不断变形 |
| **Acebuilder “Expandable Card On Click”** | 点击卡片弹全屏 modal 读内容 | 低成本实现 | 正是用户明确不要的“全屏弹窗”，不采用 |

**结论**：采用 **“选择 + 原位聚焦 + 检查器”** 三段式。单击不改变路由，只在画布内把卡片放大并居中；双击才进入全屏详情。这样既保留空间感，又有明确的信息落点。

### 2.2 触控板双指与捏合

| 来源 | 关键结论 |
|---|---|
| MDN `WheelEvent` | `deltaX`/`deltaY` 是横向/纵向滚动量；`deltaMode`：0=像素、1=行、2=页。触控板通常是像素，鼠标滚轮常是行 |
| Tiger Abrodi《Trackpad Pinch vs Two-Finger Scroll》 | 浏览器对“双指滚动”和“捏合”都只发 `wheel`；**用 `ctrlKey` 区分捏合**，不要用小 delta 猜；缩放时把 `deltaY` 夹到约 ±10 防过冲 |
| Excalidraw 手势优化（中文技术分析） | 先按 `deltaMode` 把 delta 归一化为像素（行 × 15–20），再按设备调灵敏度；`ctrlKey/metaKey` 走缩放 |
| macOS 触控板手势资料 | 双指左右轻扫默认触发浏览器前进/后退；系统设置“在页面之间轻扫”或 Chrome 的 `AppleEnableSwipeNavigateWithScrolls` 可关；网页 JS 不能保证拦掉系统级手势 |
| Windows Precision Touchpad 资料 | 双指横滑会以 `deltaX` 进入页面；没有 macOS 那种浏览器历史轻扫，处理 `deltaX` 即可 |

**结论**：
- 墙：`deltaX`/`deltaY` 都进相机平移；`ctrlKey/metaKey` 才缩放；Shift+wheel 作为鼠标横向兜底。
- 详情：1× 时横向 wheel 累积到阈值就切上/下一张；放大时横向/纵向 wheel 都用于平移图片；捏合仍缩放。
- macOS 若系统把横滑吞成前进/后退，页面无法可靠阻止；在文档和验收里明确“需要关闭系统轻扫手势”，并提供 `?gestures=1` 的诊断覆盖层（显示最近一次 `deltaX/deltaY/deltaMode/ctrlKey`），方便在用户机器上确认真实输入。

### 2.3 信息侧栏

| 参考 | 可借的点 |
|---|---|
| Finder Preview 栏 | 信息项完整、分组、可开关；预览图与元数据同栏 |
| Folia full info seam | 完整信息条 **300px**；相机让位 **0.42s**；纸面 + 大投影、无边框；导航缝 120px 是首页态 |
| Figma 属性面板 | “选中什么显示什么”；分区、层级清楚；未激活项弱化 |
| Lightroom / Photos 检查器 | 图像居中、信息在侧栏；标签/相似/操作分区；滚动而不是把图挤没 |

**结论**：采用 **120px 缝 + 300px 信息检查器**（信息面板贴在缝右侧，浮在画布上；相机按“缝 + 信息栏”之后的剩余区域居中）。备选是“缝本身从 120 展开到 300”，见 §9。

## 3. 推荐交互模型（三档）

```
浏览（默认）
  单击卡片 ─────────────► 聚焦（spotlight）
  双击卡片 / Enter ─────► 详情（detail）
  Esc / 点空白 ─────────► 回到浏览

聚焦
  点另一张卡 ───────────► 换聚焦
  双击 / Enter ─────────► 详情
  点“详情”按钮 ─────────► 详情
  Esc / 点空白 / 点关闭 ─► 浏览

详情
  ←/→、横滑、横向 wheel ─► 上/下一张
  Esc ─────────────────► 返回聚焦（若从聚焦进）或列表
```

- **单击**：不跳路由。卡片放大、相机居中、信息栏展开；其他卡压暗。
- **双击**：进入详情（保留现有 `#/d/<id>` 路由与共享元素展开动画）。
- **聚焦卡再次单击**：不自动进详情（避免和“单击=聚焦”语义冲突）；详情走双击/Enter/侧栏按钮。
- **触屏**：单击 = 聚焦（与鼠标一致）；详情由信息栏“打开详情”按钮兜底；长按概要保持现状（待确认是否保留，见 §9）。

## 4. 聚焦卡几何与视觉

| 项 | 规格 |
|---|---|
| 目标尺寸 | 内容区（视口减缝、减信息栏）的 **宽度 48–60% / 高度 62–76%**，保持原图比例；取两者中更紧的约束 |
| 缩放 | `scale = clamp(min(targetW / (rect.w × zoom), targetH / (rect.h × zoom)), 1.35, 2.8)` |
| 相机 | 把卡片中心移到内容区中心；无限/有限世界都走同一路径；有限世界靠边时允许夹取，不强行越界 |
| 动画 | 相机 320–420ms（Folia 相机让位 0.42s）；卡片 `transform` 300ms；`cubic-bezier(.2,.7,.25,1)`；`prefers-reduced-motion` 直接落位 |
| 层序 | 聚焦卡 `z-index` 高于普通卡；缝/信息栏在其上；**不新增全屏 scrim**，用其他卡的 `filter: brightness(.55) saturate(.85)` 压暗 |
| 图片档位 | 聚焦卡改用 `detailSources`（原图 → t2048 → t1024 → t512）；普通卡继续 t256/t512 srcset；聚焦切换时重置失败档索引 |
| 文字 | 聚焦卡保留标题；角标/分辨率行可隐藏或弱化；完整信息在侧栏，不在卡上重复 |
| 关闭 | Esc、点空白、点信息栏关闭；点另一张卡则换聚焦 |
| 滚动/拖拽 | 聚焦期间仍可拖动画布（卡片跟着世界走）；相机不再自动追卡；用户主动拖走即视为“退出自动居中”，但保持聚焦尺寸 |

**不做**：不重排邻卡（无限格子重排会牵动不可见格）；不弹 modal；不改变 URL。

## 5. 侧栏信息检查器（移除截图 1 的底部条）

### 5.1 位置与宽度

- 桌面（>900px）：缝 120px 保留；检查器宽 **300px**，从 `left: 120px` 开始，顶到底，浮在画布上；相机/详情图按 `120 + 300 = 420px` 的左侧占用让位。
- 窄屏（≤900px）：缝 64px；检查器改为 **底部 sheet**（最大高度 55vh，圆角 14px 顶部，拖拽把手），图上、信息下；避免 64+300 把 390 屏挤满。
- 详情页：检查器默认展开；`I` 或关闭按钮可收起，图区回到全宽。信息栏收起时保留一个小“信息”按钮（缝底部）。
- 墙聚焦：检查器自动展开；关闭检查器同时退出聚焦（或保留聚焦卡但收起信息？推荐关闭=退出聚焦，语义简单）。

### 5.2 内容结构（按重要性）

```
┌ 当前作品 ──────────────── [收起] ┐
│ 标题（2–3 行，20–24px）           │
│ 文件名（小字，可省略重复）         │
│ ─────────────────────────────── │
│ 尺寸      1120 × 1992（140:249）  │
│ 日期      2026-10-24 18:08        │
│ 分级      G 一般                  │
│ 星级      ★★★☆☆                 │
│ 分组      原创                    │
│ 标签      44 个                   │
│ ─────────────────────────────── │
│ 标签                              │
│   通用   animal_ears 1girl … +34  │
│   作品   indie_virtual_youtuber … │
│   元信息 highres                  │
│   分级   rating:g                 │
│   [全部 44 个标签]                │
│ ─────────────────────────────── │
│ 相似                              │
│   [缩略图] [缩略图] [缩略图]      │
│   [缩略图] [缩略图] [缩略图]      │
│ ─────────────────────────────── │
│ [打开详情]  [原图]                │
└──────────────────────────────────┘
```

- 元数据用两列 label/value；数字 tabular-nums；分隔线 `--hairline`。
- 标签沿用现有分组与折叠规则（§4.9）；检查器内单独滚动，标签展开不把相似条推走。
- 相似条用 3 列网格（约 72–80px），点击 = 聚焦该图（墙）或切到该图（详情）。
- 操作：聚焦态“打开详情”；详情态“返回列表”；有 `orig` 时“原图”；关闭/收起。
- 信息栏自身 `overflow-y:auto`；底部渐隐 mask；键盘焦点进入后 Esc 关闭并还原到触发卡。
- 材质：纸面 92% `--surface` + `0 24px 64px` 投影；**不用大面积 backdrop-filter**（Folia 300px 纸条每帧重算的成本）；信息栏是持续开启的，模糊只在细边/状态点。
- 视觉层级：标题 20–24/600；标签 11–12 小写距；元数据 12–13；分区标题 11/700/.14em；强调色只用于“当前项/链接/星级”，不做大面积填充。

### 5.3 详情页的改动

- 删除 `.view__info` 底部层（标题/元数据/标签/相似条），`Detail.tsx` 只保留图与手势。
- 图区 `left = var(--seam-w) + var(--inspector-w)`（详情检查器展开时）；收起时 `left = var(--seam-w)`；`object-fit: contain` 不变。
- 相似条、标签、原图链接全部进检查器；详情页不再有底部遮挡。
- 保留 `←/→/Esc`、横滑、缩放/平移、双击复位；新增横向 wheel 切换（§6）。
- 共享元素展开动画的落点仍按“图区”计算，不再按整屏；检查器里也可以“打开详情”。

## 6. 触控板/滚轮规格

### 6.1 归一化

```ts
function wheelToPixels(event: WheelEvent, viewportHeight: number) {
  const unit =
    event.deltaMode === WheelEvent.DOM_DELTA_LINE
      ? 16
      : event.deltaMode === WheelEvent.DOM_DELTA_PAGE
        ? viewportHeight
        : 1;
  return { dx: event.deltaX * unit, dy: event.deltaY * unit };
}
```

- 触控板像素模式 delta 可能只有 1–5；**不要用 >20 的阈值过滤**，否则横滑永远不触发。只忽略 `|dx| + |dy| < 0.5` 的抖动。
- 鼠标行模式 ×16（Excalidraw 经验 15–20）。
- 捏合：Chrome/Edge 在触控板捏合时给 `ctrlKey=true` 的 wheel；`metaKey` 也一并当缩放。缩放因子 `Math.exp(-deltaY * 0.0022)`，deltaY 夹到 ±10 防过冲。

### 6.2 墙

```ts
if (ctrlKey || metaKey) {
  // 以光标为锚缩放（现有逻辑）
} else {
  const { dx, dy } = wheelToPixels(e, size.h);
  // Shift+wheel：鼠标横向兜底
  const horizontal = e.shiftKey && Math.abs(dx) < 0.5 ? dy : dx;
  moveCamera({ ...cam, x: cam.x + horizontal, y: cam.y + dy });
}
```

- 现有 `deltaMode` 处理保留并扩展。
- `event.preventDefault()` 仍用非被动监听；`overscroll-behavior: none` 加在画布上。
- 墙不能依赖 `wheel` 被浏览器历史轻扫拦掉：macOS 需用户关闭“在页面之间轻扫”；Windows 没有这个问题。

### 6.3 详情

| 状态 | 横向 wheel | 纵向 wheel | Ctrl/⌘+wheel |
|---|---|---|---|
| 1× | 累积 `dx`；超过 ±80px 且 450ms 内未触发 → 上一张/下一张；带冷却 | 忽略（或信息栏收起时控制缩略图？不做事） | 缩放 |
| >1× | 平移图片 x | 平移图片 y | 缩放 |

- 累积器在 250ms 无新事件后归零；方向反转立刻归零，避免来回误触。
- 触控板连续横滑会持续发事件；冷却 450ms 防止一次手势连跳三张。
- 缩放状态下不切图，避免“想看图细节却翻页”。
- 移动端仍用 pointer/touch 横滑；wheel 只在精确指针设备上生效。
- 诊断：`?gestures=1` 时在角落显示最近一次 `deltaX/deltaY/deltaMode/ctrlKey`，不发请求、不写日志；用于确认用户机器上的真实事件。

## 7. 代码落点（实施时）

| 文件 | 改动 |
|---|---|
| `web/src/components/Wall.tsx` | 新增 `spotlight` 状态与 `onSpotlightChange`；wheel 支持 dx；相机 tween；聚焦卡缩放/居中；`infoInset` 参与内容区居中；点空白/Esc 关闭；重置/换页清理 |
| `web/src/components/WallCard.tsx` | `spotlight` prop：高分辨率 `detailSources`、`tile--spotlight` 类、隐藏/弱化角标；切换时重置失败档 |
| `web/src/components/Sidebar.tsx` | 新增信息检查器（或新组件 `Inspector.tsx`）；`info` prop；移动端底部 sheet；与搜索/排序面板的互斥/层序 |
| `web/src/components/Detail.tsx` | 删除 `.view__info`；媒体区按检查器宽度让位；横向 wheel 切图；保留缩放/横滑 |
| `web/src/App.tsx` | 提升 `spotlightItem` 状态；聚焦/详情共用信息数据；强调色：聚焦用本图、首页聚合；路由/焦点回收 |
| CSS | 聚焦卡、压暗、信息栏、底部 sheet、详情媒体让位；删除旧 `.view__info`/详情底部样式；`prefers-reduced-motion` |
| 测试 | `wall_shots` 增单击聚焦断言；新探针测 wheel dx、详情横向 wheel、检查器字段；1440/390 截图 |

数据契约不变：现有 item 字段（id/w/h/n/rt/st/fav/tags/ph/mtime/orig/sharp/group）足够；不需要新 schema。

## 8. 验收标准

1. **单击聚焦**：不改变 `location.hash`；聚焦卡中心与内容区中心误差 ≤2px；尺寸达到目标区间；其他卡可见但压暗；检查器出现并含标题/尺寸/日期/分级/标签/相似/操作。
2. **双击详情**：进入 `#/d/<id>`；检查器仍在；详情画面无底部信息条；`Esc` 回到聚焦/列表。
3. **触控板墙**：`Input.dispatchMouseEvent` 的 wheel 带 `deltaX=80` → 相机 x 增加约 80；`deltaY=80` → y 增加；Shift+wheel → x 增加。
4. **触控板详情**：1× 时 `deltaX` 累积 120 → 只切一张；450ms 内再次 → 不连跳；放大后 `deltaX` → 只平移，不切图；Ctrl+wheel → 缩放。
5. **信息栏**：1440×900 无横向溢出；标签展开时相似条仍可见/可滚；390 下为底部 sheet，单手可开合；对比度 ≥4.5:1。
6. **性能**：30s 连续拖拽的堆/DOM/帧率不劣于 §4.4 基线；信息栏打开不新增每帧 backdrop-filter；聚焦/关闭动画不产生长任务。
7. **回归**：四页+概要+390 的 `wall_shots` failures 为空；无控制台错误。

## 9. 开放问题（需拍板）

1. **信息栏形态**：推荐 120px 缝 + 300px 检查器；备选“缝从 120 展开到 300”。前者保留导航一致性，后者更像 Folia。选哪个？
2. **右键概要**：检查器上线后，右键概要（`.tile__info`）与侧栏信息重复。保留为快速预览，还是退役？
3. **触屏单击**：推荐单击=聚焦、信息栏“打开详情”兜底、双击=详情；是否接受？
4. **聚焦卡再点一次**：推荐不自动进详情（详情走双击/Enter/按钮）；是否接受？
5. **详情信息栏默认**：推荐默认展开、`I` 收起；是否保持“全屏图优先”，默认收起？
6. **详情横向 wheel**：推荐 1× 切图、放大后平移；是否希望横滑始终切图？
7. **信息栏宽度**：300px（Folia）还是 320/360px（更宽的中文标签）？

## 10. 参考资料

- Apple Support — Finder 画廊视图与预览栏：<https://support.apple.com/zh-sg/guide/macbook-air/apddf030866a/2024/mac/14.3>
- Figma 属性面板（中文指南）：<https://figmachina.com/guide/interface/properties-panel.html>
- Tiger Abrodi — Trackpad Pinch-to-Zoom vs Two-Finger Scroll：<https://tigerabrodi.blog/how-to-handle-trackpad-pinch-to-zoom-vs-two-finger-scroll-in-javascript-canvas-apps>
- MDN — WheelEvent：<https://developer.mozilla.org/en-US/docs/Web/API/WheelEvent>
- Excalidraw 触控板手势优化（deltaMode/ctrlKey 归一化）：<https://m.blog.csdn.net/weixin_32311823/article/details/156123148>
- macOS 双指轻扫前进/后退与关闭方法：<https://www.cnblogs.com/rxbook/p/17979662>
- Microsoft Edge 触控板滚动/缩放 flags：<https://www.biniku.com/article/1281563.html>
- GSAP Infinite Draggable Gallery（中心放大的视觉参考）：<https://gsapvault.com/blog/gsap-animation-examples>
- Acebuilder Expandable Card On Click（modal 反例）：<https://www.acebuilder.ai/blocks>
- 本地 Folia 规格：`.kanalib/folia-ui-spec.md`（120px 首页缝 / 300px 完整信息条 / 相机让位 0.42s / 6×6 展开卡 / 层序）
