//! Assigned Runbook execution: canonical snapshots and results, no host paths.
use crate::{runbooks::Document, swarms::valid_id};
use serde::{Deserialize, Serialize};

pub const MAX_CELL_OUTPUT_BYTES: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellOutcome {
    pub cell_id: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}
impl CellOutcome {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_id(&self.cell_id)
            || self.stdout.len().saturating_add(self.stderr.len()) > MAX_CELL_OUTPUT_BYTES
            || self.success != (self.exit_code == Some(0))
        {
            return Err("invalid assigned Runbook cell result");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Prepare {
        runbook_id: String,
    },
    Cell {
        run_id: String,
        outcome: CellOutcome,
    },
    BeginCell {
        run_id: String,
        cell_id: String,
    },
    Finish {
        run_id: String,
    },
}
impl Request {
    pub fn validate(&self) -> Result<(), &'static str> {
        let id = match self {
            Self::Prepare { runbook_id } => runbook_id,
            Self::BeginCell { run_id, cell_id } => {
                if !valid_id(cell_id) {
                    return Err("invalid Runbook cell identity");
                }
                run_id
            }
            Self::Cell { run_id, outcome } => {
                outcome.validate()?;
                run_id
            }
            Self::Finish { run_id } => run_id,
        };
        if !valid_id(id) {
            return Err("invalid assigned Runbook identity");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedRun {
    pub run_id: String,
    pub document: Document,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinishedRun {
    pub run_id: String,
    pub runbook_id: String,
    pub success: bool,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub identity: crate::swarm_worker::ExecutionIdentity,
    pub result: ResultKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResultKind {
    Prepared { run: PreparedRun },
    CellAccepted { run_id: String, cell_id: String },
    CellReady { run_id: String, cell_id: String },
    Finished { run: FinishedRun },
    Denied {},
    Rejected { error: crate::tool_error::ToolError },
}

impl Reply {
    pub fn validate_for(
        &self,
        identity: &crate::swarm_worker::ExecutionIdentity,
        request: &Request,
    ) -> Result<(), &'static str> {
        self.identity.validate()?;
        if self.identity != *identity {
            return Err("Runbook reply identity mismatch");
        }
        let valid = match (&self.result, request) {
            (
                ResultKind::CellReady { run_id, cell_id },
                Request::BeginCell {
                    run_id: expected_run,
                    cell_id: expected_cell,
                },
            ) => run_id == expected_run && cell_id == expected_cell,
            (ResultKind::Denied {}, Request::Prepare { .. }) => true,
            (ResultKind::Rejected { error }, Request::Prepare { .. }) => error.validate().is_ok(),
            (ResultKind::Prepared { run }, Request::Prepare { runbook_id }) => {
                let d = &run.document;
                valid_id(&run.run_id)
                    && d.id == *runbook_id
                    && d.workspace_id == identity.workspace_id
                    && d.revision > 0
                    && !d.cells.is_empty()
                    && d.cells.len() <= 64
                    && serde_json::to_vec(d).is_ok_and(|v| v.len() <= 56_000)
                    && d.cells.iter().all(|c| valid_id(c.id()))
                    && d.cells
                        .iter()
                        .map(|c| c.id())
                        .collect::<std::collections::HashSet<_>>()
                        .len()
                        == d.cells.len()
                    && d.cells
                        .iter()
                        .any(|c| matches!(c, crate::runbooks::Cell::Code { .. }))
            }
            (
                ResultKind::CellAccepted { run_id, cell_id },
                Request::Cell {
                    run_id: expected,
                    outcome,
                },
            ) => run_id == expected && cell_id == &outcome.cell_id,
            (ResultKind::Finished { run }, Request::Finish { run_id }) => {
                run.run_id == *run_id && valid_id(&run.runbook_id) && run.output.len() <= 64_000
            }
            _ => false,
        };
        if !valid {
            return Err("invalid Runbook reply");
        }
        Ok(())
    }
}
