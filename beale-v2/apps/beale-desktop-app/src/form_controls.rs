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
        .size(26.0, 26.0)
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
    let track = if checked { "#f2f2f2" } else { "#414141" };
    let thumb = if checked {
        "#0d0d0d"
    } else if available {
        "#f2f2f2"
    } else {
        "#969696"
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
        .fill(theme.color(track))
        .border(
            1.0,
            theme.color(if checked { "#f2f2f2" } else { "#555555" }),
        )
        .child(
            el().key(format!("{key}-thumb"))
                .size(16.0, 16.0)
                .ml(if checked { 19.0 } else { 3.0 })
                .radius(8.0)
                .fill(theme.color(thumb)),
        );
    if available {
        toggle = toggle.hover(theme.color(if checked { "#eeeeee" } else { "#4a4a4a" }));
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
