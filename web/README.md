# kanalib-wall — 云端图片墙前端（F-081）

Kanalib 的**只读静态图片墙**前端：纯静态 SPA（Vite + React 19 + TypeScript），
数据全部来自发布器导出的静态文件（`index.json` + 分片 + 缩略图），不连后端、不带写入能力。

- 墙页 `#/`：Masonry 混排拼贴（1x1 / 1x2 / 2x1 / 2x2 尺寸档），滚动贴近底部顺序加载下一片
- 详情页 `#/a/<id>`：大图、标签 chips、元数据、相似推荐（pHash 汉明距离）
- 交互：单击 220ms 去抖后开 PhotoSwipe lightbox，双击直接进详情；lightbox 工具栏常驻「详情」（移动端兜底）
- 整墙缩放：可选的「缩放模式」（工具栏按钮 / `W` 键），默认关闭，滚轮仍然用来滚动加载

契约以 [`docs/backlog/photo-wall-contract.md`](../docs/backlog/photo-wall-contract.md) v1 为准（字段、目录、不变量）。

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
├─ t256/<id>.jpg         # 256 长边缩略图（网格用，可缺）
├─ t512/<id>.jpg         # 512 主档（必存在）
├─ t1024/<id>.jpg        # 可选，lightbox 优先
└─ o/<id>.<ext>          # 可选原图（详情页用）
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

`src/source/` 里是一个很小的抽象，上层（墙、详情、lightbox）只认接口：

```ts
interface WallSource {
  init(): Promise<WallIndex>;              // 拉 index.json
  loadMore(): Promise<WallItem[]>;         // 顺序追加下一片（不并发拉全部）
  items(): readonly WallItem[];            // 已加载条目
  getItem(id: string): WallItem | undefined;
  readonly paths: TierPaths;               // t256/t512/t1024/original 目录
  readonly hasMore: boolean;
}
```

当前实现 `StaticSource`：`index.json` 首次加载 → 接近列表末尾时按 `shards` 顺序追加；某片请求失败会被记为该片已消费（避免无限重试同一片）。
将来接只读 API 时只需再写一个 `ApiSource` 实现同一接口，页面层不用改。

容错口径（`normalizeItem`）：id 必须是 40/64 位 `[0-9a-z]`；缺可选字段不报错；`w/h` 非正的条目直接跳过并计数。

## 交互与键盘

| 操作 | 行为 |
| --- | --- |
| 单击卡片 | 约 220ms 去抖后打开 lightbox |
| 双击卡片 | 取消待触发的单击，直接路由到 `#/a/<id>` |
| lightbox 内 | `Esc` 关闭、`←`/`→` 切图、滚轮缩放、右上「详情」按钮进详情页 |
| `W` | 切换整墙缩放模式（缩放中滚轮=缩放、拖拽=平移，页面滚动被锁住） |
| `Esc`（无 lightbox） | 先退出缩放模式 |
| 「适应屏幕」 | 复位缩放并退回普通滚动 |
| 标签 chip（详情页） | 带上 `#/?tag=<name>` 回到墙并作为筛选条件 |

## 筛选

工具栏的标签、收藏、分级、星级、分组、文件名子串**全部在浏览器侧对已加载数据做子集过滤**（不重新请求数据），
统计区同时显示「已加载 X / 共 Y · 命中 Z」。因为只过滤已加载的部分，脚本会在命中 0 时提示继续向下滚动加载。
分组筛选依赖契约 §4 的可选字段 `g`；数据里没有 `g` 时该下拉只有「全部」，详情页显示「未分组」。
标签 facet 为空（例如合成图库）时，标签面板显示「这份数据没有标签 facet」。

## 图片档位与降级

- 网格：`t256` → 404 自动回退 `t512`；两者都缺时显示「图片不可用」占位，不阻断排版
- lightbox：`t1024`（首次探测存在才用）→ `t512` → `t256`，单张 404 自动降档重试
- 详情页：有 `orig` 用 `o/<id>.<ext>`，否则 `t512`；每个 `img` 都带 `onError` 降级

## 相似推荐

详情页对**当前已加载集合**里带 `ph`（pHash，16 位小写 hex）的条目算汉明距离，升序取前 12 条；
条目没有 `ph` 或集合里没有其它带 `ph` 的条目时显示空态说明。

## 目录结构

```text
web/
├─ index.html
├─ vite.config.ts          # base './'，dev 5273 / preview 4273
├─ tsconfig.json
├─ public/                 # 静态资源（data/ 为本地数据，不入库）
└─ src/
   ├─ main.tsx
   ├─ App.tsx              # 路由、筛选状态、缩放模式、滚动记忆
   ├─ styles.css           # 暗色主题
   ├─ source/              # StaticSource + 契约类型 + 数据根配置
   ├─ lib/                 # 路由、pHash 汉明距离、探测、格式化、档位 URL
   ├─ state/               # useWall（顺序装片）、筛选
   └─ components/          # Wall / WallCard / ZoomStage / Toolbar / Lightbox / Detail
```

## 依赖许可

只使用 MIT / Apache-2.0 / BSD 许可的库，且都锁在 `package-lock.json` 里：

| 包 | 用途 | 许可 |
| --- | --- | --- |
| react / react-dom | UI | MIT |
| @egjs/react-infinitegrid | 混排无限网格 | MIT |
| photoswipe | lightbox | MIT |
| react-zoom-pan-pinch | 整墙缩放 | MIT |
| vite / @vitejs/plugin-react / typescript | 构建与类型检查 | MIT / MIT / Apache-2.0 |

不引入遥测、外链字体或 CDN 资源，页面自带 `noindex`（公开墙默认不挡搜索引擎，若需要私密部署可在托管层加鉴权/robots）。
