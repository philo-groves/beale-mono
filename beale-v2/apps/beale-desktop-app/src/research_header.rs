//! Workspace and session destinations share a persistent pair of header tabs.
use crate::design::FadeText;
use crate::{
    PrimaryView, State, ThemeStyle, ViewLocation, icons, ui_font, workspace_view::WorkspaceTab,
};
use rune::{El, Icon, el, icon, row, text};

pub(crate) fn workspace(state: &State) -> Option<(uuid::Uuid, WorkspaceTab)> {
    let id = state
        .active_session
        .as_ref()
        .and_then(|detail| detail.summary.as_ref())
        .map(|summary| summary.workspace_id.as_str())
        .or_else(|| {
            state
                .selected_research_workspace()
                .map(|workspace| workspace.id.as_str())
        })?;
    let id = uuid::Uuid::parse_str(id).ok()?;
    if state.primary_view != PrimaryView::NewResearch
        && !(state.primary_view == PrimaryView::Workspace
            && state.workspace_id == Some(id)
            && state.research_workspace_view.is_some())
    {
        return None;
    }
    let tab = state
        .research_workspace_view
        .filter(|(workspace, _)| *workspace == id)
        .map(|(_, tab)| tab)
        .unwrap_or_else(|| {
            if crate::extensions_view::surfaces_enabled(state, &id.to_string()) {
                WorkspaceTab::Surfaces
            } else {
                WorkspaceTab::Rules
            }
        });
    Some((id, tab))
}

const DEFAULT_TAB_MAX_WIDTH: f32 = 240.0;
const WORKSPACE_TAB_MAX_WIDTH: f32 = 360.0;
const TAB_SEPARATOR_INSET: f32 = 3.0;

struct TabSeparators<'a> {
    left: bool,
    right: bool,
    hovered_key: Option<&'a str>,
    next_key: Option<&'a str>,
}

fn tab(
    key: impl Into<String>,
    label: &str,
    glyph: Option<&'static Icon>,
    active: bool,
    dense: bool,
    separators: TabSeparators<'_>,
    theme: ThemeStyle<'_>,
) -> El {
    let key = key.into();
    let hover_key = key.clone();
    let hovered = separators.hovered_key == Some(key.as_str());
    let mut item = row()
        .key(key.clone())
        .role("tab")
        .label(format!("{label}{}", if active { ", selected" } else { "" }))
        .h(34.0)
        .grow(1.0)
        .max_w(DEFAULT_TAB_MAX_WIDTH)
        .shrink()
        .px(if dense { 7.0 } else { 12.0 })
        .gap(if dense { 5.0 } else { 8.0 })
        .center_y()
        .radius(11.0)
        .continuous_corners()
        .hover(theme.color("#303030"))
        .on_hover_change(move |state: &mut State, inside, _| {
            if inside {
                state.hovered_header_tab = Some(hover_key.clone());
            } else if state.hovered_header_tab.as_deref() == Some(hover_key.as_str()) {
                state.hovered_header_tab = None;
            }
        });
    if active {
        item = item
            .fill(theme.color("#303030"))
            .border(1.0, theme.color("#3a3a3a"));
    } else if !hovered {
        if separators.left {
            item = item.child(
                el().key(format!("{key}-left-separator"))
                    .pin(
                        Some(0.0),
                        Some(TAB_SEPARATOR_INSET),
                        None,
                        Some(TAB_SEPARATOR_INSET),
                    )
                    .w(1.0)
                    .fill(theme.color("#3a3a3a"))
                    .inert(),
            );
        }
        if separators.right
            && separators
                .next_key
                .is_none_or(|next| separators.hovered_key != Some(next))
        {
            item = item.child(
                el().key(format!("{key}-right-separator"))
                    .pin(
                        None,
                        Some(TAB_SEPARATOR_INSET),
                        Some(0.0),
                        Some(TAB_SEPARATOR_INSET),
                    )
                    .w(1.0)
                    .fill(theme.color("#3a3a3a"))
                    .inert(),
            );
        }
    }
    if let Some(glyph) = glyph {
        item = item.child(
            icon(glyph, if dense { 14.0 } else { 16.0 }).color(theme.color(if active {
                "#dddddd"
            } else {
                "#aaaaaa"
            })),
        );
    }
    item
}

fn role_tab_label(role: beale_graphs::Role, narrow: bool) -> &'static str {
    match role {
        beale_graphs::Role::DiscoveryHarness | beale_graphs::Role::Discovery => {
            if narrow {
                "Disc"
            } else {
                "Discovery"
            }
        }
        beale_graphs::Role::ValidationSystem | beale_graphs::Role::Validation => {
            if narrow {
                "Valid"
            } else {
                "Validation"
            }
        }
        beale_graphs::Role::Reporter => {
            if narrow {
                "Report"
            } else {
                "Reporter"
            }
        }
        beale_graphs::Role::PatchGeneration => "Patch",
        _ => role.label(),
    }
}

pub(crate) fn selected_role(state: &State, graph_id: &str) -> String {
    let Some(graph) = beale_graphs::graph_definition(graph_id) else {
        return "main".to_owned();
    };
    graph
        .nodes
        .iter()
        .copied()
        .filter(|role| role.is_top_level())
        .find(|role| role.id() == state.session_agent_role)
        .or_else(|| beale_graphs::entry_role(graph_id))
        .unwrap_or(beale_graphs::Role::Main)
        .id()
        .to_owned()
}

pub(crate) fn sync_model_for_role(state: &mut State, role: &str) {
    let model = state.active_session.as_ref().and_then(|detail| {
        detail
            .graph_models
            .iter()
            .find(|model| model.node_id == role)
            .or_else(|| {
                beale_graphs::entry_role(&detail.graph_id).and_then(|entry| {
                    detail
                        .graph_models
                        .iter()
                        .find(|model| model.node_id == entry.id())
                })
            })
            .cloned()
    });
    if let Some(model) = model
        && let (Ok(provider_id), Ok(method)) = (
            beale_contracts::v1::ProviderId::try_from(model.provider),
            beale_contracts::v1::AuthMethod::try_from(model.method),
        )
        && let Some(provider) = crate::Provider::from_id(provider_id)
    {
        state.new_research_model = Some(crate::new_research::ModelSelection {
            provider,
            method,
            id: model.model,
        });
    }
}

pub(crate) fn tabs(state: &State, theme: ThemeStyle<'_>, window_width: f32) -> Option<El> {
    let (id, section) = workspace(state)?;
    let name = state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == id.to_string())
        .map_or("Workspace", |workspace| workspace.name.as_str());
    let in_workspace = state.primary_view == PrimaryView::Workspace;
    let started = state.active_session.is_some()
        || state.session_busy
        || state.session_loading_id.is_some()
        || state.session_pending_prompt.is_some();
    let graph_id = state
        .active_session
        .as_ref()
        .map(|session| session.graph_id.as_str())
        .unwrap_or(&state.new_research_graph_id);
    let peer_graph = started
        && matches!(
            graph_id,
            beale_graphs::CLOUDFLARE_ID | beale_graphs::OPENAI_ID | beale_graphs::SEQUENTIAL_ID
        );
    let session_label = if started {
        "Main Agent"
    } else {
        "New Research"
    };
    let compact = peer_graph && window_width < 1250.0;
    let narrow = peer_graph && window_width < 950.0;
    let selected_role = peer_graph.then(|| selected_role(state, graph_id));
    let next_tab_active = !in_workspace
        && if peer_graph {
            beale_graphs::graph_definition(graph_id)
                .and_then(|graph| graph.nodes.iter().find(|role| role.is_top_level()))
                .is_some_and(|role| selected_role.as_deref() == Some(role.id()))
        } else {
            true
        };
    let next_tab_key = if peer_graph {
        beale_graphs::graph_definition(graph_id)
            .and_then(|graph| graph.nodes.iter().find(|role| role.is_top_level()))
            .map(|role| format!("header-agent-tab-{}", role.id()))
    } else {
        Some("header-session-tab".to_owned())
    };
    let workspace_tab_max_width = if compact {
        DEFAULT_TAB_MAX_WIDTH
    } else {
        let name_width = rune::text::measure(name, &[], ui_font(13.0, 500.0), None).0;
        let section_width = rune::text::measure(section.label(), &[], ui_font(13.0, 400.0), None).0;
        (name_width + section_width + 72.0).clamp(DEFAULT_TAB_MAX_WIDTH, WORKSPACE_TAB_MAX_WIDTH)
    };
    let mut workspace_tab = tab(
        "header-view-tab".to_owned(),
        &format!("{name}, {}", section.label()),
        Some(&icons::FOLDER_OPEN),
        in_workspace,
        peer_graph,
        TabSeparators {
            left: false,
            right: !next_tab_active,
            hovered_key: state.hovered_header_tab.as_deref(),
            next_key: next_tab_key.as_deref(),
        },
        theme,
    )
    // Longer workspace labels can use extra room before the name fades.
    .max_w(workspace_tab_max_width)
    .min_w(0.0);
    if !compact {
        workspace_tab = workspace_tab.child(
            text(name)
                .min_w(0.0)
                .shrink()
                .fade_truncate()
                .help_when_truncated(name.to_owned())
                .font(ui_font(13.0, 500.0))
                .color(theme.color("#e2e2e2")),
        );
    }
    if !narrow {
        workspace_tab = workspace_tab.child(
            text(section.label())
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#aaaaaa")),
        );
    }
    let workspace_tab = workspace_tab.on_press(move |state: &mut State| {
        state.research_workspace_view = Some((id, section));
        state.navigate_to(ViewLocation::Workspace(id, section));
    });
    let mut tabs = row()
        .key("header-research-tabs")
        .role("tablist")
        .label("Workspace and agents")
        .grow(1.0)
        .min_w(0.0)
        .shrink()
        .h_full()
        .center_y()
        .gap(2.0)
        .child(workspace_tab);
    if peer_graph {
        if let Some(graph) = beale_graphs::graph_definition(graph_id) {
            let roles: Vec<_> = graph
                .nodes
                .iter()
                .copied()
                .filter(|role| role.is_top_level())
                .collect();
            for (index, role) in roles.iter().copied().enumerate() {
                let role_id = role.id().to_owned();
                let next_key = roles
                    .get(index + 1)
                    .map(|next| format!("header-agent-tab-{}", next.id()));
                let next_active = !in_workspace
                    && roles
                        .get(index + 1)
                        .is_some_and(|next| selected_role.as_deref() == Some(next.id()));
                let label = role.label();
                let tab_label = role_tab_label(role, narrow);
                let status = state
                    .active_session
                    .as_ref()
                    .and_then(|detail| {
                        detail
                            .graph_agents
                            .iter()
                            .find(|agent| agent.role == role_id)
                    })
                    .map_or("idle", |agent| agent.status.as_str());
                let status_color = match status {
                    "working" => "#9ac3ed",
                    "queued" => "#d9a467",
                    "failed" => "#e07676",
                    _ => "#737373",
                };
                tabs = tabs.child(
                    tab(
                        format!("header-agent-tab-{role_id}"),
                        &format!("{label}, {status}"),
                        (!narrow).then_some(&icons::AGENT),
                        !in_workspace && selected_role.as_deref() == Some(role_id.as_str()),
                        true,
                        TabSeparators {
                            left: false,
                            right: !next_active,
                            hovered_key: state.hovered_header_tab.as_deref(),
                            next_key: next_key.as_deref(),
                        },
                        theme,
                    )
                    .child(
                        el().size(6.0, 6.0)
                            .radius(3.0)
                            .fill(theme.color(status_color)),
                    )
                    .child(
                        text(tab_label)
                            .min_w(0.0)
                            .shrink()
                            .fade_truncate()
                            .font(ui_font(13.0, 500.0))
                            .color(theme.color("#e2e2e2")),
                    )
                    .on_press(move |state: &mut State| {
                        state.session_agent_role = role_id.clone();
                        state.session_agent_tab_manual = true;
                        sync_model_for_role(state, &role_id);
                        state.clear_tool_detail();
                        state.navigate_to(ViewLocation::NewResearch);
                    }),
                );
            }
        }
    } else {
        tabs = tabs.child(
            tab(
                "header-session-tab".to_owned(),
                session_label,
                Some(if started {
                    &icons::AGENT
                } else {
                    &icons::NEW_RESEARCH
                }),
                !in_workspace,
                false,
                TabSeparators {
                    left: false,
                    right: true,
                    hovered_key: state.hovered_header_tab.as_deref(),
                    next_key: None,
                },
                theme,
            )
            .child(
                text(session_label)
                    .font(ui_font(13.0, 500.0))
                    .color(theme.color("#e2e2e2")),
            )
            .on_press(|state: &mut State| state.navigate_to(ViewLocation::NewResearch)),
        );
    }
    Some(tabs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppearanceTheme, TextSize, view, visual_preview};
    use beale_contracts::v1::{SessionStreamEvent, SessionStreamKind, SessionToolSummary};
    use rune::{Env, Runtime, TextEdit};

    fn click(runtime: &mut Runtime<State>, key: &str) {
        let (x, y) = runtime.find(key).unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
    }

    #[test]
    fn workspace_and_session_tabs_preserve_origin_draft_and_running_state() {
        for section in [
            WorkspaceTab::Surfaces,
            WorkspaceTab::Findings,
            WorkspaceTab::Rules,
            WorkspaceTab::Foundation,
        ] {
            let mut state = visual_preview::fixture("surfaces");
            let id = state.workspace_id.unwrap();
            state.workspace_tab = section;
            state.start_new_research();
            state.new_research_prompt = TextEdit::new("Review the synthetic example");
            assert_eq!(workspace(&state), Some((id, section)));
            let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
            runtime.frame(0.0);
            click(&mut runtime, "header-view-tab");
            assert_eq!(
                runtime.state.view_location(),
                ViewLocation::Workspace(id, section)
            );
            click(&mut runtime, "header-session-tab");
            assert_eq!(runtime.state.primary_view, PrimaryView::NewResearch);
            assert_eq!(
                runtime.state.new_research_prompt.text,
                "Review the synthetic example"
            );
            runtime.state.session_busy = true;
            runtime.state.session_pending_prompt = Some("Synthetic prompt".into());
            runtime.frame(0.0);
            assert!(
                runtime
                    .elements()
                    .iter()
                    .any(|item| item.text.as_deref() == Some("Main Agent"))
            );
            click(&mut runtime, "header-view-tab");
            click(&mut runtime, "header-session-tab");
            assert!(runtime.state.session_busy);
            assert_eq!(
                runtime.state.session_pending_prompt.as_deref(),
                Some("Synthetic prompt")
            );
        }
    }

    #[test]
    fn research_header_tabs_fit_all_themes_and_existing_sessions() {
        for theme in [
            AppearanceTheme::Light,
            AppearanceTheme::Dark,
            AppearanceTheme::Cream,
            AppearanceTheme::Midnight,
        ] {
            for scene in ["research", "session"] {
                let mut state = visual_preview::fixture(scene);
                state.appearance.theme = theme;
                state.appearance.text_size = TextSize::ExtraLarge;
                let mut runtime = Runtime::new(state, view, (840.0, 720.0), Env::new(true, 1.0));
                runtime.frame(0.0);
                let (area, _) = runtime.presented("header-title").unwrap();
                let (left, _) = runtime.presented("header-view-tab").unwrap();
                let (right, _) = runtime.presented("header-session-tab").unwrap();
                assert!(left[0] >= area[0] && left[0] + left[2] <= right[0]);
                assert!(
                    right[0] + right[2] <= area[0] + area[2] + 1.0,
                    "{scene}: {right:?}, {area:?}"
                );
                let label = if scene == "session" {
                    "Main Agent"
                } else {
                    "New Research"
                };
                assert!(
                    runtime
                        .elements()
                        .iter()
                        .any(|item| item.text.as_deref() == Some(label))
                );
                click(&mut runtime, "header-view-tab");
                click(&mut runtime, "header-session-tab");
                assert_eq!(runtime.state.primary_view, PrimaryView::NewResearch);
                assert_eq!(runtime.state.active_session.is_some(), scene == "session");
            }
        }
    }

    #[test]
    fn pipeline_roles_have_distinct_navigable_tabs() {
        let mut state = visual_preview::fixture("session");
        state.active_session.as_mut().unwrap().graph_id = beale_graphs::OPENAI_ID.to_owned();
        let workspace_id = state.workspace_id.unwrap();
        let mut runtime = Runtime::new(state, view, (1100.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        for role in [
            beale_graphs::Role::Discovery,
            beale_graphs::Role::Validation,
            beale_graphs::Role::Reporter,
            beale_graphs::Role::PatchGeneration,
        ] {
            assert!(
                runtime
                    .find(&format!("header-agent-tab-{}", role.id()))
                    .is_some()
            );
        }
        assert!(runtime.find("header-session-tab").is_none());
        click(&mut runtime, "header-agent-tab-validation");
        assert_eq!(runtime.state.session_agent_role, "validation");
        assert_eq!(runtime.state.primary_view, PrimaryView::NewResearch);
        click(&mut runtime, "header-view-tab");
        assert_eq!(runtime.state.primary_view, PrimaryView::Workspace);
        click(&mut runtime, "header-agent-tab-reporter");
        assert_eq!(runtime.state.session_agent_role, "reporter");
        assert_eq!(runtime.state.workspace_id, Some(workspace_id));
    }

    #[test]
    fn two_stage_graph_has_one_tab_per_peer() {
        let mut state = visual_preview::fixture("session");
        state.active_session.as_mut().unwrap().graph_id = beale_graphs::CLOUDFLARE_ID.to_owned();
        let mut runtime = Runtime::new(state, view, (840.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("header-agent-tab-discovery_harness").is_some());
        assert!(runtime.find("header-agent-tab-validation_system").is_some());
        assert!(runtime.find("header-session-tab").is_none());
        click(&mut runtime, "header-agent-tab-validation_system");
        assert_eq!(runtime.state.session_agent_role, "validation_system");
    }

    #[test]
    fn pipeline_tabs_show_their_own_live_tools() {
        let mut state = visual_preview::fixture("session");
        state.active_session.as_mut().unwrap().graph_id = beale_graphs::OPENAI_ID.to_owned();
        state.session_busy = true;
        for role in ["discovery", "validation"] {
            state.session_live_events.push(SessionStreamEvent {
                kind: SessionStreamKind::ToolStarted as i32,
                agent_role: role.to_owned(),
                tool: Some(SessionToolSummary {
                    id: format!("{role}-example"),
                    name: "read".to_owned(),
                    summary: "Read example.txt".to_owned(),
                    is_error: false,
                    agent_role: role.to_owned(),
                }),
                ..Default::default()
            });
        }
        let mut runtime = Runtime::new(state, view, (1100.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        click(&mut runtime, "header-agent-tab-discovery");
        assert!(runtime.find("session-tool-discovery-example").is_some());
        assert!(runtime.find("session-tool-validation-example").is_none());
        click(&mut runtime, "header-agent-tab-validation");
        assert!(runtime.find("session-tool-discovery-example").is_none());
        assert!(runtime.find("session-tool-validation-example").is_some());
    }

    #[test]
    fn compact_pipeline_tabs_fit_header() {
        for theme in [
            AppearanceTheme::Light,
            AppearanceTheme::Dark,
            AppearanceTheme::Cream,
            AppearanceTheme::Midnight,
        ] {
            let mut state = visual_preview::fixture("graph-session");
            state.appearance.theme = theme;
            let mut runtime = Runtime::new(state, view, (840.0, 720.0), Env::new(true, 1.0));
            runtime.frame(0.0);
            let (header, _) = runtime.presented("header-title").unwrap();
            for role in ["discovery", "validation", "reporter", "patch_generation"] {
                let (area, _) = runtime
                    .presented(&format!("header-agent-tab-{role}"))
                    .unwrap();
                assert!(
                    area[0] >= header[0] && area[0] + area[2] <= header[0] + header[2] + 1.0,
                    "{theme:?} {role}: {area:?} outside {header:?}"
                );
            }
        }
    }

    #[test]
    fn header_tabs_expand_then_shrink_with_tab_count() {
        let mut sparse = Runtime::new(
            visual_preview::fixture("session"),
            view,
            (1100.0, 720.0),
            Env::new(true, 1.0),
        );
        sparse.frame(0.0);
        let (sparse_workspace, _) = sparse.presented("header-view-tab").unwrap();
        let (sparse_session, _) = sparse.presented("header-session-tab").unwrap();
        assert!(sparse.find("header-view-tab-left-separator").is_none());
        assert!(sparse_workspace[2] >= 200.0);
        assert!(sparse_session[2] >= 200.0);

        let mut crowded = Runtime::new(
            visual_preview::fixture("graph-session"),
            view,
            (1100.0, 720.0),
            Env::new(true, 1.0),
        );
        crowded.frame(0.0);
        let (header, _) = crowded.presented("header-title").unwrap();
        let (crowded_workspace, _) = crowded.presented("header-view-tab").unwrap();
        assert!(crowded_workspace[2] < sparse_workspace[2]);
        for role in ["discovery", "validation", "reporter", "patch_generation"] {
            let (area, _) = crowded
                .presented(&format!("header-agent-tab-{role}"))
                .unwrap();
            assert!(area[0] >= header[0]);
            assert!(area[0] + area[2] <= header[0] + header[2] + 1.0);
        }
    }

    #[test]
    fn workspace_tab_only_expands_for_long_names() {
        let mut short = visual_preview::fixture("session");
        for workspace in &mut short.workspaces {
            workspace.name = "Example".to_owned();
        }
        let mut short = Runtime::new(short, view, (1100.0, 720.0), Env::new(true, 1.0));
        short.frame(0.0);
        let (short_tab, _) = short.presented("header-view-tab").unwrap();
        assert!(short_tab[2] <= DEFAULT_TAB_MAX_WIDTH + 1.0);

        let mut long = visual_preview::fixture("session");
        for workspace in &mut long.workspaces {
            workspace.name = "Example research workspace with a long name".to_owned();
        }
        let mut long = Runtime::new(long, view, (1100.0, 720.0), Env::new(true, 1.0));
        long.frame(0.0);
        let (long_tab, _) = long.presented("header-view-tab").unwrap();
        let (session_tab, _) = long.presented("header-session-tab").unwrap();
        assert!(long_tab[2] > short_tab[2]);
        assert!(long_tab[2] <= WORKSPACE_TAB_MAX_WIDTH + 1.0);
        assert!(long_tab[0] + long_tab[2] <= session_tab[0]);
    }

    #[test]
    fn hovered_inactive_tab_hides_adjacent_separators() {
        let mut runtime = Runtime::new(
            visual_preview::fixture("graph-session"),
            view,
            (1100.0, 720.0),
            Env::new(true, 1.0),
        );
        runtime.frame(0.0);
        assert!(
            runtime
                .find("header-agent-tab-validation-right-separator")
                .is_some()
        );
        assert!(
            runtime
                .find("header-agent-tab-reporter-right-separator")
                .is_some()
        );
        let (x, y) = runtime.find("header-agent-tab-reporter").unwrap();
        runtime.pointer_move(x, y);
        runtime.frame(0.0);
        runtime.frame(0.2);
        let elements = runtime.elements();
        let active = elements
            .iter()
            .find(|item| item.key.as_deref() == Some("header-agent-tab-discovery"))
            .unwrap();
        let hovered = elements
            .iter()
            .find(|item| item.key.as_deref() == Some("header-agent-tab-reporter"))
            .unwrap();
        assert!(hovered.hover > 0.0);
        assert_eq!(hovered.radius, active.radius);
        assert!(
            runtime
                .find("header-agent-tab-validation-right-separator")
                .is_none()
        );
        assert!(
            runtime
                .find("header-agent-tab-reporter-right-separator")
                .is_none()
        );
        assert!(
            runtime
                .find("header-agent-tab-patch_generation-right-separator")
                .is_some()
        );
        runtime.pointer_move(0.0, 0.0);
        runtime.frame(0.0);
        assert!(
            runtime
                .find("header-agent-tab-validation-right-separator")
                .is_some()
        );
        assert!(
            runtime
                .find("header-agent-tab-reporter-right-separator")
                .is_some()
        );
    }

    #[test]
    fn live_handoffs_follow_the_active_role_until_a_tab_is_selected() {
        let mut state = visual_preview::fixture("graph-session");
        state.session_busy = true;
        state.session_generation = 1;
        state.apply_session_progress(
            1,
            SessionStreamEvent {
                kind: SessionStreamKind::MessageDelta as i32,
                text: "Example validation update".to_owned(),
                agent_role: "validation".to_owned(),
                segment_index: 1,
                ..Default::default()
            },
        );
        assert_eq!(state.session_agent_role, "validation");
        state.session_agent_tab_manual = true;
        state.apply_session_progress(
            1,
            SessionStreamEvent {
                kind: SessionStreamKind::MessageDelta as i32,
                text: "Example report update".to_owned(),
                agent_role: "reporter".to_owned(),
                segment_index: 2,
                ..Default::default()
            },
        );
        assert_eq!(state.session_agent_role, "validation");
    }
}
