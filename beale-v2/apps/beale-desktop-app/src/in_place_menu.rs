use rune::{Align, Color, El, ScrollSpec, Side, Tx, col, el, icon, row, text};
use std::rc::Rc;

use crate::appearance::ThemeStyle;
use crate::icons::CHECK;
use crate::ui_font;

pub(crate) struct MenuItem {
    pub value: String,
    pub label: String,
    pub detail: Option<String>,
    pub selected: bool,
}

pub(crate) fn popup_surface(panel: El, fill: Color) -> El {
    panel
        .radius(12.0)
        .continuous_corners()
        .fill(fill)
        .shadow(8.0, 24.0, Color::BLACK.alpha(0.6))
}

pub(crate) fn panel<S: 'static>(
    anchor: &'static str,
    title: &'static str,
    width: f32,
    active: usize,
    items: Vec<MenuItem>,
    theme: ThemeStyle<'_>,
    actions: (
        impl Fn(&mut S, usize) + 'static,
        impl Fn(&mut S, String, &mut Tx) + 'static,
    ),
) -> El {
    let (on_hover, on_select) = actions;
    let color = |value| theme.color(value);
    let hover = Rc::new(on_hover);
    let select = Rc::new(on_select);
    let has_detail = items.iter().any(|item| item.detail.is_some());
    let item_height = if has_detail { 46.0 } else { 34.0 };
    let height = (items.len() as f32 * (item_height + 2.0) + 10.0).min(320.0);
    let mut options = col().w_full().gap(2.0);
    for (index, item) in items.into_iter().enumerate() {
        let on_hover = hover.clone();
        let on_select = select.clone();
        let value = item.value;
        let label = item.label;
        let mut labels = col().grow(1.0).min_w(0.0).child(
            text(label.clone())
                .truncate()
                .font(ui_font(14.0, if item.selected { 550.0 } else { 400.0 }))
                .color(color("#eeeeee")),
        );
        if let Some(detail) = item.detail {
            labels = labels.child(
                text(detail)
                    .truncate()
                    .font(ui_font(12.5, 400.0))
                    .color(color("#969696")),
            );
        }
        options = options.child(
            row()
                .key(format!("selection-menu-item-{index}"))
                .role("menuitem")
                .label(label)
                .w_full()
                .h(item_height)
                .px(10.0)
                .center_y()
                .gap(8.0)
                .radius(6.0)
                .fill(if index == active {
                    color("#383838")
                } else {
                    Color::TRANSPARENT
                })
                .hover(color("#424242"))
                .on_hover(move |state: &mut S| on_hover(state, index))
                .on_press_tx(move |state: &mut S, tx| on_select(state, value.clone(), tx))
                .child(labels)
                .child(if item.selected {
                    icon(&CHECK, 14.0).color(color("#c8c8c8"))
                } else {
                    el().size(14.0, 14.0)
                }),
        );
    }
    popup_surface(
        col()
            .key("selection-menu")
            .role("menu")
            .label(title)
            .w(width)
            .h(height)
            .min_h(0.0)
            .padding(5.0),
        color("#242424"),
    )
    .anchored_fit(anchor, Side::Bottom, Align::End, 4.0)
    .child(
        col()
            .key("selection-menu-scroll")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(options),
    )
}
