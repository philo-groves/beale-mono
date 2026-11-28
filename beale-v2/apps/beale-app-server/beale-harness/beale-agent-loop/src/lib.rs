//! The agent round loop shared by in-process sessions and assigned workers.
//!
//! This crate holds no host state: no database, providers, credentials, or
//! tools. Hosts implement [`AgentPorts`] for provider access, tool execution,
//! persistence, and stop/yield signals; the loop owns call bounds, replay,
//! result formatting, retries, the activity safeguard, and how a turn ends.
pub mod answers;
mod machine;
mod policy;

pub use machine::{
    AgentPorts, Checkpoint, Ending, Finished, InferOutcome, LoopConfig, LoopError, LoopState,
    ModelStep, PendingCall, Phase, SWARM_TOOL_ACTIVITY_LIMIT, ToolOutcome, run, step,
};
pub use policy::{
    CompletedCall, MAX_CALL_ARGUMENT_BYTES, MAX_CALL_ID_BYTES, MAX_CALL_NAME_BYTES, Replay,
    RetryPolicy, append_tool_result, call_within_bounds, canonical_arguments, flush_tool_results,
    replay,
};
