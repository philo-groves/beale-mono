use rune::tok::TextStyle;
use rune::{Align, Color, El, ScrollSpec, Side, col, el, row, text};
use std::rc::Rc;

pub(crate) struct MenuItem {
    pub value: String,
    pub label: String,
    pub detail: Option<String>,
    pub selected: bool,
}

fn color(value: &str) -> Color {
    rune::color::hex(value).unwrap_or(Color::BLACK)
}

pub(crate) fn dismiss_layer<S: 'static>(dismiss: impl Fn(&mut S) + 'static) -> El {
    el().key("selection-menu-dismiss")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .on_press(dismiss)
}

pub(crate) fn panel<S: 'static>(
    anchor: &'static str,
    title: &'static str,
    width: f32,
    active: usize,
    items: Vec<MenuItem>,
    on_hover: impl Fn(&mut S, usize) + 'static,
    on_select: impl Fn(&mut S, String) + 'static,
) -> El {
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
                .font(TextStyle::new(
                    12.0,
                    if item.selected { 550.0 } else { 400.0 },
                ))
                .color(color("#eeeeee")),
        );
        if let Some(detail) = item.detail {
            labels = labels.child(
                text(detail)
                    .truncate()
                    .font(TextStyle::new(10.5, 400.0))
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
                .on_press(move |state: &mut S| on_select(state, value.clone()))
                .child(labels)
                .child(if item.selected {
                    el().size(6.0, 6.0).radius(3.0).fill(color("#c8c8c8"))
                } else {
                    el().size(6.0, 6.0)
                }),
        );
    }
    col()
        .key("selection-menu")
        .role("menu")
        .label(title)
        .w(width)
        .h(height)
        .min_h(0.0)
        .padding(5.0)
        .radius(9.0)
        .fill(color("#242424"))
        .border(1.0, color("#3c3c3c"))
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
