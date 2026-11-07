# 后续任务路线图（2026-11-04 起继续）

> 状态：**GUI 验收已开始**（2026-11-04，窗口限定副屏）。纯代码与后台自验已完成，C 类逐项推进；未勾选项仍不代表通过。
> 上级索引：[README.md](README.md) · 未完成项速览：[pending/overview.md](pending/overview.md) · 实现记录：[pending/browse.md](pending/browse.md)、[ui-refactor.md](ui-refactor.md)。

## 1. 当前状态

| 项目 | 值 |
|---|---|
| 提交点 | `eb26239` `docs(backlog): 记录第八轮副屏验收与 D5 原串泄漏清单`；第八轮验收记录与 D5 清单均已提交 |
| 工作区 | 干净 |
| registry | 56 条：42 已实现、5 远期、9 实现中（剩余以手动/真实环境验收为主） |
| 未完成 ID | F-030 / F-033 / F-036 / F-069 / F-070 / F-071 / F-072 / F-073 / F-074 |
| 最近完成 | 第八轮副屏验收（2026-11-07）：D4 English 扫描/相似完成态实机、D1 重试 zh+en 全链路、Shift+Tab 合成限制复核；D5 拍板并按 D4 模式落地（来源错误 keyed + `watch_retry` + 去重「文件更大」+ 打开链单前缀 + 英文单数）；A2 关闭 F-070 方案 B |
| 后台自验 | `cargo test` bin 6 / core 214 / UI 74（新增单数化用例）、`cargo check --workspace`、`cargo check --features frame-profiler`、debug 构建、`check_ui_invariants.py` 9 项、`check_backlog_docs.sh`、headless 冒烟（15 张示例库，退出码 0）—— 全部通过且**未启动窗口** |
| GUI 验收（2026-11-04，副屏） | F-070 四种侧栏/设置锚点/标签单搜索与返回/折叠隔离/上下文菜单/Tab 顺序/3 万标签、F-073 16 张基线；发现并修复去重页窄宽头部溢出与根 FocusScope 缺失导致 Tab 不可达；证据 `.kanalib/verify_f070/` |
| GUI 验收（2026-11-07，副屏） | D4 English 状态栏/banner 扫描与相似完成态、D1 重试 zh+en 全链路（1024 宽）、Shift+Tab 合成复核；证据 `.kanalib/verify_round8/`；泄漏与未触发项见 `manual-acceptance.md` 73-1/73-9 与 D5。D5 修复后的 English 渲染尚未实机复验 |

**唯一既有失败**：`sources::tests::immich_missing_key_and_disconnect_do_not_block_local_library`
（`http://127.0.0.1:1` 的错误 kind 不在断言集合，属环境/网络分类差异），与本轮改动无关，未写成通过。

## 2. 决定排序的用户口径

1. 纯代码部分先做完 → **已完成**。
2. 可在后台自行验收的做完 → **已基本做完**。
3. 需要用户手动验收的 → **往后放**。
4. 验收测试**不抢前台**，全部后台跑；任何会启动或激活 GUI 窗口的操作都不自动执行。

判断依据：`src/main.rs` 的 `KANALIB_HEADLESS` 分支在 GPUI 启动前 `return`（约 310–395 行），是唯一无需窗口的功能链路；`docs/PLAN.md` 的帧率采集必须窗口在前台，失焦即作废。

## 3. 后续任务清单

### A. 可立即推进（不抢前台、不需用户介入）

| # | 任务 | 产物 | 代价 |
|---|---|---|---|
| A1 | **手动验收清单**：把散在 `pending/browse.md`、`ui-refactor.md`、`PLAN.md`、`pending/library-sources-formats.md` 的 GUI/UIA/截图/回归项整理成可勾选清单，标注每项的前台依赖与证据路径 | `docs/backlog/manual-acceptance.md`（2026-11-06 已建：9 个分区 + 已通过项索引 + 长耗时项） | 纯文档，零风险 |
| A2 | 关闭已被取代的方案：F-070 方案 B 在方案 A 落地后失去意义；方案 C 的收敛项已被 A 继承 | registry / overview 的状态备注（2026-11-07 已关闭：D3 标记关闭，registry/overview/browse 补备注） | 纯文档 |
| A3 | 若需要，为 A1 配「不抢前台」的静态/文件检查脚本（不 `Start-Process`、不 `SetForegroundWindow`） | `scripts/` | 纯代码 |

### B. 后台可跑但耗时长（不是手动验收，只是时间成本）

| # | 任务 | 前置与耗时 | 口径 |
|---|---|---|---|
| B1 | 100k 合成库重新生成 | `scripts/generate_stress_library.py`（PIL 13.0 可用、D 盘 85G 空闲）；上次 10m51s 只到 21604 张（21.6%）后按用户要求停止，部分文件留在被忽略的 `.kanalib/stress_100k/` | `--out .kanalib/stress_100k/part0 --count 50000 --folders 10 --seed 43 --corrupt 5`，part1 用 seed 44 |
| B2 | 100k 冷导入 + 搜索 p95 + 相似扫描指标 | 冷导入基线 42.3min（`docs/PLAN.md`）；搜索/相似可用 headless 复现 | `KANALIB_HEADLESS=1 KANALIB_ROOT=<隔离根> KANALIB_SKIP_TAG=1 KANALIB_SEARCH_RUNS=100 KANALIB_SIMILAR_SCAN=1 KANALIB_METRICS_PATH=<jsonl> target/release/kanalib.exe <100k 根>` |
| B3 | 基准口径澄清 | 100k 排序 p95 的 `KANALIB_BENCH_ROWS` 代码已不在仓库，无法用 `cargo test` 复现；只剩 headless 的 4 查询指标 | 文档标注；不要把 vendor 的 `benches/budget.rs` 当作项目基线 |

> B1+B2 合计 1 小时以上，**未经用户确认不擅自启动**（上次已按此执行）。

### C. 需要用户手动 / 前台窗口（按用户要求往后放）

**F-070 上下文侧栏**

- [x] 四种上下文侧栏（图库 / 标签 / 设置 / 去重）的亮暗 × 1024/1200 逻辑宽截图基线（16 张，水平越界 0；`.kanalib/verify_f070/`）
- [ ] 1440/1920 逻辑宽基线（副屏逻辑宽仅 1280，放不下；需改 DPI 或占用主屏）
- [x] 设置页分区锚点点击后真的滚到对应卡片（`立即备份` y 3722→-143、`复制诊断信息` 4761→326、`编辑排除规则` 6165→-200）
- [x] 标签页侧栏搜索 / chips / 排序与页内列表联动，屏幕内只有一套搜索（仅 1 个 Edit；`girl`→2 行；0 使用→0 行；名称排序生效）
- [x] 3 万标签虚拟列表/搜索/滚动：首屏 152ms、精确搜索 1 条 513ms（含键入）、滚动正常、工作集 133→121MB；侧栏 chips 为固定分类，不随 3 万变化
- [x] 侧栏 UIA 名称、设置锚点焦点环、「查看图片 → Esc 返回」分层（证据 `uia_*.txt`）
- [x] Tab 顺序：设置页/标签页逐站记录与焦点环（根 FocusScope 修复后；`tab_walk_settings.log`、`tab_walk.log`）
- [ ] Shift+Tab、焦点全遍历、Esc 其它分层路径
- [x] 回归：F-006 折叠与重启持久化（`ui.sidebar_collapsed=sources`，旧键生效；宽度拖拽未验）+ 方案 C 的「查看图片 → Esc 返回 → 状态保留」
- [x] F-011 侧栏标签面（chips/搜索/排序/查看图片在标签页验收中覆盖）
- [x] F-025 低频/0 使用筛选与计数（低频 30156、0 使用 0；无自动删除入口）
- [x] F-067 撤销（3 星 → 底栏「可撤销」→ 已撤销 toast → DB star 归 0；证据 `f067_after_star.png`、`f067_after_undo.png`）
- [ ] F-020 别名合并 GUI 提交链路（对话框可开、字段可见；源码确认空目标时提交按钮 `disabled`、点遮罩会关闭，合成点击/键入时对话框提前关闭，未真正触发提交；核心 `db::tests::renaming_and_merging_tags_keep_aliases_and_fts` 通过）

**F-071 / F-072**

- [x] F-071 主题切换、设置页选中态、标题栏无主题按钮（标题栏拖拽区未验）
- [ ] F-072 标题栏搜索 Ctrl+F 全选、清除、历史选择、命令面板失焦（副屏 WM_CHAR 焦点受限，未写成通过；标签侧栏搜索清除已验）

**F-073 全局 UI**

- [x] 1024/1200 逻辑宽截图基线补四种新侧栏（16 张，水平越界 0）
- [x] 去重页评审区窄宽头部溢出修复（1024 宽 4 个按钮、1200 宽 2 个按钮越界 → 修复后均 0）
- [x] 上下文溢出菜单项 UIA 名称（图库 3 项 / 设置 1 项）
- [x] 根 FocusScope 修复 + Tab 遍历（`.root().node(...)`；`check_ui_invariants.py` 第 6 项）
- [x] 项目级 i18n 第一批（zh-CN/en-US 目录、设置页语言切换、壳层/导航/标题栏；`ui.locale` 持久化）
- [x] 项目级 i18n 第二批（侧栏 8 个分区标题、状态栏计数/队列/命令面板键；错误 compact 下部分项未截图）
- [x] 项目级 i18n 第三批（侧栏过滤/评级行、标签计数/分类/空状态/更多、搜索占位改 `placeholder_key`）
- [x] 项目级 i18n 第四批（扫描阶段/进度/摘要/吞吐/ETA/来源；设置页当前状态模板）
- [x] 项目级 i18n 第五批（撤销状态栏/toast/状态覆盖、自动归类固定名与「作品:」前缀）
- [x] 项目级 i18n 第六批（UI 全量收口：设置/去重/侧栏/视图/主应用/检查器/对比/标签/查看器/搜索等；六页 English 巡检）
- [x] core 状态消息结构化（D4）：`Shared.status` 改为 `StatusMessage { key, args, text }`；state.rs 110 处 + watch.rs 2 处迁移，138 个 `notice.*` 键补中英文；UI `core_status` 统一渲染；headless/日志保留中文 `text`。2026-11-07 副屏部分验收：扫描开始/完成、相似完成、来源卡 last-scan 英文通过；中途瞬态未截到、来源错误行/去重原因泄漏转 D5。
- [x] D5 core 原串泄漏结构化（2026-11-07 拍板）：`SourceScanStatus.error` 改 `StatusMessage`（11 处 keyed、10 个新 `notice.source.*` 键）、`watch_retry` 替代文案 `contains` 判定、去重「文件更大」映射、打开链单前缀 + 缺失文件预检、英文「1 <单数>」；`check_ui_invariants.py` 第 9 项。English 实机复验未做。
- [ ] 1440/1920 与浮层/查看器截图基线
- [ ] GUI 弹窗 / 菜单 / 表单 / 命令面板 UIA 与悬停验收（tooltip、loading spinner、`.invalid()` 红框、清除按钮对 150/240px 固定宽输入的挤压）
- [ ] 渲染后 WCAG 实测（现有 `check_theme_contrast.py` 只是 token 设计值）
- [ ] 帧率 `present p99`：必须窗口前台，`run_frame_probe.ps1` 每秒置前，失焦样本为 0，数据作废
- [ ] 100k 滚动峰值工作集 326–329MB（`KANALIB_UI_SCROLL_TEST`，需前台）

**F-069 / F-074 / 其它真实环境**

- [x] F-069 双击进入查看器、适应/手动缩放标签（工具条「放大」→75%，「适应」恢复）
- [x] F-069 超大图 100MP 阈值与提示（默认快速预览 + 强制原图入口；10001×10000 实测）
- [ ] F-069 `0`/`1` 快捷键（合成按键焦点受限）
- [x] F-074 双击入口与缺失/损坏原图错误态（离线来源显示「无法打开这张图片」）
- [x] F-074 超大图 100MP 阈值与回退提示（与 F-069 同一实现）
- [ ] F-074 宽色域 ICC 样本、连续切图内存与资源释放
- [ ] F-030：真实 OneDrive/网络盘事件语义与断线重放（环境可用但未获授权写入，未实测）
- [ ] F-033：HEIC/HEIF、RAW 真实样本与可分发解码链（仓库无样本、LGPL ffmpeg 无对应 demuxer ⇒ 维持「不支持」）
- [ ] F-036：真实资源管理器拖放导入
- [ ] 中文 IME 组字/候选窗、Ctrl 多选、系统播放器入口

> **会抢前台**的脚本：`run_frame_probe.ps1`、`focus_window.ps1`、`ui_baseline.ps1`（需 `-Hwnd`）、`capture_window.ps1`、`uia_*`、`window_type.ps1`、`window_wheel.ps1`。默认只由用户手动执行；经用户明确授权时（2026-11-04 起）可在副屏运行，窗口只落在副屏、只操作自己启动的 PID，结束即收实例。

### D. 需要方案决策（不得自行定）

| # | 任务 | 为什么需要拍板 |
|---|---|---|
| D1 | error 通用重试入口 | 已落地（2026-11-06）：仅 UI 打开失败登记 `UiRetryAction`，底栏/主图库/去重/设置错误条显示「重试」；未改 core 错误模型；剩 GUI/E2E 验收 |
| D2 | 项目级 i18n | 已落地第一批（2026-11-04）：zh-CN/en-US 项目目录、`ui.locale` 持久化、设置页语言切换、壳层/导航/标题栏迁移；键名 `区域.用途`；其余 1500+ 条按页面分批 |
| D3 | F-070 方案 B | **已关闭（2026-11-07）**：被方案 A 取代，不实现；registry/overview/browse 已补备注 |
| D4 | core 状态消息结构化 | 已按「core 结构化 key+args」落地（2026-11-06）：`StatusMessage` + `notice.*` 双语目录 + `check_ui_invariants.py` 第 7 项；2026-11-07 副屏部分验收通过，剩余泄漏见 D5 |
| D5 | core 原串泄漏结构化（来源错误/去重保留原因/打开链 bail/计数复数） | **已拍板并落地（2026-11-07）**：用户选择「按 D4 模式结构化」。`SourceScanStatus.error` 改为 `StatusMessage`（11 处写点全部 keyed）；新增 `watch_retry: bool` 替代 `contains("监听"/"离线")` 恢复判定；去重「文件更大」补 UI 映射；打开链去掉内层「打开失败」前缀并在 UI 预检缺失文件；英文计数单数化（`finish_text`）。`check_ui_invariants.py` 第 9 项固化。剩 English 实机渲染验收（来源错误行/去重原因/复数组卡） |

## 4. 红线

- **不抢前台**：不自动运行任何 `Start-Process` GUI / `SetForegroundWindow` / `PrintWindow` / UIA 连窗口的脚本。
- **不擅自启动长任务**：100k 生成 + 冷导入、帧率采集需用户明确确认。
- **不改冻结布局**：主图库网格、查看器、检查器（`constraints.md` 的「UI 重构边界」条目）；侧栏 / 标签 / 设置 / 去重允许结构级调整。
- **不引入 schema 迁移**：折叠状态、设置项一律走 settings JSON（`constraints.md` 的「依赖/构建」与评分口径条目）。
- **不把未验收写成通过**：所有 GUI/UIA/截图/帧率项在 backlog 里保持未勾选。
- **提交规范**：conventional 前缀 + 中文正文、一个功能一个 commit、无 trailer。

## 5. 下一步选择

| 选项 | 说明 |
|---|---|
| D5 方案决策 | ✅ 已拍板（2026-11-07）：按 D4 模式结构化，代码与静态检查已落地；剩 English 实机复验 73-1/73-9 |
| C 类手动验收 | 按 `docs/backlog/manual-acceptance.md` 逐项走；剩 1440/1920、Shift+Tab（真实键盘）、去重/设置错误条重试、浮层基线，以及 D5 修复的 English 实机复验 |
| B1 启动 100k 后台全量回归 | 约 1 小时以上，纯后台；需要先确认 |
| F-012 搜索 p95 优化 | 需完整 100k 库与长跑基准；当前 150ms 目标已改为同库实测上限口径 |
| 暂停 | 保持现状，等用户有空做 C 类验收 |

## 6. 命令速查（全部后台可跑）

```bash
bash scripts/check_backlog_docs.sh
python scripts/check_ui_invariants.py
python scripts/check_theme_contrast.py
PATH="/c/Users/Administrator/.cargo/bin:$PATH" cargo check --workspace
PATH="/c/Users/Administrator/.cargo/bin:$PATH" cargo test -p kanalib-ui -p kanalib
```

便携包 pre-flight 必须走 PowerShell 工具执行（Bash 里调 `pwsh` 会被安全策略拒绝）：

```powershell
pwsh -NoLogo -NoProfile -File scripts/build_portable.ps1 -ValidateOnly
```

headless 冒烟（进程在 GPUI 启动前 `return`，不建窗）：

```bash
KANALIB_HEADLESS=1 KANALIB_ROOT=<隔离根> KANALIB_SKIP_TAG=1 \
KANALIB_SEARCH_RUNS=30 KANALIB_SIMILAR_SCAN=1 \
KANALIB_LOG_FILE=<日志> KANALIB_METRICS_PATH=<jsonl> \
target/debug/kanalib.exe sample_library
```
