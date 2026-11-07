# Kanalib 轻量测试流程

> 状态：已审核（2026-11-07），自即日起作为后续开发的测试约束。
>
> 目标：给后续开发一条「最小但足够」的验证路径。不新增测试框架、不新增 CI、不追求覆盖率，只规定**什么改动跑什么测试、什么不该测**。
>
> 适用范围：Rust workspace（core / UI / bin）、sidecar、脚本、便携打包。安全、性能、缓存、许可等既有约束不因本文放宽。

## 1. 原则

1. **风险定档，不按改动行数定档。** 数据安全、删除/恢复、扫描监听、远程凭据、性能热路径、打包许可从严；文案、样式、注释从轻。
2. **能下层测的不上层测。** 逻辑放单元/隔离测试，跨模块放集成，UI 只守真实用户可见且下层测不到的部分。
3. **同一行为只在一个层级守护。** 上层测试只验证它独有的集成/交互行为，不重复断言下层已经覆盖的细节。
4. **测行为，不测实现。** 断言调用方可观察的结果；不断言私有字段、内部调用顺序、框架内部行为。可以测私有函数，但断言其输出/状态变化。
5. **默认最小验证。** 只有触发升级条件才扩大范围；不因为「顺手」就跑全量、100k 或整套 GUI 验收。
6. **默认确定性。** 默认套件不依赖网络、真实图库、ffmpeg、sidecar 或 GUI；这些放按需层，并用环境变量或 `#[ignore]` 明确隔离。

## 2. 分层

| 层级 | 定位 | 允许 | 禁止 | 典型入口 |
|---|---|---|---|---|
| L0 静态门禁 | 不运行程序，只查编译/结构/文档一致性 | `cargo check`、只读脚本 | 启动 GUI、建库、跑扫描 | `cargo check --workspace`、`check_ui_invariants.py`、`check_backlog_docs.sh`、`check_theme_contrast.py` |
| L1 隔离测试 | 单进程、可重复的单元与本地集成 | 临时目录、临时 SQLite、`127.0.0.1:0` 假服务、mock HTTP | 真实图库、真实网络、ffmpeg/sidecar、GUI、固定端口 | inline `#[cfg(test)]`、`cargo test -p <crate> <filter>` |
| L2 headless 冒烟 | 不建窗，跑真实 core 链路 | `KANALIB_HEADLESS=1` + `sample_library`；可选 ffmpeg/sidecar | GUI、100k、真实用户库 | 现有 headless 命令（见第 8 节） |
| L3 人工 GUI | 真实渲染/输入/系统集成 | 副屏、隔离数据根、自己启动的 PID、用户明确授权 | 主屏抢前台、全局键鼠、操作真实图库 | `docs/backlog/manual-acceptance.md` |
| S 特殊验证 | 长耗时/高成本/外部环境 | 100k 性能、便携包、真实 Immich/WebDAV/OneDrive/SMB | 每轮例行执行 | 需用户单独确认 |

补充口径：

- L1 的「隔离」指数据和外部依赖可控：临时根目录、临时库、本机回环假服务；不是要求纯函数。
- L2 只证明 core 主链路能跑通，不替代 L1 的边界断言；`thumb_errors` 等预期噪声要看夹具说明。
- L3 只验「本次改动影响的可观察行为」；未改的页面/状态不重复验收。
- 性能/100k、真实远端、便携全量包属于 S，不进默认门禁。

## 3. 风险定档

| 档位 | 典型改动 | 默认验证 | 升级条件 |
|---|---|---|---|
| 高 | 删除/回收站/保护路径、备份恢复、schema/迁移、扫描/监听/忽略、远程源凭据、性能热路径、依赖/许可/打包 | 相关 L1 + 相关 L0；批次收口跑完整 core | 涉及真实磁盘/真实远端/100k/便携全量时，需用户确认后做 S |
| 中 | core 业务逻辑、搜索 SQL、UI 状态与交互逻辑、i18n 目录 | 定向 L1 + 相关 L0 | 改动跨模块或影响主浏览路径时，批次收口加 L2/完整 crate |
| 低 | 文档、注释、纯文案、样式常量、局部布局微调 | 对应 L0；文档只跑文档自检 | 出现真实视觉/交互变化时，只对改动状态做一次 L3 |

判断顺序：先问「最坏会坏什么」，再问「哪一层能最早发现它」，最后才决定跑多少。

## 4. 改动类型 → 最小验证

| 改动类型 | 默认最小验证 | 何时升级 | 明确不做 |
|---|---|---|---|
| 文档 / backlog / 注释 | `bash scripts/check_backlog_docs.sh`（仅 backlog 改动） | 文档涉及验收结论时，回查对应证据 | 编译、跑测试、开 GUI |
| 文案 / i18n / 本地化目录 | `cargo test -p kanalib-ui localization` + `python scripts/check_ui_invariants.py` | 涉及颜色 token 才加 `check_theme_contrast.py` | 为每条文案开 GUI 截图 |
| core 纯逻辑（解析/格式化/筛选/搜索 SQL） | `cargo test -p kanalib-core <模块过滤>` | 跨模块或改公共接口时，批次收口跑 `cargo test -p kanalib-core` | 同时跑 UI/headless |
| DB / 备份 / schema / settings | 定向 core DB 测试 + 批次收口完整 core | 涉及恢复/迁移/损坏库时，加失败路径与重启场景 | 用真实图库 DB 做测试 |
| 扫描 / 监听 / 忽略 / 移除 | 定向 core sources 测试 | 扫描主链路变化时加一次 L2 `sample_library` | 每轮生成 100k |
| 远程源 Immich/WebDAV | 现有 mock HTTP 测试 | 凭据/交互流程变化时才做一次用户授权的真实连接 | 默认连真实服务器 |
| UI 逻辑/状态（非渲染） | 定向 `cargo test -p kanalib-ui <过滤>` + 不变量脚本 | 影响共享状态或主路径时，批次收口跑完整 UI | 用截图替代逻辑断言 |
| UI 视觉/布局/交互 | 不变量脚本 + 改动状态的一次 L3 截图/操作（需授权） | 设计 token/布局体系整体调整时才做亮暗×多宽度矩阵 | 每个小改动重拍全量基线 |
| 性能热路径（搜索/缩略图/相似/滚动） | 同一夹具上的定向测量 | 需要 100k 或改变基线口径时，先经用户确认 | 每轮例行跑 100k |
| ffmpeg / sidecar / 格式 | 默认跳过 `#[ignore]`；有运行时才跑 gated 测试 | 格式解码/模型契约变化时，加对应 gated 测试 + 一次 L2 | 把 ffmpeg/sidecar 变成默认测试依赖 |
| 依赖 / 打包 / 许可 | `build_portable.ps1 -ValidateOnly` | 发布前跑完整打包与清单校验 | 为依赖变更跑 GUI 验收 |
| vendor / 上游同步 | `cargo check --workspace` + 项目定向测试 | 同步影响公共组件时，批次收口跑完整 default-members | 跑 vendor 自带测试（见第 8 节） |
| Bug 修复 | **在最低能复现的层加 1 个回归测试**；GUI-only 问题用一次 L3 复现并留证据（需授权） | 影响数据安全/性能时按高风险档处理 | 为单个 bug 新建 UI 自动化框架/大夹具 |

## 5. 门禁

| 时点 | 必跑 | 按需 | 不跑 |
|---|---|---|---|
| 每次编辑 | 相关 L0 + 改动点的定向 L1 | UI 改动加不变量脚本 | 全量测试、GUI、100k |
| 每次提交 | `cargo check --workspace`；改动 crate 的定向测试；文档改动跑文档自检 | UI 改动跑不变量；i18n 跑 localization | `cargo test --workspace`；全量 `cargo test` |
| 批次/轮次收口 | 默认成员完整 `cargo test`；相关只读脚本 | core 主链路变化时加一次 L2；只对改动页面做 L3 | 未改页面的重复 GUI 验收；100k |
| 发布/便携前 | 默认成员完整测试 + 不变量/文档自检 + 便携 pre-flight | 改动过的 UI 做一次 L3；热路径变化才做性能 | 未授权的前台窗口/真实远端 |

固定口径：

- 默认成员 = `.`、`crates/kanalib-core`、`crates/kanalib-ui`；直接 `cargo test` 即可。
- **不把 `cargo test --workspace` 当门禁。** 它会连带 vendor Ely 测试，当前在 gpui-pre 0.3.10 下存在既有编译失败。
- `cargo check --features frame-profiler` 只在改 `alloc_stats`/帧计时/取证路径时跑，不作为日常门禁。
- `check_theme_contrast.py` 只在改颜色 token 时跑；它算设计值，不等于渲染后 WCAG。
- `check_ui_invariants.py` 是 UI 改动的默认静态门禁，不需要窗口。

## 6. 测试预算（防止过度测试）

- **不设覆盖率目标。** 覆盖率只用于发现明显空白，不作为完成标准。
- **新增测试必须能回答「它防哪个具体回归」。** 答不上来就不加。
- **不重复覆盖。** 同一个断言只在一个层级守护；下层已覆盖的细节，上层不再重复。
- **不测显然正确的东西。** 纯常量、getter、静态文案、样式数值、第三方库/框架行为不写测试。
- **不为 UI 像素/快照建 golden 测试。** 渲染受字体、DPI、主题、平台影响；视觉用 L3 抽样，不用自动化像素比对。
- **不新增 UI 自动化框架、测试依赖或 CI。** 需要时先征得用户同意。
- **不把外部依赖带进默认套件。** 网络、真实图库、ffmpeg、sidecar、GUI 默认都不参与。
- **不用 sleep、固定端口、测试顺序依赖或重试掩盖不稳定。** 假服务用 `127.0.0.1:0`，时间/随机数尽量注入。
- **不把既有失败写成通过。** 环境性失败单独记录，不重复追查无关失败。
- **不为每个小改动重跑全量。** 全量只在提交前按 crate、批次收口、发布前跑。
- **不把 100k/帧率/真实远端当日常回归。** 只在热路径变更且用户确认时做。

## 7. 测试放置与稳定性

- **单元测试**：与实现同文件的 `#[cfg(test)] mod tests`；这是当前项目的主要形态。
- **集成测试**：只有需要验证公共 API/跨 crate 契约时才建 `tests/`；共享工具放 `tests/common/mod.rs`，避免每个文件重复。
- **命名**：`被测对象_条件_预期`，例如 `parse_query_keeps_tag_alias`；一个测试只表达一个失败理由。
- **数据隔离**：每个测试独立临时目录/临时库；不读真实图库；结束后清理；并发安全。
- **外部依赖门控**：需要 ffmpeg/sidecar/真实运行时的测试用 `#[ignore = "requires ..."]`，默认不跑；原因写清环境变量，不能只写 `ignore`。
- **不稳定测试**：先分类是产品缺陷、测试缺陷还是环境问题；能修就修，不能修就带原因隔离，不用重试或放宽断言掩盖。
- **实现耦合测试**：如果一次内部重构就导致大量测试失败，优先改写/删除测试，而不是阻止重构；但行为变更必须同步更新断言并说明原因。

## 8. 证据与汇报

- 每次验证只汇报四类结果：**通过 / 失败 / 未跑 / 既有失败**，并写清命令与范围。
- 只跑了子集就写「定向测试通过」，不写「全部通过」。
- 既有环境性失败单独列出，不混入本次改动结论；不要为了「全绿」反复重跑无关测试。
- GUI/UIA/截图证据统一放 `.kanalib/<轮次>/`；只记录改动的页面/状态，复用 `manual-acceptance.md` 已有条目，不重复拍未变页面。
- 已知基线（当前口径，不是测试数量目标）：
  - 默认 `cargo test` 覆盖 bin/core/UI；历史上 core 约 214 通过 + 4 个按需忽略，UI 约 73–74，bin 6。
  - `sources::tests::immich_missing_key_and_disconnect_do_not_block_local_library` 曾因本机网络错误分类差异间歇失败；未改相关代码时按既有环境问题记录。
  - `cargo test --workspace` 会连带 vendor 测试编译失败；日常不用。
  - 测试数量会随功能变化，不把某一轮的数字当门禁。

常用命令（全部后台可跑）：

```bash
cargo check --workspace
cargo test -p kanalib-core <过滤词>
cargo test -p kanalib-ui <过滤词>
cargo test -p kanalib
cargo test                          # default-members 完整回归
python scripts/check_ui_invariants.py
python scripts/check_theme_contrast.py
bash scripts/check_backlog_docs.sh
```

headless 冒烟（只在 core 主链路变化或批次收口时跑）：

```bash
KANALIB_HEADLESS=1 KANALIB_ROOT=<隔离根> KANALIB_SKIP_TAG=1 \
KANALIB_SEARCH_RUNS=30 KANALIB_SIMILAR_SCAN=1 \
KANALIB_LOG_FILE=<日志> KANALIB_METRICS_PATH=<jsonl> \
target/debug/kanalib.exe sample_library
```

便携 pre-flight（只在依赖/打包/许可变化或发布前跑）：

```powershell
pwsh -NoLogo -NoProfile -File scripts/build_portable.ps1 -ValidateOnly
```

## 9. 例外与审批

以下事项需要用户明确同意，不能由执行者自行扩大：

- 新增测试框架、测试依赖、CI 或自动化 UI 驱动。
- 运行 100k/长耗时性能测试，或改变既有性能基线口径。
- 启动 GUI、抢占前台、使用真实图库/真实远端账号。
- 删除既有测试或降低断言强度（行为变更导致的正常更新除外，但要在提交说明里写原因）。

如果紧急修复来不及补测试：先做最低层定向验证 + 记录未覆盖点，不能静默跳过；后续补一个最小回归测试，不为补测试重建整套夹具。

## 10. 维护

- 每轮收口时顺手检查：有没有重复覆盖、长期不稳定、只测实现细节的测试；有就合并/改写/删除。
- 新测试类型（快照、属性测试、UI 自动化等）先走第 9 节审批，再进默认流程。
- 本流程自身不设「测试数量」或「覆盖率」考核；如果它开始拖慢开发，按「减少层级、减少重复」的方向修订，而不是加更多门禁。

## 11. 审核结论（2026-11-07）

| # | 事项 | 结论 |
|---|---|---|
| 1 | UI 视觉改动只验「改动状态」，不再每轮做全页面/全矩阵截图 | **是**：按第 4 节执行，只有设计 token/布局体系整体调整才扩大矩阵 |
| 2 | 默认套件以 default-members 为准，禁止把 `cargo test --workspace` 当门禁 | **是**：第 5、8 节已固化；vendor 测试不进入项目门禁 |
| 3 | 100k/性能/真实远端只在热路径变更或用户明确批准时跑 | **是**：归入 S 类，不进入日常回归 |
| 4 | 暂不新增 UI 自动化框架、测试依赖与 CI | **是**：需要时按第 9 节单独审批 |

## 参考依据

- Martin Fowler, *The Practical Test Pyramid*：<https://martinfowler.com/articles/practical-test-pyramid.html>
- Google Testing Blog, *Test Sizes*：<https://testing.googleblog.com/2010/12/test-sizes.html>
- Google Testing Blog, *Just Say No to More End-to-End Tests*：<https://testing.googleblog.com/2015/04/just-say-no-to-more-end-to-end-tests.html>
- *Software Engineering at Google*, Ch. 11 Testing Overview：<https://abseil.io/resources/swe-book/html/ch11.html>
- The Rust Programming Language, *Test Organization*：<https://doc.rust-lang.org/book/ch11-03-test-organization.html>
- Tricentis, *Risk-based testing*：<https://www.tricentis.com/learn/risk-based-testing>
- 桌面 UI 自动化实践（UI 测试以 smoke 为界）：<https://comcomponent.com/zh-CN/blog/windows-desktop-ui-automation-testing/>
