# Kanalib 功能登记与调研

> 本文件是入口索引；条目已按职责拆分到 [`docs/backlog/`](backlog/README.md)，不要在这里继续堆条目。

## 文档地图

| 入口 | 内容 |
|---|---|
| [backlog/README.md](backlog/README.md) | 调研准则、状态定义、文档地图、推荐顺序、使用约定 |
| [backlog/registry.md](backlog/registry.md) | 全量 F-xxx 登记表 |
| [backlog/implemented.md](backlog/implemented.md) | 已实现归档与验证 |
| [backlog/pending/overview.md](backlog/pending/overview.md) | 未完成项速览 |
| [backlog/pending/](backlog/pending/) | 按职责拆分的未完成项详情 |
| [backlog/constraints.md](backlog/constraints.md) | 安全/性能/缓存/许可/本机脚本等全局约束 |
| [backlog/parallel-plan.md](backlog/parallel-plan.md) | 并行开发矩阵与第一轮任务包 |
| [backlog/round-2.md](backlog/round-2.md) | 第二轮任务包（打标/搜索/媒体） |
| [backlog/component-research.md](backlog/component-research.md) | 非 UI 现成组件调研 |
| [backlog/candidate-pool.md](backlog/candidate-pool.md) | 候选池 |
| [backlog/references.md](backlog/references.md) | 参考与本地依据 |

## 使用约定（摘要）

- 新功能先检索活跃、许可明确的开源项目，优先借鉴验证过的设计，不复制来源不明代码；每个条目补「调研来源 / 候选对比 / 复用结论」。
- 新需求先加 `registry.md`，再写 `pending/` 对应文件；实现后移入 `implemented.md`。
- 派任务先看 `parallel-plan.md`；UI 单一车道，schema/sidecar 单一负责人。
- 安全、性能、缓存、迁移、许可与本机 PowerShell 7 约束见 `constraints.md`。
