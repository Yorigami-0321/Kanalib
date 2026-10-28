use crate::*;
use super::*;

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
            .text_xs()
            .text_color(colors.subtle)
            .child("…")
            .into_any_element()
    };
    let border = if selected {
        colors.accent
    } else {
        colors.border
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
    let menu = asset_context_menu(&asset, &this);
    let this_click = this.clone();
    let path_click = path.clone();
    let focus_click = focus.clone();
    let this_right = this.clone();
    let path_right = path.clone();
    let focus_right = focus.clone();

    let mut tile = div()
        .id(SharedString::from(format!("tile-{}", asset.path.display())))
        .role(Role::Button)
        .aria_label(title.clone())
        .w(px(width))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(5.))
        .cursor_pointer()
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
                .rounded(px(8.))
                .overflow_hidden()
                .border_1()
                .border_color(border)
                .bg(colors.sunken)
                .child(media)
                .children(selected.then(|| {
                    div()
                        .absolute()
                        .top(px(6.))
                        .left(px(6.))
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
                .children(favorite.then(|| {
                    div()
                        .absolute()
                        .top(px(6.))
                        .right(px(6.))
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
                        )
                }))
                .children(has_damage.then(|| {
                    div()
                        .absolute()
                        .left(px(6.))
                        .bottom(px(6.))
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(colors.backdrop)
                        .text_xs()
                        .text_color(colors.warning)
                        .child("损坏")
                }))
                .children(has_error.then(|| {
                    div()
                        .absolute()
                        .right(px(6.))
                        .bottom(px(6.))
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(colors.backdrop)
                        .text_xs()
                        .text_color(colors.danger)
                        .child("失败")
                })),
        )
        .child(
            div()
                .truncate()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.fg)
                .child(title),
        );
    tile = tile.child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(4.))
            .child(
                div()
                    .truncate()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(if rating.is_empty() {
                        if star > 0 {
                            format!("★ {star}")
                        } else {
                            "未评级".to_string()
                        }
                    } else if star > 0 {
                        format!("评级 {rating} · ★ {star}")
                    } else {
                        format!("评级 {rating}")
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(format!("{tag_count} 标签")),
            ),
    );

    div()
        .flex_none()
        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
            window.focus(&focus_right, cx);
            let _ = this_right.update(cx, |app, cx| {
                app.select_single_path(path_right.clone(), cx);
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
