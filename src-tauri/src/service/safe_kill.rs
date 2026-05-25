//! Safe-kill flow: ask the running Claude session to persist its work safely
//! (commit + push to main, or open a PR) before fleet deletes its worktree
//! and tmux session.
//!
//! Lifecycle (column = `sessions.safe_kill_state`):
//!   NULL → "requested" → ("ready" → row gone) | "failed" → user resolves
//!
//! Detection runs on the Stop hook: when Claude finishes a turn, fleet
//! captures pane scrollback, finds the last line containing the per-request
//! nonce, and branches on whether the marker says READY or FAILED.

use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Markers Claude is asked to emit. The nonce is appended at request time so
/// the marker scan can distinguish the assistant's emission from the prompt
/// echo still visible in the pane.
const READY_PREFIX: &str = "SAFE_REMOVE_READY_";
const FAILED_PREFIX: &str = "SAFE_REMOVE_FAILED_";

/// Pane scrollback depth (lines) consulted when scanning for the marker. Big
/// enough to cover a multi-step push-and-PR turn without dragging in unrelated
/// history.
const MARKER_SCAN_LINES: u32 = 3000;

#[derive(Deserialize)]
pub struct SafeKillSessionArgs {
    pub host_alias: String,
    pub tmux_name: String,
}

/// A single uncommitted entry from `git status --porcelain`.
#[derive(Debug, Clone, Serialize)]
pub struct DirtyFile {
    /// Two-letter porcelain code (e.g. " M", "??", "AM"). Trimmed of trailing
    /// whitespace but preserves leading spaces — they encode index/worktree
    /// status separately.
    pub status: String,
    pub path: String,
}

/// Pre-flight snapshot consumed by the UI before deciding which remove path
/// to take. All git fields are `None` / empty when the session has no
/// worktree attached (e.g. orphan or in-repo session).
#[derive(Debug, Clone, Serialize)]
pub struct SafeKillInspection {
    pub has_worktree: bool,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    /// e.g. `"origin/feature"` — `None` if no upstream tracking branch is set.
    pub upstream: Option<String>,
    pub dirty_files: Vec<DirtyFile>,
    /// Commits in HEAD that are not on the upstream. `0` when fully pushed,
    /// `-1` when we couldn't determine (no upstream, or the branch isn't
    /// pushed at all).
    pub unpushed_commits: i32,
    /// True iff worktree is clean AND branch has upstream AND no unpushed
    /// commits. When true, the UI can skip the Claude prompt entirely and
    /// just remove + kill.
    pub safe_to_remove: bool,
    /// If we couldn't inspect (path missing, git error), this is the message
    /// to surface. Frontend should fall back to the Claude prompt flow when
    /// set.
    pub error: Option<String>,
}

#[derive(Deserialize)]
pub struct InspectSafeKillArgs {
    pub host_alias: String,
    pub tmux_name: String,
}

#[derive(Deserialize)]
pub struct DiscardKillSessionArgs {
    pub host_alias: String,
    pub tmux_name: String,
}

fn lock_err() -> IpcError {
    IpcError::new("E_LOCK", "store mutex poisoned")
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 8 random hex chars — enough entropy to make collision with arbitrary pane
/// text vanishingly unlikely while keeping the marker short for the prompt.
fn make_nonce() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 4];
    rand::rng().fill_bytes(&mut bytes);
    let mut s = String::with_capacity(8);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The literal prompt sent to the session. The two markers (nonce-tagged)
/// give fleet a deterministic signal regardless of how Claude phrases the
/// rest of its reply.
pub fn build_safe_kill_prompt(nonce: &str) -> String {
    format!(
        "I want to safely remove this session. Make sure ALL work in this \
         worktree is preserved: every change must be either committed and \
         pushed to `main`, or committed on a feature branch with an open PR. \
         When you are fully done:\n\
         \n\
         - If everything is safely persisted, end your reply with the \
         literal line:\n  {ready}{nonce}\n\
         - If you CANNOT safely persist (merge conflict, push rejected, \
         dirty state you can't resolve, etc.), end your reply with:\n  \
         {failed}{nonce}: <one-line reason>\n\
         \n\
         Do not emit either marker until you are fully done. Do not include \
         the markers in code blocks or quoted text.",
        ready = READY_PREFIX,
        failed = FAILED_PREFIX,
        nonce = nonce
    )
}

/// Kick off a safe-kill request: stamp the DB, send the marker-baked prompt
/// to the session. Returns the updated session row.
///
/// Refuses if the session is not alive (already ghost/dead) or if a request
/// is already in flight (`safe_kill_state == "requested"`). A prior "failed"
/// request can be retried freely.
pub async fn safe_kill_session(
    args: SafeKillSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name(&args.tmux_name)?;

    // Read + state-update happens under one lock; the SSH send afterwards is
    // off-lock.
    let (session_id, nonce) = {
        let s = store.lock().map_err(|_| lock_err())?;
        let row = s
            .get_session(&args.tmux_name, &args.host_alias)
            .map_err(IpcError::from)?
            .ok_or_else(|| {
                IpcError::new(
                    "E_NOTFOUND",
                    format!(
                        "session {} not found on {}",
                        args.tmux_name, args.host_alias
                    ),
                )
            })?;
        if row.status != "running" {
            return Err(IpcError::new(
                "E_NOT_ALIVE",
                format!("session {} is not running", args.tmux_name),
            ));
        }
        if row.safe_kill_state.as_deref() == Some("requested") {
            return Err(IpcError::new(
                "E_SAFE_KILL_IN_PROGRESS",
                "a safe-kill request is already in flight for this session",
            ));
        }
        let nonce = make_nonce();
        s.set_safe_kill_requested(row.id, &nonce, now_secs())
            .map_err(IpcError::from)?;
        // Best-effort timeline entry.
        let _ = s.insert_session_event(row.id, "safe_kill_requested", Some(&nonce));
        (row.id, nonce)
    };

    let prompt = build_safe_kill_prompt(&nonce);
    // Reuse the existing send-keys path. send_prompt validates inputs again
    // (harmless) and records a `prompt_sent` timeline entry.
    if let Err(e) = crate::service::sessions::send_prompt(
        crate::service::sessions::SendPromptArgs {
            host_alias: args.host_alias.clone(),
            tmux_name: args.tmux_name.clone(),
            prompt,
            submit: true,
        },
        store,
        ssh,
    )
    .await
    {
        // Roll back state so the user can retry; the worktree is untouched.
        if let Ok(s) = store.lock() {
            let _ = s.clear_safe_kill(session_id);
            let _ = s.insert_session_event(session_id, "safe_kill_send_failed", Some(&e.message));
        }
        return Err(e);
    }

    let s = store.lock().map_err(|_| lock_err())?;
    s.get_session_by_id(session_id)
        .map_err(IpcError::from)?
        .ok_or_else(|| IpcError::new("E_NOTFOUND", "session vanished after safe-kill request"))
}

/// Inspect a worktree-backed session so the UI can decide whether to even
/// involve Claude. Returns dirty files + ahead-of-upstream info from a single
/// SSH round trip.
pub async fn inspect_safe_kill(
    args: InspectSafeKillArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SafeKillInspection, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name(&args.tmux_name)?;

    let (worktree_path, _project_base) = {
        let s = store.lock().map_err(|_| lock_err())?;
        let row = s
            .get_session(&args.tmux_name, &args.host_alias)
            .map_err(IpcError::from)?
            .ok_or_else(|| {
                IpcError::new(
                    "E_NOTFOUND",
                    format!(
                        "session {} not found on {}",
                        args.tmux_name, args.host_alias
                    ),
                )
            })?;
        let wt = match row.worktree_id {
            Some(wid) => s.worktree_path(wid).map_err(IpcError::from)?,
            None => None,
        };
        let base = match row.project_id {
            Some(pid) => s.project_base_path(pid).map_err(IpcError::from)?,
            None => None,
        };
        (wt, base)
    };

    let Some(wt) = worktree_path else {
        // No worktree: nothing to inspect. UI will offer the Claude flow as
        // the only sensible option.
        return Ok(SafeKillInspection {
            has_worktree: false,
            worktree_path: None,
            branch: None,
            upstream: None,
            dirty_files: Vec::new(),
            unpushed_commits: -1,
            safe_to_remove: false,
            error: None,
        });
    };

    let wt_q = quote(&wt);
    // Single bash invocation prints three NUL-terminated sections:
    //   1. porcelain -z dirty list
    //   2. current branch name
    //   3. upstream (origin/<branch>) or empty
    //   4. ahead count (HEAD ahead of upstream) or "-1" when unknown
    // Separator `\x1e` (RS) is illegal in branch names and unlikely in paths.
    let script = format!(
        r#"set +e
wt={wt_q}
if [ ! -d "$wt/.git" ] && [ ! -f "$wt/.git" ]; then
  printf 'ERR\x1enot a git worktree: %s\n' "$wt" 1>&2
  exit 2
fi
porcelain=$(git -C "$wt" status --porcelain=v1 2>/dev/null)
branch=$(git -C "$wt" rev-parse --abbrev-ref HEAD 2>/dev/null)
upstream=$(git -C "$wt" rev-parse --abbrev-ref --symbolic-full-name '@{{u}}' 2>/dev/null)
if [ -n "$upstream" ]; then
  ahead=$(git -C "$wt" rev-list --count "$upstream"..HEAD 2>/dev/null)
  if [ -z "$ahead" ]; then ahead=-1; fi
else
  ahead=-1
fi
printf '%s\x1e%s\x1e%s\x1e%s' "$porcelain" "$branch" "$upstream" "$ahead"
"#
    );

    let out = run_shell(ssh, &args.host_alias, &script).await?;
    if !out.status.success() {
        return Ok(SafeKillInspection {
            has_worktree: true,
            worktree_path: Some(wt),
            branch: None,
            upstream: None,
            dirty_files: Vec::new(),
            unpushed_commits: -1,
            safe_to_remove: false,
            error: Some(format!(
                "git inspect failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
        });
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut parts = stdout.split('\x1e');
    let porcelain = parts.next().unwrap_or("");
    let branch_raw = parts.next().unwrap_or("").trim();
    let upstream_raw = parts.next().unwrap_or("").trim();
    let ahead_raw = parts.next().unwrap_or("-1").trim();

    let dirty_files = parse_porcelain(porcelain);
    let branch = (!branch_raw.is_empty()).then(|| branch_raw.to_string());
    let upstream = (!upstream_raw.is_empty()).then(|| upstream_raw.to_string());
    let unpushed_commits = ahead_raw.parse::<i32>().unwrap_or(-1);

    let safe_to_remove = dirty_files.is_empty() && upstream.is_some() && unpushed_commits == 0;

    Ok(SafeKillInspection {
        has_worktree: true,
        worktree_path: Some(wt),
        branch,
        upstream,
        dirty_files,
        unpushed_commits,
        safe_to_remove,
        error: None,
    })
}

/// Parse `git status --porcelain=v1` output. Each line is `XY␣path` where XY
/// are two status chars. We keep both chars intact (leading space matters).
pub fn parse_porcelain(s: &str) -> Vec<DirtyFile> {
    s.lines()
        .filter(|l| l.len() >= 3)
        .map(|l| {
            let (code, rest) = l.split_at(2);
            DirtyFile {
                status: code.to_string(),
                path: rest.trim_start().to_string(),
            }
        })
        .collect()
}

/// Skip the Claude prompt. Used when the UI's inspect call reported
/// `safe_to_remove == true` (clean + pushed) — or when the user explicitly
/// chose "discard & kill" after seeing dirty files / unpushed commits.
///
/// Force-removes the worktree on disk (the discard contract is "I accept
/// losing local-only work"), drops the worktree DB row, then kills the tmux
/// session. The `force_discard` flag controls whether `git worktree remove`
/// gets `--force`: callers that want clean+pushed semantics pass `false`
/// (and a non-clean worktree will surface as a hard error).
pub async fn discard_kill_session(
    args: DiscardKillSessionArgs,
    force_discard: bool,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<i64, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name(&args.tmux_name)?;

    let (session_id, worktree_id, worktree_path, project_base) = {
        let s = store.lock().map_err(|_| lock_err())?;
        let row = s
            .get_session(&args.tmux_name, &args.host_alias)
            .map_err(IpcError::from)?
            .ok_or_else(|| {
                IpcError::new(
                    "E_NOTFOUND",
                    format!(
                        "session {} not found on {}",
                        args.tmux_name, args.host_alias
                    ),
                )
            })?;
        let wt = match row.worktree_id {
            Some(wid) => s.worktree_path(wid).map_err(IpcError::from)?,
            None => None,
        };
        let base = match row.project_id {
            Some(pid) => s.project_base_path(pid).map_err(IpcError::from)?,
            None => None,
        };
        (row.id, row.worktree_id, wt, base)
    };

    if let (Some(ref wt), Some(ref base)) = (&worktree_path, &project_base) {
        let force_flag = if force_discard { " --force" } else { "" };
        let rm_cmd = format!(
            "git -C {} worktree remove{force_flag} {}",
            quote(base),
            quote(wt)
        );
        let out = run_shell(ssh, &args.host_alias, &rm_cmd).await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(IpcError::new(
                "E_WORKTREE_REMOVE",
                format!("git worktree remove failed: {}", stderr.trim()),
            ));
        }
    }

    if let Some(wid) = worktree_id {
        if let Ok(s) = store.lock() {
            if let Err(e) = s.delete_worktree(wid) {
                eprintln!("[safe_kill] discard: delete_worktree({wid}) failed: {e}");
            }
            let _ = s.insert_session_event(
                session_id,
                if force_discard {
                    "safe_kill_discarded"
                } else {
                    "safe_kill_ready"
                },
                None,
            );
        }
    }

    crate::service::sessions::kill_session(
        crate::service::sessions::KillSessionArgs {
            host_alias: args.host_alias,
            name: args.tmux_name,
            force: true,
        },
        store,
        ssh,
    )
    .await
}

/// Outcome of one marker scan. `NotYet` means the assistant has not yet
/// emitted the marker (only the prompt echo is visible) — we leave the
/// session in `requested` and wait for the next Stop hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerOutcome {
    Ready,
    Failed(String),
    NotYet,
}

/// Scan a pane capture for the nonce-tagged marker. We look at the LAST
/// occurrence: the prompt also contains the marker text, so the first hit is
/// almost certainly the echoed prompt. Returns `NotYet` when only one
/// occurrence is visible (i.e., just the prompt — assistant hasn't replied
/// yet).
pub fn scan_pane_for_marker(pane: &str, nonce: &str) -> MarkerOutcome {
    let ready_tag = format!("{READY_PREFIX}{nonce}");
    let failed_tag = format!("{FAILED_PREFIX}{nonce}");
    let mut hits: Vec<&str> = pane
        .lines()
        .filter(|l| l.contains(&ready_tag) || l.contains(&failed_tag))
        .collect();
    if hits.len() < 2 {
        // 0 = neither prompt nor reply visible; 1 = only the prompt echo.
        return MarkerOutcome::NotYet;
    }
    let last = hits.pop().unwrap();
    if let Some(idx) = last.find(&failed_tag) {
        let after = &last[idx + failed_tag.len()..];
        let reason = after
            .trim_start_matches(':')
            .trim()
            .chars()
            .take(200)
            .collect::<String>();
        let detail = if reason.is_empty() {
            "(no reason given)".to_string()
        } else {
            reason
        };
        MarkerOutcome::Failed(detail)
    } else if last.contains(&ready_tag) {
        MarkerOutcome::Ready
    } else {
        MarkerOutcome::NotYet
    }
}

/// Called from the Stop hook when `safe_kill_state == "requested"`. Captures
/// the session's pane, scans for the marker, and either:
///   - "requested" → "ready"  → runs dirty check + worktree remove + kill
///   - "requested" → "failed" → records reason, leaves session alive
///   - no marker yet → no change (next Stop will try again)
///
/// All errors are logged and swallowed (the hook handler already returned
/// 204 to Claude Code). This function is fire-and-forget from a tokio
/// background task.
pub async fn handle_stop_marker_check(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    claude_session_id: String,
) {
    if let Err(e) = handle_stop_marker_check_inner(&store, &ssh, &claude_session_id).await {
        eprintln!(
            "[safe_kill] marker check for claude_session_id={claude_session_id} failed: {}",
            e.message
        );
    }
}

async fn handle_stop_marker_check_inner(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    claude_session_id: &str,
) -> Result<(), IpcError> {
    // Snapshot what we need under one lock then drop it.
    let (session_id, tmux_name, host_alias, nonce, worktree_id, project_id) = {
        let s = store.lock().map_err(|_| lock_err())?;
        let row = match s.get_session_by_claude_id(claude_session_id)? {
            Some(r) => r,
            None => return Ok(()),
        };
        if row.safe_kill_state.as_deref() != Some("requested") {
            return Ok(());
        }
        let nonce = match row.safe_kill_nonce {
            Some(n) => n,
            None => return Ok(()),
        };
        (
            row.id,
            row.tmux_name,
            row.host_alias,
            nonce,
            row.worktree_id,
            row.project_id,
        )
    };

    let tmux = exec_for(&host_alias, ssh);
    let pane = match tmux
        .capture_pane_scrollback(&tmux_name, MARKER_SCAN_LINES)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "[safe_kill] capture_pane failed for {host_alias}/{tmux_name}: {}",
                e.message
            );
            return Ok(());
        }
    };

    match scan_pane_for_marker(&pane, &nonce) {
        MarkerOutcome::NotYet => Ok(()),
        MarkerOutcome::Failed(reason) => {
            if let Ok(s) = store.lock() {
                let _ = s.set_safe_kill_outcome(session_id, "failed", Some(&reason));
                let _ = s.insert_session_event(session_id, "safe_kill_failed", Some(&reason));
            }
            Ok(())
        }
        MarkerOutcome::Ready => {
            finalize_safe_kill(
                store,
                ssh,
                session_id,
                &tmux_name,
                &host_alias,
                worktree_id,
                project_id,
            )
            .await
        }
    }
}

/// Once Claude reports READY: run our own dirty check (`git status
/// --porcelain`), then `git worktree remove`, then kill the tmux session and
/// drop the worktree DB row. Any failure short-circuits to `safe_kill_state =
/// "failed"` with the failing stderr as the detail — never silently lose
/// work.
async fn finalize_safe_kill(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    session_id: i64,
    tmux_name: &str,
    host_alias: &str,
    worktree_id: Option<i64>,
    _project_id: Option<i64>,
) -> Result<(), IpcError> {
    // Resolve paths under a brief lock.
    let (worktree_path, project_base): (Option<String>, Option<String>) = {
        let s = store.lock().map_err(|_| lock_err())?;
        let wt_path = match worktree_id {
            Some(wid) => s.worktree_path(wid).map_err(IpcError::from)?,
            None => None,
        };
        let proj_base = match _project_id {
            Some(pid) => s.project_base_path(pid).map_err(IpcError::from)?,
            None => None,
        };
        (wt_path, proj_base)
    };

    // Belt + suspenders: even after READY, double-check the worktree is clean.
    // `git status --porcelain` on a non-existent path errors out, which we
    // treat as failure too.
    if let Some(ref wt) = worktree_path {
        let dirty_cmd = format!("git -C {} status --porcelain", quote(wt));
        let out = run_shell(ssh, host_alias, &dirty_cmd).await?;
        if !out.status.success() {
            let detail = format!(
                "dirty check failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            mark_failed(store, session_id, &detail);
            return Ok(());
        }
        let porcelain = String::from_utf8_lossy(&out.stdout);
        if !porcelain.trim().is_empty() {
            let detail = format!(
                "worktree still dirty after READY: {} uncommitted entr{}",
                porcelain.lines().count(),
                if porcelain.lines().count() == 1 {
                    "y"
                } else {
                    "ies"
                }
            );
            mark_failed(store, session_id, &detail);
            return Ok(());
        }
    }

    // Remove the git worktree on the remote (no --force; we just verified
    // clean). We run from the project base so git resolves the worktree by
    // path regardless of which checkout the user invoked from.
    if let (Some(ref wt), Some(ref base)) = (&worktree_path, &project_base) {
        let rm_cmd = format!("git -C {} worktree remove {}", quote(base), quote(wt));
        let out = run_shell(ssh, host_alias, &rm_cmd).await?;
        if !out.status.success() {
            let detail = format!(
                "git worktree remove failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            mark_failed(store, session_id, &detail);
            return Ok(());
        }
    }

    // Drop the worktree DB row + kill the tmux session. Both are best-effort
    // beyond this point — the git worktree is already gone on disk; we want
    // the DB and tmux to converge to the same reality.
    if let Some(wid) = worktree_id {
        if let Ok(s) = store.lock() {
            if let Err(e) = s.delete_worktree(wid) {
                eprintln!("[safe_kill] delete_worktree({wid}) failed: {e}");
            }
        }
    }
    // Mark ready BEFORE the kill so the row event reaches the frontend; the
    // reconcile after kill_session will turn the row into a ghost.
    if let Ok(s) = store.lock() {
        let _ = s.set_safe_kill_outcome(session_id, "ready", None);
        let _ = s.insert_session_event(session_id, "safe_kill_ready", None);
    }

    if let Err(e) = crate::service::sessions::kill_session(
        crate::service::sessions::KillSessionArgs {
            host_alias: host_alias.to_string(),
            name: tmux_name.to_string(),
            force: true,
        },
        store,
        ssh,
    )
    .await
    {
        eprintln!(
            "[safe_kill] tmux kill after READY failed for {host_alias}/{tmux_name}: {}",
            e.message
        );
    }
    Ok(())
}

fn mark_failed(store: &Arc<Mutex<Store>>, session_id: i64, detail: &str) {
    if let Ok(s) = store.lock() {
        let _ = s.set_safe_kill_outcome(session_id, "failed", Some(detail));
        let _ = s.insert_session_event(session_id, "safe_kill_failed", Some(detail));
    }
}

/// Run a bash command on `host_alias` (local or remote) and return the
/// captured output. Centralizes the local-vs-SSH branch.
async fn run_shell(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    script: &str,
) -> Result<std::process::Output, IpcError> {
    if host_alias == "local" {
        tokio::process::Command::new("bash")
            .args(["-lc", script])
            .output()
            .await
            .map_err(|e| IpcError::new("E_SHELL", format!("spawn bash: {e}")))
    } else {
        ssh.run(
            host_alias,
            &["bash", "-lc", &quote(script)],
            std::time::Duration::from_secs(30),
        )
        .await
    }
}

fn exec_for(host: &str, ssh: &Arc<SshClient>) -> Box<dyn crate::tmux::TmuxExec> {
    if host == "local" {
        Box::new(crate::tmux::LocalTmux)
    } else {
        Box::new(crate::tmux::RemoteTmux {
            client: Arc::clone(ssh),
            host: host.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_is_8_hex_chars() {
        let n = make_nonce();
        assert_eq!(n.len(), 8);
        assert!(n.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn prompt_contains_both_markers_with_nonce() {
        let p = build_safe_kill_prompt("deadbeef");
        assert!(p.contains("SAFE_REMOVE_READY_deadbeef"));
        assert!(p.contains("SAFE_REMOVE_FAILED_deadbeef"));
    }

    #[test]
    fn scan_returns_not_yet_when_only_prompt_echo_visible() {
        let pane = "...
> When you are fully done emit SAFE_REMOVE_READY_abc123 or SAFE_REMOVE_FAILED_abc123: <reason>
...";
        assert_eq!(scan_pane_for_marker(pane, "abc123"), MarkerOutcome::NotYet);
    }

    #[test]
    fn scan_returns_ready_when_assistant_echoes_after_prompt() {
        // Two lines containing the nonce: first is the prompt echo, second
        // is the assistant's READY emission.
        let pane = "user: emit SAFE_REMOVE_READY_abc123 when done
assistant: pushed everything; main is up to date.
SAFE_REMOVE_READY_abc123";
        assert_eq!(scan_pane_for_marker(pane, "abc123"), MarkerOutcome::Ready);
    }

    #[test]
    fn scan_extracts_failed_reason() {
        let pane = "prompt: ... SAFE_REMOVE_READY_xyz or SAFE_REMOVE_FAILED_xyz: <reason>
assistant: I cannot push.
SAFE_REMOVE_FAILED_xyz: push rejected: non-fast-forward";
        match scan_pane_for_marker(pane, "xyz") {
            MarkerOutcome::Failed(r) => {
                assert!(r.contains("non-fast-forward"), "reason was: {r}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn scan_picks_last_marker_when_both_appear() {
        // Two real emissions (rare but possible) — last one wins, which is
        // also the most recent thing Claude said.
        let pane = "prompt
SAFE_REMOVE_FAILED_n1: tried once
... retried successfully ...
SAFE_REMOVE_READY_n1";
        assert_eq!(scan_pane_for_marker(pane, "n1"), MarkerOutcome::Ready);
    }

    #[test]
    fn scan_returns_not_yet_when_no_marker_at_all() {
        let pane = "nothing relevant here";
        assert_eq!(scan_pane_for_marker(pane, "n1"), MarkerOutcome::NotYet);
    }
}
