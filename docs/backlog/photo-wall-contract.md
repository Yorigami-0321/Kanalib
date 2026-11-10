# 图片墙静态导出契约 v1（F-080 → F-081 接口）

> 上级：[cloud-photo-wall.md](cloud-photo-wall.md)。2026-11-08 定稿，分支 `feature/cloud-photo-wall`。
> 本文件是**发布器（Rust）与墙前端（web）之间唯一的契约**。任一侧改动字段必须先改本文件，再同步另一侧。

## 1. 导出目录布局（站点根相对路径）

```
<site-root>/
  index.json          # 清单与分片索引（首包）
  m/0000.json         # 分片，按 index.shards[].path 引用
  m/0001.json
  t256/<id>.jpg       # 网格档（256 长边）
  t512/<id>.jpg       # 主档（512 长边）
  t1024/<id>.jpg      # 锐利档（1024 长边，可选）
  t2048/<id>.jpg      # 锐利档（2048 长边，可选）
  o/<id>.<ext>        # 可选原图（--with-originals 时；ext 为小写原扩展名）
```

所有 URL 都是**相对站点根**的静态路径；前端不得拼出绝对主机名。目录名固定，不做配置化。

## 2. `index.json`

```json
{
  "schema": 1,
  "generated_at": "2026-11-08T23:12:00Z",
  "count": 1234,
  "shard_size": 5000,
  "shards": [{ "i": 0, "path": "m/0000.json", "count": 1234 }],
  "facets": {
    "tags": [{ "name": "1girl", "category": "general", "count": 980 }],
    "groups": ["原创", "漫画"]
  },
  "paths": { "t256": "t256/", "t512": "t512/", "t1024": "t1024/", "t2048": "t2048/", "original": "o/" }
}
```

- `schema`：整数，当前 1。加字段不需要升版；**删字段/改语义必须升版**。
- `shards`：顺序即墙的展示顺序；`i` 从 0 连续递增；每片 `count <= shard_size`。
- `facets.tags`：按 count 降序，最多 2000 项；`category` 是**库内原值透传**，常见为 `general`/`character`/`copyright`/`artist`/`manual`/`meta`，也允许 `rating` 等其它自定义类别。前端遇到未知类别必须按默认样式渲染，不得丢弃该标签。
- `facets.groups`：`assets.group` 的去重值，最多 200 项。
- `paths`：档位前缀，缺省时前端按第 1 节默认值兜底；`t1024`/`t2048` 未产出时发布器也照常写出对应键（前端逐档回退到 t512）。

## 3. 分片 `m/NNNN.json`

```json
{ "schema": 1, "index": 0, "items": [ /* 见 §4 */ ] }
```

## 4. 条目（item）字段

| 字段 | 类型 | 必有 | 说明 |
|---|---|---|---|
| `id` | string | 是 | 见 §5；同时是 `t256/t512/o` 的文件名主干 |
| `w` | number | 是 | 像素宽（>0） |
| `h` | number | 是 | 像素高（>0） |
| `n` | string | 是 | 展示名（basename，不泄漏目录） |
| `rt` | string | 是 | 分级：`G`/`S`/`Q`/`E`（库内原值，缺失时 `G`） |
| `st` | number | 是 | 星级 0–5 |
| `fav` | boolean | 是 | 收藏 |
| `tags` | array | 是 | `[name, category, score]` 三元组数组；score 为 0–1 的数（保留 3 位小数），可为空数组 |
| `g` | string | 否 | 归属分组（`assets.group` 原值，如「原创」「漫画」）；空串或缺失表示未分组。用于墙的分组筛选，避免只能靠 `facets.groups` 猜 |
| `ph` | string | 否 | 16 位小写 hex pHash；缺失则不出现 |
| `mtime` | number | 是 | UTC 秒 |
| `orig` | string | 否 | 原图扩展名（小写，无点），如 `"png"`；缺省表示该条无原图 |
| `sharp` | number | 否 | 本条目可用的最高锐利档长边：`0` = 无，`1024`/`2048` = 有；缺省表示旧数据未声明（前端按未知逐档试探）。新发布器对每条都写（含 0） |

- 未知字段前端必须忽略；缺失可选字段不得报错。
- `tags` 用数组而非对象，是为了 100k 条规模下的首包体积。
- `tags[*][1]`（category）同样按库内原值透传，可含 `rating` 等非标准类别；校验器只要求「非空字符串」。

## 5. `id` 规则（隐私口径 W4 的落点）

- 优先 `assets.sha1`（小写 hex，40 位）；缺失时用 `assets.blake3`（64 位）；两者都缺时用 `path_key::encode(path)` 的 blake3 十六进制。
- `id` 只允许 `[0-9a-z]`，长度 40 或 64。**严禁**把绝对路径、来源根、文件名拼进 `id`。
- 同一资产跨次发布 `id` 稳定。

## 6. 前端读取规则

1. 先取 `index.json`，据 `count`/`shards` 渲染首片；滚动接近已加载末尾时按序追加下一片（不并发拉全部分片）。
2. 图片优先级：
   - 网格：`t256` → 404 回退 `t512`；有 `srcset` 时按卡片实际显示宽度（含相机缩放）在可用档位中选档。`sharp` 声明了 t1024/t2048 才把对应档位放进 srcset。
   - 详情：`o/<id>.<ext>`（`orig` 存在时）→ 按 `sharp` 声明的锐利档（高→低）→ `t512`；`sharp` 缺省时退回逐档试探 `t2048 → t1024 → t512`。
3. 数据源抽象 `StaticSource`：`init()`、`loadMore()`、`getItem(id)`、`items()`；将来动态后端换 `ApiSource`，墙/详情不动（§4.1 迁移纪律）。
4. 搜索/筛选在浏览器侧只做常用子集：标签（含手动标签）、收藏、分级、分组、文件名子串；不做完整查询语法。

## 7. 不变量（发布器验收口径）

- 导出目录内**任何文件**都不含本地绝对路径（含 `index.json`、分片、无日志文件）。
- 每个 item 的 `t512/<id>.jpg` 必定存在；`t256` 缺失时允许只写 `t512`（前端回退）。
- `t1024`/`t2048` 是可选产物：仅在原图可解码且长边大于档位长边时生成；**绝不放大**，视频、ffmpeg-only 格式（AVIF/PSD）和源文件缺失的条目不产出，前端逐档回退。
- `sharp` 必须与实际产出一致：`>= 1024` 时 `t1024/<id>.jpg` 必须存在，`>= 2048` 时 `t2048/<id>.jpg` 必须存在；`0` 表示两档都没有。新发布器对每条 item 都写 `sharp`，前端因此不需要用 404 试探。
- 锐利档编码口径：EXIF 方向套用后再按长边缩放（Lanczos3），ICC/EXIF 广色域先转 sRGB，JPEG q=90。
- 分片按固定排序（`SearchSort::Modified` 降序，同 mtime 用 path 稳定兜底）切分，重发布结果顺序稳定。
- 增量：`publish-ledger.json`（`id → {sha1, mtime, bytes, files:[...]}`）记录上次产出；未变资产不重抄缩略图/原图。该文件只写导出目录，不上传（`--no-upload-ledger` 预留）。
- `--dry-run` 只打印统计（资产数、分片数、将复制/跳过数量），不写任何文件。

## 8. 演进

- 新增档位（t1024/t2048）只加 `paths` 键与文件，不改条目字段；`sharp` 是可选的可用档声明，值域随档位扩展。
- 动态后端迁移时 API 返回**同样形状**的 item 与分片，前端零改动。
