use super::*;
use crate::FleetPeer;
use reqwest::{Client, Url, header::HeaderValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerFailure {
    Retryable,
    Rejected,
    Protocol,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerExecutionError {
    pub kind: PeerFailure,
    pub message: &'static str,
}
impl std::fmt::Display for PeerExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for PeerExecutionError {}
fn error(kind: PeerFailure, message: &'static str) -> PeerExecutionError {
    PeerExecutionError { kind, message }
}

/// Host pairing material stays here; no worker environment receives this client.
#[derive(Clone)]
pub struct HostExecutionClient {
    client: Client,
    endpoint: Url,
    bearer: HeaderValue,
    host_id: String,
}
impl HostExecutionClient {
    pub fn for_peer(peer: &FleetPeer) -> Result<Self, PeerExecutionError> {
        if !peer.enabled || peer.protocol_major != beale_contracts::PROTOCOL_MAJOR {
            return Err(error(PeerFailure::Rejected, "execution peer unavailable"));
        }
        Self::new(&peer.endpoint, &peer.control_token, &peer.id)
    }
    fn new(origin: &str, token: &str, host_id: &str) -> Result<Self, PeerExecutionError> {
        let invalid = || {
            error(
                PeerFailure::Protocol,
                "invalid execution peer configuration",
            )
        };
        validate_origin(origin).map_err(|_| invalid())?;
        if token.len() != 64
            || !token.bytes().all(|b| b.is_ascii_hexdigit())
            || !beale_contracts::swarms::valid_id(host_id)
        {
            return Err(invalid());
        }
        let mut endpoint = Url::parse(origin).map_err(|_| invalid())?;
        endpoint.set_path(HOST_EXECUTION_PATH);
        let mut bearer =
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| invalid())?;
        bearer.set_sensitive(true);
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| invalid())?;
        Ok(Self {
            client,
            endpoint,
            bearer,
            host_id: host_id.into(),
        })
    }
    /// Pairing uses the authenticated descriptor's stable host ID before saving
    /// a FleetPeer. No hostname or desktop-specific connection alias is inferred.
    pub async fn discover(
        origin: &str,
        token: &str,
        timeout: Duration,
    ) -> Result<HostExecutionDescriptor, PeerExecutionError> {
        if timeout.is_zero() || timeout > Duration::from_secs(30) {
            return Err(error(PeerFailure::Protocol, "invalid discovery timeout"));
        }
        let client = Self::new(origin, token, "discovery")?;
        let response = client
            .client
            .get(client.endpoint)
            .header("authorization", client.bearer)
            .timeout(timeout)
            .send()
            .await
            .map_err(|_| error(PeerFailure::Retryable, "execution peer unavailable"))?;
        let bytes = read_reply(response).await?;
        let descriptor: HostExecutionDescriptor = serde_json::from_slice(&bytes)
            .map_err(|_| error(PeerFailure::Protocol, "invalid execution descriptor"))?;
        if descriptor.version != HOST_EXECUTION_VERSION
            || !beale_contracts::swarms::valid_id(&descriptor.host_id)
        {
            return Err(error(
                PeerFailure::Protocol,
                "incompatible execution descriptor",
            ));
        }
        Ok(descriptor)
    }
    pub async fn send(
        &self,
        request: &HostExecutionRequest,
        timeout: Duration,
    ) -> Result<HostExecutionReply, PeerExecutionError> {
        request
            .validate()
            .map_err(|_| error(PeerFailure::Protocol, "invalid host execution request"))?;
        if request.target.host_id != self.host_id
            || timeout.is_zero()
            || timeout > Duration::from_secs(30)
        {
            return Err(error(
                PeerFailure::Protocol,
                "execution destination or timeout mismatch",
            ));
        }
        let response = self
            .client
            .post(self.endpoint.clone())
            .header("authorization", self.bearer.clone())
            .json(request)
            .timeout(timeout)
            .send()
            .await
            .map_err(|_| error(PeerFailure::Retryable, "execution peer unavailable"))?;
        let bytes = read_reply(response).await?;
        let reply: HostExecutionReply = serde_json::from_slice(&bytes)
            .map_err(|_| error(PeerFailure::Protocol, "invalid execution reply"))?;
        reply
            .validate_for(request)
            .map_err(|_| error(PeerFailure::Protocol, "execution acknowledgement mismatch"))?;
        Ok(reply)
    }
}

async fn read_reply(mut response: reqwest::Response) -> Result<Vec<u8>, PeerExecutionError> {
    if !response.status().is_success() {
        let status = response.status();
        let kind = if status.as_u16() == 501 {
            PeerFailure::Rejected
        } else if status.is_server_error() || matches!(status.as_u16(), 408 | 423 | 429) {
            PeerFailure::Retryable
        } else if status.is_redirection() {
            PeerFailure::Protocol
        } else {
            PeerFailure::Rejected
        };
        return Err(error(kind, "execution peer did not acknowledge operation"));
    }
    if response
        .content_length()
        .is_some_and(|size| size > HOST_EXECUTION_MAX_BYTES as u64)
    {
        return Err(error(PeerFailure::Protocol, "execution reply too large"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| error(PeerFailure::Retryable, "execution reply interrupted"))?
    {
        if bytes.len().saturating_add(chunk.len()) > HOST_EXECUTION_MAX_BYTES {
            return Err(error(PeerFailure::Protocol, "execution reply too large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
