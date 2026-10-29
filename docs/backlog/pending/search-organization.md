# 搜索与组织

> 上级索引：[../README.md](../README.md) · 文档地图：[README](../README.md) · 第三轮任务包：[round-3.md](../round-3.md)

## F-012 搜索与筛选性能

- **依据**：`PLAN.md` §12 与阶段 3 目标搜索 p95 < 150ms；10 万实测 p95 284ms（`synthetic` 全命中）；改造前每次文本变化都重建可见索引，`duplicate_paths` 无论是否用 `dup:true` 都构造集合，短词走 `LIKE %x%` 全表扫描。
- **已完成（第二轮）**：DB 侧筛选/排序下推 SQL（评级/星级/收藏/来源/时间/尺寸等索引列，`ensure_search_indexes` 幂等建索引）、keyset 分页（`SearchPage`/`SearchCursor`，`MAX_SEARCH_PAGE=10000`）、`search_paths_in` 保留旧语义兼容；结果与内存过滤逐条一致由单测 `search_page_matches_memory_filter_sort_and_paging` 保证。UI 侧输入 220ms 防抖、只在 `dup:true` 时构造去重路径集、查询与筛选/排序/数据版本绑定成 `SearchBundle` 快照；分页结果超限时回退内存过滤（不截断）。
- **已完成（第四轮，2026-10-30）**：
  - 同库 100k 对照（同一份合成库，30 次）：为 Modified/Captured/Size/Rating 建覆盖索引；Star/Favorite 因低基数实测回退，Resolution 收益不足，三者不建索引并清理旧实验索引。保留索引的排序 p95 为 232–304ms，对应无索引基线 367–387ms；名称排序保持不变（约 233ms）。
  - 索引体积：同一 100k 库从 98,541,568 字节增至 139,251,712 字节（+40.7MB，+41%）；曾尝试的 7 索引版本为 207,605,760 字节（+110.6MB），因 Star/Favorite 回退被否决。
  - UI：`search.rs` 仅在 `KANALIB_SEARCH_TIMING=1` 时记录防抖等待、查询耗时与命中数，不改变查询/渲染行为；真正渲染阶段与键入延迟由 E1 在 UI 侧实测。
  - 结论：`p95 ≤ 150ms` 仍未达成，按任务书改为记录同库实测上限；名称排序与旧文档 133ms 的差异来自合成库内容/负载不同，引用数字必须同时给出库标识。
- **验收（剩余）**：UI 端到端键入延迟（防抖 + 查询 + 可见结果更新）留给 E1 实测；后续若更换合成库或 schema，需要重新跑同库对照，不能直接复用本表绝对数字。
- **依赖/风险**：FTS5 对 2 字词仍回退 `LIKE`（`syn` 33ms、`sky` 13ms 可接受）；SQL 与内存过滤语义必须继续逐条对齐。

### 实测（release，`KANALIB_BENCH_ROWS=100000` 重新生成的 10 万合成库，30 次取 p95）

| 查询/排序 | 命中 | p50 | p95 |
|---|---|---|---|
| 自由词 `synthetic` + 名称排序 | 100,000 | 126ms | 133ms |
| 自由词 `合成` | 25,000 | 66ms | 71ms |
| 自由词 `sky` | 33,334 | 12ms | 13ms |
| 短词 `syn` | 100,000 | 31ms | 33ms |
| 混合词 `synthetic sky` | 33,334 | 88ms | 101ms |
| `synthetic` + modified 排序 | 100,000 | 265ms | 284ms |
| `synthetic` + captured 排序 | 100,000 | 265ms | 276ms |
| `synthetic` + size 排序 | 100,000 | 267ms | 284ms |
| `synthetic` + resolution 排序 | 100,000 | 269ms | 278ms |
| `synthetic` + star 排序 | 100,000 | 270ms | 291ms |
| `synthetic` + favorite 排序 | 100,000 | 267ms | 281ms |
| `synthetic` + rating 排序 | 100,000 | 289ms | 304ms |
| 筛选 `group:角色` | 33,334 | 31ms | 34ms |
| 筛选 `date:>2024` | 99,501 | 10ms | 12ms |
| 筛选 `is:failed` / `is:damaged` | 991 / 730 | 13ms / 10ms | 14ms / 11ms |
| keyset 第二页（名称） | 100,000 | 128ms | 141ms |
| 旧 `legacy_search_paths`（对照） | 100,000 | 211ms | 225ms |

> 数据依赖：用合并前生成的旧 bench 库复测，同一批非名称排序是 350–380ms、名称排序 140ms，差异来自合成库内容分布，因此引用数字时必须说明用的是哪份库。

### 第四轮同库对照（2026-10-30，release，100k，30 次）

库标识：`target/r4-c-bench/kanalib-bench-100k-r4c.db`（100k 行，folder `C:\bench`，由 `generate_bench_database` 生成；4 索引 139.3MB，复制后删掉 4 个排序索引得到 98.5MB 基线）。本表和上表不是同一份库，绝对 p95 不可跨表比较；本表的决策依据是同一份库上的 cover/baseline 成对数字。

| 排序 | 索引决策 | cover p50 | cover p95 | baseline p50 | baseline p95 |
|---|---|---:|---:|---:|---:|
| Name | 既有 `idx_assets_name_lower`，不新增 | 207ms | 234ms | 215ms | 232ms |
| Modified | 保留 `idx_assets_sort_modified_desc` | 213ms | 234ms | 339ms | 367ms |
| Captured | 保留 `idx_assets_sort_captured_desc` | 214ms | 232ms | 341ms | 368ms |
| Size | 保留 `idx_assets_sort_size_desc` | 209ms | 233ms | 331ms | 372ms |
| Rating | 保留 `idx_assets_sort_rating_desc` | 287ms | 304ms | 352ms | 387ms |
| Resolution | 删除索引，回基线 | 331ms | 382ms | 333ms | 357ms |
| Star | 删除索引，回基线 | 346ms | 372ms | 341ms | 366ms |
| Favorite | 删除索引，回基线 | 336ms | 378ms | 361ms | 377ms |

- 保留的 4 个索引在 `synthetic` 全命中下把 p95 降低约 21–37%；Star/Favorite 的低基数使覆盖索引反而更慢，Resolution 约 5% 的收益不足以抵消索引体积，因此都不保留。
- 早前同库对照（机器同时跑 UI/构建、负载较高）中，`synthetic sky` + Modified 从 1584ms 降到 984ms，说明保留的索引在选择性过滤上未出现回退；该组数字不作为最终绝对基准。
- `KANALIB_BENCH_SECTIONS=sorts` 可只跑本表，避开名称/自由词段落；基准开关只存在于 `#[cfg(test)]`。

### UI 端到端（第四轮 E1，`02e4960`）

库：`target/r4-task0/kanalib-100k.db` 的隔离副本（100k 行，folder `C:\bench`，源目录离线、无缩略图；这是缺图最坏情况，不代表真实缩略图库）。启动 `KANALIB_SEARCH_TIMING=1`，用窗口内 WM_CHAR 输入，记录 `ui_input_to_visible_ms`/`ui_input_to_frame_ms`；防抖固定 220ms。原始日志 `.kanalib/r4_e1/logs/search100k-limit512c.log`。

| 查询 | 命中 | page | run_search | visible | frame |
|---|---:|---:|---:|---:|---:|
| 空（启动） | 100000 | 31.7ms | 31.7ms | 328ms | 384ms |
| `synthetic` | 100000 | 422ms | 585ms | 931ms | 937ms |
| `sky` | 33334 | 263ms | 354ms | 657ms | 664ms |
| `synthetic sky` | 33334 | 1546ms | 1658ms | 1943ms | 1965ms |
| `forest` | 16667 | 1857ms | 1930ms | 2199ms | 2250ms |
| `4`（短词） | 40951 | 103ms | 713ms | 1005ms | 1062ms |
| `synthetic_0004`（修复前） | 10 | 14007ms | 14056ms | 14312ms | 14321ms |
| `synthetic_0004`（修复后） | 10 | 55ms | 105ms | 360ms | 369ms |

- 第四轮把 UI 的单次 SQL 探测上限从 core 的 `MAX_SEARCH_PAGE=10000` 降为 `UI_SEARCH_PAGE_LIMIT=512`；超过即回退内存过滤（结果语义不变，单测 `search_page_matches_memory_filter_sort_and_paging` 继续通过）。复测中 `synthetic` 从约 1.6s 降到 0.93s、`sky` 从约 1.45s 降到 0.66s。
- **窄 FTS 异常已定位并修复**：`search_extent` 原用普通 `JOIN`，SQLite 3.46 在仅 10 命中时选择扫 10 万 assets、逐行探 FTS；改为 `CROSS JOIN` 强制 FTS 外表后，核心 `search_page` 从约 19s 降到 3.5ms，UI `synthetic_0004` page 55ms / visible 360ms（修复后日志 `.kanalib/r4_e1/logs/search100k-fixed.log`；修复提交 `fed7857`）。回归测试 `fts_extent_query_plan_prefers_fts_driver` 断言 extent 计划从 FTS 驱动且不使用 `idx_assets_folder`。
- **仍未达到 150ms**：220ms 防抖已占固定成本；100k 正常缩略图库的键入延迟仍待实测。该表只作为最坏情况记录，不与 core 的同库 p50/p95 混用；`synthetic_0004` 修复后数字来自同一隔离副本与窗口内输入。
- UI 搜索语义、FTS5、排序与筛选下推均未改；所有计时仅在 `KANALIB_SEARCH_TIMING=1` 时输出，产品默认关。

## F-013 智能集合可编辑

> 第三轮已实现（schema v9 + 侧栏编辑入口），见 [implemented.md](../implemented.md)。

## F-014 手动相册/集合

> 第三轮已实现（核心相册 CRUD + 侧栏 `albums` 分区），见 [implemented.md](../implemented.md)。

