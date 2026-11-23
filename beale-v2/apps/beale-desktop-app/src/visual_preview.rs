//! Offline synthetic screenshot fixtures. Never reads host data or starts app-server polling.
use crate::*;
use beale_contracts::v1::*;

fn preview_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

pub(crate) fn enabled() -> bool {
    std::env::var_os("BEALE_UI_PREVIEW").is_some() && std::env::args().any(|arg| arg == "--snap")
}

pub(crate) fn initial_state() -> State {
    if !enabled() {
        return State::load_appearance();
    }
    let name = std::env::var("BEALE_UI_PREVIEW").unwrap_or_default();
    let mut s = fixture(&name);
    s.appearance.theme = match std::env::var("BEALE_UI_THEME").as_deref() {
        Ok("light") => AppearanceTheme::Light,
        Ok("cream") => AppearanceTheme::Cream,
        Ok("midnight") => AppearanceTheme::Midnight,
        _ => AppearanceTheme::Dark,
    };
    s.sync_color_edits();
    if name == "picker" {
        s.color_picker = None;
        toggle_color_picker(&mut s, ColorTarget::Heat(HeatLevel::Low));
    }
    s
}

pub(crate) fn fixture(variant: &str) -> State {
    let name = match variant {
        "home-collapsed" => "home",
        "home-usage" => "home",
        "session-insights"
        | "session-insights-empty"
        | "session-insights-missing"
        | "session-insights-compact"
        | "session-insights-running"
        | "session-insights-main"
        | "session-insights-subagents"
        | "session-insights-no-subagents" => "session",
        "surfaces-empty"
        | "surfaces-review"
        | "surfaces-search"
        | "surfaces-dense"
        | "surfaces-filter"
        | "surfaces-large"
        | "surfaces-web"
        | "surfaces-web-overflow"
        | "surfaces-web-scope"
        | "surfaces-web-focused"
        | "surface-connections" => "surfaces",
        "archive-empty" => "archive",
        "archive-chats-empty" => "archive-chats",
        "tracking-selected"
        | "tracking-subagents"
        | "tracking-empty"
        | "tracking-legacy"
        | "tracking-unmeasured"
        | "tracking-compact"
        | "tracking-loading"
        | "tracking-offline"
        | "tracking-error"
        | "tracking-headroom" => "tracking",
        "appearance-compact" => "appearance",
        "typography-compact" => "typography",
        "anthropic" | "openrouter" | "xai" | "zai" | "api-key" => "providers",
        "custom-editor" => "custom",
        "graph-editor" | "model-menu" | "goal-menu" | "graph-cloudflare" | "graph-openai"
        | "graph-sequential" => "research",
        "graph-session" | "session-activity" | "subagent-canvas" | "subagent-popup" => "session",
        "surface-add" | "surface-delete" | "relationship" => "surface-manage",
        "workspace-name" => "workspace",
        "workspace-rule" => "rules",
        "guidance-agents" => "rules",
        "feature-detail" => "features",
        "plugin-add" | "plugin-detail" => "plugins",
        "fleet-connect" | "fleet-import" | "fleet-detail" => "fleet",
        "safety-rule" => "safety",
        "finding-detail"
        | "finding-create"
        | "findings-plot"
        | "findings-plot-empty"
        | "findings-plot-unrated"
        | "findings-plot-cluster"
        | "findings-plot-large"
        | "findings-plot-filtered"
        | "findings-plot-hidden" => "findings",
        _ => variant,
    };
    let mut s = State {
        primary_view: PrimaryView::Home,
        connection: ConnectionStatus::Connected {
            major: beale_contracts::PROTOCOL_MAJOR,
            minor: beale_contracts::PROTOCOL_MINOR,
        },
        ..Default::default()
    };
    s.fleet.preview_fixture();
    s.lead_provider = Some(Provider::OpenAi);
    s.auth_loaded = true;
    s.workspace_list_loaded = true;
    s.archive_loaded = true;
    s.permission_loaded = true;
    for feature in &mut s.features {
        feature.enabled = true;
    }
    for (i, name) in ["ExampleCo / Application", "ExampleCo / Infrastructure"]
        .iter()
        .enumerate()
    {
        s.workspaces.push(WorkspaceSummary {
            id: format!("00000000-0000-4000-8000-{:012}", i + 1),
            name: name.to_string(),
            directory: format!("/example/research/workspace-{}", i + 1),
        });
    }
    let workspace = s.workspaces[0].clone();
    s.workspace_id = Some(uuid::Uuid::parse_str(&workspace.id).unwrap());
    s.new_research_workspace_id = Some(workspace.id.clone());
    s.workspace_name_edit = TextEdit::new(&workspace.name);
    let guidance = "Investigate the example application within its recorded scope. Keep observations, assumptions, and evidence separate. Record reproducible results in runbooks.";
    s.workspace_guidance_edit = TextEdit::new(guidance);
    s.workspace_rule_edits = vec![
        "Research is authorized only for example.test and its local synthetic fixtures.".into(),
        "Third-party services and production user data are out of scope.".into(),
    ];
    s.workspace_details = Some(WorkspaceDetails {
        summary: Some(workspace.clone()),
        guidance: guidance.into(),
        rules: s.workspace_rule_edits.clone(),
        ..Default::default()
    });
    s.workspace_draft.name = TextEdit::new("ExampleCo / New assessment");
    s.workspace_draft.directory = TextEdit::new("/example/research/new-assessment");
    s.workspace_draft.guidance = TextEdit::new(guidance);
    s.auth_status[Provider::OpenAi.index()].api_key = true;
    s.model_catalogs[catalog_index(Provider::OpenAi, AuthMethod::ApiKey)] = Some(ModelCatalog {
        models: vec![
            provider_models::ModelEntry {
                id: "example-lead".into(),
                name: "Example Research Model".into(),
            },
            provider_models::ModelEntry {
                id: "example-small".into(),
                name: "Example Review Model".into(),
            },
        ],
        lead: "example-lead".into(),
        small: "example-small".into(),
    });
    for (index, title) in [
        "Map the example trust boundary",
        "Review request validation",
        "Reproduce the candidate behavior",
    ]
    .iter()
    .enumerate()
    {
        s.session_summaries.push(SessionSummary {
            id: format!("10000000-0000-4000-8000-{:012}", index + 1),
            workspace_id: workspace.id.clone(),
            title: title.to_string(),
            last_message_at_ms: preview_now_ms().saturating_sub((index as u64 * 5 + 2) * 3_600_000),
        });
    }
    for (index, (title, instruction)) in [
        (
            "Authorized scope",
            "Check operations against the recorded workspace boundary.",
        ),
        (
            "Destructive changes",
            "Review broad or irreversible changes before execution.",
        ),
        (
            "Credential access",
            "Deny access to credentials unrelated to the research task.",
        ),
    ]
    .iter()
    .enumerate()
    {
        s.permission_rules.push(PermissionRule {
            id: format!("example-rule-{index}"),
            title: title.to_string(),
            instruction: instruction.to_string(),
            enabled: true,
            stage: ReviewStage::AutoReview as i32,
        });
    }
    s.permission_rules.push(PermissionRule {
        id: "example-guardian".into(),
        title: "Review denied operations".into(),
        instruction:
            "Reassess the denied call using its purpose, recorded scope, and available evidence."
                .into(),
        enabled: true,
        stage: ReviewStage::Guardian as i32,
    });
    for quick_chat in [false, true] {
        s.archive_entries.push(ArchiveEntry {
            id: format!("example-archive-{quick_chat}"),
            workspace_id: workspace.id.clone(),
            title: "Example assessment notes".into(),
            model: "Example Research Model".into(),
            turn_count: 8,
            created_at_ms: 1_800_000_000_000,
            archived_at_ms: 1_800_086_400_000,
            quick_chat,
            ..Default::default()
        });
    }
    for (i, (name, type_id, locator_kind, locator, excluded, review, refs, revision)) in [
        (
            "Gateway API",
            "api.endpoint",
            "url",
            "https://api.example.test/v1",
            false,
            SurfaceScopeState::Approved,
            3,
            13,
        ),
        (
            "Request parser",
            "code.module",
            "path",
            "src/parser.rs",
            false,
            SurfaceScopeState::Approved,
            4,
            9,
        ),
        (
            "Session service",
            "service",
            "url",
            "https://auth.example.test",
            false,
            SurfaceScopeState::Candidate,
            1,
            4,
        ),
        (
            "Object store",
            "service",
            "url",
            "https://storage.example.test",
            false,
            SurfaceScopeState::Approved,
            2,
            6,
        ),
        (
            "External identity",
            "service",
            "url",
            "https://identity.example.test",
            true,
            SurfaceScopeState::Approved,
            1,
            2,
        ),
        (
            "Import worker",
            "binary",
            "path",
            "bin/example-worker",
            false,
            SurfaceScopeState::Candidate,
            0,
            3,
        ),
        (
            "Protocol reference",
            "documentation",
            "url",
            "https://docs.example.test/protocol",
            true,
            SurfaceScopeState::Approved,
            2,
            2,
        ),
        (
            "Archive adapter",
            "code.module",
            "path",
            "src/archive.rs",
            false,
            SurfaceScopeState::Unknown,
            0,
            0,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        s.workspace_surface_edits.push(WorkspaceSurface {
            id: format!("S{}", i + 1),
            definition: Some(SurfaceDefinition {
                name: name.into(),
                type_id: type_id.into(),
                scope: if excluded {
                    SurfaceScope::OutOfScope
                } else {
                    SurfaceScope::InScope
                } as i32,
                scope_state: review as i32,
                locator: Some(SurfaceLocator {
                    kind: locator_kind.into(),
                    value: locator.into(),
                }),
                description: "Synthetic component for visual review of the ExampleCo surface map."
                    .into(),
                out_of_scope_reason: if excluded {
                    "Reference only; excluded from the example authorization.".into()
                } else {
                    String::new()
                },
                evidence_refs: (0..refs)
                    .map(|reference| SurfaceEvidence {
                        kind: "artifact".into(),
                        reference: format!(
                            "artifacts/example-surface-{}-{}.txt",
                            i + 1,
                            reference + 1
                        ),
                        note: "Synthetic fixture evidence.".into(),
                    })
                    .collect(),
            }),
            lineage: if revision == 0 {
                Vec::new()
            } else {
                vec![SurfaceRevision {
                    number: revision,
                    at_ms: 1_800_000_000_000 + i as u64 * 3_600_000,
                    actor: if i % 2 == 0 { "user" } else { "agent" }.into(),
                    action: "updated".into(),
                    changes: vec![SurfaceChange {
                        field: "definition.description".into(),
                        before_json: "\"Example draft\"".into(),
                        after_json: "\"Example reviewed description\"".into(),
                    }],
                }]
            },
            ..Default::default()
        });
    }
    for (source, target, label, type_id) in [
        (0, 1, "Parses input", "calls"),
        (0, 2, "Checks session", "authenticates"),
        (1, 3, "Persists data", "writes"),
        (1, 5, "Queues import", "dispatches"),
        (2, 4, "Federates login", "depends_on"),
        (5, 6, "Uses protocol", "references"),
        (5, 7, "Loads archive", "calls"),
    ] {
        s.workspace_surface_edits[source]
            .relationships
            .push(SurfaceRelationship {
                target_id: format!("S{}", target + 1),
                label: label.into(),
                type_id: type_id.into(),
                ..Default::default()
            });
    }
    if matches!(
        variant,
        "surfaces-web" | "surfaces-web-overflow" | "surfaces-web-scope" | "surfaces-web-focused"
    ) {
        if variant == "surfaces-web-scope"
            && let Some(definition) = &mut s.workspace_surface_edits[3].definition
        {
            definition.scope = SurfaceScope::OutOfScope as i32;
            s.workspace_surface_explorer.show_out_of_scope = true;
        }
        if variant == "surfaces-web"
            && let Some(definition) = &mut s.workspace_surface_edits[0].definition
        {
            definition.name = "Example organization security research guidelines".into();
        }
        for link in &mut s.workspace_surface_edits[0].relationships {
            link.label = "contains".into();
            link.type_id = "contains".into();
        }
        s.workspace_surface_edits[3]
            .relationships
            .push(SurfaceRelationship {
                target_id: "S5".into(),
                label: "contains".into(),
                type_id: "contains".into(),
                ..Default::default()
            });
        for link in &mut s.workspace_surface_edits[5].relationships {
            link.label = "contains".into();
            link.type_id = "contains".into();
        }
        s.workspace_surface_edits[0]
            .relationships
            .push(SurfaceRelationship {
                target_id: "S4".into(),
                label: "depends on".into(),
                type_id: "depends_on".into(),
                ..Default::default()
            });
        if variant == "surfaces-web-focused" {
            let mut nested = s.workspace_surface_edits[7].clone();
            nested.id = "S9".into();
            nested.relationships.clear();
            if let Some(definition) = &mut nested.definition {
                definition.name = "Nested example".into();
            }
            s.workspace_surface_edits.push(nested);
            s.workspace_surface_edits[1]
                .relationships
                .push(SurfaceRelationship {
                    target_id: "S9".into(),
                    label: "contains".into(),
                    type_id: "contains".into(),
                    ..Default::default()
                });
        }
        if variant == "surfaces-web-overflow" {
            for (id, name) in [("S10", "Example web four"), ("S11", "Example web five")] {
                let mut extra = s.workspace_surface_edits[3].clone();
                extra.id = id.into();
                extra.relationships.clear();
                if let Some(definition) = &mut extra.definition {
                    definition.name = name.into();
                }
                s.workspace_surface_edits.push(extra);
            }
        }
    }
    if variant == "surfaces-filter" {
        s.selection_menu = Some(SelectionMenu::SurfaceFilter);
    }
    if let Some(details) = &mut s.workspace_details {
        details.surfaces = s.workspace_surface_edits.clone();
    }
    use beale_contracts::findings::Status as FindingStatus;
    for (i, (title, status, impact, reachability)) in [
        (
            "Example boundary mismatch",
            FindingStatus::Screened,
            Some(88),
            Some(86),
        ),
        (
            "Example input handling",
            FindingStatus::Validated,
            Some(70),
            Some(32),
        ),
        (
            "Example access control",
            FindingStatus::Reported,
            Some(96),
            Some(72),
        ),
        (
            "Example resource limit",
            FindingStatus::Screened,
            Some(35),
            Some(78),
        ),
        (
            "Example metadata leak",
            FindingStatus::Screened,
            Some(18),
            Some(28),
        ),
        (
            "Example parser hypothesis",
            FindingStatus::Candidate,
            None,
            None,
        ),
        (
            "Example session boundary",
            FindingStatus::Validated,
            Some(52),
            None,
        ),
        (
            "Example import handling",
            FindingStatus::Screened,
            Some(88),
            Some(86),
        ),
        (
            "Example zero-impact review",
            FindingStatus::Validated,
            Some(0),
            Some(0),
        ),
        (
            "Example maximum assessment",
            FindingStatus::Reported,
            Some(100),
            Some(100),
        ),
        (
            "Example dismissed premise",
            FindingStatus::Rejected,
            Some(90),
            Some(90),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        s.workspace_findings.records.push(beale_contracts::findings::Summary {
            id: format!("F{}", i + 1), title: title.into(),
            summary: "Synthetic finding for layout review. Evidence and verification remain attached to the record.".into(),
            surface_ids: vec!["S1".into()], status, impact_score: impact, reachability_score: reachability,
            updated_at_ms: 1_800_000_000_000, revision: 2,
            evidence_count: (i as u32 * 2) % 5,
            verified: false,
        });
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    s.tracking.snapshot = Some(TrackingResponse {
        subscriptions: (0..7)
            .map(|i| TrackingSubscriptionPoint {
                provider: ProviderId::Openai as i32,
                window_id: "week".into(),
                label: "Weekly allowance".into(),
                observed_at_ms: now - (6 - i) * 86_400_000,
                remaining_percent: (95 - i * 9) as u32,
                ..Default::default()
            })
            .collect(),
        model_days: (0..7)
            .flat_map(|i| {
                let wave = [2, 4, 3, 7, 5, 9, 6][i as usize];
                [
                    (
                        ProviderId::Openai,
                        AuthMethod::Subscription,
                        "Example Research",
                        "main",
                        16_400,
                        2_400,
                    ),
                    (
                        ProviderId::Anthropic,
                        AuthMethod::ApiKey,
                        "Example Analyst",
                        "reviewer",
                        8_200,
                        1_900,
                    ),
                    (
                        ProviderId::Openai,
                        AuthMethod::ApiKey,
                        "Example Scout",
                        "scout",
                        4_800,
                        900,
                    ),
                ]
                .into_iter()
                .enumerate()
                .map(
                    move |(index, (provider, method, model, role, input, output))| {
                        let calls = (wave * (4 - index as u64)) as u32;
                        TrackingModelDay {
                            day_start_ms: now / 86_400_000 * 86_400_000 - (6 - i) * 86_400_000,
                            provider: provider as i32,
                            method: method as i32,
                            model: model.into(),
                            role: role.into(),
                            input_tokens: input * wave,
                            output_tokens: output * wave,
                            duration_ms: 42_000 * wave,
                            measured_calls: calls,
                            total_calls: calls + if i == 3 { 1 } else { 0 },
                            cache_read_tokens: Some(input * wave * (50 + index as u64 * 12) / 100),
                            cache_input_tokens: Some(input * wave),
                            cache_measured_calls: Some(calls),
                        }
                    },
                )
            })
            .collect(),
    });
    s.tracking.updated_at = Some(now);
    match name {
        "home" => {}
        "empty" => {
            s.workspaces.clear();
            s.session_summaries.clear();
        }
        "research" | "session" | "canvas" => {
            s.primary_view = PrimaryView::NewResearch;
            if variant == "graph-cloudflare" {
                s.new_research_graph_id = beale_graphs::CLOUDFLARE_ID.to_owned();
            }
            if variant == "graph-openai" {
                s.new_research_graph_id = beale_graphs::OPENAI_ID.to_owned();
            }
            if variant == "graph-sequential" {
                s.new_research_graph_id = beale_graphs::SEQUENTIAL_ID.to_owned();
            }
            if name != "research" {
                s.active_session = Some(SessionDetail {
                    summary: Some(s.session_summaries[0].clone()), model: "example-lead".into(), provider: ProviderId::Openai as i32, method: AuthMethod::ApiKey as i32,
                    total_duration_ms: 142_000,
                    messages: vec![
                        SessionMessage { role: SessionRole::User as i32, text: "Map the trust boundary for the local example application and identify the next verification step.".into(), ..Default::default() },
                        SessionMessage { role: SessionRole::Assistant as i32, text: "## Boundary review\nThe example request passes through three stages: parsing, authorization, and storage.\n\n### Observations\n- The local fixture defines two caller roles.\n- The authorization check runs before the storage operation.\n- No vulnerability has been confirmed.\n\n### Next verification\nCompare both roles against the same synthetic object and preserve the results in a runbook.".into(), duration_ms: 142_000, ..Default::default() }
                    ], token_usage: Some(SessionTokenUsage { input_tokens: 18200, output_tokens: 2400, cache_read_tokens: 12000, cache_input_tokens: 18200, measured_calls: 4, total_calls: 4, cache_measured_calls: 4 }), ..Default::default()
                });
                s.session_sidenav_visible = name == "session";
                if variant == "session-activity" {
                    let message = &mut s.active_session.as_mut().unwrap().messages[1];
                    message.reasoning_summaries = vec![
                        SessionReasoningSummary {
                            text: "Identify the local fixture's trust boundary.".into(),
                            ..Default::default()
                        },
                        SessionReasoningSummary {
                            text:
                                "Check whether both caller roles reach the same authorization path."
                                    .into(),
                            ..Default::default()
                        },
                    ];
                    message.commentary = vec![SessionCommentary {
                        text: "I found two caller roles in the synthetic fixture. I’ll compare their access paths and record the result.".into(),
                        ..Default::default()
                    }];
                    message.tools = vec![SessionToolSummary {
                        id: "tool-example".into(),
                        name: "read".into(),
                        summary: "Read example.txt".into(),
                        ..Default::default()
                    }];
                    s.session_work_expanded.insert(1);
                    s.session_sidenav_visible = false;
                }
                if variant == "graph-session" {
                    let detail = s.active_session.as_mut().unwrap();
                    detail.graph_id = beale_graphs::OPENAI_ID.to_owned();
                    detail.messages[1].agent_role = "discovery".to_owned();
                    detail.messages[1].tools = vec![SessionToolSummary {
                        id: "tool-example".to_owned(),
                        name: "read".to_owned(),
                        summary: "Read example.txt".to_owned(),
                        agent_role: "discovery".to_owned(),
                        ..Default::default()
                    }];
                    s.session_agent_role = "discovery".to_owned();
                    s.session_work_expanded.insert(1);
                    s.session_sidenav_visible = false;
                }
                if matches!(variant, "subagent-canvas" | "subagent-popup") {
                    let child = SessionSubagentSummary {
                        id: "00000000-0000-4000-8000-000000000099".into(),
                        parent_role: "discovery".into(), role: "scout".into(),
                        task: "Inspect the local example application's request boundary and record the evidence.".into(),
                        status: "completed".into(), completed_at_ms: 1_800_000_000_000,
                    };
                    let detail = s.active_session.as_mut().unwrap();
                    detail.graph_id = beale_graphs::OPENAI_ID.to_owned();
                    detail.graph_agents = vec![SessionGraphAgent {
                        role: "discovery".into(),
                        status: "idle".into(),
                    }];
                    detail.agent_metrics = vec![SessionAgentMetrics {
                        role: "discovery".into(),
                        token_usage: detail.token_usage.clone(),
                        model_usage: Vec::new(),
                        usage_snapshots: Vec::new(),
                        duration_ms: 142_000,
                    }];
                    detail.subagents = vec![child.clone()];
                    detail.messages[1].agent_role = "discovery".into();
                    s.session_agent_role = "discovery".into();
                    if variant == "subagent-canvas" {
                        s.canvas_open = true;
                        s.session_sidenav_visible = false;
                        s.session_work_expanded.insert(usize::MAX);
                        s.canvas_subagent = Some(subagent_canvas::SubagentCanvas {
                            session_id: s.session_summaries[0].id.clone(), summary: child.clone(),
                            detail: Some(SessionSubagentDetail {
                                summary: Some(child), result: "The example boundary check completed with no confirmed finding.".into(),
                                offset: 0, next_offset: 3, total_items: 3,
                                task: "Inspect the local example application's request boundary and record the evidence.".into(),
                                items: vec![
                                    SessionSubagentItem { kind: "reasoning".into(), text: "Checking the synthetic fixture's input and authorization stages.".into(), tool: None },
                                    SessionSubagentItem { kind: "tool".into(), text: String::new(), tool: Some(SessionToolSummary {
                                        id: "00000000-0000-4000-8000-000000000098".into(), name: "read".into(),
                                        summary: "Read example.txt".into(), agent_role: "discovery".into(), ..Default::default()
                                    }) },
                                    SessionSubagentItem { kind: "message".into(), text: "The example boundary check completed with no confirmed finding.".into(), tool: None },
                                ],
                            }), loading: false, error: String::new(), generation: 0,
                        });
                        s.session_live_events.push(SessionStreamEvent {
                            kind: SessionStreamKind::ToolFinished as i32,
                            agent_role: "discovery".into(),
                            subagent_id: "00000000-0000-4000-8000-000000000099".into(),
                            tool_detail: Some(SessionToolDetailPage {
                                id: "00000000-0000-4000-8000-000000000098".into(),
                                name: "read".into(),
                                arguments: "{\"path\":\"example.txt\"}".into(),
                                output: "Synthetic fixture content".into(),
                                offset: 0,
                                next_offset: 25,
                                total_bytes: 25,
                                is_error: false,
                            }),
                            ..Default::default()
                        });
                    } else {
                        s.session_sidenav_visible = true;
                        s.session_usage_menu = Some(session_sidenav::UsageMenu::Subagents);
                    }
                }
                s.canvas_open = name == "canvas" || variant == "subagent-canvas";
                if name == "canvas" {
                    s.canvas_runbook = runbook_canvas::CanvasRunbookState::with_view(runbook_canvas::RunbookView {
                        selected: Some(SessionRunbook { id: "R1".into(), title: "Example boundary verification".into(), status: "prepared".into(), ..Default::default() }),
                        workspace_id: workspace.id.clone(), session_id: Some(s.session_summaries[0].id.clone()),
                        document: Some(beale_contracts::runbooks::Document { id: "R1".into(), workspace_id: workspace.id.clone(), title: "Example boundary verification".into(), purpose: "Compare synthetic inputs and preserve a reproducible observation.".into(), cells: vec![beale_contracts::runbooks::Cell::Markdown { id: "L1".into(), name: "Procedure".into(), source: "Use only the local example fixture. Record the expected and observed results.".into() }, beale_contracts::runbooks::Cell::Code { id: "L2".into(), name: "Compare inputs".into(), language: "python".into(), source: "roles = [\"reader\", \"owner\"]\nfor role in roles:\n    print(role, \"example fixture\")".into() }], next_cell_number: 3, created_at_ms: 1_800_000_000_000, updated_at_ms: 1_800_000_000_000, revision: 1 }), ..Default::default()
                    });
                }
            }
        }
        "features" | "plugins" | "kits" => {
            s.primary_view = PrimaryView::Extensions;
            s.extensions_section = match name {
                "plugins" => extensions_view::ExtensionSection::Plugins,
                "kits" => extensions_view::ExtensionSection::ResearchKits,
                _ => extensions_view::ExtensionSection::Features,
            };
        }
        "fleet" | "vms" | "research-tools" => {
            s.primary_view = PrimaryView::Fleet;
            s.fleet_section = match name {
                "vms" => fleet_view::FleetSection::VirtualMachines,
                "research-tools" => fleet_view::FleetSection::ResearchTools,
                _ => fleet_view::FleetSection::AppServers,
            };
        }
        "automations" => {
            s.primary_view = PrimaryView::Automations;
            s.automations.jobs.push(AutomationJob {
                id: "example-job".into(),
                workspace_id: workspace.id.clone(),
                session_id: s.session_summaries[0].id.clone(),
                title: "Example daily boundary review".into(),
                repeat_preset: "day".into(),
                next_run_at_ms: now + 86_400_000,
                ..Default::default()
            });
            s.automations.selected_id = Some("example-job".into());
        }
        "tracking" => {
            s.primary_view = PrimaryView::Tracking;
        }
        "import" | "create-general" | "create-surfaces" | "create-rules" => {
            s.primary_view = PrimaryView::WorkspaceCreation;
            s.workspace_draft.tab = match name {
                "create-general" => workspace_creation::CreationTab::Foundation,
                "create-surfaces" => workspace_creation::CreationTab::Surfaces,
                "create-rules" => workspace_creation::CreationTab::Rules,
                _ => workspace_creation::CreationTab::Import,
            };
        }
        "workspace" | "rules" | "surfaces" | "surface-detail" | "surface-manage" | "findings"
        | "campaign" => {
            s.primary_view = PrimaryView::Workspace;
            s.workspace_tab = match name {
                "workspace" => workspace_view::WorkspaceTab::Foundation,
                "surfaces" | "surface-detail" | "surface-manage" => {
                    workspace_view::WorkspaceTab::Surfaces
                }
                "findings" => workspace_view::WorkspaceTab::Findings,
                "campaign" => workspace_view::WorkspaceTab::Campaign,
                _ => workspace_view::WorkspaceTab::Rules,
            };
            s.workspace_findings.loaded = true;
            if variant == "guidance-agents" {
                s.workspace_guidance_view = workspace_view::GuidanceView::Agents;
            }
            if name == "surface-manage" {
                s.workspace_surface_explorer.view = Some(workspace_surface_layout::View::Inventory);
            }
            if name == "surface-detail" {
                crate::surface_canvas::open(&mut s, 0);
            }
        }
        "chat" => {
            s.quick_chat_open = true;
            s.quick_chat_title = "Example research notes".into();
            s.quick_chat_id = Some("20000000-0000-4000-8000-000000000001".into());
            s.quick_chat_workspace_id = Some(workspace.id.clone());
            s.quick_chat_messages = vec![
                QuickChatMessage { role: SessionRole::User as i32, text: "What belongs in a reproducible research note?".into() },
                QuickChatMessage { role: SessionRole::Assistant as i32, text: "Keep the **authorized scope**, synthetic input, expected behavior, and observed result together. Link the runbook and distinguish evidence from assumptions.".into() }
            ];
        }
        "dialog" => {
            s.primary_view = PrimaryView::WorkspaceCreation;
            s.workspace_draft.cancel_dialog_open = true;
        }
        _ => {
            s.primary_view = PrimaryView::Settings;
            s.settings_section = match name {
                "providers" | "custom" => SettingsSection::Providers,
                "appearance" | "typography" | "picker" => SettingsSection::Appearance,
                "archive" | "archive-chats" => SettingsSection::Archive,
                _ => SettingsSection::Safety,
            };
            if name == "menu" {
                s.selection_menu = Some(SelectionMenu::ColorTheme);
                s.settings_section = SettingsSection::Appearance;
            }
            if name == "picker" {
                toggle_color_picker(&mut s, ColorTarget::Heat(HeatLevel::Low));
            }
            if name == "custom" {
                s.selected = Provider::Custom;
                s.custom_providers.push(CustomProvider {
                    id: "example-provider".into(),
                    name: "Example local models".into(),
                    base_url: "https://models.example.test/v1".into(),
                    models: vec![beale_contracts::v1::ModelEntry {
                        id: "example-model".into(),
                        display_name: "Example Model".into(),
                    }],
                    lead_model: "example-model".into(),
                    small_model: "example-model".into(),
                    api_key_configured: true,
                    ..Default::default()
                });
            }
            if name == "typography" {
                s.appearance_tab = AppearanceTab::Typography;
            }
            if name == "guardian" {
                s.permissions_tab = permission_settings::PermissionTab::Guardian;
            }
            if name == "archive-chats" {
                s.archive_tab = archive_view::ArchiveTab::QuickChats;
            }
        }
    }
    if variant.starts_with("session-insights") {
        s.session_insights_open = true;
        let detail = s.active_session.as_mut().unwrap();
        detail.total_duration_ms = 1_800_000;
        detail.model_usage = [
            (
                ProviderId::Openai,
                AuthMethod::Subscription,
                "Example Research",
                "main",
                18_000,
                2_400,
                12_000,
                9,
            ),
            (
                ProviderId::Anthropic,
                AuthMethod::ApiKey,
                "Example Analyst",
                "reviewer",
                9_000,
                1_800,
                4_000,
                5,
            ),
            (
                ProviderId::Openai,
                AuthMethod::ApiKey,
                "Example Scout",
                "scout",
                3_000,
                800,
                2_000,
                4,
            ),
        ]
        .into_iter()
        .map(
            |(provider, method, model, role, input, output, cached, calls)| SessionModelUsage {
                provider: provider as i32,
                method: method as i32,
                model: model.into(),
                role: role.into(),
                usage: Some(SessionTokenUsage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: cached,
                    cache_input_tokens: input,
                    measured_calls: calls,
                    total_calls: calls,
                    cache_measured_calls: calls,
                }),
            },
        )
        .collect();
        detail.token_usage = Some(SessionTokenUsage {
            input_tokens: 30_000,
            output_tokens: 5_000,
            cache_read_tokens: 18_000,
            cache_input_tokens: 30_000,
            measured_calls: 18,
            total_calls: 18,
            cache_measured_calls: 18,
        });
        detail.usage_snapshots = (0..=6)
            .map(|i| SessionUsageSnapshot {
                work_duration_ms: i * 300_000,
                main_usage: Some(SessionTokenUsage {
                    input_tokens: i * 3_000,
                    output_tokens: i * 400,
                    cache_read_tokens: i * 2_000,
                    cache_input_tokens: i * 3_000,
                    measured_calls: i * 9 / 6,
                    total_calls: i * 9 / 6,
                    cache_measured_calls: i * 9 / 6,
                }),
                subagent_usage: Some(SessionTokenUsage {
                    input_tokens: i * 2_000,
                    output_tokens: i * 2_600 / 6,
                    cache_read_tokens: i * 1_000,
                    cache_input_tokens: i * 2_000,
                    measured_calls: i * 3 - i * 9 / 6,
                    total_calls: i * 3 - i * 9 / 6,
                    cache_measured_calls: i * 3 - i * 9 / 6,
                }),
                input_tokens: i * 5_000,
                output_tokens: i * 5_000 / 6,
                cache_read_tokens: i * 3_000,
                cache_input_tokens: i * 5_000,
                measured_calls: i * 3,
                total_calls: i * 3,
                cache_measured_calls: i * 3,
            })
            .collect();
        if variant == "session-insights-empty" {
            detail.token_usage = None;
            detail.model_usage.clear();
            detail.usage_snapshots.clear();
        } else if variant == "session-insights-missing" {
            for source in &mut detail.model_usage {
                let calls = source.usage.as_ref().unwrap().total_calls;
                source.usage = Some(SessionTokenUsage {
                    total_calls: calls,
                    ..Default::default()
                });
            }
            detail.token_usage = Some(SessionTokenUsage {
                total_calls: 18,
                ..Default::default()
            });
            for point in &mut detail.usage_snapshots {
                *point = SessionUsageSnapshot {
                    work_duration_ms: point.work_duration_ms,
                    total_calls: point.total_calls,
                    main_usage: point.main_usage.as_ref().map(|u| SessionTokenUsage {
                        total_calls: u.total_calls,
                        ..Default::default()
                    }),
                    subagent_usage: point.subagent_usage.as_ref().map(|u| SessionTokenUsage {
                        total_calls: u.total_calls,
                        ..Default::default()
                    }),
                    ..Default::default()
                };
            }
        }
        if variant == "session-insights-main" {
            s.session_insights_filter = tracking_data::AgentFilter::Main;
        } else if matches!(
            variant,
            "session-insights-subagents" | "session-insights-no-subagents"
        ) {
            s.session_insights_filter = tracking_data::AgentFilter::Subagents;
            if variant == "session-insights-no-subagents" {
                detail.model_usage.retain(|source| source.role == "main");
                detail.token_usage = detail.model_usage[0].usage.clone();
                for point in &mut detail.usage_snapshots {
                    let main = point.main_usage.as_ref().unwrap();
                    point.input_tokens = main.input_tokens;
                    point.output_tokens = main.output_tokens;
                    point.measured_calls = main.measured_calls;
                    point.total_calls = main.total_calls;
                    point.cache_read_tokens = main.cache_read_tokens;
                    point.cache_input_tokens = main.cache_input_tokens;
                    point.cache_measured_calls = main.cache_measured_calls;
                    point.subagent_usage = Some(SessionTokenUsage::default());
                }
            }
        }
        s.session_busy = variant == "session-insights-running";
        if variant == "session-insights-compact" {
            s.appearance.text_size = TextSize::ExtraLarge;
        }
    }
    if variant.starts_with("findings-plot") {
        s.workspace_findings.mode = findings_view::FindingsMode::Plot;
        match variant {
            "findings-plot-empty" => s.workspace_findings.records.clear(),
            "findings-plot-unrated" => {
                for record in &mut s.workspace_findings.records {
                    record.impact_score = None;
                    record.reachability_score = None;
                }
            }
            "findings-plot-cluster" => s.workspace_findings.plot_selection = Some((88, 86)),
            "findings-plot-large" => s.appearance.text_size = TextSize::ExtraLarge,
            "findings-plot-filtered" => {
                s.workspace_findings
                    .toggle_plot_stage(FindingStatus::Candidate);
                s.workspace_findings
                    .toggle_plot_stage(FindingStatus::Screened);
            }
            "findings-plot-hidden" => {
                for status in [
                    FindingStatus::Candidate,
                    FindingStatus::Validated,
                    FindingStatus::Screened,
                    FindingStatus::Reported,
                ] {
                    s.workspace_findings.toggle_plot_stage(status);
                }
            }
            _ => {}
        }
    }
    match variant {
        "surfaces-web-focused" => {
            s.workspace_surface_explorer.focus_path = vec!["S2".into()];
        }
        "surfaces-empty" => {
            s.workspace_surface_edits.clear();
            s.workspace_details.as_mut().unwrap().surfaces.clear();
        }
        "surface-connections" => {
            s.workspace_surface_explorer.neighborhood = Some("S1".into());
            s.workspace_surface_explorer.view = Some(workspace_surface_layout::View::Graph);
            s.workspace_surface_explorer.return_to = Some(workspace_surface_layout::ReturnTo {
                view: workspace_surface_layout::View::Inventory,
                sort: workspace_surface_layout::Sort::Recorded,
                focus: workspace_surface_layout::Focus::All,
                show_out_of_scope: false,
                query: String::new(),
                page: 0,
                page_anchor: None,
            });
        }
        "surfaces-review" => s.workspace_surface_focus = workspace_surface_layout::Focus::Review,
        "surfaces-search" => s.workspace_surface_query = TextEdit::new("archive"),
        "surfaces-large" => s.appearance.text_size = TextSize::ExtraLarge,
        "surfaces-dense" => {
            let originals = s.workspace_surface_edits.clone();
            for group in 1..5 {
                for surface in &originals {
                    let mut copy = surface.clone();
                    copy.id = format!("{}-{}", surface.id, group + 1);
                    if let Some(definition) = &mut copy.definition {
                        definition.name = format!("{} {}", definition.name, group + 1);
                    }
                    for link in &mut copy.relationships {
                        link.target_id = format!("{}-{}", link.target_id, group + 1);
                    }
                    s.workspace_surface_edits.push(copy);
                }
            }
            s.workspace_details.as_mut().unwrap().surfaces = s.workspace_surface_edits.clone();
        }
        "tracking-selected" => {
            s.tracking.selected_day = Some(now / 86_400_000 * 86_400_000 - 86_400_000)
        }
        "tracking-subagents" => s.tracking.filter = tracking_data::AgentFilter::Subagents,
        "tracking-compact" => {
            s.sidenav_width = SIDENAV_MAX_WIDTH;
            s.appearance.text_size = TextSize::ExtraLarge;
        }
        "tracking-empty" => s.tracking.snapshot.as_mut().unwrap().model_days.clear(),
        "tracking-legacy" | "tracking-unmeasured" => {
            for day in &mut s.tracking.snapshot.as_mut().unwrap().model_days {
                if variant == "tracking-unmeasured" {
                    day.measured_calls = 0;
                    day.input_tokens = 0;
                    day.output_tokens = 0;
                }
                day.cache_read_tokens = None;
                day.cache_input_tokens = None;
                day.cache_measured_calls = None;
            }
        }
        "tracking-loading" => {
            s.tracking.snapshot = None;
            s.tracking.loading = true;
        }
        "tracking-offline" => {
            s.tracking.snapshot = None;
            s.connection = ConnectionStatus::Offline;
        }
        "tracking-error" => s.tracking.error = "Tracking is unavailable".into(),
        "tracking-headroom" => s.tracking.show_subscriptions = true,
        "archive-empty" | "archive-chats-empty" => s.archive_entries.clear(),
        "appearance-compact" | "typography-compact" => {
            s.sidenav_width = SIDENAV_MAX_WIDTH;
            s.appearance.text_size = TextSize::ExtraLarge;
        }
        "anthropic" => s.selected = Provider::Anthropic,
        "openrouter" => s.selected = Provider::OpenRouter,
        "xai" => s.selected = Provider::Xai,
        "zai" => s.selected = Provider::Zai,
        "api-key" => s.api_key_dialog = true,
        "custom-editor" => {
            s.custom_editor_open = true;
            s.custom_name_edit = TextEdit::new("Example inference service");
            s.custom_url_edit = TextEdit::new("https://models.example.test/v1");
            s.custom_models_edit = TextEdit::new("example-research, example-review");
        }
        "graph-editor" => {
            s.graph_editor = Some(new_research::GraphEditor {
                role: beale_graphs::Role::Main,
                models: vec![],
                model_menu: None,
            })
        }
        "model-menu" => {
            if let Some(graphs) = s
                .features
                .iter_mut()
                .find(|feature| feature.id == beale_graphs::FEATURE_ID)
            {
                graphs.enabled = false;
            }
            s.composer_menu_open = true;
        }
        "goal-menu" => s.selection_menu = Some(SelectionMenu::GoalMode),
        "surface-add" => {
            s.workspace_surface_dialog_scope = Some(workspace_creation::SurfaceScope::InScope)
        }
        "surface-delete" => s.workspace_surface_delete_index = Some(0),
        "relationship" => {
            s.workspace_surface_connecting = Some(0);
            s.workspace_surface_selected = Some(0);
            s.workspace_surface_relationship_target = Some(1);
        }
        "workspace-name" => {
            s.workspace_name_dialog_open = true;
            s.workspace_name_dialog_value = TextEdit::new(&workspace.name);
        }
        "workspace-rule" => s.workspace_rule_dialog_open = true,
        "feature-detail" => {
            s.feature_details_id = Some(s.features[0].id.clone());
            s.feature_details = Some(FeatureDetailsResponse {
                name: s.features[0].name.clone(),
                description: s.features[0].description.clone(),
                prompt_extension:
                    "Keep the research objective, evidence, and next verification step explicit."
                        .into(),
                ..Default::default()
            });
        }
        "plugin-add" => s.plugin_add_dialog_open = true,
        "plugin-detail" => {
            s.plugin_manifest_name = Some("Example research kit".into());
            s.plugin_manifest_text = "# Example research kit\n\nSynthetic package for visual review.\n\n## Workflow\nRecord scope, inspect the local fixture, and preserve evidence.".into();
        }
        "fleet-connect" => s.fleet.connect_dialog_open = true,
        "fleet-import" => s.fleet.import_dialog_open = true,
        "fleet-detail" => s.fleet.details_id = Some("local".into()),
        "safety-rule" => {
            s.permission_creating = true;
            s.permission_name_edit = TextEdit::new("Example scope review");
            s.permission_description_edit = TextEdit::new(
                "Check the proposed operation against the recorded authorized scope.",
            );
        }
        "finding-detail" => s.workspace_findings.preview_detail(),
        "finding-create" => findings_view::open_create(&mut s),
        _ => {}
    }
    if variant == "home-collapsed" {
        s.sidenav_visible = false;
    }
    if variant == "home-usage" {
        s.provider_usage = [ProviderId::Openai, ProviderId::Xai].map(|provider| {
            Some(ProviderUsageSnapshot {
                provider: provider as i32,
                windows: vec![ProviderUsageWindow {
                    label: "Weekly Limit".into(),
                    used_percent: 25,
                    ..Default::default()
                }],
                ..Default::default()
            })
        });
    }
    s
}
