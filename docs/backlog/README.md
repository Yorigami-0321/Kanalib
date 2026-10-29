# Kanalib 功能登记与调研（backlog）

> 新功能登记、调研与并行派发的统一入口；条目按职责拆分在本目录。本页保留调研准则、状态定义、文档地图与推荐顺序。
>
> 上级入口：[docs/FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md)

## 调研与实现准则

> 遇到不熟悉的功能，不要立即从零实现。先检索 GitHub 上维护活跃、许可证明确的同类开源项目，比较实现思路、依赖和已知问题。优先借鉴经过验证的设计，不要直接复制不兼容或来源不明的代码。

执行清单：

1. **先检索**：找 2–5 个同类开源项目，记录仓库地址、最近提交/发布、Issue 活跃度、许可证、平台与依赖；不要只看 README。
2. **再比较**：对比实现思路、依赖体积、许可兼容性、已知问题/限制、维护成本和本项目的适配点。
3. **后决策**：明确选择「直接复用」「借鉴设计后自研」「完全自研」；沿用已验证的设计优先，不重复造轮子。
4. **复制边界**：不直接粘贴不兼容或来源不明的代码；借鉴时遵守许可证与署名要求，必要时按本项目接口重写。
5. **有数据再拍板**：性能/算法类选型先用本项目真实数据做小基准，不因参照项目“用了”就照搬；结论与数字写回对应 F-xxx。
6. **留痕**：每个新功能在登记时补充「调研来源 / 候选对比 / 复用结论」；没有合适项目时，写明为什么自研。

## 状态与优先级

| 状态 | 含义 |
|---|---|
| 待确认 | 需求已登记，方案或范围仍需确认 |
| 待实现 | 方案已确认，尚未动代码 |
| 实现中 | 已落地一部分，剩余阶段见详情 |
| 已实现 | 已合入并验证；只保留约束/回归要点 |
| 待移除 | 已实现但产品决定下线 |
| 候选 | 未进入推荐批次，等待评估 |
| 远期 | PLAN 后续阶段或依赖外部条件 |
| 搁置 / 不做 | 暂不推进 / 明确排除 |

优先级：高 = 影响主浏览路径或用户明确要求；中 = 有明确收益但不阻塞；低 = 锦上添花。

## 文档地图

| 文档 | 内容 |
|---|---|
| [registry.md](registry.md) | 全量 F-xxx 登记表（状态/优先级/依据） |
| [implemented.md](implemented.md) | 已实现归档：关键约束 + 验证汇总 |
| [pending/overview.md](pending/overview.md) | 未完成项速览（缺口/优先级/依赖） |
| [pending/browse.md](pending/browse.md) | 浏览与查看：F-003/004/005 |
| [pending/search-organization.md](pending/search-organization.md) | 搜索与组织：F-012/013/014 |
| [pending/tags.md](pending/tags.md) | 标签与打标：F-021/023/024（已归档，本域暂无未完成项） |
| [pending/library-sources-formats.md](pending/library-sources-formats.md) | 图库/来源/格式：F-030–F-033、F-035–F-037、F-040 |
| [pending/dedup.md](pending/dedup.md) | 去重：F-045 |
| [pending/ops.md](pending/ops.md) | 性能/可靠性与交付：F-061–F-063 |
| [candidate-pool.md](candidate-pool.md) | 低优先级/远期候选 |
| [constraints.md](constraints.md) | 安全/性能/缓存/许可/本机脚本等全局约束 |
| [parallel-plan.md](parallel-plan.md) | 并行开发矩阵 + 第一轮可直接派发的任务包 |
| [round-2.md](round-2.md) | 第二轮任务包：打标质量/队列、搜索性能、媒体格式与阶段 2 |
| [round-3.md](round-3.md) | 第三轮任务包：组织、相似/浏览、扫描运维与 UI 拆分 |
| [round-4.md](round-4.md) | 第四轮任务包：备份恢复、媒体评估、搜索上限与监听收尾 |
| [component-research.md](component-research.md) | 非 UI 现成组件调研（可直接采用/评估/自研） |
| [references.md](references.md) | 外部参考与本地依据 |

## 推荐顺序

> 只表达依赖与收益，不代表排期。UI 改版与 ffmpeg 阶段 1 已完成，除非回归不重开。

| 批次 | 目标 | 条目 |
|---|---|---|
| 第 1 批 | 图库可信度与规模 | F-012（剩余：非名称排序 p95）、F-030、F-032、F-061、F-063、F-040（阶段 2 剩余） |
| 第 2 批 | 整理工作流 | F-003、F-004、F-005、F-013、F-014、F-033（剩余：HEIC/RAW/PSD）、F-036、F-037、F-045 |
| 第 3 批 | 原计划阶段 2 | F-035 |
| 候选池 | 低优先级/远期 | F-015、F-016、F-025、F-026、F-038、F-039、F-046、F-047、F-048、F-049、F-064、F-065、F-066、F-067、F-068 |

## 使用约定

- **新需求**：先在 [registry.md](registry.md) 加一行，再到 `pending/` 对应职责文件写需求/方案/验收/风险；调研遵循本页准则，补「调研来源/候选对比/复用结论」。
- **实现后**：从 `pending/` 移入 [implemented.md](implemented.md)，只保留关键约束与验证。
- **派发任务**：先看 [parallel-plan.md](parallel-plan.md)；UI 只有一条车道，schema/sidecar 单一负责人。
- **不写工时/排期**；代码行号会漂移，按函数名搜索。
- **文档自检**：`bash scripts/check_backlog_docs.sh` 校验 registry 的 F-xxx 不重复不丢失、implemented 与 pending 的状态不互相矛盾；每轮收口后重跑一次。
