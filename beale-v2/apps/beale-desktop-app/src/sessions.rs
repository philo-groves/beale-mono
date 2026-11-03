use beale_contracts::{
    PROTOBUF_CONTENT_TYPE, SESSION_DETAIL_PATH, SESSION_TOOL_DETAIL_PATH, SESSION_TURNS_PATH,
    SESSIONS_PATH,
    v1::{
        ListSessionsResponse, SessionDetail, SessionRole, SessionSummary, SessionToolDetailPage,
        SubmitSessionTurnRequest,
    },
};
use prost::Message;
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use std::{net::SocketAddr, time::Duration};

const MAX_REPLY_BYTES: usize = 4_500_000;

pub(crate) fn client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(600))
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
        && detail.messages.len() <= 64
        && matches!(
            beale_contracts::v1::ProviderId::try_from(detail.provider),
            Ok(beale_contracts::v1::ProviderId::Openai | beale_contracts::v1::ProviderId::Xai)
        )
        && matches!(
            beale_contracts::v1::AuthMethod::try_from(detail.method),
            Ok(beale_contracts::v1::AuthMethod::Subscription
                | beale_contracts::v1::AuthMethod::ApiKey)
        )
        && !detail.model.is_empty()
        && detail.messages.iter().all(|message| {
            matches!(
                SessionRole::try_from(message.role),
                Ok(SessionRole::User | SessionRole::Assistant)
            ) && !message.text.is_empty()
                && (message.role == SessionRole::Assistant as i32 || message.tools.is_empty())
                && message.tools.len() <= 32
                && message.tools.iter().all(|tool| {
                    uuid::Uuid::parse_str(&tool.id).is_ok()
                        && !tool.name.is_empty()
                        && tool.name.len() <= 128
                        && !tool.summary.is_empty()
                        && tool.summary.len() <= 512
                })
        })
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
        })
    {
        return Err("Invalid app-server session list".to_owned());
    }
    Ok(reply.sessions)
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

pub(crate) async fn submit(
    client: &Client,
    addr: SocketAddr,
    request: SubmitSessionTurnRequest,
) -> Result<SessionDetail, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let reply: SessionDetail = decode(
        client
            .post(url(addr, SESSION_TURNS_PATH))
            .bearer_auth(token)
            .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .body(request.encode_to_vec())
            .send()
            .await
            .map_err(|_| "Could not send session turn".to_owned())?,
    )
    .await?;
    if !valid_detail(&reply)
        || reply.summary.as_ref().is_none_or(|summary| {
            summary.workspace_id != request.workspace_id
                || (!request.session_id.is_empty() && summary.id != request.session_id)
        })
    {
        return Err("Invalid app-server session detail".to_owned());
    }
    Ok(reply)
}
