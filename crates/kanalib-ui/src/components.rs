use ely_gpui_component::buttons::{Button, ButtonVariant};
use ely_gpui_component::primitives::{Icon, IconName};
use ely_gpui_component::theme::{ControlSize, IconSize};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, IntoElement, SharedString, Window, div, px,
};

use crate::theme::{RADIUS_LG, SPACE_2, SPACE_6, UiColors};

/// F-073：筛选 chip 统一用 Button，保证键盘焦点、禁用与悬停状态一致。
pub(crate) fn filter_chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    Button::new(id, label)
        .size(ControlSize::Sm)
        .variant(if active {
            ButtonVariant::Secondary
        } else {
            ButtonVariant::Ghost
        })
        .on_click(on_click)
        .into_any_element()
}

/// F-073：空/失败/未开始状态共用同一结构：图标、标题、说明、一个下一步。
pub(crate) fn empty_state(
    icon: IconName,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    action: Option<AnyElement>,
    colors: UiColors,
) -> AnyElement {
    let title: SharedString = title.into();
    let description: SharedString = description.into();
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .items_center()
        .justify_center()
        .p(px(SPACE_6))
        .child(
            div()
                .max_w(px(420.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(SPACE_2))
                .p(px(SPACE_6))
                .rounded(px(RADIUS_LG))
                .border_1()
                .border_color(colors.border)
                .bg(colors.sunken)
                .text_center()
                .child(Icon::new(icon).size(IconSize::Xl).color(colors.subtle))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.fg)
                        .child(title),
                )
                .child(div().text_xs().text_color(colors.muted).child(description))
                .children(action),
        )
        .into_any_element()
}
