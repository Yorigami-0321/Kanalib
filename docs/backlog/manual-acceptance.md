# 手动验收清单（GUI / UIA / 截图 / 真实输入）

> 用途：集中需要窗口前台、真实键盘鼠标、真实系统环境或长耗时数据生成的验收项。**未勾选不代表失败，也不代表通过**；完成后在对应项打勾，并把证据路径补在「证据/备注」列。
>
> 上级索引：[README.md](README.md) · 未完成项：[pending/overview.md](pending/overview.md) · 图片墙路线图：[photo-wall-status.md](photo-wall-status.md) · 逐项实现记录：[pending/browse.md](pending/browse.md)
>
> 最后整理：2026-11-07（第八轮副屏验收后）。

## 1. 执行约定

- **不抢前台**：默认不自动启动 GUI 或激活窗口。经用户明确授权时，窗口只落副屏；只操作自己启动的 PID，结束即收实例。
- **只验改动项**：按 [../TESTING-PROCESS.md](../TESTING-PROCESS.md) 的 L3 口径，只验收本次改动影响的可观察行为；未改页面/状态不重复截图，已有证据直接复用。
- **副屏坐标**：实测主屏 `0,0..2560,1440` @125%，副屏 `-1920,-623..0,657` @150%，副屏逻辑宽约 1280。DPI 无关进程看到的 `-1536,-498 1280x853` 是虚拟化值，不能直接用于 `SetWindowPos`。
- **窗口落位**：先用 `SetWindowPos` 移到副屏，再对同一窗口设一次目标物理尺寸，最后 `GetWindowRect` 回读确认在副屏边界内。DPI 无关 PowerShell 直接设置宽高会被虚拟化。
- **合成输入限制**：`WM_CHAR` 在查看器/检查器等焦点下不可靠；合成 Shift 修饰不进入 gpui `current_modifiers`，Shift+Tab 必须用真实键盘；`SendMessage(WM_KEYDOWN)` 对 gpui 无效，需要窗口前台 + 带扫描码的按键。合成输入不能作为通过依据。
- **会抢前台的脚本**：`run_frame_probe.ps1`、`focus_window.ps1`、`ui_baseline.ps1`（需 `-Hwnd`）、`capture_window.ps1`、`uia_*`、`window_type.ps1`、`window_wheel.ps1`。默认只由用户手动执行。
- **数据隔离**：`KANALIB_ROOT=<隔离数据根>`；`library.local_roots` 必须指向夹具图库，否则会打开空库。测试清理只动隔离数据根，不碰真实图库与真实图库 DB。
- **窗口形态**：所有 GUI 验收一律使用正常横屏窗口（约 16:9），窗口必须完整落在所选屏幕内，禁止超长/超出屏幕下沿。夹具 `window_state.json` 统一写 800×450 逻辑；启动后若被放大，用 SetWindowPos 连设两次并复核（参见 `.kanalib/verify_round9/set_capture.ps1`）。
- **证据归档**：统一放 `.kanalib/<轮次>/`，文件名含页面、主题、逻辑宽；截图记录逻辑宽与 DPI，UIA/日志保留原始文本。

## 2. 夹具与证据

| 夹具 | 内容 | 用途 | 备注 |
|---|---|---|---|
| `.kanalib/accept_f070_gui` | 10k 测试库副本，268 标签、102 精确重复组；缺失来源显示离线，缩略图缓存可用 | 侧栏/标签/设置/去重回归 | 隔离根无 sidecar，会出现 `sidecar script not found` 红色提示，属环境 |
| `.kanalib/accept_f070_tags30k` | 同一 10k 库 + 30,000 个通用标签（共 30,268）与 30,871 条使用记录 | 3 万标签列表/搜索/滚动 | 同上 |
| `.kanalib/accept_r8_d1` | 单资产隔离根（`retry_probe.txt`，路径在 `probe_outside` 根内）；DB 由 accept_f070_gui 复制改写 | D1 打开失败/重试链路 | 复跑先删整个 `probe_outside` 目录再启动（根离线：不弹缺失清理对话框）；触发失败后补目录+文件再点重试即成功。根在线时增删文件会触发监听刷新清掉错误条，抢在重试前 |
| `.kanalib/accept_r9_a` | 单资产隔离根（`lib/only.png`，启动时 `lib` 不存在）；DB 由 accept_f070_gui 复制改写，`ui.locale=en-US` | D5 English：来源错误行/`1 image` 单数/打开失败单前缀；D1 设置页 banner 重试 | 根离线且无监听刷新，错误条会一直保留；用例：右键/检查器 Open → 设置页 banner。复跑保持 `lib` 目录不存在 |
| `.kanalib/accept_r9_b` | 两张同像素不同压缩级别的 PNG（`big.png` 900.8KB / `small.png` 1.6KB，pHash 相同）；headless 预先扫描入库 | D5 English：去重保留原因 `Larger file`；D1 去重 banner 重试 | 启动时用 `KANALIB_DISABLE_WATCH=1` 并删除 `big.png`，相似扫描仍从 DB 出组；Open keeper 失败后错误条持续。复跑先恢复 `big.png` 或重新扫描 |
| `.kanalib/accept_r9_tag` | 61 张图（60 张 1024×1024 + 损坏的 `broken.png`）；`runtime`/`models`/`sidecar` 用目录联接指向仓库 | 队列 running/error 与修复瞬态（需 sidecar/模型） | 用 `KANALIB_DISABLE_WATCH=1`、不设 `KANALIB_SKIP_TAG`，启动时冷扫并打标；大图用于拉长 running 窗口。复跑先清空 DB 的 assets/jobs 再启动；junction 删除只需删链接目录 |
| `.kanalib/verify_f070/` | F-070/F-073 副屏截图、Tab 日志、UIA 文本 | 既有基线 | 含 16 张亮暗 × 1024/1200 基线 |
| `.kanalib/ui_f069_fit_data` + `.kanalib/ui_f069_fit_root` | F-069 适应/100% 三张样本 | 查看器缩放 | 样本含 1120×1992、280×175、1300×1149 |
| `sample_library` | 15 张示例库（含损坏 AVIF） | headless / 快速冒烟 | 不含 100k 性能结论 |
| `.kanalib/stress_100k/` | 100k 合成库的**部分**内容（上次 21,604 张后停止） | 100k 相关项 | 需重新生成才能做全量回归 |

## 3. F-070 上下文侧栏

| # | 验收项 | 前置/前台 | 证据/备注 |
|---|---|---|---|
| 70-1 | 1440/1920 逻辑宽下四种上下文侧栏无水平溢出 | 副屏逻辑宽不足，需改 DPI 或占用主屏；会抢前台 | 已有 1024/1200 基线，见 `.kanalib/verify_f070/` |
| 70-2 | Shift+Tab 反向遍历、焦点全遍历、Esc 其它分层路径 | 真实键盘；合成 Shift 不生效 | **通过（2026-11-07，用户真实键盘实测确认）**：正向 Tab 与焦点环（`verify_f070/tab_walk*.log`）；Shift+Tab 反向遍历由用户亲自按键确认正常（`verify_round9/tab_user_final.png` 为结束态截图）；合成 Shift 经 AttachThreadInput+SetKeyboardState 尝试未证实（gpui 读真实键盘状态），后续此类验收直接请用户按键；**同日用户决定将 Tab / Shift+Tab 焦点遍历标记为待移除（F-075/F-076），这两部分不再复验；Esc 其它分层路径仍待验收** |
| 70-3 | F-020 别名/合并对话框真实提交：输入目标、Enter/点击提交、别名与 FTS 生效 | 真实键入 + 前台；空目标时提交按钮 `disabled` | 核心测试 `db::tests::renaming_and_merging_tags_keep_aliases_and_fts` 通过；2026-11-07 GUI 复核：`Rename tag` 对话框渲染正确（`#abstract affects 60 images; the old name stays an alias…`）、输入可清空/键入（截图确认 `abstract_renamed`）、空值禁用 Rename；但合成输入下 Rename/Enter/UIA Invoke 均未把改名落库（日志有 `prompt dialog … sent`，DB/FTS 不变），提交路径未验收，需真实点击复核 |
| 70-4 | 侧栏宽度拖拽、最小/最大宽度、重启后保持 | 真实鼠标拖拽 | 折叠与重启持久化已验；2026-11-07 合成拖拽：设置写入 `ui.sidebar_width=298` 但布局未变化（按下/移动未稳定命中分隔条），宽度拖拽与最小/最大夹取仍需真实鼠标复核 |

## 4. F-071 活动栏与标题栏

| # | 验收项 | 前置/前台 | 证据/备注 |
|---|---|---|---|
| 71-1 | 标题栏拖拽区可拖动窗口；右侧间距、最小化/最大化/关闭点击不受影响 | 真实鼠标 | 主题切换、设置选中态、标题栏无主题按钮已验（`f071_theme_light.png`、`f071_settings_selected.png`） |
| 71-2 | 活动栏底部「设置」与命令面板「设置」都进入设置页 | 真实点击 | **通过（2026-11-07）**：活动栏 Settings 点击进入设置页；命令面板输入 `Settings` 回车亦进入（`cmd_k_settings.png` 页头 `Settings`） |

## 5. F-072 搜索入口与搜索态

| # | 验收项 | 前置/前台 | 证据/备注 |
|---|---|---|---|
| 72-1 | `Ctrl+F` 聚焦标题栏搜索并全选 | 真实键盘 + 前台 | 副屏 WM_CHAR 焦点受限，未写成通过 |
| 72-2 | 清除按钮、搜索历史打开/选择/关闭、Esc 清空或退出 | 真实键入/点击 | **通过（2026-11-07）**：标题栏搜索键入 `sky` 后出现 Clear、Recent searches 1；Clear 清空查询；历史菜单含 `sky`/`Clear recent searches`，选择 `sky` 重新搜索；Esc 清空查询并退出过滤（`search_*.png`）；标签侧栏搜索此前已验 |
| 72-3 | 命令面板/对话框打开时搜索框不被抢回；点击侧栏/空白后焦点环消失 | 真实焦点切换 | 合成焦点不可靠 |
| 72-4 | 相似视图与标签页局部搜索文案/作用域不与全局搜索混淆 | 截图 + UIA | 与 F-070 标签页验收联动 |

## 6. F-073 全局 UI、可访问性与性能

| # | 验收项 | 前置/前台 | 证据/备注 |
|---|---|---|---|
| 73-1 | English 下扫描/队列/来源/去重/修复等瞬态状态栏，以及设置页网络来源按钮/说明/凭据/toast 真实渲染 | 副屏 GUI；触发对应动作 | 2026-11-07 部分通过：扫描开始/完成/中途（`Hashing 1687/2000 · 22.0 images/s · ETA 15 s`）、相似完成、来源卡 last-scan、来源离线错误行、监听失败 banner 英文已验；**扫码/队列/修复**用带 sidecar 的隔离夹具补验——队列 running（状态栏 `Queue waiting 45 · running 4 · failed 0`、设置卡 `Running · waiting 17 · running 4 · done 40 · throughput 3.2 images/s · ETA 7 s`）、队列 error（`Failures while idle · failed 1 · done 60`）、修复瞬态（损坏 PNG → Repair → 错误条）；证据 `verify_round8/d4_*.png`、`verify_round9/r9f_*.png`、`r9h_queue_*.png`、`r9h_run_*.png`、`r9h_repair_*.png`。**新泄漏**：队列卡 `打标已继续` 中文、最新打标错误详情中文（`文件不完整（图片数据被截断）`）、修复失败错误条中文（`修复失败：容错解码…`）——登记 D6。未覆盖：远程来源凭据对话框/toast 真实渲染（后续第九轮已补）、远程按钮/说明（静态目录已迁移）。**修复成功路径已补验**（截断 JPEG，容错解码 accepted）：`Preview cache rebuilt …` → `Repair complete (文件不完整…)`，damage 标签因文件仍截断而保留；`Repair complete` 括号内仍为中文 damage 文案（D6）。整项维持未勾选（剩余远程 toast 细节） |
| 73-2 | 1440/1920 逻辑宽与浮层/查看器/对话框/Toast 截图基线 | 需主屏或改 DPI；会抢前台 | **通过（2026-11-07，主屏授权，125% DPI）**：设置 1800/2400 物理（1440/1920 逻辑），DWM 采集 1784/2384 物理；亮暗 × 图库/重复项/标签/设置 × 宽度 + 命令面板浮层 + 查看器 = 24 张（`verify_round9/baseline/`）；UIA 水平越界审计 24 份全部 0。注：窗口在跨 DPI 移动后会被应用异步改尺寸，采用「设置×2 + 同进程立即 PrintWindow」采集 |
| 73-3 | GUI 弹窗/菜单/表单/命令面板 UIA 与悬停验收 | 窗口前台 + UIA | 2026-11-07 部分通过：in-app 模态框 UIA 与截图——`Rename source`、`Remove source?`、`Add WebDAV source`（URL 非法红框）、`Set network source credential`（username:password 字段 + Cancel/Save credential，`r9j_credentials_dialog.png`）；远程序章——添加 WebDAV 源（`Added the WebDAV source`）、`Test connection` 失败提示 `… connection failed`、移除源；命令面板 72 项英文可读，过滤/方向键/Esc；**命令执行**：键盘输入+回车执行并用 `picked <id>` 日志复核 17 条——`goto:all/untagged/failed/damaged/dup/similar/tags/settings`、`view:theme`（亮暗截图）、`view:quick`（查看器）、`view:small`、`view:select-all`、`view:clear`、`act:star3`（★3）、`act:toggle-fav`+`act:undo`（截图差）、`scan:rescan`（完成摘要）。未覆盖/未执行：原生选择框（rfd）、剪贴板命令、导出/破坏性命令（按设计跳过）、150/240px 输入挤压；坐标点击对折叠列表不可靠（仅顶部导航项可点） |
| 73-4 | 组件状态矩阵：hover/focus/pressed/selected/disabled/loading 一致性 | 截图对比亮暗主题 | **抽样通过（2026-11-07，亮暗两套）**：hover（工具条按钮、菜单项 `Compare selected (2–4)`、分段中文）、focus（收藏按钮蓝环、设置活动项、阈值输入蓝框）、pressed（View 按下）、disabled（`Copy pro…` 置灰）、selected（`All images` 选中行、标签页选中）、tooltip（Theme）、**loading**（备份按钮 spinner + `Backing up the database...` banner，亮暗各一）。滑块无可见 hover 样式（记录为现状）。证据 `r9c_*.png`、`state_*.png`、`r9i_backup2_1.png`、`r9i_backup_dark_1.png`、`menu_*`；未逐控件穷举（按组件类型抽样） |
| 73-5 | 渲染后 WCAG 实测（非 token 设计值） | 截图/取色 + 前台 | **通过（2026-11-07，抽样）**：用基线截图 + UIA 元素矩形逐控件采样（`scripts/check_rendered_contrast.py`，8 页 × 亮暗 × 2 宽）：正文最低于暗色 5.18:1 / 亮色 5.47:1，全部 ≥4.5；禁用态 `Copy prompt`/`Reset` 2.96–3.73（WCAG 1.4.3 豁免）；`Ascending; click for descending` 排序图标亮色 2.99:1 略低于非文本 3:1（WARN，临界，建议后续 token 微调）。方法说明：按元素矩形取背景模态色与对比度最高的稳定色，小字号抗锯齿会使测得值偏低 |
| 73-6 | 帧率 `present p99`：窗口前台，`run_frame_probe.ps1` 每秒置前，失焦样本为 0 | 必须前台；会抢前台 | 失焦数据作废 |
| 73-7 | 100k 滚动峰值工作集 326–329MB | 前台 + `KANALIB_UI_SCROLL_TEST` + 100k 库 | 需先生成完整 100k 库 |
| 73-8 | 100k 首屏/搜索/切图全量回归不劣于既有基线 | 前台 + 100k 库；冷导入约 42min | 既有基线见 `docs/PLAN.md`；本轮未重跑 |
| 73-9 | D1 打开失败重试按钮：底栏/主图库/去重/设置错误条显示「重试」，成功后错误清除，失败后保留入口 | 触发 6 处打开失败之一（可用不可用系统应用的文件） | **通过（2026-11-07）**：四个展示位全部实机验证——底栏/主图库错误条（`verify_round8/d1_*.png`，zh+en）、设置页 banner（`verify_round9/r9a_settings_banner_retry_en.png`）、去重 banner（`r9b_dedup_banner_retry_en.png`）；失败显示→重试失败保留→补文件重试成功清除（zh）已验；错误文案为英文单前缀 `Failed to open file: File not found:`（D5 修复）；其余打开分支共用 `open_asset_with_system` 同一链路，未逐条单独点击 |

## 7. F-069 / F-074 查看器

| # | 验收项 | 前置/前台 | 证据/备注 |
|---|---|---|---|
| 69-1 | `0` 恢复适应、`1` 切 100%；双击在适应/100% 间切换 | 真实键盘 | 适应/75% 标签与双击入口已验；`0`/`1` 合成按键受限 |
| 69-2 | 拖动侧栏/检查器宽度、切换全屏后适应模式重算；手动缩放不被强制拉回 | 真实鼠标/窗口操作 | 窗口尺寸变化与「适应」按钮恢复已验；侧栏/检查器拖动、全屏未验 |
| 69-3 | 100% 模式显示尺寸等于原图像素（按 DPR）；EXIF 方向正确 | 多 DPI 截图 | 未验 |
| 74-1 | 广色域 ICC 样本：严格原图与色彩管理预览有明确提示/入口，不静默偏色 | 真实广色域样本 | 仓库无样本；未验 |
| 74-2 | 连续切图/关闭后原图资源释放，内存与纹理不残留 | 前台 + 内存采样 | 未验 |
| 69-4 | 视频首帧、远端只读、无原图资产的适应/100% 行为明确 | 视频/远端夹具 | 部分边界有代码，未逐项验收 |
| 69-5 | 现有 `+/-`、`Ctrl+滚轮`、拖拽平移、`R`、`F`、F-003 对比同步缩放不回归 | 真实输入 | 回归项，未全量复测 |

## 8. 真实系统环境与输入

| # | 验收项 | 前置/前台 | 证据/备注 |
|---|---|---|---|
| 30-1 | 真实 OneDrive：云端占位 hydrated/dehydrated 事件语义、离线合并、重连补发 | 需授权向 OneDrive 写入/触发同步 | 第六轮只读探测：环境存在但未授权写入，未实测 |
| 30-2 | 真实网络盘（SMB）：变更通知延迟/丢失、断线重放、目录级重命名粒度 | 需授权向网络盘写入 | `Z:` 为 `\\10.0.1.1\Time Capsule`，只读探测过 |
| 33-1 | HEIC/HEIF、RAW 真实样本与可分发解码链 | 需样本 + 许可审查；LGPL ffmpeg 当前无对应 demuxer | 维持「不支持」，不写成通过 |
| 36-1 | 真实 Windows 资源管理器拖放文件夹/图片（含日文/Unicode/长路径） | 必须真实 Explorer 拖放，`ExternalPaths` 无法由 UIA/消息合成 | 窗口内模拟不作为通过依据 |
| 90-1 | 中文 IME 组字/候选窗 | 真实键盘 + 输入法 | 未验 |
| 90-2 | Ctrl 点击多选 2–4 张后进入对比 | 真实 Ctrl + 点击 | 合成点击只选中 1 张；未复现 |
| 90-3 | 视频系统播放器入口 | 真实点击 + 系统关联 | 未验 |

## 9. 已通过项索引（避免重复验收）

| 范围 | 结论 | 证据 |
|---|---|---|
| F-070 四种上下文侧栏（1024/1200 × 亮暗 16 张） | 通过 | `.kanalib/verify_f070/` |
| 设置页 15 锚点滚动 | 通过 | `verify_f070` 日志与截图 |
| 标签页单搜索/chips/排序/查看图片→Esc 返回 | 通过 | `verify_f070` |
| 侧栏折叠隔离 + 重启持久化 | 通过 | `ui.sidebar_collapsed=sources` |
| 上下文溢出菜单 UIA 名称 | 通过 | `uia_*.txt` |
| 根 FocusScope + 正向 Tab 顺序 | 通过；2026-11-07 转 F-076 待移除，不再复验 | `tab_walk_settings.log`、`tab_walk.log`、`check_ui_invariants.py` 第 6 项 |
| 3 万标签首屏/搜索/滚动 | 通过 | `tags30k_walk.log`、`tags30k_*.png` |
| F-067 撤销（3 星→底栏撤销→DB 归 0） | 通过 | `f067_after_star.png`、`f067_after_undo.png` |
| F-071 主题切换/设置选中态/标题栏无主题按钮 | 通过 | `f071_theme_light.png`、`f071_settings_selected.png` |
| 项目级 i18n 静态 UI 六页巡检 | 通过 | `i18n_final_library.png`、`i18n_full_tags.png`、`i18n_final_dedup.png`、`i18n_full_settings.png`、`i18n_final_viewer_back.png`、`i18n_final_compare.png` |
| F-069 双击入口、适应/75% 标签、100MP 阈值与回退提示 | 通过 | `f069_large_banner*.png`、`browse.md` F-069 记录 |
| F-074 双击默认原图、缺失原图错误态 | 通过 | `browse.md` F-074 记录 |
| D4 English 状态栏/banner（扫描开始/完成、相似完成、来源卡 last-scan） | 通过（瞬态中途与泄漏项除外，见 73-1） | `verify_round8/d4_settings_en.png`、`d4_rescan_summary_en.png`、`d4_similar_done_en.png` |
| D1 重试机制（四个展示位：底栏/主图库/设置 banner/去重 banner；zh+en） | 通过 | `verify_round8/d1_*.png`、`verify_round9/r9a_settings_banner_retry_en.png`、`r9b_dedup_banner_retry_en.png` |
| D5 English 复验（来源错误行/打开链单前缀/`1 image`/去重 `Larger file`） | 通过 | `verify_round9/r9a_settings_en.png`、`r9a_open_failed_en.png`、`r9a_retry_failed_en.png`、`r9b_similar_larger_en.png`、`uia_a_*.txt` |
| 73-4 组件状态抽样（hover/focus/pressed/disabled/selected/tooltip，亮暗） | 抽样通过（全矩阵未覆盖） | `verify_round9/r9c_*.png` |
| 73-3 设置页字段/对话框（重命名/移除确认/URL 非法红框） | 部分通过（loading/原生选择框未覆盖） | `verify_round9/r9d_*.png` |
| 73-1 扫描中途瞬态（English Hashing/吞吐/ETA + 完成 + 队列 waiting） | 通过（队列 running/修复仍需 sidecar） | `verify_round9/r9f_midscan_hashing_en.png`、`r9f_scan_complete_en.png` |
| 73-3 命令面板（72 项 UIA/过滤/方向键/浮层） | 通过（动作逐条执行未覆盖） | `verify_round9/r9g_palette_*.png`、`uia_g_palette.txt` |
| 73-2 1440/1920 宽基线（亮暗 × 4 页 + 浮层 + 查看器，24 张，越界 0） | 通过（主屏授权） | `verify_round9/baseline/` |
| 73-5 渲染后对比度抽样（正文 ≥4.5；禁用态豁免；排序图标 2.99 WARN） | 抽样通过 | `scripts/check_rendered_contrast.py` + `verify_round9/baseline/` |
| 73-1 队列 running/error 与修复失败瞬态（sidecar/模型实机） | 部分通过（新泄漏登记 D6） | `verify_round9/r9h_queue_*.png`、`r9h_run_*.png`、`r9h_repair_*.png` |
| 73-4 组件状态扩展（菜单 hover/开关/分段/输入焦点/loading spinner，亮暗） | 抽样通过 | `verify_round9/state_*.png`、`r9i_backup*.png` |
| 73-3 命令执行（17 条 picked 日志复核）与远程凭据对话框 | 部分通过（原生/破坏性/剪贴板命令未执行） | `verify_round9/command_kbd.log`、`uia_*`、`r9j_credentials_dialog.png`、`r9j_test_connection.png` |
| 修复成功路径（截断 JPEG：Preview cache rebuilt → Repair complete） | 通过 | `verify_round9/r9k4_*.png`、sidecar.log |
| 71-2 双入口进设置（活动栏 + 命令面板） | 通过 | `verify_round9/cmd_k_settings.png` |
| 72-2 标题栏搜索清除/历史/重选/Esc | 通过 | `verify_round9/search_*.png` |
| 原生选择框（打开 UIA 树完整、Esc/WM_CLOSE 关闭） | 通过（仅打开/取消） | `verify_round9` 日志 |

## 10. 长耗时后台项（不属手动验收，需单独确认）

| # | 任务 | 说明 |
|---|---|---|
| B1 | 生成完整 100k 合成库 | 上次 10m51s 只到 21.6%；完整生成 + 冷导入约 1 小时以上 |
| B2 | 100k 冷导入 + 搜索 p95 + 相似扫描指标 | `KANALIB_HEADLESS=1 ... target/release/kanalib.exe <100k 根>` |
| B3 | 基准口径澄清 | `KANALIB_BENCH_ROWS` 代码已不在仓库；不要用 vendor `benches/budget.rs` 当项目基线 |
