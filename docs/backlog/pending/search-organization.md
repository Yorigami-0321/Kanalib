# 搜索与组织

> 上级索引：[../README.md](../README.md) · 文档地图：[README](../README.md)

## F-012 搜索与筛选性能

- **依据**：`PLAN.md` §12 与阶段 3 目标搜索 p95 < 150ms；10 万实测 p95 284ms（`synthetic` 全命中）；改造前每次文本变化都重建可见索引，`duplicate_paths` 无论是否用 `dup:true` 都构造集合，短词走 `LIKE %x%` 全表扫描。
- **已完成（第二轮）**：DB 侧筛选/排序下推 SQL（评级/星级/收藏/来源/时间/尺寸等索引列，`ensure_search_indexes` 幂等建索引）、keyset 分页（`SearchPage`/`SearchCursor`，`MAX_SEARCH_PAGE=10000`）、`search_paths_in` 保留旧语义兼容；结果与内存过滤逐条一致由单测 `search_page_matches_memory_filter_sort_and_paging` 保证。UI 侧输入 220ms 防抖、只在 `dup:true` 时构造去重路径集、查询与筛选/排序/数据版本绑定成 `SearchBundle` 快照；分页结果超限时回退内存过滤（不截断）。
- **剩余**：命中全库的非名称排序（modified/captured/size/resolution/star/favorite/rating）仍走临时 B 树排序，p95 276–304ms，超过 150ms 目标；需要覆盖索引或预排序键才能压下去。
- **验收（剩余）**：10 万暖库非名称排序 p95 ≤ 150ms；UI 端到端键入延迟另有一次单独实测（当前只有核心 DB 数字）。
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

## F-013 智能集合可编辑

- **依据**：`PLAN.md` §8.1 智能集合；`SavedSearch` 只有 `name/filter/query`（`model.rs:495-500`）。
- **现状**：侧栏可保存、应用、删除；没有重命名、更新为当前视图、排序/置顶；应用时只恢复筛选与搜索，不恢复排序、视图模式或窗格状态。
- **建议**：增加“编辑/用当前视图更新/重命名/删除”菜单；保存时可选记录排序、视图模式、窗格与显示开关；侧栏支持置顶/拖动排序；同名冲突给出明确提示；可选导出/导入集合定义。
- **验收**：更新后集合内容与当前筛选/搜索一致；重命名不改变 ID；应用集合恢复排序与视图；重启后保持顺序与置顶。
- **依赖/风险**：`saved_searches.query_json` 增加可选字段需向后兼容；与 `ui.view_mode` 同步。

## F-014 手动相册/集合

- **依据**：`PLAN.md` §5 的 `collections`/`collection_assets`；digiKam 的相册、Eagle 的文件夹。
- **现状**：两张表只有 schema，没有任何读写；目前“集合”仅指基于筛选的智能集合。
- **建议**：实现静态相册：新建/重命名/删除；将选中图片加入/移出相册；侧栏与检查器显示所属相册；计数；同一图片可属于多个相册；不移动磁盘文件；支持导出相册清单。
- **验收**：加入/移出立即刷新计数；重启保留；删除相册只删关系，不删资产；智能集合与手动相册在侧栏分区清晰。
- **依赖/风险**：与 F-013 的命名/排序交互；删除资产时的级联关系；多选批量加入。
