//! Versioned JSON document contract for the optional Runbooks feature.

use serde::{Deserialize, Serialize};

pub const QUERY_PATH: &str = "/v1/runbooks/query";
pub const GET_PATH: &str = "/v1/runbooks/get";
pub const CELL_EDIT_PATH: &str = "/v1/runbooks/cells/edit";
pub const CELL_RUN_PATH: &str = "/v1/runbooks/cells/run";
pub const MAX_TITLE_CHARS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Cell {
    Markdown {
        id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        name: String,
        source: String,
    },
    Code {
        id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        name: String,
        language: String,
        source: String,
    },
}

impl Cell {
    pub fn id(&self) -> &str {
        match self {
            Self::Markdown { id, .. } | Self::Code { id, .. } => id,
        }
    }

    pub fn id_mut(&mut self) -> &mut String {
        match self {
            Self::Markdown { id, .. } | Self::Code { id, .. } => id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Markdown { name, .. } | Self::Code { name, .. } => name,
        }
    }

    pub fn name_mut(&mut self) -> &mut String {
        match self {
            Self::Markdown { name, .. } | Self::Code { name, .. } => name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub purpose: String,
    pub cells: Vec<Cell>,
    pub next_cell_number: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub purpose: String,
    pub languages: Vec<String>,
    pub cell_count: usize,
    pub updated_at_ms: u64,
    pub revision: u64,
}

impl From<&Document> for Summary {
    fn from(document: &Document) -> Self {
        let mut languages = Vec::new();
        for cell in &document.cells {
            if let Cell::Code { language, .. } = cell
                && !languages.contains(language)
            {
                languages.push(language.clone());
            }
        }
        Self {
            id: document.id.clone(),
            title: document.title.clone(),
            purpose: document.purpose.clone(),
            languages,
            cell_count: document.cells.len(),
            updated_at_ms: document.updated_at_ms,
            revision: document.revision,
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
    pub language: String,
    #[serde(default)]
    pub limit: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryResponse {
    pub runbooks: Vec<Summary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetRequest {
    pub workspace_id: String,
    pub id: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetResponse {
    pub runbook: Document,
    #[serde(default)]
    pub runs: Vec<RunExecution>,
    #[serde(default)]
    pub cell_results: Vec<CellResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellResult {
    pub cell_id: String,
    pub status: String,
    pub updated_at_ms: u64,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunExecution {
    pub id: String,
    pub revision: u64,
    pub status: String,
    pub output: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub current_cell_number: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellEditRequest {
    pub workspace_id: String,
    pub session_id: String,
    pub id: String,
    pub cell_id: String,
    pub expected_revision: u64,
    pub change: CellChange,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CellChange {
    SetLanguage { language: String },
    SetName { name: String },
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellEditResponse {
    pub runbook: Document,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellRunRequest {
    pub workspace_id: String,
    pub session_id: String,
    pub id: String,
    pub cell_id: String,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellRunResponse {
    pub run_id: String,
    pub status: String,
}
