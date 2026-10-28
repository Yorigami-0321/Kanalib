# 性能、可靠性与交付

> 上级索引：[../README.md](../README.md) · 文档地图：[README](../README.md) · 第三轮任务包：[round-3.md](../round-3.md)

## F-061 数据库自动备份/恢复与维护

- **依据**：当前备份只在 schema 迁移前执行（`db.rs` 的 `migration_version`/`backup_database`）；`PLAN.md` 没有定期备份 UI。
- **已完成（第一轮）**：`Db::backup_to` 与 `backup::backup_database`：WAL checkpoint、rusqlite backup、`integrity_check`、关键表计数、目标不覆盖、失败不破坏源库、扫描中拒绝；默认写入 `.kanalib/backups`，UI 提供「立即备份/打开备份目录」与报告；UIA 实测 160KB 备份完整。
- **剩余**：自动按日/周备份与保留 N 份；恢复流程（独占、校验、提示重启）；大库备份进度；VACUUM/维护入口；导出/导入设置。
- **验收（剩余）**：保留策略生效；备份可在另一数据目录恢复并正常打开；恢复失败不破坏现有库；大库备份不长时间阻塞 UI。
- **依赖/风险**：恢复时的进程独占；大库复制耗时；与 F-062 的诊断信息共享。

## F-062 GUI 日志与诊断页

> 第一轮已实现（滚动日志 + 设置页诊断区/复制诊断），见 [implemented.md](../implemented.md)。本节不再保留待办；日志尾部展示可随恢复/诊断后续扩展。

## F-063 扫描进度/吞吐/ETA

- **依据**：`PLAN.md` §6.1/§12；扫描已有“停止”按钮，但状态原本只有字符串。
- **已完成（第三轮，`fec35bf`）**：`ScanProgress`/`ScanPhase`/`ScanSummary`/`ScanSourceSummary` 真正接入遍历、哈希、缩略图、打标、对账各阶段；`ScanProgressReporter` 按 `SCAN_PROGRESS_INTERVAL` 节流写共享状态，只有活跃阶段计算吞吐与 ETA；扫描结束给出结构化摘要（新增/更新/跳过/移除/失败/耗时，逐来源）；`scan_progress()`/`scan_summary()` 为 UI 提供只读快照；停止后状态置为「已停止」且清空 ETA。
- **剩余**：底栏/侧栏进度条与扫描结束摘要的 UI 展示——属任务 G3，未做；10 万冷导入的阶段切换与 ETA 数字尚未在真实大库记录（单测覆盖 3 文件与多来源）。
- **验收（剩余）**：UI 进度刷新不影响扫描吞吐；10 万库冷导入可看到阶段与 ETA。
- **依赖/风险**：ETA 在哈希/缩略图/打标阶段差异很大，按阶段估算；节流窗口内不更新，UI 需容忍 0.5s 级延迟。
