# 去重

> 上级索引：[../README.md](../README.md) · 文档地图：[README](../README.md) · 第三轮任务包：[round-3.md](../round-3.md)

## F-045 相似阈值自定义与预览

- **依据**：`PLAN.md` §6.2 的 6/10/14 预设；当前设置页与去重页只提供三档。
- **已完成（第三轮核心，`f25a1b0`）**：`dedup::preview_similar_threshold` 与 `AppState::preview_similar_threshold`——返回 `SimilarThresholdPreview { group_count, member_count, chain_risk, chain_groups, sampled_assets, total_assets, sampled }`；阈值统一 `clamp_similar_threshold` 到 0–32（`MIN/MAX_SIMILAR_THRESHOLD`、`SIMILAR_THRESHOLD_PRESETS=[6,10,14]`），低阈值给出链式合并风险标记；≤4096 张精确计算，更大库抽 4096 个种子（10 万约 4.1%），检测到链式合并且预估组数 >2、≥25% 资产入组时回退精确全扫（10 万 1.8–2.6s）。改阈值只写阈值，不碰评审状态。
- **剩余**：设置页/去重页的 0–32 滑杆与预估数量展示、三档快捷按钮保留、临时值与默认值区分——属任务 G2，未做。预览是同步调用，UI 必须在后台线程调用。
- **验收（剩余）**：滑杆即时预览且不阻塞渲染；切换阈值不清空已审核状态；低阈值显示风险提示。
- **依赖/风险**：稀疏库若含大量长链组，抽样仍可能温和高估组数（25% 阈值是启发式）；偏差记录见 [round-3.md](../round-3.md)。
