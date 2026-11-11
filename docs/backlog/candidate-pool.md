# 候选池（低优先级/远期）

> 上级索引：[../FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md) · 文档地图：[README](./README.md)

| ID | 功能 | 说明 | 依据 |
|---|---|---|---|
| F-016 | CLIP 语义搜索 | 文本→图像向量检索，与标签搜索混合；需要模型、向量存储与 GPU 预算 | PLAN 阶段 4 |
| F-026 | XMP/标签写回 | 显式 opt-in 的 XMP sidecar 导出/写回；首版仍不修改原图 | PLAN §2.2/阶段 4 |
| F-048 | 相似扫描分桶/大规模优化 | 当前 10 万约 2.7s；更大库或更低阈值需要分桶/索引 | PLAN §11 |
| F-065 | 安装包/签名/自动更新 | MSI/签名/升级通道；便携版已具备，且第一轮已加 pre-flight/许可证/SHA256 清单（`PACKAGE-MANIFEST.txt`） | PLAN 阶段 3 |
| F-066 | 视频/小说扩展 | ffmpeg/mpv 视频缩略图与播放；EPUB/PDF 文本索引 | PLAN 阶段 4 |

> 非 UI 候选（F-016/F-026/F-065/F-066 等）的可复用组件、许可与建议见 [component-research.md](component-research.md)。

> 第六轮任务 A/B/C（2026-11-02）已实现并归档：F-015、F-025、F-046、F-047、F-049、F-064、F-068，见 [implemented.md](implemented.md)。本池只保留尚未进入推荐批次的候选/远期项。

> 第七轮立项移出候选池：F-067（会话内撤销/历史）由任务 A/B 实现并归档，见 [round-7.md](../archive/2026-11/round-7.md) 与 [implemented.md](implemented.md)。

---
