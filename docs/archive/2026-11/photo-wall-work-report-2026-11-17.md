# 图片墙工作汇报（2026-11-17）

## 本次目标

根据 [photo-wall-status.md](../../backlog/photo-wall-status.md) 的已知问题与待决策项，落实四项产品决策：

1. 内容分级闸门：不做。
2. 标签页默认形态：维持全量平铺。
3. 移动端导航形态：底部 tab。
4. 桌面单击聚焦：保留名称。

## 已完成

### 移动端底部导航

- `≤760px` 时侧缝改为底部 tab，高度固定 `56px`（`--seam-h`）。
- `≤560px` 时导航收为纯图标，避免七个带字 tab 撑爆一行；底栏支持横向滚动兜底。
- 画布手势按 `--seam-w` / `--seam-h` 双向拦截，缝区内的按下与滚轮不透给画布。

### 聚焦卡信息

- 恢复聚焦卡上的名称与主标签。
- 右下角新增「打开」按钮，点击进详情，等同双击行为。

### 探针与文档

- 更新 `wall_probe_tag360` 移动端断言：由 `64px` 侧缝改为 `56px` 底栏高度。
- 让「画布覆盖缝区」判定支持底部导航（上下边界也纳入覆盖检查）。
- 删除过时口径：`search-index.json` 发布器侧已交付，状态文档与决策表同步更新。
- 更新 `web/README.md` 的移动端导航与聚焦卡说明。

## 验证

| 项 | 结果 |
|---|---|
| `npm run typecheck` | 通过 |
| `npm run build` | 通过 |
| `wall_probe_tag360` | 未实跑（环境问题，见下） |

## 未完成

### 探针未闭环

本机 Chrome headless GPU 进程崩溃（`GPU process isn't usable`），探针无法实跑。
已给 `wall_probe_tag360` 的启动参数加 `--in-process-gpu` 规避，但尚未重跑确认。

### 已知问题仍开放

| # | 问题 | 状态 |
|---|---|---|
| 2 | 首页串行拉全量分片 | 未修（P0） |
| 6 | 深链补拉分片上限（约 4 万条） | 未修（P1） |
| 10 | 发布器非原子 | 未修（P2） |
| 11 | 语义搜索缺失 | 暂不做 |

### 待真实验证

- 移动端真实设备 / 触屏手感（底部 tab 遮挡、触控滚动、高 DPR）。
- 底部 tab 在 `≤560px` 收成图标后的可读性与误触率。

## 改动清单

| 文件 | 内容 |
|---|---|
| `web/src/styles.css` | 新增 `--seam-h`、底部 tab 布局、聚焦卡「打开」按钮样式 |
| `web/src/components/Wall.tsx` | 缝区拦截支持宽度 / 高度双向 |
| `web/src/components/WallCard.tsx` | 聚焦卡保留信息、新增「打开」按钮 |
| `scripts/wall_probe_tag360.mjs` | 移动端断言改底栏、启动参数加 `--in-process-gpu` |
| `docs/backlog/photo-wall-status.md` | 决策落地、过时记录删除 |
| `web/README.md` | 交互说明同步 |

## 下一步建议

1. 用 `--in-process-gpu` 参数重跑 `wall_probe_tag360`，确认移动端底栏断言通过。
2. 若实机手感仍偏挤，再评估检查器可折叠把手（已知问题 #9）。
3. 处理工作区中与本次无关的新增文件（`.kanalib-hook-log-check.mjs`、`docs/design/`、根目录 `package.json` / `package-lock.json`）。
