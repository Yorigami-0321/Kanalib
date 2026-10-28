use crate::*;
use super::*;

pub(crate) fn list_row_content(
    asset: &Asset,
    show_extensions: bool,
    thumb_cache: &Entity<ThumbCache>,
    colors: UiColors,
) -> AnyElement {
    let title = display_name(asset, show_extensions);
    let favorite = asset.favorite;
    let damaged = asset.damage.is_some();
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(row_thumb(asset, thumb_cache, 24., 24., colors))
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
                .px(px(4.))
                .rounded(px(3.))
                .text_xs()
                .text_color(colors.warning)
                .child("损坏")
        }))
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(colors.subtle)
                .child(row_meta(asset)),
        )
        .into_any_element()
}
