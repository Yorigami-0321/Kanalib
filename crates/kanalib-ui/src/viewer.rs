use crate::app::{KanalibApp, push_toast};
use crate::format::{
    is_animation_container, is_remote_asset, open_asset_with_system, video_fact_rows, video_meta,
};
use crate::theme::*;
use crate::*;
use kanalib_core::color::{self, PreviewOutcome};

pub(crate) const VIEWER_CONTEXT: &str = "KanalibViewer";

/// F-074：查看器源模式。默认原图直读；快速预览用缩略图；色彩管理显式生成 sRGB 预览。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewerSourceMode {
    Original,
    QuickPreview,
    ColorManaged,
}

/// On-demand sRGB preview for the viewer's "original" mode. Previews are
/// per-image, outside the thumbnail budget, and released when the viewer
/// switches image or leaves original mode.
#[derive(Clone, Debug)]
struct PendingPreview {
    target: PathBuf,
    epoch: u64,
}

#[derive(Clone, Debug)]
enum ViewerPreview {
    Pending(PendingPreview),
    Ready(PathBuf),
    /// Source is sRGB (or untagged); the original file can be shown directly.
    Passthrough,
    Failed,
}

fn viewer_previews() -> &'static std::sync::Mutex<HashMap<PathBuf, ViewerPreview>> {
    static PREVIEWS: std::sync::OnceLock<std::sync::Mutex<HashMap<PathBuf, ViewerPreview>>> =
        std::sync::OnceLock::new();
    PREVIEWS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn next_preview_epoch() -> u64 {
    static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn preview_state(path: &Path) -> Option<ViewerPreview> {
    viewer_previews().lock().unwrap().get(path).cloned()
}

fn viewer_preview_target(path: &Path, cache_dir: &Path) -> Option<PathBuf> {
    let metadata = std::fs::metadata(path).ok()?;
    let bytes = metadata.len();
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    Some(color::preview_target(path, cache_dir, mtime, bytes))
}

fn release_viewer_preview(path: &Path, cx: &mut App) {
    let state = viewer_previews().lock().unwrap().remove(path);
    let target = match state {
        Some(ViewerPreview::Pending(pending)) => Some(pending.target),
        Some(ViewerPreview::Ready(target)) => Some(target),
        _ => None,
    };
    if let Some(target) = target {
        let resource = Resource::from(target.clone());
        cx.remove_asset::<gpui::ImageAssetLoader>(&resource);
        cx.remove_asset::<gpui::ImgResourceLoader>(&resource);
        let _ = std::fs::remove_file(&target);
    }
}

pub(crate) fn viewer_caption(asset: &Asset) -> SharedString {
    let general = asset
        .all_tags()
        .filter(|tag| matches!(tag.category.as_str(), "general" | "manual"))
        .count();
    let mut parts = vec![asset.title()];
    if asset.is_video() {
        let meta = video_meta(&asset.metadata_json);
        parts.push("视频".into());
        for (_, value) in video_fact_rows(&meta, asset.width, asset.height) {
            parts.push(value);
        }
    } else {
        parts.push(format!("{} × {}", asset.width, asset.height));
    }
    if is_remote_asset(&asset.path) {
        parts.push("远端只读".into());
    }
    if asset.favorite {
        parts.push("收藏".into());
    }
    if asset.star > 0 {
        parts.push(format!("{} 星", asset.star));
    }
    parts.push(format!("{general} 个标签"));
    SharedString::from(parts.join(" · "))
}

impl KanalibApp {
    pub(crate) fn open_viewer_at(&mut self, path: &Path) {
        let (slides, at) = {
            let indices = self.current_visible_indices();
            let shared = self.state.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            let mut slides = Vec::new();
            let mut at = None;
            for &index in indices.iter() {
                let Some(asset) = assets.get(index) else {
                    continue;
                };
                if asset.path.as_path() == path {
                    at = Some(slides.len());
                }
                slides.push((asset.path.clone(), viewer_caption(asset)));
            }
            if at.is_none()
                && let Some(asset) = assets.iter().find(|asset| asset.path.as_path() == path)
            {
                slides = vec![(asset.path.clone(), viewer_caption(asset))];
                at = Some(0);
            }
            (slides, at)
        };
        if let Some(at) = at {
            self.palette_open = false;
            // 相似结果视图里快速查看后按 Esc 要回到结果，而不是跳回图库。
            if self.screen != Screen::Similar {
                self.screen = Screen::Library;
            }
            self.viewer_viewed.clear();
            if let Some((path, _)) = slides.get(at) {
                self.viewer_viewed.insert(path.clone());
            }
            self.viewer_slides = slides;
            self.viewer_at = Some(at);
            self.focus_viewer_pending = true;
            self.viewer_source_mode = ViewerSourceMode::Original;
            self.viewer_origin_filter = Some(self.state.shared.lock().unwrap().filter.clone());
            self.viewer_origin_search = self.last_search.clone();
        }
    }

    pub(crate) fn remove_viewer_asset(&self, path: &Path, cx: &mut App) {
        let thumb = {
            let shared = self.state.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            assets
                .iter()
                .find(|asset| asset.path == path)
                .and_then(|asset| asset.thumb.clone().or_else(|| asset.tile_thumb.clone()))
        };
        for resource in std::iter::once(path.to_path_buf()).chain(thumb) {
            let resource = Resource::from(resource);
            cx.remove_asset::<gpui::ImageAssetLoader>(&resource);
            cx.remove_asset::<gpui::ImgResourceLoader>(&resource);
        }
        release_viewer_preview(path, cx);
    }

    /// Start the background sRGB conversion for an original-mode image. Until
    /// it finishes the viewer keeps showing the thumbnail, so the UI thread
    /// never decodes or converts a full-resolution image.
    pub(crate) fn request_viewer_preview(&mut self, path: &Path, cx: &mut Context<Self>) {
        if viewer_previews().lock().unwrap().contains_key(path) {
            return;
        }
        let Some(target) = viewer_preview_target(path, &self.state.cache_dir()) else {
            log::warn!("viewer preview: cannot stat {}", path.display());
            return;
        };
        let epoch = next_preview_epoch();
        viewer_previews().lock().unwrap().insert(
            path.to_path_buf(),
            ViewerPreview::Pending(PendingPreview {
                target: target.clone(),
                epoch,
            }),
        );
        let source = path.to_path_buf();
        let task_source = source.clone();
        let task_target = target.clone();
        let task = cx
            .background_executor()
            .spawn(async move { color::prepare_preview(&task_source, &task_target) });
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = task.await.map_err(|error| format!("{error:#}"));
            let _ = weak.update(cx, |app, cx| {
                app.finish_viewer_preview(&source, epoch, &target, result, cx)
            });
        })
        .detach();
    }

    fn finish_viewer_preview(
        &mut self,
        source: &Path,
        epoch: u64,
        target: &Path,
        result: Result<PreviewOutcome, String>,
        cx: &mut Context<Self>,
    ) {
        let pending = {
            let previews = viewer_previews().lock().unwrap();
            match previews.get(source) {
                Some(ViewerPreview::Pending(pending)) if pending.epoch == epoch => pending.clone(),
                Some(ViewerPreview::Pending(_)) => return,
                Some(_) => return,
                None => {
                    // Released while the background task was running; remove
                    // anything it managed to write.
                    if matches!(result, Ok(PreviewOutcome::Converted)) {
                        let _ = std::fs::remove_file(target);
                    }
                    return;
                }
            }
        };
        let wanted = self.viewer_source_mode == ViewerSourceMode::ColorManaged
            && self
                .viewer_at
                .and_then(|at| self.viewer_slides.get(at))
                .is_some_and(|(path, _)| path == source);
        match result {
            Ok(PreviewOutcome::Unchanged) => {
                if wanted {
                    viewer_previews()
                        .lock()
                        .unwrap()
                        .insert(source.to_path_buf(), ViewerPreview::Passthrough);
                    cx.notify();
                } else {
                    viewer_previews().lock().unwrap().remove(source);
                }
            }
            Ok(PreviewOutcome::Converted) => {
                if wanted && pending.target.exists() {
                    viewer_previews().lock().unwrap().insert(
                        source.to_path_buf(),
                        ViewerPreview::Ready(pending.target.clone()),
                    );
                    cx.notify();
                } else {
                    viewer_previews().lock().unwrap().remove(source);
                    let _ = std::fs::remove_file(&pending.target);
                }
            }
            Err(error) => {
                log::warn!("viewer preview {}: {error}", source.display());
                if wanted {
                    viewer_previews()
                        .lock()
                        .unwrap()
                        .insert(source.to_path_buf(), ViewerPreview::Failed);
                    push_toast(
                        &self.toaster,
                        cx,
                        "无法生成 sRGB 预览，已显示缩略图",
                        Severity::Warning,
                    );
                    cx.notify();
                } else {
                    viewer_previews().lock().unwrap().remove(source);
                    let _ = std::fs::remove_file(&pending.target);
                }
            }
        }
    }

    pub(crate) fn close_viewer(&mut self, cx: &mut Context<Self>) {
        let paths = self.take_viewer_cleanup();
        self.focus_grid_pending = true;
        for path in paths {
            self.remove_viewer_asset(&path, cx);
        }
        cx.notify();
    }

    pub(crate) fn set_viewer_at(&mut self, at: usize, cx: &mut Context<Self>) {
        let count = self.viewer_slides.len();
        if count == 0 || at >= count {
            return;
        }
        let previous = self
            .viewer_at
            .and_then(|previous| self.viewer_slides.get(previous))
            .map(|(path, _)| path.clone());
        self.viewer_at = Some(at);
        if let Some((path, _)) = self.viewer_slides.get(at) {
            let path = path.clone();
            self.viewer_viewed.insert(path.clone());
            self.selection = Arc::new(HashSet::from([path.clone()]));
            self.selection_anchor = Some(path.clone());
            self.state.select(Some(path));
        }
        if let Some(previous) = previous
            && self
                .viewer_at
                .and_then(|at| self.viewer_slides.get(at))
                .is_none_or(|(path, _)| path != &previous)
        {
            self.remove_viewer_asset(&previous, cx);
        }
        if self.viewer_source_mode == ViewerSourceMode::ColorManaged
            && let Some((path, _)) = self.viewer_slides.get(at)
        {
            let path = path.clone();
            if !kanalib_core::model::is_video(&path) && !is_remote_asset(&path) {
                self.request_viewer_preview(&path, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn step_viewer(&mut self, delta: isize, wrap: bool, cx: &mut Context<Self>) {
        let count = self.viewer_slides.len();
        if count == 0 {
            return;
        }
        let current = self.viewer_at.unwrap_or(0) as isize;
        let target = if wrap {
            (current + delta).rem_euclid(count as isize) as usize
        } else {
            (current + delta).clamp(0, count as isize - 1) as usize
        };
        if Some(target) != self.viewer_at {
            self.set_viewer_at(target, cx);
        }
    }

    pub(crate) fn set_viewer_source_mode(
        &mut self,
        mode: ViewerSourceMode,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self
            .viewer_at
            .and_then(|at| self.viewer_slides.get(at))
            .map(|(path, _)| path.clone())
        else {
            return;
        };
        // 视频只显示首帧；远端虚拟路径没有本地原图，只能快速预览。
        if (kanalib_core::model::is_video(&path) || is_remote_asset(&path))
            && mode != ViewerSourceMode::QuickPreview
        {
            return;
        }
        if self.viewer_source_mode == mode {
            return;
        }
        self.viewer_source_mode = mode;
        self.remove_viewer_asset(&path, cx);
        if mode == ViewerSourceMode::ColorManaged {
            self.request_viewer_preview(&path, cx);
        }
        cx.notify();
    }

    pub(crate) fn open_quick_view(&mut self, cx: &mut Context<Self>) {
        let path = self
            .state
            .shared
            .lock()
            .unwrap()
            .selected
            .clone()
            .or_else(|| self.selection.iter().next().cloned())
            .or_else(|| self.visible_paths().first().cloned());
        match path {
            Some(path) => {
                self.open_viewer_at(&path);
                cx.notify();
            }
            None => push_toast(&self.toaster, cx, "没有可查看的图片", Severity::Warning),
        }
    }

    pub(crate) fn take_viewer_cleanup(&mut self) -> Vec<PathBuf> {
        if let Some(at) = self.viewer_at
            && let Some((path, _)) = self.viewer_slides.get(at)
        {
            self.viewer_viewed.insert(path.clone());
        }
        self.viewer_at = None;
        self.viewer_slides.clear();
        self.viewer_origin_filter = None;
        self.viewer_origin_search.clear();
        self.viewer_viewed.drain().collect()
    }

    pub(crate) fn render_viewer(
        &self,
        this: &Entity<Self>,
        at: usize,
        colors: UiColors,
    ) -> Option<AnyElement> {
        let (path, caption) = self.viewer_slides.get(at)?.clone();
        let (is_video, remote, thumb) = {
            let shared = self.state.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            match assets.iter().find(|asset| asset.path == path) {
                Some(asset) => (
                    asset.is_video(),
                    is_remote_asset(&asset.path),
                    asset.thumb.clone().or_else(|| asset.tile_thumb.clone()),
                ),
                None => (
                    kanalib_core::model::is_video(&path),
                    is_remote_asset(&path),
                    None,
                ),
            }
        };
        // 视频固定首帧；远端虚拟路径没有本地原图，只能快速预览。
        let can_original = !is_video && !remote;
        let mode = if can_original {
            self.viewer_source_mode
        } else {
            ViewerSourceMode::QuickPreview
        };
        // 色彩管理是显式模式：等待/失败期间继续用缩略图；原图模式不经过预览管线。
        let preview = (mode == ViewerSourceMode::ColorManaged)
            .then(|| preview_state(&path))
            .flatten();
        let source = match mode {
            ViewerSourceMode::Original => Some(path.clone()),
            ViewerSourceMode::QuickPreview => thumb.clone(),
            ViewerSourceMode::ColorManaged => match &preview {
                Some(ViewerPreview::Ready(preview)) => Some(preview.clone()),
                Some(ViewerPreview::Passthrough) => Some(path.clone()),
                _ => thumb.clone(),
            },
        };
        let count = self.viewer_slides.len();
        let animated = is_animation_container(&path);
        let external_path = path.clone();
        let external_toaster = self.toaster.clone();
        let external_state = self.state.clone();
        let can_external =
            (is_video || animated) && !remote && kanalib_core::open::can_open_externally(&path);
        let external_label = if is_video {
            "用系统播放器打开"
        } else {
            "用系统应用播放"
        };
        let mut hint_parts = vec![
            "← → 切图",
            "0 适应 / 1 100%",
            "+/- 缩放",
            "Ctrl+滚轮",
            "拖拽平移",
            "R 旋转",
            "F 全屏",
        ];
        if is_video {
            hint_parts.push("视频仅显示首帧，播放请用系统播放器");
        } else if animated {
            hint_parts.push("动画仅首帧，播放请用系统应用");
        }
        if remote {
            hint_parts.push("远端只读 · 未下载原图");
        }
        match mode {
            ViewerSourceMode::Original => hint_parts.push("原图直读（未做色彩转换）"),
            ViewerSourceMode::QuickPreview if can_original => {
                hint_parts.push("快速预览 · 缩略图");
            }
            ViewerSourceMode::ColorManaged => match &preview {
                Some(ViewerPreview::Ready(_)) => hint_parts.push("色彩管理 · 已转换到 sRGB"),
                Some(ViewerPreview::Pending(_)) => hint_parts.push("正在生成色彩管理预览"),
                Some(ViewerPreview::Failed) => hint_parts.push("色彩管理预览失败 · 显示缩略图"),
                Some(ViewerPreview::Passthrough) => hint_parts.push("色彩管理 · 原图为 sRGB"),
                None => {}
            },
            ViewerSourceMode::QuickPreview => {}
        }
        let hint = hint_parts.join(" · ");
        let empty_label = if mode == ViewerSourceMode::Original {
            "无法加载原图"
        } else if mode == ViewerSourceMode::ColorManaged {
            match &preview {
                Some(ViewerPreview::Failed) => "无法生成色彩管理预览",
                _ => "正在生成色彩管理预览…",
            }
        } else if is_video {
            "暂无视频首帧（生成首帧需要 FFmpeg）"
        } else {
            "远端资产未下载原图，暂无预览"
        };
        let preview: AnyElement = match source {
            Some(source) => ImageViewer::new("kanalib-viewer", source).into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(colors.subtle)
                .child(empty_label)
                .into_any_element(),
        };
        let title = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let close = this.clone();
        let prev = this.clone();
        let next = this.clone();
        let prev_disabled = at == 0;
        let next_disabled = at + 1 >= count;
        let root = div()
            .id("kanalib-viewer-inline")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.fg)
            .key_context(VIEWER_CONTEXT)
            .track_focus(&self.viewer_focus)
            .on_action(move |_: &ViewerClose, _, cx| {
                let _ = close.update(cx, |app, cx| app.close_viewer(cx));
            })
            .on_action(move |_: &ViewerPrev, _, cx| {
                let _ = prev.update(cx, |app, cx| app.step_viewer(-1, false, cx));
            })
            .on_action(move |_: &ViewerNext, _, cx| {
                let _ = next.update(cx, |app, cx| app.step_viewer(1, false, cx));
            })
            .on_action(move |_: &ViewerFullscreen, window, _| {
                window.toggle_fullscreen();
            });
        let back_button = this.clone();
        let close_button = this.clone();
        let source_button = this.clone();
        let prev_button = this.clone();
        let next_button = this.clone();
        Some(
            root.child(
                div()
                    .flex_none()
                    .h(px(VIEWER_HEADER_HEIGHT))
                    .px(px(SPACE_3))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(SPACE_2))
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .min_w(px(0.))
                            .flex()
                            .items_center()
                            .gap(px(SPACE_2))
                            .child(
                                Button::new(
                                    "viewer-back",
                                    if self.screen == Screen::Similar {
                                        "返回结果"
                                    } else {
                                        "返回图库"
                                    },
                                )
                                .icon(IconName::ChevronLeft)
                                .size(ControlSize::Sm)
                                .variant(ButtonVariant::Ghost)
                                .on_click(move |_, _, cx| {
                                    let _ = back_button.update(cx, |app, cx| app.close_viewer(cx));
                                }),
                            )
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(SPACE_2))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.subtle)
                                    .child(format!("{} / {count}", at + 1)),
                            )
                            .children(can_original.then(|| {
                                SegmentedControl::new(
                                    "viewer-source",
                                    match mode {
                                        ViewerSourceMode::Original => "original",
                                        ViewerSourceMode::QuickPreview => "quick",
                                        ViewerSourceMode::ColorManaged => "color",
                                    },
                                )
                                .size(ControlSize::Sm)
                                .segment("original", "原图", None)
                                .segment("quick", "快速预览", None)
                                .segment("color", "色彩管理", None)
                                .on_change(move |value, _, cx| {
                                    let mode = match value.as_ref() {
                                        "quick" => ViewerSourceMode::QuickPreview,
                                        "color" => ViewerSourceMode::ColorManaged,
                                        _ => ViewerSourceMode::Original,
                                    };
                                    let _ = source_button.update(cx, |app, cx| {
                                        app.set_viewer_source_mode(mode, cx);
                                    });
                                })
                            }))
                            .children(can_external.then(|| {
                                Button::new("viewer-open-external", external_label)
                                    .size(ControlSize::Sm)
                                    .variant(ButtonVariant::Ghost)
                                    .icon(IconName::Play)
                                    .on_click(move |_, _, cx| {
                                        if let Err(error) = open_asset_with_system(&external_path) {
                                            // F-073 6.2：toast 之外还要有常驻的就地说明。
                                            external_state.set_ui_error(format!(
                                                "用系统应用打开失败：{error}"
                                            ));
                                            push_toast(
                                                &external_toaster,
                                                cx,
                                                error,
                                                Severity::Danger,
                                            );
                                        }
                                    })
                            }))
                            .child(
                                IconButton::new("viewer-fullscreen", IconName::Fullscreen)
                                    .size(ControlSize::Sm)
                                    .tooltip("全屏 (F)")
                                    .on_click(move |_, window, _| {
                                        window.toggle_fullscreen();
                                    }),
                            )
                            .child(
                                IconButton::new("viewer-close", IconName::X)
                                    .size(ControlSize::Sm)
                                    .tooltip("关闭 (Esc)")
                                    .on_click(move |_, _, cx| {
                                        let _ =
                                            close_button.update(cx, |app, cx| app.close_viewer(cx));
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .p(px(SPACE_3))
                    .child(preview),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(SPACE_3))
                    .py(px(SPACE_2))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(SPACE_2))
                    .border_t_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .min_w(px(0.))
                            .flex()
                            .items_center()
                            .gap(px(SPACE_1_5))
                            .child(
                                IconButton::new("viewer-prev", IconName::ChevronLeft)
                                    .size(ControlSize::Sm)
                                    .tooltip("上一张 (←)")
                                    .disabled(prev_disabled)
                                    .on_click(move |_, _, cx| {
                                        let _ = prev_button
                                            .update(cx, |app, cx| app.step_viewer(-1, false, cx));
                                    }),
                            )
                            .child(
                                IconButton::new("viewer-next", IconName::ChevronRight)
                                    .size(ControlSize::Sm)
                                    .tooltip("下一张 (→)")
                                    .disabled(next_disabled)
                                    .on_click(move |_, _, cx| {
                                        let _ = next_button
                                            .update(cx, |app, cx| app.step_viewer(1, false, cx));
                                    }),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(colors.muted)
                                    .child(caption),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_xs()
                            .text_right()
                            .text_color(colors.subtle)
                            .child(hint),
                    ),
            )
            .into_any_element(),
        )
    }
}
