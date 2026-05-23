//! Store mutations triggered by Claude Code HTTP hook events.
//!
//! Called from `mcp::hooks::handle_hook` after token auth passes.

use crate::ipc_error::IpcError;
use crate::mcp::hooks::HookPayload;
use crate::store::{ProjectRow, Store};
use std::sync::{Arc, Mutex};

fn lock_err() -> IpcError {
    IpcError::new("E_LOCK", "store mutex poisoned")
}

/// Dispatch a hook event to the appropriate handler.
/// Unknown events are silently ignored.
pub fn apply_hook(store: &Arc<Mutex<Store>>, payload: &HookPayload) -> Result<(), IpcError> {
    match payload.hook_event_name.as_deref() {
        Some("Stop") => apply_stop_hook(store, payload),
        Some("PostToolUse") if payload.tool_name.as_deref() == Some("WorktreeCreate") => {
            apply_worktree_hook(store, payload)
        }
        _ => Ok(()),
    }
}

/// Mark the matching session's `claude_status` as "idle".
/// Matches by `claude_session_id`. No-ops if no session has this ID.
///
/// Claude Code's `Stop` hook fires when the agent finishes a turn and is ready
/// for input again — NOT when the session terminates. So the right status is
/// "idle", not "stopped": stamping "stopped" here made every normal
/// turn-completion mark the session stopped, and reconcile's pane heuristic
/// never produced "stopped" to clear it, so sessions hung in "stopped".
fn apply_stop_hook(store: &Arc<Mutex<Store>>, payload: &HookPayload) -> Result<(), IpcError> {
    let session_id = match &payload.session_id {
        Some(id) => id.clone(),
        None => return Ok(()),
    };
    let s = store.lock().map_err(|_| lock_err())?;
    s.set_claude_status_by_session_id(&session_id, "idle")
}

/// Auto-register a worktree created by Claude Code's WorktreeCreate tool.
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
        .filter(|s| !s.is_empty());
    let name = std::path::Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .to_string();

    let s = store.lock().map_err(|_| lock_err())?;
    let projects = s.list_projects().map_err(IpcError::from)?;
    let project_id = match find_project_id_for_path(&projects, &path) {
        Some(id) => id,
        None => {
            eprintln!("[hook] WorktreeCreate: no project found for path {path}");
            return Ok(());
        }
    };
    s.upsert_worktree(project_id, &name, &path, branch)
        .map_err(IpcError::from)?;
    Ok(())
}

fn find_project_id_for_path(projects: &[ProjectRow], worktree_path: &str) -> Option<i64> {
    projects
        .iter()
        .filter(|p| is_path_prefix(&p.base_path, worktree_path))
        .max_by_key(|p| p.base_path.len())
        .map(|p| p.id)
}

/// True iff `base` is a path-component prefix of `path`.
/// Prevents "/home/u/proj" from matching "/home/u/project/...".
fn is_path_prefix(base: &str, path: &str) -> bool {
    if path == base {
        return true;
    }
    match path.strip_prefix(base) {
        Some(rest) => rest.starts_with('/'),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::hooks::HookPayload;
    use crate::store::Store;
    use std::sync::Arc;

    fn make_store() -> Arc<Mutex<Store>> {
        Arc::new(Mutex::new(Store::open_in_memory().unwrap()))
    }

    fn make_payload(event: &str, session_id: &str) -> HookPayload {
        HookPayload {
            session_id: Some(session_id.into()),
            hook_event_name: Some(event.into()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            cwd: None,
        }
    }

    #[test]
    fn stop_hook_on_unknown_session_is_noop() {
        let store = make_store();
        let payload = make_payload("Stop", "no-such-id");
        assert!(apply_hook(&store, &payload).is_ok());
    }

    #[test]
    fn stop_hook_sets_matching_session_to_idle() {
        // A turn finishing (Stop hook) means the session is idle/ready, not
        // terminated — see apply_stop_hook.
        let store = make_store();
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let id = s
                .upsert_session("sess", "local", None, None, 0, 0, "running", None)
                .unwrap();
            s.set_claude_session_id(id, "uuid-1").unwrap();
            // Pretend it was last seen working.
            s.set_claude_status_by_session_id("uuid-1", "working")
                .unwrap();
        }
        apply_hook(&store, &make_payload("Stop", "uuid-1")).unwrap();
        let s = store.lock().unwrap();
        let row = s.get_session("sess", "local").unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
    }

    #[test]
    fn unknown_event_is_noop() {
        let store = make_store();
        let payload = make_payload("UserPromptSubmit", "s1");
        assert!(apply_hook(&store, &payload).is_ok());
    }

    #[test]
    fn worktree_hook_without_tool_input_is_noop() {
        let store = make_store();
        let payload = HookPayload {
            session_id: Some("s1".into()),
            hook_event_name: Some("PostToolUse".into()),
            tool_name: Some("WorktreeCreate".into()),
            tool_input: None,
            tool_response: None,
            cwd: None,
        };
        assert!(apply_hook(&store, &payload).is_ok());
    }

    #[test]
    fn find_project_id_for_path_longest_prefix_wins() {
        let projects = vec![
            ProjectRow {
                id: 1,
                owner: "o".into(),
                repo: "r".into(),
                base_path: "/home/u/proj".into(),
                last_session_at: None,
            },
            ProjectRow {
                id: 2,
                owner: "o".into(),
                repo: "r2".into(),
                base_path: "/home/u/proj/sub".into(),
                last_session_at: None,
            },
        ];
        assert_eq!(
            find_project_id_for_path(&projects, "/home/u/proj/sub/.worktrees/feat"),
            Some(2)
        );
        assert_eq!(
            find_project_id_for_path(&projects, "/home/u/proj/.worktrees/feat"),
            Some(1)
        );
        assert_eq!(find_project_id_for_path(&projects, "/other/path"), None);
    }

    #[test]
    fn find_project_id_rejects_partial_dirname_match() {
        let projects = vec![ProjectRow {
            id: 1,
            owner: "o".into(),
            repo: "r".into(),
            base_path: "/home/u/proj".into(),
            last_session_at: None,
        }];
        // "/home/u/project/..." must NOT match "/home/u/proj"
        assert_eq!(
            find_project_id_for_path(&projects, "/home/u/project/.worktrees/feat"),
            None
        );
        // Exact prefix with separator must still match
        assert_eq!(
            find_project_id_for_path(&projects, "/home/u/proj/.worktrees/feat"),
            Some(1)
        );
    }
}
