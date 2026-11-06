//! Versioned JSON contract for the optional Knowledge feature.

use serde::{Deserialize, Serialize};

pub const QUERY_PATH: &str = "/v1/knowledge/query";
pub const GET_PATH: &str = "/v1/knowledge/get";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Memory,
    Claim,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub id: String,
    pub tool_call_id: String,
    pub session_id: String,
    pub summary: String,
    pub output_sha256: String,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub target_id: String,
    pub relation: String,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub number: u64,
    pub at_ms: u64,
    pub actor: String,
    pub session_id: String,
    pub reason: String,
    pub before_json: String,
    pub after_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Data {
    Memory {
        category: String,
        role: String,
        support: String,
    },
    Claim {
        stage: String,
        disposition: String,
        rating: String,
        #[serde(default)]
        attacker_control: String,
        #[serde(default)]
        violated_invariant: String,
        #[serde(default)]
        observed_behavior: String,
        impact: String,
        reproduction_steps: String,
        source_revision: String,
        environment: String,
    },
}

impl Data {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Memory { .. } => Kind::Memory,
            Self::Claim { .. } => Kind::Claim,
        }
    }

    pub fn status(&self) -> &str {
        match self {
            Self::Memory { support, .. } => support,
            Self::Claim { stage, .. } => stage,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub summary: String,
    pub body: String,
    pub surface_id: String,
    pub data: Data,
    pub evidence: Vec<Evidence>,
    pub links: Vec<Link>,
    pub duplicate_of: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub revision: u64,
    pub history: Vec<Revision>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub summary: String,
    pub category: String,
    pub role: String,
    pub status: String,
    pub disposition: String,
    pub surface_id: String,
    pub evidence_count: usize,
    pub updated_at_ms: u64,
    pub revision: u64,
}

impl From<&Record> for Summary {
    fn from(record: &Record) -> Self {
        let (category, role, disposition) = match &record.data {
            Data::Memory { category, role, .. } => (category.clone(), role.clone(), String::new()),
            Data::Claim { disposition, .. } => (String::new(), String::new(), disposition.clone()),
        };
        Self {
            id: record.id.clone(),
            kind: record.data.kind(),
            title: record.title.clone(),
            summary: record.summary.clone(),
            category,
            role,
            status: record.data.status().to_owned(),
            disposition,
            surface_id: record.surface_id.clone(),
            evidence_count: record.evidence.len(),
            updated_at_ms: record.updated_at_ms,
            revision: record.revision,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    pub workspace_id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub kind: Option<Kind>,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub surface_id: String,
    #[serde(default)]
    pub include_duplicates: bool,
    #[serde(default)]
    pub limit: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryResponse {
    pub records: Vec<Summary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetRequest {
    pub workspace_id: String,
    pub id: String,
    #[serde(default)]
    pub history_offset: u32,
    #[serde(default)]
    pub history_limit: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetResponse {
    pub record: Record,
    pub history_total: usize,
    pub next_history_offset: Option<u32>,
}

pub fn page_record(mut record: Record, offset: u32, limit: u32) -> GetResponse {
    let history_total = record.history.len();
    let limit = if limit == 0 { 5 } else { limit.min(5) } as usize;
    let offset = offset as usize;
    record.history = record
        .history
        .into_iter()
        .rev()
        .skip(offset)
        .take(limit)
        .collect();
    let next = offset.saturating_add(record.history.len());
    GetResponse {
        record,
        history_total,
        next_history_offset: (next < history_total).then_some(next as u32),
    }
}
