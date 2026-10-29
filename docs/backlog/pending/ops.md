# 性能、可靠性与交付

> 上级索引：[../README.md](../README.md) · 文档地图：[README](../README.md) · 第三轮任务包：[round-3.md](../round-3.md)

## F-061 数据库自动备份/恢复与维护

- **依据**：当前备份只在 schema 迁移前执行（`db.rs` 的 `migration_version`/`backup_database`）；`PLAN.md` 没有定期备份 UI。
- **已完成（第一轮）**：`Db::backup_to` 与 `backup::backup_database`：WAL checkpoint、rusqlite backup、`integrity_check`、关键表计数、目标不覆盖、失败不破坏源库、扫描中拒绝；默认写入 `.kanalib/backups`，UI 提供「立即备份/打开备份目录」与报告；UIA 实测 160KB 备份完整。
- **已完成（第四轮任务 A，核心 + 设置页入口）**：
  - **计划/保留**：`backup::BackupPlan`（`enabled`/`frequency`/`keep_files`/`keep_days`）JSON 存 `settings` 键 `backup.plan`，不加 schema；`BackupPlan::{parse,from_json,to_json,sanitized,next_due_at,is_due}`、`backup::{load_plan,save_plan}`。到期口径为“距最近一份本应用生成的备份满 24h/7d”；启用且从未备份时立即到期；时钟回拨按到期处理。`backup::prune_backups` 只删除 `backups/kanalib-YYYYMMDD-HHMMSS(-N).db`，至少保留最新一份且永不删除 `kanalib-pre-restore-*.db`/用户文件；`latest_managed_backup`/`is_managed_backup_name` 为纯路径辅助。
  - **调度**：`AppState::new` 在正常 GUI（`watch_allowed` 且未设 `KANALIB_DISABLE_BACKUP_SCHEDULER`）启动 `kanalib-backup` 轻量线程，每 30s 调用 `run_scheduled_backup`；headless/测试构造不启动。扫描或打标（`tag_worker`）运行中跳过并写 `BackupStatus.skipped_reason`，不阻塞 UI；`Drop` 通知线程退出。手动/自动共用 `AppState::backup_now`：`backup_lock` 串行、扫描中拒绝、`Db::backup_to` 的 WAL checkpoint/完整性/关键表校验、计划启用时顺带按保留策略清理。
  - **恢复**：`AppState::restore_backup` 独占检查（扫描/打标中拒绝）→ `backup::stage_restore` 只读校验候选（`Db::inspect_database`：`PRAGMA integrity_check`、`user_version`、关键表计数；缺 `settings`/`assets`、较新 schema、完整性失败拒绝；较旧 schema 警告并说明重启会走现有迁移）→ 对当前库执行 `backups/kanalib-pre-restore-*.db` 安全备份 → 候选复制为 `kanalib.restore-pending.db` 并写 `kanalib.restore-pending.json`（含 schema 与关键表计数）。运行期不替换数据库（Windows 句柄/watcher/缩略图内存状态会过期）；`AppState::open` 在任何 `Db::open` 前调用 `backup::apply_pending_restore` 完成替换并复核计数，失败自动回滚当前库；成功后提示用户重启。`backup::pending_restore`/`discard_pending_restore` 供取消/展示；校验失败会清理坏请求、当前库不变。
  - **UI（settings.rs）**：自动备份开关、日/周频率、保留份数、可选按天清理、计划摘要、下次备份/待恢复提示、最近备份与跳过原因；恢复入口用 `rfd::AsyncFileDialog` 异步选文件 + `rfd::AsyncMessageDialog` 确认，后台执行校验；复用 `backup_running`/`last_backup`/`settings_notice`，未改 `app.rs`。E1 可用 `AppState::backup_status()`（含 `plan`/`next_due`/`skipped_reason`/`pending_restore`）接底栏、命令面板与启动提示。
  - **UI 收口（第四轮 E1，`02e4960`）**：`app.rs` 命令面板新增「备份与恢复」组——立即备份、恢复备份…、取消待恢复、打开设置·数据库备份；启动时若检测到 `pending_restore`，写 `pending_restore_hint` 并在底栏/设置页提示“重启后生效，可在设置页取消”，不自动替换运行中的库。`settings.rs` 的备份区实际截图见 `.kanalib/r4_e1/screens/settings-backup-section.png`、`settings-backup-plan-enabled.png`、`settings-backup-now.png`；损坏候选的拒绝提示见 `restore-corrupt-failure.png`。恢复仍走“校验 + safety backup + 下次启动应用”，运行期不换库；取消待恢复复用 `backup::discard_pending_restore`。边界：大库备份进度/取消、VACUUM、设置导入导出仍未做。
  - **并发边界**：`backup_lock` 只保证单进程内备份/恢复串行；扫描线程与打标线程通过 `shared.scanning`/`tag_worker` 判断，备份在扫描中始终拒绝、恢复在扫描/打标中拒绝；恢复的“切换”发生在重启后首个数据库连接之前，运行中的应用继续读写当前库直到退出；多实例互斥仍依赖 F-039。
- **验收记录**：`cargo test -p kanalib-core` 123 passed / 11 ignored；`cargo test -p kanalib-ui` 34 passed；`cargo test --workspace` 通过（bin 3 passed）；`cargo check --workspace` 无代码警告（仅 vendor profile 配置提示）。新增覆盖：计划日/周/跨天/时钟回拨、保留只删 `kanalib-*` 且保最新、WAL 备份恢复、候选损坏/缺关键表/标记计数不符不破坏当前库、扫描/打标中拒绝、恢复后安全备份存在、重启应用恢复。
- **剩余/边界**：大库备份进度与取消、VACUUM/维护入口、导出/导入设置、备份加密/云备份未做；待恢复请求目前只能通过重启应用或 E1 增加“取消待恢复”入口清理（核心已提供 `discard_pending_restore`）；恢复旧 schema 备份会触发现有迁移（迁移前另存备份）；恢复前安全备份不参与自动保留清理。
- **依赖/风险**：恢复时的进程独占（当前仅单实例内）；大库复制耗时；与 F-062 的诊断信息共享。

## F-062 GUI 日志与诊断页

> 第一轮已实现（滚动日志 + 设置页诊断区/复制诊断），见 [implemented.md](../implemented.md)。本节不再保留待办；日志尾部展示可随恢复/诊断后续扩展。

## F-063 扫描进度/吞吐/ETA

> 第三轮已实现（核心结构化进度 + 底栏/设置页展示），见 [implemented.md](../implemented.md)。10 万库冷导入的阶段/ETA 数字尚未在大库实测记录。

