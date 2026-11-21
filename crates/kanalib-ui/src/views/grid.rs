use super::*;
use crate::*;
use ely_gpui_component::theme::Elevation;

pub(crate) fn tile(
    asset: Asset,
    selected: bool,
    this: Entity<KanalibApp>,
    thumb_cache: Entity<ThumbCache>,
    width: f32,
    image_height: f32,
    focus: FocusHandle,
    colors: UiColors,
    prefer_large: bool,
    show_extensions: bool,
    cx: &App,
) -> impl IntoElement {
    let path = asset.path.clone();
    let thumb = if prefer_large {
        asset.thumb.clone().or_else(|| asset.tile_thumb.clone())
    } else {
        asset.tile_thumb.clone().or_else(|| asset.thumb.clone())
    };
    let media: AnyElement = if let Some(thumb) = thumb {
        img(thumb)
            .image_cache(&thumb_cache)
            .size_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element()
    } else {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                Icon::new(IconName::ImageOff)
                    .size(IconSize::Lg)
                    .color(colors.subtle),
            )
            .into_any_element()
    };
    let has_error = asset.error.is_some();
    let has_damage = asset.damage.is_some();
    let title = display_name(&asset, show_extensions);
    let tag_count = asset
        .all_tags()
        .filter(|tag| matches!(tag.category.as_str(), "general" | "manual"))
        .count();
    let rating = asset.rating.to_uppercase();
    let favorite = asset.favorite;
    let star = asset.star;
    let video = asset.is_video();
    let media_icon = if video {
        IconName::Play
    } else {
        IconName::Image
    };
    let media_color = if video { colors.info } else { colors.muted };
    // 卡头色码只标“非普通图片”的部分；普通图片不做装饰，让照片本身最亮。
    let rail = if has_error {
        Some(colors.danger)
    } else if has_damage {
        Some(colors.warning)
    } else if video {
        Some(colors.info)
    } else {
        None
    };
    let media_label = if video {
        let duration = crate::format::video_meta(&asset.metadata_json)
            .duration_secs
            .map(crate::format::format_duration);
        match duration {
            Some(duration) => crate::localization::t_args(
                cx,
                "views.tile.video_duration",
                &[("duration", &duration)],
            )
            .to_string(),
            None => crate::localization::t(cx, "views.tile.video").to_string(),
        }
    } else if asset.width > 0 && asset.height > 0 {
        format!("{}×{}", asset.width, asset.height)
    } else {
        crate::inspector::friendly_format_label(&asset.path)
    };
    let rating_text = if rating.is_empty() {
        if star > 0 {
            format!("★ {star}")
        } else {
            crate::localization::t(cx, "views.rating.none").to_string()
        }
    } else if star > 0 {
        format!(
            "{} · ★ {star}",
            crate::localization::t_args(cx, "views.rating.value", &[("rating", &rating)])
        )
    } else {
        crate::localization::t_args(cx, "views.rating.value", &[("rating", &rating)]).to_string()
    };
    let menu = asset_context_menu(&asset, &this, cx);
    let this_click = this.clone();
    let path_click = path.clone();
    let focus_click = focus.clone();
    let this_right = this.clone();
    let path_right = path.clone();
    let focus_right = focus.clone();
    let hover_group = SharedString::from(format!("tile-hover-{}", path.display()));
    let hover_group_footer = hover_group.clone();
    let raised = cx.theme().elevation(Elevation::Raised);
    let hover_border = if selected {
        colors.accent
    } else {
        colors.border_strong
    };

    let tile = div()
        .id(SharedString::from(format!("tile-{}", asset.path.display())))
        .group(hover_group)
        .role(Role::Button)
        .aria_label(title.clone())
        .w(px(width))
        .flex_none()
        .flex()
        .flex_col()
        .rounded(px(RADIUS_CARD))
        .overflow_hidden()
        .bg(colors.surface)
        .border_1()
        .border_color(if selected {
            colors.accent
        } else {
            colors.border
        })
        .cursor_pointer()
        .hover(move |style| {
            style
                .bg(colors.overlay)
                .border_color(hover_border)
                .shadow(raised.clone())
        })
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
        .child(
            div()
                .relative()
                .w_full()
                .h(px(image_height))
                .flex_none()
                .overflow_hidden()
                .bg(colors.sunken)
                .child(media)
                .children(rail.map(|color| {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(3.))
                        .bg(color)
                }))
                .children(selected.then(|| {
                    div()
                        .absolute()
                        .top(px(7.))
                        .left(px(7.))
                        .size(px(20.))
                        .rounded_full()
                        .bg(colors.accent)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Check)
                                .size(IconSize::Xs)
                                .color(colors.bg),
                        )
                }))
                // 右上角状态列：收藏、失败、损坏依次堆叠，避免任意两个角标互相压住。
                .children((favorite || has_error || has_damage).then(|| {
                    let mut badges = div()
                        .absolute()
                        .top(px(7.))
                        .right(px(7.))
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(4.));
                    if favorite {
                        badges = badges.child(
                            div()
                                .size(px(22.))
                                .rounded_full()
                                .bg(colors.backdrop)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Heart)
                                        .size(IconSize::Sm)
                                        .color(colors.danger),
                                ),
                        );
                    }
                    if has_error {
                        badges = badges.child(
                            div()
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(RADIUS_SM))
                                .bg(colors.overlay)
                                .text_xs()
                                .text_color(colors.danger)
                                .child(crate::localization::t(cx, "views.tile.failed")),
                        );
                    }
                    if has_damage {
                        badges = badges.child(
                            div()
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(RADIUS_SM))
                                .bg(colors.overlay)
                                .text_xs()
                                .text_color(colors.warning)
                                .child(crate::localization::t(cx, "views.tile.damaged")),
                        );
                    }
                    badges
                }))
                .child(
                    div()
                        .absolute()
                        .left(px(7.))
                        .bottom(px(7.))
                        .max_w(px(if width < 150. {
                            width - 22.
                        } else {
                            width - 24.
                        }))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .px(px(7.))
                        .py(px(2.))
                        .rounded(px(RADIUS_SM))
                        .bg(colors.overlay)
                        .child(Icon::new(media_icon).size(IconSize::Xs).color(media_color))
                        .child(
                            div()
                                .min_w(px(0.))
                                .truncate()
                                .text_xs()
                                .text_color(colors.fg)
                                .child(media_label),
                        ),
                ),
        )
        .child(
            div()
                .h(px(CARD_FOOTER_HEIGHT))
                .w_full()
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(2.))
                .px(px(10.))
                .py(px(6.))
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
                        .gap(px(4.))
                        .child(Icon::new(media_icon).size(IconSize::Xs).color(media_color))
                        .child(
                            div()
                                .min_w(px(0.))
                                .truncate()
                                .text_xs()
                                .text_color(colors.subtle)
                                .child(rating_text),
                        )
                        .child(div().flex_1())
                        .child(div().flex_none().text_xs().text_color(colors.subtle).child(
                            crate::localization::t_args(
                                cx,
                                "views.tag_count",
                                &[("n", &tag_count.to_string())],
                            ),
                        ))
                        .child(
                            div()
                                .flex_none()
                                .invisible()
                                .group_hover(hover_group_footer, |style| style.visible())
                                .child(
                                    Icon::new(IconName::Maximize2)
                                        .size(IconSize::Xs)
                                        .color(colors.muted),
                                ),
                        ),
                ),
        );

    div()
        .flex_none()
        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
            window.focus(&focus_right, cx);
            let _ = this_right.update(cx, |app, cx| {
                app.handle_tile_right_click(path_right.clone(), cx);
            });
        })
        .child(
            ContextMenu::new(
                SharedString::from(format!("tile-context-{}", asset.path.display())),
                menu,
            )
            .child(tile),
        )
}
