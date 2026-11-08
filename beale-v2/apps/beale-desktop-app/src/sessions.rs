use beale_contracts::{
    PROTOBUF_CONTENT_TYPE, PROTOBUF_STREAM_CONTENT_TYPE, SESSION_ARCHIVE_PATH, SESSION_DETAIL_PATH,
    SESSION_STREAM_MAX_FRAME_BYTES, SESSION_TOOL_DETAIL_PATH, SESSION_TURN_STREAM_PATH,
    SESSIONS_PATH,
    v1::{
        GoalStatus, ListSessionsResponse, SessionDetail, SessionRole, SessionStreamEvent,
        SessionStreamKind, SessionSummary, SessionToolDetailPage, SubmitSessionTurnRequest,
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
                && message.tools.len() <= 32
                && message.duration_ms <= 86_400_000
                && message.reasoning_summaries.len() <= 128
                && message.commentary.len() <= 8
                && message.commentary.iter().all(|item| {
                    !item.text.trim().is_empty()
                        && item.text.len() <= 8_192
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

fn valid_stream_event(event: &SessionStreamEvent) -> bool {
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
        Ok(SessionStreamKind::Completed | SessionStreamKind::TurnCompleted) => {
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
    mut on_progress: impl FnMut(SessionStreamEvent),
) -> Result<SessionDetail, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let mut response = client
        .post(url(addr, SESSION_TURN_STREAM_PATH))
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
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

#[cfg(test)]
mod tests {
    use super::*;
    use beale_contracts::session_stream_frame;

    #[test]
    fn stream_decoder_handles_fragmented_and_adjacent_frames() {
        let first = SessionStreamEvent {
            kind: SessionStreamKind::ReasoningDelta as i32,
            text: "Checking example input".to_owned(),
            tool: None,
            detail: None,
            segment_index: 1,
        };
        let second = SessionStreamEvent {
            kind: SessionStreamKind::MessageDelta as i32,
            text: "Example answer".to_owned(),
            tool: None,
            detail: None,
            segment_index: 1,
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
            kind: SessionStreamKind::SubagentsSpawned as i32,
            ..Default::default()
        };
        assert!(valid_stream_event(&event));
        assert!(!valid_stream_event(&SessionStreamEvent {
            text: "unexpected".to_owned(),
            ..event
        }));
    }
}
