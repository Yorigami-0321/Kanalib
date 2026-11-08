# Kanalib 功能登记与调研

> 本文件是入口索引；条目已按职责拆分到 [`docs/backlog/`](backlog/README.md)，不要在这里继续堆条目。

## 文档地图

| 入口 | 内容 |
|---|---|
| [backlog/README.md](backlog/README.md) | 调研准则、状态定义、文档地图、推荐顺序、使用约定 |
| [TESTING-PROCESS.md](TESTING-PROCESS.md) | 轻量测试流程：测试分层、风险定档、改动类型矩阵、门禁与反过度测试约束 |
| [backlog/registry.md](backlog/registry.md) | 全量 F-xxx 登记表 |
| [backlog/implemented.md](backlog/implemented.md) | 已实现归档与验证 |
| [backlog/pending/overview.md](backlog/pending/overview.md) | 未完成项速览 |
| [backlog/pending/](backlog/pending/) | 按职责拆分的未完成项详情 |
| [backlog/constraints.md](backlog/constraints.md) | 安全/性能/缓存/许可/本机脚本等全局约束 |
| [backlog/parallel-plan.md](backlog/parallel-plan.md) | 并行开发矩阵与第一轮任务包 |
| [backlog/round-2.md](backlog/round-2.md) | 第二轮任务包（打标/搜索/媒体） |
| [backlog/round-3.md](backlog/round-3.md) | 第三轮任务包与执行结果 |
| [backlog/round-4.md](backlog/round-4.md) | 第四轮任务包（备份/媒体/搜索/监听） |
| [backlog/round-5.md](backlog/round-5.md) | 第五轮任务包（桌面基础/网络源/视频） |
| [backlog/round-6.md](backlog/round-6.md) | 第六轮任务包（UI 小项/保护路径/ICC） |
| [backlog/round-7.md](backlog/round-7.md) | 第七轮任务包（F-067 撤销/历史） |
| [backlog/component-research.md](backlog/component-research.md) | 非 UI 现成组件调研 |
| [backlog/ui-refactor.md](backlog/ui-refactor.md) | UI 重构调研、参考层级（图片管理器为主，VS Code 仅局部参考）、设计 token、组件状态与验收基线 |
| [backlog/candidate-pool.md](backlog/candidate-pool.md) | 候选池 |
| [backlog/cloud-photo-wall.md](backlog/cloud-photo-wall.md) | 云端无界图片墙计划（F-080–F-083）与本地条目精简提案 |
| [backlog/references.md](backlog/references.md) | 参考与本地依据 |

## 使用约定（摘要）

- 新功能先检索活跃、许可明确的开源项目，优先借鉴验证过的设计，不复制来源不明代码；每个条目补「调研来源 / 候选对比 / 复用结论」。
- 新需求先加 `registry.md`，再写 `pending/` 对应文件；实现后移入 `implemented.md`。
- 派任务先看 `parallel-plan.md`；UI 单一车道，schema/sidecar 单一负责人。
- 验证按 [`TESTING-PROCESS.md`](TESTING-PROCESS.md) 的改动类型矩阵与门禁执行；默认最小验证，不做过度测试。
- 安全、性能、缓存、迁移、许可与本机 PowerShell 7 约束见 `constraints.md`。
