# 全局约束（不要回退）

> 上级索引：[../FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md) · 文档地图：[README](./README.md)

- **安全**：绝不自动删除；删除只进 Windows 回收站且仅限本地固定磁盘；保护路径优先保留且不可移入回收站；只读来源不写回；移除来源/资产只动 DB，不改磁盘文件。
- **性能**：100k 历史基线 151.5fps / present p99 15.84ms / 峰值 329MB；搜索以「同一份库上的实测上限」为口径（2026-10-27 决定：`p95 <150ms` 不再是硬门禁）——当前记录：名称排序 p95 133ms 达标，命中全库的非名称排序 p95 276–304ms 为已知上限，瓶颈是临时 B 树排序；数字口径与用的哪份库见 [pending/search-organization.md](pending/search-organization.md)；相似扫描 100k 约 2.7–3.0s；冷导入 100k 42.3min（ffmpeg 阶段 1）。任何重写不得劣于这些记录值，或必须给出新的实测上限与原因；引用数字必须说明用的是哪份库。
- **缓存/内存**：所有 `img()` 必须显式 `.image_cache(&thumb_cache)`，不得回退 gpui 全局资产缓存；内存 LRU 128 项；网格用 t256、大图标/检查器用 512；磁盘预算 10GB、缓存键 `v4`（F-064 起；v3 文件不命中，旧文件由 10GB 预算淘汰；查看器 sRGB 预览在 `thumbs/previews/`，不计缩略图预算）、`thumb_access`/`thumb_sources` 保持一致。
- **UI 固定规格**：6 种视图；图标格固定图片区与行高（大 288/268、中 160/150、小 128/112，行高 +68），`Contain` 全图不裁切；小图标九宫格；无平铺/旧大图标；无幻灯片；查看器内联，不做全窗口 overlay；侧栏 48+260（200–420）；底栏 26px；主题/设置入口在活动栏底部（主题在上、设置在下），标题栏不再放主题按钮。
- **UI 重构边界（F-073）**：主图库网格与查看器/检查器布局保持，不因视觉重构重排；重点在 UI token、组件状态、侧栏/标签/设置/去重/搜索等外围界面与可访问性；不得改变数据语义/schema；参考竞品只借鉴原则，不复制品牌资产、图标或 GPL 代码。
- **UI 确认层（F-073，2026-11-04）**：破坏性/不可逆操作统一走内建 `ConfirmDialog`/`Dialog`（Esc、焦点陷阱、返回焦点、`.destructive()` 危险按钮），不再用 `rfd::AsyncMessageDialog` 或直接执行；「清理孤儿缓存」「恢复数据库备份」「重新扫描图库」分别由 `cache_cleanup_confirm`/`restore_confirm`/`rescan_confirm` 会话态驱动，确认后才调用原业务逻辑。禁用菜单项保持固定标签，原因走 `MenuItem::disabled_reason`（tooltip + UIA 描述）；`.keys(...)` 只标 `cx.bind_keys` 真实绑定的键，不编造键位。
- **依赖/构建**：gpui-pre / gpui-pre-platform 0.3.10；Rust 1.98；`vendor/ely-gpui-components` 为上游 main（`dbfdf0c`）workspace，其 `ely-*` crate 已合并进本仓库 workspace，`default-members` 仅 `.`、`crates/kanalib-core`、`crates/kanalib-ui`，日常用 `cargo test`/`cargo check`；`cargo test --workspace` 会连带 vendor 的 Ely 测试。`vendor/gpui-pre-windows` 为 0.3.10 + 本地 ClearType/Atlas 取证/窗口拖动补丁。
- **迁移/持久化**：`ui.view_mode` 旧键 `icons-xl`/`icons-lg`/`tiles` 需映射；`ui.sidebar_width`/`ui.sidebar_collapsed`/`ui.pane_mode`；旧 `ui.density` 兜底；缩略图缓存 `v4`（F-064；v3 文件不命中并由 10GB 预算淘汰）；ffmpeg 路径/env/回退顺序不可打乱。
- **许可**：项目 MIT；ffmpeg 当前 `runtime/ffmpeg` 为 LGPL shared、`tools/ffmpeg` GPL 仅本地开发，不得把 GPL 随包；`rawloader`/`libheif` 为 LGPL、`epub` 为 GPL-3.0，引入前必须做分发审查；crate 版本落库前复核。
- **搜索/标签语义**：类别、`group:`、`path:`、`size:`、`date:`、`is:failed/damaged`、比较符与 warnings 保持；标签别名/合并后旧名仍可搜；非本地 `source:` 仍明确提示不支持。
- **来源/数据**：本地优先，多来源=多本地文件夹；SQLite WAL；恢复/备份前独占；远程源只读；`docs/FFMPEG.md` 为 ffmpeg 细节的权威记录。
- **备份（F-061）**：默认写入 `.kanalib/backups`；备份前 WAL checkpoint，使用 rusqlite backup；备份库必须 `integrity_check=ok` 且关键表计数一致；拒绝覆盖已有目标；扫描中拒绝；失败不修改源库；备份为单文件（DELETE 日志模式）。自动定时/恢复仍未做，不要把手动备份误当恢复方案。
- **日志/诊断（F-062）**：正常 GUI 默认 `.kanalib/logs/kanalib.log`（10MiB/5 个/14 天，`KANALIB_LOG_*` 可调）；`KANALIB_LOG_FILE` 精确覆盖、headless 写 `.kanalib/headless.log`；诊断快照只读，脱敏 URL 凭据与 token 类键值；不要在日志/诊断中输出环境变量全量或凭据。
- **来源监听/排除（F-030/31/32）**：监听只改 DB/内存，绝不删磁盘；只有来源可访问且 `NotFound` 时才移除记录；离线/权限/不确定一律保留；被排除但仍在磁盘上的文件不能被当作重命名来源；无过滤配置走 walkdir 快路径，启用过滤时关闭 gitignore 语义；默认 `include_hidden=true` 保持旧扫描结果。
- **忽略/移除（F-037，第三轮）**：忽略判定只有一套——`scan_filter.rs` 编译规则 + `state.rs` 统一入口，重扫与监听共用；忽略列表（路径/SHA1/glob）只写 `settings` JSON，**不得为它另加 schema 迁移**；「从图库移除」只动 DB，磁盘文件必须原样保留，恢复后重新可导入。
- **扫描进度（F-063，第三轮）**：`ScanProgress`/`ScanSummary` 按 `SCAN_PROGRESS_INTERVAL` 节流写共享状态，禁止每文件刷新；只有活跃阶段计算吞吐/ETA，非活跃阶段报 0；UI 只读快照，不得回写扫描状态。
- **保护路径/只读来源（F-049）**：唯一判定入口 `ProtectedMatcher`；`dedup.protected_paths` 仍是 settings JSON，无 schema；普通路径按目录前缀、含 `* ? [ ] { }` 的条目按 glob（Windows 大小写/分隔符等价，目录命中保护子树）。保护项优先作为 keeper 且永远不可移入回收站；单组拒绝、批量跳过并计 `protected_skipped`。Immich/WebDAV 与 DB 中显式只读的本地 locator 前缀一律拒绝；扫描器自动创建的本地 FK 记录（`read_only=1` + `{"path": locator}`）仍视为可写，避免旧库整体变只读。
- **ICC/色彩管理（F-064）**：`lcms2` 6.2 `static`（MIT，自编 Little CMS，无系统 DLL）把非 sRGB ICC/EXIF Adobe RGB 统一到 sRGB；ffmpeg 缩略图后处理与 image 解码路径共用同一转换，查看器原图后台生成 `thumbs/previews` sRGB 预览（切图/关闭清理，不计预算）。无 ICC/sRGB 直通，非法 ICC 只降级不失败。性能口径：1k 冷缩略图 untagged 11.30s、sRGB-ICC 11.39s、P3 16.35s，profile probe 0.07ms/张，ffmpeg 后处理并行 3.02ms/张（200 样本）；真实广色域样本与 GUI 查看器未验收，不得写成通过。
- **第六轮 UI 持久化（F-015/F-047/F-068）**：`search.history` 与 `dedup.last_trash` 存 settings JSON，无 schema；复制图片写 PNG + CF_DIB（失败回退路径），远端只读走回退；回收站入口只打开系统回收站，不做应用内撤销。
- **会话内撤销（F-067）**：`undo.rs` 有界栈默认 50 条，仅内存、不跨重启、不加 schema；覆盖收藏/星级/手工标签，批量操作算一条；自动标签移除保存 category/score/vocab_index/origin 并精确恢复；删除/回收站、去重评审、来源移除、文件系统操作不可撤销。UI 入口 `Ctrl+Z`（排除 `ElyInput` 文本输入）、命令面板「撤销上一步」、底栏「可撤销 N」；撤销失败保留栈顶，不得伪装成功。
- **UI 结构（第三轮拆分）**：`crates/kanalib-ui` 已拆成 `app.rs`、`views/`、`sidebar.rs`、`inspector.rs`、`settings.rs` 等模块；`KanalibApp` 字段、命令面板注册与底栏接线只在 `app.rs` 写，`lib.rs` 只保留 actions、共享枚举与模块声明。多个 UI 任务并行时按模块分工，禁止同时改 `app.rs`。
- **便携打包（F-065 部分）**：`scripts/build_portable.ps1` 强制 pre-flight、LGPL 探测与 `PACKAGE-MANIFEST.txt`（SHA256/版本/ffmpeg 状态），拒绝 GPL；`.kanalib` 数据目录不得进入包；MSI/签名/自动更新仍未实现。
- **本机脚本**：PowerShell 7.6.7（`pwsh`）已安装，所有 `.ps1` 优先用 `pwsh -NoLogo -NoProfile` 执行；只有无 `pwsh` 时才回退 5.1，并注意 LF/here-string 坑。
