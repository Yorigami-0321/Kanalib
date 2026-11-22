# 图片墙工作汇报（2026-11-21）

## 本次目标

按 [photo-wall-status.md](../../backlog/photo-wall-status.md) §4/§5 把网页侧剩余待办收口：

1. P0 首屏渐进加载：无限墙不再等全库分片。
2. P1 深链补拉去掉 8 分片上限。
3. P2 移动端检查器可折叠。
4. P2 发布器原子输出。

## 已完成

### 首屏渐进加载（#2）

- 无限墙（首页随机墙、随机排序的大标签墙）首片到达就铺满；剩余分片用 `requestIdleCallback`
  （无此 API 时 180ms 延时）在空闲期补拉，`WallView ready` 不再卡全量。
- 标签总览在 `index.json` 到达后立即渲染目录与张数（来自 `index.facets`），封面随分片补齐；
  收藏/分级/分组的计数只能从条目算，仍等全量到位再报数（卡片本身照常渐进渲染）。
- 搜索与有限墙（显式排序、收藏/分级/分组墙）仍按序立刻补全，保证结果与计数不在半库上算。

### 深链全库探测（#6）

- `useWall.findItem` 去掉 `MAX_PROBE_SHARDS = 8`，改为读到命中或 `hasMore()` 为 false。
- `StaticSource` 只在请求成功后才推进游标，失败片不会被反复重试；无效深链仍会终止并落到缺失页。

### 移动端检查器折叠（#9）

- 把手从装饰性 `<span>` 改成真实 `<button>`，带 `aria-expanded` / `aria-controls` / 标题。
- ≤900px：展开 55vh；收起只留把手 + 标题行（实测 88px），操作区隐藏；再点恢复。
- 桌面端把手 `display:none`，仍是 300px 左侧栏；`I` / 关闭仍完全隐藏检查器。

### 发布器原子输出（#10）

- 正式发布先写同级隐藏暂存目录 `.<out>.staging-<pid>-<nonce>`，成功后整目录 rename 替换；
  旧目录先移到备份，替换失败时还原，成功后删除备份。
- 旧产物用硬链接预置到暂存目录（跨盘/不支持时回退复制），账本增量跳过语义不变。
- 关键坑：`fs::copy` 会原地截断已存在目标，硬链接下会把正式目录写坏；`sync_file` 复制前先断开链接。
- `--dry-run` 仍不创建任何目录；`--index-only` 行为不变。

### 顺带修复

- 工作区里 `--seam-h` 只声明了 `0px`、移动端从未赋值，底部 tab 实际只有 17px 高；
  `wall_probe_tag360` 因此失败。已在 ≤760px 媒体块设为 `56px`，探针恢复通过。
- `wall_probe_state.mjs` 支持 `--out <file>` 落盘报告（本机 `process.exit` 会丢 stdout）。

## 验证

| 项 | 结果 |
|---|---|
| `tsc --noEmit` | 通过 |
| `vite build` | 通过（JS 324.22 kB / gzip 105.19 kB） |
| `cargo test -p kanalib-publish` | 25 单元 + 24 集成，全部通过 |
| `wall_probe_state` | passed（无效深链 8s 内缺失页、console 0） |
| `wall_probe_detail_nav` | passed（深链刷新、翻页不拉分片） |
| `wall_probe_edge` | passed（缺失页/空态动作/历史） |
| `wall_probe_cold_search` | passed（142 张、索引 404/损坏降级） |
| `wall_probe_tag_threshold` | passed（1/7 有限，8/9/大标签无界） |
| `wall_probe_tag360` | passed（桌面/移动端、缝区拦截；移动底栏 56px） |
| `wall_probe_inspector_sheet` | passed（464px → 88px → 464px；桌面 300px、把手隐藏） |

## 未完成 / 边界

- 发布器线程池（大库首编耗时）未做，属独立性能项。
- 语义/CLIP 搜索按决策「先不做」。
- 标签总览全量平铺按决策维持。
- 真实触屏/触控板手感仍需实机复验；headless 只证明轨迹与几何。
- 新增探针与报告文件未提交（工作区仍保留 2026-11-17 的未提交改动）。

## 改动清单

| 文件 | 内容 |
|---|---|
| `web/src/state/useWall.ts` | 去掉深链 8 分片上限 |
| `web/src/App.tsx` | 无限墙空闲补拉、总览按 index 先出、`loadingTags` |
| `web/src/components/Inspector.tsx` | 可折叠把手按钮与状态 |
| `web/src/components/inspector.css` | 收起态样式、过渡、焦点环 |
| `web/src/styles.css` | 移动端 `--seam-h: 56px` |
| `crates/kanalib-publish/src/publish.rs` | `Staging` 暂存/替换、硬链接预置、复制前断链 |
| `crates/kanalib-publish/src/fsutil.rs` | `unique_suffix` |
| `crates/kanalib-publish/tests/publish_contract.rs` | 原子替换与失败保留测试 |
| `scripts/wall_probe_inspector_sheet.mjs` | 新增移动端检查器探针 |
| `scripts/wall_probe_state.mjs` | `--out` 报告落盘 |
| `docs/backlog/photo-wall-status.md`、`photo-wall-contract.md`、`web/README.md` | 状态与契约同步 |
