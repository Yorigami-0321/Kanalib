use crate::app::{KanalibApp, UiRetryAction, push_toast};
use crate::components::*;
use crate::format::*;
use crate::platform::*;
use crate::theme::*;
use crate::thumb_cache::ThumbCache;
use crate::*;
use std::rc::Rc;

mod content;
mod details;
mod grid;
mod grouped;
mod list;

use self::content::content_row_content;
use self::details::{DetailColumns, detail_columns, details_header, details_row_content};
use self::grid::tile;
use self::grouped::{GroupLayout, GroupedList, HeaderRender, RowRender, group_header};
use self::list::list_row_content;

pub(crate) use self::grouped::{GroupCache, initial_group_mode};

pub(crate) const GRID_GAP: f32 = 12.;
pub(crate) const GRID_PADDING: f32 = 16.;
pub(crate) const ROW_PADDING: f32 = 12.;
pub(crate) const GRID_CONTEXT: &str = "KanalibGrid";

pub(crate) fn grid_columns(available: f32, min_width: f32) -> usize {
    let usable = (available - GRID_PADDING * 2.0 + GRID_GAP) / (min_width + GRID_GAP);
    (usable.floor() as usize).max(1)
}

pub(crate) fn grid_tile_width(available: f32, columns: usize) -> f32 {
    let usable = available - GRID_PADDING * 2.0 - GRID_GAP * columns.saturating_sub(1) as f32;
    (usable / columns.max(1) as f32).max(1.0)
}

pub(crate) fn row_thumb(
    asset: &Asset,
    thumb_cache: &Entity<ThumbCache>,
    width: f32,
    height: f32,
    colors: UiColors,
) -> AnyElement {
    let thumb = asset.tile_thumb.clone().or_else(|| asset.thumb.clone());
    match thumb {
        Some(path) => img(path)
            .image_cache(thumb_cache)
            .w(px(width))
            .h(px(height))
            .flex_none()
            .rounded(px(4.))
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        None => div()
            .w(px(width))
            .h(px(height))
            .flex_none()
            .rounded(px(4.))
            .bg(colors.sunken)
            .into_any_element(),
    }
}

pub(crate) fn asset_row_shell(
    asset: &Asset,
    selected: bool,
    this: Entity<KanalibApp>,
    focus: FocusHandle,
    colors: UiColors,
    height: f32,
    divider: bool,
    content: AnyElement,
    cx: &App,
) -> impl IntoElement {
    let path = asset.path.clone();
    let title = asset.title();
    let menu = asset_context_menu(asset, &this, cx);
    let this_click = this.clone();
    let path_click = path.clone();
    let focus_click = focus.clone();
    let this_right = this.clone();
    let path_right = path.clone();
    let focus_right = focus.clone();
    div()
        .id(SharedString::from(format!("asset-row-{}", path.display())))
        .role(Role::Button)
        .aria_label(title)
        .w_full()
        .h(px(height))
        .flex()
        .items_center()
        .px(px(GRID_PADDING))
        .cursor_pointer()
        .when(divider, |row| row.border_b_1().border_color(colors.border))
        .hover(move |style| style.bg(colors.hover))
        .when(selected, |row| row.bg(colors.selection))
        .on_click(move |event, window, cx| {
            window.focus(&focus_click, cx);
            let (modifiers, click_count) = match event {
                ClickEvent::Mouse(event) => (event.down.modifiers, event.down.click_count),
                ClickEvent::Keyboard(_) | ClickEvent::Touch(_) => (Modifiers::default(), 1),
            };
            let path = path_click.clone();
            let _ = this_click.update(cx, |app, cx| {
                app.handle_tile_click(path, modifiers, click_count, cx)
            });
        })
        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
            window.focus(&focus_right, cx);
            let _ = this_right.update(cx, |app, cx| {
                app.handle_tile_right_click(path_right.clone(), cx);
            });
        })
        .child(
            ContextMenu::new(
                SharedString::from(format!("row-context-{}", path.display())),
                menu,
            )
            .w_full()
            .child(div().w_full().child(content)),
        )
}

pub(crate) fn row_meta(asset: &Asset, cx: &App) -> String {
    let code = asset.rating.to_uppercase();
    let star = asset.star;
    let tag_count = asset
        .all_tags()
        .filter(|tag| matches!(tag.category.as_str(), "general" | "manual"))
        .count();
    let rating = if code.is_empty() {
        crate::localization::t(cx, "views.rating.none").to_string()
    } else {
        crate::localization::t_args(cx, "views.rating.value", &[("rating", &code)]).to_string()
    };
    let star = if star > 0 {
        format!(" · ★ {star}")
    } else {
        String::new()
    };
    let tags = crate::localization::t_args(cx, "views.tag_count", &[("n", &tag_count.to_string())]);
    format!("{rating}{star} · {tags}")
}

pub(crate) fn display_name(asset: &Asset, show_extensions: bool) -> String {
    if show_extensions {
        return asset.title();
    }
    asset
        .path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| asset.title())
}

pub(crate) fn asset_context_menu(asset: &Asset, this: &Entity<KanalibApp>, cx: &App) -> Menu {
    let path = asset.path.clone();
    let favorite = asset.favorite;
    let star_menu = {
        let mut menu = Menu::new();
        for (value, stars) in [(1u8, "1"), (2, "2"), (3, "3"), (4, "4"), (5, "5")] {
            let this = this.clone();
            let path = path.clone();
            menu = menu.item(
                MenuItem::new(crate::localization::t_args(
                    cx,
                    "views.star.value",
                    &[("n", stars)],
                ))
                .keys(value.to_string())
                .on_click(move |_, cx| {
                    let _ = this.update(cx, |app, cx| {
                        app.select_single_path(path.clone(), cx);
                        app.apply_star(value, cx);
                    });
                }),
            );
        }
        let this = this.clone();
        let path = path.clone();
        menu.separator().item(
            MenuItem::new(crate::localization::t(cx, "views.star.clear"))
                .keys("0")
                .on_click(move |_, cx| {
                    let _ = this.update(cx, |app, cx| {
                        app.select_single_path(path.clone(), cx);
                        app.apply_star(0, cx);
                    });
                }),
        )
    };
    let quick_this = this.clone();
    let quick_path = path.clone();
    let similar_this = this.clone();
    let similar_path = path.clone();
    let compare_this = this.clone();
    let favorite_this = this.clone();
    let favorite_path = path.clone();
    let prompt_this = this.clone();
    let prompt_path = path.clone();
    let paths_this = this.clone();
    let paths_path = path.clone();
    let image_this = this.clone();
    let image_path = path.clone();
    let open_file = path.clone();
    let reveal_file = path.clone();
    let open_this = this.clone();
    let reveal_this = this.clone();
    let video = asset.is_video();
    let remote = is_remote_asset(&path);
    let remote_reason = crate::localization::t(cx, "views.menu.remote_readonly");
    let open_label = if video {
        crate::localization::t(cx, "views.menu.open_video")
    } else {
        crate::localization::t(cx, "views.menu.open_file")
    };
    let open_icon = if video {
        IconName::Play
    } else {
        IconName::Image
    };
    Menu::new()
        .item(
            MenuItem::new(crate::localization::t(cx, "views.menu.quick_view"))
                .icon(IconName::Maximize2)
                .keys("enter")
                .on_click(move |_, cx| {
                    let _ = quick_this.update(cx, |app, cx| {
                        app.select_single_path(quick_path.clone(), cx);
                        app.open_quick_view(cx);
                    });
                }),
        )
        .item(
            MenuItem::new(crate::localization::t(cx, "views.menu.find_similar"))
                .icon(IconName::Search)
                .on_click(move |_, cx| {
                    let _ = similar_this.update(cx, |app, cx| {
                        app.open_similar(similar_path.clone(), cx);
                    });
                }),
        )
        .item(
            MenuItem::new(crate::localization::t(cx, "views.menu.compare"))
                .icon(IconName::Columns2)
                .on_click(move |_, cx| {
                    let _ = compare_this.update(cx, |app, cx| app.open_compare_selected(cx));
                }),
        )
        .item(
            MenuItem::new(if favorite {
                crate::localization::t(cx, "views.menu.unfavorite")
            } else {
                crate::localization::t(cx, "views.menu.favorite")
            })
            .icon(IconName::Heart)
            .keys("f")
            .on_click(move |_, cx| {
                let _ = favorite_this.update(cx, |app, cx| {
                    app.select_single_path(favorite_path.clone(), cx);
                    app.toggle_favorite(cx);
                });
            }),
        )
        .item(MenuItem::submenu(
            crate::localization::t(cx, "views.menu.stars"),
            star_menu,
        ))
        .separator()
        .item(
            MenuItem::new(crate::localization::t(cx, "views.menu.copy_prompt"))
                .icon(IconName::ClipboardCheck)
                .on_click(move |_, cx| {
                    let _ = prompt_this.update(cx, |app, cx| {
                        app.select_single_path(prompt_path.clone(), cx);
                        app.copy_selected_prompt(cx);
                    });
                }),
        )
        .item(
            MenuItem::new(crate::localization::t(cx, "views.menu.copy_path"))
                .icon(IconName::Copy)
                .on_click(move |_, cx| {
                    let _ = paths_this.update(cx, |app, cx| {
                        app.select_single_path(paths_path.clone(), cx);
                        app.copy_selected_paths(cx);
                    });
                }),
        )
        .item(
            MenuItem::new(crate::localization::t(cx, "views.menu.copy_image"))
                .icon(IconName::Image)
                .on_click(move |_, cx| {
                    let _ = image_this.update(cx, |app, cx| {
                        app.copy_image_path(image_path.clone(), cx);
                    });
                }),
        )
        .separator()
        .item(
            MenuItem::new(open_label)
                .icon(open_icon)
                .disabled(remote)
                .disabled_reason(remote_reason.clone())
                .on_click(move |_, cx| {
                    if let Err(error) = open_asset_with_system(&open_file) {
                        let message = crate::localization::t_args(
                            cx,
                            "views.menu.open_failed",
                            &[("error", &error)],
                        )
                        .to_string();
                        let retry_path = open_file.clone();
                        let _ = open_this.update(cx, |app, cx| {
                            // F-073 6.2：toast 之外还要有常驻的就地说明与重试。
                            app.set_ui_error_with_retry(
                                message,
                                UiRetryAction::OpenWithSystem(retry_path),
                                cx,
                            );
                            push_toast(&app.toaster, cx, error, Severity::Danger);
                        });
                    }
                }),
        )
        .item(
            MenuItem::new(crate::localization::t(cx, "views.menu.reveal"))
                .icon(IconName::FolderOpen)
                .disabled(remote)
                .disabled_reason(remote_reason)
                .on_click(move |_, cx| {
                    if !can_touch_local_file(&reveal_file) {
                        let _ = reveal_this.update(cx, |app, cx| {
                            push_toast(&app.toaster, cx, remote_readonly_note(), Severity::Warning);
                        });
                        return;
                    }
                    reveal_path(&reveal_file);
                }),
        )
}

impl KanalibApp {
    pub(crate) fn render_main(
        &mut self,
        this: &Entity<Self>,
        assets: Arc<RwLock<Vec<Asset>>>,
        visible_indices: Arc<Vec<usize>>,
        selection: Arc<HashSet<PathBuf>>,
        roots_empty: bool,
        error: Option<&str>,
        search_warnings: &[String],
        scanning: bool,
        width: Pixels,
        colors: UiColors,
        cx: &App,
    ) -> impl IntoElement {
        let star_menu = {
            let menu_item = |this: &Entity<Self>, star: u8, label: SharedString| {
                let this = this.clone();
                MenuItem::new(label).on_click(move |_, cx| {
                    let _ = this.update(cx, |app, cx| app.apply_star(star, cx));
                })
            };
            let star_value =
                |star: &str| crate::localization::t_args(cx, "views.star.value", &[("n", star)]);
            Menu::new()
                .item(menu_item(this, 1, star_value("1")))
                .item(menu_item(this, 2, star_value("2")))
                .item(menu_item(this, 3, star_value("3")))
                .item(menu_item(this, 4, star_value("4")))
                .item(menu_item(this, 5, star_value("5")))
                .separator()
                .item(menu_item(
                    this,
                    0,
                    crate::localization::t(cx, "views.star.clear"),
                ))
        };

        let tag_menu = {
            let add = this.clone();
            let remove = this.clone();
            Menu::new()
                .item(
                    MenuItem::new(crate::localization::t(cx, "views.tags.add"))
                        .icon(IconName::Tag)
                        .on_click(move |_, cx| {
                            let _ = add
                                .update(cx, |app, cx| app.open_tag_dialog(TagDialogMode::Add, cx));
                        }),
                )
                .item(
                    MenuItem::new(crate::localization::t(cx, "views.tags.remove"))
                        .icon(IconName::X)
                        .on_click(move |_, cx| {
                            let _ = remove.update(cx, |app, cx| {
                                app.open_tag_dialog(TagDialogMode::Remove, cx)
                            });
                        }),
                )
        };
        let export_menu = self.export_menu(this, cx);
        let is_grid = self.view_mode.is_grid();
        let columns = if is_grid {
            grid_columns(width.as_f32(), self.view_mode.min_width())
        } else {
            1
        };
        let tile_width = grid_tile_width(width.as_f32(), columns);
        let image_height = self.view_mode.image_height();
        let row_height = self.view_mode.row_height();
        // 分组只重排可见资产；返回的顺序同时用于渲染、框选与键盘导航。
        let visible_indices =
            self.refresh_group_view(visible_indices, columns, row_height, &assets);
        let group_layout = self.grouped_layout();
        let visible = visible_indices.len();
        let row_count = match &group_layout {
            Some(layout) => layout.total_rows,
            None if is_grid => visible.div_ceil(columns),
            None => visible,
        };
        let assets_for_list = assets;
        let rows_for_list = visible_indices;
        let detail_columns = detail_columns(width.as_f32());
        let view_mode = self.view_mode;
        let show_extensions = self.show_extensions;
        let sort_mode = self.sort;
        let sort_ascending = self.sort_ascending;
        let grid: AnyElement = if rows_for_list.is_empty() {
            let this_empty = this.clone();
            let action = (!roots_empty && !scanning).then(|| {
                Button::new(
                    "library-empty-reset",
                    crate::localization::t(cx, "views.empty.clear_filters"),
                )
                .size(ControlSize::Sm)
                .on_click(move |_, _, cx| {
                    let _ = this_empty.update(cx, |app, cx| {
                        app.apply_search_text(String::new(), cx);
                        app.set_filter(Filter::All, cx);
                    });
                })
                .into_any_element()
            });
            let (icon, title, description) = if roots_empty {
                (
                    IconName::FolderOpen,
                    crate::localization::t(cx, "views.empty.no_roots.title"),
                    crate::localization::t(cx, "views.empty.no_roots.description"),
                )
            } else if scanning {
                (
                    IconName::RefreshCw,
                    crate::localization::t(cx, "views.empty.scanning.title"),
                    crate::localization::t(cx, "views.empty.scanning.description"),
                )
            } else {
                (
                    IconName::SearchX,
                    crate::localization::t(cx, "views.empty.no_results.title"),
                    crate::localization::t(cx, "views.empty.no_results.description"),
                )
            };
            empty_state(icon, title, description, action, colors)
        } else if let Some(layout) = group_layout.clone() {
            self.render_grouped_list(
                this,
                layout,
                assets_for_list,
                rows_for_list,
                selection.clone(),
                view_mode,
                columns,
                tile_width,
                image_height,
                row_height,
                detail_columns,
                show_extensions,
                sort_mode,
                sort_ascending,
                colors,
                cx,
            )
        } else if is_grid {
            let this_for_list = this.clone();
            let thumb_cache_for_list = self.thumb_cache.clone();
            let focus_for_list = self.grid_focus.clone();
            let selection_for_list = selection.clone();
            let prefer_large = self.view_mode.prefers_large_thumb();
            uniform_list("asset-grid", row_count, move |range, _, cx| {
                let Ok(assets) = assets_for_list.read() else {
                    return Vec::new();
                };
                range
                    .map(|row| {
                        let start = row * columns;
                        let end = (start + columns).min(rows_for_list.len());
                        let mut line = div()
                            .w_full()
                            .h(px(row_height))
                            .flex()
                            .gap(px(GRID_GAP))
                            .px(px(GRID_PADDING))
                            .pt(px(ROW_PADDING))
                            .pb(px(ROW_PADDING));
                        for &asset_index in &rows_for_list[start..end] {
                            let Some(asset) = assets.get(asset_index) else {
                                continue;
                            };
                            let is_selected = selection_for_list.contains(&asset.path);
                            line = line.child(tile(
                                asset.clone(),
                                is_selected,
                                this_for_list.clone(),
                                thumb_cache_for_list.clone(),
                                tile_width,
                                image_height,
                                focus_for_list.clone(),
                                colors,
                                prefer_large,
                                show_extensions,
                                cx,
                            ));
                        }
                        line
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.grid_scroll)
            .flex_1()
            .w_full()
            .min_h(px(0.))
            .into_any_element()
        } else {
            let this_for_list = this.clone();
            let thumb_cache_for_list = self.thumb_cache.clone();
            let focus_for_list = self.grid_focus.clone();
            let selection_for_list = selection.clone();
            let list_id = match view_mode {
                ViewMode::Details => "asset-details",
                ViewMode::Content => "asset-content",
                _ => "asset-list",
            };
            let list = uniform_list(list_id, row_count, move |range, _, cx| {
                let Ok(assets) = assets_for_list.read() else {
                    return Vec::new();
                };
                range
                    .map(|position| {
                        let Some(asset) = assets.get(rows_for_list[position]) else {
                            return div().h(px(row_height)).into_any_element();
                        };
                        let is_selected = selection_for_list.contains(&asset.path);
                        let content = match view_mode {
                            ViewMode::Details => details_row_content(
                                asset,
                                detail_columns,
                                show_extensions,
                                &thumb_cache_for_list,
                                colors,
                                cx,
                            ),
                            ViewMode::Content => content_row_content(
                                asset,
                                show_extensions,
                                &thumb_cache_for_list,
                                colors,
                                cx,
                            ),
                            _ => list_row_content(
                                asset,
                                show_extensions,
                                &thumb_cache_for_list,
                                colors,
                                cx,
                            ),
                        };
                        asset_row_shell(
                            asset,
                            is_selected,
                            this_for_list.clone(),
                            focus_for_list.clone(),
                            colors,
                            row_height,
                            view_mode == ViewMode::Details,
                            content,
                            cx,
                        )
                        .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.grid_scroll)
            .flex_1()
            .w_full()
            .min_h(px(0.));
            if view_mode == ViewMode::Details {
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(details_header(
                        this.clone(),
                        detail_columns,
                        sort_mode,
                        sort_ascending,
                        colors,
                        cx,
                    ))
                    .child(list)
                    .into_any_element()
            } else {
                list.into_any_element()
            }
        };
        let grid_area = div()
            .id("library-grid")
            .role(Role::Grid)
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .flex()
            .flex_col()
            .track_focus(&self.grid_focus)
            .key_context(GRID_CONTEXT)
            .on_mouse_down(MouseButton::Left, {
                let focus = self.grid_focus.clone();
                move |_, window, cx| window.focus(&focus, cx)
            })
            .child(grid);

        let bulk = (selection.len() >= 2).then(|| {
            let this_fav = this.clone();
            let this_unfav = this.clone();
            let this_prompt = this.clone();
            let this_paths = this.clone();
            let this_bulk_clear = this.clone();
            div()
                .px(px(16.))
                .py(px(6.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .bg(colors.sunken)
                .border_b_1()
                .border_color(colors.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.fg)
                                .child(crate::localization::t_args(
                                    cx,
                                    "views.bulk.selected",
                                    &[("n", &selection.len().to_string())],
                                )),
                        )
                        .child(
                            Button::new(
                                "bulk-favorite",
                                crate::localization::t(cx, "views.menu.favorite"),
                            )
                            .size(ControlSize::Sm)
                            .icon(IconName::Heart)
                            .on_click(move |_, _, cx| {
                                let _ = this_fav.update(cx, |app, cx| app.apply_favorite(true, cx));
                            }),
                        )
                        .child(
                            Button::new(
                                "bulk-unfavorite",
                                crate::localization::t(cx, "views.menu.unfavorite"),
                            )
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ =
                                    this_unfav.update(cx, |app, cx| app.apply_favorite(false, cx));
                            }),
                        )
                        .child(
                            DropdownMenu::new(
                                "bulk-star",
                                crate::localization::t(cx, "views.menu.stars"),
                                star_menu,
                            )
                            .icon(IconName::Star)
                            .variant(ButtonVariant::Ghost),
                        )
                        .child(
                            DropdownMenu::new(
                                "bulk-tags",
                                crate::localization::t(cx, "views.bulk.tags"),
                                tag_menu,
                            )
                            .icon(IconName::Tag)
                            .variant(ButtonVariant::Ghost),
                        )
                        .child(
                            Button::new(
                                "bulk-copy-prompt",
                                crate::localization::t(cx, "views.menu.copy_prompt"),
                            )
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ =
                                    this_prompt.update(cx, |app, cx| app.copy_selected_prompt(cx));
                            }),
                        )
                        .child(
                            Button::new(
                                "bulk-copy-paths",
                                crate::localization::t(cx, "views.menu.copy_path"),
                            )
                            .size(ControlSize::Sm)
                            .on_click(move |_, _, cx| {
                                let _ =
                                    this_paths.update(cx, |app, cx| app.copy_selected_paths(cx));
                            }),
                        )
                        .child(
                            DropdownMenu::new(
                                "bulk-export",
                                crate::localization::t(cx, "titlebar.export"),
                                export_menu,
                            )
                            .icon(IconName::Download)
                            .variant(ButtonVariant::Ghost),
                        ),
                )
                .child(
                    Button::new("bulk-clear", crate::localization::t(cx, "views.bulk.clear"))
                        .size(ControlSize::Sm)
                        .on_click(move |_, _, cx| {
                            let _ = this_bulk_clear.update(cx, |app, cx| app.clear_selection(cx));
                        }),
                )
        });

        div()
            .w(width)
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .children(bulk)
            .children(error.map(|message| {
                let retry = self.error_retry_action(message).is_some();
                let this_retry = this.clone();
                div()
                    .mx(px(16.))
                    .mt(px(10.))
                    .p(px(10.))
                    .rounded(px(6.))
                    .bg(colors.sunken)
                    .text_xs()
                    .text_color(colors.danger)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().min_w(px(0.)).child(message.to_string()))
                    .children(retry.then(|| {
                        Button::new(
                            "library-error-retry",
                            crate::localization::t(cx, "status.retry"),
                        )
                        .size(ControlSize::Sm)
                        .variant(ButtonVariant::Ghost)
                        .on_click(move |_, _, cx| {
                            let _ = this_retry.update(cx, |app, cx| app.retry_ui_error(cx));
                        })
                    }))
            }))
            .children((!search_warnings.is_empty()).then(|| {
                div()
                    .mx(px(16.))
                    .mt(px(10.))
                    .p(px(8.))
                    .rounded(px(6.))
                    .bg(colors.sunken)
                    .text_xs()
                    .text_color(colors.warning)
                    .child(crate::localization::t_args(
                        cx,
                        "views.search_warnings",
                        &[("warnings", &search_warnings.join("；"))],
                    ))
            }))
            .child(grid_area)
    }

    /// F-005：分组视图。行只按可见范围构建，组头由 `GroupedList` 自己绘制并吸顶。
    #[allow(clippy::too_many_arguments)]
    fn render_grouped_list(
        &mut self,
        this: &Entity<Self>,
        layout: Arc<GroupLayout>,
        assets: Arc<RwLock<Vec<Asset>>>,
        ordered: Arc<Vec<usize>>,
        selection: Arc<HashSet<PathBuf>>,
        view_mode: ViewMode,
        columns: usize,
        tile_width: f32,
        image_height: f32,
        row_height: f32,
        detail_columns: DetailColumns,
        show_extensions: bool,
        sort_mode: SortMode,
        sort_ascending: bool,
        colors: UiColors,
        cx: &App,
    ) -> AnyElement {
        let reveal_row = self.pending_reveal_group.take().and_then(|key| {
            layout
                .groups
                .iter()
                .position(|group| group.key == key)
                .map(|index| layout.groups[index].start_row)
        });
        let is_grid = view_mode.is_grid();
        let prefer_large = view_mode.prefers_large_thumb();
        let item = self.thumb_cache.clone();
        let focus = self.grid_focus.clone();
        let row_layout = layout.clone();
        let row_ordered = ordered.clone();
        let row_assets = assets.clone();
        let row_selection = selection.clone();
        let row_this = this.clone();
        let row_thumb = item.clone();
        let row_focus = focus.clone();
        let render_row: RowRender = Rc::new(move |display_row, _window, cx| {
            let Some((group_index, local_row)) = row_layout.content_row_of_display(display_row)
            else {
                return div().into_any_element();
            };
            let group = &row_layout.groups[group_index];
            let Ok(assets) = row_assets.read() else {
                return div().into_any_element();
            };
            if is_grid {
                let start = group.start_asset + local_row * columns;
                let end = (start + columns).min(group.start_asset + group.count);
                let mut line = div()
                    .w_full()
                    .h(px(row_height))
                    .flex()
                    .gap(px(GRID_GAP))
                    .px(px(GRID_PADDING))
                    .pt(px(ROW_PADDING))
                    .pb(px(ROW_PADDING));
                for position in start..end {
                    let Some(&index) = row_ordered.get(position) else {
                        break;
                    };
                    let asset = &assets[index];
                    let is_selected = row_selection.contains(&asset.path);
                    line = line.child(tile(
                        asset.clone(),
                        is_selected,
                        row_this.clone(),
                        row_thumb.clone(),
                        tile_width,
                        image_height,
                        row_focus.clone(),
                        colors,
                        prefer_large,
                        show_extensions,
                        cx,
                    ));
                }
                line.into_any_element()
            } else {
                let position = group.start_asset + local_row;
                let Some(&index) = row_ordered.get(position) else {
                    return div().into_any_element();
                };
                let asset = &assets[index];
                let is_selected = row_selection.contains(&asset.path);
                let content = match view_mode {
                    ViewMode::Details => details_row_content(
                        asset,
                        detail_columns,
                        show_extensions,
                        &row_thumb,
                        colors,
                        cx,
                    ),
                    ViewMode::Content => {
                        content_row_content(asset, show_extensions, &row_thumb, colors, cx)
                    }
                    _ => list_row_content(asset, show_extensions, &row_thumb, colors, cx),
                };
                asset_row_shell(
                    asset,
                    is_selected,
                    row_this.clone(),
                    row_focus.clone(),
                    colors,
                    row_height,
                    view_mode == ViewMode::Details,
                    content,
                    cx,
                )
                .into_any_element()
            }
        });
        let header_layout = layout.clone();
        let header_this = this.clone();
        let render_header: HeaderRender = Rc::new(move |group_index, pinned, _window, cx| {
            group_header(
                &header_layout.groups[group_index],
                pinned,
                &header_this,
                colors,
                cx,
            )
        });
        let list_id = match view_mode {
            ViewMode::Details => "asset-details-grouped",
            ViewMode::Content => "asset-content-grouped",
            _ if is_grid => "asset-grid-grouped",
            _ => "asset-list-grouped",
        };
        let list = GroupedList::new(
            list_id,
            layout,
            row_height,
            reveal_row,
            render_row,
            render_header,
        )
        .track_scroll(&self.grid_scroll)
        .flex_1()
        .w_full()
        .min_h(px(0.));
        if view_mode == ViewMode::Details {
            div()
                .flex_1()
                .min_h(px(0.))
                .w_full()
                .flex()
                .flex_col()
                .child(details_header(
                    this.clone(),
                    detail_columns,
                    sort_mode,
                    sort_ascending,
                    colors,
                    cx,
                ))
                .child(list)
                .into_any_element()
        } else {
            list.into_any_element()
        }
    }
}
