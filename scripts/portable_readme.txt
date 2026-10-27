Kanalib 便携版

启动
- 双击 kanalib.exe，不会显示控制台窗口。
- 首次启动会生成 .kanalib 数据目录，但不会自动添加任何文件夹；请通过左侧「添加文件夹」选择，可添加多个。
- 来源项支持右键菜单；「重命名」只修改图库内的显示别名，不改磁盘文件夹名。
- 打标需要 runtime/python 与 dist/models；本目录已包含，不要单独移动 exe。
- 缩略图优先使用 PATH 中的 ffmpeg；也可把 LGPL 构建放到程序目录的 runtime/ffmpeg（本包默认随附）。两者都缺失或失败时回退内置解码。
- 需要重新获取 LGPL ffmpeg 时，可在本目录运行 fetch-ffmpeg-lgpl.ps1（固定版本/SHA256，下载到 runtime/ffmpeg）。许可证与构建信息见 runtime/ffmpeg/ 下的 LICENSE.txt、FFMPEG-SOURCE.txt 与 FFMPEG-BUILD.txt。

数据
- 数据库、缩略图缓存、设置都在 exe 同级的 .kanalib 下。
- 正常界面运行的日志写入 .kanalib/logs/kanalib.log（按大小/天数滚动保留）；设置环境变量 KANALIB_LOG_FILE 可把日志改到指定文件。
- 无界面模式（KANALIB_HEADLESS=1）的日志写入 .kanalib/headless.log；sidecar 的诊断输出写入 .kanalib/sidecar.log。
- 数据库备份默认写入 .kanalib/backups；设置页会提供「立即备份/打开备份目录」入口（后续版本继续完善恢复流程）。备份前请避免同时进行扫描。
- 移除来源只从图库视图移除，不会删除磁盘文件。
- 去重删除只会把副本移入 Windows 回收站，不会自动删除。

迁移
- 整个目录可以复制到其他固定磁盘；路径变化后请在设置里重新添加图库文件夹。
- 不建议放在只读目录或网络共享路径下运行。

校验
- 根目录的 PACKAGE-MANIFEST.txt 记录应用版本、ffmpeg 版本与许可探测结果（LGPL/GPL）、runtime/models/sidecar 关键文件的 SHA256，以及打包文件总数与总大小。
- 可用 PowerShell 复核单个文件：Get-FileHash -LiteralPath <文件> -Algorithm SHA256，把结果与清单中的 sha256 对比；不一致时请勿运行，重新获取完整包。
- 清单按路径排序且不含生成时间，同一份构建重复打包应得到相同内容；文件总数不含清单自身。

损坏图片
- 详情面板会把解码失败标成「损坏/不完整」；哈希与其余元数据仍保留。
- 「容错重解」只重建预览缓存并重试打标，不改原文件；「导出修复副本」会另存为新文件。
- sidecar 的诊断输出写在 .kanalib/sidecar.log。

常见问题
- 如果启动提示缺少模型或 Python，请确认 runtime/python、dist/models 与 sidecar 目录完整。
- 推理设备与队列状态显示在底部状态栏；设置页可以查看缓存并清理孤儿缩略图。
- 便携版默认无控制台。无界面扫描或诊断时设置 KANALIB_HEADLESS=1，日志写入 .kanalib/headless.log；也可用 KANALIB_LOG_FILE 指定日志路径。
