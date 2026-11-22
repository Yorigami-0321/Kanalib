# Kanalib 功能登记与调研

> 本文件是入口索引；条目已按职责拆分到 [`docs/backlog/`](backlog/README.md)，不要在这里继续堆条目。

## 文档地图

| 入口 | 内容 |
|---|---|
| [backlog/README.md](backlog/README.md) | 调研准则、状态定义、文档地图、推荐顺序、使用约定 |
| [TESTING-PROCESS.md](TESTING-PROCESS.md) | 轻量测试流程：测试分层、风险定档、改动类型矩阵、门禁与反过度测试约束 |
| [PLAN.md](PLAN.md) | 项目总体调研与实施方案：模型事实、架构、数据模型、索引/缩略图/去重、sidecar、来源接入、阶段与基准 |
| [FFMPEG.md](FFMPEG.md) | FFmpeg 许可与进程模型（F-040）：LGPL/GPL 边界、二进制发现、批处理进程池与冷导入基准 |
| [backlog/registry.md](backlog/registry.md) | 全量 F-xxx 登记表 |
| [backlog/implemented.md](backlog/implemented.md) | 已实现归档与验证 |
| [backlog/pending/overview.md](backlog/pending/overview.md) | 未完成项速览 |
| [backlog/pending/](backlog/pending/) | 按职责拆分的未完成项详情 |
| [backlog/constraints.md](backlog/constraints.md) | 安全/性能/缓存/许可/本机脚本等全局约束 |
| [backlog/component-research.md](backlog/component-research.md) | 非 UI 现成组件调研 |
| [backlog/ui-refactor.md](backlog/ui-refactor.md) | UI 重构调研、参考层级（图片管理器为主，VS Code 仅局部参考）、设计 token、组件状态与验收基线 |
| [backlog/candidate-pool.md](backlog/candidate-pool.md) | 候选池 |
| [backlog/references.md](backlog/references.md) | 参考与本地依据 |

## 图片墙文档（F-080–F-083）

> 代码是事实来源；下列文档只保留仍然有效的规定。

| 文档 | 作用 |
|---|---|
| [backlog/photo-wall-status.md](backlog/photo-wall-status.md) | **现状与路线图**：已实现功能面、验证基线、已知问题、下一批任务 |
| [backlog/photo-wall-contract.md](backlog/photo-wall-contract.md) | 静态导出契约 v1：目录布局/index.json/分片/item 字段/id 规则/前端读取规则/不变量 |
| [backlog/photo-wall-v3.md](backlog/photo-wall-v3.md) | 视觉与交互规格：空间画布/黑白竖排缝/动态强调色/两级入口/全屏详情 + 硬规则 |
| [backlog/cloud-photo-wall.md](backlog/cloud-photo-wall.md) | 计划与决策记录：产品边界、W1–W5 决策、F-083 部署 |
| [backlog/pixiv-integration.md](backlog/pixiv-integration.md) | Pixiv 归档能力合并总计划（F-084–F-086、F-089）：box 入库过滤 → 网页轻量后端 → 桌面工作台；不含数据迁移 |
| [backlog/web-rail-horizontal.md](backlog/web-rail-horizontal.md) | 网页导航横向化规格（F-087）：208px 横排文字、折叠、窄屏底部条 |
| [backlog/web-dynamic-backend.md](backlog/web-dynamic-backend.md) | 网页轻量后端与动态化设计（F-088）：独立 `kanalib-web`、SQLite、API、媒体、`ApiSource`、安全与阶段 |
| [backlog/ingest-service.md](backlog/ingest-service.md) | 入库过滤服务设计（F-089）：box 上的来源适配、下载、打标、配置化黑名单（用户+标签）、R-18G 三路判定、标签不足补标、审核、HMAC 推送与签名回源 |
| [backlog/easytier-connectivity.md](backlog/easytier-connectivity.md) | 三端互联与 EasyTier 方案（F-090）：公网只读、私网写入、双 listener、魔法 DNS、ACL、secure mode、流量账、迁移与回退 |
| [backlog/private-networking-options.md](backlog/private-networking-options.md) | 私网组网备选对比（F-091）：EasyTier 主方案；Tailscale/Headscale/NetBird/Nebula/ZeroTier/Cloudflare/frp/WireGuard |
| [backlog/execution-workstreams.md](backlog/execution-workstreams.md) | 并行执行工作流：P0 契约闸门、P1–P5 车道、文件所有权、派发顺序与最低验收 |
| [backlog/p2-reuse-research.md](backlog/p2-reuse-research.md) | P2 之后可复用组件调研：Pixiv 客户端、axum/tower-http、打标 sidecar 复用、签名/队列选型、许可与版本冲突 |

## 使用约定（摘要）

- 新功能先检索活跃、许可明确的开源项目，优先借鉴验证过的设计，不复制来源不明代码；每个条目补「调研来源 / 候选对比 / 复用结论」。
- 新需求先加 `registry.md`，再写 `pending/` 对应文件；实现后移入 `implemented.md`。
- 派任务先看 [backlog/photo-wall-status.md](backlog/photo-wall-status.md) 的路线图与 `pending/overview.md`；UI 单一车道，schema/sidecar 单一负责人。
- 历史任务包（第一至七轮）已归档到 [archive/2026-11/](archive/2026-11/)，仅作追溯，不再更新；现行口径以本页与代码为准。
- 验证按 [`TESTING-PROCESS.md`](TESTING-PROCESS.md) 的改动类型矩阵与门禁执行；默认最小验证，不做过度测试。
- 安全、性能、缓存、迁移、许可与本机 PowerShell 7 约束见 `constraints.md`。
