# 图库、来源与格式

> 上级索引：[../README.md](../README.md) · 文档地图：[README](../README.md)

## F-030 文件监听与增量同步

- **依据**：`PLAN.md` §6.1 的 `notify` 文件监听；当前依赖只有 `walkdir`，重扫是手动触发。
- **已完成（第一轮）**：按 root 自动启动 `notify` + `notify-debouncer-mini`（1.8s 防抖）；新增/修改增量索引，删除仅在 root 可访问且 `NotFound` 时移除 DB 行，重命名用 SHA1 恢复标签；单批 >500 个有效事件回退整来源重扫；扫描期间忽略事件；headless/`KANALIB_DISABLE_WATCH` 关闭；`rescan_root` 成功后恢复监听。
- **剩余**：目录级重命名/跨来源移动（旧记录可能需手动/全量重扫）；启动即离线的来源不会自动恢复监听；真实 notify 集成测试（当前为分类纯函数与临时目录测试）；监听批次不重建去重分组（等下一次扫描）。
- **验收（剩余）**：目录重命名、离线恢复、OneDrive/网络盘事件语义；10 万库事件风暴不阻塞 UI；不产生重复条目。
- **依赖/风险**：网络盘/OneDrive 的事件语义；事件风暴与 DB 写入竞争；必须与 F-032 的排除规则一致。

## F-031 单来源扫描与来源状态

> 第一轮已实现（单来源重扫/运行时状态/离线不清理/UI 展示），见 [implemented.md](../implemented.md)。

## F-032 扫描排除规则

- **依据**：`PLAN.md` §8.1 的“排除规则”；改造前 `walkdir` 只按扩展名过滤。
- **已完成（第一轮）**：`ignore`/globset 按来源应用排除 glob（显式关闭 gitignore/global/parents/.ignore 语义）、隐藏开关、min/max 字节；默认 `include_hidden=true` 保持旧语义；配置存 `settings.scan.filters`；提供同步预览命中数/样例 API；无规则时走原 walkdir 快路径；设置页显示只读摘要。
- **剩余**：排除规则编辑器（UI 录入/校验）；对已入库但被新规则排除的资产提供「保留/移出图库」选择；预览在超大隐藏目录下的性能。
- **验收（剩余）**：规则修改后可编辑/预览/执行清理；不会因规则误删磁盘文件；重扫结果可解释。
- **依赖/风险**：glob 语义与 Windows 路径分隔符；排除策略不能与缺失对账混淆（已用 `definitely_missing` 避免误清理/误重命名）。

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

- **依据**：代码/常见桌面 UX；当前应用没有 `on_drop`/`ExternalPaths` 处理，只能通过“添加文件夹”对话框。
- **现状**：把文件夹拖进窗口不会发生任何事；Ely 的聊天组件已有拖放示例可参考。
- **建议**：拖入文件夹时提示“添加为来源”；拖入图片时，若位于已有来源内则选中并定位，否则询问是否添加其父目录；提供拖拽高亮遮罩；不复制文件（Kanalib 仍是索引式图库）；与 F-032 的排除规则、F-030 的监听协同。
- **验收**：拖入文件夹可添加并开始扫描；拖入来源内图片可定位；取消不产生副作用；重复来源有提示。
- **依赖/风险**：误添加大目录；权限/网络路径；拖拽事件与自绘标题栏的兼容。

## F-037 从图库移除选中资产

- **依据**：`PLAN.md` §6.2“绝不自动删除”与来源移除语义；当前只能移除整个来源或确认缺失清理。
- **现状**：没有“把选中的图片从图库移除但不删除磁盘文件”的操作；直接删除 DB 行后，下次重扫会重新导入。
- **建议**：增加“从图库移除”与“不再导入此路径/此规则”；移除只删 DB 记录并刷新计数/选择/去重；忽略列表持久化，重扫时跳过；提供查看/恢复忽略列表的入口；与 F-032 的排除规则合并成统一“忽略规则”。
- **验收**：移除后磁盘文件不变、网格与计数立即更新；重扫不会重新导入已忽略路径；可恢复；去重与标签关系同步清理。
- **依赖/风险**：忽略粒度（路径/哈希/glob）；路径变化后的识别；误操作恢复。

## F-040 缩略图/文件处理改用 ffmpeg（阶段 2）

> 阶段 1 已实现：512 缩略图走 ffmpeg 多输入批处理池；默认 LGPL，GPL 不随包；缓存键 `v3`；扫描前预生成、失败二分重试、`image` 回退。详细许可/进程模型/基准见 [`docs/FFMPEG.md`](../../FFMPEG.md)。本节只保留剩余阶段与不可回退约束。

- **已完成**：路径解析 env → PATH → `runtime/ffmpeg` → exe 同级（debug 另含 `tools/ffmpeg`）；`--enable-gpl` 探测；每批 ≤32、120s 超时、`-protocol_whitelist file`、`max_pixels`、无窗口；alpha 合成白底 JPEG；t256 从 512 派生；缓存键 `v3`；10k 缩略图 52.2s、100k 738.7s、总 42.3min；`runtime/ffmpeg` 为 LGPL shared，`tools/ffmpeg` GPL 仅本地开发。第二轮阶段 2：AVIF 走 ffmpeg-only 路径（`image` 0.25 无 AVIF 解码器，哈希照算、尺寸从 ispe box 读、缺 ffmpeg 时给“需要 FFmpeg 解码”提示而不误判损坏）；动画 GIF/WebP 取首帧；含 alpha 的新格式先写临时 PNG 再白底合成；批次内损坏输入被隔离。pHash 仍走自研 DCT（不换 `image_hasher`），损坏修复仍走 Python。
- **剩余阶段**：pHash/损坏修复迁移到 ffmpeg 的可行性结论与实施；视频（第一帧/时长）扩展；ffprobe/打包与许可证落盘；HEIC/RAW/PSD 仅在实测样本与许可审查后承诺。
- **约束**：批处理参数不得经过 shell；Unicode/长路径用 `Command` 参数；临时文件必须清理；GPL 构建不得随包；ffmpeg 缺失/损坏时回退 `image` 并记日志；100k 指标不得劣于 738.7s/42.3min。
- **验收**：阶段 2 的真实/合成样本（竖图、横图、alpha、CMYK、动画首帧、Unicode、超大图）通过；无进程/临时文件泄漏；许可与打包清单明确；pHash/修复迁移不改变现有去重评审语义。
- **未决问题**：pHash 是否换 `image_hasher`（会改变哈希/评审状态）；修复迁 ffmpeg 还是保留 Python；是否随包 `ffprobe`；LGPL 打包范围；alpha 白底还是 PNG；失败回退策略。
- **依赖/风险**：F-009 的大图标 512 档；F-060 缓存预算/访问记录；F-033 格式扩展；F-002 查看器原图；GPL/LGPL、213MB 单文件体积是首要风险。
