//! Operator-facing JSON contract for local Memories maintenance.

use serde::{Deserialize, Serialize};

pub const STATUS_PATH: &str = "/v1/memories/status";
pub const CLEAR_PATH: &str = "/v1/memories/clear";
pub const RETRY_PATH: &str = "/v1/memories/retry";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRequest {
    pub workspace_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetryRequest {
    pub workspace_id: String,
    pub job_id: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StatusResponse {
    pub entities: u64,
    pub assertions: u64,
    pub processed_turns: u64,
    pub pending_turns: u64,
    pub failed_turns: u64,
    pub failed_job_ids: Vec<i64>,
}
