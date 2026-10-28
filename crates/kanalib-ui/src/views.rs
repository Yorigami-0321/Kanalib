use crate::*;
use crate::app::KanalibApp;
use crate::format::*;
use crate::theme::*;
use crate::platform::*;
use crate::thumb_cache::ThumbCache;
use std::rc::Rc;

mod content;
mod details;
mod grid;
mod grouped;
mod list;

use self::content::content_row_content;
use self::details::{DetailColumns, detail_columns, details_header, details_row_content};
use self::grid::tile;
use self::grouped::{
    GroupLayout, GroupedList, HeaderRender, RowRender, group_header,
};
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
) -> impl IntoElement {
    let path = asset.path.clone();
    let title = asset.title();
    let menu = asset_context_menu(asset, &this);
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
        .when(divider, |row| {
            row.border_b_1().border_color(colors.border)
        })
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

pub(crate) fn row_meta(asset: &Asset) -> String {
    let rating = asset.rating.to_uppercase();
    let star = asset.star;
    let tag_count = asset
        .all_tags()
        .filter(|tag| matches!(tag.category.as_str(), "general" | "manual"))
        .count();
    let rating = if rating.is_empty() {
        "未评级".to_string()
    } else {
        format!("评级 {rating}")
    };
    let star = if star > 0 {
        format!(" · ★ {star}")
    } else {
        String::new()
    };
    format!("{rating}{star} · {tag_count} 标签")
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

pub(crate) fn asset_context_menu(asset: &Asset, this: &Entity<KanalibApp>) -> Menu {
    let path = asset.path.clone();
    let favorite = asset.favorite;
    let star_menu = {
        let mut menu = Menu::new();
        for (value, label) in [
            (1u8, "1 星"),
            (2, "2 星"),
            (3, "3 星"),
            (4, "4 星"),
            (5, "5 星"),
        ] {
            let this = this.clone();
            let path = path.clone();
            menu = menu.item(MenuItem::new(label).on_click(move |_, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.select_single_path(path.clone(), cx);
                    app.apply_star(value, cx);
                });
            }));
        }
        let this = this.clone();
        let path = path.clone();
        menu.separator()
            .item(MenuItem::new("清除星级").on_click(move |_, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.select_single_path(path.clone(), cx);
                    app.apply_star(0, cx);
                });
            }))
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
    let open_file = path.clone();
    let reveal_file = path.clone();
    Menu::new()
        .item(
            MenuItem::new("快速查看")
                .icon(IconName::Maximize2)
                .on_click(move |_, cx| {
                    let _ = quick_this.update(cx, |app, cx| {
                        app.select_single_path(quick_path.clone(), cx);
                        app.open_quick_view(cx);
                    });
                }),
        )
        .item(
            MenuItem::new("查找相似")
                .icon(IconName::Search)
                .on_click(move |_, cx| {
                    let _ = similar_this.update(cx, |app, cx| {
                        app.open_similar(similar_path.clone(), cx);
                    });
                }),
        )
        .item(
            MenuItem::new("对比选中（2–4 张）")
                .icon(IconName::Columns2)
                .on_click(move |_, cx| {
                    let _ = compare_this.update(cx, |app, cx| app.open_compare_selected(cx));
                }),
        )
        .item(
            MenuItem::new(if favorite { "取消收藏" } else { "收藏" })
                .icon(IconName::Heart)
                .on_click(move |_, cx| {
                    let _ = favorite_this.update(cx, |app, cx| {
                        app.select_single_path(favorite_path.clone(), cx);
                        app.toggle_favorite(cx);
                    });
                }),
        )
        .item(MenuItem::submenu("星级", star_menu))
        .separator()
        .item(
            MenuItem::new("复制 Prompt")
                .icon(IconName::ClipboardCheck)
                .on_click(move |_, cx| {
                    let _ = prompt_this.update(cx, |app, cx| {
                        app.select_single_path(prompt_path.clone(), cx);
                        app.copy_selected_prompt(cx);
                    });
                }),
        )
        .item(
            MenuItem::new("复制路径")
                .icon(IconName::Copy)
                .on_click(move |_, cx| {
                    let _ = paths_this.update(cx, |app, cx| {
                        app.select_single_path(paths_path.clone(), cx);
                        app.copy_selected_paths(cx);
                    });
                }),
        )
        .separator()
        .item(
            MenuItem::new("打开文件")
                .icon(IconName::Image)
                .on_click(move |_, _| open_path(&open_file)),
        )
        .item(
            MenuItem::new("在资源管理器中显示")
                .icon(IconName::FolderOpen)
                .on_click(move |_, _| reveal_path(&reveal_file)),
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
    ) -> impl IntoElement {
        let star_menu = {
            let menu_item = |this: &Entity<Self>, star: u8, label: &'static str| {
                let this = this.clone();
                MenuItem::new(label).on_click(move |_, cx| {
                    let _ = this.update(cx, |app, cx| app.apply_star(star, cx));
                })
            };
            Menu::new()
                .item(menu_item(this, 1, "1 星"))
                .item(menu_item(this, 2, "2 星"))
                .item(menu_item(this, 3, "3 星"))
                .item(menu_item(this, 4, "4 星"))
                .item(menu_item(this, 5, "5 星"))
                .separator()
                .item(menu_item(this, 0, "清除星级"))
        };

        let tag_menu = {
            let add = this.clone();
            let remove = this.clone();
            Menu::new()
                .item(
                    MenuItem::new("添加标签…")
                        .icon(IconName::Tag)
                        .on_click(move |_, cx| {
                            let _ = add.update(cx, |app, cx| {
                                app.open_tag_dialog(TagDialogMode::Add, cx)
                            });
                        }),
                )
                .item(
                    MenuItem::new("移除标签…")
                        .icon(IconName::X)
                        .on_click(move |_, cx| {
                            let _ = remove.update(cx, |app, cx| {
                                app.open_tag_dialog(TagDialogMode::Remove, cx)
                            });
                        }),
                )
        };
        let export_menu = self.export_menu(this);
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
            div()
                .flex_1()
                .p(px(24.))
                .child(
                    div()
                        .p(px(24.))
                        .rounded(px(8.))
                        .bg(colors.sunken)
                        .text_sm()
                        .text_color(colors.muted)
                        .child(if roots_empty {
                            "还没有图库文件夹。点击侧栏“添加文件夹”开始。"
                        } else if scanning {
                            "正在扫描图片..."
                        } else {
                            "没有符合筛选的图片。"
                        }),
                )
                .into_any_element()
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
            )
        } else if is_grid {
            let this_for_list = this.clone();
            let thumb_cache_for_list = self.thumb_cache.clone();
            let focus_for_list = self.grid_focus.clone();
            let selection_for_list = selection.clone();
            let prefer_large = self.view_mode.prefers_large_thumb();
            uniform_list("asset-grid", row_count, move |range, _, _| {
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
                            let asset = &assets[asset_index];
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
            let list = uniform_list(list_id, row_count, move |range, _, _| {
                let Ok(assets) = assets_for_list.read() else {
                    return Vec::new();
                };
                range
                    .map(|position| {
                        let asset = &assets[rows_for_list[position]];
                        let is_selected = selection_for_list.contains(&asset.path);
                        let content = match view_mode {
                            ViewMode::Details => details_row_content(
                                asset,
                                detail_columns,
                                show_extensions,
                                &thumb_cache_for_list,
                                colors,
                            ),
                            ViewMode::Content => content_row_content(
                                asset,
                                show_extensions,
                                &thumb_cache_for_list,
                                colors,
                            ),
                            _ => list_row_content(
                                asset,
                                show_extensions,
                                &thumb_cache_for_list,
                                colors,
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
                                .child(format!("已选 {} 张", selection.len())),
                        )
                        .child(
                            Button::new("bulk-favorite", "收藏")
                                .size(ControlSize::Sm)
                                .icon(IconName::Heart)
                                .on_click(move |_, _, cx| {
                                    let _ =
                                        this_fav.update(cx, |app, cx| app.apply_favorite(true, cx));
                                }),
                        )
                        .child(
                            Button::new("bulk-unfavorite", "取消收藏")
                                .size(ControlSize::Sm)
                                .on_click(move |_, _, cx| {
                                    let _ = this_unfav
                                        .update(cx, |app, cx| app.apply_favorite(false, cx));
                                }),
                        )
                        .child(
                            DropdownMenu::new("bulk-star", "星级", star_menu)
                                .icon(IconName::Star)
                                .variant(ButtonVariant::Ghost),
                        )
                        .child(
                            DropdownMenu::new("bulk-tags", "标签", tag_menu)
                                .icon(IconName::Tag)
                                .variant(ButtonVariant::Ghost),
                        )
                        .child(
                            Button::new("bulk-copy-prompt", "复制 Prompt")
                                .size(ControlSize::Sm)
                                .on_click(move |_, _, cx| {
                                    let _ = this_prompt
                                        .update(cx, |app, cx| app.copy_selected_prompt(cx));
                                }),
                        )
                        .child(
                            Button::new("bulk-copy-paths", "复制路径")
                                .size(ControlSize::Sm)
                                .on_click(move |_, _, cx| {
                                    let _ = this_paths
                                        .update(cx, |app, cx| app.copy_selected_paths(cx));
                                }),
                        )
                        .child(
                            DropdownMenu::new("bulk-export", "导出", export_menu)
                                .icon(IconName::Download)
                                .variant(ButtonVariant::Ghost),
                        ),
                )
                .child(
                    Button::new("bulk-clear", "取消选择")
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
                div()
                    .mx(px(16.))
                    .mt(px(10.))
                    .p(px(10.))
                    .rounded(px(6.))
                    .bg(colors.sunken)
                    .text_xs()
                    .text_color(colors.danger)
                    .child(message.to_string())
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
                    .child(format!("搜索条件提示：{}", search_warnings.join("；")))
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
        let render_row: RowRender = Rc::new(move |display_row, _window, _cx| {
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
                    ),
                    ViewMode::Content => {
                        content_row_content(asset, show_extensions, &row_thumb, colors)
                    }
                    _ => list_row_content(asset, show_extensions, &row_thumb, colors),
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
                )
                .into_any_element()
            }
        });
        let header_layout = layout.clone();
        let header_this = this.clone();
        let render_header: HeaderRender = Rc::new(move |group_index, pinned, _window, _cx| {
            group_header(
                &header_layout.groups[group_index],
                pinned,
                &header_this,
                colors,
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
                ))
                .child(list)
                .into_any_element()
        } else {
            list.into_any_element()
        }
    }
}
