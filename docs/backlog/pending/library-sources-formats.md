# 图库、来源与格式

> 上级索引：[../README.md](../README.md) · 文档地图：[README](../README.md) · 第三轮任务包：[round-3.md](../round-3.md)

## F-030 文件监听与增量同步

- **依据**：`PLAN.md` §6.1 的 `notify` 文件监听；当前依赖只有 `walkdir`，重扫是手动触发。
- **已完成（第一轮）**：按 root 自动启动 `notify` + `notify-debouncer-mini`（1.8s 防抖）；新增/修改增量索引，删除仅在 root 可访问且 `NotFound` 时移除 DB 行，重命名用 SHA1 恢复标签；单批 >500 个有效事件回退整来源重扫；扫描期间忽略事件；headless/`KANALIB_DISABLE_WATCH` 关闭；`rescan_root` 成功后恢复监听。
- **已完成（第三轮，`4dfdf39`）**：目录级重命名与跨来源移动识别——旧来源可访问、旧路径 `NotFound`、全库唯一且按 `idx_assets_sha1` 查询才采纳，多候选一律放弃；启动时离线/权限不足的来源在恢复可访问后自动恢复监听；真实 `notify` 集成测试（临时目录新增/重命名/删除事件、事件风暴不阻塞、不产生重复条目；headless 可跑）。
- **剩余**：OneDrive/网络盘的目录事件语义仍无真实环境验证；监听批次不重建去重分组（等下一次扫描）。
- **验收（剩余）**：网络盘/OneDrive 事件语义；10 万库事件风暴不阻塞 UI。
- **依赖/风险**：网络盘事件语义；事件风暴与 DB 写入竞争；必须与 F-032 的忽略规则一致。

## F-031 单来源扫描与来源状态

> 第一轮已实现（单来源重扫/运行时状态/离线不清理/UI 展示），见 [implemented.md](../implemented.md)。

## F-032 扫描排除规则

> 第三轮已实现（核心影响面/批量保留移出 + 设置页规则编辑器），见 [implemented.md](../implemented.md)。

## F-033 图片格式扩展

- **依据**：`PLAN.md` §8.1 的格式扩展；digiKam/PhotoPrism 支持 RAW/HEIC 等；改造前 `IMAGE_EXTENSIONS` 只有 jpg/jpeg/png/webp/gif/bmp。
- **已完成（第二轮）**：`image` 0.25 打开 `tiff`/`ico`/`qoi`/`exr` 特性，扩展名白名单扩到 tif/tiff/ico/qoi/exr/avif；新格式可算内容哈希、读尺寸，损坏文件仍保留哈希并标记损坏（`new_image_formats_decode_for_hashes`、`truncated_new_format_keeps_content_hashes`）；AVIF 留在 ffmpeg 路径（`FFMPEG_ONLY_EXTENSIONS`），尺寸从 ispe box 读取，ffmpeg 缺失时回退并保留哈希不误判损坏（`avif_dimensions_read_ispe_box`、`avif_ffmpeg_fallback_keeps_hashes_without_damage`）；检查器展示格式友好名与解码能力提示（内置解码 / 需要 FFmpeg）。
- **剩余**：HEIC/HEIF（`libheif-rs`）、RAW（`rawloader`/LibRaw）、PSD 预览——都需先做许可与实测样本评估，不承诺未验证格式；GIF/WebP 动画播放入口（当前只取首帧）。
- **验收（剩余）**：新格式可扫描、生成缩略图、显示尺寸与哈希；损坏文件不阻塞扫描；安装包体积与解码安全性有记录。
- **依赖/风险**：二进制体积、解码器许可证、解码安全（解压炸弹）；不同格式的 EXIF/方向与 F-034 共享处理。

## F-035 Immich/WebDAV 只读来源

- **依据**：`PLAN.md` §9 与阶段 2；`sources` 表已有 `kind/config_json/read_only/last_sync_at/sync_cursor`；当前只有本地根。
- **现状**：没有 HTTP 客户端依赖；无法连接 Immich 或 WebDAV；跨源统一视图与跨源去重也未实现。
- **建议**：定义统一 `Source` trait；Immich 只读（版本探测、分页搜索、`updatedAfter` 增量、缩略图下载、只读约束）；WebDAV（PROPFIND、ETag/Last-Modified、并发限流）；凭据进 Windows Credential Manager；远程缩略图落本地缓存；离线不阻塞本地库；SHA1 跨源精确去重；来源设置页支持测试连接/同步。
- **验收**：按 PLAN 阶段 2 验收：同一张图在本地与 Immich 能归入同一精确重复组；网盘断线不影响本地库；远端不写回。
- **依赖/风险**：范围大；凭据与网络安全；API 版本差异；远程缩略图缓存与 F-060 预算共享。

## F-036 拖拽导入文件夹/图片

- **已完成（第三轮，`f4e72b2`）**：接 `ExternalPaths`/`on_drop`——文件夹提示「添加为来源」、来源内图片定位选中、来源外图片询问是否添加父目录、拖拽高亮遮罩、取消无副作用；路径归属与提示分支有单测（`path_within_respects_separator_and_case`、`folders_are_sources_or_already_known`、`images_locate_inside_and_ask_parent_outside`）。
- **剩余**：真实 Windows 资源管理器拖放未做端到端验证（含日文/Unicode 与长路径）；拖拽载荷与自绘标题栏的兼容性只在窗口消息层验证。
- **验收（剩余）**：真实拖入文件夹可添加并扫描；拖入来源内图片可定位；取消无副作用；重复来源有提示。
- **依赖/风险**：误添加大目录；权限/网络路径；拖放事件与自绘标题栏的兼容。

## F-037 从图库移除选中资产

> 第三轮已实现（核心移除 + 路径/SHA1/glob 忽略列表 + 设置页忽略列表 UI），见 [implemented.md](../implemented.md)。

## F-040 缩略图/文件处理改用 ffmpeg（阶段 2）

> 阶段 1 已实现：512 缩略图走 ffmpeg 多输入批处理池；默认 LGPL，GPL 不随包；缓存键 `v3`；扫描前预生成、失败二分重试、`image` 回退。详细许可/进程模型/基准见 [`docs/FFMPEG.md`](../../FFMPEG.md)。本节只保留剩余阶段与不可回退约束。

- **已完成**：路径解析 env → PATH → `runtime/ffmpeg` → exe 同级（debug 另含 `tools/ffmpeg`）；`--enable-gpl` 探测；每批 ≤32、120s 超时、`-protocol_whitelist file`、`max_pixels`、无窗口；alpha 合成白底 JPEG；t256 从 512 派生；缓存键 `v3`；10k 缩略图 52.2s、100k 738.7s、总 42.3min；`runtime/ffmpeg` 为 LGPL shared，`tools/ffmpeg` GPL 仅本地开发。第二轮阶段 2：AVIF 走 ffmpeg-only 路径（`image` 0.25 无 AVIF 解码器，哈希照算、尺寸从 ispe box 读、缺 ffmpeg 时给“需要 FFmpeg 解码”提示而不误判损坏）；动画 GIF/WebP 取首帧；含 alpha 的新格式先写临时 PNG 再白底合成；批次内损坏输入被隔离。pHash 仍走自研 DCT（不换 `image_hasher`），损坏修复仍走 Python。
- **剩余阶段**：pHash/损坏修复迁移到 ffmpeg 的可行性结论与实施；视频（第一帧/时长）扩展；ffprobe/打包与许可证落盘；HEIC/RAW/PSD 仅在实测样本与许可审查后承诺。
- **约束**：批处理参数不得经过 shell；Unicode/长路径用 `Command` 参数；临时文件必须清理；GPL 构建不得随包；ffmpeg 缺失/损坏时回退 `image` 并记日志；100k 指标不得劣于 738.7s/42.3min。
- **验收**：阶段 2 的真实/合成样本（竖图、横图、alpha、CMYK、动画首帧、Unicode、超大图）通过；无进程/临时文件泄漏；许可与打包清单明确；pHash/修复迁移不改变现有去重评审语义。
- **未决问题**：pHash 是否换 `image_hasher`（会改变哈希/评审状态）；修复迁 ffmpeg 还是保留 Python；是否随包 `ffprobe`；LGPL 打包范围；alpha 白底还是 PNG；失败回退策略。
- **依赖/风险**：F-009 的大图标 512 档；F-060 缓存预算/访问记录；F-033 格式扩展；F-002 查看器原图；GPL/LGPL、213MB 单文件体积是首要风险。
