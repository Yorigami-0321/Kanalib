# 全局约束（不要回退）

> 上级索引：[../FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md) · 文档地图：[README](./README.md)

- **安全**：绝不自动删除；删除只进 Windows 回收站且仅限本地固定磁盘；保护路径优先保留且不可移入回收站；只读来源不写回；移除来源/资产只动 DB，不改磁盘文件。
- **性能**：100k 历史基线 151.5fps / present p99 15.84ms / 峰值 329MB；搜索以「同一份库上的实测上限」为口径（2026-10-27 决定：`p95 <150ms` 不再是硬门禁）——当前记录：名称排序 p95 133ms 达标，命中全库的非名称排序 p95 276–304ms 为已知上限，瓶颈是临时 B 树排序；数字口径与用的哪份库见 [pending/search-organization.md](pending/search-organization.md)；相似扫描 100k 约 2.7–3.0s；冷导入 100k 42.3min（ffmpeg 阶段 1）。任何重写不得劣于这些记录值，或必须给出新的实测上限与原因；引用数字必须说明用的是哪份库。
- **缓存/内存**：所有 `img()` 必须显式 `.image_cache(&thumb_cache)`，不得回退 gpui 全局资产缓存；内存 LRU 128 项；网格用 t256、大图标/检查器用 512；磁盘预算 10GB、缓存键 `v3`、`thumb_access`/`thumb_sources` 保持一致。
- **UI 固定规格**：6 种视图；图标格固定图片区与行高（大 288/268、中 160/150、小 128/112，行高 +68），`Contain` 全图不裁切；小图标九宫格；无平铺/旧大图标；无幻灯片；查看器内联，不做全窗口 overlay；侧栏 48+260（200–420）；底栏 26px；主题按钮 `Md`+Ghost。
- **迁移/持久化**：`ui.view_mode` 旧键 `icons-xl`/`icons-lg`/`tiles` 需映射；`ui.sidebar_width`/`ui.sidebar_collapsed`/`ui.pane_mode`；旧 `ui.density` 兜底；缩略图缓存 `v3`；ffmpeg 路径/env/回退顺序不可打乱。
- **许可**：项目 MIT；ffmpeg 当前 `runtime/ffmpeg` 为 LGPL shared、`tools/ffmpeg` GPL 仅本地开发，不得把 GPL 随包；`rawloader`/`libheif` 为 LGPL、`epub` 为 GPL-3.0，引入前必须做分发审查；crate 版本落库前复核。
- **搜索/标签语义**：类别、`group:`、`path:`、`size:`、`date:`、`is:failed/damaged`、比较符与 warnings 保持；标签别名/合并后旧名仍可搜；非本地 `source:` 仍明确提示不支持。
- **来源/数据**：本地优先，多来源=多本地文件夹；SQLite WAL；恢复/备份前独占；远程源只读；`docs/FFMPEG.md` 为 ffmpeg 细节的权威记录。
- **备份（F-061）**：默认写入 `.kanalib/backups`；备份前 WAL checkpoint，使用 rusqlite backup；备份库必须 `integrity_check=ok` 且关键表计数一致；拒绝覆盖已有目标；扫描中拒绝；失败不修改源库；备份为单文件（DELETE 日志模式）。自动定时/恢复仍未做，不要把手动备份误当恢复方案。
- **日志/诊断（F-062）**：正常 GUI 默认 `.kanalib/logs/kanalib.log`（10MiB/5 个/14 天，`KANALIB_LOG_*` 可调）；`KANALIB_LOG_FILE` 精确覆盖、headless 写 `.kanalib/headless.log`；诊断快照只读，脱敏 URL 凭据与 token 类键值；不要在日志/诊断中输出环境变量全量或凭据。
- **来源监听/排除（F-030/31/32）**：监听只改 DB/内存，绝不删磁盘；只有来源可访问且 `NotFound` 时才移除记录；离线/权限/不确定一律保留；被排除但仍在磁盘上的文件不能被当作重命名来源；无过滤配置走 walkdir 快路径，启用过滤时关闭 gitignore 语义；默认 `include_hidden=true` 保持旧扫描结果。
- **忽略/移除（F-037，第三轮）**：忽略判定只有一套——`scan_filter.rs` 编译规则 + `state.rs` 统一入口，重扫与监听共用；忽略列表（路径/SHA1/glob）只写 `settings` JSON，**不得为它另加 schema 迁移**；「从图库移除」只动 DB，磁盘文件必须原样保留，恢复后重新可导入。
- **扫描进度（F-063，第三轮）**：`ScanProgress`/`ScanSummary` 按 `SCAN_PROGRESS_INTERVAL` 节流写共享状态，禁止每文件刷新；只有活跃阶段计算吞吐/ETA，非活跃阶段报 0；UI 只读快照，不得回写扫描状态。
- **UI 结构（第三轮拆分）**：`crates/kanalib-ui` 已拆成 `app.rs`、`views/`、`sidebar.rs`、`inspector.rs`、`settings.rs` 等模块；`KanalibApp` 字段、命令面板注册与底栏接线只在 `app.rs` 写，`lib.rs` 只保留 actions、共享枚举与模块声明。多个 UI 任务并行时按模块分工，禁止同时改 `app.rs`。
- **便携打包（F-065 部分）**：`scripts/build_portable.ps1` 强制 pre-flight、LGPL 探测与 `PACKAGE-MANIFEST.txt`（SHA256/版本/ffmpeg 状态），拒绝 GPL；`.kanalib` 数据目录不得进入包；MSI/签名/自动更新仍未实现。
- **本机脚本**：PowerShell 7.6.7（`pwsh`）已安装，所有 `.ps1` 优先用 `pwsh -NoLogo -NoProfile` 执行；只有无 `pwsh` 时才回退 5.1，并注意 LF/here-string 坑。
