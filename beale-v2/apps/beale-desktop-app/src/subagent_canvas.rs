use beale_contracts::v1::{SessionStreamKind, SessionSubagentDetail, SessionSubagentSummary};
use rune::{El, ScrollSpec, col, el, icon, row, text};

use crate::{
    AUTH_HANDLE, CONTENT_SURFACE_COLOR, State, ThemeStyle,
    icons::{BRAIN, CLOSE},
    ui_font,
};

pub(crate) struct SubagentCanvas {
    pub session_id: String,
    pub summary: SessionSubagentSummary,
    pub detail: Option<SessionSubagentDetail>,
    pub loading: bool,
    pub error: String,
    pub generation: u64,
}

pub(crate) fn open(state: &mut State, summary: SessionSubagentSummary) {
    state.canvas_surface.deactivate();
    let Some(session_id) = state
        .active_session
        .as_ref()
        .and_then(|detail| detail.summary.as_ref())
        .map(|summary| summary.id.clone())
    else {
        return;
    };
    let reused = state
        .canvas_subagent
        .as_ref()
        .is_some_and(|view| view.session_id == session_id && view.summary.id == summary.id);
    if !reused {
        state.canvas_subagent = Some(SubagentCanvas {
            session_id,
            summary,
            detail: None,
            loading: false,
            error: String::new(),
            generation: 0,
        });
        state.clear_tool_detail();
    }
    state.canvas_open = true;
    state.session_work_expanded.insert(usize::MAX);
    state.session_sidenav_visible = false;
    state.session_usage_menu = None;
    state.session_usage_row_hovered = None;
    state.session_usage_popup_hovered = false;
    refresh(state);
}

pub(crate) fn refresh(state: &mut State) {
    let Some(view) = state.canvas_subagent.as_mut() else {
        return;
    };
    if let Some(saved) = state.active_session.as_ref().and_then(|detail| {
        detail
            .subagents
            .iter()
            .find(|item| item.id == view.summary.id)
    }) {
        view.summary = saved.clone();
    }
    if view.summary.status == "active" || view.loading || view.detail.is_some() {
        return;
    }
    load_page(state, 0);
}

fn load_more(state: &mut State) {
    let Some(view) = state.canvas_subagent.as_ref() else {
        return;
    };
    if view.loading {
        return;
    }
    let Some(detail) = view.detail.as_ref() else {
        return;
    };
    if detail.next_offset >= detail.total_items {
        return;
    }
    load_page(state, detail.next_offset);
}

fn load_page(state: &mut State, offset: u64) {
    let Some(view) = state.canvas_subagent.as_mut() else {
        return;
    };
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        view.error = "Desktop connection is not ready.".to_owned();
        return;
    };
    view.generation = view.generation.wrapping_add(1);
    let generation = view.generation;
    let session_id = view.session_id.clone();
    let agent_id = view.summary.id.clone();
    view.loading = true;
    view.error.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = crate::connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = crate::sessions::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize scan connection".to_owned())?;
            runtime.block_on(crate::sessions::load_subagent(
                &client,
                addr,
                &session_id,
                &agent_id,
                offset,
            ))
        })();
        handle.update(move |state, _| {
            let Some(view) = state.canvas_subagent.as_mut() else {
                return;
            };
            if view.generation != generation
                || view.summary.id != agent_id
                || view.session_id != session_id
            {
                return;
            }
            view.loading = false;
            match result {
                Ok(detail) => {
                    view.summary = detail.summary.clone().unwrap_or(view.summary.clone());
                    if offset == 0 {
                        view.detail = Some(detail);
                    } else if let Some(current) = view.detail.as_mut() {
                        current.items.extend(detail.items);
                        current.next_offset = detail.next_offset;
                        current.total_items = detail.total_items;
                        if !detail.result.is_empty() {
                            current.result = detail.result;
                        }
                    }
                    view.error.clear();
                }
                Err(error) => view.error = error,
            }
        });
    });
}

pub(crate) fn tab_bar(state: &State, theme: ThemeStyle<'_>) -> Option<El> {
    let view = state.canvas_subagent.as_ref()?;
    let role = beale_swarms::ProfileId::from_id(&view.summary.role)
        .map_or(view.summary.role.as_str(), |role| role.label())
        .to_owned();
    Some(
        row()
            .key("subagent-canvas-tab-bar")
            .w_full()
            .h(crate::runbook_canvas::TAB_BAR_HEIGHT)
            .px(12.0)
            .center_y()
            .fill(theme.color(CONTENT_SURFACE_COLOR))
            .child(
                row()
                    .key("subagent-canvas-tab")
                    .h(34.0)
                    .px(9.0)
                    .center_y()
                    .gap(7.0)
                    .radius(11.0)
                    .fill(theme.color("#303030"))
                    .border(1.0, theme.color("#555555"))
                    .child(icon(&BRAIN, 16.0).color(theme.color("#d8d8d8")))
                    .child(
                        text(role)
                            .font(ui_font(13.0, 500.0))
                            .color(theme.color("#eeeeee")),
                    )
                    .child(
                        el().key("subagent-canvas-close")
                            .role("button")
                            .label("Close subagent view")
                            .size(20.0, 20.0)
                            .center()
                            .radius(6.0)
                            .hover(theme.color("#484848"))
                            .on_press(|state: &mut State| {
                                state.canvas_subagent = None;
                                state.canvas_open = state.canvas_runbook.has_views();
                                if !state.canvas_open {
                                    state.session_sidenav_visible = true;
                                }
                                state.clear_tool_detail();
                            })
                            .child(icon(&CLOSE, 12.0).color(theme.color("#a6a6a6"))),
                    ),
            ),
    )
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, height: f32) -> El {
    let Some(view) = state.canvas_subagent.as_ref() else {
        return col().w_full().h(height);
    };
    let mut transcript = col()
        .key("subagent-transcript")
        .w_full()
        .max_w(750.0)
        .self_center()
        .gap(14.0)
        .px(14.0)
        .pt(22.0)
        .pb(24.0)
        .child(crate::new_research::transcript_message(
            "subagent-task".to_owned(),
            view.detail
                .as_ref()
                .map_or(view.summary.task.clone(), |detail| detail.task.clone()),
            true,
            false,
            theme,
        ));
    let active = view.summary.status == "active";
    transcript = transcript.child(crate::new_research::work_header(
        "subagent-work-header".to_owned(),
        usize::MAX,
        0,
        active,
        state.session_work_expanded.contains(&usize::MAX),
        theme,
    ));
    if let Some(detail) = &view.detail
        && state.session_work_expanded.contains(&usize::MAX)
    {
        let mut grouped_through = 0;
        for (index, item) in detail.items.iter().enumerate() {
            if index < grouped_through {
                continue;
            }
            transcript = match item.kind.as_str() {
                "message" => transcript.child(crate::new_research::transcript_message(
                    format!("subagent-message-{index}"),
                    item.text.clone(),
                    false,
                    false,
                    theme,
                )),
                "reasoning" => transcript.child(crate::new_research::transcript_reasoning(
                    item.text.clone(),
                    format!("subagent-reasoning-{index}"),
                    false,
                    theme,
                )),
                "tool" => match item.tool.as_ref() {
                    Some(tool) => {
                        let mut end = index + 1;
                        if crate::new_research::series_tool(tool) {
                            while let Some(next) = detail.items.get(end) {
                                if next.kind != "tool"
                                    || next.tool.as_ref().is_none_or(|next_tool| {
                                        !crate::new_research::series_tool(next_tool)
                                            || next_tool.agent_role != tool.agent_role
                                    })
                                {
                                    break;
                                }
                                end += 1;
                            }
                        }
                        grouped_through = end;
                        let series: Vec<_> = detail.items[index..end]
                            .iter()
                            .filter_map(|item| item.tool.as_ref())
                            .collect();
                        transcript.child(crate::new_research::transcript_tool_series(
                            &series, false, true, state, theme,
                        ))
                    }
                    None => transcript,
                },
                _ => transcript,
            };
        }
        if !detail.result.is_empty()
            && !detail
                .items
                .iter()
                .rev()
                .find(|item| item.kind == "message")
                .is_some_and(|item| item.text.trim() == detail.result.trim())
        {
            transcript = transcript.child(crate::new_research::transcript_message(
                "subagent-result".to_owned(),
                detail.result.clone(),
                false,
                false,
                theme,
            ));
        }
        if detail.next_offset < detail.total_items {
            transcript = transcript.child(
                el().key("subagent-load-more")
                    .role("button")
                    .label("Load more subagent activity")
                    .h(32.0)
                    .px(10.0)
                    .center_y()
                    .radius(8.0)
                    .hover(theme.color("#303030"))
                    .on_press(load_more)
                    .child(
                        text("Load more activity")
                            .font(ui_font(12.0, 450.0))
                            .color(theme.color("#b8b8b8")),
                    ),
            );
        }
    } else {
        let events: Vec<_> = state
            .session_live_events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.subagent_id == view.summary.id)
            .collect();
        let mut grouped_through = 0;
        for (position, &(index, event)) in events.iter().enumerate() {
            if position < grouped_through {
                continue;
            }
            transcript = match SessionStreamKind::try_from(event.kind) {
                Ok(SessionStreamKind::MessageDelta) => {
                    transcript.child(crate::new_research::transcript_message(
                        format!("subagent-live-message-{index}"),
                        event.text.clone(),
                        false,
                        true,
                        theme,
                    ))
                }
                Ok(SessionStreamKind::ReasoningDelta) => {
                    transcript.child(crate::new_research::transcript_reasoning(
                        event.text.clone(),
                        format!("subagent-live-reasoning-{index}"),
                        true,
                        theme,
                    ))
                }
                Ok(SessionStreamKind::ToolStarted | SessionStreamKind::ToolFinished) => {
                    match event.tool.as_ref() {
                        Some(tool) => {
                            let mut end = position + 1;
                            if crate::new_research::series_tool(tool) {
                                while let Some((_, next)) = events.get(end) {
                                    if !matches!(
                                        SessionStreamKind::try_from(next.kind),
                                        Ok(SessionStreamKind::ToolStarted
                                            | SessionStreamKind::ToolFinished)
                                    ) || next.tool.as_ref().is_none_or(|next_tool| {
                                        !crate::new_research::series_tool(next_tool)
                                            || next_tool.agent_role != tool.agent_role
                                    }) {
                                        break;
                                    }
                                    end += 1;
                                }
                            }
                            grouped_through = end;
                            let series: Vec<_> = events[position..end]
                                .iter()
                                .filter_map(|(_, event)| event.tool.as_ref())
                                .collect();
                            let active = events[position..end].iter().any(|(_, event)| {
                                event.kind == SessionStreamKind::ToolStarted as i32
                            });
                            transcript.child(crate::new_research::transcript_tool_series(
                                &series, active, false, state, theme,
                            ))
                        }
                        None => transcript,
                    }
                }
                _ => transcript,
            };
        }
    }
    if view.loading {
        transcript = transcript.child(
            text("Loading subagent transcript…")
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    if !view.error.is_empty() {
        transcript = transcript.child(
            text(view.error.clone())
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    col()
        .key("subagent-canvas-view")
        .w_full()
        .h(height)
        .px(20.0)
        .pb(6.0)
        .child(
            col()
                .key("subagent-transcript-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: true })
                .child(transcript),
        )
}
