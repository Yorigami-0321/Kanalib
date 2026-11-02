# 扩展条目参考与本地依据

> 上级索引：[../FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md) · 文档地图：[README](./README.md)

- [digiKam（KDE）](https://apps.kde.org/de/digikam/)：标签、评级、相册、EXIF/IPTC/XMP、RAW、Light Table。
- [PhotoPrism 文档](https://docs.photoprism.app/)：搜索过滤器、相册、重复检测、EXIF、RAW/视频。
- [Eagle 功能参考（Pixso 汇总）](https://pixso.cn/designskills/inspiration-collection-tools/)：智能文件夹、标签、重复识别、以图搜图、自动监视文件夹。
- [VS Code User Interface](https://code.visualstudio.com/docs/getstarted/userinterface)：活动栏、侧栏、资源管理器与面板布局。
- [VS Code Keyboard Shortcuts](https://code.visualstudio.com/docs/getstarted/keybindings)：`Ctrl+B` 切换侧栏、`Ctrl+Shift+E` 聚焦资源管理器。
- [FFmpeg 文档](https://ffmpeg.org/ffmpeg.html)：缩略图命令、`scale`/`thumbnail` 滤镜与输入输出。
- [FFmpeg 滤镜](https://ffmpeg.org/ffmpeg-filters.html)：`scale`、`thumbnail`、`zscale`。
- [FFmpeg 许可](https://ffmpeg.org/legal.html)：GPL/LGPL 构建与分发义务。
- [Lucide grid-3x3](https://lucide.dev/icons/grid-3x3)：F-009 小图标九宫格参考（Ely 当前未内置该 SVG）。
- 现成组件（2026-10-26 查询）：[notify](https://crates.io/crates/notify)、[ignore](https://crates.io/crates/ignore)、[globset](https://crates.io/crates/globset)、[reqwest_dav](https://crates.io/crates/reqwest_dav)、[rusqlite](https://crates.io/crates/rusqlite)、[flexi_logger](https://crates.io/crates/flexi_logger)、[ffmpeg-sidecar](https://crates.io/crates/ffmpeg-sidecar)、[image_hasher](https://crates.io/crates/image_hasher)、[usearch](https://crates.io/crates/usearch)、[lcms2](https://crates.io/crates/lcms2)、[rbook](https://crates.io/crates/rbook)。
- F-064 许可与实现依据：`lcms2` 6.2.0 / `lcms2-sys` 4.0.7 为 MIT，`static` 特性自编 Little CMS（MIT），不依赖系统 DLL；ICC 提取使用 `image` 0.25 的 `ImageDecoder::icc_profile`，未引入额外样本资产。F-049 glob 使用 `globset` 0.4。
- 本地代码与文档：`docs/PLAN.md` 第 2、5、6、7、8、9、10、11、12、15 节；`crates/kanalib-ui/src/lib.rs`；`crates/kanalib-core/src/{model,state,db,media}.rs`；`sidecar/tagger/server.py`；`vendor/ely-gpui-components/src/{menus,overlays,media}/`。
- Windows 资源管理器/VS Code 快捷键与界面参考见上。
