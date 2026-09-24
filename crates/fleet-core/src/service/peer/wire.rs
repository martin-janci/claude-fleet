//! The `peer_exchange` wire. Field doc comments are one line each: they are
//! served in the tool's input schema and count against the tool budget.

use rmcp::schemars;
use serde::{Deserialize, Serialize};

pub const PROTO: u32 = 1;
pub const PEER_BODY_MAX: usize = 32 * 1024;
pub const PEER_BATCH_MAX: usize = 50;
pub const PEER_WAIT_MAX_MS: u64 = 25_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireRef {
    pub fleet: String,
    pub id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireMessage {
    /// The sending hub's message id.
    pub id: i64,
    pub from_addr: String,
    pub to_addr: String,
    pub body: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<WireRef>,
    pub sent_at: i64,
    #[serde(default)]
    pub wake: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireResult {
    pub id: i64,
    pub status: ResultStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExchangeRequest {
    pub proto: u32,
    /// The calling hub's fleet id.
    pub fleet_id: String,
    #[serde(default)]
    pub send: Vec<WireMessage>,
    /// Highest id of yours the caller has stored.
    #[serde(default)]
    pub after: i64,
    /// The caller's rejections of your earlier messages.
    #[serde(default)]
    pub results: Vec<WireResult>,
    /// Long-poll budget, ms; clamped to 25000.
    #[serde(default)]
    pub wait_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeResponse {
    pub proto: u32,
    pub fleet_id: String,
    pub results: Vec<WireResult>,
    pub messages: Vec<WireMessage>,
    pub more: bool,
}

impl WireResult {
    pub fn accepted(id: i64) -> Self {
        Self {
            id,
            status: ResultStatus::Accepted,
            code: None,
            message: None,
        }
    }
    pub fn rejected(id: i64, code: &str, message: impl Into<String>) -> Self {
        Self {
            id,
            status: ResultStatus::Rejected,
            code: Some(code.into()),
            message: Some(message.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_without_optional_fields_parses() {
        let r: ExchangeRequest =
            serde_json::from_str(r#"{"proto":1,"fleet_id":"fleet-a"}"#).unwrap();
        assert!(r.send.is_empty() && r.results.is_empty());
        assert_eq!((r.after, r.wait_ms), (0, 0));
    }
}
