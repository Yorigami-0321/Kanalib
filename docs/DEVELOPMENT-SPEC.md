# Kanalib 开发规范

> 上级入口：[FEATURE-BACKLOG.md](FEATURE-BACKLOG.md)
>
> 依据：本项目 2026-11-09 至 2026-11-23 的协作记录（`.qoder/sessions/redacted/` 共 8 个会话、32 份压缩摘要）、`docs/backlog/` 现行文档与提交历史。
> 本文件规定**怎么协作、用什么工具、怎么改、怎么连服务器、怎么证明改对了、怎么提交**；测试分层见 [TESTING-PROCESS.md](TESTING-PROCESS.md)，硬约束见 [backlog/constraints.md](backlog/constraints.md)，视觉硬规则见 [backlog/photo-wall-v3.md](backlog/photo-wall-v3.md)，数据契约见 [backlog/photo-wall-contract.md](backlog/photo-wall-contract.md)，部署模板见 [../deploy/README.md](../deploy/README.md)。本文不重复这些内容。

## 1. 效力与适用范围

- 适用于 `web/`（图片墙前端）、`crates/`（core / UI / publish / ingest / web）、`scripts/`（探针与校验）与 `docs/`（文档与登记）。
- 与既有文档冲突时：`constraints.md` 的安全/数据约束 > 契约文档 > 本文 > 会话中的临时说法。
- 会话里的口头结论只有落到代码、契约或 `photo-wall-status.md` / `registry.md` 才算生效；**归档不等于通过验收**（[archive/README.md](archive/README.md)）。

## 2. 事实来源优先级

1. **代码与实测**：行号、探针输出、网络/像素/帧率数字。
2. **契约与规格**：`photo-wall-contract.md`、`photo-wall-v3.md`、`web-rail-horizontal.md`。
3. **现状与登记**：`photo-wall-status.md`、`registry.md`、`pending/overview.md`。
4. **归档与历史会话**：只作追溯，不作执行依据。

规则：

- 不拿旧文档当需求依据；引用旧结论前先回代码或现行文档核对。
- 行号会漂移，文档里定位优先写**函数名/选择器/常量名**，行号只作辅助。
- 结论必须可复算：写清用的是哪份库（937 条真实库 / 合成库 / 夹具）、哪个视口、哪次提交。

## 3. 角色与决策权

| 角色 | 职责 | 禁止 |
|---|---|---|
| 人类 owner（Yorigami_Nya） | 产品口径、优先级、是否上线、是否提交/推送 | — |
| 协调者（管理员一号） | 拆分任务、划定写域、评审、统一提交、文档收口 | **不改代码**（用户明确要求："你的职责不是修改代码"） |
| 实现者（前端/UI/后端） | 在写域内实现、自测、出证据、报残余风险 | 越域改文件、自行提交/推送、替用户做产品决策 |
| 审计者 | 只读走查，输出问题清单与验收标准 | 审计阶段改代码 |

规则：

- 产品决策（内容分级、导航形态、是否上线、是否推送）只能由人类 owner 拍板；执行者不代答、不"顺手实现"。
- 协调者发现缺陷时派单，不自己动手；紧急修复也必须走"派单 → 实现 → 评审"。
- 并发按 **4 条车道**估算；超过时排队，不靠加人硬塞。

## 4. 阶段流转

```
审计（只读） → 合并问题清单 → 分批派单（写域+验收） → 实现（不提交）
   → 实现者报告（证据+数字+残余风险） → 协调者复验 → 冻结该批 → 统一提交
```

硬性要求：

1. **审计阶段只读**：可以起服务、跑探针、截图，不改仓库文件。
2. **一批一冻结**：某批验收通过后即冻结，后续改动另开批次，不在已验收文件上继续叠加。
3. **提交权单一**：实现者一律"不 commit、不 push"，由协调者评审后统一提交；用户明确说"提交"才提交，说"推送先往后放"就不推。
4. **部署要当次授权**：网页与入库链路已经上线（F-083 / F-088 / F-089 均已在云端与 box 实机运行），但日常批次仍只交付本地预览。部署、重启线上服务、改 box 配置都需要 owner 在当次明确同意，不因"上次部署过"默认放行。

## 5. 任务派发契约

每个任务必须同时给出以下五项，缺一项就不算可执行任务：

1. **目标与现状**：缺陷是什么、在哪复现、期望行为。
2. **写域**：允许修改的文件清单（精确到文件）。
3. **禁区**：明确不能碰的文件/目录（如 `Sidebar.tsx`、`sidebar.css`、`web/public/data`）。
4. **验收**：可执行命令 + 可判定结果（数字、断言、截图状态）。
5. **纪律**：是否提交、证据放哪、完成后向谁报告。

派发模板：

```text
【批次/编号】
目标：<缺陷或需求>；复现：<路由/视口/操作>；期望：<可观察结果>
写域：<file A>、<file B>
禁区：<file C>、<file D>、web/public/data、源图库
验收：<命令 1>；<命令 2>；<探针> passed；<数字门槛>
纪律：不提交、不 push；证据放 <目录>；完成后报告 <角色>
```

## 6. 写域与并行

- **一人一文件集**：同一时刻一个文件只有一个写者。共享文件（`App.tsx`、`Wall.tsx`、`world.ts`、`styles.css`）默认不并行拆给两人。
- **跨域发现只报告**：需要改别人的文件时，停下来说明"改哪里、为什么、验收是什么"，等授权；不得"先改了再说"。
- **不覆盖他人未提交改动**：工作区里别人的未提交修改视为在途资产，禁止回退、格式化或整文件重写。
- **契约先行**：跨端字段（`index.json`、`search-index.json`、`WallItemDto`）先改契约文档，再同步两侧；契约冻结由一个人完成，其他人只读。
- **DOM/结构契约**：组件结构一旦被多方依赖（如 `aside.inspector > handle + scroll + actions`），先写进任务说明，CSS 与 TSX 各自按契约实现。

## 7. 证据规范

### 7.1 什么算证据

- `文件:行号` + 代码事实；或
- 实测数字（体积、耗时、p95、帧率、像素亮度、请求数、DOM 数）；或
- 探针 `passed: true` 的结构化输出；或
- 改前/改后成对截图（视觉类必须成对）。

### 7.2 硬规则

- **只报四类结果**：通过 / 失败 / 未跑 / 既有失败。跑了子集就写"定向通过"，不写"全部通过"。
- **不把探针假象当产品缺陷**：合成点击失败、缓存命中导致 0 请求、headless 的 `3g` 判定等，先排除测试缺陷再上报。
- **不过度声称**：验证了 A 就不能说验证了 A+B。会话中真实出现过"详情翻页 replace 已验证"只证明了"一次返回回到列表"，被协调者纠正——按实际断言范围写结论。
- **证据不入库**：截图、报告、临时脚本放 `web/screenshots/`、`.kanalib/<轮次>/`、`%TEMP%` 或共享目录，均已被 `.gitignore` 覆盖；**截图含库内容时只留本地**，不上传、不入库。
- **改前/改后成对**：缺陷修复必须能指出"改前失败、改后通过"的同一断言。

### 7.3 交付报告模板

一批完成后只发一条公开消息，包含：

```text
改动文件：<逐项列出，注明新增/修改>
逐项验收：<每条需求 → 命令/探针 → 结果数字>
证据路径：<截图目录、report.json、探针输出>
未完成项：<没做的、做不了的、以及原因>
残余风险：<未覆盖的场景、headless/合成输入的局限>
状态：未提交、未 push（或按指示写明已提交的 commit）
```

不写"基本完成""应该没问题"这类无法判定的结论；不把未跑项藏在段落里。

## 8. 缺陷修复流程

1. **先复现**：给出路由、视口、缩放、操作序列；能写成探针就写成探针。
2. **找最小根因**：区分几何/像素/状态/时序/缓存，不用"加个 `!important`/加个延时"掩盖。
3. **最小改动**：只动根因所在层；不顺手重构、不夹带文案或格式变更。
4. **固化回归**：把断言写进对应探针（`scripts/wall_probe_*.mjs`），避免同类问题复发。
5. **报残余风险**：明确"仍未覆盖什么"，例如 headless 不代表真机手感、`:has` 的浏览器下限、缓存导致的重复下载。

歧义处理：用户反馈与截图不一致、或一句话有多种解释时，**先问再做**。会话中"侧边栏不能被穿透"被误解为"侧边栏不可见"，导致返工并被用户点名——不确定就问，不要猜。

## 9. 验证与门禁

### 9.1 前端（`web/`）

```powershell
Push-Location web
npm run build                      # 已含 tsc --noEmit，无需再单独跑
Pop-Location

# preview 是常驻进程：后台起，别前台阻塞
Start-Process pwsh -WindowStyle Hidden -ArgumentList '-NoLogo','-NoProfile','-Command',
  'cd D:\Code\Kanalib\web; npx vite preview --port 4273 --strictPort --host 127.0.0.1 *> $env:TEMP\kanalib-preview.log'
Start-Sleep 5; (Invoke-WebRequest http://127.0.0.1:4273/ -UseBasicParsing -TimeoutSec 5).StatusCode   # 期望 200

# 探针必须在仓库根跑，--out 相对路径也按仓库根解析
node scripts/wall_shots.mjs --base http://127.0.0.1:4273/ --out web/screenshots/<批次>
```

- `npm run build` = `tsc --noEmit && vite build`；单独再跑 `tsc` 只用于定位类型错误，不作为额外门禁。
- 探针一律从**仓库根**执行（`scripts/` 在根目录，不在 `web/scripts`）。
- **先确认探针真的跑了**：必须有新生成的 `report.json`。本机 Chrome 157 / Edge 158 在缺 `--remote-allow-origins=*` 时会连上 CDP 后立刻断开，进程以 `0` 退出且不写报告——这是假通过。跑完先 `Test-Path <out>\report.json` 并看 `LastWriteTime`。
- 本机 headless 的 GPU 进程会崩（`GPU process isn't usable`），浏览器要带 `--disable-gpu`；现有 `scripts/*.mjs` 没带这个开关，跑之前按 14.4 处理。
- 探针按改动面选择，不全量重跑。`wall_shots` 是一次约 50 秒的全页扫描（首页/详情/标签/概要/390/交互/远走），只在改布局、动效、画布或交互时跑：

| 改动面 | 必跑 |
|---|---|
| 布局/动效/画布/交互/跨页面 | `npm run build` + `wall_shots`（判 `report.json` 的 `failures`、`consoleErrors`） |
| 单组件样式、文案、局部颜色 | `npm run build` + 对应状态的一次真实浏览器检查（需要时补该组件的探针） |
| 数据加载/失败路径 | `wall_probe_recover.mjs` |
| 搜索/URL/深链 | `wall_probe_state.mjs`、`wall_probe_cold_search.mjs` |
| 详情/翻页 | `wall_probe_detail_nav.mjs`（默认 `http://127.0.0.1:4288/`，必须显式传 `--base`） |
| 详情预取/缓存 | `wall_probe_detail_preload.mjs`（自带静态服务，用 `--serve-dist web/dist`） |
| 标签墙/无界/阈值 | `wall_probe_tag360.mjs`、`wall_probe_tag_threshold.mjs` |
| 边界态/空态/404 | `wall_probe_edge.mjs` |
| 移动端检查器 | `wall_probe_inspector_sheet.mjs` |
| 遮罩/边缘像素 | `wall_probe_mask_edge.mjs`（带 `--label before/after`） |

- 11 个 `wall_probe_*.mjs` / `wall_api_check` 输出 `passed`；`wall_shots` / `wall_perf` / `wall_ui_check`（写 `css-check.json`）/ `wall_wheel_perf` / `wall_hover_edge_check` 用 `failures: []` 判定。两种口径不要混写。
- `wall_shots` 在本机稳定报 5 条失败，其中 3 条已定位为探针口径问题：390 下 `.seam` 变成底部 tab 条、`getBoundingClientRect().right` 等于 390，于是 `coverage()` 从 `seam + 20 = 410` 开始采样（`total` 恒为 0 → `cover: 0`）、长按选点也用 `seam + 20` 作左边（找不到卡 → "找不到可点的卡片"）。截图本身铺满，无空白带。另外两条滚轮/触控板跟随误差需在带 GPU 的真实浏览器里复测后再定性。已知项在报告里单列，不算新回归，也不算"全部通过"。

### 9.2 Rust / 数据管线

```powershell
cargo check --workspace
cargo test                          # default-members：. / kanalib-core / kanalib-publish / kanalib-ui
python scripts/wall_manifest_check.py web/public/data
```

- **不把 `cargo test --workspace` 当门禁**（会连带 vendor 测试的既有编译失败）。
- `cargo test` 已含 `kanalib-publish`，不必再单独跑 `cargo test -p kanalib-publish`；只有需要单独定位发布器时才用 `-p` 收窄。
- 发布器改动必须验证**二跑幂等**（第二次不重编、不重写）。
- 真实库重导（`LEDGER_VERSION` 升级触发全量重编）属于高成本操作，需用户单独确认，不在日常批次里执行。当前 `web/dist/data` 实测约 398 MB / 3009 张 jpg。

### 9.3 视觉与真实设备

- 视觉缺陷先在真实浏览器里看（用户明确要求"直接开浏览器控制，尽量减少不必要的多余测试"），再决定是否补探针。
- headless 走 SwiftShader，**帧率与 GPU 合成成本不代表真机**；触控板/触屏手感必须标注"待实机确认"。
- 只验改动影响的状态，不复拍未改页面；同一断言不在多层重复。

## 10. 代码规范

### 10.1 通用

- 不引入第二套 UI 组件库；新增运行时依赖需先确认（当前前端运行时依赖只有 `react` / `react-dom`）。
- 不新增测试框架、CI、UI 自动化依赖（见 `TESTING-PROCESS.md` §9）。
- 不做"仅注释/仅格式"的伪改动来回应缺陷；用户明确要求过"不要做注释的改动"。
- 不修与任务无关的既有问题；发现了就报告，不夹带。
- 注释只解释"为什么"，不复述代码；中文注释与现有文件保持一致。

### 10.2 TypeScript / React

- 严格类型：不用 `as any`、`@ts-ignore`、`eslint-disable` 绕过（当前 `web/src` 为 0 命中，保持）。
- 派生数据用 `useMemo`，回调用 `useCallback` 并给稳定依赖；不在渲染期写 `ref`（改 `useEffect`）。
- 副作用必须可清理：`rAF`、定时器、`ResizeObserver`、事件监听、`pointercapture` 在卸载/切换时释放。
- 请求失败要保留已加载数据，游标只在成功后推进；重试需要 cache-bust（`?r=<nonce>`），否则浏览器复用失败资源。
- 不引入路由/状态库；hash 路由的唯一写入口保持在 `lib/router.ts`，`replaceState` 后必须通知订阅者（`ROUTE_CHANGE` 事件），不能只依赖 `hashchange`。
- 状态写入 URL 时保持单一写者（`lastWritten` 语义），避免 React state 与 hash 事件互相触发。

### 10.3 CSS

- 遵守 `photo-wall-v3.md` 十一条硬规则：无首页脚手架、一状态一视觉通道、角标不 blur/圆角/渐变、焦点用内描边、列表 `cover`/详情 `contain`、背景层不拦指针、390px 单手可用。
- 遮罩/描边类问题优先在**像素层**验证：分数缩放（1.2635/1.6）与 DPR 2.5 下四边最外 2px 亮度差 ≤3/255。
- 装饰环画在遮罩之上（`.tile__ring`，`pointer-events:none`），不靠透光细线表达状态。
- 主断点是 900 / 760 / 560，另有 `inspector.css` 的 `430px`（只调检查器标题字号）。新增断点要说明理由并写进 `photo-wall-v3.md`；改断点前先 `rg -n "@media \(max-width" web/src` 确认没有第二个同值块被漏改。
- 不覆盖别人的未提交 CSS；同一选择器的修改先确认归属。

### 10.4 Rust

- 新代码过 `rustfmt` 与 `clippy`；不顺手格式化历史文件（避免无关 diff）。
- 错误路径 fail-closed：缺配置、缺规则文件时拒绝启动，不静默放行。
- 写文件用显式 UTF-8（Windows 默认 GBK 会导致校验器报"不是 UTF-8"）。
- 发布器：先写同级暂存目录再整体替换；`fs::copy` 前断开硬链接，避免截断旧产物。

## 11. 数据与安全边界

- `web/public/data/` 与源图库**只读**；测试用隔离根（`KANALIB_ROOT`、临时目录），不写真实库。
- 不把真实库导出、截图、日志提交进仓库；`web/public/data`、`web/screenshots/`、`.kanalib/` 已在 `.gitignore`。
- 凭据（token、HMAC key、refresh token）不进日志、不进诊断快照、不进报告；诊断输出需脱敏 URL 凭据与 token 类键值。
- 删除类操作只进回收站且仅限本地固定磁盘；保护路径永不可移入回收站（详见 `constraints.md`）。
- 云端只读：网页端不做账号、上传、编辑、评论。

## 12. 文档规范

- 新需求：先在 `backlog/registry.md` 加 `F-xxx`，再写 `pending/` 对应文件；实现后移入 `implemented.md`，只留约束与验证要点。
- 文档地图只维护入口，不在 `FEATURE-BACKLOG.md` 里堆条目。
- 过期内容**归档或删除**，不留在现行文档里当依据；归档时修正相对链接，并在 `archive/README.md` 补一行。
- 每轮收口跑 `& 'C:\Program Files\Git\bin\bash.exe' -lc 'cd /d/Code/Kanalib && bash scripts/check_backlog_docs.sh'`，校验 registry 不重不漏、implemented 与 pending 不矛盾。脚本内部硬编码了 `cd /d/Code/Kanalib/docs/backlog`，换检出路径会直接退出 1；退出码 0 才算过，输出末尾是 `== 校验结束 ==`。
- 不写工时与排期；写依赖顺序、写域与验收。
- 文档里的数字必须带口径（哪份库、哪个视口、哪次提交）；数字变化时更新来源页，不在多处复写基线。

## 13. 提交规范

格式：`type(scope): 中文摘要`（Conventional Commits，摘要不加句号，一条提交只做一件事）。

| type | 用途 |
|---|---|
| `feat` | 新功能/新能力 |
| `fix` | 缺陷修复 |
| `docs` | 文档、登记、交接 |
| `test` | 探针、回归断言、夹具 |
| `chore` | 构建、忽略规则、依赖清理 |
| `style` | 纯样式/格式（不含行为变化） |

常用 scope：`wall`、`publish`、`ingest`、`tagger`、`web`、`desktop`、`ui`、`monitor`、`backlog`、`cloud`、`detail`、`testing`。

提交纪律：

- 按写域分批暂存；仓库存在已 staged 的无关 rename 时，先 `git reset` 再按域 `git add`，避免夹带。
- 提交前确认无凭据、无真实库内容、无截图产物。
- 只有用户明确说"提交"才提交；说"推送先往后放"就保持本地。
- 推送失败（网络/代理）不改写历史、不强推；记录失败原因并等指示。

## 14. 开发环境与工具链

### 14.1 本机实测版本（2026-11-23）

| 工具 | 版本 / 位置 | 备注 |
|---|---|---|
| PowerShell | `pwsh` 7.6.9（WindowsApps 下的 OpenAI runtime） | **唯一默认 shell**，见 14.2 |
| Node / npm | v26.13.0 / 11.20.0 | 探针只依赖内置 `fetch` / `WebSocket` |
| Rust / Cargo | 1.98.1 / 1.98.1 | 在 `%USERPROFILE%\.cargo\bin\`，**非登录 shell 不在 PATH** |
| Python | 3.14.7 | Windows CPython 无 `socket.AF_UNIX` |
| Git | 2.55.0.windows.3 | 仓库 `origin` = `github.com/Yorigami-0321/Kanalib` |
| OpenSSH | 9.5p2（`C:\Windows\System32\OpenSSH\`） | 有 `ssh`/`scp`/`sftp`，**无 `rsync`** |
| 其他 | `tar`、`curl` 随系统；无 `rsync`、无 `sqlite3` CLI | 压缩/传输用 `tar` + `scp` |

仓库结构：workspace 成员 `.`、`crates/kanalib-core`、`crates/kanalib-ingest`、`crates/kanalib-publish`、`crates/kanalib-ui`、`crates/kanalib-wall-model`、`crates/kanalib-web`、`vendor/ely-gpui-components`；`default-members` 为 `.` / `kanalib-core` / `kanalib-publish` / `kanalib-ui`（不含 ingest / web / wall-model，`cargo test` 不会覆盖它们，改这些 crate 要显式 `-p`）。

### 14.2 PowerShell 7 强制规则

1. **一律用 `pwsh`**：交互命令直接跑；脚本用 `pwsh -NoLogo -NoProfile -File <脚本>`。只有在 `pwsh` 不存在的机器上才回退 5.1，并在报告里注明。
2. **不写 Bash heredoc**：本机 Git Bash 在 `C:\Program Files\Git\bin\bash.exe`（`bash` 在 PATH 上的是 WSL 存根，没有发行版，执行即报错）；多行脚本写成 `.ps1` 或 `.py` 文件再执行，不用 `<<'EOF'`。确实需要 POSIX 工具时显式调 Git Bash 的绝对路径。
3. **显式 UTF-8**：`Get-Content -Encoding utf8`、`Set-Content -Encoding utf8`、`Out-File -Encoding utf8`；Python 侧 `encoding='utf-8'`，必要时 `$env:PYTHONIOENCODING='utf-8'`。否则中文在 GBK 控制台会乱码。
4. **Cargo 用绝对路径**：`& "$env:USERPROFILE\.cargo\bin\cargo.exe" test`；或先 `$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"`。
5. **路径带 `[` `]` 用 `-LiteralPath`**：如 `web/screenshots/...` 里的方括号会被当通配符。
6. **后台服务用 `Start-Process -WindowStyle Hidden`**，并重定向到日志；不要弹可见窗口。启动后必须用 `netstat -ano | Select-String ":<port>"` + 一次 HTTP 请求确认真的在服务，别只看日志为空就当成功。
7. **启动端口前先探测**：`netstat -ano | Select-String ":4273"`（本沙箱里 `Get-NetTCPConnection` 会因 CIM 访问被拒而返回空，不能用来判断端口）；再 `Invoke-WebRequest http://127.0.0.1:4273/ -UseBasicParsing` 确认返回 200。已被占用就复用并确认服务的是最新 `dist`，**不杀未知进程**。
8. **代理变量会破坏私网访问**：访问 EasyTier 地址时先清 `HTTP_PROXY`/`HTTPS_PROXY`，或 curl 加 `--noproxy '*'`。
9. **HTTPS 检查用 Node**：本机 Windows schannel 对 `patchouli.us.ci` 握手失败（`Invoke-WebRequest` 与 `curl.exe` 都报 `SEC_E_NO_CREDENTIALS` / 退出码 35，加 `--ssl-no-revoke` 无效），改用 `node -e "fetch('https://patchouli.us.ci/api/health').then(r=>r.text()).then(console.log)"`，实测返回 `{"ok":true,...}`。
10. **不把密钥写进命令行**：`ssh -i` 可以，但不要把私钥内容、HMAC、token 拼进命令或日志。

### 14.3 日常开发循环

```powershell
# 1. 定位：优先 rg，不用 findstr
rg -n "cellTilesInView" web/src

# 2. 改代码（只在写域内）
# 3. 构建（前端已含类型检查）
Push-Location web; npm run build; Pop-Location

# 4. 起预览（后台、固定端口）
Start-Process pwsh -WindowStyle Hidden -ArgumentList '-NoLogo','-NoProfile','-Command',
  'cd D:\Code\Kanalib\web; npx vite preview --port 4273 --strictPort --host 127.0.0.1 *> $env:TEMP\kanalib-preview.log'

# 5. 跑探针（从仓库根，不是 web/；先确认 4273 已在监听）
cd D:\Code\Kanalib
node scripts/wall_shots.mjs --base http://127.0.0.1:4273/ --out web/screenshots/<批次>

# 6. 收尾：停自己起的进程，证据留在 gitignore 目录
```

后台启动后先 `netstat -ano | Select-String ":4273"` 确认在监听，再请求一次首页；日志为空通常意味着子进程没起来（命令拼错），不是服务已就绪。

Rust 侧：

```powershell
& "$env:USERPROFILE\.cargo\bin\cargo.exe" check --workspace
& "$env:USERPROFILE\.cargo\bin\cargo.exe" test
python scripts\wall_manifest_check.py web/public/data
```

### 14.4 浏览器与探针

- 探针是**零依赖 CDP**（Node 原生 `fetch` + `WebSocket`），不引 Playwright/Puppeteer；Chrome/Edge 按 `scripts/*.mjs` 里的候选路径自动探测（本机 Chrome 157.0.8103.80 / Edge 158.0.4370.33）。
- **本机必须补两个浏览器参数**（脚本里还没有）：`--remote-allow-origins=*` 和 `--disable-gpu`。前者缺失时 Chrome 157+ 在 WebSocket 握手后立刻断开，探针以 `0` 退出、无 `report.json`；后者缺失时 GPU 进程崩溃（`GPU process isn't usable. Goodbye.`），`--screenshot` / `--dump-dom` 都拿不到输出，`wall_verify.ps1` 会报"浏览器没有产出截图"。补上后 `wall_shots` 约 50 秒跑完，落 11 张 PNG + `report.json`。
- `--headless=new` 在本机依赖 `--disable-gpu` 才能截图；不要再退回旧 `--headless`（`wall_verify.ps1` 里的重试分支同样会失败）。
- `browser-harness` 在本机不可用（依赖 Unix socket，Windows CPython 没有 `socket.AF_UNIX`）；不要为此装 WSL（`wsl.exe` 存在但无发行版，`bash` 直接报错），直接用 Node 脚本。
- CDP 常见坑：
  - 同 URL `Page.navigate` 不重载，深链/移动端场景要开新 target 或加查询串。
  - `Network.setBlockedURLs` 拦不住图片，用 `Fetch.enable` + `Fetch.failRequest`。
  - `Fetch.disable` 后可能残留 "Invalid InterceptionId"，过滤或等待。
  - headless 默认上报 `effectiveType: '3g'`，测快网需覆写 `NetworkInformation`。
  - `Emulation.setTouchEmulationEnabled({enabled:false, maxTouchPoints:0})` 非法；禁用时只传 `{enabled:false}`。
  - 合成点击要选完全在视口内、且不在缝/标题层下的元素；详情断言限定中心 pane。
  - 键盘激活按钮需 `Emulation.setFocusEmulationEnabled` + `keyDown`（不是 `rawKeyDown`）。
- 端口：dev `5273`、preview `4273`（`web/vite.config.ts`）。探针默认值不统一，**一律显式传 `--base`**：`wall_shots` / `wall_perf` / `wall_probe_anim` / `wall_probe_case` / `wall_probe_spot` / `wall_probe_wall` / `wall_probe_wheel` / `wall_repro` 默认 `4318`，`wall_probe_detail_nav` 默认 `4288`，`wall_probe_edge` 默认 `4281`，`wall_probe_cold_search` / `state` / `recover` / `tag360` / `tag_threshold` / `inspector_sheet` / `mask_edge` / `detail_preload` 默认 `4273`。不传就打到空端口，探针会以"没图/超时"失败，容易被误判成产品缺陷。
- 参数形式也不统一：`wall_probe_anim` / `case` / `spot` / `wall` / `wheel` 只认 `--base=URL`（等号），`wall_shots` / `perf` / `detail_nav` / `edge` / `recover` / `state` / `tag360` / `mask_edge` 用 `--base URL`（空格）；`wall_hover_edge_check` / `wall_ui_check` / `wall_wheel_perf` 走位置参数。
- Windows 路径：Git Bash 的 `/tmp` 是 MSYS 映射，与 `%TEMP%`（本机 `D:\AppData\Local\Temp`）不是同一位置；`check_backlog_docs.sh` 内部用 `/tmp` 没问题，但传给 Node/Python 的路径一律用绝对 Windows 路径或仓库相对路径。
- 探针里写 `--out $env:TEMP\...` 时 PowerShell 会先把 `$env:TEMP` 展开成 `D:\AppData\Local\Temp`，脚本能正常建目录；但 `--out` 相对路径按**当前工作目录**解析，从 `web/` 跑就会落到 `web/web/screenshots`，所以要么在仓库根跑，要么传绝对路径。

### 14.5 工具选择

| 任务 | 工具 | 不要用 |
|---|---|---|
| 搜代码/文件 | `rg`、`rg --files` | `findstr`、递归 `Get-ChildItem` 全盘 |
| 改文件 | `apply_patch`（精确 diff） | 整文件重写、正则批量替换未核对 |
| 前端构建 | `npm run build` | 只跑 `vite build` 跳过类型检查 |
| Rust 检查 | `cargo check` / `cargo test`（绝对路径） | `cargo test --workspace` 当门禁 |
| 视觉验证 | 真实浏览器 + `scripts/wall_*.mjs` | 只读代码推断像素结果 |
| 文档自检 | `& 'C:\Program Files\Git\bin\bash.exe' -lc 'cd /d/Code/Kanalib && bash scripts/check_backlog_docs.sh'`（Git Bash 已装，不需要 WSL） | 跳过一致性检查 |
| 远程执行 | `ssh` / box MCP | 在本地模拟服务器状态 |

## 15. 服务器访问与部署

### 15.1 拓扑（实测，2026-11-23）

```text
公网浏览器 ──443──> patchouli.us.ci (128.1.132.31)
                       └─ Nginx ──> kanalib-web 127.0.0.1:8787（只读 public）
                                        └─ /media/ 静态媒体
                       └─ /easytier/ ──> 127.0.0.1:11012（EasyTier WSS）

EasyTier 虚拟网 10.126.126.0/24
  ├─ 10.126.126.1  kanalib-pc（本机 Windows）
  ├─ 10.126.126.2  kanalib-web（2c2g 云，private 8788）
  └─ 10.126.126.3  kanalib-box（入库，ingest 8790 / admin 8792）
```

打标服务（原 `sidecar/tagger` + 云端 8793 代理）已于 2026-11-27 随 GPU 主机被服务商回收而彻底下线；`ingest.toml` 的 `[tagging].backend/endpoint` 已删除，box 不再发起任何打标请求。

| 节点 | 地址 | 运行内容 | 实测 |
|---|---|---|---|
| 云端 web | `10.126.126.2`，公网 `128.1.132.31` / `patchouli.us.ci` | Nginx 443、`kanalib-web` public `127.0.0.1:8787`、private `10.126.126.2:8788` | `https://patchouli.us.ci/api/health` → `{"ok":true,"db":true,"media_root":true}` |
| box | `10.126.126.3` | `kanalib-ingest`（`--config /etc/kanalib/ingest.toml`）、`kanalib-easytier`、`kanalib-etproxy` | 服务 active；`/admin/api/health` 免 token `200`，其余管理接口需 `x-admin-token`；`push_batches` 无 pending/failed |
| 本机 | `10.126.126.1` | `easytier-gui` Windows 服务 | `easytier-cli peer`：web `p2p ~370ms`、box `relay(2) ~680ms`（中继流量不计入该表 `rx/tx` 列，已实测可达） |

**无打标端时的行为**：规则判定到 `needs_tagging` 时，`pipeline::decide_item` 检测到没有 backend 就把该条目直接记 `review`（原因 `tag budget short; tagger unavailable`），不发起 HTTP、不占全局锁，同一轮里其它 `allow` 条目照常进入推送队列。2026-11-27 实测：`ingest.toml` 移除 backend 后，25 条 allow 批次 `allow-4d8a0eb8…` 全部 ack，t256/t512 各 50 个缩略图落到 web 媒体目录；服务每 60s 一轮稳定 `decided=0`，日志无打标相关错误。

本机与 box 之间的 EasyTier relay 现已可用：`ping 10.126.126.3` → ~680ms，`curl 10.126.126.3:8792/admin/api/health` → `200`。此前不通的根因是本机 `secure_mode.local_public_key` 与私钥不匹配——`secure_mode` 下中继数据要先做端到端 Noise 握手，公钥错误使握手必然失败、数据被静默缓冲丢弃；2026-11-27 已修正（原文件备份为 `config.d/kanalib.toml.bak-20261127-pubkey`）。`peer` 表里 box 仍显示 `relay(2)`/`rx,tx 0 B` 属正常，中继流量不记在直连列。

端口约定：

| 端口 | 位置 | 用途 | 暴露面 |
|---|---|---|---|
| 443 | web | Nginx TLS、静态站、只读 API | 公网 |
| 8787 | web | `kanalib-web` public listener | 仅回环，Nginx 反代 |
| 8788 | web | `kanalib-web` private listener（ingest / desktop / 原图） | 仅 EasyTier |
| 8790 | box | `kanalib-ingest` 媒体/公开基址 | 仅 EasyTier |
| 8792 | box | 管理页与指标 | 仅 EasyTier |
| 11012 | web | EasyTier WSS 入口（`/easytier/` 反代） | 公网（协议自身鉴权） |

`8793` 随打标服务于 2026-11-27 下线，云端代理与本机 sidecar 均不再监听。

### 15.2 访问方式

**公网（只读）**

```powershell
node -e "fetch('https://patchouli.us.ci/api/health').then(r=>r.text()).then(console.log)"
```

**云端 shell（SSH）**

```powershell
$deployHost = 'yorigami@128.1.132.31'
ssh -o BatchMode=yes -i $env:USERPROFILE\.ssh\kanalib_deploy $deployHost "hostname; systemctl is-active nginx kanalib-web"
```

- 用户名 `yorigami`，主机 `128.1.132.31`（与 EasyTier `10.126.126.2` 是同一台）。
- 私钥 `~/.ssh/kanalib_deploy`；当前沙箱内该文件 ACL 不可读（`Permission denied`），需在有权限的终端执行，或由 owner 授权后运行。
- 若私钥不可用，走 **box MCP 中转**：box 上 `10.126.126.2:8788`（private）可达，可在 box 执行 `curl` 做健康检查；但 box 没有 web 的 SSH 私钥，不能代替部署。
- 首次连接会写 `known_hosts`；只接受一次，不要 `StrictHostKeyChecking=no` 长期使用。
- 只做维护与部署，不在服务器上直接改代码。

**私网（EasyTier）**

```powershell
ipconfig | Select-String 10.126.126.1
& 'D:\APP\Easytier\easytier-cli.exe' -p 127.0.0.1:15999 peer
```

- **目标态**是业务配置只写 FQDN（`kanalib-web.et.net` / `kanalib-box.et.net`），换 IP 只改 EasyTier 配置。
- **现状是魔法 DNS 没生效**：实测本机（`accept_dns = false`）与 box（`nameserver 127.0.0.153`，无 `~et.net` 域路由）都把 `kanalib-web.et.net` / `kanalib-box.et.net` 解析到公网 IP（`76.223.54.146` / `13.248.169.48`），不是 `10.126.126.x`。所以现在**探针、`ingest.toml`、临时排障一律用 IP**；等 DNS 修好再切 FQDN，切换前先 `resolvectl query` / `Resolve-DnsName` 确认拿到的是私网地址。
- 修 DNS 的路径（Linux）：`sudo resolvectl domain et0 '~et.net'` + `sudo resolvectl dns et0 100.100.100.101`，然后 `resolvectl query kanalib-box.et.net` 应返回 `10.126.126.3`；Windows 侧把 `accept_dns` 改回 `true` 并重启 `easytier-gui` 服务（需管理员）。改完在两端各验一次再切配置。
- 本机 EasyTier 由 `easytier-gui` 服务承载；**重启需要管理员权限**，非管理员会 `OpenService FAILED 5`。
- 不要同时跑 GUI 与独立 `easytier-core`，会抢 `11010/11012`。
- `secure_mode` 是表不是布尔；`config-dir` 实例不继承命令行 `--secure-mode`；缺私钥会一直 `Connecting`。

**从 box 侧验证**

box 上执行（通过 box MCP `shell`）：

```bash
systemctl status kanalib-ingest --no-pager -n 15
curl -s --noproxy '*' http://10.126.126.3:8792/admin/api/health
curl -s --noproxy '*' http://10.126.126.2:8788/api/health
```

实测：`8788`、`8792` 正常；`10.126.126.2:8787` 从 box 不可达——public listener 只绑回环（正确）。box→本机 relay 的 ICMP 已通（~660ms）。

box 上还有两条容易踩的网络事实：

- 出网代理是 `127.0.0.1:3128`（`silkd`），拉 GitHub 用 `HTTPS_PROXY=http://127.0.0.1:3128`，同时把私网地址放进 `NO_PROXY=10.126.126.0/24,.et.net,127.0.0.1,localhost`，否则私网请求会被代理吞掉。
- `127.0.0.1:443` 是 `kanalib-etproxy` 的出网桥（→ `patchouli.us.ci:443`），不是本地站点。

### 15.3 部署流程

1. **本地构建**

   ```powershell
   Push-Location web; npm run build; Pop-Location
   ```

   **服务端二进制不能在 Windows 上编了再传**：本机只装了 `x86_64-pc-windows-msvc` target，`cargo build` 产出的是 PE，Linux 上直接 `Exec format error`。正确做法是二选一：

   - 在 box 上编 ingest（既有流程，见 16.3 第 1 条）：`cargo build --release -p kanalib-ingest` → `install` 到 `/usr/local/bin`。
   - 在 web 主机上编 `kanalib-web`（或加 `rustup target add x86_64-unknown-linux-gnu` + 交叉 linker 后在本机交叉编译）。

   另外 box 的 cargo 是 1.100.0、本机 1.98.1，release 产物与 `target/` 都不通用；两边各自编。

2. **发布静态数据**（发布器只读导出，不写源库）

   ```powershell
   $cargo = "$env:USERPROFILE\.cargo\bin\cargo.exe"
   & $cargo run --release -p kanalib-publish -- --db <库文件> --root <图库根> --out web/public/data
   # 只重建搜索索引（不读库、不重编图片）：
   & $cargo run --release -p kanalib-publish -- --out web/public/data --index-only
   python scripts\wall_manifest_check.py web/public/data
   ```

   `--dry-run` 只打印统计；`LEDGER_VERSION` 升级会触发全量重编（产物约 400 MB 量级），需 owner 确认。

3. **上传**（Windows 无 `rsync`，用 `tar` + `scp` 或 box 中转）

   ```powershell
   tar -czf $env:TEMP\kanalib-dist.tgz -C web dist
   $deployHost = '128.1.132.31'
   scp -i $env:USERPROFILE\.ssh\kanalib_deploy $env:TEMP\kanalib-dist.tgz "${deployHost}:/tmp/"
   ssh -i $env:USERPROFILE\.ssh\kanalib_deploy $deployHost `
     "sudo tar -xzf /tmp/kanalib-dist.tgz -C /var/lib/kanalib-web/dist --strip-components=1"
   ```

   静态目录是 `/var/lib/kanalib-web/dist`（`kanalib-web.service` 的 `--web` 与 Nginx `root` 都指这里，解包后属主需回到 `kanalib:kanalib`）。`/var/www/kanalib` 是早期草稿里的路径，线上不存在。数据目录 `web/dist/data` 约 400 MB，整包上传慢，日常只传 `dist/assets` + `index.html` 更快；只在发布器重导后才需要搬 `data/`。

4. **重启与健康检查**

   ```bash
   sudo systemctl restart kanalib-web
   systemctl is-active nginx kanalib-web
   curl -s --noproxy '*' http://127.0.0.1:8787/api/health
   ```

5. **回滚**：保留上一版 `dist` 与二进制；schema 升级后旧二进制会因 `user_version` 更高拒绝启动，回滚前先确认数据库版本。

部署纪律：

- **只有 owner 明确要求才部署**；默认交付到本地预览。
- 部署前必须 `npm run build` + 对应探针通过；不在服务器上改代码、不手改 `dist`。
- `web/public/data` 含真实库内容，上传走加密通道，不放进仓库、不贴到聊天。
- Nginx 必须显式 404 `/internal/`、`/api/desktop/`、`/api/items/<id>/original`、`/original/`（模板已写）。

### 15.4 密钥与配置位置

| 文件 | 位置 | 权限 | 内容 |
|---|---|---|---|
| `web.env` | web `/etc/kanalib/` | `0600` root/kanalib | `KANALIB_INGEST_SECRET`、`KANALIB_MEDIA_SIGNING_KEY` |
| `ingest.env` | box `/etc/kanalib/` | `0600` | 推送密钥等 |
| `ingest.toml` | box `/etc/kanalib/` | `0640` root:kanalib | 来源、打标、推送、管理口 |
| `rules.toml` | box `/etc/kanalib/` | `0664` root:kanalib（实测） | 规则；支持 `SIGHUP` 热重载；改前 `cp` 成 `rules.toml.bak-<日期>` |
| `ingest-hmac` / `media-signing-key` | box `/etc/kanalib/` | `0600` kanalib | HMAC 与原图签名 |
| `admin-token` | box `/etc/kanalib/` | `0600` root | `kanalib-ingest` 管理面 `x-admin-token`（`/admin/api/health` 免鉴权） |
| `easytier.env` | 各节点 `/etc/kanalib/` | `0600` root | `ET_NETWORK_SECRET` |
| `backup.env` / `health.env` / `keepalive.env` | box `/etc/kanalib/` | `0640` root:kanalib | 备份保留、健康阈值与告警 webhook、保活目标 |

规则：

- 密钥只由进程环境读取，**不写进 unit 文件、不写进仓库、不进日志/报告**。
- 轮换顺序：`new,old` → 重启 web → 切 box 到 `new` → 去掉 `old` → 再重启 web。

### 15.5 线上基线（2026-11-27）

| 服务 | 版本/哈希 | 备份与回滚 |
|---|---|---|
| `kanalib-web`（web） | `aa2b850`，sha256 `4a6633298006d876d6b74dcb0ae43c108ca899f04e7866719392fc702e8cd2b9` | 二进制 `/usr/local/bin/kanalib-web.bak-20261127-b`（上一版 `…bak-20261127`）；DB `/var/lib/kanalib-web/kanalib-web.db.bak-20261127-local`。schema v2 只新增 `items.origin`，旧库自动 ALTER |
| `kanalib-ingest`（box） | `37314d9`，sha256 `4d5c43e8a9807888fce2a90ab7b16971fab839a17f9e53c72b3ef39c69ced93f` | 二进制 `/usr/local/bin/kanalib-ingest.bak-20261127`；配置 `/etc/kanalib/ingest.toml.bak-20261127-tagging` |

- 两台二进制都必须 ≥ `0812627`（X-02：缩略图签名纳入批次 id）。只升一边会在缩略图上传处得到 `401/403`。
- 重放方式：停 `kanalib-ingest` → `kanalib-ingest --config /etc/kanalib/ingest.toml --once --replay-push <batch-id>` → 起服务。手工重放会继承调用者的 `http_proxy`，私网地址不在 `NO_PROXY` 时会被 `3128` 代理 403 拦截；service 自身的 `ingest.env` 已含 `NO_PROXY=10.126.126.0/24,.et.net`，正常轮询不受影响。

### 15.6 本地图片发布

本机目录 → web 的两阶段发布，不经过 box；条目 `origin='local'`，缩略图生成成功后才从 `review` 翻成 `allow`。

1. 生成设备令牌（一次性显示，DB 只存 SHA-256）：

   ```bash
   sudo -u kanalib kanalib-web --db /var/lib/kanalib-web/kanalib-web.db \
     --media /var/lib/kanalib-web/media --issue-device kanalib-pc
   ```

   当前线上已为 PC 生成 `kanalib-pc`，token 存在本机 `D:\Code\Kanalib\.kanalib\sync-token`（已被 `.gitignore` 忽略）。

2. PC 侧发布：

   ```powershell
   & 'D:\Code\Kanalib\target\release\kanalib-sync.exe' `
     --web http://10.126.126.2:8788 --token-file D:\Code\Kanalib\.kanalib\sync-token `
     --dir D:\Pictures\相册 --recursive [--dry-run] [--limit 20]
   ```

   `kanalib-sync` 扫描 jpg/jpeg/png/webp/gif，按原图 sha1 生成 id；重复运行是幂等的（`updated` 后重新上传原图）。

3. 服务端行为：`POST /api/desktop/items` 写元数据；`POST /api/desktop/upload` 把原图写到 `media/originals/{id}.{ext}`，再用 `image` 解码生成 t256/t512（长边 ≥1024/2048 时加 t1024/t2048）JPEG q90，刷新 FTS 与作品封面。`/original/{id}` 对本地条目直接读盘，仍要求 HMAC 签名与有效期。

4. 网络注意：本机当前访问 `10.126.126.0/24` 的 **TCP** 会返回 Windows `WSAEACCES 10013`（ICMP 正常，box 与 web 之间的 8788 正常）。这是本机过滤层（安全软件 / EasyTier 接口 profile），不是服务端问题。绕行方式：

   ```powershell
   ssh -N -L 18788:10.126.126.2:8788 yorigami@128.1.132.31
   kanalib-sync --web http://127.0.0.1:18788 --token-file .kanalib\sync-token --dir <目录>
   ```

   2026-11-27 已用该隧道在 Windows 客户端实测 2 张图发布成功（含 PNG 1200×800、JPEG 900×1400），公网缩略图 t256/t512/t1024 与私网原图均 200，重复运行幂等；测试条目已清理。
- 读配置做排障时先脱敏（`sed -E 's/(secret|token|key|password)\s*=\s*".*"/\1 = "<redacted>"/I'`）。
- 诊断快照只读，脱敏 URL 凭据与 token 类键值。

### 15.5 备份、健康与常见故障

```bash
# web 上
/usr/local/sbin/kanalib-web-backup.sh
KANALIB_PUBLIC_HEALTH=http://127.0.0.1:8787/api/health \
KANALIB_PRIVATE_HEALTH=http://10.126.126.2:8788/api/health \
/usr/local/sbin/kanalib-health.sh

# box 上（只有 ingest 与 health/keepalive；web 的备份脚本不在这里）
/usr/local/sbin/kanalib-ingest-backup.sh
KANALIB_INGEST_HEALTH=http://10.126.126.3:8792/admin/api/health \
KANALIB_INGEST_METRICS=http://10.126.126.3:8792/admin/api/metrics \
/usr/local/sbin/kanalib-health.sh
```

实测：box 的 `/usr/local/sbin` 只有 `kanalib-health.sh`、`kanalib-ingest-backup.sh`、`kanalib-keepalive.sh`；把三个健康地址塞进一条命令在 box 上跑会因为缺 `kanalib-web` 而必然失败。

定时器实测：`kanalib-health.timer` 每 5 分钟、`kanalib-keepalive.timer` 每 1 分钟（ping 云端私网地址，连续 3 次失败重启链路）、`kanalib-ingest-backup.timer` 每天 04:02 UTC。备份默认保留 7 天，落在 `/var/backups/kanalib`；`kanalib-ingest-backup.sh` 会一并快照 `rules.toml` 与媒体硬链接。

| 现象 | 先查 | 处理 |
|---|---|---|
| 公网 502/超时 | `systemctl is-active nginx kanalib-web`、`curl 127.0.0.1:8787/api/health` | 重启 `kanalib-web`；看 `journalctl -u kanalib-web` |
| 私网不可达 | `easytier-cli peer`、`ip -4 addr show et0` | 重启 EasyTier（需管理员）；确认 `secure_mode` 私钥存在。本机 `local_public_key` 必须由同一私钥推导（不匹配时 relay 握手静默失败，表现为 `relay(2)`/`rx,tx 0 B`） |
| box 入库停滞 | `journalctl -u kanalib-ingest -n 50`、`/admin/api/health` | 看 `model_error_streak`、规则版本、队列 |
| 打标失败 | 本机 `curl 10.126.126.1:8793/health`；box `curl 10.126.126.2:8793/health` | 本机打标服务与云端代理是否都在线；离线时条目应落 `review` |
| 推送失败 | box `[push]` 配置、HMAC 文件权限 | 用 `--replay-push <BATCH_ID>` 重放，不改内容 |
| 磁盘不足 | `df -h /var/lib` | 清理旧备份/parts，先确认保留策略 |
| 告警没收到 | `systemctl status kanalib-alert-sink`、`/etc/kanalib/health.env` 的 `KANALIB_ALERT_WEBHOOK` | 收件箱：公网 `https://patchouli.us.ci/kanalib-alert/<token>`、box 私网 `http://10.126.126.2:8795/kanalib-alert`；webhook 地址只放 `health.env`（0600） |

实测注意：box 上 `kanalib-etproxy`（`/usr/local/bin/kanalib-etproxy.py`）监听 `127.0.0.1:443`，把流量桥到 `patchouli.us.ci:443`，这是**出网代理**，不是本地 Web 服务；在 box 上 `curl https://127.0.0.1/...` 会打到公网站点。box 拉 GitHub 走 `127.0.0.1:3128`（`silkd`），不是 443。

## 16. box MCP 协作

### 16.1 可用工具

| 工具 | 用途 | 注意 |
|---|---|---|
| `box_status` | 查状态、镜像、`view_url` | 先查再操作；休眠时先 `box_ensure` |
| `box_ensure` / `box_wake` | 启动/唤醒 | 唤醒后先复查 `kanalib-ingest` 与 EasyTier 是否随机器恢复 |
| `box_hibernate` | 休眠 | 运行中的进程与内存保留，下次唤醒继续；**不是重启**，但长期休眠会拖慢入库 |
| `shell` | 在 box 上跑 bash（默认 `/workspace`，用户 `user`，持久会话） | 主入口；`sudo -n` 可用（实测免密）；输出超 ~30 KB 只留首尾；`timeout_s` 默认 120，上限 600 |
| `write_file` | 写文件（自动建父目录） | 只写 box，不写本地仓库 |
| `box_snapshots` | 列出快照（时间倒序） | 误删后配合 `box_restore_paths` |
| `script_run` | 跑桌面自动化脚本（`/opt/desk`） | 入库/部署不需要，别滥用 |
| `box_restore_paths` | 从快照恢复文件到 `~/Restored/<时间>` | 误删后的恢复路径 |
| `box_create` | 新建 box | 只有需要独立环境时才建 |
| `box_upgrade` | 升级镜像 | 保留 `/home` 与 `/workspace`，**丢弃 `/opt`、`/usr/local` 里非 apt 安装的软件**；box 上有 `/usr/local/bin/kanalib-ingest`，升级后需重装 |

### 16.2 当前 box 事实（实测）

| 项 | 值 |
|---|---|
| 名称 / ID | `main` / `cb-f83ed3cb3f08` |
| 系统 | Ubuntu 24.04.5 LTS，kernel 6.8，2 vCPU / 3.8 GB / 20 GB（根分区已用 10%） |
| 镜像 | `sbxdesk:v27`（最新已到 v30，**暂不升级**，见 16.3 第 8 条） |
| 用户 | `user`，`/workspace` 可写；`sudo -n` 免密 |
| 网络 | `10.126.126.3/24`（`et0`），`kanalib-easytier` 常驻 |
| 服务 | `kanalib-ingest`、`kanalib-easytier`、`kanalib-etproxy`；定时器 `kanalib-health`（5 分钟）、`kanalib-keepalive`（1 分钟）、`kanalib-ingest-backup`（每日 04:02 UTC） |
| 数据 | `/var/lib/kanalib-ingest/ingest.db`、`media/{originals,parts,thumbs}` |
| 配置 | `/etc/kanalib/`（`ingest.toml`、`rules.toml`、密钥，权限见 15.4）；`[tagging].endpoint` 指向云端代理 `10.126.126.2:8793` |

### 16.3 使用规范

1. **box 是运行环境，不是主要开发仓库**：改代码在 Windows 本地，构建产物再上传。box 上 `/home/user/Kanalib` 是 `origin/main` 的检出（cargo 1.100.0，`target/` 约 492 MB），只用于"拉取 → 编 `kanalib-ingest` → 装到 `/usr/local/bin`"这条既有流程；不要在那儿开分支、改代码或提交。拉取前先 `git status` 确认没有本地改动——`git reset --hard origin/main` 会直接丢掉它们。
2. **先读后写**：改 `ingest.toml` / `rules.toml` 前先 `cp` 备份（现有 `*.bak-2026xxxx` 命名），改完 `systemctl restart kanalib-ingest` 并看日志。
3. **不碰媒体与数据库**：`/var/lib/kanalib-ingest` 只读排查；删除、清空、`rm -rf` 一律不做，需要时先备份并请示。
4. **命令要可复制**：一条命令只做一件事；长脚本先 `write_file` 再执行，避免转义问题。
5. **脱敏输出**：`env`、`cat /etc/kanalib/*`、日志里的 token/HMAC 不外传；报告只写键名。
6. **时区**：box 是 UTC，日志时间与本地（Asia/Shanghai）差 8 小时；对时间线时换算。
7. **box 离线/鉴权过期**：MCP 返回 `HTTP 502/503`（实测过）或 `magpie's sign-in ... has run out` 时，先 `box_status`；单次 502 往往几秒后自愈，最多重试一次；仍失败就报告 owner，不要反复重试或改用 SSH 绕过。
8. **升级前确认**：`box_upgrade` 会保留 `/home`、`/workspace`，但丢弃 `/usr/local` 里的非 apt 安装；`/usr/local/bin/kanalib-ingest`、`kanalib-easytier`、`kanalib-etproxy.py` 需重装，`/etc/kanalib`（含 `ingest.toml`、`rules.toml`、密钥）也不在保留范围，必须先备份；非必要不升级。

### 16.4 常用操作

```bash
# 服务与日志
systemctl status kanalib-ingest --no-pager -n 20
sudo journalctl -u kanalib-ingest -n 50 --no-pager

# 健康与规则版本
curl -s --noproxy '*' http://10.126.126.3:8792/admin/api/health
sudo grep -E '^version' /etc/kanalib/rules.toml

# 改配置（先备份）
sudo cp /etc/kanalib/ingest.toml /etc/kanalib/ingest.toml.bak-$(date +%Y%m%d-%H%M)
sudo systemctl restart kanalib-ingest

# 网络
ip -4 addr show et0
curl -s --noproxy '*' http://10.126.126.2:8788/api/health
```

## 17. 反模式清单（来自真实返工）

| 反模式 | 后果 | 正确做法 |
|---|---|---|
| 把"侧边栏不能被点击穿透"理解成"侧边栏不可见" | 改错层，破坏玻璃效果，被用户点名返工 | 复现 + 追问，确认是命中区还是视觉 |
| 让画布从缝右缘开始以"避开"侧栏 | 侧栏下没有图，玻璃失效 | 画布全幅 + 指针层在缝区拦截 |
| 越域加"重试"按钮/改错误条 | 与他人的批次冲突 | 停下报告，等授权 |
| 只改 `replaceState` 不通知订阅者 | URL 变了、画面不变 | 统一路由事件 + 探针固化"500ms 内画面变化" |
| 重试复用失败 URL | Chrome 直接复用失败资源 | 加 `?r=<nonce>` |
| 用 `Network.setBlockedURLs` 测降级 | 实际没拦住，结论无效 | 用 `Fetch.failRequest` |
| 同一探针跑出不同结论仍当结论 | 假缺陷/假通过 | 先判定 flake，再复跑并标注 |
| 探针连不上 CDP、以 0 退出就报"通过" | 没截图也没报告，实际什么都没验 | 先确认 `report.json` 是新生成的，再看 `failures` |
| 只跑子集却写"全部通过" | 交付失真 | 写"定向通过"并列出未跑项 |
| 把截图/报告提交进仓库 | 泄露库内容、污染历史 | 放 gitignore 目录，只报路径 |
| 用合成输入证明 GUI 可点 | 结论不成立 | 真实鼠标/键盘，或标注"待人工确认" |

## 18. 检查清单

派发前：

- [ ] 目标、复现路径、期望行为写清
- [ ] 写域与禁区精确到文件
- [ ] 验收命令与数字门槛可判定
- [ ] 说明是否提交、证据放哪、向谁报告

提交前：

- [ ] `npm run build` 通过（前端）或 `cargo check` / `cargo test` 通过（Rust）
- [ ] 探针确实产出了新的 `report.json`（不是连不上 CDP 的空跑），再按各自口径判：`wall_shots` 看 `failures` / `consoleErrors`，其余看 `passed`
- [ ] 改前/改后证据成对，数字带口径
- [ ] 残余风险已写，未覆盖项标注"未跑/待实机"
- [ ] 无凭据、无真实库内容、无截图产物
- [ ] 只暂存本批写域内的文件

收口时：

- [ ] 批次已冻结，后续改动另开批次
- [ ] 文档与登记同步（`registry.md` / `implemented.md` / `photo-wall-status.md`）
- [ ] 归档链接已修正，`check_backlog_docs.sh` 无矛盾
- [ ] 向 owner 交付：改了什么、怎么验的、还剩什么
