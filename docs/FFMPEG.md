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
- 获取/更新：仓库内运行 `pwsh -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/fetch_ffmpeg_lgpl.ps1 -Force`（本机优先 PowerShell 7；只有没有 `pwsh` 时才回退 `powershell.exe`）。便携包根目录也会附带同内容的 `fetch-ffmpeg-lgpl.ps1`，按提示可重新下载到 `runtime/ffmpeg`。脚本固定 autobuild tag、ZIP SHA256 与下载地址，并写入 `FFMPEG-SOURCE.txt`。当前 shared 安装约 160MB（含 DLL）；打包只带 `bin` 中实际文件，不含 `ffplay`/doc。
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
  - 每个输入一个 `-map i:v` 输出，`scale=512:512:force_original_aspect_ratio=decrease`、`-frames:v 1`；不透明格式直接写 JPEG，PNG/WebP/GIF/PSD 先写临时 PNG，再由 Rust 白底合成并保存 JPEG。
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

## 阶段 2 结论（任务 C，2026-10-27）

### 格式矩阵

| 扩展名 | 扫描/哈希 | pHash | ffmpeg 缩略图 | `image` 回退 | 方向/EXIF | 备注 |
|---|---|---|---|---|---|---|
| tif/tiff | 是 | 是 | 是 | 是（`tiff`） | `kamadak-exif` 读 TIFF | 含 alpha 时走临时 PNG 白底 |
| ico | 是 | 是 | 是 | 是（`ico`） | 无 | 含 alpha 时白底 |
| qoi | 是 | 是 | 是 | 是（`qoi`） | 无 | 含 alpha 时白底 |
| exr | 是 | 是 | 是 | 是（`exr`） | 无 | HDR 浮点在 512 缩放后转 8 位白底 |
| avif | 是 | 否 | 是（LGPL 构建含 libdav1d） | **否** | 无 | `image` 0.25 只有编码器；无 ffmpeg 时标为“需要 FFmpeg 解码 AVIF” |
| heic/heif、RAW、PSD | 否 | 否 | 未承诺 | 否 | 不适用 | 本轮只评估，见下 |

> 第四轮更新（2026-10-29）：PSD 已接入并走 ffmpeg-only 路径，HEIC/HEIF/RAW 仍不支持；上表保留第三轮记录，最新矩阵与证据见“第四轮阶段 2 复评”。

- `Cargo.toml` 为 `image` 新增 `tiff`/`ico`/`qoi`/`exr`；`ico` 是 `image` 内置（复用 bmp+png），没有额外 crate。MSRV：`image 0.25.10` 要求 Rust 1.88，工作区 `rust-version = 1.95` 满足。
- 许可：`tiff` MIT、`exr` BSD-3-Clause、`qoi` MIT/Apache-2.0、`ico` 随 `image`（MIT/Apache-2.0）。这些都是纯 Rust 解码，不增加原生 DLL。
- **AVIF 的 feature 结论**：`image 0.25.10` 的 `avif` 只启用 `ravif`+`rgb`（编码器，会拉入 `rav1e`，编译与体积成本高）；解码 `AvifDecoder` 只在 `avif-native` 下可用，依赖 `dav1d`/`mp4parse` 原生库。按本轮“不引入新的原生解码库”的边界，两者都未启用。AVIF 由图库侧 ffmpeg 批处理生成缩略图；SHA1/BLAKE3 照常计算，尺寸从 ISO-BMFF `meta/iprp/ipco/ispe` 读取，避免把 512 缩略图尺寸当成原图尺寸。没有 ffmpeg 时 AVIF 仍可入库，但无缩略图/感知哈希，并给出明确损坏提示。
- 新增格式的 alpha 仍按现有规则：PNG/WebP/GIF/TIFF/ICO/QOI/EXR/AVIF/PSD 先让 ffmpeg 写临时 PNG，Rust 合成白底后保存 JPEG；`image` 回退同样白底。批处理参数、`v3` 缓存键、GPL 探测/拒包语义均未改。
- 动画 GIF/WebP 取首帧：ffmpeg 路径固定 `-frames:v 1` 且每输入 `-map i:v`；`image` 路径的 GIF/WebP 解码器同样只取首帧。测试用两帧 GIF 验证首帧颜色。

### HEIC/RAW/PSD 评估（第三轮；PSD 在第四轮接入）

- **HEIC/HEIF**：`libheif-rs` 是 MIT 包装，但需要原生 `libheif`（LGPL）及其 HEVC 解码器；有的构建还牵涉 x265 的 GPL/专利问题。体积、许可与分发复杂度都高于本轮收益；没有真实样本实测，不承诺。
- **RAW**：`rawloader` 是 LGPL-2.1；Rust 默认静态链接，LGPL 合规需要可重链接对象，和当前 MIT/便携包策略冲突。也不应该在没有真实相机样本时承诺格式覆盖。
- **PSD**：`psd` crate 为 MIT/Apache，但只能取合成预览，图层/智能对象/16-bit 等情况需要逐项实测；留到有真实样本时再评估。
- 本轮没有新增 libheif/rawloader/psd 依赖，也没有 ffmpeg-sidecar 或 libav 绑定。

### pHash：保留自研 DCT，不切换 `image_hasher`

用同一批 64×64 合成样本比较（渐变、反色、轻噪声、纯色、灰度）：

| 样本 | 自研 DCT pHash | `image_hasher` 默认 Gradient | `image_hasher` Median+DCT |
|---|---|---|---|
| 渐变 | `aa549f7c875a8c23` | `ffffffffffffffff` | `156a609eea609fea` |
| 反色 | `ffa905a171a303d3` | `0000000000000000` | `ff23a35d03a35c03` |
| 噪声 | `854df67da002ba76` | `7f7f7f7f7f7f7f7f` | `a1d2a7d01c7aa7aa` |
| 与渐变距离（反色/噪声） | 42 / 28 | 64 / 8 | 34 / 30 |

- `image_hasher 3.1.1`（MIT/Apache）没有与现有实现等价的算法：默认是 8×8 Gradient、Lanczos3、LSB-first；`Median + preproc_dct` 也不是“32×32 DCT 取左上 8×8 + MSB-first”。
- 换库会改变所有 `phash` 值，使现有精确/相似去重评审、相似组和阈值失效；小样本距离也显示判定差异（噪声样本 8 vs 28）。本轮**不切换**，继续使用 `media::perceptual_hash`；新增 TIFF/ICO/QOI/EXR 直接复用该实现。
- 若未来要做，需要一次性迁移：保留旧哈希只读、重算并重建相似组/评审状态，且单独验收，不能混在格式扩展里。

### 损坏修复：保留 Python `repair.py`

- 实测：对半截断的 320×240 JPEG，`ffmpeg -err_detect ignore_err` 能输出一张 6170 字节的 JPEG（exit 0），修复能力可用；但对 12 字节的坏文件，ffmpeg 仍 exit 0 却“Nothing was written”，调用方必须额外校验输出文件，否则会把失败当成功。
- ffmpeg 的输出是“猜测补全”的像素，且要重新实现 `--max-size`、PNG/JPEG 输出、`damage/strict_error` JSON 契约与 EXIF 方向；Pillow 的 `LOAD_TRUNCATED_IMAGES` + `exif_transpose` 已经覆盖现有测试，并保留 alpha 白底规则。
- 本轮没有找到“明确、可测、且不改变输出契约”的收益，因此继续走 Python；`repair.py` 无需改动，`sidecar.rs`/`server.py` 的 tag 协议也未改。动画 GIF/WebP 在 Pillow 中默认读取首帧，与缩略图一致。

### 测试与基线

- 新增核心测试：`is_image` 新扩展名/大小写、TIFF/ICO/QOI/EXR 解码哈希、AVIF `ispe` 尺寸解析、QOI alpha 白底、两帧 GIF 首帧、ffmpeg 批处理 alpha/首帧；AVIF 端到端用 `#[ignore]`，需要本机 ffmpeg。
- 运行：`CARGO_TARGET_DIR=D:/Code/Kanalib/target/r2-c cargo test -p kanalib-core --offline`；AVIF 手工测试另加 `KANALIB_FFMPEG=runtime/ffmpeg/bin/ffmpeg.exe` 并加 `--ignored`。
- 10k/100k 基线（52.2s / 738.7s / 42.3min）不受影响：批处理参数、并发、缓存键、回退和 JPEG 校验都没有改；JPEG/PNG 的解码分支和扩展名判断代价不变，新增 feature 只扩大了解码器注册表。本轮未重跑 100k。

## 第四轮阶段 2 复评（任务 B，2026-10-29）

### 结论摘要

- PSD 接入（ffmpeg-only）；HEIC/HEIF、RAW 不接入；动画仍只取首帧，不提供播放器。
- pHash 继续自研 DCT，不换 `image_hasher`；损坏修复继续 Python `repair.py`，sidecar 协议未改。
- `ffprobe.exe` 确认随 LGPL 包分发；视频首帧可用现有批处理验证，时长用 ffprobe 可行但成本约 150ms/文件，本轮不接视频功能。
- `image` 回退、缓存键 `v3`、批处理参数、GPL 探测/拒包均未改。

### 构建、许可与打包核对

| 项 | 实测 |
|---|---|
| 版本 | `n8.1.3-24-g92611900bb-20261026`（BtbN `win64-lgpl-shared-8.1`，archive SHA256 `96AFE9BEE66012F4CBC336A473F0B595060D73BDC5EFE4331D55101500D0A93E`） |
| configuration | 无 `--enable-gpl`；含 `--enable-version3`、`--enable-shared --disable-static`、`--disable-libx264 --disable-libx265 --disable-libxvid` |
| `LICENSE.txt` | GNU Lesser General Public License Version 3（LGPLv3，不是 LGPLv2.1） |
| ffprobe | 与 ffmpeg 同一构建、同一 configuration、同一许可 |
| 打包 | `fetch_ffmpeg_lgpl.ps1` 与 `build_portable.ps1` 复制 `bin` 中除 `ffplay.exe` 外的全部文件，因此 ffprobe 默认随包（该复制规则此前已存在，本轮未改）；`FFMPEG-BUILD.txt` 记录版本、GPL 标志、configuration 与每个二进制的 SHA256 |
| 体积 | `runtime/ffmpeg/bin` 合计 167,598,080 字节（159.8 MiB）；`ffprobe.exe` 226,816 字节（约 222 KiB）；PSD 的运行时/依赖增量 **0 字节**（psd 解码器已在 avcodec-62.dll 内，应用代码仅增加少量分支） |
| 依赖 | PSD 不新增 crate/原生库/DLL；HEIC/RAW 未引入 libheif/rawloader/LibRaw |

GPL 构建仍禁止随包；运行时只把 ffmpeg/ffprobe 当外部进程，不链接 libav。

### 格式矩阵（本轮更新行）

| 扩展名 | 扫描/哈希 | pHash | 缩略图 | `image` 回退 | 方向/EXIF | 备注 |
|---|---|---|---|---|---|---|
| psd | 是 | 否 | 是（LGPL psd） | 否 | 无 | 只读合成图；RGB/Gray/CMYK；alpha 通道不保留；无 ffmpeg 时提示需要 FFmpeg |
| heic/heif | 否 | 否 | 否 | 否 | 不适用 | LGPL 构建无 HEIC/HEIF demuxer；无合法真实样本；未接入 |
| RAW（dng/cr2/nef/arw…） | 否 | 否 | 否 | 否 | 不适用 | ffmpeg 无相机 RAW demuxer；rawloader LGPL 未引入；无真实样本 |

其余格式见上表，未变。PSD 走 `FFMPEG_ONLY_EXTENSIONS`，尺寸从 26 字节文件头读取；缓存键仍为 `v3`。

### PSD：实测与边界

样本全部由测试代码在本机生成（26 字节文件头 + 未压缩平面），不使用第三方图片：

| 样本 | 结果 |
|---|---|
| RGB 96×64（3 通道） | 扫描入库；SHA1/BLAKE3 正常；宽高 96×64；512 缩略图 512×341；phash 为空（`image` 无 PSD 解码器） |
| CMYK 32×32（Photoshop 反相存储） | ffmpeg 转 RGB 为品红 ≈ (254,0,254)，未出现全黑 |
| RGBA 32×32（4 通道） | ffmpeg psd 解码器输出 `gbrp`，alpha 不保留；仍走临时 PNG + 白底路径，无中间文件残留 |
| 灰度 96×64 | 正常解码 |
| 仅文件头（64×64，图像数据截断） | 仍入库并保留内容哈希；扫描不中断；ffmpeg 存在时不标损坏，缺失时提示“需要 FFmpeg 解码 PSD”，不会误判为截断 |
| Unicode/长路径（>150 字符） | 缩略图与尺寸正常 |

边界：

- ffmpeg 的 psd 解码器读取合成图；图层、智能对象、调整层不单独展开。
- PSD 的 alpha 通道当前不保留（ffmpeg 输出不透明合成）；规则仍是透明格式走临时 PNG 再白底。
- 只支持 `.psd`（版本 1），不支持 `.psb`。
- 没有 pHash：相似去重不含 PSD；SHA1/BLAKE3 精确去重正常。
- 没有 ffmpeg 时：可扫描/算内容哈希，但没有缩略图、phash 和回填尺寸；UI 提示需要 FFmpeg。
- 新增 `ensure_thumb` 的 ffmpeg-only 回退：监听新增或缓存淘汰后，`image` 解码失败时按单文件调用 ffmpeg 生成 512，避免只有全量扫描预取才有缩略图；临时 PNG 仍由批处理清理。
- 安全/解压炸弹：PSD 只读固定 26 字节头并校验通道/尺寸/位深/颜色模式；真正的像素解码在 ffmpeg 子进程内，继续受 `-max_pixels 268435456`、120s 超时、`-protocol_whitelist file`、`-threads 1` 与批次隔离约束，Rust 侧不会按 PSD 头分配图像内存。

### HEIC/HEIF、RAW：本轮不接入

- `ffmpeg -formats` 无 `heic`/`heif` demuxer；`-decoders` 有 `hevc`，但没有能读 HEIF 容器的 demuxer。机器上没有合法 HEIC 样本，也没有 HEIC 编码器（ffmpeg 无 heic muxer，Pillow 无 pillow-heif），无法完成真实样本门槛。
- RAW：`ffmpeg -formats` 无 `cr2/cr3/nef/arw/dng` 等相机 RAW demuxer；`image` 无 RAW 解码；`rawloader`/LibRaw 为 LGPL，静态链接分发需额外合规，且无真实相机样本。
- 结论：不写入支持列表；后续接入必须同时满足“真实样本 + 可分发解码链 + 体积/安全/许可审查”。

### 动画 GIF/WebP 与播放入口

- 缩略图仍只取首帧：GIF 有既有测试；两帧 WebP 实测 `image` 回退取首帧（红色）。
- 本机 LGPL ffmpeg n8.1.3 的 `webp` 解码器拒绝动画 WebP（`image data not found`），批处理预取会记一次失败，随后 `ensure_thumb` 用 `image` 回退生成首帧；用户侧仍有缩略图。
- 结论：**需要**播放/逐帧入口（查看器内，非缩略图），否则用户看不到动画；本轮不做播放器，也不新增帧提取 API。入口留给 E1 在查看器内接，Core 继续只提供首帧缩略图。

### pHash：继续自研 DCT（复核）

- 维持第三轮结论：`image_hasher 3.1.1` 没有与现有“32×32 DCT + 左上 8×8 + 中位数 + MSB-first”等价的算法；换库会导致所有 `phash` 变化，使相似组、阈值与人工评审状态失效。
- 本轮没有新增依赖或迁移代码。新增 PSD 因 `image` 无解码器而没有 pHash；AVIF 同理。精确去重按 SHA1 不受影响。
- 若未来切换，必须作为单独迁移：旧哈希只读、全量重算、重建相似组/评审并单独验收。

### 损坏修复：保留 Python `repair.py`

本轮 n8.1.3 复测：

- 4000 字节截断 JPEG：`ffmpeg -err_detect ignore_err` exit 0，输出 4354 字节 JPEG（可读）。
- 仅 JPEG 文件头（SOI/APP0）：ffmpeg exit 69，“Nothing was written”；12 字节垃圾：exit 183，无输出。
- 即使能输出，ffmpeg 仍缺少 `repair.py` 的 `ok/width/height/damage/strict_error` JSON 契约、`--max-size`、`exif_transpose`、alpha 白底与错误分类；调用方还必须额外校验输出文件。
- 结论：继续 Python；`repair.py`、`server.py`、sidecar 打标/修复协议均未改。Pillow 的 `LOAD_TRUNCATED_IMAGES` 继续覆盖现有修复测试。

### 视频首帧与 ffprobe（评估，不接视频功能）

- ffprobe 已随 LGPL 包（见上）；同一 configuration，无 GPL。
- 2 秒 320×240 MP4：`ffprobe -show_entries stream=width,height,duration -show_entries format=duration` 返回 320×240、2.000000；命令可脚本化。
- 成本：本机 100 次 ffprobe 调用 15.0s（约 150ms/文件，含 Windows 进程启动），10k 视频约 25 分钟，不能按图片批处理方式每文件探测。
- 首帧：现有 ffmpeg 批处理用 `-map i:v -frames:v 1` 对 2 个视频输入 0.33s 生成 512×384 JPEG，说明首帧可复用现有池；时长/元数据需要独立探测与缓存。
- 结论：视频首帧可行、时长可读，但扫描/播放/时长索引属于完整视频功能，本轮不做；不把视频扩展名加入 `is_image`。

### 本轮测试与基线

- `cargo test -p kanalib-core`：**118 passed / 11 ignored**（无 ffmpeg 环境；新增 PSD/动画 WebP 测试在无 ffmpeg 时提前返回并计为通过）。
- `KANALIB_FFMPEG=runtime/ffmpeg/bin/ffmpeg.exe cargo test -p kanalib-core psd_ -- --nocapture`：7 passed；`animated_webp` 1 passed；AVIF 2 个 `--ignored` 测试通过。
- 10k 冷导入（同一工作树、同一机器、debug）：

| 构建 | `thumb_prefetch_secs` | 总耗时 |
|---|---:|---:|
| 基线 `bd8f5d7` 测试二进制 | 103.7s | 307.2s |
| 本轮代码 | 78.5s | 284.5s |

历史 52.2s/152.3s 在当前机器状态/负载下，基线二进制本身也无法复现（同机基线 103.7s）；本轮相对同机基线缩略图快 24%，总耗时快 7%，没有回退。100k 未重跑；批处理 ≤32、worker、120s 超时、`max_pixels`、`v3` 缓存键与回退顺序均未改。

## 范围与后续

- 阶段 1 只迁移 **512 缩略图**；SHA1/BLAKE3 仍按字节流计算，EXIF 仍用 `kamadak-exif`。
- pHash 仍由 `image` crate 解码；损坏修复仍走 Python `repair.py`。阶段 2 评估结论见上：两项都保留。
- HEIC/AVIF/RAW/PSD 取决于 ffmpeg 构建：AVIF、PSD 已走 ffmpeg；HEIC/RAW 未支持。
- GPL 随包分发必须由产品/法务明确确认；默认路径是 LGPL 或外部用户自备。
