# FFmpeg 许可与进程模型（F-040）

> 2026-10-27 决策：Kanalib 默认只使用 **LGPL** 构建；GPL full build 只允许本机开发使用，打包脚本默认拒绝。缩略图采用 **多输入批处理 CLI 进程池**，不引入 libav 绑定。

## 许可

| 构建 | 用途 | 随包分发 |
|---|---|---|
| LGPL（无 `--enable-gpl`；当前构建带 `--enable-version3`，即 LGPLv3） | 默认运行时 | 允许；保留 `LICENSE.txt`、构建记录与源码获取方式 |
| GPL（含 `--enable-gpl`） | 本地开发/个人使用 | **默认禁止**；需 `-AllowGplFfmpeg` 且完成 GPL 合规审查 |
| 未安装 | 回退 `image` crate | 允许；只影响缩略图速度与格式覆盖 |

- 项目自身是 MIT；ffmpeg 是独立可执行进程，不作为库链接进 Kanalib。
- 运行时探测 `ffmpeg -version` 的 `configuration:`；含 `--enable-gpl` 即判为 GPL，并在日志中警告。
- 当前开发机使用 BtbN 的 LGPL shared 构建：`n8.1.3-24-g92611900bb-20261026`，安装位置 `runtime/ffmpeg`（被 `.gitignore` 忽略）。FFmpeg 源码获取入口：<https://ffmpeg.org/download.html>；BtbN 构建脚本与归档见 <https://github.com/BtbN/FFmpeg-Builds>。
- 获取/更新：仓库内运行 `powershell -ExecutionPolicy Bypass -File scripts/fetch_ffmpeg_lgpl.ps1 -Force`；便携包根目录也会附带同内容的 `fetch-ffmpeg-lgpl.ps1`，按提示可重新下载到 `runtime/ffmpeg`。脚本固定 autobuild tag、ZIP SHA256 与下载地址，并写入 `FFMPEG-SOURCE.txt`。当前 shared 安装约 160MB（含 DLL）；打包只带 `bin` 中实际文件，不含 `ffplay`/doc。
- 运行时优先使用 `PATH` 中的 ffmpeg；程序目录的 `runtime/ffmpeg` 是离线备选。若用户希望覆盖二者，设置 `KANALIB_FFMPEG` 即可。
- 打包：`scripts/build_portable.ps1` 默认只接受 LGPL；拒绝 GPL 并提示改用 LGPL 脚本。包内写入 `runtime/ffmpeg/FFMPEG-BUILD.txt`（版本、GPL 标志、构建参数、每个二进制的 SHA256）。

## 二进制发现顺序

1. `KANALIB_FFMPEG`：显式覆盖路径；设为 `off` 强制回退 `image` crate。
2. `PATH` 中的 `ffmpeg`（实际使用优先）。
3. `<root>/runtime/ffmpeg/bin/ffmpeg[.exe]`（程序目录备选；便携包随附 LGPL）。
4. 可执行文件同级的 `runtime/ffmpeg/bin`。
5. 仅 debug 构建继续探测 `<root>/tools/ffmpeg/bin`（工作区开发便利，GPL，不进入打包）。

## 进程模型

- 一次进程处理一批输入，避免每张图启动一次 ffmpeg：
  - 每批最多 32 个输入（`KANALIB_FFMPEG_BATCH`，4–48）。
  - 每个输入一个 `-map i:v` 输出，`scale=512:512:force_original_aspect_ratio=decrease`、`-frames:v 1`；不透明格式直接写 JPEG，PNG/WebP/GIF 先写临时 PNG，再由 Rust 白底合成并保存 JPEG。
  - 固定参数：`-nostdin -v error -y -threads 1 -max_pixels 268435456`、`-protocol_whitelist file`、`-map_metadata -1`；Windows 使用 `CREATE_NO_WINDOW`。
  - 并发 worker 默认 `min(4, CPU/2)`（`KANALIB_FFMPEG_WORKERS`，1–16），每个 worker 独立批次。
  - 单批超时默认 120s（`KANALIB_FFMPEG_TIMEOUT_SECS`，10–600）；超时/启动失败不重试，直接回退。
- 批内某个损坏文件可能让 ffmpeg 提前结束并牵连其他输出。失败文件会按二分方式单独重试（最多 6 层），合法文件仍能生成；最终失败的文件由 `ensure_thumb` 回退 `image` crate。
- 输出经过尺寸校验；部分写入/损坏的 JPEG 会被删除后回退。
- 缓存键带 `v3` 版本；旧缓存会在下一次扫描按当前编码器**一次性重建**（这是本次升级的有意失效）。扫描前批量预生成缺失的 512 缩略图，主循环只做缓存读取；t256 仍从 512 派生。
- EXIF 方向继续由 `kamadak-exif` 读取；ffmpeg 默认 autorotate，输出方向与现有 `apply_orientation` 一致（orientation=6 实测 175×280 源 → 320×512 缩略图）。
- 透明图白底规则与现有实现一致：RGBA 与白色按 alpha 合成后写 JPEG。

## 回退与可观测性

- ffmpeg 缺失、非 LGPL/GPL 构建探测失败、批次失败、超时：记录日志并回退 `image` crate，不阻塞扫描。
- 扫描指标新增：`thumb_prefetch_jobs/generated/failed/secs`；每个批次失败会记录数量与 stderr 摘要。
- `KANALIB_FFMPEG=off` 可用于对比基线与排障。

## 10k 冷导入基准（2026-10-27，本机 debug 构建，16 逻辑核）

| 路径 | 缩略图耗时 | 总耗时（metrics） | 备注 |
|---|---:|---:|---|
| ffmpeg 批处理（10k） | 52.2s（10,000 任务，9,990 成功，10 损坏回退） | 152.3s | `KANALIB_FFMPEG=runtime/ffmpeg/bin/ffmpeg.exe` |
| `image` crate 回退（10k） | 124.3s | 230.9s | `KANALIB_FFMPEG=off` |
| ffmpeg 批处理（100k） | 738.7s（100,000 任务，99,926 成功，74 失败；3159 个批次含隔离重试） | 2537.4s（42.3min） | 同一 debug 构建；t256 回填另计 552.4s，已包含在总耗时 |

10k 缩略图阶段约 **2.4×**，整次冷导入约 **1.5×**；100k 缩略图阶段 738.7s，低于历史基线 1130s，总导入 42.3min 也低于历史 49.3min。均满足“不得高于现有基线 1.2 倍”的门禁，暂不转向 libav 绑定。热重扫 10k：10 个损坏文件重试、9,990 个直接跳过，metrics 5.6s / wall 12.8s。

> 100k 的 t256 回填仍是主要剩余成本（552s）；后续若继续优化，应优先把 t256 也纳入 ffmpeg 批处理或从 512 批量派生，而不是先换 libav。

## 范围与后续

- 阶段 1 只迁移 **512 缩略图**；SHA1/BLAKE3 仍按字节流计算，EXIF 仍用 `kamadak-exif`。
- pHash 仍由 `image` crate 解码；损坏修复仍走 Python `repair.py`。这两项属于后续阶段。
- HEIC/AVIF/RAW 取决于 ffmpeg 构建；未在 UI 中承诺格式支持。
- GPL 随包分发必须由产品/法务明确确认；默认路径是 LGPL 或外部用户自备。
