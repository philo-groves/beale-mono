//! Session-scoped composition of the shared token dashboard components.
use std::collections::BTreeMap;

use beale_contracts::v1::{SessionDetail, SessionTokenUsage, SessionUsageSnapshot};
use rune::{El, col, el, row};

use crate::{
    State, ThemeStyle, design,
    icons::SESSION_TOKENS,
    modal_dialog,
    token_charts::{self, ActivityPoint, exact, muted},
    tracking_data::{AgentFilter, ModelTotal, Totals},
};

pub(crate) fn open(state: &mut State) {
    state.session_usage_menu = None;
    state.session_usage_row_hovered = None;
    state.session_usage_popup_hovered = false;
    state.session_insights_filter = AgentFilter::All;
    state.session_insights_open = true;
}

fn close(state: &mut State) {
    state.session_insights_open = false;
}

fn totals(usage: &SessionTokenUsage) -> Totals {
    Totals {
        input: usage.input_tokens,
        output: usage.output_tokens,
        calls: usage.total_calls,
        measured: usage.measured_calls,
        cached: usage.cache_read_tokens,
        cache_input: usage.cache_input_tokens,
        cache_calls: usage.cache_measured_calls,
    }
}

fn model_totals(detail: &SessionDetail, filter: AgentFilter) -> Vec<ModelTotal> {
    let mut models = BTreeMap::<(i32, i32, String), ModelTotal>::new();
    for source in detail
        .model_usage
        .iter()
        .filter(|source| filter.accepts(&source.role))
    {
        let Some(usage) = &source.usage else {
            continue;
        };
        let model = models
            .entry((source.provider, source.method, source.model.clone()))
            .or_insert_with(|| ModelTotal {
                provider: source.provider,
                method: source.method,
                model: source.model.clone(),
                ..Default::default()
            });
        model.roles.insert(source.role.clone());
        let value = totals(usage);
        model.total.input = model.total.input.saturating_add(value.input);
        model.total.output = model.total.output.saturating_add(value.output);
        model.total.calls = model.total.calls.saturating_add(value.calls);
        model.total.measured = model.total.measured.saturating_add(value.measured);
        model.total.cached = model.total.cached.saturating_add(value.cached);
        model.total.cache_input = model.total.cache_input.saturating_add(value.cache_input);
        model.total.cache_calls = model.total.cache_calls.saturating_add(value.cache_calls);
    }
    let mut models: Vec<_> = models.into_values().collect();
    models.sort_by(|a, b| {
        b.total
            .tokens()
            .cmp(&a.total.tokens())
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.provider.cmp(&b.provider))
            .then_with(|| a.method.cmp(&b.method))
    });
    models
}

fn work_label(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds >= 3600 {
        format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60)
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

fn activity_points(snapshots: &[SessionUsageSnapshot], filter: AgentFilter) -> Vec<ActivityPoint> {
    if snapshots.iter().any(|point| match filter {
        AgentFilter::All => false,
        AgentFilter::Main => point.main_usage.is_none(),
        AgentFilter::Subagents => point.subagent_usage.is_none(),
    }) {
        return Vec::new();
    }
    // Select actual cumulative samples, including both endpoints. Never interpolate usage.
    let count = snapshots.len().min(7);
    (0..count).map(|i| {
        let index = i * snapshots.len().saturating_sub(1) / count.saturating_sub(1).max(1);
        let point = &snapshots[index];
        let total = match filter {
            AgentFilter::Main => point.main_usage.as_ref().map(totals).unwrap_or_default(),
            AgentFilter::Subagents => point.subagent_usage.as_ref().map(totals).unwrap_or_default(),
            AgentFilter::All => Totals {
            input: point.input_tokens, output: point.output_tokens,
            calls: point.total_calls, measured: point.measured_calls,
            cached: point.cache_read_tokens, cache_input: point.cache_input_tokens,
            cache_calls: point.cache_measured_calls,
            },
        };
        let label = work_label(point.work_duration_ms);
        let description = if total.tokens_unreported() {
            format!("{label} active work: {} calls. Token counts were not reported.", total.calls)
        } else {
            format!("{label} active work: {} cumulative input, {} output, {} total tokens. {} of {} calls reported tokens.", exact(total.input), exact(total.output), exact(total.tokens()), total.measured, total.calls)
        };
        ActivityPoint { id: point.work_duration_ms, key: format!("session-insights-point-{i}"), label, description, total, selected: false }
    }).collect()
}

pub(crate) fn dialog(state: &State, theme: ThemeStyle<'_>, window_width: f32) -> [El; 2] {
    let width = (window_width - 32.0).clamp(280.0, 1120.0);
    let inner = width - 50.0;
    let live_detail = crate::sessions::detail_for_agent(
        state.active_session.as_ref(),
        &state.session_live_events,
        &state.session_agent_role,
        state.session_role_live_ms(),
    );
    let detail = live_detail.as_ref();
    let title = detail
        .and_then(|d| d.summary.as_ref())
        .map_or("Current scan", |s| s.title.as_str());
    let main_label = if detail.is_some_and(|detail| beale_graphs::is_peer_graph(&detail.graph_id)) {
        "Top-level"
    } else {
        "Main"
    };
    let mut body = col()
        .key("session-insights-body")
        .w_full()
        .min_w(0.0)
        .gap(20.0)
        .child(muted(title, theme).key("session-insights-session-title"))
        .child(design::tabs_with_control(
            token_charts::agent_filters(
                state.session_insights_filter,
                main_label,
                "session-insights",
                false,
                |state, filter| state.session_insights_filter = filter,
                theme,
            ),
            design::segmented_group_width(&AgentFilter::ALL.map(|filter| {
                design::segmented_tab_width(
                    if filter == AgentFilter::Main {
                        main_label
                    } else {
                        filter.label()
                    },
                    true,
                )
            })),
            design::badge("All recorded turns", theme),
            145.0,
            inner,
        ));
    let filter = state.session_insights_filter;
    let models = detail
        .map(|detail| model_totals(detail, filter))
        .unwrap_or_default();
    let total = if filter == AgentFilter::All {
        detail
            .and_then(|detail| detail.token_usage.as_ref())
            .map(totals)
            .unwrap_or_default()
    } else {
        models.iter().fold(Totals::default(), |mut sum, model| {
            let value = model.total;
            sum.input = sum.input.saturating_add(value.input);
            sum.output = sum.output.saturating_add(value.output);
            sum.calls = sum.calls.saturating_add(value.calls);
            sum.measured = sum.measured.saturating_add(value.measured);
            sum.cached = sum.cached.saturating_add(value.cached);
            sum.cache_input = sum.cache_input.saturating_add(value.cache_input);
            sum.cache_calls = sum.cache_calls.saturating_add(value.cache_calls);
            sum
        })
    };
    if let Some(detail) = detail.filter(|_| total.calls > 0) {
        let points = activity_points(&detail.usage_snapshots, filter);
        let trend: Vec<_> = points.iter().map(|p| p.total).collect();
        body = body.child(token_charts::metrics(total, &trend, inner, theme));
        let chart_width = if inner >= 900.0 {
            (inner - 16.0) * 0.66
        } else {
            inner
        };
        let activity = if points.is_empty() {
            token_charts::card(theme)
                .w(chart_width)
                .min_h(350.0)
                .child(design::empty_state(
                    &SESSION_TOKENS,
                    "Timeline unavailable",
                    "No activity samples were recorded for this selection.",
                    theme,
                ))
        } else {
            token_charts::activity_chart(
                &points,
                "Cumulative tokens · active work time (m:ss or h/m).",
                None,
                chart_width,
                theme,
            )
        };
        if inner >= 900.0 {
            body = body.child(row().w_full().gap(16.0).child(activity).child(
                token_charts::mix_chart(total, inner - 16.0 - chart_width, theme),
            ));
        } else {
            body = body
                .child(activity)
                .child(token_charts::mix_chart(total, inner, theme));
        }
        if models.is_empty() {
            body = body.child(token_charts::card(theme).child(muted(
                "Model breakdown is unavailable for this scan.",
                theme,
            )));
        } else {
            body = body.child(token_charts::model_breakdown(total, &models, inner, theme));
        }
        body = body.child(muted(format!("Reporting coverage · {} of {} calls include token counts. Cache telemetry is available for {} calls. Missing counts are excluded, not estimated.", total.measured, total.calls, total.cache_calls), theme));
    } else {
        let (heading, description) = if filter != AgentFilter::All
            && detail.is_some_and(|d| d.token_usage.as_ref().is_some_and(|u| u.total_calls > 0))
        {
            if detail.is_some_and(|d| d.model_usage.is_empty()) {
                (
                    "Agent breakdown unavailable",
                    "This scan has no recorded agent breakdown. Select All agents to see its totals.",
                )
            } else {
                (
                    "No calls for this agent group",
                    "This scan has no recorded calls for the selected group. Select All agents to see the scan totals.",
                )
            }
        } else {
            (
                "No recorded usage yet",
                "Token insights appear after this scan completes a model turn.",
            )
        };
        body = body.child(
            design::empty_state(&SESSION_TOKENS, heading, description, theme)
                .key("session-insights-empty"),
        );
    }
    body = body.child(muted(
        if state.session_busy {
            "A turn is running. These recorded totals update when it completes."
        } else {
            "Usage from this scan only. Cached tokens are included in input."
        },
        theme,
    ));
    let actions = row().w_full().child(el().grow(1.0)).child(
        crate::form_button("Done", theme)
            .key("session-insights-done")
            .on_press(close),
    );
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "session-insights-dialog",
            title: "Scan insights",
            width,
            focus_return: "session-sidenav-chart-button",
        },
        body,
        actions,
        theme,
        close,
    )
    .map(El::raise)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppearanceTheme, view, visual_preview};
    use rune::{Env, Key, KeyEvent, Runtime, WheelEvent};

    fn click(runtime: &mut Runtime<State>, key: &str) {
        let (x, y) = runtime.find(key).unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
    }

    #[test]
    fn insights_button_opens_session_only_modal_and_closes_without_hover_dismissal() {
        let mut state = visual_preview::fixture("session-insights");
        state.session_insights_open = false;
        let mut runtime = Runtime::new(state, view, (1440.0, 960.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        click(&mut runtime, "session-sidenav-chart-button");
        assert!(runtime.find("session-insights-dialog").is_some());
        assert!(runtime.find("session-usage-popup").is_none());
        assert!(
            runtime
                .elements()
                .into_iter()
                .any(|e| e.key.as_deref() == Some("tracking-metric-value-0")
                    && e.text.as_deref() == Some("35.0K"))
        );
        assert_eq!(
            model_totals(
                runtime.state.active_session.as_ref().unwrap(),
                AgentFilter::All
            )
            .len(),
            3
        );
        runtime.pointer_move(2.0, 2.0);
        runtime.frame(0.6);
        assert!(runtime.state.session_insights_open);
        runtime.update(|state, _| {
            let detail = state.active_session.as_mut().unwrap();
            detail.token_usage.as_mut().unwrap().input_tokens += 1_000;
            detail.model_usage[0].usage.as_mut().unwrap().input_tokens += 1_000;
            detail.usage_snapshots.last_mut().unwrap().input_tokens += 1_000;
        });
        runtime.frame(0.0);
        assert!(
            runtime
                .elements()
                .into_iter()
                .any(|e| e.key.as_deref() == Some("tracking-metric-value-0")
                    && e.text.as_deref() == Some("36.0K"))
        );
        runtime.key(KeyEvent {
            key: Key::Escape,
            mods: Default::default(),
        });
        runtime.frame(0.0);
        assert!(!runtime.state.session_insights_open);
        click(&mut runtime, "session-sidenav-chart-button");
        click(&mut runtime, "session-insights-done");
        assert!(!runtime.state.session_insights_open);
        open(&mut runtime.state);
        runtime.state.start_new_research();
        assert!(!runtime.state.session_insights_open);
    }

    #[test]
    fn agent_filters_scope_every_chart_and_reset_when_reopened() {
        let mut state = visual_preview::fixture("session-insights");
        state.tracking.filter = AgentFilter::Main;
        let mut runtime = Runtime::new(state, view, (1440.0, 960.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        for (filter, label, expected, count, rate) in [
            (AgentFilter::Main, "20.4K", 20_400, 1, "66.7%"),
            (AgentFilter::Subagents, "14.6K", 14_600, 2, "50.0%"),
            (AgentFilter::All, "35.0K", 35_000, 3, "60.0%"),
        ] {
            click(&mut runtime, &format!("session-insights-filter-{filter:?}"));
            assert_eq!(runtime.state.session_insights_filter, filter);
            assert_eq!(runtime.state.tracking.filter, AgentFilter::Main);
            let elements = runtime.elements();
            for (key, value) in [
                ("tracking-metric-value-0", label),
                ("tracking-metric-value-3", rate),
            ] {
                assert!(
                    elements
                        .iter()
                        .any(|e| e.key.as_deref() == Some(key) && e.text.as_deref() == Some(value)),
                    "{filter:?}: {key}"
                );
            }
            let detail = runtime.state.active_session.as_ref().unwrap();
            let points = activity_points(&detail.usage_snapshots, filter);
            assert_eq!(points.last().unwrap().total.tokens(), expected);
            assert_eq!(points.last().unwrap().id, detail.total_duration_ms);
            assert_eq!(model_totals(detail, filter).len(), count);
            assert!(
                runtime
                    .find(&format!("tracking-model-row-{}", count - 1))
                    .is_some()
            );
            assert!(
                runtime
                    .find(&format!("tracking-model-row-{count}"))
                    .is_none()
            );
        }
        click(&mut runtime, "session-insights-filter-Subagents");
        runtime.update(|state, _| {
            let detail = state.active_session.as_mut().unwrap();
            detail.model_usage[1].usage.as_mut().unwrap().input_tokens += 1_000;
            detail.token_usage.as_mut().unwrap().input_tokens += 1_000;
            let last = detail.usage_snapshots.last_mut().unwrap();
            last.input_tokens += 1_000;
            last.subagent_usage.as_mut().unwrap().input_tokens += 1_000;
        });
        runtime.frame(0.0);
        assert_eq!(
            runtime.state.session_insights_filter,
            AgentFilter::Subagents
        );
        assert!(
            runtime
                .elements()
                .iter()
                .any(|e| e.key.as_deref() == Some("tracking-metric-value-0")
                    && e.text.as_deref() == Some("15.6K"))
        );
        click(&mut runtime, "session-insights-done");
        click(&mut runtime, "session-sidenav-chart-button");
        assert_eq!(runtime.state.session_insights_filter, AgentFilter::All);
    }

    #[test]
    fn peer_graph_insights_name_top_level_agents() {
        let mut state = visual_preview::fixture("session-insights");
        state.session_agent_role = "discovery".to_owned();
        let detail = state.active_session.as_mut().unwrap();
        detail.graph_id = beale_graphs::OPENAI_ID.to_owned();
        detail.agent_metrics = vec![beale_contracts::v1::SessionAgentMetrics {
            role: "discovery".to_owned(),
            token_usage: detail.token_usage.clone(),
            model_usage: detail.model_usage.clone(),
            usage_snapshots: detail.usage_snapshots.clone(),
            duration_ms: detail.total_duration_ms,
        }];
        let mut runtime = Runtime::new(state, view, (1200.0, 800.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(
            runtime
                .elements()
                .iter()
                .any(|item| item.text.as_deref() == Some("Top-level"))
        );
        click(&mut runtime, "session-insights-filter-Main");
        assert_eq!(runtime.state.session_insights_filter, AgentFilter::Main);
    }

    #[test]
    fn filtered_empty_missing_and_legacy_usage_remain_distinct() {
        let state = visual_preview::fixture("session-insights-no-subagents");
        let mut runtime = Runtime::new(state, view, (1440.0, 960.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(
            runtime
                .elements()
                .iter()
                .any(|e| e.text.as_deref() == Some("No calls for this agent group"))
        );
        click(&mut runtime, "session-insights-filter-Main");
        assert!(runtime.find("tracking-metrics").is_some());
        runtime.update(|state, _| {
            *state = visual_preview::fixture("session-insights-missing");
            state.session_insights_filter = AgentFilter::Subagents;
        });
        runtime.frame(0.0);
        assert!(runtime.find("tracking-metrics").is_some());
        assert!(
            runtime
                .elements()
                .iter()
                .any(|e| e.key.as_deref() == Some("tracking-metric-value-0")
                    && e.text.as_deref() == Some("—"))
        );
        runtime.update(|state, _| {
            *state = visual_preview::fixture("session-insights");
            state.session_insights_filter = AgentFilter::Main;
            for point in &mut state.active_session.as_mut().unwrap().usage_snapshots {
                point.main_usage = None;
                point.subagent_usage = None;
            }
        });
        runtime.frame(0.0);
        assert!(
            runtime
                .elements()
                .iter()
                .any(|e| e.text.as_deref() == Some("Timeline unavailable"))
        );
        assert!(runtime.find("tracking-metrics").is_some());
        runtime.update(|state, _| state.active_session.as_mut().unwrap().model_usage.clear());
        runtime.frame(0.0);
        assert!(
            runtime
                .elements()
                .iter()
                .any(|e| e.text.as_deref() == Some("Agent breakdown unavailable"))
        );
        click(&mut runtime, "session-insights-filter-All");
        assert!(runtime.find("tracking-metrics").is_some());
    }

    #[test]
    fn insights_modal_keeps_controls_and_shared_cards_within_bounds() {
        for theme in [
            AppearanceTheme::Light,
            AppearanceTheme::Dark,
            AppearanceTheme::Cream,
            AppearanceTheme::Midnight,
        ] {
            for (width, height, fixture) in [
                (1440.0, 960.0, "session-insights"),
                (840.0, 600.0, "session-insights-compact"),
            ] {
                let mut state = visual_preview::fixture(fixture);
                state.appearance.theme = theme;
                let mut runtime = Runtime::new(state, view, (width, height), Env::new(true, 1.0));
                runtime.frame(0.0);
                let (body, _) = runtime.presented("session-insights-body").unwrap();
                for key in ["session-insights-dialog-close", "session-insights-done"] {
                    let (r, _) = runtime.presented(key).unwrap();
                    assert!(
                        r[0] >= 16.0
                            && r[1] >= 16.0
                            && r[0] + r[2] <= width - 16.0
                            && r[1] + r[3] <= height - 16.0,
                        "{key}: {r:?}"
                    );
                }
                for key in [
                    "session-insights-agent-filters",
                    "session-insights-filter-All",
                    "session-insights-filter-Main",
                    "session-insights-filter-Subagents",
                    "tracking-metrics",
                    "tracking-model-card",
                    "tracking-mix-card",
                    "tracking-model-breakdown",
                ] {
                    let (r, _) = runtime.presented(key).unwrap();
                    assert!(
                        r[0] >= body[0] - 1.0 && r[0] + r[2] <= body[0] + body[2] + 1.0,
                        "{key}: {r:?}, body {body:?}"
                    );
                }
                let (done, _) = runtime.presented("session-insights-done").unwrap();
                runtime.wheel_event(
                    WheelEvent::new(0.0, -5000.0)
                        .at(body[0] + body[2] / 2.0, height / 2.0)
                        .precise(true),
                );
                runtime.frame(0.0);
                let (last_model, _) = runtime.presented("tracking-model-row-2").unwrap();
                assert!(
                    last_model[1] > 60.0 && last_model[1] + last_model[3] < done[1],
                    "model breakdown remains reachable by scrolling: {last_model:?}"
                );
                assert_eq!(runtime.presented("session-insights-done").unwrap().0, done);
            }
        }
    }

    #[test]
    fn session_models_merge_roles_and_keep_sources_and_cache_coverage_separate() {
        let mut state = visual_preview::fixture("session-insights");
        let detail = state.active_session.as_mut().unwrap();
        let mut extra = detail.model_usage[0].clone();
        extra.role = "scout".into();
        let usage = extra.usage.as_mut().unwrap();
        usage.cache_read_tokens = 0;
        usage.cache_input_tokens = 0;
        usage.cache_measured_calls = 0;
        detail.model_usage.push(extra.clone());
        extra.method = 2;
        detail.model_usage.push(extra);
        let models = model_totals(detail, AgentFilter::All);
        assert_eq!(models.len(), 4);
        assert_eq!(models[0].total.input, 36_000);
        assert_eq!(models[0].total.cache_input, 18_000);
        assert_eq!(models[0].total.cache_calls, 9);
        assert_eq!(models[0].roles.len(), 2);
        let points = activity_points(&detail.usage_snapshots, AgentFilter::All);
        assert_eq!(points.first().unwrap().id, 0);
        assert_eq!(points.last().unwrap().total.tokens(), 35_000);
        assert!(activity_points(&[], AgentFilter::All).is_empty());
    }
}
