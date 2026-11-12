use rune::{Color, El, ScrollSpec, col, el, icon, row, text};

use crate::{State, ThemeStyle, icons::CLOSE, ui_font};

pub(crate) struct DialogSpec {
    pub(crate) key: &'static str,
    pub(crate) title: &'static str,
    pub(crate) width: f32,
    pub(crate) focus_return: &'static str,
}

pub(crate) fn render(
    spec: DialogSpec,
    body: El,
    actions: El,
    theme: ThemeStyle<'_>,
    close: fn(&mut State),
) -> [El; 2] {
    let DialogSpec {
        key,
        title,
        width,
        focus_return,
    } = spec;
    let color = |value| theme.color(value);
    let panel = col()
        .key(key)
        .role("dialog")
        .label(title)
        .focus_return(focus_return)
        .w_full()
        .max_w(width)
        .shrink()
        .min_h(0.0)
        .padding(24.0)
        .gap(20.0)
        .radius(16.0)
        .continuous_corners()
        .fill(color("#191919"))
        .border(1.0, color("#303030"))
        .shadow(8.0, 28.0, Color::BLACK.alpha(0.24))
        .child(
            row()
                .w_full()
                .center_y()
                .child(
                    text(title)
                        .grow(1.0)
                        .font(ui_font(19.0, 600.0))
                        .color(color("#f2f2f2")),
                )
                .child(
                    el().key(format!("{key}-close"))
                        .role("button")
                        .label("Close dialog")
                        .size(26.0, 26.0)
                        .center()
                        .radius(8.0)
                        .continuous_corners()
                        .hover(color("#303030"))
                        .on_press(close)
                        .child(icon(&CLOSE, 16.0).color(color("#bdbdbd"))),
                ),
        )
        .child(body.shrink().min_h(0.0).scroll(ScrollSpec {
            stick_bottom: false,
        }))
        .child(actions)
        .modal(true, close);
    [
        el().key(format!("{key}-scrim"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(Color::BLACK.alpha(0.55)),
        col()
            .key(format!("{key}-layer"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(16.0)
            .center()
            .child(panel),
    ]
}
