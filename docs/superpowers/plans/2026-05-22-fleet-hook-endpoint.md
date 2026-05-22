# Fleet Hook Endpoint Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `POST /hook` HTTP endpoint to the fleet MCP server so Claude Code can push real-time session-stop and WorktreeCreate events directly to the fleet — eliminating the poll lag for local sessions.

**Architecture:** The existing Axum router in `mcp/mod.rs` gains a second nested router for `/hook` that sits outside the MCP bearer-auth middleware (hooks use a `?token=` query-param instead). Two event types are handled: `Stop` updates `claude_status = "stopped"` on the matching session row; `PostToolUse[WorktreeCreate]` calls `store.upsert_worktree()` to auto-register the new worktree. An `install_fleet_hook` Tauri command writes the hook URL into the local `~/.claude/settings.json` so Claude Code sessions on the same machine start firing hooks immediately.

**Tech Stack:** Rust/axum (existing), serde_json, tokio, rusqlite via `Store`, existing `EventBus`

**Dependency:** Plan A (claude-session-intelligence) must be executed first. The `sessions.claude_session_id` and `sessions.claude_status` columns must already exist.

---

## File map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `src-tauri/src/mcp/hooks.rs` | `HookPayload` struct, `check_query_token`, Axum handler `handle_hook` |
| Modify | `src-tauri/src/mcp/mod.rs` | Scope bearer auth to `/mcp` only; add `/hook` sub-router with `HookState` |
| Create | `src-tauri/src/service/hooks.rs` | `apply_stop_hook`, `apply_worktree_hook` — Store mutations + EventBus |
| Modify | `src-tauri/src/service/mod.rs` | Declare `pub mod hooks;` |
| Modify | `src-tauri/src/commands/mcp.rs` | Add `install_fleet_hook` Tauri command |
| Modify | `src-tauri/src/lib.rs` | Register `install_fleet_hook` in `invoke_handler` |
| Modify | `src/lib/mcp.ts` | Add `installFleetHook(hostAlias)` frontend call |
| Modify | `src/lib/SettingsDialog.svelte` | "Install Hook" button in the MCP section |

---

### Task 1: `HookPayload` struct and query-param token check

**Files:**
- Create: `src-tauri/src/mcp/hooks.rs`

- [ ] **Step 1: Write failing tests**

```rust
// src-tauri/src/mcp/hooks.rs  (new file — add at bottom)
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
```

- [ ] **Step 2: Run to verify compile failure**

Run: `cd src-tauri && cargo test mcp::hooks 2>&1 | head -20`
Expected: error — module `mcp::hooks` not found

- [ ] **Step 3: Create `src-tauri/src/mcp/hooks.rs`**

```rust
//! `/hook` endpoint: receives Claude Code HTTP hook events in real-time.
//!
//! Claude Code POST JSON here on Stop and PostToolUse[WorktreeCreate] events.
//! Auth uses `?token=<token>` (hooks cannot set custom request headers).

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use std::{collections::HashMap, sync::{Arc, Mutex}};

use crate::service::hooks as svc;
use crate::store::Store;

/// Shared state injected into the `/hook` sub-router.
#[derive(Clone)]
pub struct HookState {
    pub store: Arc<Mutex<Store>>,
    pub token: Arc<String>,
}

/// The JSON body Claude Code sends for any hook event.
/// `deny_unknown_fields` is intentionally absent — Claude may add new fields
/// in future versions and we must not break on them.
#[derive(Debug, Deserialize)]
pub struct HookPayload {
    /// The Claude Code session ID (`CLAUDE_CODE_SESSION_ID` env var).
    pub session_id: Option<String>,
    /// "Stop", "PostToolUse", "PreToolUse", "UserPromptSubmit", "Notification".
    pub hook_event_name: Option<String>,
    /// Present for PostToolUse/PreToolUse. "WorktreeCreate", "Bash", etc.
    pub tool_name: Option<String>,
    /// Tool arguments (schema varies per tool).
    pub tool_input: Option<serde_json::Value>,
    /// Tool result. Present for PostToolUse.
    pub tool_response: Option<serde_json::Value>,
    /// Working directory of the Claude session when the hook fired.
    pub cwd: Option<String>,
}

/// Constant-time token comparison. Returns `true` iff `provided` is non-empty
/// and equals `expected`. Length check is fine here (token length is public).
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

/// `POST /hook?token=<token>` — accepts any Claude Code hook event.
pub async fn handle_hook(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<HookState>,
    Json(payload): Json<HookPayload>,
) -> StatusCode {
    // Token auth via query param.
    let provided = params.get("token").map(String::as_str).unwrap_or("");
    if !check_query_token(&state.token, provided) {
        eprintln!("[hook] rejected: bad token");
        return StatusCode::UNAUTHORIZED;
    }

    eprintln!(
        "[hook] event={:?} session={:?} tool={:?}",
        payload.hook_event_name, payload.session_id, payload.tool_name
    );

    match svc::apply_hook(&state.store, &payload) {
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
```

- [ ] **Step 4: Run tests**

Run: `cd src-tauri && cargo test mcp::hooks`
Expected: all 6 tests pass

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/mcp/hooks.rs
git commit -m "feat: add HookPayload types and query-token check in mcp/hooks.rs"
```

---

### Task 2: `service/hooks.rs` — store mutations for hook events

**Files:**
- Create: `src-tauri/src/service/hooks.rs`
- Modify: `src-tauri/src/service/mod.rs`

- [ ] **Step 1: Declare `pub mod hooks;` in service/mod.rs**

Read `src-tauri/src/service/mod.rs` first, then add the line:
```rust
pub mod hooks;
```
alongside the other `pub mod` declarations in that file.

- [ ] **Step 2: Write failing test (place at bottom of new hooks.rs)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NoopEventBus;
    use crate::mcp::hooks::HookPayload;
    use crate::store::Store;
    use std::sync::{Arc, Mutex};

    fn make_store() -> Arc<Mutex<Store>> {
        Arc::new(Mutex::new(
            Store::open_in_memory(Arc::new(NoopEventBus)).unwrap(),
        ))
    }

    #[test]
    fn stop_hook_on_unknown_session_is_noop() {
        let store = make_store();
        let payload = HookPayload {
            session_id: Some("no-such-id".into()),
            hook_event_name: Some("Stop".into()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            cwd: None,
        };
        // Should not error even when no session matches.
        let result = apply_hook(&store, &payload);
        assert!(result.is_ok());
    }

    #[test]
    fn unknown_event_is_noop() {
        let store = make_store();
        let payload = HookPayload {
            session_id: Some("s1".into()),
            hook_event_name: Some("UserPromptSubmit".into()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            cwd: None,
        };
        assert!(apply_hook(&store, &payload).is_ok());
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cd src-tauri && cargo test service::hooks 2>&1 | head -20`
Expected: compile error — module not found

- [ ] **Step 4: Create `src-tauri/src/service/hooks.rs`**

```rust
//! Store mutations triggered by Claude Code HTTP hook events.
//!
//! Called from `mcp::hooks::handle_hook` after token auth passes. Each
//! function is synchronous (Store is behind Mutex, must not hold across
//! `.await`) and returns `IpcError` on hard failures.

use crate::ipc_error::IpcError;
use crate::mcp::hooks::HookPayload;
use crate::store::Store;
use std::sync::{Arc, Mutex};

/// Dispatch a hook event to the appropriate handler. Unknown events are
/// silently ignored so future Claude Code versions don't break the server.
pub fn apply_hook(store: &Arc<Mutex<Store>>, payload: &HookPayload) -> Result<(), IpcError> {
    match payload.hook_event_name.as_deref() {
        Some("Stop") => apply_stop_hook(store, payload),
        Some("PostToolUse") if payload.tool_name.as_deref() == Some("WorktreeCreate") => {
            apply_worktree_hook(store, payload)
        }
        _ => Ok(()), // graceful no-op for unhandled events
    }
}

/// Mark the matching session's `claude_status` as "stopped".
/// Matches by `claude_session_id` (set during reconcile enrichment from Plan A).
/// If no session has this ID, that's fine — the reconcile will catch it.
fn apply_stop_hook(store: &Arc<Mutex<Store>>, payload: &HookPayload) -> Result<(), IpcError> {
    let session_id = match &payload.session_id {
        Some(id) => id.clone(),
        None => return Ok(()),
    };
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.set_claude_status_by_session_id(&session_id, "stopped")
}

/// Auto-register a worktree created by Claude Code's WorktreeCreate tool.
/// Extracts `worktree_path` and `branch` from `tool_input`, resolves project_id
/// from path, and calls `Store::upsert_worktree`.
fn apply_worktree_hook(store: &Arc<Mutex<Store>>, payload: &HookPayload) -> Result<(), IpcError> {
    let input = match &payload.tool_input {
        Some(v) => v,
        None => return Ok(()),
    };
    let path = match input.get("worktree_path").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => return Ok(()),
    };
    let branch = input
        .get("branch")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // Derive worktree name: last path component.
    let name = std::path::Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .to_string();

    let mut s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;

    // Resolve project_id from the worktree path using the existing helper.
    let projects = s.list_projects().map_err(IpcError::from)?;
    let project_id = find_project_id_for_path(&projects, &path);
    let project_id = match project_id {
        Some(id) => id,
        None => {
            eprintln!("[hook] WorktreeCreate: no project found for path {path}");
            return Ok(());
        }
    };

    s.upsert_worktree(project_id, &name, &path, &branch)
        .map_err(IpcError::from)?;
    Ok(())
}

/// Walk `projects` to find one whose `base_path` is a prefix of `worktree_path`.
/// Returns the project_id of the longest matching prefix (most specific).
fn find_project_id_for_path(
    projects: &[crate::store::ProjectRow],
    worktree_path: &str,
) -> Option<i64> {
    projects
        .iter()
        .filter(|p| worktree_path.starts_with(&p.base_path))
        .max_by_key(|p| p.base_path.len())
        .map(|p| p.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NoopEventBus;
    use crate::mcp::hooks::HookPayload;
    use crate::store::Store;
    use std::sync::Arc;

    fn make_store() -> Arc<Mutex<Store>> {
        Arc::new(Mutex::new(
            Store::open_in_memory(Arc::new(NoopEventBus)).unwrap(),
        ))
    }

    #[test]
    fn stop_hook_on_unknown_session_is_noop() {
        let store = make_store();
        let payload = HookPayload {
            session_id: Some("no-such-id".into()),
            hook_event_name: Some("Stop".into()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            cwd: None,
        };
        assert!(apply_hook(&store, &payload).is_ok());
    }

    #[test]
    fn unknown_event_is_noop() {
        let store = make_store();
        let payload = HookPayload {
            session_id: Some("s1".into()),
            hook_event_name: Some("UserPromptSubmit".into()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            cwd: None,
        };
        assert!(apply_hook(&store, &payload).is_ok());
    }
}
```

- [ ] **Step 5: Add `set_claude_status_by_session_id` to Store**

Read `src-tauri/src/store.rs`, find the `impl Store` block, and add:

```rust
/// Update `claude_status` for the session whose `claude_session_id` matches.
/// No-ops silently when no row matches (hook arrived before reconcile enriched it).
pub fn set_claude_status_by_session_id(
    &self,
    claude_session_id: &str,
    status: &str,
) -> Result<(), IpcError> {
    self.conn
        .execute(
            "UPDATE sessions SET claude_status = ?1 WHERE claude_session_id = ?2",
            rusqlite::params![status, claude_session_id],
        )
        .map_err(IpcError::from)?;
    // If rows_changed == 0, that's fine — reconcile will pick it up later.
    // Emit session:updated for any rows we did change.
    // For simplicity we skip the event emit here (reconcile re-emits anyway).
    Ok(())
}
```

Also add `ProjectWithWorktrees` and `list_projects` to the public API if not already exposed. Check the existing `list_projects_with_worktrees` function in store.rs and use whatever return type it uses. Adjust `find_project_id_for_path` in hooks.rs to match.

- [ ] **Step 6: Run tests**

Run: `cd src-tauri && cargo test service::hooks`
Expected: both tests pass

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/service/hooks.rs src-tauri/src/service/mod.rs src-tauri/src/store.rs
git commit -m "feat: service/hooks.rs with apply_stop_hook and apply_worktree_hook"
```

---

### Task 3: Wire `/hook` into the Axum router

**Files:**
- Modify: `src-tauri/src/mcp/mod.rs`
- Modify: `src-tauri/src/mcp/hooks.rs` (add `mod hooks;` declaration)

The key structural change: the existing bearer-auth middleware scopes only to `/mcp`; the `/hook` route sits in a sibling router that does its own auth inside the handler.

- [ ] **Step 1: Read the current `mcp/mod.rs`**

Read `src-tauri/src/mcp/mod.rs` to get the exact current router construction.

- [ ] **Step 2: Add `mod hooks;` to `mcp/mod.rs`**

After `mod auth;`:
```rust
mod auth;
pub mod hooks;
mod tools;
```

- [ ] **Step 3: Refactor the router to scope bearer auth to `/mcp` only**

Current router (in the `start` async fn):
```rust
let app =
    axum::Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let token = Arc::clone(&token);
                async move {
                    match auth::check_request(request.headers(), &token) {
                        Ok(()) => Ok(next.run(request).await),
                        Err(status) => {
                            eprintln!("[mcp] rejected request: {status}");
                            Err(status)
                        }
                    }
                }
            },
        ));
```

Replace with:

```rust
// The bearer-auth middleware applies only to /mcp routes.
let mcp_token = Arc::clone(&token);
let mcp_router = axum::Router::new()
    .nest_service("/", service)
    .layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let token = Arc::clone(&mcp_token);
            async move {
                match auth::check_request(request.headers(), &token) {
                    Ok(()) => Ok(next.run(request).await),
                    Err(status) => {
                        eprintln!("[mcp] rejected request: {status}");
                        Err(status)
                    }
                }
            }
        },
    ));

// The /hook route validates the token itself via ?token= query param.
let hook_state = hooks::HookState {
    store: Arc::clone(&store),
    token: Arc::clone(&token),
};
let hook_router = axum::Router::new()
    .route("/", axum::routing::post(hooks::handle_hook))
    .with_state(hook_state);

let app = axum::Router::new()
    .nest("/mcp", mcp_router)
    .nest("/hook", hook_router);
```

Note: `store` must be passed to `start()` — it already is (`Arc<Mutex<Store>>`). The existing `token: String` parameter must become `Arc<String>` to be shareable. Change the function signature and adjust callers.

- [ ] **Step 4: Update `start()` signature to pass `Arc<String>` token**

In `mcp/mod.rs`:
```rust
pub async fn start(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    reg: Arc<CancellationRegistry>,
    port: u16,
    token: String,        // ← keep as String here; wrap inside fn
) -> Result<CancellationToken, String> {
    // ...
    let token = Arc::new(token);   // wrap once at the top of the fn
    // ... rest uses Arc::clone(&token)
```

- [ ] **Step 5: Verify the server compiles**

Run: `cd src-tauri && cargo build 2>&1 | head -40`
Expected: compiles without errors

- [ ] **Step 6: Smoke-test the hook endpoint manually**

With the app running and MCP enabled, in a terminal:
```bash
# Replace TOKEN with the token shown in Settings > MCP
curl -s -X POST "http://127.0.0.1:4180/hook?token=TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"session_id":"test","hook_event_name":"Stop"}'
# Expected: HTTP 204 No Content
```

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/mcp/mod.rs src-tauri/src/mcp/hooks.rs
git commit -m "feat: add /hook route to Axum server, scope bearer auth to /mcp only"
```

---

### Task 4: `install_fleet_hook` Tauri command

**Files:**
- Modify: `src-tauri/src/commands/mcp.rs`
- Modify: `src-tauri/src/lib.rs`

The command reads the current port+token from the store and merges the fleet hook into the local `~/.claude/settings.json`. Only the `local` host is supported (remote hosts can't reach 127.0.0.1 on the user's machine without a tunnel).

- [ ] **Step 1: Write the test first**

Add to `commands/mcp.rs` tests section:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_hook_config_produces_valid_json() {
        let cfg = build_hook_config("http://127.0.0.1:4180/hook?token=abc");
        // Must be valid JSON and contain both Stop and PostToolUse hooks.
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert!(v["hooks"]["Stop"].is_array());
        assert!(v["hooks"]["PostToolUse"].is_array());
        let url = v["hooks"]["Stop"][0]["hooks"][0]["url"].as_str().unwrap();
        assert!(url.contains("4180"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test commands::mcp`
Expected: compile error — `build_hook_config` not found

- [ ] **Step 3: Add `build_hook_config` and `install_fleet_hook` to `commands/mcp.rs`**

```rust
/// Build the hook configuration JSON fragment to merge into ~/.claude/settings.json.
/// Returns a serde_json::Value (not a string) for easy merging.
fn build_hook_block(url: &str) -> serde_json::Value {
    serde_json::json!([{
        "matcher": "",
        "hooks": [{ "type": "http", "url": url }]
    }])
}

/// Build the full settings JSON fragment containing both hook entries.
/// Only used by tests; the real command merges into an existing file.
pub fn build_hook_config(url: &str) -> String {
    let v = serde_json::json!({
        "hooks": {
            "Stop": build_hook_block(url),
            "PostToolUse": [{
                "matcher": "WorktreeCreate",
                "hooks": [{ "type": "http", "url": url }]
            }]
        }
    });
    serde_json::to_string_pretty(&v).unwrap()
}

/// Install (or update) the fleet hook in the local `~/.claude/settings.json`.
///
/// The command reads the current token and port from the store, constructs
/// the hook URL, then merges the hook entries into the settings file without
/// disturbing unrelated settings. Any existing fleet hooks pointing at the
/// same port are replaced to avoid duplicates.
///
/// Only supports `host_alias == "local"` — remote machines can't reach
/// 127.0.0.1 on this machine without a tunnel.
#[tauri::command]
pub fn install_fleet_hook(
    host_alias: String,
    store: State<'_, Arc<Mutex<Store>>>,
    runtime: State<'_, Mutex<McpRuntime>>,
) -> Result<String, IpcError> {
    if host_alias != "local" {
        return Err(IpcError::new(
            "E_UNSUPPORTED",
            "install_fleet_hook only supports the local host",
        ));
    }

    let (port, token) = {
        let s = store.lock().map_err(|_| lock_err())?;
        let port = s
            .get_setting(mcp::SETTING_PORT)?
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(mcp::DEFAULT_PORT);
        let token = s.get_setting(mcp::SETTING_TOKEN)?.unwrap_or_default();
        (port, token)
    };

    if token.is_empty() {
        return Err(IpcError::new(
            "E_NO_TOKEN",
            "MCP token not configured — enable the MCP server first",
        ));
    }

    // Verify MCP server is actually running (hook is useless if server is off).
    {
        let rt = runtime.lock().map_err(|_| lock_err())?;
        if !rt.is_running() {
            return Err(IpcError::new(
                "E_NOT_RUNNING",
                "MCP server is not running — enable it in Settings > MCP first",
            ));
        }
    }

    let hook_url = format!("http://127.0.0.1:{port}/hook?token={token}");

    // Read existing ~/.claude/settings.json (create empty object if absent).
    let settings_path = dirs::home_dir()
        .ok_or_else(|| IpcError::new("E_HOME", "cannot determine home directory"))?
        .join(".claude")
        .join("settings.json");

    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = std::fs::read_to_string(&settings_path)
            .map_err(|e| IpcError::new("E_IO", format!("read settings.json: {e}")))?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Ensure hooks object exists.
    let hooks = settings
        .as_object_mut()
        .ok_or_else(|| IpcError::new("E_PARSE", "settings.json root is not an object"))?
        .entry("hooks")
        .or_insert(serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| IpcError::new("E_PARSE", "hooks is not an object"))?;

    // Helper: strip any existing fleet hook entries (127.0.0.1 + our port).
    let fleet_prefix = format!("http://127.0.0.1:{port}/hook");
    let strip_fleet = |arr: &serde_json::Value| -> serde_json::Value {
        let items = arr.as_array().cloned().unwrap_or_default();
        serde_json::Value::Array(
            items
                .into_iter()
                .filter(|block| {
                    // Keep blocks that don't contain a fleet hook URL.
                    let hooks_arr = block.get("hooks").and_then(|h| h.as_array());
                    hooks_arr.map_or(true, |hs| {
                        !hs.iter().any(|h| {
                            h.get("url")
                                .and_then(|u| u.as_str())
                                .map_or(false, |u| u.starts_with(&fleet_prefix))
                        })
                    })
                })
                .collect(),
        )
    };

    // Stop hook: fires when Claude finishes a session.
    let mut stop_arr = strip_fleet(hooks.get("Stop").unwrap_or(&serde_json::json!([])));
    stop_arr
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "matcher": "",
            "hooks": [{ "type": "http", "url": hook_url }]
        }));
    hooks.insert("Stop".into(), stop_arr);

    // PostToolUse[WorktreeCreate]: fires after a worktree is created.
    let mut ptu_arr = strip_fleet(hooks.get("PostToolUse").unwrap_or(&serde_json::json!([])));
    ptu_arr
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "matcher": "WorktreeCreate",
            "hooks": [{ "type": "http", "url": hook_url }]
        }));
    hooks.insert("PostToolUse".into(), ptu_arr);

    // Write back.
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| IpcError::new("E_IO", format!("create .claude dir: {e}")))?;
    }
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|e| IpcError::new("E_SERIALIZE", e.to_string()))?;
    std::fs::write(&settings_path, json)
        .map_err(|e| IpcError::new("E_IO", format!("write settings.json: {e}")))?;

    Ok(format!(
        "Hook installed at {hook_url}\nSettings written to {}",
        settings_path.display()
    ))
}
```

Also add `dirs` to `Cargo.toml` if not already present:
```toml
dirs = "5"
```

Check if `dirs` is already a dependency first: `grep dirs src-tauri/Cargo.toml`

- [ ] **Step 4: Run test**

Run: `cd src-tauri && cargo test commands::mcp`
Expected: `build_hook_config_produces_valid_json` passes

- [ ] **Step 5: Register in `lib.rs`**

Find the `invoke_handler![]` call in `src-tauri/src/lib.rs` and add `commands::mcp::install_fleet_hook`.

- [ ] **Step 6: Verify compile**

Run: `cd src-tauri && cargo build 2>&1 | head -30`
Expected: clean build

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/commands/mcp.rs src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat: install_fleet_hook command — writes hook config to ~/.claude/settings.json"
```

---

### Task 5: Frontend — hook install button in SettingsDialog

**Files:**
- Create: `src/lib/mcp.ts` (or modify if it already exists — check first)
- Modify: `src/lib/SettingsDialog.svelte`

- [ ] **Step 1: Read `src/lib/SettingsDialog.svelte` to understand the MCP section**

Read the file and find where `mcp_status` and `mcp_configure` are called.

- [ ] **Step 2: Add `installFleetHook` to `src/lib/mcp.ts`** (create if doesn't exist)

Check first: `ls src/lib/mcp.ts 2>/dev/null`

If it doesn't exist, the IPC calls for MCP live directly in `SettingsDialog.svelte`. Add the function there or in a new `src/lib/mcp.ts`:

```typescript
import { invoke } from "@tauri-apps/api/core";

export async function installFleetHook(hostAlias: string): Promise<string> {
  return invoke<string>("install_fleet_hook", { hostAlias });
}
```

- [ ] **Step 3: Add "Install Hook" button to SettingsDialog.svelte**

In the MCP settings section, after the existing controls, add:

```svelte
<script lang="ts">
  // ... existing script
  import { installFleetHook } from "$lib/mcp";

  let hookInstallMsg = $state<string | null>(null);
  let hookInstallError = $state<string | null>(null);
  let installingHook = $state(false);

  async function doInstallHook() {
    installingHook = true;
    hookInstallMsg = null;
    hookInstallError = null;
    try {
      hookInstallMsg = await installFleetHook("local");
    } catch (e: unknown) {
      hookInstallError = e instanceof Error ? e.message : String(e);
    } finally {
      installingHook = false;
    }
  }
</script>

<!-- In the template, inside the MCP section: -->
<div class="hook-section">
  <p class="hook-desc">
    Install a real-time hook so local Claude Code sessions notify fleet
    immediately on stop or worktree creation.
  </p>
  <button
    class="btn-secondary"
    onclick={doInstallHook}
    disabled={installingHook || !mcpStatus?.running}
    data-testid="install-fleet-hook"
  >
    {installingHook ? "Installing…" : "Install Hook (local)"}
  </button>
  {#if hookInstallMsg}
    <p class="hook-ok">{hookInstallMsg}</p>
  {/if}
  {#if hookInstallError}
    <p class="hook-err">{hookInstallError}</p>
  {/if}
</div>
```

- [ ] **Step 4: Run frontend type-check**

Run: `pnpm check`
Expected: no type errors

- [ ] **Step 5: Run frontend tests**

Run: `pnpm test`
Expected: no regressions

- [ ] **Step 6: Commit**

```bash
git add src/lib/SettingsDialog.svelte src/lib/mcp.ts
git commit -m "feat: Install Hook button in MCP settings panel"
```

---

### Task 6: Final verification

- [ ] **Step 1: Run all backend tests**

Run: `cd src-tauri && cargo test`
Expected: all tests pass

- [ ] **Step 2: Run frontend tests and type-check**

Run: `pnpm test && pnpm check`
Expected: no failures

- [ ] **Step 3: Manual end-to-end smoke test**

1. Start the app, go to Settings → MCP, enable MCP server
2. Click "Install Hook (local)"
3. Verify `~/.claude/settings.json` now contains both Stop and PostToolUse hooks pointing to `127.0.0.1:4180`
4. In a terminal, POST a fake Stop hook:
   ```bash
   curl -s -X POST "http://127.0.0.1:4180/hook?token=<TOKEN>" \
     -H "Content-Type: application/json" \
     -d '{"session_id":"test-session","hook_event_name":"Stop"}'
   ```
   Expected: HTTP 204 response

- [ ] **Step 4: Commit final state if needed**

```bash
git status && git add -p && git commit -m "chore: plan B final verification"
```
