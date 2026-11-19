use rune::{El, Icon, el, icon, row};

use crate::ThemeStyle;

pub(crate) fn icon_action_button(
    key: impl Into<String>,
    label: impl Into<String>,
    glyph: &'static Icon,
    theme: ThemeStyle<'_>,
) -> El {
    let label = label.into();
    el().key(key.into())
        .role("button")
        .label(label.clone())
        .help(label)
        .size(30.0, 30.0)
        .center()
        .radius(8.0)
        .continuous_corners()
        .hover(theme.color("#303030"))
        .child(icon(glyph, 16.0).color(theme.color("#bdbdbd")))
}

pub(crate) fn toggle_button(
    key: impl Into<String>,
    label: impl Into<String>,
    checked: bool,
    available: bool,
    theme: ThemeStyle<'_>,
) -> El {
    let key = key.into();
    let status = if checked { "on" } else { "off" };
    let label = label.into();
    // Dark themes use neutral accents, so "off" recedes (muted thumb on a dim track) and the
    // lit accent track is the only bright state.
    let dark = theme.is_dark();
    let track = if checked {
        theme.accent()
    } else {
        theme.color(if dark { "#2b2b2b" } else { "#414141" })
    };
    let thumb = if checked {
        "#0d0d0d"
    } else if !available {
        if dark { "#5c5c5c" } else { "#969696" }
    } else if dark {
        "#8c8c8c"
    } else {
        "#f2f2f2"
    };
    let mut toggle = row()
        .key(key.clone())
        .role("switch")
        .label(format!("{label}, {status}"))
        .help(format!(
            "Turn {label} {}",
            if checked { "off" } else { "on" }
        ))
        .size(38.0, 22.0)
        .center_y()
        .radius(11.0)
        .continuous_corners()
        .fill(track)
        .border(
            1.0,
            if checked {
                theme.accent()
            } else {
                theme.color(if dark { "#474747" } else { "#555555" })
            },
        )
        .child(
            el().key(format!("{key}-thumb"))
                .size(16.0, 16.0)
                .ml(if checked { 19.0 } else { 3.0 })
                .radius(8.0)
                .fill(if checked {
                    theme.accent_ink()
                } else {
                    theme.color(thumb)
                }),
        );
    if available {
        toggle = toggle.hover(theme.color(if checked {
            "#eeeeee"
        } else if dark {
            "#363636"
        } else {
            "#4a4a4a"
        }));
    }
    toggle
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::AppearancePreferences;
    use rune::{Cx, Env, Runtime};

    struct State {
        checked: bool,
        appearance: AppearancePreferences,
    }

    fn view(state: &State, _: &mut Cx<State>) -> El {
        toggle_button(
            "example-toggle",
            "Example rule",
            state.checked,
            true,
            state.appearance.style(),
        )
        .on_press(|state: &mut State| state.checked = !state.checked)
    }

    #[test]
    fn reusable_toggle_moves_thumb_and_updates_state() {
        let state = State {
            checked: false,
            appearance: AppearancePreferences::default(),
        };
        let mut runtime = Runtime::new(state, view, (120.0, 80.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        runtime.frame(1.0);
        let (left, _) = runtime.presented("example-toggle-thumb").unwrap();
        let (x, y) = runtime.find("example-toggle").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(2.0);
        let (right, _) = runtime.presented("example-toggle-thumb").unwrap();
        assert!(runtime.state.checked);
        assert!(right[0] > left[0]);
    }
}
