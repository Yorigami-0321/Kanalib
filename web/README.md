# kanalib-wall — 云端图片墙前端（F-081）

Kanalib 的**只读静态图片墙**前端：纯静态 SPA（Vite + React 19 + TypeScript），
数据全部来自发布器导出的静态文件（`index.json` + 分片 + 缩略图），不连后端、不带写入能力。

- 墙页 `#/`：360° 全向无限画布，格子按 `(种子, 格号)` 按需生成；横/竖排混搭与跨行主图
- 墙页单击卡片 = **原位聚焦**：**被点卡片留在点击位置**按原图比例放大，同格其余图挤到它周围的右侧列/底部带；
  列数按图片平均比例计算，不再把邻图压成细条。点旁边的图时只把“放大对象”切到那一张，图片不会搬到固定英雄位、
  也不会与旧英雄互换位置；相机不居中，只在英雄卡被缝/检查器/sheet 遮住时做最小平移；不跳路由
- 双击卡片 = **全屏详情** `#/d/<id>`：画面只留图，标题/元数据/标签/相似/原图全部在左侧检查器；
  触摸横滑/`←`/`→`/横向 wheel 换图，`P` 自动放映（4.2s/张，底部进度线），`F` 全屏，
  `M` 留白视图（画框 + 收起信息时的极简图注）；
  `Ctrl+滚轮`/双指缩放，放大后拖拽平移，双击复位；检查器可复制当前图的深链
- `N` 单色预览：网格、标签封面、相似条与详情图一起去色，UI 与动态强调色保留
- 四套两级入口：标签 `#/tags → #/t/<tag>`、收藏 `#/favs → #/fav`、分级 `#/ratings → #/r/<G|S|Q|E>`、
  分组 `#/groups → #/g/<group>`；第一层是封面墙，第二层才是全部图片
- 触控板：双指横滑/纵滑都平移画布；详情 1× 时横向 wheel 换图；`Ctrl/⌘+滚轮` 才是捏合缩放
- 左侧 120px（窄屏 64px）竖排「缝」收四套入口、搜索/排序与功能键；≤900px 信息检查器改为底部 sheet

契约以 [`docs/backlog/photo-wall-contract.md`](../docs/backlog/photo-wall-contract.md) v1 为准（字段、目录、不变量）。
交互标准与验收口径见 [`docs/backlog/photo-wall-v3.md`](../docs/backlog/photo-wall-v3.md)、
[`docs/backlog/photo-wall-status.md`](../docs/backlog/photo-wall-status.md)。

## 快速开始

```bash
cd web
npm install
npm run dev        # http://127.0.0.1:5273
```

开发时把发布器导出的数据放到 `web/public/data/`（该目录被 `.gitignore` 忽略，不入库）：

```text
web/public/data/
├─ index.json            # schema / count / shards / facets / paths
├─ m/0000.json           # 分片，items 数组
├─ t256/<id>.jpg         # 256 长边缩略图（网格 1x）
├─ t512/<id>.jpg         # 512 主档（必存在；网格 2x 与详情兜底）
├─ t1024/<id>.jpg        # 可选锐利档（详情回退链）
├─ t2048/<id>.jpg        # 可选锐利档（详情回退链）
└─ o/<id>.<ext>          # 可选原图（详情优先，--with-originals 时产出）
```

然后：

```bash
npm run typecheck  # tsc --noEmit
npm run build      # tsc --noEmit && vite build -> dist/
npm run preview    # http://127.0.0.1:4273，服务 dist/
```

### 指向真实数据目录

默认数据根是站点相对的 `./data/`。若数据由别的域名/CDN 提供，构建（或 dev 时的 `.env.local`）里覆盖：

```bash
# .env.local（web/ 下，已被 .gitignore 忽略）
VITE_WALL_DATA=https://cdn.example.com/kanalib-wall/
# 或开发期指向导出目录
VITE_WALL_DATA=./wall_export/
```

值可以是绝对 URL 或相对路径；结尾斜杠有无都可以，内部统一拼接 `index.json` / 分片路径 / 档位目录。

## 部署

`npm run build` 产出 `web/dist/`，是**纯静态**目录，任何静态托管（对象存储 + CDN、Nginx、GitHub Pages…）都能直接放。
`vite.config.ts` 里 `base: './'`，因此可以部署在任意子路径（例如 `https://host/photos/`），不需要改配置。

数据目录与站点同源时直接放 `dist/data/`；数据放 CDN 时用 `VITE_WALL_DATA` 指过去（注意 CORS：`index.json` 与分片需要允许跨域读取）。

## 数据层

`src/source/` 是一个很小的抽象，页面层只认接口（`WallSource`）：

```ts
interface WallSource {
  init(): Promise<WallIndex>;              // 拉 index.json
  loadMore(): Promise<WallItem[]>;         // 顺序追加下一片（不并发拉全部）
  items(): readonly WallItem[];
  getItem(id: string): WallItem | undefined;
  paths(): TierPaths;                      // t256/t512/t1024/t2048/original 目录
  hasMore(): boolean;
  index(): WallIndex | null;
}
```

当前实现 `StaticSource`：`index.json` 首次加载 → 接近列表末尾时按 `shards` 顺序追加；某片请求失败会被记为该片已消费（避免无限重试同一片）。
将来接只读 API 时只需再写一个 `ApiSource` 实现同一接口，页面层不用改。

容错口径（`normalizeItem`）：id 必须是 40/64 位 `[0-9a-z]`；缺可选字段不报错；`w/h` 非正的条目直接跳过并计数。

## 交互与键盘

| 操作 | 行为 |
| --- | --- |
| 左键拖拽画布 | 平移（无限模式四向无边界；有限列表撞边回弹） |
| 滚轮 | 平移；`Ctrl+滚轮` 以光标为锚缩放，右上角出现瞬态比例提示 |
| 双指 | 缩放（锚点固定双指中点） |
| 单击卡片 | 原位聚焦：被点卡片原地放大、邻图挤到它周围（按比例选列，不切条）；点旁边的图只切放大对象、不交换位置；相机只做必要平移；不跳路由 |
| 双击 / 触屏双击 | 展开全屏详情（共享元素动画）；第一次单击的聚焦会保留，详情返回墙上仍在 |
| 右键 / 长按 500ms | 在卡片上叠出概要（scrim + 文件名/meta/标签 + 详情/关闭） |
| 详情页 · 换图 | 触摸/鼠标横滑、`←`/`→`、横向 wheel、侧栏上一张/下一张；`Esc` 返回聚焦/列表 |
| 详情页 · 缩放 | `Ctrl/⌘+滚轮` 或双指缩放（1–4×），放大后拖拽/横向 wheel 平移，双击复位 |
| 详情页 · 信息 | 左侧检查器默认展开；`I` 或缝里的信息钮收起/展开，收起后图回到全宽 |
| 详情页 · 放映 | 缝里的 ▶ / `P` 自动放映（4.2s/张、到尾回环），底部 2px 进度线；❚❚ 或离开详情停止 |
| 详情页 · 全屏 | `F` 或检查器的「全屏」按钮；浏览器拒绝时保持原状 |
| 详情页 · 分享 | 检查器「复制链接」复制 `#/d/<id>` 深链，成功后按钮变「✓ 已复制链接」 |
| 详情页 · 留白 | `M` 或侧栏画框按钮切换留白视图；收起信息后底部显示文件名 + 尺寸图注 |
| 全局 · 单色 | `N` 或侧栏底部半实心圆按钮；只给图片去色，截图/强调色不受影响 |
| 详情页 · 标签 | 检查器内按类别分组；默认 18 个 + `+N`，点「全部 N 个标签」展开，列表单独滚动 |
| 墙/详情 · 触控板 | 双指 deltaX/deltaY 平移；1× 详情横向 wheel 累积到 80px 切图（450ms 冷却）；捏合走 ctrlKey |
| 画布焦点 | 方向键/PageUp/PageDown 平移，`+`/`-` 缩放，`Home` 回起点（全局快捷键） |
| 键盘 · Tab | Tab 落到视口外的卡片时，相机最小平移把它带回可见区（不居中） |
| 左侧缝 | 四套入口（标签/收藏/分级/分组）、搜索、排序、缩放/复位/回起点、详情页返回/翻页、底部计数 |

## 筛选

标签/收藏/分级/分组是四套独立的「封面总览 → 二级墙」路径（Q8），不再叠加成侧栏筛选。
搜索面板只按文件名过滤，但会在**全部分片**上执行（列表/总览页会把分片拉齐后再算）；
搜索收窄时只淡入、不重播入场波次；换入口/换标签/扩宽仍播波次。
排序在无限首页选择非「随机」时会切成有限有序墙，保证顺序真实可见。

## 单击聚焦与信息检查器

- 聚焦卡：同格其余图**仍被挤到英雄周围**。布局以**点击瞬间那张卡的 world rect** 为锚，英雄卡留在原位置放大（约 60% 宽 / 62% 高、保持图片比例），其余图用 `gridColumns` 按“区域宽高比 × 平均图片比例”选列挤到四周；不再把新英雄搬到格子左上角、旧英雄挪到它原来的位置。实测点邻卡后英雄 `anchorDelta=[0,-20]`（中心基本不动），无图片位移交换。
- 相机不居中：只在英雄卡被缝/检查器/sheet 遮住时补最小平移。实测初始聚焦 `camDelta=69`、点邻卡 `camDelta=169`、`centerDelta=-249`，是避让而非居中。
- 卡片 key 与 DOM 顺序只含 `item.id + 格子 + index`，不含坐标/布局顺序——重排只改 rect，节点不移动也不重挂，邻图挤压和英雄放大都走 420ms 宽高过渡（150ms 时测得中间尺寸）。
- 聚焦卡图片切到 `spotlightSources`（按 `sharp` 声明的 t2048/t1024，最后 t512），卡上的标题/角标/遮罩隐藏，信息只在检查器。
- 网格卡用 `loading="eager"`（画布已虚拟化，浏览器懒加载在平移世界里不可靠），聚焦卡 `fetchpriority="high"`；
  未加载完的卡先显示 1.5s 骨架扫光，图片到帧后用 700ms 从 `blur(14px) scale(1.035)` 回到清晰。
- 检查器 300px：标题、两列元数据（尺寸/日期/分级/星级/分组/标签数）、按类别标签、3 列相似、打开详情/复制深链/全屏/原图；
  实色纸面 + 投影（透明纸面会让墙上的标题透字），不做常驻 backdrop-filter。
- 详情页：同图 t512 模糊铺满作为环境层；邻图预取 + 解码，滑动轨道 520ms，目标屏跳过淡入；`P` 自动放映，`F` 全屏。
- 视图模式（不写数据）：`N` 单色预览；`M` 留白视图（详情图收进画框，信息收起时底部一行文件名/尺寸图注）。
- 详情页 stage 让出 `seam + 300px`；≤900px 检查器改为底部 sheet（左从缝右缘开始，55vh）。聚焦重排仍按整格进行，相机只做最小平移，保证英雄卡落在 sheet 上方。
- `?gestures=1` 显示最近几次 wheel 的 `dx/dy/deltaMode/ctrlKey`，用于排查触控板事件。

## 图片档位与降级

- 网格：`img` 带 `srcset`，`sizes = 卡片宽 × 相机缩放`；基础两档 `t256 256w` / `t512 512w`，条目 `sharp >= 1024/2048` 时再追加 `t1024 1024w` / `t2048 2048w`。`t256` 404 回退 `t512`，两者都缺显示「缩略图缺失」占位
- 详情：`o/<id>.<ext>`（有 `orig` 时）→ 按 `sharp` 声明的档位（高→低）→ `t512`；旧数据没有 `sharp` 时逐档试探 `t2048 → t1024 → t512`
- 锐利档是可选产物；源图长边不够、视频、ffmpeg-only 格式（AVIF/PSD）都会缺档，`sharp` 会如实写 0 或较低档，前端因此不做多余请求

## 相似推荐

左侧信息检查器对**当前已加载集合**里带 `ph`（pHash，16 位小写 hex）的条目算汉明距离，升序取最近 6 条（距离 ≤12）；
条目没有 `ph` 或集合里没有相似项时显示空态。点击相似图进入详情（墙上的近似项不一定在当前可见格）。

## 目录结构

```text
web/
├─ index.html
├─ vite.config.ts          # base './'，dev 5273 / preview 4273
├─ tsconfig.json
├─ public/                 # 静态资源（data/ 为本地数据，不入库）
└─ src/
   ├─ main.tsx
   ├─ App.tsx              # 路由、筛选、概要、详情展开
   ├─ styles.css           # 主题变量、画布、缝
   ├─ source/              # StaticSource + 契约类型 + 数据根配置
   ├─ lib/                 # 路由、pHash、世界布局、强调色、共享元素展开、档位 URL
   ├─ state/               # useWall（顺序装片）、筛选
   └─ components/          # Wall / WallCard / Sidebar / TagGallery / BucketGallery / Detail / Inspector / GestureDebug + 各自 CSS
```

## 依赖许可

运行时只依赖 React；其余是构建工具。全部 MIT / Apache-2.0，锁在 `package-lock.json` 里：

| 包 | 用途 | 许可 |
| --- | --- | --- |
| react / react-dom | UI | MIT |
| vite / @vitejs/plugin-react / typescript | 构建与类型检查 | MIT / MIT / Apache-2.0 |

不引入遥测、外链字体或 CDN 资源；公开墙默认可被搜索引擎收录（W4）。需要私密部署时在托管层加鉴权，
或给 `index.html` 补回 `<meta name="robots" content="noindex, nofollow">`。
