//! Assigned verification observations. The leader alone evaluates the oracle.
use crate::{findings::Oracle, swarm_worker::ExecutionIdentity, swarms::valid_id};
use serde::{Deserialize, Serialize};

pub const VERIFY_RUNS: u8 = 2;
// Command collection bounds raw bytes at 8 MB. Lossy UTF-8 can expand them.
pub const MAX_OUTPUT_BYTES: usize = 24_000_000;
pub const MAX_OBSERVATION_BYTES: u64 = 144_010_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub id: String,
    pub expected_revision: u64,
    pub command: String,
    pub oracle: Oracle,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}
fn default_timeout() -> u64 {
    120
}
impl Command {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_id(&self.id)
            || self.expected_revision == 0
            || self.expected_revision > i64::MAX as u64
            || self.command.trim().is_empty()
            || self.command.len() > 16_384
            || !(1..=600).contains(&self.timeout_seconds)
        {
            return Err("invalid verification command");
        }
        if let Oracle::StdoutContains { value }
        | Oracle::StderrContains { value }
        | Oracle::OutputContains { value } = &self.oracle
            && (value.trim().is_empty() || value.len() > 1_024 || self.command.contains(value))
        {
            return Err("invalid verification oracle text");
        }
        Ok(())
    }
}

/// Full transcript in an accepted artifact, never a worker-supplied verdict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub identity: ExecutionIdentity,
    pub run_id: String,
    pub index: u8,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: i64,
    pub error: Option<String>,
}
impl Observation {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.identity.validate()?;
        if !valid_id(&self.run_id)
            || self.index >= VERIFY_RUNS
            || self.duration_ms < 0
            || self.stdout.len().saturating_add(self.stderr.len()) > MAX_OUTPUT_BYTES
            || self.error.as_ref().is_some_and(|error| {
                error.trim().is_empty()
                    || error.len() > 2_048
                    || self.exit_code.is_some()
                    || !self.stdout.is_empty()
                    || !self.stderr.is_empty()
            })
        {
            return Err("invalid verification observation");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Prepare {
        tool_call_id: String,
        command: Command,
    },
    Observe {
        run_id: String,
        index: u8,
        artifact_id: String,
    },
    Finish {
        run_id: String,
    },
}
impl Request {
    pub fn validate(&self) -> Result<(), &'static str> {
        let valid = match self {
            Self::Prepare {
                tool_call_id,
                command,
            } => {
                command.validate()?;
                valid_id(tool_call_id)
            }
            Self::Observe {
                run_id,
                index,
                artifact_id,
            } => valid_id(run_id) && *index < VERIFY_RUNS && valid_id(artifact_id),
            Self::Finish { run_id } => valid_id(run_id),
        };
        if !valid {
            return Err("invalid assigned verification request");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinishedRun {
    pub run_id: String,
    /// False when a concurrent finding edit prevented attachment of results.
    pub recorded: bool,
    pub verified: bool,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub identity: ExecutionIdentity,
    pub result: ResultKind,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResultKind {
    Prepared { run_id: String },
    Observed { run_id: String, index: u8 },
    Finished { run: FinishedRun },
    Denied {},
    Rejected { error: crate::tool_error::ToolError },
}
impl Reply {
    pub fn validate_for(
        &self,
        identity: &ExecutionIdentity,
        request: &Request,
    ) -> Result<(), &'static str> {
        self.identity.validate()?;
        request.validate()?;
        let valid = match (&self.result, request) {
            (ResultKind::Prepared { run_id }, Request::Prepare { .. }) => valid_id(run_id),
            (ResultKind::Denied {}, Request::Prepare { .. }) => true,
            (ResultKind::Rejected { error }, Request::Prepare { .. }) => error.validate().is_ok(),
            (
                ResultKind::Observed { run_id, index },
                Request::Observe {
                    run_id: expected,
                    index: ordinal,
                    ..
                },
            ) => run_id == expected && index == ordinal,
            (ResultKind::Finished { run }, Request::Finish { run_id }) => {
                run.run_id == *run_id
                    && (!run.verified || run.recorded)
                    && run.output.len() <= 64_000
            }
            _ => false,
        };
        if self.identity != *identity || !valid {
            return Err("invalid assigned verification reply");
        }
        Ok(())
    }
}
