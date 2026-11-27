//! Host-only provider accounting protocol. Never included in worker/model context.
use crate::swarms::PacingTarget;
use serde::{Deserialize, Serialize};

pub const PATH: &str = "/v1/provider-accounting";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AccountBinding {
    pub group_id: String,
    pub provider_id: String,
    pub owner_host_id: String,
    pub generation: u64,
    pub participants: Vec<String>,
    pub limited_identity: bool,
}

/// Stable account denomination; contains no active rate or allowance amount.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AccountingUnit {
    Currency { currency: String },
    Credits,
    Subscription { window_id: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CallClass {
    Research,
    Supporting,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Admission {
    pub id: String,
    pub group_id: String,
    pub generation: u64,
    pub participant_id: String,
    pub scan_id: String,
    pub target: Option<PacingTarget>,
    pub class: CallClass,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CostQuality {
    Measured,
    Estimated,
    Unknown,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoordinationMode {
    Coordinated,
    Degraded,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Charge {
    /// Stable event ID equals the physical attempt ID; retries use new IDs.
    pub attempt_id: String,
    pub amount: Option<u64>,
    pub quality: CostQuality,
    /// Interrupted streams can report billable tokens without establishing the
    /// final total. Retain those tokens and label possible additional cost.
    pub uncertain: bool,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub model: String,
    pub currency_or_credit: String,
    /// Currency/credit micros per million tokens, not dollars per token.
    pub input_micros: u64,
    pub output_micros: u64,
    pub cached_input_micros: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub window_id: String,
    pub used_basis_points: u32,
    pub resets_at_ms: u64,
    pub observed_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdmissionReply {
    pub admitted: bool,
    pub retry_at_ms: u64,
    /// Acknowledged conservative allocation, persisted for disconnected use.
    pub allocation: Option<PacingTarget>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AccountingRequest {
    Drain {
        group_id: String,
        generation: u64,
        participant_id: String,
    },
    Resolve {
        provider: i32,
        subscription: bool,
        model: String,
    },
    Bind {
        route: String,
        binding: AccountBinding,
    },
    Status {
        group_id: String,
    },
    Admit {
        request: Admission,
    },
    Settle {
        request: Admission,
        charge: Charge,
    },
    Reconcile {
        request: Admission,
        charge: Charge,
    },
    Cancel {
        request: Admission,
    },
    Observe {
        group_id: String,
        generation: u64,
        observation: Observation,
    },
    Price {
        group_id: String,
        price: Price,
    },
    /// Freeze first, then explicitly transfer the reconciled durable snapshot.
    Freeze {
        group_id: String,
        generation: u64,
    },
    Import {
        snapshot: Box<AccountingSnapshot>,
        owner_host_id: String,
    },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountingSnapshot {
    pub binding: Option<AccountBinding>,
    /// Established by accepted admissions/reconciliation, independently of history.
    pub unit: Option<AccountingUnit>,
    pub frozen: bool,
    pub coordination_mode: Option<CoordinationMode>,
    pub drained_participants: Vec<String>,
    pub next_at_ms: u64,
    pub scan_next: std::collections::BTreeMap<String, u64>,
    pub reservations: Vec<Reservation>,
    pub observations: Vec<Observation>,
    pub prices: Vec<Price>,
    pub last_class: Option<CallClass>,
    /// Older closed attempts remain in the prior authority's ledger. Transfer
    /// preserves scheduling debt, not an unbounded copy of historical events.
    pub omitted_reservations: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reservation {
    pub request: Admission,
    pub queued_at_ms: u64,
    pub touched_at_ms: u64,
    pub admitted: bool,
    pub canceled: bool,
    pub estimate: u64,
    /// Account rate reserved at admission, independent of this scan's target.
    pub aggregate_target: Option<PacingTarget>,
    pub charge: Option<Charge>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum AccountingReply {
    Saved,
    Binding(AccountBinding),
    Admission(AdmissionReply),
    Status(Box<AccountingSnapshot>),
}
