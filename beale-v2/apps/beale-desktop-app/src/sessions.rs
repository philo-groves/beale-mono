use beale_contracts::{
    PROTOBUF_CONTENT_TYPE, PROTOBUF_STREAM_CONTENT_TYPE, SESSION_ARCHIVE_PATH,
    SESSION_DETAIL_MAX_TOOL_SUMMARIES, SESSION_DETAIL_PATH, SESSION_STREAM_MAX_FRAME_BYTES,
    SESSION_SUBAGENT_DETAIL_PATH, SESSION_TOOL_DETAIL_PATH, SESSION_TURN_ID_HEADER,
    SESSION_TURN_STOP_PATH, SESSION_TURN_STREAM_PATH, SESSIONS_PATH,
    v1::{
        GoalStatus, ListSessionsResponse, SessionDetail, SessionRole, SessionStreamEvent,
        SessionStreamKind, SessionSubagentDetail, SessionSubagentSummary, SessionSummary,
        SessionToolDetailPage, SubmitSessionTurnRequest,
    },
};
use prost::Message;
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use std::{net::SocketAddr, time::Duration};

const MAX_REPLY_BYTES: usize = 4_500_000;

#[derive(Default)]
struct StreamDecoder {
    pending: Vec<u8>,
}

impl StreamDecoder {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<SessionStreamEvent>, String> {
        self.pending.extend_from_slice(chunk);
        let mut events = Vec::new();
        loop {
            if self.pending.len() < 4 {
                break;
            }
            let length = u32::from_be_bytes(
                self.pending[..4]
                    .try_into()
                    .map_err(|_| "Invalid session stream frame")?,
            ) as usize;
            if length == 0 || length > SESSION_STREAM_MAX_FRAME_BYTES {
                return Err("Invalid session stream frame".to_owned());
            }
            if self.pending.len() < length + 4 {
                break;
            }
            let event = SessionStreamEvent::decode(&self.pending[4..length + 4])
                .map_err(|_| "Invalid session stream event".to_owned())?;
            self.pending.drain(..length + 4);
            events.push(event);
        }
        if self.pending.len() > SESSION_STREAM_MAX_FRAME_BYTES + 4 {
            return Err("Session stream frame is too large".to_owned());
        }
        Ok(events)
    }
}

pub(crate) fn client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize session connection".to_owned())
}

pub(crate) fn stream_client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize session connection".to_owned())
}

fn url(addr: SocketAddr, path: &str) -> String {
    format!("http://{addr}{path}")
}

async fn decode<T: Message + Default>(response: reqwest::Response) -> Result<T, String> {
    if response.status() != StatusCode::OK {
        return Err(match response.status() {
            StatusCode::NOT_FOUND => "Workspace or session is unavailable.".to_owned(),
            StatusCode::CONFLICT => "Session changed or reached its turn limit.".to_owned(),
            StatusCode::BAD_REQUEST => "The session request is invalid.".to_owned(),
            StatusCode::BAD_GATEWAY => "The provider could not complete this turn.".to_owned(),
            _ => format!("Session request failed ({})", response.status()),
        });
    }
    if response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err("Invalid app-server session response".to_owned());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read session response".to_owned())?;
    if bytes.len() > MAX_REPLY_BYTES {
        return Err("Session response is too large".to_owned());
    }
    T::decode(bytes).map_err(|_| "Invalid app-server session response".to_owned())
}

fn usage_values(usage: &beale_contracts::v1::SessionTokenUsage) -> [u128; 7] {
    [
        usage.input_tokens,
        usage.output_tokens,
        usage.measured_calls,
        usage.total_calls,
        usage.cache_read_tokens,
        usage.cache_input_tokens,
        usage.cache_measured_calls,
    ]
    .map(u128::from)
}

fn add_usage(
    total: &mut beale_contracts::v1::SessionTokenUsage,
    delta: &beale_contracts::v1::SessionTokenUsage,
) {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    total.measured_calls = total.measured_calls.saturating_add(delta.measured_calls);
    total.total_calls = total.total_calls.saturating_add(delta.total_calls);
    total.cache_read_tokens = total
        .cache_read_tokens
        .saturating_add(delta.cache_read_tokens);
    total.cache_input_tokens = total
        .cache_input_tokens
        .saturating_add(delta.cache_input_tokens);
    total.cache_measured_calls = total
        .cache_measured_calls
        .saturating_add(delta.cache_measured_calls);
}

pub(crate) fn detail_with_live_usage(
    detail: Option<&SessionDetail>,
    events: &[SessionStreamEvent],
    live_ms: u64,
) -> Option<SessionDetail> {
    use beale_contracts::v1::{SessionTokenUsage, SessionUsageSnapshot};
    let mut detail = detail?.clone();
    let deltas: Vec<_> = events
        .iter()
        .filter(|event| event.kind == SessionStreamKind::UsageUpdated as i32)
        .filter_map(|event| event.usage_delta.as_ref())
        .collect();
    if deltas.is_empty() {
        return Some(detail);
    }
    let mut total = detail.token_usage.take().unwrap_or_default();
    let mut main = detail
        .usage_snapshots
        .last()
        .and_then(|p| p.main_usage.clone())
        .unwrap_or_default();
    let mut subagents = detail
        .usage_snapshots
        .last()
        .and_then(|p| p.subagent_usage.clone())
        .unwrap_or_default();
    for delta in deltas {
        let Some(usage) = &delta.usage else { continue };
        add_usage(&mut total, usage);
        let scope = if beale_graphs::Role::from_id(&delta.role)
            .is_some_and(beale_graphs::Role::is_top_level)
            || delta.role.is_empty()
        {
            &mut main
        } else {
            &mut subagents
        };
        add_usage(scope, usage);
        if let Some(existing) = detail.model_usage.iter_mut().find(|item| {
            (
                item.provider,
                item.method,
                item.model.as_str(),
                item.role.as_str(),
            ) == (
                delta.provider,
                delta.method,
                delta.model.as_str(),
                delta.role.as_str(),
            )
        }) {
            add_usage(
                existing
                    .usage
                    .get_or_insert_with(SessionTokenUsage::default),
                usage,
            );
        } else {
            detail.model_usage.push((*delta).clone());
        }
    }
    detail.token_usage = Some(total.clone());
    detail.total_duration_ms = detail.total_duration_ms.saturating_add(live_ms).max(
        detail
            .usage_snapshots
            .last()
            .map_or(0, |point| point.work_duration_ms.saturating_add(1)),
    );
    detail.usage_snapshots.push(SessionUsageSnapshot {
        work_duration_ms: detail.total_duration_ms,
        input_tokens: total.input_tokens,
        output_tokens: total.output_tokens,
        measured_calls: total.measured_calls,
        total_calls: total.total_calls,
        cache_read_tokens: total.cache_read_tokens,
        cache_input_tokens: total.cache_input_tokens,
        cache_measured_calls: total.cache_measured_calls,
        main_usage: Some(main),
        subagent_usage: Some(subagents),
    });
    Some(detail)
}

pub(crate) fn detail_for_agent(
    detail: Option<&SessionDetail>,
    events: &[SessionStreamEvent],
    role: &str,
    live_ms: u64,
) -> Option<SessionDetail> {
    let detail = detail?;
    if !matches!(
        detail.graph_id.as_str(),
        beale_graphs::CLOUDFLARE_ID | beale_graphs::OPENAI_ID
    ) {
        return detail_with_live_usage(Some(detail), events, live_ms);
    }
    let metrics = detail
        .agent_metrics
        .iter()
        .find(|metrics| metrics.role == role)?;
    let mut scoped = detail.clone();
    scoped.token_usage = metrics.token_usage.clone();
    scoped.model_usage = metrics.model_usage.clone();
    scoped.usage_snapshots = metrics.usage_snapshots.clone();
    scoped.total_duration_ms = metrics.duration_ms;
    let events = events
        .iter()
        .filter(|event| event.agent_role == role)
        .cloned()
        .collect::<Vec<_>>();
    detail_with_live_usage(Some(&scoped), &events, live_ms)
}

fn valid_model_usage(detail: &SessionDetail) -> bool {
    if detail.model_usage.len() > 1024 {
        return false;
    }
    let mut total = [0u128; 7];
    let mut sources = std::collections::BTreeSet::new();
    for model in &detail.model_usage {
        let Some(usage) = &model.usage else {
            return false;
        };
        if !(1..=6).contains(&model.provider)
            || !(1..=2).contains(&model.method)
            || model.model.is_empty()
            || model.model.len() > 200
            || model.role.len() > 20
            || !sources.insert((model.provider, model.method, &model.model, &model.role))
            || usage.measured_calls > usage.total_calls
            || usage.cache_measured_calls > usage.measured_calls
            || usage.cache_read_tokens > usage.cache_input_tokens
            || usage.cache_input_tokens > usage.input_tokens
            || (usage.cache_measured_calls == 0 && usage.cache_input_tokens != 0)
        {
            return false;
        }
        for (sum, value) in total.iter_mut().zip(usage_values(usage)) {
            *sum += value;
        }
    }
    // An omitted breakdown remains readable; a supplied breakdown must reconcile.
    detail.model_usage.is_empty()
        || detail
            .token_usage
            .as_ref()
            .is_some_and(|usage| usage_values(usage) == total)
}

fn valid_scoped_snapshots(detail: &SessionDetail) -> bool {
    use beale_contracts::v1::SessionTokenUsage;
    // Legacy samples omit both scopes. Never infer their values from overall usage.
    if detail
        .usage_snapshots
        .iter()
        .all(|p| p.main_usage.is_none() && p.subagent_usage.is_none())
    {
        return true;
    }
    let mut previous = [[0u128; 7]; 2];
    for point in &detail.usage_snapshots {
        let (Some(main), Some(subagents)) = (&point.main_usage, &point.subagent_usage) else {
            return false;
        };
        let all = usage_values(&SessionTokenUsage {
            input_tokens: point.input_tokens,
            output_tokens: point.output_tokens,
            measured_calls: point.measured_calls,
            total_calls: point.total_calls,
            cache_read_tokens: point.cache_read_tokens,
            cache_input_tokens: point.cache_input_tokens,
            cache_measured_calls: point.cache_measured_calls,
        });
        let scopes = [usage_values(main), usage_values(subagents)];
        if [main, subagents].iter().any(|usage| {
            usage.measured_calls > usage.total_calls
                || usage.cache_measured_calls > usage.measured_calls
                || usage.cache_read_tokens > usage.cache_input_tokens
                || usage.cache_input_tokens > usage.input_tokens
                || (usage.cache_measured_calls == 0 && usage.cache_input_tokens != 0)
        }) || (0..7).any(|i| {
            scopes[0][i] + scopes[1][i] > all[i]
                || scopes[0][i] < previous[0][i]
                || scopes[1][i] < previous[1][i]
        }) {
            return false;
        }
        previous = scopes;
    }
    let mut expected = [[0u128; 7]; 2];
    for source in &detail.model_usage {
        let index = if source.role.is_empty() {
            continue;
        } else if beale_graphs::Role::from_id(&source.role)
            .is_some_and(beale_graphs::Role::is_top_level)
        {
            0
        } else {
            1
        };
        let Some(usage) = &source.usage else {
            return false;
        };
        for (sum, value) in expected[index].iter_mut().zip(usage_values(usage)) {
            *sum += value;
        }
    }
    previous == expected
}

fn valid_graph_state(detail: &SessionDetail) -> bool {
    if detail.graph_agents.len() > 4 || detail.graph_handoffs.len() > 128 {
        return false;
    }
    let graph = beale_graphs::graph_definition(&detail.graph_id);
    let mut roles = std::collections::BTreeSet::new();
    if !detail.graph_agents.iter().all(|agent| {
        beale_graphs::Role::from_id(&agent.role).is_some_and(|role| {
            role.is_top_level() && graph.is_some_and(|graph| graph.nodes.contains(&role))
        }) && matches!(
            agent.status.as_str(),
            "idle" | "queued" | "working" | "failed"
        ) && roles.insert(agent.role.as_str())
    }) {
        return false;
    }
    detail.graph_handoffs.iter().all(|item| {
        uuid::Uuid::parse_str(&item.id).is_ok()
            && !item.task.trim().is_empty()
            && item.task.len() <= 4_096
            && matches!(
                item.status.as_str(),
                "pending" | "running" | "completed" | "failed"
            )
            && beale_graphs::Role::from_id(&item.from_role).is_some_and(|role| role.is_top_level())
            && beale_graphs::Role::from_id(&item.to_role).is_some_and(|role| role.is_top_level())
    })
}

pub(crate) fn valid_detail(detail: &SessionDetail) -> bool {
    let Some(summary) = &detail.summary else {
        return false;
    };
    uuid::Uuid::parse_str(&summary.id).is_ok()
        && uuid::Uuid::parse_str(&summary.workspace_id).is_ok()
        && !summary.title.is_empty()
        && summary.last_message_at_ms > 0
        && detail.messages.len() <= 128
        && detail.first_turn_number > 0
        && matches!(
            beale_contracts::v1::ProviderId::try_from(detail.provider),
            Ok(beale_contracts::v1::ProviderId::Openai
                | beale_contracts::v1::ProviderId::Xai
                | beale_contracts::v1::ProviderId::Anthropic
                | beale_contracts::v1::ProviderId::Zai
                | beale_contracts::v1::ProviderId::Openrouter
                | beale_contracts::v1::ProviderId::Custom)
        )
        && matches!(
            beale_contracts::v1::AuthMethod::try_from(detail.method),
            Ok(beale_contracts::v1::AuthMethod::Subscription
                | beale_contracts::v1::AuthMethod::ApiKey)
        )
        && !detail.model.is_empty()
        && detail.total_duration_ms
            >= detail
                .messages
                .iter()
                .map(|message| message.duration_ms)
                .fold(0_u64, u64::saturating_add)
        && detail.token_usage.as_ref().is_none_or(|usage| {
            usage.measured_calls <= usage.total_calls
                && usage.cache_measured_calls <= usage.measured_calls
                && usage.cache_read_tokens <= usage.cache_input_tokens
                && usage.cache_input_tokens <= usage.input_tokens
        })
        && valid_model_usage(detail)
        && valid_scoped_snapshots(detail)
        && valid_graph_state(detail)
        && detail.subagents.len() <= 1_024
        && detail.subagents.iter().all(valid_subagent_summary)
        && detail.agent_metrics.len() <= 8
        && detail.agent_metrics.iter().all(|item| {
            !item.role.is_empty()
                && item.role.len() <= 32
                && item
                    .token_usage
                    .as_ref()
                    .is_some_and(|usage| usage.measured_calls <= usage.total_calls)
                && item.usage_snapshots.len() <= 2_048
        })
        && detail.usage_snapshots.len() <= 2_048
        && detail.usage_snapshots.iter().all(|point| {
            point.work_duration_ms <= detail.total_duration_ms
                && point.measured_calls <= point.total_calls
                && point.cache_measured_calls <= point.measured_calls
                && point.cache_read_tokens <= point.cache_input_tokens
                && point.cache_input_tokens <= point.input_tokens
        })
        && detail.usage_snapshots.windows(2).all(|pair| {
            pair[0].work_duration_ms < pair[1].work_duration_ms
                && pair[0].input_tokens <= pair[1].input_tokens
                && pair[0].output_tokens <= pair[1].output_tokens
                && pair[0].cache_read_tokens <= pair[1].cache_read_tokens
                && pair[0].cache_input_tokens <= pair[1].cache_input_tokens
        })
        && detail.usage_snapshots.last().is_none_or(|last| {
            last.work_duration_ms == detail.total_duration_ms
                && detail.token_usage.as_ref().is_some_and(|usage| {
                    last.input_tokens == usage.input_tokens
                        && last.output_tokens == usage.output_tokens
                        && last.cache_read_tokens == usage.cache_read_tokens
                        && last.cache_input_tokens == usage.cache_input_tokens
                })
        })
        && detail.goal.as_ref().is_none_or(|goal| {
            !goal.objective.trim().is_empty()
                && goal.objective.len() <= 16_384
                && goal.turns_used > 0
                && matches!(
                    GoalStatus::try_from(goal.status),
                    Ok(GoalStatus::Active
                        | GoalStatus::Complete
                        | GoalStatus::Waiting
                        | GoalStatus::Paused)
                )
                && (goal.status == GoalStatus::Waiting as i32) != goal.required_state.is_empty()
                && goal.required_state.len() <= 1_024
        })
        && detail.runbooks.len() <= 1_024
        && detail.runbooks.iter().all(|book| {
            !book.id.is_empty()
                && book.id.len() <= 128
                && !book.title.is_empty()
                && book.title.len() <= 200
                && matches!(book.status.as_str(), "running" | "prepared" | "done")
                && book.current_cell_number <= 64
                && book.current_cell_name.chars().count() <= 40
        })
        && detail.messages.iter().all(|message| {
            matches!(
                SessionRole::try_from(message.role),
                Ok(SessionRole::User | SessionRole::Assistant)
            ) && !message.text.is_empty()
                && (message.role == SessionRole::Assistant as i32
                    || (message.tools.is_empty()
                        && message.reasoning_summaries.is_empty()
                        && message.commentary.is_empty()
                        && message.duration_ms == 0))
                && message.tools.len() <= SESSION_DETAIL_MAX_TOOL_SUMMARIES
                && message.duration_ms <= 86_400_000
                && message.reasoning_summaries.len() <= 8_193
                && message.commentary.len() <= 8_449
                && message.commentary.iter().all(|item| {
                    !item.text.trim().is_empty()
                        && item.text.len() <= 2_097_152
                        && item.after_tool_count as usize <= message.tools.len()
                })
                && message
                    .commentary
                    .windows(2)
                    .all(|pair| pair[0].after_tool_count <= pair[1].after_tool_count)
                && message.reasoning_summaries.iter().all(|summary| {
                    !summary.text.trim().is_empty()
                        && summary.text.len() <= 16_384
                        && summary.after_tool_count as usize <= message.tools.len()
                })
                && message
                    .reasoning_summaries
                    .windows(2)
                    .all(|pair| pair[0].after_tool_count <= pair[1].after_tool_count)
                && message.tools.iter().all(|tool| {
                    uuid::Uuid::parse_str(&tool.id).is_ok()
                        && !tool.name.is_empty()
                        && tool.name.len() <= 128
                        && !tool.summary.is_empty()
                        && tool.summary.len() <= 512
                })
        })
}

fn valid_subagent_summary(item: &SessionSubagentSummary) -> bool {
    uuid::Uuid::parse_str(&item.id).is_ok()
        && !item.parent_role.is_empty()
        && item.parent_role.len() <= 32
        && !item.role.is_empty()
        && item.role.len() <= 32
        && !item.task.is_empty()
        && item.task.len() <= 16_384
        && matches!(item.status.as_str(), "active" | "completed" | "failed")
        && (item.status == "active") == (item.completed_at_ms == 0)
}

fn valid_stream_event(event: &SessionStreamEvent) -> bool {
    if !event.subagent_id.is_empty() && uuid::Uuid::parse_str(&event.subagent_id).is_err() {
        return false;
    }
    if event.subagent.is_some()
        && !matches!(
            SessionStreamKind::try_from(event.kind),
            Ok(SessionStreamKind::SubagentStarted | SessionStreamKind::SubagentCompleted)
        )
    {
        return false;
    }
    if event.kind != SessionStreamKind::UsageUpdated as i32 && event.usage_delta.is_some() {
        return false;
    }
    if event.kind != SessionStreamKind::RunbookProgress as i32 && event.runbook_progress.is_some() {
        return false;
    }
    match SessionStreamKind::try_from(event.kind) {
        Ok(SessionStreamKind::MessageDelta | SessionStreamKind::ReasoningDelta) => {
            !event.text.is_empty()
                && event.text.len() <= 1_048_576
                && event.tool.is_none()
                && event.detail.is_none()
                && event.segment_index > 0
        }
        Ok(SessionStreamKind::ToolStarted | SessionStreamKind::ToolFinished) => {
            event.text.is_empty()
                && event.detail.is_none()
                && event.tool.as_ref().is_some_and(|tool| {
                    uuid::Uuid::parse_str(&tool.id).is_ok()
                        && !tool.name.is_empty()
                        && tool.name.len() <= 128
                        && !tool.summary.is_empty()
                        && tool.summary.len() <= 512
                })
        }
        Ok(
            SessionStreamKind::Completed
            | SessionStreamKind::TurnCompleted
            | SessionStreamKind::SessionStarted
            | SessionStreamKind::AgentStarted
            | SessionStreamKind::AgentTurnCompleted,
        ) => {
            event.text.is_empty()
                && event.tool.is_none()
                && event.detail.as_ref().is_some_and(valid_detail)
        }
        Ok(SessionStreamKind::Failed) => {
            !event.text.is_empty()
                && event.text.len() <= 512
                && event.tool.is_none()
                && event.detail.is_none()
        }
        Ok(SessionStreamKind::SubagentsSpawned) => {
            event.text.is_empty() && event.tool.is_none() && event.detail.is_none()
        }
        Ok(SessionStreamKind::SubagentStarted | SessionStreamKind::SubagentCompleted) => {
            event.text.is_empty()
                && event.tool.is_none()
                && event.detail.is_none()
                && event.subagent.as_ref().is_some_and(|item| {
                    valid_subagent_summary(item)
                        && item.id == event.subagent_id
                        && item.parent_role == event.agent_role
                        && (event.kind == SessionStreamKind::SubagentStarted as i32)
                            == (item.status == "active")
                })
        }
        Ok(SessionStreamKind::RunbookProgress) => {
            event.text.is_empty()
                && event.tool.is_none()
                && event.detail.is_none()
                && event.runbook_progress.as_ref().is_some_and(|progress| {
                    !progress.runbook_id.is_empty()
                        && progress.runbook_id.len() <= 128
                        && !progress.title.is_empty()
                        && progress.title.len() <= 200
                        && progress.cell_number <= 64
                        && progress.cell_name.chars().count() <= 40
                })
        }
        Ok(SessionStreamKind::UsageUpdated) => {
            event.text.is_empty()
                && event.tool.is_none()
                && event.detail.is_none()
                && event.usage_delta.as_ref().is_some_and(|model| {
                    (1..=6).contains(&model.provider)
                        && (1..=2).contains(&model.method)
                        && !model.model.is_empty()
                        && model.model.len() <= 200
                        && model.role.len() <= 20
                        && model.usage.as_ref().is_some_and(|usage| {
                            usage.total_calls == 1
                                && usage.measured_calls <= 1
                                && usage.cache_measured_calls <= usage.measured_calls
                                && usage.cache_read_tokens <= usage.cache_input_tokens
                                && usage.cache_input_tokens <= usage.input_tokens
                        })
                })
        }
        _ => false,
    }
}

pub(crate) async fn load_tool_page(
    client: &Client,
    addr: SocketAddr,
    session_id: &str,
    tool_id: &str,
    offset: u64,
) -> Result<SessionToolDetailPage, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let path = SESSION_TOOL_DETAIL_PATH
        .replace("{id}", session_id)
        .replace("{tool_id}", tool_id);
    let reply: SessionToolDetailPage = decode(
        client
            .get(url(addr, &format!("{path}?offset={offset}")))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| "Could not load tool details".to_owned())?,
    )
    .await?;
    if reply.id != tool_id
        || reply.name.is_empty()
        || reply.arguments.len() > 65_536
        || reply.output.len() > 8_000
        || reply.offset != offset
        || reply.next_offset < offset
        || reply.next_offset > reply.total_bytes
    {
        return Err("Invalid app-server tool details".to_owned());
    }
    Ok(reply)
}

pub(crate) async fn load_subagent(
    client: &Client,
    addr: SocketAddr,
    session_id: &str,
    agent_id: &str,
    offset: u64,
) -> Result<SessionSubagentDetail, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let path = SESSION_SUBAGENT_DETAIL_PATH
        .replace("{id}", session_id)
        .replace("{agent_id}", agent_id);
    let reply: SessionSubagentDetail = decode(
        client
            .get(url(addr, &format!("{path}?offset={offset}")))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| "Could not load subagent".to_owned())?,
    )
    .await?;
    if !reply.summary.as_ref().is_some_and(|summary| {
        valid_subagent_summary(summary) && summary.id == agent_id && summary.status != "active"
    }) || reply.items.len() > 64
        || reply.offset != offset
        || reply.next_offset != offset.saturating_add(reply.items.len() as u64)
        || reply.next_offset > reply.total_items
        || (reply.items.is_empty() && reply.next_offset < reply.total_items)
        || reply.result.len() > 65_536
        || reply.task.is_empty()
        || reply.task.len() > 16_384
        || reply.items.iter().any(|item| {
            !matches!(item.kind.as_str(), "message" | "reasoning" | "tool")
                || item.text.len() > 65_536
                || (item.kind == "tool") != item.tool.is_some()
                || item
                    .tool
                    .as_ref()
                    .is_some_and(|tool| uuid::Uuid::parse_str(&tool.id).is_err())
        })
    {
        return Err("Invalid app-server subagent detail".to_owned());
    }
    Ok(reply)
}

pub(crate) async fn list(client: &Client, addr: SocketAddr) -> Result<Vec<SessionSummary>, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let reply: ListSessionsResponse = decode(
        client
            .get(url(addr, SESSIONS_PATH))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| "Could not list sessions".to_owned())?,
    )
    .await?;
    if reply.sessions.len() > 1_024
        || reply.sessions.iter().any(|session| {
            uuid::Uuid::parse_str(&session.id).is_err()
                || uuid::Uuid::parse_str(&session.workspace_id).is_err()
                || session.title.is_empty()
                || session.title.len() > 512
                || session.last_message_at_ms == 0
        })
    {
        return Err("Invalid app-server session list".to_owned());
    }
    Ok(reply.sessions)
}

pub(crate) async fn archive(client: &Client, addr: SocketAddr, id: &str) -> Result<(), String> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err("Invalid session ID".to_owned());
    }
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let path = SESSION_ARCHIVE_PATH.replace("{id}", id);
    let response = client
        .post(url(addr, &path))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Could not archive session".to_owned())?;
    match response.status() {
        StatusCode::NO_CONTENT => Ok(()),
        StatusCode::NOT_FOUND => Err("Session is no longer available".to_owned()),
        _ => Err(format!("Could not archive session ({})", response.status())),
    }
}

pub(crate) async fn load(
    client: &Client,
    addr: SocketAddr,
    id: &str,
) -> Result<SessionDetail, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let path = SESSION_DETAIL_PATH.replace("{id}", id);
    let reply: SessionDetail = decode(
        client
            .get(url(addr, &path))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| "Could not load session".to_owned())?,
    )
    .await?;
    if !valid_detail(&reply)
        || reply
            .summary
            .as_ref()
            .is_none_or(|summary| summary.id != id)
    {
        return Err("Invalid app-server session detail".to_owned());
    }
    Ok(reply)
}

pub(crate) async fn submit_stream(
    client: &Client,
    addr: SocketAddr,
    request: SubmitSessionTurnRequest,
    turn_id: &str,
    mut on_progress: impl FnMut(SessionStreamEvent),
) -> Result<SessionDetail, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let mut response = client
        .post(url(addr, SESSION_TURN_STREAM_PATH))
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .header(SESSION_TURN_ID_HEADER, turn_id)
        .body(request.encode_to_vec())
        .send()
        .await
        .map_err(|_| "Could not send session turn".to_owned())?;
    if response.status() != StatusCode::OK {
        return Err(format!("Session request failed ({})", response.status()));
    }
    if response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_STREAM_CONTENT_TYPE)
    {
        return Err("Invalid app-server session stream".to_owned());
    }
    let mut decoder = StreamDecoder::default();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Session stream was interrupted".to_owned())?
    {
        for event in decoder.push(&chunk)? {
            if !valid_stream_event(&event) {
                return Err("Invalid app-server session stream event".to_owned());
            }
            match SessionStreamKind::try_from(event.kind) {
                Ok(SessionStreamKind::TurnCompleted) => {
                    if event.detail.as_ref().is_none_or(|detail| {
                        detail
                            .summary
                            .as_ref()
                            .is_none_or(|summary| summary.workspace_id != request.workspace_id)
                    }) {
                        return Err("Invalid app-server session detail".to_owned());
                    }
                    on_progress(event);
                }
                Ok(SessionStreamKind::Completed) => {
                    let detail = event.detail.ok_or("Missing session detail".to_owned())?;
                    if detail.summary.as_ref().is_none_or(|summary| {
                        summary.workspace_id != request.workspace_id
                            || (!request.session_id.is_empty() && summary.id != request.session_id)
                    }) {
                        return Err("Invalid app-server session detail".to_owned());
                    }
                    return Ok(detail);
                }
                Ok(SessionStreamKind::Failed) => return Err(event.text),
                _ => on_progress(event),
            }
        }
    }
    Err("Session stream ended before completion".to_owned())
}

pub(crate) async fn stop_turn(
    client: &Client,
    addr: SocketAddr,
    turn_id: &str,
) -> Result<(), String> {
    if uuid::Uuid::parse_str(turn_id).is_err() {
        return Err("Invalid turn ID".to_owned());
    }
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let path = SESSION_TURN_STOP_PATH.replace("{id}", turn_id);
    let response = client
        .post(url(addr, &path))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Could not stop session turn".to_owned())?;
    match response.status() {
        StatusCode::NO_CONTENT => Ok(()),
        StatusCode::NOT_FOUND => Err("Session turn has already finished".to_owned()),
        _ => Err(format!(
            "Could not stop session turn ({})",
            response.status()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_contracts::session_stream_frame;

    #[test]
    fn model_breakdown_validates_coverage_and_reconciles_with_session_totals() {
        use beale_contracts::v1::{SessionModelUsage, SessionTokenUsage};
        let usage = SessionTokenUsage {
            input_tokens: 100,
            output_tokens: 20,
            measured_calls: 1,
            total_calls: 2,
            cache_read_tokens: 40,
            cache_input_tokens: 100,
            cache_measured_calls: 1,
        };
        let mut detail = SessionDetail {
            token_usage: Some(usage.clone()),
            model_usage: vec![SessionModelUsage {
                provider: 1,
                method: 1,
                model: "example-model".into(),
                role: "main".into(),
                usage: Some(usage),
            }],
            ..Default::default()
        };
        assert!(valid_model_usage(&detail));
        detail.model_usage[0].usage.as_mut().unwrap().input_tokens += 1;
        assert!(!valid_model_usage(&detail));
        detail.model_usage[0].usage.as_mut().unwrap().input_tokens -= 1;
        detail.model_usage.push(detail.model_usage[0].clone());
        assert!(!valid_model_usage(&detail));
        detail.model_usage.pop();
        detail.model_usage[0]
            .usage
            .as_mut()
            .unwrap()
            .cache_read_tokens = 101;
        assert!(!valid_model_usage(&detail));
        detail.model_usage.clear();
        assert!(valid_model_usage(&detail));
    }

    #[test]
    fn scoped_activity_reconciles_with_roles_and_rejects_partial_or_decreasing_samples() {
        let state = crate::visual_preview::fixture("session-insights");
        let detail = state.active_session.unwrap();
        assert!(valid_scoped_snapshots(&detail));
        let mut invalid = detail.clone();
        invalid.usage_snapshots[1].main_usage = None;
        assert!(!valid_scoped_snapshots(&invalid));
        invalid = detail.clone();
        invalid.usage_snapshots[2]
            .main_usage
            .as_mut()
            .unwrap()
            .input_tokens = 1;
        assert!(!valid_scoped_snapshots(&invalid));
        invalid = detail.clone();
        invalid
            .usage_snapshots
            .last_mut()
            .unwrap()
            .subagent_usage
            .as_mut()
            .unwrap()
            .total_calls -= 1;
        assert!(!valid_scoped_snapshots(&invalid));
        invalid = detail.clone();
        invalid.model_usage[0].role = "scout".into();
        assert!(!valid_scoped_snapshots(&invalid));
        invalid = detail.clone();
        invalid.usage_snapshots[1]
            .main_usage
            .as_mut()
            .unwrap()
            .cache_read_tokens = 100_000;
        assert!(!valid_scoped_snapshots(&invalid));
        invalid = detail;
        for point in &mut invalid.usage_snapshots {
            point.main_usage = None;
            point.subagent_usage = None;
        }
        assert!(valid_scoped_snapshots(&invalid));
    }

    #[test]
    fn live_usage_updates_reconcile_across_top_level_and_subagents() {
        use beale_contracts::v1::{SessionModelUsage, SessionTokenUsage};
        let state = crate::visual_preview::fixture("session-insights");
        let saved = state.active_session.unwrap();
        let before = saved.token_usage.as_ref().unwrap().clone();
        let event = |role: &str, input_tokens| SessionStreamEvent {
            kind: SessionStreamKind::UsageUpdated as i32,
            agent_role: "discovery".to_owned(),
            usage_delta: Some(SessionModelUsage {
                provider: 1,
                method: 1,
                model: "example-model".to_owned(),
                role: role.to_owned(),
                usage: Some(SessionTokenUsage {
                    input_tokens,
                    output_tokens: 4,
                    measured_calls: 1,
                    total_calls: 1,
                    ..Default::default()
                }),
            }),
            ..Default::default()
        };
        let events = [event("discovery", 10), event("scout", 20)];
        assert!(events.iter().all(valid_stream_event));
        let live = detail_with_live_usage(Some(&saved), &events, 5_000).unwrap();
        let total = live.token_usage.as_ref().unwrap();
        assert_eq!(total.input_tokens, before.input_tokens + 30);
        assert_eq!(total.output_tokens, before.output_tokens + 8);
        assert_eq!(total.total_calls, before.total_calls + 2);
        assert!(valid_model_usage(&live));
        assert!(valid_scoped_snapshots(&live));
        let last = live.usage_snapshots.last().unwrap();
        assert_eq!(
            last.main_usage.as_ref().unwrap().input_tokens,
            saved
                .usage_snapshots
                .last()
                .unwrap()
                .main_usage
                .as_ref()
                .unwrap()
                .input_tokens
                + 10
        );
        assert_eq!(
            last.subagent_usage.as_ref().unwrap().input_tokens,
            saved
                .usage_snapshots
                .last()
                .unwrap()
                .subagent_usage
                .as_ref()
                .unwrap()
                .input_tokens
                + 20
        );
        let mut invalid = events[0].clone();
        invalid
            .usage_delta
            .as_mut()
            .unwrap()
            .usage
            .as_mut()
            .unwrap()
            .total_calls = 0;
        assert!(!valid_stream_event(&invalid));
    }

    #[test]
    fn stream_decoder_handles_fragmented_and_adjacent_frames() {
        let first = SessionStreamEvent {
            runbook_progress: None,
            kind: SessionStreamKind::ReasoningDelta as i32,
            text: "Checking example input".to_owned(),
            tool: None,
            detail: None,
            segment_index: 1,
            ..Default::default()
        };
        let second = SessionStreamEvent {
            runbook_progress: None,
            kind: SessionStreamKind::MessageDelta as i32,
            text: "Example answer".to_owned(),
            tool: None,
            detail: None,
            segment_index: 1,
            ..Default::default()
        };
        let mut bytes = session_stream_frame(&first).unwrap();
        bytes.extend(session_stream_frame(&second).unwrap());
        let mut decoder = StreamDecoder::default();
        assert!(decoder.push(&bytes[..2]).unwrap().is_empty());
        let events = decoder.push(&bytes[2..]).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].text, first.text);
        assert_eq!(events[1].text, second.text);
        assert!(decoder.pending.is_empty());
    }

    #[test]
    fn stream_decoder_rejects_oversized_frames() {
        let mut decoder = StreamDecoder::default();
        let length = (SESSION_STREAM_MAX_FRAME_BYTES as u32 + 1).to_be_bytes();
        assert!(decoder.push(&length).is_err());
    }

    #[test]
    fn spawn_event_has_no_model_visible_payload() {
        let event = SessionStreamEvent {
            runbook_progress: None,
            kind: SessionStreamKind::SubagentsSpawned as i32,
            ..Default::default()
        };
        assert!(valid_stream_event(&event));
        assert!(!valid_stream_event(&SessionStreamEvent {
            runbook_progress: None,
            text: "unexpected".to_owned(),
            ..event
        }));
    }

    #[test]
    fn runbook_progress_event_requires_bounded_identity_and_cell() {
        let event = SessionStreamEvent {
            kind: SessionStreamKind::RunbookProgress as i32,
            runbook_progress: Some(beale_contracts::v1::RunbookProgress {
                runbook_id: "R1".to_owned(),
                title: "Example procedure".to_owned(),
                cell_number: 2,
                cell_name: "Example step".to_owned(),
            }),
            ..Default::default()
        };
        assert!(valid_stream_event(&event));
        let mut invalid = event.clone();
        invalid.runbook_progress.as_mut().unwrap().cell_number = 65;
        assert!(!valid_stream_event(&invalid));
        invalid = event.clone();
        invalid.runbook_progress.as_mut().unwrap().title.clear();
        assert!(!valid_stream_event(&invalid));
        let mut invalid = event;
        invalid.runbook_progress.as_mut().unwrap().cell_name = "x".repeat(41);
        assert!(!valid_stream_event(&invalid));
    }

    #[test]
    fn peer_graph_detail_accepts_top_level_usage_and_graph_turn_activity() {
        use beale_contracts::v1::{
            SessionGraphAgent, SessionMessage, SessionModelUsage, SessionTokenUsage,
            SessionUsageSnapshot,
        };
        let usage = SessionTokenUsage {
            input_tokens: 100,
            output_tokens: 20,
            measured_calls: 1,
            total_calls: 1,
            ..Default::default()
        };
        let mut detail = SessionDetail {
            summary: Some(SessionSummary {
                id: uuid::Uuid::new_v4().to_string(),
                workspace_id: uuid::Uuid::new_v4().to_string(),
                title: "Example research".to_owned(),
                last_message_at_ms: 1,
            }),
            messages: vec![
                SessionMessage {
                    role: SessionRole::User as i32,
                    text: "Check the example package".to_owned(),
                    agent_role: "discovery".to_owned(),
                    ..Default::default()
                },
                SessionMessage {
                    role: SessionRole::Assistant as i32,
                    text: "Example observation".to_owned(),
                    duration_ms: 1,
                    agent_role: "discovery".to_owned(),
                    ..Default::default()
                },
            ],
            provider: 1,
            method: 1,
            model: "example-model".to_owned(),
            first_turn_number: 1,
            graph_id: beale_graphs::OPENAI_ID.to_owned(),
            total_duration_ms: 1,
            token_usage: Some(usage.clone()),
            model_usage: vec![SessionModelUsage {
                provider: 1,
                method: 1,
                model: "example-model".to_owned(),
                role: "discovery".to_owned(),
                usage: Some(usage.clone()),
            }],
            usage_snapshots: vec![SessionUsageSnapshot {
                work_duration_ms: 1,
                input_tokens: 100,
                output_tokens: 20,
                measured_calls: 1,
                total_calls: 1,
                main_usage: Some(usage),
                subagent_usage: Some(SessionTokenUsage::default()),
                ..Default::default()
            }],
            graph_agents: ["discovery", "validation", "reporter", "patch_generation"]
                .into_iter()
                .map(|role| SessionGraphAgent {
                    role: role.to_owned(),
                    status: "idle".to_owned(),
                })
                .collect(),
            ..Default::default()
        };
        let message = &mut detail.messages[1];
        for index in 0..708 {
            message.tools.push(beale_contracts::v1::SessionToolSummary {
                id: uuid::Uuid::new_v4().to_string(),
                name: "read".to_owned(),
                summary: format!("Read synthetic file {index}"),
                agent_role: "discovery".to_owned(),
                ..Default::default()
            });
        }
        assert!(valid_detail(&detail));
        detail.messages[1].commentary = (0..40)
            .map(|index| beale_contracts::v1::SessionCommentary {
                text: format!("Example progress {index}"),
                after_tool_count: index,
                agent_role: "discovery".to_owned(),
            })
            .collect();
        assert!(valid_detail(&detail));
        let started = SessionStreamEvent {
            kind: SessionStreamKind::SessionStarted as i32,
            detail: Some(detail.clone()),
            ..Default::default()
        };
        assert!(valid_stream_event(&started));
        let mut oversized = detail.clone();
        let message = &mut oversized.messages[1];
        for _ in message.tools.len()..=SESSION_DETAIL_MAX_TOOL_SUMMARIES {
            message.tools.push(beale_contracts::v1::SessionToolSummary {
                id: uuid::Uuid::new_v4().to_string(),
                name: "read".to_owned(),
                summary: "Read synthetic file".to_owned(),
                ..Default::default()
            });
        }
        assert!(!valid_detail(&oversized));
        let mut invalid = detail;
        invalid.graph_agents[1].status = "unknown".to_owned();
        assert!(!valid_detail(&invalid));
    }
}
