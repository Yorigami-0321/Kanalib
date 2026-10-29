# 浏览与查看

> 上级索引：[../README.md](../README.md) · 文档地图：[README](../README.md) · 第三轮任务包：[round-3.md](../round-3.md)

## F-003 图片对比

> 第三轮已实现（并排 2–4 栏 + 同步缩放 + 差异表 + 保留标记），见 [implemented.md](../implemented.md)。

- **第四轮验证（2026-10-30）**：从 10k 去重组「对比此组」发起 3 张对比通过——并排 3 栏、同步缩放（×1.95，三栏一致）/同步平移、差异表（差异 1 项/相同 8 项）、保留标记（只标记不删移）、Esc 返回去重组与滚动位置。证据 `.kanalib/r4_task0/screens/10k-compare-3up.png`、`10k-compare-zoomin.png`、`10k-compare-pan-left.png`、`10k-compare-esc-return.png`。
- **剩余**：Ctrl 多选 2–4 张后进入对比的入口仍未复现——`10k-multiselect-ctrl-attempt.png` 显示 Ctrl 点击多张后只选中 1 张（`synthetic_00002_city_合成.jpg`），UIA 视图内点击无法构成多选；不能把去重组「对比此组」当作多选入口的验证。真实 Ctrl/Shift 点击与框选行为待手动验收。
- **验收（剩余）**：手动 Ctrl/Shift 选中 2–4 张 →「对比选中」进入对比，栏数与选中数一致；Esc 返回后选择与滚动保持；Ctrl 多选在 1 张时给出不可对比的明确提示。

## F-004 以图找相似

> 第三轮已实现（核心 `similar_to_anchor` + 「查找相似」结果视图），见 [implemented.md](../implemented.md)。

## F-005 分组浏览与时间线

> 第三轮已实现（吸顶分组头 + 折叠，按修改/拍摄时间、来源、评级），见 [implemented.md](../implemented.md)。

- **第四轮验证（2026-10-30）**：10 万库（100000 张合成库）分组浏览实测——修改时间/拍摄时间/来源/评级四种分组菜单可用，组头显示名称与数量、滚动吸顶、折叠展开、连续滚动均正常；600s 连续滚动采样工作集约 145MB、私有约 190MB，无增长（`100k-scroll-memory.csv`，无表头，30 行，第一列 20–600s）。10k 副本（缩略图缓存可用、源目录离线）分组/滚动视觉通过。证据 `.kanalib/r4_task0/screens/100k-*.png`、`10k-group-*.png`、`uia-100k-grouped.txt`、`uia-100k-menu.txt`、`logs/100k-group.log`。
- **剩余/边界**：10 万合成库的来源目录 `C:\bench` 已不存在（日志 `source inaccessible: C:\bench`，缩略图 `thumb missing and could not rebuild`），网格只显示占位符；所以本轮只证明分组布局、行级虚拟化与内存稳定，不能证明 10 万张真实缩略图的滚动/解码。时间线的日/月分段仍未单独做（按修改/拍摄时间分组已覆盖大部分场景）。
- **验收（剩余）**：在缩略图可访问的 10 万库上重复吸顶/折叠/连续滚动并记录帧率与内存；确认 600s 长滚动下缩略图缓存不越界（F-060 预算）。

