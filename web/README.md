# kanalib-wall — 云端图片墙前端（F-081）

Kanalib 的**只读静态图片墙**前端：纯静态 SPA（Vite + React 19 + TypeScript），
数据全部来自发布器导出的静态文件（`index.json` + 分片 + 缩略图），不连后端、不带写入能力。

- 墙页 `#/`：360° 全向无限画布，格子按 `(种子, 格号)` 按需生成；横/竖排混搭与跨行主图
- 详情页 `#/d/<id>`：整屏大图（contain）、标签 chips、元数据、相似推荐（pHash 汉明距离），左右切换走水平滑动
- 标签页 `#/tags`、单标签墙 `#/t/<tag>`
- 交互：拖拽平移、滚轮平移、`Ctrl+滚轮`/双指缩放；单击选中、双击（或触屏单击）展开全屏；
  右键/长按在卡片上叠出概要；左侧 120px（窄屏 64px）竖排「缝」收页签、筛选与功能键

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
| 单击卡片 | 选中高亮（竖线 + 提亮）；同一张 360ms 内再次抬起 = 展开 |
| 双击/触屏单击 | 展开全屏详情（共享元素动画） |
| 右键 / 长按 500ms | 在卡片上叠出概要（scrim + 文件名/meta/标签 + 详情/关闭） |
| 详情页 | `←`/`→` 水平滑动切图，`Esc` 返回；焦点归位到触发卡 |
| 画布焦点 | 方向键/PageUp/PageDown 平移，`+`/`-` 缩放，`Home` 回起点（全局快捷键） |
| 左侧缝 | 页签（墙/标签）、搜索与筛选、缩放/复位/回起点、详情页返回/上一张/下一张、底部计数 |

## 筛选

左侧「缝」里的标签、收藏、分级、分组、文件名子串**全部在浏览器侧对已加载数据做子集过滤**（不重新请求数据）。
筛选收窄时只淡入、不重播入场波次；换页签/换标签/扩宽仍播波次。
标签 facet 为空（例如合成图库）时，标签面板显示空态说明。

## 图片档位与降级

- 网格：`img` 带 `srcset`，`sizes = 卡片宽 × 相机缩放`；基础两档 `t256 256w` / `t512 512w`，条目 `sharp >= 1024/2048` 时再追加 `t1024 1024w` / `t2048 2048w`。`t256` 404 回退 `t512`，两者都缺显示「缩略图缺失」占位
- 详情：`o/<id>.<ext>`（有 `orig` 时）→ 按 `sharp` 声明的档位（高→低）→ `t512`；旧数据没有 `sharp` 时逐档试探 `t2048 → t1024 → t512`
- 锐利档是可选产物；源图长边不够、视频、ffmpeg-only 格式（AVIF/PSD）都会缺档，`sharp` 会如实写 0 或较低档，前端因此不做多余请求

## 相似推荐

详情页对**当前已加载集合**里带 `ph`（pHash，16 位小写 hex）的条目算汉明距离，升序取最近 6 条（距离 ≤12）；
条目没有 `ph` 或集合里没有相似项时隐藏相似条。

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
   └─ components/          # Wall / WallCard / Sidebar / TagGallery / Detail + 各自 CSS
```

## 依赖许可

运行时只依赖 React；其余是构建工具。全部 MIT / Apache-2.0，锁在 `package-lock.json` 里：

| 包 | 用途 | 许可 |
| --- | --- | --- |
| react / react-dom | UI | MIT |
| vite / @vitejs/plugin-react / typescript | 构建与类型检查 | MIT / MIT / Apache-2.0 |

不引入遥测、外链字体或 CDN 资源；公开墙默认不挡搜索引擎（需要私密部署时在托管层加鉴权/robots）。
