//! `/hook` endpoint — receives Claude Code hook events and forwards them to
//! the service layer.
//!
//! The route sits behind the same Origin/Host + bearer middleware as `/mcp`
//! (see `mcp::start`); the middleware puts the authenticated [`Caller`] into
//! the request extensions and this handler reads it from there. Hooks are
//! installed as Claude Code `type: "http"` hooks carrying
//! `Authorization: Bearer <per-host token>` (see
//! `commands::mcp::merge_hook_into_settings_json`), so the token never
//! appears in a process argv. The legacy `?token=` query form written by
//! older installs is still accepted by the middleware until every host is
//! re-provisioned.

use axum::{extract::State, http::StatusCode, Extension, Json};
use serde::Deserialize;
use std::sync::{Arc, Mutex};

use super::auth::Caller;
use crate::ssh::SshClient;
use crate::store::Store;

/// Axum router state for the `/hook` endpoint.
#[derive(Clone)]
pub struct HookState {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
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
    /// Absolute path of the session's JSONL transcript. Claude Code sends it
    /// in every hook body; fleet stores it (after validation) and prefers it
    /// over any path derived from the cwd.
    pub transcript_path: Option<String>,
}

/// Axum handler for `POST /hook`. Auth has already happened in the
/// middleware; the [`Caller`] extension says which host's token signed the
/// request (or the master token).
pub async fn handle_hook(
    State(state): State<HookState>,
    Extension(caller): Extension<Caller>,
    Json(payload): Json<HookPayload>,
) -> StatusCode {
    eprintln!(
        "[hook] caller={} event={:?} session={:?} tool={:?}",
        caller.label(),
        payload.hook_event_name,
        payload.session_id,
        payload.tool_name
    );
    match crate::service::hooks::apply_hook(&state.store, &state.ssh, &payload, &caller) {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(e) if e.code == "E_VALIDATE" || e.code == "E_INVALID" => {
            eprintln!("[hook] rejected payload: {} {}", e.code, e.message);
            StatusCode::BAD_REQUEST
        }
        Err(e) if e.code == "E_FORBIDDEN" => {
            eprintln!("[hook] refused: {} {}", e.code, e.message);
            StatusCode::FORBIDDEN
        }
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
    fn transcript_path_and_enter_worktree_response_deserialize() {
        let json = r#"{
            "session_id":"s1",
            "hook_event_name":"PostToolUse",
            "tool_name":"EnterWorktree",
            "tool_input":{"name":"feat"},
            "tool_response":{"worktreePath":"/home/u/proj/.claude/worktrees/feat","branch":"feat"},
            "transcript_path":"/home/u/.claude/projects/-home-u-proj/s1.jsonl"
        }"#;
        let p: HookPayload = serde_json::from_str(json).unwrap();
        assert_eq!(p.tool_name.as_deref(), Some("EnterWorktree"));
        assert_eq!(
            p.transcript_path.as_deref(),
            Some("/home/u/.claude/projects/-home-u-proj/s1.jsonl")
        );
        assert_eq!(
            p.tool_response.unwrap()["worktreePath"],
            "/home/u/proj/.claude/worktrees/feat"
        );
    }

    #[test]
    fn extra_fields_are_ignored() {
        let json = r#"{"unknown_future_field":"x","session_id":"s1"}"#;
        let p: HookPayload = serde_json::from_str(json).unwrap();
        assert_eq!(p.session_id.as_deref(), Some("s1"));
    }
}
