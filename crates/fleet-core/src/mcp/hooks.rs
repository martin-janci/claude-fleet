//! `/hook` endpoint — receives Claude Code hook events and forwards them to
//! the service layer.
//!
//! The route sits behind the same Origin/Host + bearer middleware as `/mcp`
//! (see `mcp::start`); the middleware puts the authenticated [`Caller`] into
//! the request extensions and this handler reads it from there. Hooks are
//! installed as Claude Code `type: "http"` hooks carrying
//! `Authorization: Bearer <per-host token>` (see
//! `service::hooks_install::merge_hook_into_settings_json`), so the token never
//! appears in a process argv. The legacy `?token=` query form written by
//! older installs is still accepted by the middleware until every host is
//! re-provisioned.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
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
#[derive(Debug, Default, Deserialize)]
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
    /// `SessionEnd`: why the session ended (`logout`, `prompt_input_exit`,
    /// `other`, `clear`, `resume`).
    pub reason: Option<String>,
    /// `Notification`: which type fired (see `NOTIFICATION_MATCHER`).
    pub notification_type: Option<String>,
    /// `Notification`: human text. Read for nothing; never stored.
    pub message: Option<String>,
    /// `Notification`: optional title. Never stored.
    pub title: Option<String>,
    /// `StopFailure`: the API error type (`rate_limit`, `overloaded`, …).
    pub error: Option<String>,
    /// `StopFailure`: free-text detail, when Claude Code has one.
    pub error_details: Option<String>,
    /// `SessionStart`: startup | resume | clear | compact | fork.
    pub source: Option<String>,
    /// `SessionStart`: the model id.
    pub model: Option<String>,
    /// `PreCompact` / `PostCompact`: manual | auto.
    pub trigger: Option<String>,
    /// `UserPromptSubmit`: the prompt text (first 200 chars stored as the
    /// conversation's `first_prompt`; never logged).
    pub prompt: Option<String>,
    /// `Stop`: the final assistant message (first 200 chars go to the
    /// `turn_done` timeline event; never logged).
    pub last_assistant_message: Option<String>,
}

/// `X-Fleet-Pane`: `$TMUX_PANE` of the hook's process (`%` + digits). Empty
/// (outside tmux, or a CLI that does not expand header env vars — it then
/// sends the literal `$TMUX_PANE`) or malformed → `None`.
pub fn pane_header(headers: &axum::http::HeaderMap) -> Option<String> {
    let v = headers.get("x-fleet-pane")?.to_str().ok()?.trim();
    let digits = v.strip_prefix('%')?;
    (!digits.is_empty() && digits.len() <= 10 && digits.chars().all(|c| c.is_ascii_digit()))
        .then(|| v.to_string())
}

/// Axum handler for `POST /hook`. Auth has already happened in the
/// middleware; the [`Caller`] extension says which host's token signed the
/// request (or the master token).
pub async fn handle_hook(
    State(state): State<HookState>,
    Extension(caller): Extension<Caller>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<HookPayload>,
) -> Response {
    use crate::ipc_error::codes;
    // A paired client (a phone) has no business reporting hook events: hooks
    // are Claude Code's own callbacks, and `service::hooks::caller_host` maps
    // a caller with no host binding to `local`, which would file a phone's
    // events against the local host's sessions. The `Json` extractor above
    // already deserialized the payload — axum runs it before this handler
    // body — but it is never used: a misconfigured client polling this
    // endpoint would otherwise fill the log at `warn`, so this logs at
    // `debug`, matching the accepted path two lines below.
    if caller.is_client() {
        tracing::debug!(caller = %caller.label(), "[hook] refused: clients do not report hooks");
        return StatusCode::FORBIDDEN.into_response();
    }
    let pane_id = pane_header(&headers);
    // Every hook event lands here (several per turn): debug, not info. Only
    // identifiers are logged, never the payload body (`prompt`,
    // `last_assistant_message`, `message` stay out of the log).
    tracing::debug!(
        caller = %caller.label(),
        event = ?payload.hook_event_name,
        session = ?payload.session_id,
        pane = ?pane_id,
        tool = ?payload.tool_name,
        "[hook] received"
    );
    let ctx = crate::service::hooks::HookContext {
        caller: &caller,
        pane_id,
    };
    match crate::service::hooks::apply_hook(&state.store, &state.ssh, &payload, &ctx) {
        Ok(()) => {
            // Delivery rides the response body of exactly these two events:
            // they are the only hooks Claude Code reads `additionalContext`
            // from, and both must stay SYNCHRONOUS (never `async: true`) or
            // the body is discarded. See the phase-2b exception in
            // docs/superpowers/specs/2026-09-22-fleet-mesh-addressing-and-delivery-design.md
            let event = payload.hook_event_name.as_deref().unwrap_or("");
            if event == "Stop" {
                return match crate::service::hooks::take_pending_stop_delivery(
                    &state.store,
                    &payload,
                    &ctx,
                ) {
                    Some((packed, crate::service::delivery::StopAction::Block))
                        if !packed.text.is_empty() =>
                    {
                        tracing::debug!(
                            event,
                            delivered = packed.included.len(),
                            remaining = packed.remaining,
                            "[hook] blocking Stop for a pending question"
                        );
                        // No truncation here, by design: the service layer
                        // already packed this batch against `reason`'s own
                        // 2000-char / 20-line budget (see
                        // `delivery::pack_within`), so every body in it is
                        // whole and the "(N more…)" tail is present and
                        // accurate. Cutting the text at 2000 chars — what
                        // this did before — split a body mid-sentence and
                        // removed that tail first, since it sits last.
                        axum::Json(serde_json::json!({
                            "decision": "block",
                            "reason": packed.text,
                        }))
                        .into_response()
                    }
                    Some((packed, _)) if !packed.text.is_empty() => {
                        tracing::debug!(
                            event,
                            delivered = packed.included.len(),
                            remaining = packed.remaining,
                            "[hook] carrying a delivery"
                        );
                        axum::Json(serde_json::json!({
                            "hookSpecificOutput": {
                                "hookEventName": event,
                                "additionalContext": packed.text,
                            }
                        }))
                        .into_response()
                    }
                    _ => StatusCode::NO_CONTENT.into_response(),
                };
            }
            // SessionStart answers only when installed synchronously (work
            // graph M4.5, `work.session_start_context`): an async command
            // hook discards the body anyway.
            if event == "SessionStart" {
                return match crate::service::hooks::session_start_context(
                    &state.store,
                    &payload,
                    &ctx,
                ) {
                    Some(text) if !text.is_empty() => {
                        tracing::debug!(event, "[hook] carrying work context");
                        axum::Json(serde_json::json!({
                            "hookSpecificOutput": {
                                "hookEventName": event,
                                "additionalContext": text,
                            }
                        }))
                        .into_response()
                    }
                    _ => StatusCode::NO_CONTENT.into_response(),
                };
            }
            if event != "UserPromptSubmit" {
                return StatusCode::NO_CONTENT.into_response();
            }
            match crate::service::hooks::take_pending_delivery(&state.store, &payload, &ctx) {
                Some(packed) if !packed.text.is_empty() => {
                    tracing::debug!(
                        event,
                        delivered = packed.included.len(),
                        remaining = packed.remaining,
                        "[hook] carrying a delivery"
                    );
                    axum::Json(serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": event,
                            "additionalContext": packed.text,
                        }
                    }))
                    .into_response()
                }
                _ => StatusCode::NO_CONTENT.into_response(),
            }
        }
        Err(e) if e.code == codes::E_VALIDATE || e.code == codes::E_INVALID => {
            tracing::warn!(code = %e.code, error = %e.message, "[hook] rejected payload");
            StatusCode::BAD_REQUEST.into_response()
        }
        Err(e) if e.code == codes::E_FORBIDDEN => {
            tracing::warn!(
                caller = %caller.label(),
                code = %e.code,
                error = %e.message,
                "[hook] refused"
            );
            StatusCode::FORBIDDEN.into_response()
        }
        Err(e) => {
            tracing::error!(code = %e.code, error = %e.message, "[hook] apply_hook failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_client_caller_is_refused_with_403() {
        use crate::mcp::auth::{ClientRef, TokenMode};
        let state = HookState {
            store: Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap())),
            ssh: Arc::new(SshClient::new()),
        };
        let client = Caller {
            host_alias: None,
            client: Some(ClientRef {
                id: 1,
                name: "phone".into(),
                trusted: false,
            }),
            mode: TokenMode::Full,
        };
        let payload = HookPayload {
            session_id: Some("s1".into()),
            hook_event_name: Some("Stop".into()),
            ..Default::default()
        };
        assert_eq!(
            handle_hook(
                State(state),
                Extension(client),
                axum::http::HeaderMap::new(),
                Json(payload)
            )
            .await
            .into_response()
            .status(),
            StatusCode::FORBIDDEN,
            "a paired client must never report hook events"
        );
    }

    /// Work graph M4.5: a SessionStart answers `hookSpecificOutput` with the
    /// work context when the setting is on, and 204 otherwise.
    #[tokio::test]
    async fn session_start_answers_additional_context_only_when_enabled() {
        let store = Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let id = s
                .upsert_session("dev", "local", None, None, 0, 0, "running", None)
                .unwrap();
            s.set_claude_session_id(id, "c1").unwrap();
            s.link_session_work(id, crate::store::WorkTarget::Key("ABC-1"), "manual")
                .unwrap();
        }
        let call = |store: Arc<Mutex<crate::store::Store>>| async move {
            let payload = HookPayload {
                session_id: Some("c1".into()),
                hook_event_name: Some("SessionStart".into()),
                source: Some("startup".into()),
                ..Default::default()
            };
            handle_hook(
                State(HookState {
                    store,
                    ssh: Arc::new(SshClient::new()),
                }),
                Extension(Caller::master()),
                axum::http::HeaderMap::new(),
                Json(payload),
            )
            .await
            .into_response()
        };
        assert_eq!(call(store.clone()).await.status(), StatusCode::NO_CONTENT);
        crate::service::settings::set(
            &store.lock().unwrap(),
            crate::service::settings::WORK_SESSION_START_CONTEXT,
            "true",
        )
        .unwrap();
        let resp = call(store.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert!(v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("ABC-1"));
    }

    #[test]
    fn session_end_stop_failure_and_notification_fields_deserialize() {
        let p: HookPayload = serde_json::from_str(
            r#"{"session_id":"s","hook_event_name":"SessionEnd","reason":"logout","unknown":1}"#,
        )
        .unwrap();
        assert_eq!(p.reason.as_deref(), Some("logout"));
        let p: HookPayload = serde_json::from_str(
            r#"{"session_id":"s","hook_event_name":"StopFailure","error":"rate_limit","error_details":"429","last_assistant_message":"API Error"}"#,
        )
        .unwrap();
        assert_eq!(p.error.as_deref(), Some("rate_limit"));
        assert_eq!(p.error_details.as_deref(), Some("429"));
        let p: HookPayload = serde_json::from_str(
            r#"{"session_id":"s","hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission","title":"Permission needed"}"#,
        )
        .unwrap();
        assert_eq!(p.notification_type.as_deref(), Some("permission_prompt"));
        assert_eq!(p.message.as_deref(), Some("Claude needs your permission"));
        assert_eq!(p.title.as_deref(), Some("Permission needed"));
        let d = HookPayload::default();
        assert!(d.session_id.is_none() && d.reason.is_none());
    }

    #[test]
    fn pane_header_accepts_only_tmux_pane_ids() {
        let h = |v: &str| {
            let mut m = axum::http::HeaderMap::new();
            m.insert("x-fleet-pane", v.parse().unwrap());
            pane_header(&m)
        };
        assert_eq!(h("%17"), Some("%17".into()));
        assert_eq!(h(""), None);
        assert_eq!(h("$TMUX_PANE"), None);
        assert_eq!(h("%1;rm"), None);
        assert_eq!(h("%"), None);
        assert_eq!(h("%12345678901"), None);
        assert_eq!(pane_header(&axum::http::HeaderMap::new()), None);
    }

    #[test]
    fn conversation_fields_deserialize() {
        let p: HookPayload = serde_json::from_str(
            r#"{"session_id":"s","hook_event_name":"SessionStart","source":"clear","model":"claude-opus-5"}"#,
        )
        .unwrap();
        assert_eq!(p.source.as_deref(), Some("clear"));
        assert_eq!(p.model.as_deref(), Some("claude-opus-5"));
        let p: HookPayload = serde_json::from_str(
            r#"{"hook_event_name":"PreCompact","trigger":"auto","prompt":"hi","last_assistant_message":"done"}"#,
        )
        .unwrap();
        assert_eq!(p.trigger.as_deref(), Some("auto"));
        assert_eq!(p.prompt.as_deref(), Some("hi"));
        assert_eq!(p.last_assistant_message.as_deref(), Some("done"));
    }

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

    #[tokio::test]
    async fn a_hook_with_nothing_pending_still_answers_204() {
        let state = HookState {
            store: Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap())),
            ssh: Arc::new(SshClient::new()),
        };
        let payload = HookPayload {
            session_id: Some("conv-x".into()),
            hook_event_name: Some("UserPromptSubmit".into()),
            ..Default::default()
        };
        let res = handle_hook(
            State(state),
            Extension(Caller::master()),
            axum::http::HeaderMap::new(),
            Json(payload),
        )
        .await
        .into_response();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn a_pending_message_comes_back_as_hook_specific_output() {
        let store = Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
        let (a, b) = {
            let s = store.lock().unwrap();
            let a = seed(&s, "alpha");
            let b = seed(&s, "beta");
            s.insert_message(a, b, "ping", "message", None).unwrap();
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET claude_session_id='conv-b' WHERE id=?1",
                    rusqlite::params![b],
                )
                .unwrap();
            (a, b)
        };
        let _ = (a, b);
        let state = HookState {
            store: store.clone(),
            ssh: Arc::new(SshClient::new()),
        };
        let payload = HookPayload {
            session_id: Some("conv-b".into()),
            hook_event_name: Some("UserPromptSubmit".into()),
            ..Default::default()
        };
        let res = handle_hook(
            State(state),
            Extension(Caller::master()),
            axum::http::HeaderMap::new(),
            Json(payload),
        )
        .await
        .into_response();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(ctx.contains("ping"), "{ctx}");
    }

    #[tokio::test]
    async fn a_client_caller_is_still_refused_and_gets_no_delivery() {
        // Regression guard: the 403 path must not become a delivery channel.
        use crate::mcp::auth::{ClientRef, TokenMode};
        let state = HookState {
            store: Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap())),
            ssh: Arc::new(SshClient::new()),
        };
        let client = Caller {
            host_alias: None,
            client: Some(ClientRef {
                id: 1,
                name: "phone".into(),
                trusted: false,
            }),
            mode: TokenMode::Full,
        };
        let res = handle_hook(
            State(state),
            Extension(client),
            axum::http::HeaderMap::new(),
            Json(HookPayload::default()),
        )
        .await
        .into_response();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    fn seed(s: &crate::store::Store, name: &str) -> i64 {
        s.upsert_host("local").unwrap();
        s.upsert_session(name, "local", None, None, 0, 0, "running", None)
            .unwrap()
    }

    /// A pending message must never leak through, or be stamped, on any hook
    /// event outside `UserPromptSubmit`/`Stop` — those are the only two
    /// Claude Code actually reads `additionalContext` from (see the comment
    /// on the `matches!` gate in `handle_hook`). Without this test the gate
    /// could be deleted and nothing here would notice.
    #[tokio::test]
    async fn a_non_delivery_event_with_a_message_pending_still_answers_204_and_stamps_nothing() {
        let store = Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
        let b = {
            let s = store.lock().unwrap();
            let a = seed(&s, "alpha");
            let b = seed(&s, "beta");
            s.insert_message(a, b, "ping", "message", None).unwrap();
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET claude_session_id='conv-b' WHERE id=?1",
                    rusqlite::params![b],
                )
                .unwrap();
            b
        };
        let state = HookState {
            store: store.clone(),
            ssh: Arc::new(SshClient::new()),
        };
        let payload = HookPayload {
            session_id: Some("conv-b".into()),
            hook_event_name: Some("PreCompact".into()),
            ..Default::default()
        };
        let res = handle_hook(
            State(state),
            Extension(Caller::master()),
            axum::http::HeaderMap::new(),
            Json(payload),
        )
        .await
        .into_response();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        let s = store.lock().unwrap();
        assert_eq!(
            s.list_undelivered_for_session(b, 10).unwrap().len(),
            1,
            "a non-delivery event must never stamp the pending message"
        );
    }

    /// When a pending message is individually over `pack`'s budget, it gets
    /// a stub (naming its id, pointing at `inbox`) instead of its body, and
    /// IS stamped delivered — the old contract here (`included` empty,
    /// nothing stamped, message stuck reachable only via `inbox`) was the
    /// permanent-stall bug: an oldest-first, undelivered-only queue with an
    /// oversized message at the head meant `pack` broke on it every single
    /// hook, forever, and nothing behind it was ever delivered. See
    /// `service::delivery::pack`.
    #[tokio::test]
    async fn an_over_budget_message_gets_a_stub_and_is_stamped_delivered() {
        let store = Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
        let (b, mid) = {
            let s = store.lock().unwrap();
            let a = seed(&s, "alpha");
            let b = seed(&s, "beta");
            let huge = "x".repeat(crate::service::delivery::CTX_MAX_CHARS + 1);
            let mid = s.insert_message(a, b, &huge, "message", None).unwrap();
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET claude_session_id='conv-b' WHERE id=?1",
                    rusqlite::params![b],
                )
                .unwrap();
            (b, mid)
        };
        let state = HookState {
            store: store.clone(),
            ssh: Arc::new(SshClient::new()),
        };
        let payload = HookPayload {
            session_id: Some("conv-b".into()),
            hook_event_name: Some("UserPromptSubmit".into()),
            ..Default::default()
        };
        let res = handle_hook(
            State(state),
            Extension(Caller::master()),
            axum::http::HeaderMap::new(),
            Json(payload),
        )
        .await
        .into_response();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(
            ctx.contains("inbox"),
            "the stub must point at the inbox tool: {ctx}"
        );
        // The `#<id>` marker, not a bare digit: the id is how the agent
        // names the message to `inbox`, and a stub without it is useless.
        assert!(
            ctx.contains(&format!("#{mid}")),
            "the stub must name the message id as #{mid}: {ctx}"
        );
        assert!(
            !ctx.contains("1 more"),
            "the stub message itself was delivered, so it is not left over: {ctx}"
        );
        let s = store.lock().unwrap();
        assert_eq!(
            s.list_undelivered_for_session(b, 10).unwrap().len(),
            0,
            "the stub gets the message stamped delivered — it must not stay stuck at the head"
        );
    }

    #[tokio::test]
    async fn a_stop_event_delivers_the_pending_message() {
        let store = Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
        {
            let s = store.lock().unwrap();
            let a = seed(&s, "alpha");
            let b = seed(&s, "beta");
            s.insert_message(a, b, "stop-time ping", "message", None)
                .unwrap();
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET claude_session_id='conv-b' WHERE id=?1",
                    rusqlite::params![b],
                )
                .unwrap();
        }
        let state = HookState {
            store: store.clone(),
            ssh: Arc::new(SshClient::new()),
        };
        let payload = HookPayload {
            session_id: Some("conv-b".into()),
            hook_event_name: Some("Stop".into()),
            ..Default::default()
        };
        let res = handle_hook(
            State(state),
            Extension(Caller::master()),
            axum::http::HeaderMap::new(),
            Json(payload),
        )
        .await
        .into_response();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "Stop");
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(ctx.contains("stop-time ping"), "{ctx}");
    }

    #[tokio::test]
    async fn a_stop_event_with_a_question_blocks_the_turn() {
        let store = Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
        {
            let s = store.lock().unwrap();
            let a = seed(&s, "alpha");
            let b = seed(&s, "beta");
            s.insert_message(a, b, "need an answer", "question", None)
                .unwrap();
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET claude_session_id='conv-b' WHERE id=?1",
                    rusqlite::params![b],
                )
                .unwrap();
        }
        let state = HookState {
            store: store.clone(),
            ssh: Arc::new(SshClient::new()),
        };
        let payload = HookPayload {
            session_id: Some("conv-b".into()),
            hook_event_name: Some("Stop".into()),
            ..Default::default()
        };
        let res = handle_hook(
            State(state),
            Extension(Caller::master()),
            axum::http::HeaderMap::new(),
            Json(payload),
        )
        .await
        .into_response();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["decision"], "block");
        assert!(v["reason"].as_str().unwrap().contains("need an answer"));
    }

    /// `STOP_BLOCK_STREAK_MAX` (3): a remote sender must never be able to
    /// trap a session in a never-ending turn by always sending a fresh
    /// question. The fourth consecutive question in a row must fall back to
    /// `additionalContext`, and the trip must reset the streak.
    #[tokio::test]
    async fn the_stop_block_streak_caps_at_three_in_a_row() {
        let store = Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
        let (a, b) = {
            let s = store.lock().unwrap();
            let a = seed(&s, "alpha");
            let b = seed(&s, "beta");
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET claude_session_id='conv-b' WHERE id=?1",
                    rusqlite::params![b],
                )
                .unwrap();
            (a, b)
        };
        let state = HookState {
            store: store.clone(),
            ssh: Arc::new(SshClient::new()),
        };
        let payload = || HookPayload {
            session_id: Some("conv-b".into()),
            hook_event_name: Some("Stop".into()),
            ..Default::default()
        };
        for i in 0..3 {
            {
                let s = store.lock().unwrap();
                s.insert_message(a, b, &format!("question {i}"), "question", None)
                    .unwrap();
            }
            let res = handle_hook(
                State(state.clone()),
                Extension(Caller::master()),
                axum::http::HeaderMap::new(),
                Json(payload()),
            )
            .await
            .into_response();
            assert_eq!(res.status(), StatusCode::OK);
            let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
                .await
                .unwrap();
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(v["decision"], "block", "block #{i}");
        }
        {
            let s = store.lock().unwrap();
            assert_eq!(s.stop_block_streak(b).unwrap(), 3);
        }
        // The fourth question arrives while the streak sits at the cap: it
        // must not block a fourth time.
        {
            let s = store.lock().unwrap();
            s.insert_message(a, b, "question 4", "question", None)
                .unwrap();
        }
        let res = handle_hook(
            State(state.clone()),
            Extension(Caller::master()),
            axum::http::HeaderMap::new(),
            Json(payload()),
        )
        .await
        .into_response();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            v.get("decision").is_none(),
            "cap reached: must not block again: {v}"
        );
        assert!(v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("question 4"));
        let s = store.lock().unwrap();
        assert_eq!(
            s.stop_block_streak(b).unwrap(),
            0,
            "hitting the cap must reset the streak"
        );
    }

    // ---- the block `reason` budget (final review, Critical 1) ----
    //
    // Claude Code caps a `Stop` block's `reason` at 2000 characters AND 20
    // lines — both far under the packer's 8000/200 `additionalContext`
    // budget. These are written as literals here on purpose: they are the
    // CLI's caps, and a test that read them from the same constant the
    // implementation packs against could not tell a wrong constant from a
    // right one.
    const REASON_CHARS: usize = 2000;
    const REASON_LINES: usize = 20;

    /// Drive one `Stop` hook for `conv-b` and return the decoded body.
    async fn stop_hook_json(store: &Arc<Mutex<crate::store::Store>>) -> serde_json::Value {
        let state = HookState {
            store: store.clone(),
            ssh: Arc::new(SshClient::new()),
        };
        let payload = HookPayload {
            session_id: Some("conv-b".into()),
            hook_event_name: Some("Stop".into()),
            ..Default::default()
        };
        let res = handle_hook(
            State(state),
            Extension(Caller::master()),
            axum::http::HeaderMap::new(),
            Json(payload),
        )
        .await
        .into_response();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 256 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// Two sessions with `beta` bound to `conv-b`, so a `Stop` payload
    /// naming `conv-b` passes the conversation guard.
    fn two_sessions(store: &Arc<Mutex<crate::store::Store>>) -> (i64, i64) {
        let s = store.lock().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        s.conn_ref()
            .execute(
                "UPDATE sessions SET claude_session_id='conv-b' WHERE id=?1",
                rusqlite::params![b],
            )
            .unwrap();
        (a, b)
    }

    /// Ids whose `delivered_at` the hook just stamped: everything that was
    /// inserted, minus what is still undelivered.
    fn stamped_ids(store: &Arc<Mutex<crate::store::Store>>, to: i64, all: &[i64]) -> Vec<i64> {
        let s = store.lock().unwrap();
        let left: Vec<i64> = s
            .list_undelivered_for_session(to, 1000)
            .unwrap()
            .iter()
            .map(|m| m.id)
            .collect();
        all.iter()
            .copied()
            .filter(|id| !left.contains(id))
            .collect()
    }

    /// The `#<id>` markers a packed text carries, in order.
    fn carried_ids(text: &str) -> Vec<i64> {
        text.split('#')
            .skip(1)
            .filter_map(|rest| {
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse().ok()
            })
            .collect()
    }

    /// A body that cannot fit the block budget must not be CUT to fit it:
    /// it rides whole or it does not ride at all. Packing to the smaller
    /// budget turns it into the same stub the `additionalContext` path uses
    /// for an oversized message — which names the id and points at `inbox`,
    /// so the agent can still answer. Truncating `packed.text` instead left
    /// a half sentence and no pointer at all.
    #[tokio::test]
    async fn a_body_over_the_block_budget_is_never_cut_mid_body() {
        let store = Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
        let (a, b) = two_sessions(&store);
        let body = "the middle of this sentence must never be where it ends. ".repeat(50);
        assert!(body.chars().count() > REASON_CHARS, "the premise");
        let id = {
            let s = store.lock().unwrap();
            s.insert_message(a, b, &body, "question", None).unwrap()
        };
        let v = stop_hook_json(&store).await;
        assert_eq!(v["decision"], "block", "a question still blocks: {v}");
        let reason = v["reason"].as_str().unwrap();
        assert!(
            reason.chars().count() <= REASON_CHARS,
            "reason chars = {}",
            reason.chars().count()
        );
        assert!(
            reason.contains(&body) || !reason.contains(&body[..200]),
            "the body must ride whole or not at all, never cut: {reason}"
        );
        assert!(
            reason.contains(&format!("#{id}")),
            "the reason must name the message id: {reason}"
        );
        assert!(
            reason.contains("inbox"),
            "a body that could not ride must point at the inbox tool: {reason}"
        );
        assert_eq!(
            stamped_ids(&store, b, &[id]),
            vec![id],
            "the stub carries the id, so it is stamped"
        );
    }

    /// The `"(N more message(s) waiting…)"` tail is the only pointer at
    /// `inbox` a block carries, and `pack` puts it LAST — so truncating the
    /// text cut it first, and its count was the full-budget count anyway.
    /// Packing to the block budget keeps it, and makes it true.
    #[tokio::test]
    async fn the_block_reason_keeps_the_tail_and_names_the_right_remaining_count() {
        let store = Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
        let (a, b) = two_sessions(&store);
        let mut ids = Vec::new();
        {
            let s = store.lock().unwrap();
            ids.push(
                s.insert_message(a, b, "answer me", "question", None)
                    .unwrap(),
            );
            // Enough plain mail that even the 8000-char budget overflows,
            // so the pre-fix text had a tail — at its very end, where the
            // 2000-char cut removed it.
            for i in 0..60 {
                let body = format!("{i:03}{}", "m".repeat(200));
                ids.push(s.insert_message(a, b, &body, "message", None).unwrap());
            }
        }
        let v = stop_hook_json(&store).await;
        assert_eq!(v["decision"], "block", "{v}");
        let reason = v["reason"].as_str().unwrap();
        assert!(
            reason.chars().count() <= REASON_CHARS,
            "reason chars = {}",
            reason.chars().count()
        );
        let stamped = stamped_ids(&store, b, &ids);
        let remaining = ids.len() - stamped.len();
        assert!(remaining > 0, "the premise: not everything can ride");
        assert!(
            reason.contains(&format!("({remaining} more message(s) waiting")),
            "the tail must survive and name the real remaining count ({remaining}): {reason}"
        );
    }

    /// Only what the block actually carried may be stamped `delivered_at`.
    /// Stamping the full-budget pack and then cutting the text to 2000 chars
    /// marked messages delivered that the session never saw.
    #[tokio::test]
    async fn the_stamped_ids_are_exactly_the_ones_the_block_carried() {
        let store = Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
        let (a, b) = two_sessions(&store);
        let mut ids = Vec::new();
        {
            let s = store.lock().unwrap();
            ids.push(
                s.insert_message(a, b, "answer me", "question", None)
                    .unwrap(),
            );
            for i in 0..60 {
                let body = format!("{i:03}{}", "m".repeat(200));
                ids.push(s.insert_message(a, b, &body, "message", None).unwrap());
            }
        }
        let v = stop_hook_json(&store).await;
        let reason = v["reason"].as_str().unwrap();
        let mut carried = carried_ids(reason);
        carried.sort_unstable();
        let mut stamped = stamped_ids(&store, b, &ids);
        stamped.sort_unstable();
        assert_eq!(
            stamped, carried,
            "stamped {stamped:?} but the block carried {carried:?}"
        );
    }

    /// `reason`'s 20-LINE cap was never accounted for anywhere: a batch of
    /// short multi-line bodies fits 2000 chars easily and still gets cut by
    /// the CLI.
    #[tokio::test]
    async fn a_multi_line_batch_respects_the_twenty_line_reason_cap() {
        let store = Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
        let (a, b) = two_sessions(&store);
        {
            let s = store.lock().unwrap();
            s.insert_message(a, b, "one\ntwo\nthree\nfour\nfive", "question", None)
                .unwrap();
            for i in 0..30 {
                let body = format!("{i}a\n{i}b\n{i}c\n{i}d\n{i}e");
                s.insert_message(a, b, &body, "message", None).unwrap();
            }
        }
        let v = stop_hook_json(&store).await;
        assert_eq!(v["decision"], "block", "{v}");
        let reason = v["reason"].as_str().unwrap();
        assert!(
            reason.lines().count() <= REASON_LINES,
            "reason lines = {}: {reason}",
            reason.lines().count()
        );
    }

    /// `stop_action` must decide on what will actually be CARRIED, not on
    /// the whole pending list: a question that falls outside the block
    /// budget would otherwise cause a block whose `reason` does not contain
    /// it — a burnt turn that tells the agent nothing.
    #[tokio::test]
    async fn a_question_outside_the_block_budget_does_not_block() {
        let store = Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
        let (a, b) = two_sessions(&store);
        let question_id = {
            let s = store.lock().unwrap();
            // Oldest first: plain mail fills the 2000-char budget, and the
            // question — inserted last — cannot be in it.
            for i in 0..30 {
                let body = format!("{i:03}{}", "m".repeat(200));
                s.insert_message(a, b, &body, "message", None).unwrap();
            }
            s.insert_message(a, b, "answer me", "question", None)
                .unwrap()
        };
        let v = stop_hook_json(&store).await;
        assert!(
            v.get("decision").is_none(),
            "the question cannot ride this block, so nothing may block: {v}"
        );
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(
            ctx.contains(&format!("#{question_id}")) || ctx.contains("more message(s) waiting"),
            "the question is either carried as context or reported as waiting: {ctx}"
        );
    }
}
