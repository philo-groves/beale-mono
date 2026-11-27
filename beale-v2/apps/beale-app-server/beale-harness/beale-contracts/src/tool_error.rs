//! Known, non-executed tool rejections. Host/IO/provider errors must not use this type.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

pub const MAX_TOOL_ERROR_BYTES: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorCode {
    InvalidArguments,
    NotFound,
    Conflict,
    Rejected,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolError {
    pub code: ToolErrorCode,
    pub message: String,
}
impl ToolError {
    fn new(code: ToolErrorCode, message: impl Into<String>) -> Self {
        // Keep internal diagnostics intact until the host has redacted secrets.
        // Truncating first could split a secret and defeat exact-value redaction.
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::InvalidArguments, message)
    }
    pub fn missing(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::NotFound, message)
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::Conflict, message)
    }
    pub fn rejected(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::Rejected, message)
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.message.trim().is_empty() || self.message.len() > MAX_TOOL_ERROR_BYTES {
            Err("invalid tool error")
        } else {
            Ok(())
        }
    }
    pub fn feedback(&self) -> Value {
        serde_json::json!({"output":self.message,"is_error":true,"error":self})
    }
}
impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ToolError {}

/// Apply only to model-supplied values, never persisted records or provider results.
pub fn arguments<T: DeserializeOwned>(value: Value) -> Result<T, ToolError> {
    serde_json::from_value(value)
        .map_err(|error| ToolError::invalid(format!("Invalid tool arguments: {error}")))
}
