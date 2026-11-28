//! Saved answers for turns that end without a model answer. Scheduling and
//! history reconstruction recognize these exact texts, so every loop uses
//! these constants rather than its own copies.

/// Prefix of an answer saved when the provider could not finish after tools ran.
pub const INTERRUPTED_ANSWER: &str =
    "The provider could not complete this turn. Tool activity was saved.";
/// Saved answer for a turn stopped by the operator.
pub const STOPPED_ANSWER: &str = "Stopped by the user. Completed tool activity was saved.";
/// Saved answer for a turn that ended because Auto-Review was unavailable.
pub const REVIEW_UNAVAILABLE_ANSWER: &str = "Auto-Review was unavailable, so the requested tool was not run. Send a follow-up after the review service is available to resume from the saved activity.";
/// Saved answer for a turn paused by the per-turn tool activity safeguard.
pub const SAFEGUARD_ANSWER: &str =
    "Execution paused after the per-turn tool activity safeguard; saved work can be resumed.";
/// Saved answer for a deputy that yielded its execution slot to a child.
pub const DELEGATION_ANSWER: &str =
    "Delegation accepted. Waiting for child execution after a safe checkpoint.";
/// Detail appended to [`INTERRUPTED_ANSWER`] for a malformed tool call.
pub const INVALID_CALL_DETAIL: &str = "The provider returned an invalid tool call.";
/// Detail appended to [`INTERRUPTED_ANSWER`] for a reused call identity.
pub const REUSED_CALL_DETAIL: &str = "The provider reused a tool identity with different content.";

/// Provider failure text for a step with neither text nor tool calls before
/// any tool ran.
pub const EMPTY_STEP_MESSAGE: &str =
    "The provider completed without a message or tool call. Try again.";
/// Detail appended to [`INTERRUPTED_ANSWER`] for an empty final answer after
/// tools ran.
pub const EMPTY_ANSWER_DETAIL: &str = "The provider completed without a message.";

/// An interrupted answer with its reason.
pub fn interrupted(detail: &str) -> String {
    format!("{INTERRUPTED_ANSWER} {detail}")
}
