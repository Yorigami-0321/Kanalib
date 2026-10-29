use crate::*;
use crate::app::{KanalibApp, push_toast};
use crate::format::is_animation_container;
use crate::platform::open_path;
use crate::theme::*;

pub(crate) const VIEWER_CONTEXT: &str = "KanalibViewer";

pub(crate) fn viewer_caption(asset: &Asset) -> SharedString {
    let general = asset
        .all_tags()
        .filter(|tag| matches!(tag.category.as_str(), "general" | "manual"))
        .count();
    let mut parts = vec![asset.title(), format!("{} × {}", asset.width, asset.height)];
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
            let shared = self.state.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            let mut slides = Vec::new();
            let mut at = None;
            for &index in self.view.visible_indices.iter() {
                let Some(asset) = assets.get(index) else {
                    continue;
                };
                if asset.path.as_path() == path {
                    at = Some(slides.len());
                }
                slides.push((asset.path.clone(), viewer_caption(asset)));
            }
            if at.is_none() {
                if let Some(asset) = assets.iter().find(|asset| asset.path.as_path() == path) {
                    slides = vec![(asset.path.clone(), viewer_caption(asset))];
                    at = Some(0);
                }
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
            self.viewer_original = false;
            self.viewer_origin_filter = Some(
                self.state
                    .shared
                    .lock()
                    .unwrap()
                    .filter
                    .clone(),
            );
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

    pub(crate) fn toggle_viewer_source(&mut self, cx: &mut Context<Self>) {
        self.viewer_original = !self.viewer_original;
        if let Some(path) = self
            .viewer_at
            .and_then(|at| self.viewer_slides.get(at))
            .map(|(path, _)| path.clone())
        {
            self.remove_viewer_asset(&path, cx);
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
        if let Some(at) = self.viewer_at {
            if let Some((path, _)) = self.viewer_slides.get(at) {
                self.viewer_viewed.insert(path.clone());
            }
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
        let source = if self.viewer_original {
            path.clone()
        } else {
            let shared = self.state.shared.lock().unwrap();
            let assets = shared.assets.read().unwrap();
            assets
                .iter()
                .find(|asset| asset.path == path)
                .and_then(|asset| asset.thumb.clone().or_else(|| asset.tile_thumb.clone()))
                .unwrap_or_else(|| path.clone())
        };
        let count = self.viewer_slides.len();
        // 动画容器不内置播放器：只显示首帧，并给“系统应用播放”出口，避免假播放。
        let animated = is_animation_container(&path);
        let external_path = path.clone();
        let hint = if animated {
            "← → 切图 · +/- 缩放 · Ctrl+滚轮缩放 · 拖拽平移 · R 旋转 · F 全屏 · 动画仅首帧，播放请用系统应用"
        } else {
            "← → 切图 · +/- 缩放 · Ctrl+滚轮缩放 · 拖拽平移 · R 旋转 · F 全屏"
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
                    .h(px(48.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .min_w(px(0.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
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
                                    let _ =
                                        back_button.update(cx, |app, cx| app.close_viewer(cx));
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
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.subtle)
                                    .child(format!("{} / {count}", at + 1)),
                            )
                            .child(
                                Button::new(
                                    "viewer-source",
                                    if self.viewer_original { "预览" } else { "原图" },
                                )
                                .size(ControlSize::Sm)
                                .variant(ButtonVariant::Ghost)
                                .on_click(move |_, _, cx| {
                                    let _ = source_button
                                        .update(cx, |app, cx| app.toggle_viewer_source(cx));
                                }),
                            )
                            .children(animated.then(|| {
                                Button::new("viewer-open-external", "用系统应用播放")
                                    .size(ControlSize::Sm)
                                    .variant(ButtonVariant::Ghost)
                                    .icon(IconName::Play)
                                    .on_click(move |_, _, _| open_path(&external_path))
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
                                        let _ = close_button
                                            .update(cx, |app, cx| app.close_viewer(cx));
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .p(px(12.))
                    .child(ImageViewer::new("kanalib-viewer", source)),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.))
                    .border_t_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .min_w(px(0.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                IconButton::new("viewer-prev", IconName::ChevronLeft)
                                    .size(ControlSize::Sm)
                                    .tooltip("上一张 (←)")
                                    .disabled(prev_disabled)
                                    .on_click(move |_, _, cx| {
                                        let _ = prev_button.update(cx, |app, cx| {
                                            app.step_viewer(-1, false, cx)
                                        });
                                    }),
                            )
                            .child(
                                IconButton::new("viewer-next", IconName::ChevronRight)
                                    .size(ControlSize::Sm)
                                    .tooltip("下一张 (→)")
                                    .disabled(next_disabled)
                                    .on_click(move |_, _, cx| {
                                        let _ = next_button.update(cx, |app, cx| {
                                            app.step_viewer(1, false, cx)
                                        });
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
                            .flex_none()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(hint),
                    ),
            )
            .into_any_element(),
        )
    }

}
