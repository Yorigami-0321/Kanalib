# 搜索与组织

> 上级索引：[../README.md](../README.md) · 文档地图：[README](../README.md) · 第三轮任务包：[round-3.md](../round-3.md)

## F-012 搜索与筛选性能

- **依据**：`PLAN.md` §12 与阶段 3 目标搜索 p95 < 150ms；10 万实测 p95 284ms（`synthetic` 全命中）；改造前每次文本变化都重建可见索引，`duplicate_paths` 无论是否用 `dup:true` 都构造集合，短词走 `LIKE %x%` 全表扫描。
- **已完成（第二轮）**：DB 侧筛选/排序下推 SQL（评级/星级/收藏/来源/时间/尺寸等索引列，`ensure_search_indexes` 幂等建索引）、keyset 分页（`SearchPage`/`SearchCursor`，`MAX_SEARCH_PAGE=10000`）、`search_paths_in` 保留旧语义兼容；结果与内存过滤逐条一致由单测 `search_page_matches_memory_filter_sort_and_paging` 保证。UI 侧输入 220ms 防抖、只在 `dup:true` 时构造去重路径集、查询与筛选/排序/数据版本绑定成 `SearchBundle` 快照；分页结果超限时回退内存过滤（不截断）。
- **剩余**：命中全库的非名称排序（modified/captured/size/resolution/star/favorite/rating）仍走临时 B 树排序，p95 276–304ms；需要覆盖索引或预排序键才能压下去。
- **验收（剩余）**：2026-10-27 决定把 `p95 ≤ 150ms` 从硬门禁改为「先小基准、取实测上限并写回本文档」；UI 端到端键入延迟仍需要一次单独实测（当前只有核心 DB 数字）。这条线在第三轮列为顺延（见 [round-3.md](../round-3.md) 的「顺延 3」），未派发。
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

- **依据**：`PLAN.md` §8.1 智能集合；`SavedSearch` 原本只有 `name/filter/query`。
- **已完成（第三轮核心，`6c461c2`，schema v9）**：`saved_searches` 增 9 列（`sort_key`/`sort_ascending`/`view_mode`/`pane_mode`/`show_sidebar`/`show_extensions`/`pinned`/`position`/`updated_at`）；新增 `SavedSearchView` 与 `create_saved_search`/`load_saved_search`/`rename_saved_search`/`update_saved_search`/`set_saved_search_pinned`/`reorder_saved_searches`；同名冲突返回 `DuplicateNameError`（`OrganizationKind` 区分集合/相册），旧行按默认视图读取。迁移前自动备份 `kanalib.db.bak-v8-<ts>`。
- **剩余**：侧栏「智能集合」的编辑/用当前视图更新/重命名/置顶与拖动排序入口，以及应用集合时恢复排序/视图/窗格——属任务 G1，未做。
- **验收（剩余）**：重命名后 ID 不变；应用集合恢复排序与视图；重启后顺序与置顶保持。
- **依赖/风险**：`query_json` 保持向后兼容；UI 消费新增字段时注意空字符串语义是「沿用当前」而非缺失。

## F-014 手动相册/集合

- **依据**：`PLAN.md` §5 的 `collections`/`collection_assets`；digiKam 的相册、Eagle 的文件夹。
- **已完成（第三轮核心，`6c461c2`）**：`Collection` 结构与相册 CRUD（`create/rename/delete_collection`）、批量加入/移出（`add_assets_to_collection`/`remove_assets_from_collection`）、计数（`collection_asset_count`）、按相册列举（`collection_paths`/`load_collection_assets`）、反查所属相册（`asset_collections`）、导出路径清单（`export_collection_paths`，拒绝覆盖且不复制文件）；同一资产可属多个相册；删除相册只删关系；资产删除与来源移除级联清理关系行。
- **剩余**：侧栏「相册」分区（新建/重命名/删除、加入/移出、计数与所属相册展示）——属任务 G1，未做。**侧栏现有分区 id `collections` 指智能集合**，新分区必须另起 id（如 `albums`）。
- **验收（剩余）**：侧栏增删即时刷新计数；重启保留；不移动磁盘文件。
- **依赖/风险**：与 F-013 的命名/排序交互；多选批量加入的 UI 入口。
