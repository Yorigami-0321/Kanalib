# 后续任务路线图（2026-11-04 暂停点）

> 状态：**功能推进已暂停**，本页只整理后续任务，不含新实现、不代表验收结论。
> 上级索引：[README.md](README.md) · 未完成项速览：[pending/overview.md](pending/overview.md) · 实现记录：[pending/browse.md](pending/browse.md)、[ui-refactor.md](ui-refactor.md)。

## 1. 当前状态

| 项目 | 值 |
|---|---|
| 提交点 | `7b84377` `docs(backlog): 记录方案 A 落地后的补充后台自验 (F-070)` |
| 工作区 | 干净；仅 `.workbuddy/`（会话记忆，未跟踪、不入库） |
| registry | 56 条：42 已实现、5 远期、9 实现中（4 个带阶段备注） |
| 未完成 ID | F-030 / F-033 / F-036 / F-069 / F-070 / F-071 / F-072 / F-073 / F-074 |
| 最近完成 | F-070 方案 A（S1–S5，6 个提交）；F-073 五批（菜单/确认层/状态/键位/错误反馈/键盘可达）；取证档 `unsafe` 兼容警告清理；上下文侧栏结构静态断言 |
| 后台自验 | 编译（含 `--features frame-profiler`）、完整 debug 构建、`cargo test`、UI/bin 测试、文档自检、5 项 UI 不变量、亮暗对比度、便携包 pre-flight、headless 冒烟 —— 全部通过且**未启动窗口** |

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
| A1 | **手动验收清单**：把散在 `pending/browse.md`、`ui-refactor.md`、`PLAN.md`、`pending/library-sources-formats.md` 的 GUI/UIA/截图/回归项整理成可勾选清单，标注每项的前台依赖与证据路径 | `docs/backlog/manual-acceptance.md`（待写） | 纯文档，零风险 |
| A2 | 关闭已被取代的方案：F-070 方案 B 在方案 A 落地后失去意义；方案 C 的收敛项已被 A 继承 | registry / overview 的状态备注 | 纯文档 |
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

- [ ] 四种上下文侧栏（图库 / 标签 / 设置 / 去重）的亮暗 × 1024/1440/1920 截图基线
- [ ] 设置页分区锚点点击后是否真的滚到对应卡片
- [ ] 标签页侧栏搜索 / chips / 排序与页内列表联动，屏幕内只有一套搜索
- [ ] 3 万标签下侧栏 chips 换行、虚拟列表滚动与搜索延迟
- [ ] 侧栏 UIA 名称、焦点环、Tab 顺序、Esc 分层
- [ ] 回归：F-006 侧栏宽度/折叠（含旧 `ui.sidebar_collapsed` 升级后不丢）、F-011 标签面、F-020 别名合并、F-025 低频清理、F-067 撤销、方案 C 的「查看图片 → Esc 返回 → 状态保留」

**F-073 全局 UI**

- [ ] 1440/1920 与浮层/查看器截图基线补四种新侧栏
- [ ] GUI 弹窗 / 菜单 / 表单 / 命令面板 UIA 与悬停验收（tooltip、loading spinner、`.invalid()` 红框、清除按钮对 150/240px 固定宽输入的挤压）
- [ ] 渲染后 WCAG 实测（现有 `check_theme_contrast.py` 只是 token 设计值）
- [ ] 帧率 `present p99`：必须窗口前台，`run_frame_probe.ps1` 每秒置前，失焦样本为 0，数据作废
- [ ] 100k 滚动峰值工作集 326–329MB（`KANALIB_UI_SCROLL_TEST`，需前台）

**F-069 / F-074 / 其它真实环境**

- [ ] F-069：100MP 超大图阈值与提示、`0`/`1` 与双击切换、按键验收
- [ ] F-074：宽色域 ICC 样本、超大图阈值、连续切图内存与资源释放、缺失/损坏原图
- [ ] F-030：真实 OneDrive/网络盘事件语义与断线重放（环境可用但未获授权写入，未实测）
- [ ] F-033：HEIC/HEIF、RAW 真实样本与可分发解码链（仓库无样本、LGPL ffmpeg 无对应 demuxer ⇒ 维持「不支持」）
- [ ] F-036：真实资源管理器拖放导入
- [ ] 中文 IME 组字/候选窗、Ctrl 多选、系统播放器入口

> **会抢前台、只由用户手动执行**的脚本：`run_frame_probe.ps1`、`focus_window.ps1`、`ui_baseline.ps1`（需 `-Hwnd`）、`capture_window.ps1`、`uia_*`、`window_type.ps1`、`window_wheel.ps1`。

### D. 需要方案决策（不得自行定）

| # | 任务 | 为什么需要拍板 |
|---|---|---|
| D1 | error 通用重试入口 | 6 处打开失败已「toast + 就地说明」，按钮本身可再点即天然重试；要做通用重试控件需新增布局元素或让全局 `shared.error` 携带动作，属结构设计 |
| D2 | 项目级 i18n | Kanalib 侧文案体系未定；vendor 已用组件的英文默认值均被业务侧覆盖、`ely-media` 未接入，改动面与命名规则需先定 |
| D3 | F-070 方案 B | 已被方案 A 取代，建议直接关闭而非实现 |

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
| **A1 整理手动验收清单（推荐）** | 纯文档、零风险、不抢前台，方便后续逐项走 C 类 |
| B1 启动 100k 后台全量回归 | 约 1 小时以上，纯后台；需要先确认 |
| D1 先出 error 重试方案 | 只出可评审方案，拍板后再动代码 |
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
