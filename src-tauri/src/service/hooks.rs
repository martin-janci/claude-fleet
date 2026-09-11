//! Store mutations triggered by Claude Code HTTP hook events.
//!
//! Called from `mcp::hooks::handle_hook` after token auth passes.

use crate::ipc_error::IpcError;
use crate::mcp::hooks::HookPayload;
use crate::mcp::Caller;
use crate::projects::path_identity::{canonical, canonical_str, is_within};
use crate::service::projects::LOCAL_HOST;
use crate::service::sessions::HostPaths;
use crate::ssh::SshClient;
use crate::store::{ProjectRow, SessionRow, Store};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Dispatch a hook event to the appropriate handler. `caller` is the
/// identity behind the request's bearer token; a per-host caller may only
/// report about sessions on its own host. Unknown events are silently
/// ignored.
pub fn apply_hook(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    payload: &HookPayload,
    caller: &Caller,
) -> Result<(), IpcError> {
    match payload.hook_event_name.as_deref() {
        Some("Stop") => apply_stop_hook(store, ssh, payload, caller),
        Some("UserPromptSubmit") => apply_prompt_submit_hook(store, payload, caller),
        // `EnterWorktree` is the real tool (the installed matcher).
        // `WorktreeCreate` is a hook EVENT that replaces git worktree
        // creation, not a tool — no PostToolUse ever carries it; it is still
        // accepted here only so a hand-posted legacy body keeps validating.
        Some("PostToolUse")
            if matches!(
                payload.tool_name.as_deref(),
                Some("EnterWorktree") | Some("WorktreeCreate")
            ) =>
        {
            apply_worktree_hook(store, payload, caller)
        }
        // `ExitWorktree { action: "remove" }` deleted the worktree: drop its
        // row on the caller's host. The `WorktreeRemove` hook EVENT is never
        // installed: removal fails when its hook leaves the directory behind,
        // so fleet cannot be (or sit beside) that hook.
        Some("PostToolUse") if payload.tool_name.as_deref() == Some("ExitWorktree") => {
            apply_worktree_exit_hook(store, payload, caller)
        }
        _ => Ok(()),
    }
}

/// Accept a hook-reported `transcript_path` only when it is an absolute,
/// `..`-free, control-free path under a `.claude/projects/` directory whose
/// file name is exactly `<claude_session_id>.jsonl`. It becomes a file fleet
/// later `tail`s on the host, so it gets the same scrutiny as a worktree
/// path. Anything else is ignored (never an error — the hook still counts).
pub fn valid_transcript_path(path: &str, claude_session_id: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 4096
        && !path.chars().any(|c| c.is_control())
        && !path.split('/').any(|c| c == "..")
        && path.contains("/.claude/projects/")
        && std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            == Some(format!("{claude_session_id}.jsonl").as_str())
}

/// Store the hook's transcript path on the row when it validates.
fn remember_transcript_path(s: &Store, payload: &HookPayload, claude_session_id: &str) {
    if let Some(p) = payload
        .transcript_path
        .as_deref()
        .filter(|p| valid_transcript_path(p, claude_session_id))
    {
        let _ = s.set_transcript_path_by_claude_id(claude_session_id, p);
    }
}

/// Look up the session a hook is about and apply the caller's host binding:
/// a host token may only flip sessions on ITS host — host A's token must
/// not be able to mark host B's session idle (and so trigger B's safe-kill
/// finalisation or complete B's tasks). Unknown session → `None` (the hook
/// arrived before reconcile enriched the row; a no-op, as before).
fn host_checked_row(
    s: &Store,
    claude_session_id: &str,
    caller: &Caller,
) -> Result<Option<SessionRow>, IpcError> {
    let row = s.get_session_by_claude_id(claude_session_id)?;
    if let (Some(row), Some(h)) = (&row, &caller.host_alias) {
        if &row.host_alias != h {
            return Err(IpcError::new(
                "E_FORBIDDEN",
                format!(
                    "session {} is on host {}; this token is bound to {h}",
                    row.tmux_name, row.host_alias
                ),
            ));
        }
    }
    Ok(row)
}

/// The Stop hook: a turn just completed. Marks the session `idle`, bumps
/// `turn_seq` and stamps `last_stop_at` (the completion signal `send_prompt`
/// / `wait_for_session` / `run_prompt` build on), then kicks off the
/// background checks that read the pane / transcript: the safe-kill marker
/// scan and the task-completion marker scan. Both are spawned so the HTTP
/// response returns fast.
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
    caller: &Caller,
) -> Result<(), IpcError> {
    let session_id = match &payload.session_id {
        Some(id) => id.clone(),
        None => return Ok(()),
    };
    // Snapshot whether a safe-kill / open task is in flight BEFORE we update
    // status; the follow-ups (pane capture + SSH) run off the hook handler.
    let (safe_kill_in_flight, task_worker) = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let Some(before) = host_checked_row(&s, &session_id, caller)? else {
            return Ok(());
        };
        let in_flight = before.safe_kill_state.as_deref() == Some("requested");
        remember_transcript_path(&s, payload, &session_id);
        let after = s.record_stop_hook(&session_id)?;
        let has_open_tasks = s
            .open_tasks_for_worker(before.id)
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        (in_flight, after.filter(|_| has_open_tasks))
    };
    if safe_kill_in_flight {
        let store = Arc::clone(store);
        let ssh = Arc::clone(ssh);
        let sid = session_id.clone();
        tauri::async_runtime::spawn(async move {
            crate::service::safe_kill::handle_stop_marker_check(store, ssh, sid).await;
        });
    }
    if let Some(worker) = task_worker {
        let store = Arc::clone(store);
        let ssh = Arc::clone(ssh);
        let cwd = payload.cwd.clone();
        tauri::async_runtime::spawn(async move {
            crate::service::tasks::handle_stop_for_worker(store, ssh, worker, cwd).await;
        });
    }
    Ok(())
}

/// The UserPromptSubmit hook: a turn is starting. Marks the session
/// `working` so an idle-looking pane between the submit and the first
/// spinner frame is not mistaken for "still idle" — and so `wait_for_session
/// { until: "idle" }` after a `send_prompt` does not return before the turn
/// even begins.
fn apply_prompt_submit_hook(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    caller: &Caller,
) -> Result<(), IpcError> {
    let session_id = match &payload.session_id {
        Some(id) => id.clone(),
        None => return Ok(()),
    };
    let s = store.lock().map_err(|_| IpcError::lock())?;
    if host_checked_row(&s, &session_id, caller)?.is_none() {
        return Ok(());
    }
    remember_transcript_path(&s, payload, &session_id);
    s.record_prompt_submit_hook(&session_id)?;
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

/// The worktree path + branch an `EnterWorktree` call reported. Claude
/// Code's docs do not pin the tool's result shape (and no local transcript
/// had a sample), so the path is read from the keys it is known or likely to
/// use — `tool_response` first, then `tool_input` — and nothing is guessed
/// beyond that: no path → no-op.
pub fn worktree_fields(payload: &HookPayload) -> (Option<String>, Option<String>) {
    const PATH_KEYS: [&str; 3] = ["worktreePath", "worktree_path", "path"];
    const BRANCH_KEYS: [&str; 3] = ["branch", "branchName", "worktreeBranch"];
    let pick = |keys: &[&str]| -> Option<String> {
        [payload.tool_response.as_ref(), payload.tool_input.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|v| {
                keys.iter()
                    .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
                    .map(str::to_string)
            })
    };
    (pick(&PATH_KEYS), pick(&BRANCH_KEYS))
}

/// The host a hook's paths live on: the per-host token's host, or `local`
/// for the master token (desktop / local agent use).
fn caller_host(caller: &Caller) -> &str {
    caller.host_alias.as_deref().unwrap_or(LOCAL_HOST)
}

/// `ExitWorktree` returned. When it REMOVED the worktree
/// (`tool_input.action == "remove"`), delete that checkout's row on the
/// caller's host; `keep` leaves the row. The removed path comes from the tool
/// result ([`worktree_fields`]); without one this is a no-op, as for
/// EnterWorktree. Sessions still pointing at the row are cleared, since the
/// directory is gone. Only the caller's own host is touched, so one host's
/// token can never drop another host's rows.
fn apply_worktree_exit_hook(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    caller: &Caller,
) -> Result<(), IpcError> {
    let removed = payload
        .tool_input
        .as_ref()
        .and_then(|v| v.get("action"))
        .and_then(|a| a.as_str())
        == Some("remove");
    if !removed {
        return Ok(());
    }
    let (path, _) = worktree_fields(payload);
    let Some(path) = path else {
        return Ok(());
    };
    validate_worktree_path(&path)?;
    let host = caller_host(caller);
    // Local rows are stored canonically; a removed directory resolves
    // through its nearest existing ancestor.
    let path = if host == LOCAL_HOST {
        canonical_str(&path)
    } else {
        path
    };
    let s = store.lock().map_err(|_| IpcError::lock())?;
    s.delete_worktrees_at(host, &path).map_err(IpcError::from)?;
    Ok(())
}

/// Register a worktree Claude Code entered via its `EnterWorktree` tool, as a
/// row of the CALLER's host.
///
/// The path must validate ([`validate_worktree_path`]) and belong to a known
/// project; anything else is `E_VALIDATE` (the handler answers 400) rather
/// than a silent upsert of an arbitrary row. "Belongs" depends on whose
/// filesystem the path is on:
///
/// - Local (the master token, or the `local` host token): the path is
///   canonicalized (under a symlinked root Claude may report the logical
///   spelling while the scan stores physical ones), must sit under a
///   project's `base_path`, and the row stores the canonical path.
/// - Remote (another host's token): the path is on THAT host, so the central
///   machine's `base_path`s say nothing about it. It must resolve to a known
///   project's owner/repo under that host's configured projects root and
///   layout (`HostPaths`, the matcher reconcile links remote sessions with).
///   The row is stored for that host only: rows are keyed (project, host,
///   name), so it never overwrites the local checkout's same-named row, and
///   the local project refresh never prunes it.
fn apply_worktree_hook(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    caller: &Caller,
) -> Result<(), IpcError> {
    let (path, branch) = worktree_fields(payload);
    let Some(path) = path else {
        return Ok(());
    };
    validate_worktree_path(&path)?;
    let branch = branch.as_deref().filter(|s| !s.is_empty());
    if let Some(b) = branch {
        crate::validate::git_ref(b).map_err(|e| IpcError::new("E_VALIDATE", e.message))?;
    }

    let host = caller_host(caller);
    if host != LOCAL_HOST {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let projects = s.list_projects().map_err(IpcError::from)?;
        let paths = HostPaths::for_host(&s, host);
        let Some(project_id) = crate::service::sessions::find_project_id_for_path(
            &projects,
            host,
            Path::new(&path),
            &paths,
        ) else {
            return Err(IpcError::new(
                "E_VALIDATE",
                format!("worktree_path {path} is not under a known project on host {host}"),
            ));
        };
        let name = Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed")
            .to_string();
        s.upsert_worktree_on(host, project_id, &name, &path, branch)
            .map_err(IpcError::from)?;
        return Ok(());
    }

    // Local: resolve symlinks (off-lock; it is filesystem IO), then validate
    // the physical form too, since that is what gets stored.
    let path = canonical_str(&path);
    validate_worktree_path(&path)?;
    let name = Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .to_string();
    let projects = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        s.list_projects().map_err(IpcError::from)?
    };
    let Some(project_id) = find_project_id_for_path(&projects, &path) else {
        return Err(IpcError::new(
            "E_VALIDATE",
            format!("worktree_path {path} is not under any known project base"),
        ));
    };
    let s = store.lock().map_err(|_| IpcError::lock())?;
    s.upsert_worktree(project_id, &name, &path, branch)
        .map_err(IpcError::from)?;
    Ok(())
}

/// The project whose `base_path` contains the LOCAL `worktree_path` (already
/// canonical), by whole components (`/home/u/proj` does not contain
/// `/home/u/project/...`); the longest base wins. The raw base_paths are
/// tried first; only when none contains the path are the bases
/// canonicalized (rows stored before the scan canonicalized hold the logical
/// spelling of a symlinked root), so the common case costs no syscalls.
fn find_project_id_for_path(projects: &[ProjectRow], worktree_path: &str) -> Option<i64> {
    let path = Path::new(worktree_path);
    let longest = |within: &dyn Fn(&ProjectRow) -> bool| {
        projects
            .iter()
            .filter(|p| within(p))
            .max_by_key(|p| p.base_path.len())
            .map(|p| p.id)
    };
    longest(&|p| is_within(path, Path::new(&p.base_path)))
        .or_else(|| longest(&|p| is_within(path, &canonical(Path::new(&p.base_path)))))
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
            transcript_path: None,
        }
    }

    #[test]
    fn stop_hook_on_unknown_session_is_noop() {
        let store = make_store();
        let payload = make_payload("Stop", "no-such-id");
        assert!(apply_hook(&store, &make_ssh(), &payload, &Caller::master()).is_ok());
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
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("Stop", "uuid-1"),
            &Caller::master(),
        )
        .unwrap();
        let s = store.lock().unwrap();
        let row = s.get_session("sess", "local").unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
    }

    #[test]
    fn stop_hook_from_another_hosts_token_is_forbidden() {
        // Host A's token must not be able to flip host B's session to idle
        // (which would also trigger B's safe-kill finalisation).
        let store = make_store();
        {
            let s = store.lock().unwrap();
            s.upsert_host("hostb").unwrap();
            let id = s
                .upsert_session("sess", "hostb", None, None, 0, 0, "running", None)
                .unwrap();
            s.set_claude_session_id(id, "uuid-b").unwrap();
            s.set_claude_status_by_session_id("uuid-b", "working")
                .unwrap();
        }
        let host_a = Caller {
            host_alias: Some("hosta".into()),
            mode: crate::mcp::TokenMode::Full,
        };
        let err = apply_hook(
            &store,
            &make_ssh(),
            &make_payload("Stop", "uuid-b"),
            &host_a,
        )
        .unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
        {
            let s = store.lock().unwrap();
            let row = s.get_session("sess", "hostb").unwrap().unwrap();
            assert_eq!(row.claude_status.as_deref(), Some("working"), "untouched");
        }
        // The session's own host token (and the master token) may.
        let host_b = Caller {
            host_alias: Some("hostb".into()),
            mode: crate::mcp::TokenMode::Readonly,
        };
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("Stop", "uuid-b"),
            &host_b,
        )
        .unwrap();
        let s = store.lock().unwrap();
        let row = s.get_session("sess", "hostb").unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
        // An unknown session stays a no-op for any caller.
        drop(s);
        assert!(apply_hook(&store, &make_ssh(), &make_payload("Stop", "nope"), &host_a).is_ok());
    }

    #[test]
    fn unknown_event_is_noop() {
        let store = make_store();
        let payload = make_payload("SessionStart", "s1");
        assert!(apply_hook(&store, &make_ssh(), &payload, &Caller::master()).is_ok());
    }

    #[test]
    fn stop_hook_bumps_turn_seq_and_stamps_last_stop_at() {
        let store = make_store();
        let id = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let id = s
                .upsert_session("sess", "local", None, None, 0, 0, "running", None)
                .unwrap();
            s.set_claude_session_id(id, "uuid-1").unwrap();
            id
        };
        for expected in 1..=3 {
            apply_hook(
                &store,
                &make_ssh(),
                &make_payload("Stop", "uuid-1"),
                &Caller::master(),
            )
            .unwrap();
            let s = store.lock().unwrap();
            let row = s.get_session_by_id(id).unwrap().unwrap();
            assert_eq!(row.turn_seq, expected);
            assert!(row.last_stop_at.is_some());
            assert_eq!(row.last_stop_at, row.last_turn_at);
            assert_eq!(row.claude_status.as_deref(), Some("idle"));
        }
    }

    #[test]
    fn user_prompt_submit_marks_the_session_working_and_is_host_checked() {
        let store = make_store();
        let id = {
            let s = store.lock().unwrap();
            s.upsert_host("hostb").unwrap();
            let id = s
                .upsert_session("sess", "hostb", None, None, 0, 0, "running", None)
                .unwrap();
            s.set_claude_session_id(id, "uuid-b").unwrap();
            s.set_claude_status_by_session_id("uuid-b", "idle").unwrap();
            id
        };
        let host_a = Caller {
            host_alias: Some("hosta".into()),
            mode: crate::mcp::TokenMode::Full,
        };
        let err = apply_hook(
            &store,
            &make_ssh(),
            &make_payload("UserPromptSubmit", "uuid-b"),
            &host_a,
        )
        .unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
        {
            let s = store.lock().unwrap();
            let row = s.get_session_by_id(id).unwrap().unwrap();
            assert_eq!(row.claude_status.as_deref(), Some("idle"), "untouched");
            assert!(row.idle_since.is_some());
        }
        let host_b = Caller {
            host_alias: Some("hostb".into()),
            mode: crate::mcp::TokenMode::Readonly,
        };
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("UserPromptSubmit", "uuid-b"),
            &host_b,
        )
        .unwrap();
        let s = store.lock().unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        assert_eq!(row.idle_since, None);
        // A submit does not count as a turn.
        assert_eq!(row.turn_seq, 0);
        drop(s);
        // Unknown session: no-op for any caller.
        assert!(apply_hook(
            &store,
            &make_ssh(),
            &make_payload("UserPromptSubmit", "nope"),
            &host_a
        )
        .is_ok());
    }

    #[test]
    fn worktree_hook_without_tool_input_is_noop() {
        let store = make_store();
        let payload = HookPayload {
            session_id: Some("s1".into()),
            hook_event_name: Some("PostToolUse".into()),
            tool_name: Some("EnterWorktree".into()),
            tool_input: None,
            tool_response: None,
            cwd: None,
            transcript_path: None,
        };
        assert!(apply_hook(&store, &make_ssh(), &payload, &Caller::master()).is_ok());
    }

    fn worktree_payload(path: &str, branch: Option<&str>) -> HookPayload {
        let mut response = serde_json::json!({ "worktreePath": path });
        if let Some(b) = branch {
            response["branch"] = serde_json::Value::String(b.into());
        }
        HookPayload {
            session_id: Some("s1".into()),
            hook_event_name: Some("PostToolUse".into()),
            tool_name: Some("EnterWorktree".into()),
            tool_input: Some(serde_json::json!({ "name": "feat" })),
            tool_response: Some(response),
            cwd: None,
            transcript_path: None,
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
            &Caller::master(),
        )
        .unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
        let err = apply_hook(
            &store,
            &make_ssh(),
            &worktree_payload("../../etc/passwd", None),
            &Caller::master(),
        )
        .unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
        // A branch that looks like a git option is refused too.
        let err = apply_hook(
            &store,
            &make_ssh(),
            &worktree_payload("/home/u/proj/.worktrees/feat", Some("--upload-pack=x")),
            &Caller::master(),
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
            &Caller::master(),
        )
        .unwrap();
        let s = store.lock().unwrap();
        let rows = s.list_worktrees_for_project(pid).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, canonical_str("/home/u/proj/.worktrees/feat"));
        assert_eq!(rows[0].branch.as_deref(), Some("feat"));
    }

    fn host_caller(host: &str) -> Caller {
        Caller {
            host_alias: Some(host.into()),
            mode: crate::mcp::TokenMode::Full,
        }
    }

    fn exit_payload(action: &str, path: Option<&str>) -> HookPayload {
        HookPayload {
            session_id: Some("s1".into()),
            hook_event_name: Some("PostToolUse".into()),
            tool_name: Some("ExitWorktree".into()),
            tool_input: Some(serde_json::json!({ "action": action })),
            tool_response: path.map(|p| serde_json::json!({ "worktreePath": p })),
            cwd: None,
            transcript_path: None,
        }
    }

    #[test]
    fn exit_worktree_remove_drops_only_the_callers_row() {
        let store = make_store();
        let wt = "/home/m/projects/github.com/o/r/.claude/worktrees/feat";
        let local_row = {
            let s = store.lock().unwrap();
            s.upsert_host("mefistos").unwrap();
            s.upsert_host("other").unwrap();
            let pid = s
                .upsert_project("o", "r", "/Users/me/projects/github.com/o/r")
                .unwrap();
            s.upsert_worktree_on("mefistos", pid, "feat", wt, None)
                .unwrap();
            // Another host reporting the same path, and the local row of the
            // same name: neither is the caller's.
            s.upsert_worktree_on("other", pid, "feat", wt, None)
                .unwrap();
            s.upsert_worktree(
                pid,
                "feat",
                "/Users/me/projects/github.com/o/r/.claude/worktrees/feat",
                None,
            )
            .unwrap()
        };
        let mef = host_caller("mefistos");
        let on = |host: &str| {
            store
                .lock()
                .unwrap()
                .list_worktrees_on_host(host)
                .unwrap()
                .len()
        };
        // `keep` leaves the row; a result without a path is a no-op.
        apply_hook(&store, &make_ssh(), &exit_payload("keep", Some(wt)), &mef).unwrap();
        apply_hook(&store, &make_ssh(), &exit_payload("remove", None), &mef).unwrap();
        assert_eq!(on("mefistos"), 1);
        // `remove` drops the caller's row, and only that one.
        apply_hook(&store, &make_ssh(), &exit_payload("remove", Some(wt)), &mef).unwrap();
        assert_eq!(on("mefistos"), 0);
        assert_eq!(on("other"), 1);
        assert!(store
            .lock()
            .unwrap()
            .get_worktree_row(local_row)
            .unwrap()
            .is_some());
    }

    #[test]
    fn exit_worktree_remove_matches_local_rows_canonically_and_validates() {
        let store = make_store();
        let gone = "/home/u/proj/.worktrees/gone";
        let id = {
            let s = store.lock().unwrap();
            let pid = s.upsert_project("o", "r", "/home/u/proj").unwrap();
            // Local rows hold the canonical path (the directory is removed,
            // so it resolves through its nearest existing ancestor).
            s.upsert_worktree(pid, "gone", &canonical_str(gone), None)
                .unwrap()
        };
        apply_hook(
            &store,
            &make_ssh(),
            &exit_payload("remove", Some(gone)),
            &Caller::master(),
        )
        .unwrap();
        assert!(store
            .lock()
            .unwrap()
            .get_worktree_row(id)
            .unwrap()
            .is_none());
        let err = apply_hook(
            &store,
            &make_ssh(),
            &exit_payload("remove", Some("../../etc")),
            &host_caller("mefistos"),
        )
        .unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
    }

    #[test]
    fn remote_worktree_hook_is_accepted_by_owner_repo_on_the_callers_host() {
        use crate::service::settings;
        let store = make_store();
        let pid = {
            let s = store.lock().unwrap();
            s.upsert_host("mefistos").unwrap();
            // The central (Mac) checkout: nothing like the remote path.
            s.upsert_project("o", "r", "/Users/me/projects/github.com/o/r")
                .unwrap()
        };
        let mef = host_caller("mefistos");
        // Default root on the host: accepted (was always 400 before).
        apply_hook(
            &store,
            &make_ssh(),
            &worktree_payload(
                "/home/m/projects/github.com/o/r/.claude/worktrees/feat",
                Some("feat"),
            ),
            &mef,
        )
        .unwrap();
        // Stored as a row of the CALLER's host, never a local one.
        {
            let s = store.lock().unwrap();
            assert!(s.list_worktrees_for_project(pid).unwrap().is_empty());
            let rows = s.list_worktrees_on_host("mefistos").unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].project_id, pid);
            assert_eq!(rows[0].name, "feat");
            assert_eq!(
                rows[0].path,
                "/home/m/projects/github.com/o/r/.claude/worktrees/feat"
            );
            assert_eq!(rows[0].branch.as_deref(), Some("feat"));
        }
        // An unknown repo on that host is still refused.
        let err = apply_hook(
            &store,
            &make_ssh(),
            &worktree_payload("/home/m/projects/github.com/o/other/.worktrees/f", None),
            &mef,
        )
        .unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
        // The host's own configured root + layout are honoured.
        {
            let s = store.lock().unwrap();
            settings::set(&s, settings::PROJECTS_BASE_PATH, r#"{"mefistos":"~/code"}"#).unwrap();
            settings::set(&s, settings::PROJECTS_LAYOUT, "flat").unwrap();
        }
        let custom = worktree_payload("/home/m/code/r/.worktrees/f", None);
        apply_hook(&store, &make_ssh(), &custom, &mef).unwrap();
        // The same path under the master token is judged against the LOCAL
        // bases, where it belongs to nothing.
        let err = apply_hook(&store, &make_ssh(), &custom, &Caller::master()).unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
        // A path that fails validation is refused before any matching.
        let err = apply_hook(
            &store,
            &make_ssh(),
            &worktree_payload("/home/m/code/r/../../etc", None),
            &mef,
        )
        .unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
    }

    #[cfg(unix)]
    #[test]
    fn local_worktree_hook_stores_the_canonical_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let real = tmp.path().join("mnt").join("o").join("r");
        std::fs::create_dir_all(real.join(".worktrees").join("feat")).unwrap();
        let link = tmp.path().join("projects");
        std::os::unix::fs::symlink(tmp.path().join("mnt"), &link).unwrap();
        let base = canonical(&real);
        let store = make_store();
        let pid = store
            .lock()
            .unwrap()
            .upsert_project("o", "r", &base.to_string_lossy())
            .unwrap();
        // Claude reports the logical spelling through the symlink.
        let logical = link.join("o").join("r").join(".worktrees").join("feat");
        apply_hook(
            &store,
            &make_ssh(),
            &worktree_payload(&logical.to_string_lossy(), Some("feat")),
            &host_caller("local"),
        )
        .unwrap();
        let s = store.lock().unwrap();
        let rows = s.list_worktrees_for_project(pid).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].path,
            base.join(".worktrees").join("feat").to_string_lossy()
        );
    }

    #[test]
    fn worktree_fields_read_response_then_input_and_legacy_name_still_routes() {
        let mut p = worktree_payload("/home/u/proj/.worktrees/feat", Some("feat"));
        assert_eq!(
            worktree_fields(&p),
            (
                Some("/home/u/proj/.worktrees/feat".into()),
                Some("feat".into())
            )
        );
        // snake_case keys in tool_input are the fallback.
        p.tool_response = None;
        p.tool_input = Some(serde_json::json!({
            "worktree_path": "/home/u/proj/.worktrees/x", "branch": "x"
        }));
        assert_eq!(
            worktree_fields(&p),
            (Some("/home/u/proj/.worktrees/x".into()), Some("x".into()))
        );
        // Nothing path-like → no-op, not an error.
        p.tool_input = Some(serde_json::json!({ "name": "feat" }));
        let store = make_store();
        assert!(apply_hook(&store, &make_ssh(), &p, &Caller::master()).is_ok());
        // A hand-posted legacy `WorktreeCreate` body is still validated.
        let mut legacy = worktree_payload("../../etc", None);
        legacy.tool_name = Some("WorktreeCreate".into());
        let err = apply_hook(&store, &make_ssh(), &legacy, &Caller::master()).unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
    }

    #[test]
    fn valid_transcript_path_requires_the_session_file_under_claude_projects() {
        let sid = "uuid-1";
        assert!(valid_transcript_path(
            "/home/u/.claude/projects/-home-u-p/uuid-1.jsonl",
            sid
        ));
        for bad in [
            "relative/.claude/projects/x/uuid-1.jsonl",
            "/home/u/.claude/projects/x/other.jsonl",
            "/home/u/.claude/projects/../../etc/uuid-1.jsonl",
            "/etc/uuid-1.jsonl",
            "/home/u/.claude/projects/x/uuid-1.jsonl\n",
        ] {
            assert!(!valid_transcript_path(bad, sid), "{bad}");
        }
    }

    #[test]
    fn hooks_store_a_valid_transcript_path_and_ignore_a_bad_one() {
        let store = make_store();
        let id = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let id = s
                .upsert_session("sess", "local", None, None, 0, 0, "running", None)
                .unwrap();
            s.set_claude_session_id(id, "uuid-1").unwrap();
            id
        };
        let mut p = make_payload("UserPromptSubmit", "uuid-1");
        p.transcript_path = Some("/etc/passwd".into());
        apply_hook(&store, &make_ssh(), &p, &Caller::master()).unwrap();
        assert_eq!(
            store.lock().unwrap().session_transcript_path(id).unwrap(),
            None
        );
        let good = "/home/u/.claude/projects/-home-u-p/uuid-1.jsonl";
        let mut p = make_payload("Stop", "uuid-1");
        p.transcript_path = Some(good.into());
        apply_hook(&store, &make_ssh(), &p, &Caller::master()).unwrap();
        let s = store.lock().unwrap();
        assert_eq!(
            s.session_transcript_path(id).unwrap().as_deref(),
            Some(good)
        );
        // The hook still counted as a turn.
        assert_eq!(s.get_session_by_id(id).unwrap().unwrap().turn_seq, 1);
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
