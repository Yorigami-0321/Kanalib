# 已实现归档（关键约束与验证）

> 上级索引：[../FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md) · 文档地图：[README](./README.md)

| ID | 最终状态与关键约束 | 验证 |
|---|---|---|
| F-001 | 查看菜单、列表/详细信息/内容、`PaneMode` 三态、快捷键与旧 `ui.density` 迁移已实现；最终视图形态由 F-009 收敛 | UI 测试与 `.kanalib/f001_*.png` |
| F-002 | 查看器保留缩放/平移/旋转/全屏/原图切换；默认入口已改为内联（F-007）；幻灯片已移除（F-008）；切图/关闭时清理图片资源 | `.kanalib/f002_*.png`，Esc/切图实测 |
| F-006 | 48px 活动栏 + 260px 面板（200–420 可拖拽）；分组折叠、宽度/折叠持久化、`Ctrl+B`、失败/待审角标；侧栏品牌与平铺操作按钮移除 | `.kanalib/f006_*.png`，UIA/重启保持 |
| F-007 | 查看器移入内容区；侧栏/检查器/标题栏/底栏保留；Esc 返回原视图与滚动；筛选后当前图不可见则关闭；`lightbox_*` 改名 `viewer_*` | `.kanalib/f007_inline.png`、`f007_after_esc.png` |
| F-008 | 已删除 `ViewerSlideshow`、`viewer_slideshow`/`slideshow_task`、S/Space 绑定、播放按钮与提示；全屏/切图/缩放保留 | `grep` 无残留，`cargo check` 通过 |
| F-009 | `ViewMode` 收敛为 6 种：大图标 288/268、中图标 160/150、小图标 128/112、列表、详细信息、内容；三格统一 `Contain` 全图 + 名称 + 评级/星级 + 标签数，行高 `图片区+68`；新增 `grid-3x3.svg`/`IconName::Grid3x3`；删除平铺、旧大图标、平铺摘要与相关快捷键；快捷键 1/3/4/5/6/8（2/7 留空），旧 `icons-xl`/`icons-lg`/`tiles` 映射到 `icons-large`/`icons-medium` | `view_modes_are_converged_to_six` 等 6 测试，`.kanalib/f006_viewmenu.png` |
| F-010 | 搜索支持类别/`group:`/`name:`/`path:`/`size:`/`width:`/`height:`/`ratio:`/`date:`/`is:failed`/`is:damaged`/比较符/warnings；非本地 `source:` 仍提示不支持 | 单测 `extended_search_syntax...` |
| F-011 | 侧栏标签面有类别 chips（通用/角色/作品/风格/元数据/手工）、搜索、使用量/名称排序、Top N + 查看更多；Ctrl/Shift 点击追加 `tag:` | `.kanalib/f011_*.png`，筛选实测 |
| F-020 | 标签重命名/合并；`tags.alias_of`（schema v6）保留旧名别名并扁平化别名链；FTS `tags_text` 并入别名 | 单测 `renaming_and_merging_tags_keep_aliases_and_fts` |
| F-022 | 启动灌入 30,877 词表；补全为全词表前缀/子串 + 使用次数排序，最多 50 条 | `.kanalib/f022_suggest2.png` |
| F-034 | `kamadak-exif` 解析方向/拍摄时间/相机/镜头/ISO/光圈/快门/焦距；缩略图按方向旋转并交换尺寸；`captured_at` 入库；`sort:captured` 与 `date:` 过滤；GPS 未展示 | 单测 `exif_orientation...`、`exif_datetime...` |
| F-040 | 阶段 1：512 缩略图走 ffmpeg 多输入批处理池（≤32/批、120s 超时、`-protocol_whitelist file`、`max_pixels`、无窗口）；路径 env → PATH → `runtime/ffmpeg` → exe 同级（debug 另含 `tools/ffmpeg`）；`--enable-gpl` 探测；失败二分重试、`image` 回退；缓存键 `v3`；`runtime/ffmpeg` 为 LGPL shared，GPL 仅本地开发；pHash/修复仍走 `image`/Python | `docs/FFMPEG.md`；ffmpeg 4 测试；10k/100k 基准见下 |
| F-041 | 主题按钮在导出与窗口控制之间，`ControlSize::Md` + Ghost，非图库页仍在窗口控制左侧 | `cargo check` 通过 |
| F-042 | 26px 底栏：左侧状态/总数/已打标/未打标/失败/损坏可点跳筛选；右侧队列/设备/命令面板；标题栏命令面板与侧栏重复状态卡移除 | `.kanalib/ui_shell_statusbar.png` |
| F-060 | 10GB 默认磁盘预算（可调）；`thumb_access.json` 记录访问；512+t256 组 LRU 并保护当前解码/最近访问；设置页显示用量/命中率/清理；缺失缩略图按 `thumb_sources` 重建 | 单测 `evicted_thumbs_rebuild...`，`.kanalib/f060_settings2.png` |
| F-031 | `rescan_root` 单来源扫描与运行时 `SourceScanStatus`（last_scan/资产数/计数/错误/在线）；离线/遍历不完整不清理，重扫成功恢复监听；侧栏离线/错误标识与右键「重新扫描此来源」，设置页显示逐来源状态与只读排除摘要 | state 5 项（状态/删除/重命名/离线/过滤）通过；`.kanalib/round1_sources.png`、`round1_settings.png`；`cargo test --workspace` |
| F-062 | flexi_logger 0.31 滚动日志（默认 10MiB/5 个/14 天，`KANALIB_LOG_*` 可调；`KANALIB_LOG_FILE` 与 headless 路径兼容）；`DiagnosticSnapshot` 只读采集、URL/凭据脱敏，设置页诊断区/复制诊断/打开日志目录 | diagnostics 5 项 + bin 日志 3 项；`.kanalib/round1_settings.png`；`cargo test --workspace` |

验证汇总：`cargo test --workspace` core 32 + UI 5 通过（4 项按需忽略）；ffmpeg 4 项、视图收敛 6 项通过。10k 冷导入缩略图 ffmpeg 52.2s vs image 124.3s，总 152.3s vs 230.9s；100k 缩略图 738.7s（历史 1130s），总 42.3min（历史 49.3min）。历史 100k 滚动 151.5fps、present p99 15.84ms、峰值 329MB；搜索 p95 284ms（目标 <150ms）。所有 UI 冒烟使用隔离数据根，未触碰真实图库。

第一轮并行交付（2026-10-27）：任务 0/A/B/C/E/D 全部合入；`cargo test --workspace` core 61 + UI 8 通过（4 项按需忽略）。备份 4 项（WAL/完整性/失败/扫描拒绝）、扫描过滤/监听/单来源 5 项、诊断 5 项通过；便携包 `-SkipBuild` 完整打包 12,700 文件/3.93GB/36s，清单 SHA256 可复现（zip 2.8GB/5m24s）。UI 冒烟使用隔离数据根 `.kanalib/round1_ui_root`，UIA 实测「立即备份」生成 160KB 备份且完整性通过；截图 `.kanalib/round1_settings.png`、`.kanalib/round1_sources.png`。
