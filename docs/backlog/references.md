# 扩展条目参考与本地依据

> 上级索引：[../FEATURE-BACKLOG.md](../FEATURE-BACKLOG.md) · 文档地图：[README](./README.md)
>
> 参考层级（2026-11-02）：主参考为图片/素材管理器；桌面系统惯例为次；VS Code 仅作命令面板、键盘/焦点、菜单行为等局部参考，不作为整体 UI 方向。

- [digiKam（KDE）](https://apps.kde.org/de/digikam/)：标签、评级、相册、EXIF/IPTC/XMP、RAW、Light Table（主参考）。
- [PhotoPrism 文档](https://docs.photoprism.app/)：搜索过滤器、相册、重复检测、EXIF、RAW/视频（主参考）。
- [Eagle 功能参考（Pixso 汇总）](https://pixso.cn/designskills/inspiration-collection-tools/)：智能文件夹、标签、重复识别、以图搜图、自动监视文件夹（主参考）。
- [Lightroom Classic Keywords](https://helpx.adobe.com/lightroom-classic/help/keywords.html)：导航筛选、关键词分配与 Keyword List 管理（主参考）。
- [Immich Tags](https://docs.immich.app/features/tags)、[Capture One](https://www.captureone.com/)：标签页/检查器分配与关键字库分离（主参考）。
- [VS Code User Interface](https://code.visualstudio.com/docs/getstarted/userinterface)：仅局部参考命令面板、键盘/焦点与菜单行为，不作为整体布局或搜索视图目标。
- [VS Code Keyboard Shortcuts](https://code.visualstudio.com/docs/getstarted/keybindings)：仅参考快捷键习惯（`Ctrl+B` 侧栏、`Ctrl+F` 搜索）。
- [FFmpeg 文档](https://ffmpeg.org/ffmpeg.html)：缩略图命令、`scale`/`thumbnail` 滤镜与输入输出。
- [FFmpeg 滤镜](https://ffmpeg.org/ffmpeg-filters.html)：`scale`、`thumbnail`、`zscale`。
- [FFmpeg 许可](https://ffmpeg.org/legal.html)：GPL/LGPL 构建与分发义务。
- [Lucide grid-3x3](https://lucide.dev/icons/grid-3x3)：F-009 小图标九宫格参考（Ely 当前未内置该 SVG）。
- 现成组件（2026-10-26 查询）：[notify](https://crates.io/crates/notify)、[ignore](https://crates.io/crates/ignore)、[globset](https://crates.io/crates/globset)、[reqwest_dav](https://crates.io/crates/reqwest_dav)、[rusqlite](https://crates.io/crates/rusqlite)、[flexi_logger](https://crates.io/crates/flexi_logger)、[ffmpeg-sidecar](https://crates.io/crates/ffmpeg-sidecar)、[image_hasher](https://crates.io/crates/image_hasher)、[usearch](https://crates.io/crates/usearch)、[lcms2](https://crates.io/crates/lcms2)、[rbook](https://crates.io/crates/rbook)。
- F-064 许可与实现依据：`lcms2` 6.2.0 / `lcms2-sys` 4.0.7 为 MIT，`static` 特性自编 Little CMS（MIT），不依赖系统 DLL；ICC 提取使用 `image` 0.25 的 `ImageDecoder::icc_profile`，未引入额外样本资产。F-049 glob 使用 `globset` 0.4。
- 本地代码与文档：`docs/PLAN.md` 第 2、5、6、7、8、9、10、11、12、15 节；`crates/kanalib-ui/src/lib.rs`；`crates/kanalib-core/src/{model,state,db,media}.rs`；`sidecar/tagger/server.py`；`vendor/ely-gpui-components/src/{menus,overlays,media}/`。
- Windows 资源管理器作为桌面壳层次参考；VS Code 仅作局部交互参考，不主导整体 UI 方向。
