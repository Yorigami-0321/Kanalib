use crate::*;
use super::*;

#[derive(Clone, Copy)]
pub(crate) struct DetailColumns {
    resolution: bool,
    size: bool,
    modified: bool,
    rating: bool,
    star: bool,
}

pub(crate) fn detail_columns(width: f32) -> DetailColumns {
    if width >= 820. {
        DetailColumns {
            resolution: true,
            size: true,
            modified: true,
            rating: true,
            star: true,
        }
    } else if width >= 680. {
        DetailColumns {
            resolution: true,
            size: true,
            modified: true,
            rating: true,
            star: false,
        }
    } else if width >= 600. {
        DetailColumns {
            resolution: true,
            size: true,
            modified: true,
            rating: false,
            star: false,
        }
    } else {
        DetailColumns {
            resolution: false,
            size: true,
            modified: true,
            rating: false,
            star: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum CellAlign {
    Left,
    Right,
    Center,
}

pub(crate) fn detail_header_cell(
    label: &'static str,
    width: Option<f32>,
    align: CellAlign,
    mode: SortMode,
    sort: SortMode,
    ascending: bool,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> impl IntoElement {
    let active = sort == mode;
    let mut cell = div()
        .h_full()
        .flex()
        .items_center()
        .gap(px(2.))
        .text_xs()
        .text_color(if active { colors.fg } else { colors.subtle })
        .cursor_pointer();
    cell = match width {
        Some(width) => {
            let cell = cell.w(px(width)).flex_none();
            match align {
                CellAlign::Left => cell,
                CellAlign::Right => cell.justify_end(),
                CellAlign::Center => cell.justify_center(),
            }
        }
        None => cell.flex_1().min_w(px(180.)),
    };
    cell.id(label)
        .on_click(move |_, _, cx| {
        let _ = this.update(cx, |app, cx| {
            if app.sort == mode {
                app.set_sort_ascending(!app.sort_ascending, cx);
            } else {
                app.choose_sort_mode(mode, cx);
            }
        });
    })
    .child(label)
    .children(active.then(|| {
        Icon::new(if ascending {
            IconName::ArrowUp
        } else {
            IconName::ArrowDown
        })
        .size(IconSize::Xs)
        .color(colors.subtle)
    }))
}

pub(crate) fn details_header(
    this: Entity<KanalibApp>,
    columns: DetailColumns,
    sort: SortMode,
    ascending: bool,
    colors: UiColors,
) -> impl IntoElement {
    div()
        .h(px(30.))
        .flex_none()
        .w_full()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(GRID_PADDING))
        .border_b_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .child(detail_header_cell(
            "名称",
            None,
            CellAlign::Left,
            SortMode::Name,
            sort,
            ascending,
            this.clone(),
            colors,
        ))
        .children(columns.resolution.then(|| {
            detail_header_cell(
                "尺寸",
                Some(110.),
                CellAlign::Left,
                SortMode::Resolution,
                sort,
                ascending,
                this.clone(),
                colors,
            )
        }))
        .children(columns.size.then(|| {
            detail_header_cell(
                "大小",
                Some(90.),
                CellAlign::Right,
                SortMode::Size,
                sort,
                ascending,
                this.clone(),
                colors,
            )
        }))
        .children(columns.modified.then(|| {
            detail_header_cell(
                "修改时间",
                Some(120.),
                CellAlign::Left,
                SortMode::Modified,
                sort,
                ascending,
                this.clone(),
                colors,
            )
        }))
        .children(columns.rating.then(|| {
            detail_header_cell(
                "评级",
                Some(64.),
                CellAlign::Center,
                SortMode::Rating,
                sort,
                ascending,
                this.clone(),
                colors,
            )
        }))
        .children(columns.star.then(|| {
            detail_header_cell(
                "星级",
                Some(64.),
                CellAlign::Center,
                SortMode::Star,
                sort,
                ascending,
                this.clone(),
                colors,
            )
        }))
}

pub(crate) fn details_row_content(
    asset: &Asset,
    columns: DetailColumns,
    show_extensions: bool,
    thumb_cache: &Entity<ThumbCache>,
    colors: UiColors,
) -> AnyElement {
    let title = display_name(asset, show_extensions);
    let favorite = asset.favorite;
    let damaged = asset.damage.is_some();
    let rating = asset.rating.to_uppercase();
    let star = asset.star;
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(row_thumb(asset, thumb_cache, 20., 20., colors))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_xs()
                        .text_color(colors.fg)
                        .child(title),
                )
                .children(favorite.then(|| {
                    Icon::new(IconName::Heart)
                        .size(IconSize::Xs)
                        .color(colors.danger)
                }))
                .children(damaged.then(|| {
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(colors.warning)
                        .child("损坏")
                })),
        )
        .children(columns.resolution.then(|| {
            div()
                .w(px(110.))
                .flex_none()
                .truncate()
                .text_xs()
                .text_color(colors.muted)
                .child(format!("{} × {}", asset.width, asset.height))
        }))
        .children(columns.size.then(|| {
            div()
                .w(px(90.))
                .flex_none()
                .flex()
                .justify_end()
                .text_xs()
                .text_color(colors.muted)
                .child(format_bytes(asset.bytes))
        }))
        .children(columns.modified.then(|| {
            div()
                .w(px(120.))
                .flex_none()
                .text_xs()
                .text_color(colors.muted)
                .child(format_date(asset.mtime))
        }))
        .children(columns.rating.then(|| {
            div()
                .w(px(64.))
                .flex_none()
                .flex()
                .justify_center()
                .text_xs()
                .text_color(colors.muted)
                .child(if rating.is_empty() {
                    "—".to_string()
                } else {
                    rating.clone()
                })
        }))
        .children(columns.star.then(|| {
            div()
                .w(px(64.))
                .flex_none()
                .flex()
                .justify_center()
                .text_xs()
                .text_color(colors.muted)
                .child(if star > 0 {
                    format!("★ {star}")
                } else {
                    "—".to_string()
                })
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {

    #[test]
    fn detail_columns_hide_from_narrow_to_wide() {
        let all = super::detail_columns(900.);
        assert!(all.resolution && all.size && all.modified && all.rating && all.star);
        let no_star = super::detail_columns(700.);
        assert!(no_star.rating && !no_star.star);
        let no_rating = super::detail_columns(640.);
        assert!(!no_rating.rating && !no_rating.star);
        let no_resolution = super::detail_columns(560.);
        assert!(!no_resolution.resolution && no_resolution.size && no_resolution.modified);
    }

}
