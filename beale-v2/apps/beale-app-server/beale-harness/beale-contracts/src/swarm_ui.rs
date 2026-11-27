//! Client presentation snapshots. No provider credentials, checkpoint context,
//! command arguments, or host filesystem paths are exposed here.
use crate::swarms::{AgentSummary, Destination, PacingTarget};
use serde::{Deserialize, Serialize};

pub const PEER_REGISTRATION_PATH: &str = "/v1/fleet/execution-peer";
/// Trusted Desktop-to-host pairing only. Never included in snapshots or model
/// context; deliberately does not implement Debug.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerRegistration {
    pub host_id: String,
    pub endpoint: String,
    pub control_token: String,
    pub enabled: bool,
    pub protocol_major: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanOverview {
    pub revision: u64,
    pub definition_id: String,
    pub definition_revision: u32,
    pub leader_rule: String,
    pub destinations: Vec<Destination>,
    pub pacing: Vec<PacingTarget>,
    pub agents: Vec<AgentOverview>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOverview {
    pub summary: AgentSummary,
    pub pending_assignments: usize,
    pub queued_wakes: usize,
    pub termination_unconfirmed: bool,
    pub unknown_commands: usize,
    pub resumable: bool,
}
