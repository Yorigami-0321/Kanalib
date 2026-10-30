# 图库、来源与格式

> 上级索引：[../README.md](../README.md) · 文档地图：[README](../README.md) · 第三轮任务包：[round-3.md](../round-3.md)

## F-030 文件监听与增量同步

- **依据**：`PLAN.md` §6.1 的 `notify` 文件监听；当前依赖只有 `walkdir`，重扫是手动触发。
- **已完成（第一轮）**：按 root 自动启动 `notify` + `notify-debouncer-mini`（1.8s 防抖）；新增/修改增量索引，删除仅在 root 可访问且 `NotFound` 时移除 DB 行，重命名用 SHA1 恢复标签；单批 >500 个有效事件回退整来源重扫；扫描期间忽略事件；headless/`KANALIB_DISABLE_WATCH` 关闭；`rescan_root` 成功后恢复监听。
- **已完成（第三轮，`4dfdf39`）**：目录级重命名与跨来源移动识别——旧来源可访问、旧路径 `NotFound`、全库唯一且按 `idx_assets_sha1` 查询才采纳，多候选一律放弃；启动时离线/权限不足的来源在恢复可访问后自动恢复监听；真实 `notify` 集成测试（临时目录新增/重命名/删除事件、事件风暴不阻塞、不产生重复条目；headless 可跑）。
- **已完成（第四轮，2026-10-30）**：
  - **批次收尾去重刷新**：`apply_watch_batch` 收集本批新增/修改/重命名的当前路径与旧路径（`renamed_from`）以及确认删除路径，在批次收尾只调用一次 `DedupState::apply_exact_changes`。它只重建受影响的 SHA1 组（一次 O(n) 受影响哈希扫描 + 小组重建），重命名/跨来源移动/修改后旧路径随当前 `assets` 消失而不残留在组里；组从 2 张降到 1 张时立即消失。相似组不因监听新增/修改而清空，删除仍走 `prune_similar`（整组剪枝语义不变）。
  - **revision**：去重刷新本身不再 bump revision，每批只在收尾统一 bump 一次；分组重建只在批次收尾发生，不在逐文件循环里。逐文件索引沿用既有 `Shared::upsert`/`move_asset` 的 revision（未改 `model.rs`）。
  - **成本实测（本机，合成 10 万资产 / 5 万组最坏组数）**：`rebuild_exact` 单次 release 385ms（dev test profile 371–461ms）；`apply_exact_changes` 10 条受影响路径 release 68ms（dev 65–97ms）；未受影响的组不重建、不全量排序，只按原比较器插回。选择增量而非“每批全量重建”：小批次成本约 1/6，且刷新发生在防抖线程（1.8s 窗口），不占 UI 线程；扫描结束仍用全量 `refresh_dedup`。
  - **事件风暴**：10 万路径分类实测 all-changed 75.8ms / all-missing 65.2ms（dev test profile），超过 `WATCH_EVENT_LIMIT` 后 `changed`/`missing` 清空并回退整来源重扫，不逐文件哈希/缩略图、不产生删除候选；分类与 DB 写入都在 `notify-debouncer` 回调线程，UI 只在 revision 变化后读快照。
  - **网络盘/离线语义（临时目录模拟）**：来源目录不可见时缺失事件一律保留记录、不重建组、来源状态置离线；目录恢复后下一批次把来源标回在线；监听重试线程（10s）对启动即离线或运行中报“离线/监听”错误的来源，在目录恢复可访问后丢弃旧句柄并重建 watcher，新写入的文件能被真实 `notify` 事件入库。权限/IO 错误（`WatchProbe::Uncertain`）只计入 ignored，绝不进入 `missing`。目录级重命名若平台补发文件级事件，同批次按 SHA1 接管并更新组；若只发目录事件，监听层忽略并由下一次扫描对账。跨来源移动仍要求旧来源可访问、旧路径 `NotFound`、全库唯一。
- **剩余**：真实 OneDrive/网络盘环境未实测——没有真实云端占位文件（dehydrated/hydrated）、网络共享或断线重放环境，因此云端占位符事件、离线期间事件合并/丢失、重连后的通知补发、SMB 变更通知的延迟/丢失率、目录级重命名的事件粒度均不能假设为与本地 NTFS 相同；监听仍不直接处理“目录事件”本身（依赖平台补发文件事件或扫描对账）；跨来源移动目前只覆盖本地根目录之间。
- **验收（剩余）**：在真实 OneDrive 同步目录与网络共享上验证新增/修改/删除/目录重命名/离线重连事件语义，并记录与本地临时目录的差异；若平台不补发文件事件，确认扫描对账可作为兜底且不产生过期去重组。
- **依赖/风险**：网络盘/云端事件语义未实测；事件风暴与 DB 写入竞争；必须与 F-032 的忽略规则一致。

## F-031 单来源扫描与来源状态

> 第一轮已实现（单来源重扫/运行时状态/离线不清理/UI 展示），见 [implemented.md](../implemented.md)。

## F-032 扫描排除规则

> 第三轮已实现（核心影响面/批量保留移出 + 设置页规则编辑器），见 [implemented.md](../implemented.md)。

## F-033 图片格式扩展

- **依据**：`PLAN.md` §8.1 的格式扩展；digiKam/PhotoPrism 支持 RAW/HEIC 等；改造前 `IMAGE_EXTENSIONS` 只有 jpg/jpeg/png/webp/gif/bmp。
- **已完成（第二轮）**：`image` 0.25 打开 `tiff`/`ico`/`qoi`/`exr` 特性，扩展名白名单扩到 tif/tiff/ico/qoi/exr/avif；新格式可算内容哈希、读尺寸，损坏文件仍保留哈希并标记损坏（`new_image_formats_decode_for_hashes`、`truncated_new_format_keeps_content_hashes`）；AVIF 留在 ffmpeg 路径（`FFMPEG_ONLY_EXTENSIONS`），尺寸从 ispe box 读取，ffmpeg 缺失时回退并保留哈希不误判损坏（`avif_dimensions_read_ispe_box`、`avif_ffmpeg_fallback_keeps_hashes_without_damage`）；检查器展示格式友好名与解码能力提示（内置解码 / 需要 FFmpeg）。
- **已完成（第四轮，2026-10-29）**：PSD 接入（ffmpeg-only）——`.psd` 进入扫描白名单，SHA1/BLAKE3 照常；尺寸读 26 字节文件头（`media::fallback_dimensions`）；512 缩略图交给 LGPL ffmpeg 的 psd 解码器，alpha 仍走临时 PNG + 白底；监听新增/缓存淘汰时 `ensure_thumb` 可按单文件回退 ffmpeg。实测合成样本覆盖 RGB、CMYK（反相存储转品红 ≈254,0,254）、灰度、RGBA（ffmpeg 输出 `gbrp`，alpha 不保留）、仅文件头截断、Unicode/长路径；扫描/尺寸/哈希/缩略图测试见 `media.rs`/`state.rs`。PSD 未新增 crate/原生库，运行时/依赖包体积增量 0（应用代码仅少量分支）。动画两帧 WebP 的 `image` 回退确认取首帧；本机 ffmpeg n8.1.3 拒绝动画 WebP，预取失败后由 `image` 回退补首帧。
  - **UI 收口（第四轮 E1，`02e4960`）**：检查器把 PSD 显示为「格式 PSD · 需要 FFmpeg 解码 · 仅合成图」，并写明图层/智能对象不展开、Alpha 不保留、无 pHash、不支持 `.psb`；无 ffmpeg 时提示只有哈希/尺寸、无缩略图。GIF/WebP 显示「内置解码 · 动画仅首帧」，查看器提供「用系统应用播放」出口并在底部提示“动画仅首帧，播放请用系统应用”；本轮不实现应用内逐帧播放。HEIC/RAW 仍不进入支持列表。截图：`.kanalib/r4_e1/screens/psd-inspector.png`、`gif-inspector.png`、`gif-viewer.png`。
- **第五轮（2026-10-30，任务 C）**：格式门统一为 `is_unsupported_format` + `UNSUPPORTED_FORMAT_NOTE`；HEIC/HEIF、相机 RAW 不扫描/不入库/不解码，UI 只提示「不支持」，不伪装支持；无真实样本、可分发解码链与许可结论，状态仍为未完成。
- **剩余**：HEIC/HEIF（`libheif-rs` + HEVC）与 RAW（`rawloader`/LibRaw）仍不接入：本机 LGPL ffmpeg 无 HEIC/HEIF 与相机 RAW demuxer，仓库/机器无合法真实样本，rawloader/LibRaw 为 LGPL 且静态链接分发需额外合规。PSD 的图层/智能对象只显示合成图、alpha 不保留、无 pHash（相似去重不含 PSD）、不支持 `.psb`；动画 GIF/WebP 未做应用内逐帧播放，E1 只提供“用系统应用播放”出口，Core 继续只取首帧。
- **验收（剩余）**：HEIC/RAW 需真实样本 + 可分发解码链 + 体积/安全/许可审查后才写入支持列表；动画入口需查看器内实测；PSD 只在有真实分层/CMYK 样本时再扩大承诺。
- **依赖/风险**：二进制体积、解码器许可证、解码安全（解压炸弹，PSD 已受 `max_pixels`/120s 超时约束）；不同格式的 EXIF/方向与 F-034 共享处理；无 ffmpeg 时 PSD 只有哈希、没有缩略图/尺寸/phash，UI 提示需要 FFmpeg。

## F-035 Immich/WebDAV 只读来源

- **依据**：`PLAN.md` §9 与阶段 2；`sources` 表已有 `kind/config_json/read_only/last_sync_at/sync_cursor`；当前只有本地根。
- **第四轮现状（2026-10-28 前）**：没有 HTTP 客户端依赖；无法连接 Immich 或 WebDAV；跨源统一视图与跨源去重也未实现。
- **建议**：定义统一 `Source` trait；Immich 只读（版本探测、分页搜索、`updatedAfter` 增量、缩略图下载、只读约束）；WebDAV（PROPFIND、ETag/Last-Modified、并发限流）；凭据进 Windows Credential Manager；远程缩略图落本地缓存；离线不阻塞本地库；SHA1 跨源精确去重；来源设置页支持测试连接/同步。
- **第五轮已完成（2026-10-30，任务 B + D；`8340f71`/`3d9bb40`，合并 `18a0155`/`3025605`）**：
  1. **Source 抽象**：本地 roots 包成 `Source` trait，行为与数据不变；`Shared.sources` 统一列表，本地扫描/监听/清理仍只用 roots。
  2. **Immich 只读 MVP**：版本探测、分页搜索、`updatedAfter` 增量、缩略图缓存、只读约束；凭据进 `keyring`；断线不阻塞本地库。
  3. **WebDAV 只读**：PROPFIND Depth 0/1、ETag/Last-Modified、4 并发限流 + 退避、目录缓存；凭据同上。
  4. **跨源精确去重**：按 SHA1 归组，复用现有去重评审状态；远端资产用 `immich://`/`webdav://` 虚拟路径，回收站/移动/删除三重拦截。
- **交付验证（2026-10-30）**：core 12 项 mock 单测（本地不回归、版本/分页/`updatedAfter`、断线、凭据不落库、PROPFIND/限流、跨源归组）+ UI 测试 3 项 + 设置页截图 `.kanalib/r5d/settings3.png`、`.kanalib/r5d/dialog.png`；合并后 core 177/16、UI 52、bin 6。
- **未验证边界**：未联真实 Immich/WebDAV（全部回环 mock）；WebDAV 无 SHA1 不参与精确归组且不下载 original；远端删除暂不清理旧资产行；添加/测试/同步的完整 UI 提交链路未用真实键入跑通（GPUI TextInput 不接受 UIA `ValuePattern`）。
- **第五轮边界（不做）**：写回、删除/移动远端、上传、下载 original、跨源相似去重（后续轮次）；不改本地 Source 的既有行为；网络源测试只用隔离数据根/测试容器，不碰真实图库；`sources` 表迁移前必须先备份。
- **验收（第五轮）**：同一张图在本地与 Immich 归入同一 SHA1 组；断线/凭据失效不阻塞本地库；远端零写入；本地 Source 行为不回归；`sources` 表迁移前备份存在；来源测试连接/同步/空结果有隔离根截图或 UIA。
- **依赖/风险**：范围大；凭据与网络安全；API 版本差异；远程缩略图缓存与 F-060 预算共享。

## F-036 拖拽导入文件夹/图片

- **已完成（第三轮，`f4e72b2`）**：接 `ExternalPaths`/`on_drop`——文件夹提示「添加为来源」、来源内图片定位选中、来源外图片询问是否添加父目录、拖拽高亮遮罩、取消无副作用；路径归属与提示分支有单测（`path_within_respects_separator_and_case`、`folders_are_sources_or_already_known`、`images_locate_inside_and_ask_parent_outside`）。
- **剩余（2026-10-30 复核）**：真实 Windows 资源管理器拖放仍未做端到端验证（含日文/Unicode 与长路径）。窗口内 UIA/消息自动化无法可靠合成系统拖放载荷（`ExternalPaths` 由系统拖放目标注入），本轮不重跑、也不写成已通过；拖拽载荷与自绘标题栏的兼容性只在窗口消息层由单测覆盖。**待用户手动验收。**
- **手动验收清单（待用户手动验收）**：
  1. **文件夹**：从资源管理器拖入未加入的文件夹 → 出现「添加为来源」提示；确认后来源列表出现该文件夹并开始扫描，计数增加。
  2. **来源内图片**：拖入已添加来源内的图片 → 定位并选中该图，不新增来源、不重复扫描。
  3. **来源外图片**：拖入未加入来源的图片 → 询问是否添加父目录；确认后加入，取消则无任何变化。
  4. **取消**：拖拽中按 Esc 或拖回原处 → 无提示残留、无来源/选择变化、高亮遮罩消失。
  5. **Unicode/日文/长路径**：`D:\测试\日本語\写真\...`（含 >260 字符路径）→ 路径不乱码、不截断，能正常导入与定位。
  6. **重复来源**：拖入已在来源列表中的文件夹 → 有明确「已存在/已加入」提示，不重复添加、不重复扫描。
  7. **自绘标题栏兼容**：从窗口标题栏/边缘进入并放下 → 高亮范围与落点正确，无标题栏吞事件。
- **验收（剩余）**：清单 1–7 手动通过后才把 F-036 记为已实现；任一项失败按缺陷回到实现线，并记录失败路径与截图。
- **依赖/风险**：误添加大目录；权限/网络路径；拖放事件与自绘标题栏的兼容。

## F-037 从图库移除选中资产

> 第三轮已实现（核心移除 + 路径/SHA1/glob 忽略列表 + 设置页忽略列表 UI），见 [implemented.md](../implemented.md)。

## F-040 缩略图/文件处理改用 ffmpeg（阶段 2，第五轮 C+D 已收口）

> 阶段 1 已实现：512 缩略图走 ffmpeg 多输入批处理池；默认 LGPL，GPL 不随包；缓存键 `v3`；扫描前预生成、失败二分重试、`image` 回退。详细许可/进程模型/基准见 [`docs/FFMPEG.md`](../../FFMPEG.md)。本节只保留剩余阶段与不可回退约束。

- **已完成**：路径解析 env → PATH → `runtime/ffmpeg` → exe 同级（debug 另含 `tools/ffmpeg`）；`--enable-gpl` 探测；每批 ≤32、120s 超时、`-protocol_whitelist file`、`max_pixels`、无窗口；alpha 合成白底 JPEG；t256 从 512 派生；缓存键 `v3`；10k 缩略图 52.2s、100k 738.7s、总 42.3min；`runtime/ffmpeg` 为 LGPL shared，`tools/ffmpeg` GPL 仅本地开发。第二轮阶段 2：AVIF 走 ffmpeg-only 路径（`image` 0.25 无 AVIF 解码器，哈希照算、尺寸从 ispe box 读、缺 ffmpeg 时给“需要 FFmpeg 解码”提示而不误判损坏）；动画 GIF/WebP 取首帧；含 alpha 的新格式先写临时 PNG 再白底合成；批次内损坏输入被隔离。pHash 仍走自研 DCT（不换 `image_hasher`），损坏修复仍走 Python。第四轮（2026-10-29）：PSD 以 ffmpeg-only 接入（详见 F-033）；复核 pHash/修复结论不变；核对 ffprobe 与 LGPLv3 构建参数，ffprobe 默认随包；视频首帧/时长仅评估，未接视频功能。
- **第五轮已完成（2026-10-30，任务 C + D；`40baa4f`/`582d9e2`，合并 `d1df29a`/`3025605`）**：视频作为新 `media_kind`；首帧走既有批处理池（512、120s、`-frames:v 1`）；ffprobe 取时长/宽高/编码（20s 超时、可关）；系统默认播放器入口只 `spawn`；视频不进 pHash/打标队列/不标损坏；schema v9→v10 幂等补列并先备份；网格视频瓦片首帧+时长角标，查看器固定首帧并提供系统播放；远端虚拟路径在 UI 全面只读拦截。core 合并后 177/16、UI 52；10k 冷导入 213.7s / 0 失败；视频样本 2.0s/320×240、首帧 512×384（`.kanalib/r5c/`、`.kanalib/r5d/grid2.png`）。
- **剩余阶段**：HEIC/RAW 仅在真实样本与许可审查后承诺（F-033 继续 pending）；PSD 的图层/Alpha/pHash/PSB 边界；动画 GIF/WebP 不做应用内逐帧播放（仅系统应用出口）；不做内置视频播放器/转码/批量视频打标；100k 未重跑、系统播放入口未实点打开。
- **约束**：批处理参数不得经过 shell；Unicode/长路径用 `Command` 参数；临时文件必须清理；GPL 构建不得随包；ffmpeg 缺失/损坏时回退 `image` 并记日志；100k 指标不得劣于 738.7s/42.3min；ffprobe 与 ffmpeg 同 configuration、同 LGPL 许可，不单独引入 GPL 工具。
- **验收**：阶段 2 的真实/合成样本（竖图、横图、alpha、CMYK、动画首帧、Unicode、超大图）通过；无进程/临时文件泄漏；许可与打包清单明确；pHash/修复迁移不改变现有去重评审语义；ffprobe 随包体积与许可落盘。
- **未决问题（第四轮已决）**：pHash 不换（会改变全部哈希/评审状态，且无等价算法）；修复保留 Python（ffmpeg 缺少 JSON/`--max-size`/EXIF/alpha 契约）；`ffprobe` 随包（同一 LGPL 构建，`bin` 除 `ffplay` 外全部复制）；LGPL 打包范围保持 `runtime/ffmpeg` 全 `bin`；alpha 继续白底 JPEG；失败回退保持预取二分 + `image`。
- **依赖/风险**：F-009 的大图标 512 档；F-060 缓存预算/访问记录；F-033 格式扩展；F-002 查看器原图；GPL/LGPL、213MB 单文件体积是首要风险；同机 10k 复测基线二进制 103.7s、本轮 78.5s（历史 52.2s 在当前机器状态不可复现），100k 未重跑。
