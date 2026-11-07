//! Versioned JSON document contract for the optional Runbooks feature.

use serde::{Deserialize, Serialize};

pub const QUERY_PATH: &str = "/v1/runbooks/query";
pub const GET_PATH: &str = "/v1/runbooks/get";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Cell {
    Markdown {
        id: String,
        source: String,
    },
    Code {
        id: String,
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetResponse {
    pub runbook: Document,
}
