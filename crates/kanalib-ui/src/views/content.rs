use super::*;
use crate::inspector::friendly_media_error;
use crate::*;

pub(crate) fn content_row_content(
    asset: &Asset,
    show_extensions: bool,
    thumb_cache: &Entity<ThumbCache>,
    colors: UiColors,
    cx: &App,
) -> AnyElement {
    let title = display_name(asset, show_extensions);
    let error = asset
        .error
        .clone()
        .map(|value| friendly_media_error(&value));
    let summary = error
        .clone()
        .or_else(|| asset.damage.clone())
        .unwrap_or_else(|| asset.summary());
    let summary_color = if error.is_some() {
        colors.danger
    } else if asset.damage.is_some() {
        colors.warning
    } else {
        colors.muted
    };
    let rating = asset.rating.to_uppercase();
    let star = asset.star;
    let tag_count = asset
        .all_tags()
        .filter(|tag| matches!(tag.category.as_str(), "general" | "manual"))
        .count();
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(row_thumb(asset, thumb_cache, 96., 64., colors))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
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
                        .truncate()
                        .text_xs()
                        .text_color(colors.muted)
                        .child(format!(
                            "{} × {} · {} · {}",
                            asset.width,
                            asset.height,
                            format_bytes(asset.bytes),
                            format_date(asset.mtime)
                        )),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(summary_color)
                        .child(summary),
                ),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(4.))
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(if rating.is_empty() {
                            if star > 0 {
                                format!("★ {star}")
                            } else {
                                crate::localization::t(cx, "views.rating.none").to_string()
                            }
                        } else if star > 0 {
                            format!(
                                "{} · ★ {star}",
                                crate::localization::t_args(
                                    cx,
                                    "views.rating.value",
                                    &[("rating", &rating)],
                                )
                            )
                        } else {
                            crate::localization::t_args(
                                cx,
                                "views.rating.value",
                                &[("rating", &rating)],
                            )
                            .to_string()
                        }),
                )
                .child(div().text_xs().text_color(colors.subtle).child(
                    crate::localization::t_args(
                        cx,
                        "views.tag_count",
                        &[("n", &tag_count.to_string())],
                    ),
                )),
        )
        .into_any_element()
}
