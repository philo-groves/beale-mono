use crate::{State, ThemeStyle, design, form_button, icons::*, ui_font};
use rune::{El, ScrollSpec, col, el, icon, row, text};

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>) -> El {
    let mut page = col()
        .w_full()
        .max_w(1020.0)
        .self_center()
        .padding(36.0)
        .gap(28.0)
        .child(
            col()
                .w_full()
                .gap(10.0)
                .child(design::breadcrumb_title(
                    "Beale",
                    text("Research desk")
                        .role("heading")
                        .truncate_fade()
                        .font(crate::view_title_font())
                        .color(theme.color("#f2f2f2")),
                    theme,
                ))
                .child(design::subtitle(
                    "Define your scope. Follow the evidence. Build a defensible finding.",
                    theme,
                )),
        )
        .child(
            row()
                .gap(10.0)
                .child(
                    design::primary_button("New research", theme)
                        .key("overview-new-research")
                        .on_press(State::start_new_research),
                )
                .child(
                    form_button("Create workspace", theme)
                        .key("overview-new-workspace")
                        .on_press(State::start_workspace_creation),
                ),
        );
    let mut stats = row().w_full().gap(12.0);
    for (label, value, note) in [
        (
            "WORKSPACES",
            state.workspaces.len(),
            "Registered research scopes",
        ),
        (
            "SESSIONS",
            state.session_summaries.len(),
            "Saved research conversations",
        ),
        (
            "CAPABILITIES",
            state.features.iter().filter(|f| f.enabled).count(),
            "Enabled research features",
        ),
    ] {
        stats = stats.child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .padding(18.0)
                .gap(10.0)
                .radius(design::CARD_RADIUS)
                .fill(theme.color("#191919"))
                .border(1.0, theme.color("#2a2a2a"))
                .child(design::eyebrow(label, theme))
                .child(
                    text(value.to_string())
                        .font(ui_font(30.0, 500.0))
                        .color(theme.color("#f2f2f2")),
                )
                .child(design::subtitle(note, theme)),
        );
    }
    page = page.child(stats).child(
        row()
            .w_full()
            .center_y()
            .child(
                text("Your workspaces")
                    .font(ui_font(18.0, 600.0))
                    .color(theme.color("#f2f2f2")),
            )
            .child(el().grow(1.0))
            .child(design::badge("RESEARCH DESK", theme)),
    );
    if state.workspaces.is_empty() {
        page = page.child(design::empty_state(&FOLDER_OPEN, "Set the boundary for your research",
            "Create a workspace to keep authorized scope, sessions, surfaces, and findings together. Start with a directory and a scope rule.", theme));
    } else {
        let mut list = col().w_full().gap(10.0);
        for workspace in &state.workspaces {
            let Ok(id) = uuid::Uuid::parse_str(&workspace.id) else {
                continue;
            };
            let sessions = state
                .session_summaries
                .iter()
                .filter(|s| s.workspace_id == workspace.id)
                .count();
            list = list.child(
                row()
                    .key(format!("overview-workspace-{id}"))
                    .role("button")
                    .label(format!("Open {}", workspace.name))
                    .w_full()
                    .padding(18.0)
                    .gap(16.0)
                    .center_y()
                    .radius(design::CARD_RADIUS)
                    .fill(theme.color("#191919"))
                    .border(1.0, theme.color("#2a2a2a"))
                    .hover(theme.color("#242424"))
                    .on_press(move |s: &mut State| s.open_workspace(id))
                    .child(
                        el().size(40.0, 40.0)
                            .center()
                            .radius(10.0)
                            .fill(theme.subtle_accent())
                            .child(icon(&FOLDER_OPEN, 20.0).color(theme.accent())),
                    )
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(5.0)
                            .child(
                                text(workspace.name.clone())
                                    .truncate()
                                    .font(ui_font(15.0, 550.0))
                                    .color(theme.color("#f2f2f2")),
                            )
                            .child(design::subtitle(
                                format!("{sessions} saved sessions"),
                                theme,
                            )),
                    )
                    .child(icon(&CHEVRON_RIGHT, 16.0).color(theme.color("#969696"))),
            );
        }
        page = page.child(list);
    }
    col()
        .key("home-overview")
        .w_full()
        .h_full()
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(page)
}
