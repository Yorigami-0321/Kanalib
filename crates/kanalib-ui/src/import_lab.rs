use crate::app::KanalibApp;
use crate::theme::*;
use crate::*;
use ely_gpui_component::motion::ProgressBar;
use kanalib_core::model::Asset;

const LAB_TILE_WIDTH: f32 = 188.;
const LAB_IMAGE_HEIGHT: f32 = 118.;
const LAB_FOOTER_HEIGHT: f32 = 58.;
const LAB_MAX_TILES: usize = 60;

impl KanalibApp {
    /// 冷导入灯箱：进度 + 本次扫描新增图片的实时缩略图流，可边导入边打分/收藏/定位。
    pub(crate) fn render_import_lab(
        &self,
        this: &Entity<Self>,
        colors: UiColors,
        cx: &App,
    ) -> AnyElement {
        let (scanning, progress, summary, imports, source_aliases, revision) = {
            let shared = self.state.shared.lock().unwrap();
            (
                shared.scanning,
                shared.scan_progress.clone(),
                shared.scan_summary.clone(),
                shared.scan_imports.clone(),
                shared.source_aliases.clone(),
                shared.revision,
            )
        };

        let close_this = this.clone();
        let close = move |_: &mut Window, cx: &mut App| {
            let _ = close_this.update(cx, |app, cx| {
                app.import_lab_open = false;
                cx.notify();
            });
        };

        let detail = if scanning {
            let mut parts = vec![crate::status_bar::scan_progress_counts(cx, &progress)];
            if progress.throughput > 0.0 {
                parts.push(
                    crate::localization::t_args(
                        cx,
                        "scan.throughput",
                        &[("rate", &format!("{:.1}", progress.throughput))],
                    )
                    .to_string(),
                );
            }
            if let Some(eta) = progress.eta_secs {
                parts.push(
                    crate::localization::t_args(
                        cx,
                        "scan.eta",
                        &[("eta", &crate::settings::format_tag_eta(Some(eta)))],
                    )
                    .to_string(),
                );
            }
            if let Some(source) = &progress.source {
                let source = crate::sidebar::source_label(source, &source_aliases);
                let key = if progress.source_total > 1 {
                    "scan.source_multi"
                } else {
                    "scan.source"
                };
                parts.push(
                    crate::localization::t_args(
                        cx,
                        key,
                        &[
                            ("index", &progress.source_index.to_string()),
                            ("total", &progress.source_total.to_string()),
                            ("source", &source),
                        ],
                    )
                    .to_string(),
                );
            }
            parts.join(" · ")
        } else if let Some(summary) = &summary {
            crate::status_bar::format_scan_summary(cx, summary, false)
        } else {
            crate::localization::t(cx, "import_lab.idle_detail").to_string()
        };

        let ratio = if progress.total > 0 {
            Some((progress.done as f32 / progress.total as f32).clamp(0.0, 1.0))
        } else {
            None
        };
        let bar = if scanning {
            match ratio {
                Some(value) => ProgressBar::new("import-lab-progress", value),
                None => ProgressBar::indeterminate("import-lab-progress"),
            }
        } else {
            ProgressBar::new("import-lab-progress", ratio.unwrap_or(0.0))
        };

        let new_count = crate::localization::t_args(
            cx,
            "import_lab.new_count",
            &[("n", &imports.len().to_string())],
        );
        let heading_row = div().flex().items_center().gap(px(SPACE_2)).child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.fg)
                .child(new_count),
        );
        let progress_section = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_2))
            .child(bar)
            .child(heading_row);

        let recent = self.recent_import_assets(&imports, scanning, revision);
        let stream: AnyElement = if recent.is_empty() {
            let message = if scanning {
                crate::localization::t(cx, "import_lab.waiting")
            } else {
                crate::localization::t(cx, "import_lab.empty_done")
            };
            div()
                .flex_none()
                .w_full()
                .min_h(px(220.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(SPACE_2))
                .rounded(px(RADIUS_CARD))
                .border_1()
                .border_color(colors.border)
                .bg(colors.bg)
                .child(
                    Icon::new(IconName::Image)
                        .size(IconSize::Lg)
                        .color(colors.subtle),
                )
                .child(div().text_sm().text_color(colors.muted).child(message))
                .into_any_element()
        } else {
            let mut grid = div()
                .flex_none()
                .w_full()
                .flex()
                .flex_wrap()
                .gap(px(SPACE_3));
            for asset in &recent {
                grid = grid.child(import_tile(asset, this, &self.thumb_cache, colors, cx));
            }
            grid.into_any_element()
        };

        let stop_label = crate::localization::t(cx, "import_lab.stop");
        let close_label = crate::localization::t(
            cx,
            if scanning {
                "import_lab.background"
            } else {
                "import_lab.browse"
            },
        );
        let mut dialog = Dialog::new(
            "import-lab",
            crate::localization::t(cx, "import_lab.title"),
            close,
        )
        .fullscreen()
        .detail(detail)
        .child(progress_section)
        .child(stream);
        if scanning {
            let stop_this = this.clone();
            dialog = dialog.action(move |_| {
                Button::new("import-lab-stop", stop_label.clone())
                    .variant(ButtonVariant::Danger)
                    .on_click(move |_, _, cx| {
                        let _ = stop_this.update(cx, |app, cx| {
                            app.state.stop();
                            cx.notify();
                        });
                    })
            });
        }
        let browse_this = this.clone();
        let browsing = !scanning;
        dialog
            .action(move |close| {
                let browse_this = browse_this.clone();
                Button::new("import-lab-close", close_label.clone())
                    .primary()
                    .on_click(move |_, window, cx| {
                        if browsing {
                            let _ = browse_this.update(cx, |app, cx| {
                                app.screen = Screen::Library;
                                app.tag_return_pending = false;
                                app.dedup_confirm = None;
                                cx.notify();
                            });
                        }
                        close(window, cx)
                    })
            })
            .into_any_element()
    }

    /// 把本次扫描的路径映射成资产快照（新的在前），并限制灯箱一次渲染的瓦片数。
    ///
    /// 扫描期间每个来源的新资产先追加到 `assets` 尾部（排序在该来源收尾才发生），因此
    /// 只读尾部 `imports.len()` 条再按导入集过滤；多来源时较早来源的导入可能已散开，
    /// 灯箱优先展示当前来源的最新流，扫描结束后再按全量映射补齐。扫描结束后资产已
    /// 排序，改为按 `revision` 缓存一次全库映射，避免灯箱每帧对十万级图库做全量索引。
    fn recent_import_assets(
        &self,
        imports: &[PathBuf],
        scanning: bool,
        revision: u64,
    ) -> Vec<Asset> {
        if imports.is_empty() {
            return Vec::new();
        }
        let assets = self.state.shared.lock().unwrap().assets.clone();
        let assets = assets.read().unwrap();
        if scanning {
            let wanted = imports
                .iter()
                .map(|path| path.as_path())
                .collect::<HashSet<_>>();
            let start = assets.len().saturating_sub(imports.len());
            return assets[start..]
                .iter()
                .rev()
                .filter(|asset| wanted.contains(asset.path.as_path()))
                .take(LAB_MAX_TILES)
                .cloned()
                .collect();
        }
        if let Some((cached_revision, cached)) = self.import_lab_cache.borrow().as_ref()
            && *cached_revision == revision
        {
            return cached.clone();
        }
        let by_path = assets
            .iter()
            .map(|asset| (asset.path.as_path(), asset))
            .collect::<HashMap<_, _>>();
        let recent = imports
            .iter()
            .rev()
            .filter_map(|path| by_path.get(path.as_path()).map(|asset| (*asset).clone()))
            .take(LAB_MAX_TILES)
            .collect::<Vec<_>>();
        *self.import_lab_cache.borrow_mut() = Some((revision, recent.clone()));
        recent
    }
}

fn import_tile(
    asset: &Asset,
    this: &Entity<KanalibApp>,
    thumb_cache: &Entity<ThumbCache>,
    colors: UiColors,
    cx: &App,
) -> AnyElement {
    let path = asset.path.clone();
    let title = asset.title();
    let thumb = asset.thumb.clone().or_else(|| asset.tile_thumb.clone());
    let media: AnyElement = match thumb {
        Some(thumb) => img(thumb)
            .image_cache(thumb_cache)
            .w_full()
            .h_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        None => div()
            .w_full()
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                Icon::new(IconName::ImageOff)
                    .size(IconSize::Lg)
                    .color(colors.subtle),
            )
            .into_any_element(),
    };

    let video_badge = asset.is_video().then(|| {
        let label = crate::format::video_meta(&asset.metadata_json)
            .duration_secs
            .map(crate::format::format_duration)
            .unwrap_or_else(|| crate::localization::t(cx, "views.tile.video").to_string());
        div()
            .absolute()
            .left(px(6.))
            .bottom(px(6.))
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(6.))
            .py(px(2.))
            .rounded(px(RADIUS_SM))
            .bg(colors.overlay)
            .child(
                Icon::new(IconName::Play)
                    .size(IconSize::Xs)
                    .color(colors.info),
            )
            .child(div().text_xs().text_color(colors.fg).child(label))
    });

    let favorite = asset.favorite;
    let favorite_this = this.clone();
    let favorite_path = path.clone();
    let favorite_label = crate::localization::t(
        cx,
        if favorite {
            "views.menu.unfavorite"
        } else {
            "import_lab.favorite"
        },
    );
    let favorite_button = div()
        .id(SharedString::from(format!("import-fav-{}", path.display())))
        .role(Role::Button)
        .aria_label(favorite_label.clone())
        .tab_index(0)
        .focus(move |style| style.bg(colors.hover))
        .tooltip(Tooltip::text(favorite_label))
        .absolute()
        .top(px(6.))
        .right(px(6.))
        .size(px(24.))
        .rounded_full()
        .bg(colors.backdrop)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            let _ = favorite_this.update(cx, |app, cx| {
                app.toggle_imported_favorite(favorite_path.clone(), !favorite, cx)
            });
        })
        .child(
            Icon::new(IconName::Heart)
                .size(IconSize::Sm)
                .color(if favorite { colors.danger } else { colors.fg }),
        );

    let reveal_this = this.clone();
    let reveal_path = path.clone();
    let reveal_label = crate::localization::t(cx, "import_lab.reveal");
    let image = div()
        .id(SharedString::from(format!(
            "import-tile-{}",
            path.display()
        )))
        .role(Role::Button)
        .aria_label(title.clone())
        .tab_index(0)
        .focus(move |style| style.bg(colors.hover))
        .tooltip(Tooltip::text(reveal_label))
        .relative()
        .w_full()
        .h(px(LAB_IMAGE_HEIGHT))
        .flex_none()
        .overflow_hidden()
        .bg(colors.sunken)
        .cursor_pointer()
        .child(media)
        .children(video_badge)
        .child(favorite_button)
        .on_click(move |_, _, cx| {
            let _ = reveal_this.update(cx, |app, cx| {
                app.focus_imported_asset(reveal_path.clone(), cx)
            });
        });

    let rate_this = this.clone();
    let rate_path = path.clone();
    let rating = Rating::new(
        SharedString::from(format!("import-star-{}", path.display())),
        asset.star,
    )
    .label(crate::localization::t(cx, "import_lab.rate"))
    .on_change(move |value, _, cx| {
        let _ = rate_this.update(cx, |app, cx| {
            app.rate_imported_asset(rate_path.clone(), value, cx)
        });
    });

    let meta = if asset.width > 0 && asset.height > 0 {
        format!("{}×{}", asset.width, asset.height)
    } else {
        crate::inspector::friendly_format_label(&asset.path)
    };

    div()
        .flex_none()
        .w(px(LAB_TILE_WIDTH))
        .flex()
        .flex_col()
        .rounded(px(RADIUS_CARD))
        .overflow_hidden()
        .bg(colors.surface)
        .border_1()
        .border_color(colors.border)
        .hover(move |style| style.border_color(colors.border_strong))
        .child(image)
        .child(
            div()
                .h(px(LAB_FOOTER_HEIGHT))
                .w_full()
                .px(px(9.))
                .py(px(6.))
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(2.))
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.fg)
                        .child(title),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(4.))
                        .child(div().flex_none().child(rating))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_xs()
                                .text_color(colors.subtle)
                                .child(meta),
                        ),
                ),
        )
        .into_any_element()
}
