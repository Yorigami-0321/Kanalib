# 归档文档

> 上级入口：[docs/FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md)

本目录保存已完成、被取代或只作追溯的历史文档。**归档不等于通过验收**，其中的计划、日期、数字和“待办”都不代表当前状态。

## 生效规则

- 现行口径以**代码**与 [backlog/photo-wall-status.md](../backlog/photo-wall-status.md)、[backlog/registry.md](../backlog/registry.md) 为准。
- 归档文件不再更新；如需引用其中结论，先到代码或现行文档核对。
- 相对链接已按归档路径修正，文件之间的引用仍可打开。

## 2026-11 归档清单

| 文件 | 原用途 | 归档原因 |
|---|---|---|
| [parallel-plan.md](2026-11/parallel-plan.md) | 并行开发矩阵与第一轮任务包 | 第一轮已全部合入，任务包失效 |
| [round-2.md](2026-11/round-2.md) | 第二轮任务包（打标/搜索/媒体） | 已完成并合入 |
| [round-3.md](2026-11/round-3.md) | 第三轮任务包与执行结果 | 已完成并合入 |
| [round-4.md](2026-11/round-4.md) | 第四轮任务包（备份/媒体/搜索/监听） | 已完成并合入 |
| [round-5.md](2026-11/round-5.md) | 第五轮任务包（桌面/网络源/视频） | 已完成并合入 |
| [round-6.md](2026-11/round-6.md) | 第六轮任务包（UI 小项/保护路径/ICC） | 已完成并合入 |
| [round-7.md](2026-11/round-7.md) | 第七轮任务包（F-067 撤销/历史） | 已完成并合入 |
| [photo-wall-status-2026-11-10.md](2026-11/photo-wall-status-2026-11-10.md) | 2026-11-10 前的图片墙完整开发日志（v3/v4/v5 逐轮实测） | 已重写为简版现状页，过程细节归档 |
| [photo-wall-feedback-2026-11-10.md](2026-11/photo-wall-feedback-2026-11-10.md) | 两轮实机反馈的根因与修复明细 | 已闭环，细节仅作追溯 |
| [photo-wall-audit-2026-11-11.md](2026-11/photo-wall-audit-2026-11-11.md) | 功能与旅程审计、失败路径、第二批 A 复验 | 已闭环，细节仅作追溯 |
| [photo-wall-work-report-2026-11-17.md](2026-11/photo-wall-work-report-2026-11-17.md) | 一次性工作汇报：移动端底栏、聚焦卡信息、探针调整 | 已完成；结论已并入 [photo-wall-status.md](../backlog/photo-wall-status.md) |
| [photo-wall-work-report-2026-11-21.md](2026-11/photo-wall-work-report-2026-11-21.md) | 一次性工作汇报：渐进加载、深链全库探测、检查器折叠、发布器原子输出 | 已完成；结论与回归已并入 [photo-wall-status.md](../backlog/photo-wall-status.md) |

新增归档时：把文件移入 `2026-11/`（或对应月份目录），修正相对链接，并在上表补一行。

## 2026-11-22 清理记录

已删除完全被现行文档取代的 6 个文件：`next-steps.md`、`photo-wall-design.md`、`photo-wall-focus-mode.md`、`pixiv-integration-v1.md`、`web-dynamic-backend-v1.md`、`tailscale-connectivity-v1.md`。

- 前三个在 git 历史中可追溯；后三个是未入库的草稿，已不可恢复。
- 保留 `parallel-plan.md`、`round-2..7`、`photo-wall-status-2026-11-10.md`、`photo-wall-feedback-2026-11-10.md`、`photo-wall-audit-2026-11-11.md`，因为 `implemented.md`、`photo-wall-status.md` 与 `pending/*` 仍把它们当作提交号、测试基线或闭环证据的出处。
- 现行口径仍以代码与 [backlog/](../backlog/) 为准；归档不再更新。

同日另删 `docs/backlog/tailscale-connectivity.md`（370 字节跳转存根，无任何入链）；EasyTier 主方案见 [easytier-connectivity.md](../backlog/easytier-connectivity.md)。
