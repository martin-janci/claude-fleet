//! Store mutations triggered by Claude Code HTTP hook events.
//!
//! Called from `mcp::hooks::handle_hook` after token auth passes.

use crate::ipc_error::IpcError;
use crate::mcp::hooks::HookPayload;
use crate::ssh::SshClient;
use crate::store::{ProjectRow, Store};
use std::sync::{Arc, Mutex};

/// Dispatch a hook event to the appropriate handler.
/// Unknown events are silently ignored.
pub fn apply_hook(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    payload: &HookPayload,
) -> Result<(), IpcError> {
    match payload.hook_event_name.as_deref() {
        Some("Stop") => apply_stop_hook(store, ssh, payload),
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
fn apply_stop_hook(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    payload: &HookPayload,
) -> Result<(), IpcError> {
    let session_id = match &payload.session_id {
        Some(id) => id.clone(),
        None => return Ok(()),
    };
    // Snapshot whether a safe-kill is in flight BEFORE we update status —
    // if it is, we spawn the marker check off the hook handler so the HTTP
    // response returns fast (the work involves pane capture + SSH).
    let safe_kill_in_flight = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let in_flight = s
            .get_session_by_claude_id(&session_id)
            .ok()
            .flatten()
            .map(|r| r.safe_kill_state.as_deref() == Some("requested"))
            .unwrap_or(false);
        s.set_claude_status_by_session_id(&session_id, "idle")?;
        in_flight
    };
    if safe_kill_in_flight {
        let store = Arc::clone(store);
        let ssh = Arc::clone(ssh);
        let sid = session_id.clone();
        tauri::async_runtime::spawn(async move {
            crate::service::safe_kill::handle_stop_marker_check(store, ssh, sid).await;
        });
    }
    Ok(())
}

/// Validate a `worktree_path` from a hook body before it becomes a row:
/// absolute, no `..` component, no control characters, and its basename a
/// safe path component. The hook body is network input signed only by a
/// host token, so it gets the same scrutiny as a frontend value.
pub fn validate_worktree_path(path: &str) -> Result<(), IpcError> {
    if path.is_empty() || path.len() > 4096 {
        return Err(IpcError::new(
            "E_VALIDATE",
            "worktree_path must be a non-empty path under 4096 bytes",
        ));
    }
    if !path.starts_with('/') {
        return Err(IpcError::new(
            "E_VALIDATE",
            "worktree_path must be absolute",
        ));
    }
    if path.chars().any(|c| c.is_control()) {
        return Err(IpcError::new(
            "E_VALIDATE",
            "worktree_path must not contain control characters",
        ));
    }
    if path.split('/').any(|c| c == "..") {
        return Err(IpcError::new(
            "E_VALIDATE",
            "worktree_path must not contain a '..' component",
        ));
    }
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| IpcError::new("E_VALIDATE", "worktree_path has no final component"))?;
    crate::validate::path_component("worktree name", name)
        .map_err(|e| IpcError::new("E_VALIDATE", e.message))?;
    Ok(())
}

/// Auto-register a worktree created by Claude Code's WorktreeCreate tool.
///
/// The path must validate ([`validate_worktree_path`]) AND sit under a known
/// project's `base_path`; anything else is `E_VALIDATE` (the handler answers
/// 400) rather than a silent upsert of an arbitrary row.
fn apply_worktree_hook(store: &Arc<Mutex<Store>>, payload: &HookPayload) -> Result<(), IpcError> {
    let input = match &payload.tool_input {
        Some(v) => v,
        None => return Ok(()),
    };
    let path = match input.get("worktree_path").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => return Ok(()),
    };
    validate_worktree_path(&path)?;
    let branch = input
        .get("branch")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    if let Some(b) = branch {
        crate::validate::git_ref(b).map_err(|e| IpcError::new("E_VALIDATE", e.message))?;
    }
    let name = std::path::Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .to_string();

    let s = store.lock().map_err(|_| IpcError::lock())?;
    let projects = s.list_projects().map_err(IpcError::from)?;
    let project_id = match find_project_id_for_path(&projects, &path) {
        Some(id) => id,
        None => {
            return Err(IpcError::new(
                "E_VALIDATE",
                format!("worktree_path {path} is not under any known project base"),
            ));
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
    use crate::ssh::SshClient;
    use crate::store::Store;
    use std::sync::Arc;

    fn make_store() -> Arc<Mutex<Store>> {
        Arc::new(Mutex::new(Store::open_in_memory().unwrap()))
    }

    fn make_ssh() -> Arc<SshClient> {
        Arc::new(SshClient::new())
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
        assert!(apply_hook(&store, &make_ssh(), &payload).is_ok());
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
        apply_hook(&store, &make_ssh(), &make_payload("Stop", "uuid-1")).unwrap();
        let s = store.lock().unwrap();
        let row = s.get_session("sess", "local").unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
    }

    #[test]
    fn unknown_event_is_noop() {
        let store = make_store();
        let payload = make_payload("UserPromptSubmit", "s1");
        assert!(apply_hook(&store, &make_ssh(), &payload).is_ok());
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
        assert!(apply_hook(&store, &make_ssh(), &payload).is_ok());
    }

    fn worktree_payload(path: &str, branch: Option<&str>) -> HookPayload {
        let mut input = serde_json::json!({ "worktree_path": path });
        if let Some(b) = branch {
            input["branch"] = serde_json::Value::String(b.into());
        }
        HookPayload {
            session_id: Some("s1".into()),
            hook_event_name: Some("PostToolUse".into()),
            tool_name: Some("WorktreeCreate".into()),
            tool_input: Some(input),
            tool_response: None,
            cwd: None,
        }
    }

    #[test]
    fn validate_worktree_path_accepts_absolute_clean_paths() {
        assert!(validate_worktree_path("/home/u/proj/.worktrees/feat").is_ok());
        assert!(validate_worktree_path("/home/u/proj/.worktrees/feat-x.y_z").is_ok());
    }

    #[test]
    fn validate_worktree_path_rejects_relative_traversal_and_control() {
        for bad in [
            "",
            "relative/path",
            "~/proj/.worktrees/feat",
            "/home/u/proj/../../etc",
            "/home/u/proj/.worktrees/..",
            "/home/u/proj/.worktrees/bad\nname",
            "/home/u/proj/.worktrees/-rf",
        ] {
            let err = validate_worktree_path(bad).expect_err(bad);
            assert_eq!(err.code, "E_VALIDATE", "{bad}");
        }
    }

    #[test]
    fn worktree_hook_rejects_path_outside_known_projects() {
        let store = make_store();
        {
            let s = store.lock().unwrap();
            s.upsert_project("o", "r", "/home/u/proj").unwrap();
        }
        let err = apply_hook(
            &store,
            &make_ssh(),
            &worktree_payload("/home/u/elsewhere/.worktrees/feat", None),
        )
        .unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
        let err = apply_hook(
            &store,
            &make_ssh(),
            &worktree_payload("../../etc/passwd", None),
        )
        .unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
        // A branch that looks like a git option is refused too.
        let err = apply_hook(
            &store,
            &make_ssh(),
            &worktree_payload("/home/u/proj/.worktrees/feat", Some("--upload-pack=x")),
        )
        .unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
    }

    #[test]
    fn worktree_hook_upserts_row_under_known_project() {
        let store = make_store();
        let pid = {
            let s = store.lock().unwrap();
            s.upsert_project("o", "r", "/home/u/proj").unwrap()
        };
        apply_hook(
            &store,
            &make_ssh(),
            &worktree_payload("/home/u/proj/.worktrees/feat", Some("feat")),
        )
        .unwrap();
        let s = store.lock().unwrap();
        let rows = s.list_worktrees_for_project(pid).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "/home/u/proj/.worktrees/feat");
        assert_eq!(rows[0].branch.as_deref(), Some("feat"));
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
