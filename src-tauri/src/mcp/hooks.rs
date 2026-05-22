//! `/hook` endpoint — receives Claude Code hook events and forwards them to
//! the service layer.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::store::Store;

/// Axum router state for the `/hook` endpoint.
#[derive(Clone)]
pub struct HookState {
    pub store: Arc<Mutex<Store>>,
    pub token: Arc<String>,
}

/// Body deserialized from a POST to `/hook`.
///
/// All fields are optional: Claude Code sends different subsets depending on
/// the hook event type. `deny_unknown_fields` is intentionally absent — future
/// Claude Code versions may add fields and we should ignore them gracefully.
// Some fields are deserialized from the hook payload but not yet read.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct HookPayload {
    pub session_id: Option<String>,
    pub hook_event_name: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    pub tool_response: Option<serde_json::Value>,
    pub cwd: Option<String>,
}

/// Constant-time comparison of two token strings.
///
/// Returns `false` immediately if either string is empty, preventing
/// acceptance of blank tokens regardless of configuration.
pub fn check_query_token(expected: &str, provided: &str) -> bool {
    if provided.is_empty() || expected.is_empty() {
        return false;
    }
    provided.len() == expected.len()
        && provided
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// Axum handler for `POST /hook?token=<token>`.
///
/// Validates the query-param token, then delegates to
/// [`crate::service::hooks::apply_hook`].
pub async fn handle_hook(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<HookState>,
    Json(payload): Json<HookPayload>,
) -> StatusCode {
    let provided = params.get("token").map(String::as_str).unwrap_or("");
    if !check_query_token(&state.token, provided) {
        eprintln!("[hook] rejected: bad token");
        return StatusCode::UNAUTHORIZED;
    }
    eprintln!(
        "[hook] event={:?} session={:?} tool={:?}",
        payload.hook_event_name, payload.session_id, payload.tool_name
    );
    match crate::service::hooks::apply_hook(&state.store, &payload) {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(e) => {
            eprintln!("[hook] apply_hook error: {} {}", e.code, e.message);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_hook_deserializes() {
        let json = r#"{"session_id":"abc123","hook_event_name":"Stop"}"#;
        let p: HookPayload = serde_json::from_str(json).unwrap();
        assert_eq!(p.session_id.as_deref(), Some("abc123"));
        assert_eq!(p.hook_event_name.as_deref(), Some("Stop"));
        assert!(p.tool_name.is_none());
    }

    #[test]
    fn worktree_hook_deserializes() {
        let json = r#"{
            "session_id":"abc123",
            "hook_event_name":"PostToolUse",
            "tool_name":"WorktreeCreate",
            "tool_input":{"worktree_path":"/home/user/proj/.worktrees/feat","branch":"feat"}
        }"#;
        let p: HookPayload = serde_json::from_str(json).unwrap();
        assert_eq!(p.hook_event_name.as_deref(), Some("PostToolUse"));
        assert_eq!(p.tool_name.as_deref(), Some("WorktreeCreate"));
        let inp = p.tool_input.unwrap();
        assert_eq!(
            inp.get("worktree_path").and_then(|v| v.as_str()),
            Some("/home/user/proj/.worktrees/feat")
        );
    }

    #[test]
    fn extra_fields_are_ignored() {
        let json = r#"{"unknown_future_field":"x","session_id":"s1"}"#;
        let p: HookPayload = serde_json::from_str(json).unwrap();
        assert_eq!(p.session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn check_query_token_correct() {
        assert!(check_query_token("mysecret", "mysecret"));
    }

    #[test]
    fn check_query_token_wrong() {
        assert!(!check_query_token("mysecret", "wrong"));
    }

    #[test]
    fn check_query_token_empty_provided_rejected() {
        assert!(!check_query_token("mysecret", ""));
    }
}
