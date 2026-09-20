//! Store mutations triggered by Claude Code HTTP hook events.
//!
//! Called from `mcp::hooks::handle_hook` after token auth passes.

use crate::ipc_error::lock;
use crate::ipc_error::{codes, IpcError};
use crate::mcp::hooks::HookPayload;
use crate::mcp::Caller;
use crate::projects::path_identity::{canonical, canonical_str, is_within};
use crate::service::pane_intel::{ClaudeStatus, StuckKind};
use crate::service::projects::LOCAL_HOST;
use crate::service::sessions::HostPaths;
use crate::ssh::SshClient;
use crate::store::{ProjectRow, SessionRow, StartSource, Store};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Who sent a hook and from which tmux pane.
pub struct HookContext<'a> {
    pub caller: &'a Caller,
    /// `X-Fleet-Pane` (validated `%N`), `None` outside tmux / old CLIs.
    pub pane_id: Option<String>,
}

/// Dispatch a hook event to the appropriate handler. `ctx.caller` is the
/// identity behind the request's bearer token; a per-host caller may only
/// report about sessions on its own host. Unknown events are silently
/// ignored.
pub fn apply_hook(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    payload: &HookPayload,
    ctx: &HookContext,
) -> Result<(), IpcError> {
    match payload.hook_event_name.as_deref() {
        Some("Stop") => apply_stop_hook(store, ssh, payload, ctx),
        Some("UserPromptSubmit") => apply_prompt_submit_hook(store, payload, ctx),
        Some("SessionStart") => apply_session_start_hook(store, payload, ctx),
        Some("PreCompact") => apply_pre_compact_hook(store, payload, ctx),
        Some("PostCompact") => apply_post_compact_hook(store, payload, ctx),
        Some("SessionEnd") => apply_session_end_hook(store, payload, ctx),
        Some("StopFailure") => apply_stop_failure_hook(store, ssh, payload, ctx),
        Some("Notification") => apply_notification_hook(store, payload, ctx),
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
            apply_worktree_hook(store, payload, ctx.caller)
        }
        // `ExitWorktree { action: "remove" }` deleted the worktree: drop its
        // row on the caller's host. The `WorktreeRemove` hook EVENT is never
        // installed: removal fails when its hook leaves the directory behind,
        // so fleet cannot be (or sit beside) that hook.
        Some("PostToolUse") if payload.tool_name.as_deref() == Some("ExitWorktree") => {
            apply_worktree_exit_hook(store, payload, ctx.caller)
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

/// The payload's transcript path when it validates for `claude_session_id`.
fn payload_transcript_path<'p>(
    payload: &'p HookPayload,
    claude_session_id: &str,
) -> Option<&'p str> {
    payload
        .transcript_path
        .as_deref()
        .filter(|p| valid_transcript_path(p, claude_session_id))
}

/// Store the hook's transcript path when it validates: on the row (while the
/// id is still its current conversation) and on that conversation's row.
fn remember_transcript_path(
    s: &Store,
    row_id: i64,
    payload: &HookPayload,
    claude_session_id: &str,
) {
    if let Some(p) = payload_transcript_path(payload, claude_session_id) {
        let _ = s.set_transcript_path_for_row(row_id, claude_session_id, p);
        let _ = s.set_conversation_transcript_path(row_id, claude_session_id, p);
    }
}

/// A short identifier-like value from a hook body (`reason`, `trigger`,
/// `model`) that ends up in a column or timeline detail: printable ASCII
/// from a small alphabet, at most 128 chars. Anything else is dropped.
fn hook_token(v: Option<&str>) -> Option<&str> {
    v.map(str::trim).filter(|v| {
        !v.is_empty()
            && v.len() <= 128
            && v.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '[' | ']'))
    })
}

/// Look up the session a hook is about by `claude_session_id` and apply the
/// caller's host binding: a host token may only flip sessions on ITS host —
/// host A's token must not be able to mark host B's session idle (and so
/// trigger B's safe-kill finalisation or complete B's tasks). Unknown
/// session → `None` (the hook arrived before reconcile enriched the row; a
/// no-op, as before). An id shared by more than one row → `None` too: the
/// id cannot say which row the hook is about, so only the pane step may
/// resolve such a row.
fn host_checked_row(
    s: &Store,
    claude_session_id: &str,
    caller: &Caller,
) -> Result<Option<SessionRow>, IpcError> {
    let mut rows = s.sessions_by_claude_id(claude_session_id)?;
    if rows.len() != 1 {
        return Ok(None);
    }
    let row = rows.pop();
    if let (Some(row), Some(h)) = (&row, &caller.host_alias) {
        if &row.host_alias != h {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!(
                    "session {} is on host {}; this token is bound to {h}",
                    row.tmux_name, row.host_alias
                ),
            ));
        }
    }
    Ok(row)
}

/// Events that may move a row onto a new conversation id. Everything else
/// carrying a non-current id only updates the conversation it names
/// (spec: "/clear mid-turn"). `SessionStart(compact)` keeps its id, so a
/// late one from a replaced conversation must not rebind back either.
fn may_rebind(payload: &HookPayload) -> bool {
    match payload.hook_event_name.as_deref() {
        Some("UserPromptSubmit") => true,
        Some("SessionStart") => {
            StartSource::from_hook(payload.source.as_deref().unwrap_or("")) != StartSource::Compact
        }
        _ => false,
    }
}

/// Find the row a hook is about (spec §1.2), in order:
///
/// 1. The caller's host + `ctx.pane_id`, when exactly one live row there last
///    showed that pane (whether it may move to a new id: [`rebind_eligible`]).
/// 2. `claude_session_id = payload.session_id`, host-checked
///    ([`host_checked_row`]; abstains when two rows share the id).
/// 3. Rebinding events from host callers only (never the master token):
///    the rows on the caller's host awaiting a rebind (`SessionEnd(clear |
///    resume)` within the TTL) whose cwd agrees — `payload.cwd` is absent,
///    or the row's known cwd (worktree path, else project base path) is
///    absent, or both are equal after `canonical_str`. It matches when
///    exactly one such row exists, and never when some row already holds
///    `payload.session_id`.
///
/// Otherwise `None`: the hook is a no-op. The step that matched is returned
/// with the row: a pane match says only which pane sent the hook, not that
/// the payload's conversation is the row's (see [`rebind_eligible`]).
fn resolve_hook_row(
    s: &Store,
    payload: &HookPayload,
    ctx: &HookContext,
    may_rebind: bool,
) -> Result<Option<(SessionRow, ResolvedBy)>, IpcError> {
    if let (Some(host), Some(pane)) = (&ctx.caller.host_alias, &ctx.pane_id) {
        if let Some(row) = s.find_session_by_pane(host, pane)? {
            return Ok(Some((row, ResolvedBy::Pane)));
        }
    }
    let Some(id) = payload.session_id.as_deref() else {
        return Ok(None);
    };
    if let Some(row) = host_checked_row(s, id, ctx.caller)? {
        return Ok(Some((row, ResolvedBy::Id)));
    }
    let Some(host) = ctx.caller.host_alias.as_deref() else {
        return Ok(None);
    };
    if !may_rebind {
        return Ok(None);
    }
    // Step 2 abstained because several rows hold the id: never add a
    // third holder through the awaiting mark.
    if !s.sessions_by_claude_id(id)?.is_empty() {
        return Ok(None);
    }
    let matching: Vec<SessionRow> = s
        .sessions_awaiting_rebind(host)?
        .into_iter()
        .filter(|r| match (payload.cwd.as_deref(), row_cwd(s, r)) {
            (Some(a), Some(b)) => canonical_str(a) == canonical_str(&b),
            _ => true,
        })
        .collect();
    Ok(if matching.len() == 1 {
        matching
            .into_iter()
            .next()
            .map(|r| (r, ResolvedBy::Awaiting))
    } else {
        None
    })
}

/// Which step of [`resolve_hook_row`] found the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedBy {
    Pane,
    Id,
    Awaiting,
}

/// May a hook that reached `row` through its PANE move it onto a new id?
/// Any `claude` started in the pane — a Bash-tool `claude -p` included —
/// inherits `$TMUX_PANE`, so the pane alone does not prove the payload's
/// conversation replaced the row's. It did when:
///
/// (a) the row has no id yet;
/// (b) the row awaits a rebind (`SessionEnd(clear | resume)` within the TTL);
/// (c) the row's current conversation has ended, or its Claude is `stopped`;
/// (d) the event is `SessionStart(clear)` — only the interactive session in
///     the pane emits it. Not `resume`: a nested `claude -p --resume <id>` /
///     `-c` starts with it too; an interactive `/resume` rebinds via (b),
///     since its SessionEnd(resume) sets the awaiting mark first.
fn rebind_eligible(s: &Store, row: &SessionRow, payload: &HookPayload) -> Result<bool, IpcError> {
    let Some(current) = row.claude_session_id.as_deref() else {
        return Ok(true);
    };
    if s.is_awaiting_rebind(row.id)?
        || row.claude_status.as_deref() == Some(ClaudeStatus::Stopped.as_str())
        || s.get_conversation(row.id, current)?
            .is_some_and(|c| c.ended_at.is_some())
    {
        return Ok(true);
    }
    Ok(payload.hook_event_name.as_deref() == Some("SessionStart")
        && StartSource::from_hook(payload.source.as_deref().unwrap_or("")) == StartSource::Clear)
}

/// The source of a UserPromptSubmit rebind onto a row awaiting one: the
/// SessionStart that names it may still be in flight (it is async), so
/// the just-closed conversation's end reason says how this one began.
fn prompt_rebind_source(s: &Store, row: &SessionRow) -> Result<StartSource, IpcError> {
    if !s.is_awaiting_rebind(row.id)? {
        return Ok(StartSource::Unknown);
    }
    let Some(current) = row.claude_session_id.as_deref() else {
        return Ok(StartSource::Unknown);
    };
    Ok(
        match s
            .get_conversation(row.id, current)?
            .and_then(|c| c.end_reason)
            .as_deref()
        {
            Some("clear") => StartSource::Clear,
            Some("resume") => StartSource::Resume,
            _ => StartSource::Unknown,
        },
    )
}

/// The row's known cwd on its own host: its worktree path when that
/// worktree row belongs to the same host, else (local rows only) its
/// project's base path. A remote row's project base path is the LOCAL
/// checkout, which says nothing about the remote cwd, so it counts as
/// unknown.
fn row_cwd(s: &Store, row: &SessionRow) -> Option<String> {
    let wt = row
        .worktree_id
        .and_then(|w| s.get_worktree_row(w).ok().flatten())
        .filter(|w| w.host_alias == row.host_alias)
        .map(|w| w.path);
    wt.or_else(|| {
        (row.host_alias == LOCAL_HOST)
            .then_some(row.project_id)
            .flatten()
            .and_then(|p| s.project_base_path(p).ok().flatten())
    })
}

/// How a resolved row relates to the payload's conversation id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    /// The payload's id already was the row's current conversation.
    Current,
    /// The row was just moved onto the payload's id (now current).
    Rebound,
    /// The payload names a conversation the row has since left.
    Stale,
}

/// Resolve, then — for a rebinding event whose id differs from the row's —
/// move the row onto the payload's conversation (timeline
/// `conversation_started`). Returns the (possibly rebound) row and how it
/// relates to the payload's id.
///
/// A row found by its pane moves only when [`rebind_eligible`]. Otherwise a
/// non-current id is the row's own earlier conversation (`Stale`) when the
/// row has one by that id, and else a foreign `claude` sharing the pane
/// (a nested `claude -p`): `None`, a no-op that never touches the row.
///
/// A UserPromptSubmit rebind takes its source from [`prompt_rebind_source`]
/// and keeps the prompt that started the turn.
fn resolve_and_rebind(
    s: &Store,
    payload: &HookPayload,
    ctx: &HookContext,
    source: StartSource,
) -> Result<Option<(SessionRow, Binding)>, IpcError> {
    let Some(id) = payload.session_id.as_deref() else {
        return Ok(None);
    };
    let rebind_ok = may_rebind(payload);
    let Some((row, by)) = resolve_hook_row(s, payload, ctx, rebind_ok)? else {
        return Ok(None);
    };
    if row.claude_session_id.as_deref() == Some(id) {
        return Ok(Some((row, Binding::Current)));
    }
    let movable = rebind_ok && (by != ResolvedBy::Pane || rebind_eligible(s, &row, payload)?);
    if !movable {
        if by == ResolvedBy::Pane && s.get_conversation(row.id, id)?.is_none() {
            return Ok(None);
        }
        return Ok(Some((row, Binding::Stale)));
    }
    crate::validate::claude_session_id(id)
        .map_err(|e| IpcError::new(codes::E_VALIDATE, e.message))?;
    let prompt = payload.hook_event_name.as_deref() == Some("UserPromptSubmit");
    let source = if prompt {
        prompt_rebind_source(s, &row)?
    } else {
        source
    };
    let rebound = s
        .rebind_conversation_opts(
            row.id,
            id,
            source,
            payload_transcript_path(payload, id),
            hook_token(payload.model.as_deref()),
            prompt,
        )?
        .unwrap_or(row);
    // The hook now owns this binding: a reconcile pass already in flight
    // must not write the replaced id back (the upsert's in-flight guard).
    s.record_hook_seen(rebound.id)?;
    best_effort_event_for(
        s,
        rebound.id,
        Some(id),
        "conversation_started",
        Some(source.as_str()),
    );
    Ok(Some((rebound, Binding::Rebound)))
}

/// Spawn the post-turn context refresh (spec §1.5) off the hook's response
/// path. Best-effort: skipped when no runtime is reachable (sync tests).
fn spawn_refresh_context(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>, row_id: i64) {
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    let _ = crate::rt::try_spawn(async move {
        crate::service::context::refresh_context(store, ssh, row_id).await;
    });
}

/// The SessionStart hook (command hook, spec §1.1): opens / reopens a
/// conversation. A new id rebinds the row; the row's own id (a fleet-created
/// session starting with its `--session-id`, or `/resume` back to the
/// current conversation) re-runs the rebind so the conversation is open and
/// the source's resets apply. `compact` keeps the id and records a
/// compaction instead.
fn apply_session_start_hook(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    ctx: &HookContext,
) -> Result<(), IpcError> {
    let source = StartSource::from_hook(payload.source.as_deref().unwrap_or(""));
    let s = lock(store)?;
    let Some((row, binding)) = resolve_and_rebind(&s, payload, ctx, source)? else {
        return Ok(());
    };
    let id = payload.session_id.as_deref().unwrap_or_default();
    if source == StartSource::Compact {
        if s.conversation_record_compaction(row.id, id)? {
            best_effort_event_for(
                &s,
                row.id,
                Some(id),
                "compact_done",
                compact_trigger(payload),
            );
        }
        return Ok(());
    }
    if binding == Binding::Current {
        // A turn already began on this conversation (its UserPromptSubmit
        // won the race and rebound the row): turns and first_prompt are
        // written only by the conversation's own hooks, never at a genuine
        // start, so either one proves this SessionStart is late.
        let turn_started = s
            .get_conversation(row.id, id)?
            .is_some_and(|c| c.turns > 0 || c.first_prompt.is_some());
        s.rebind_conversation_opts(
            row.id,
            id,
            source,
            payload_transcript_path(payload, id),
            hook_token(payload.model.as_deref()),
            turn_started,
        )?;
        best_effort_event_for(
            &s,
            row.id,
            Some(id),
            "conversation_started",
            Some(source.as_str()),
        );
    }
    Ok(())
}

/// `trigger` of a compaction hook: `manual` | `auto`, anything else dropped.
fn compact_trigger(payload: &HookPayload) -> Option<&str> {
    payload
        .trigger
        .as_deref()
        .filter(|t| matches!(*t, "manual" | "auto"))
}

/// The PreCompact hook: a compaction is starting. Sets `current_activity =
/// compacting` (cleared by PostCompact or the next turn boundary) and
/// records `compact_started`. A non-current id is a no-op.
fn apply_pre_compact_hook(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    ctx: &HookContext,
) -> Result<(), IpcError> {
    let s = lock(store)?;
    let Some((row, Binding::Current)) = resolve_and_rebind(&s, payload, ctx, StartSource::Unknown)?
    else {
        return Ok(());
    };
    s.set_current_activity(row.id, Some("compacting"))?;
    best_effort_event_for(
        &s,
        row.id,
        payload.session_id.as_deref(),
        "compact_started",
        compact_trigger(payload),
    );
    Ok(())
}

/// The PostCompact hook: counts the compaction on the conversation it names
/// (deduped against `SessionStart(compact)`, which also fires), marks the
/// context stale when that is the current conversation, ends the
/// `compacting` activity and records `compact_done`.
fn apply_post_compact_hook(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    ctx: &HookContext,
) -> Result<(), IpcError> {
    let s = lock(store)?;
    let Some((row, binding)) = resolve_and_rebind(&s, payload, ctx, StartSource::Unknown)? else {
        return Ok(());
    };
    let id = payload.session_id.as_deref().unwrap_or_default();
    if binding == Binding::Current && row.current_activity.as_deref() == Some("compacting") {
        s.set_current_activity(row.id, None)?;
    }
    if s.conversation_record_compaction(row.id, id)? {
        best_effort_event_for(
            &s,
            row.id,
            Some(id),
            "compact_done",
            compact_trigger(payload),
        );
    }
    Ok(())
}

/// The Stop hook: a turn just completed. Counts the turn on the conversation
/// the payload names; when that is the row's current conversation it also
/// marks the session `idle`, bumps `turn_seq` and stamps `last_stop_at` (the
/// completion signal `send_prompt` / `wait_for_session` / `run_prompt` build
/// on), records `turn_done`, then kicks off the background follow-ups: the
/// safe-kill marker scan, the task-completion marker scan and the context
/// refresh. All are spawned so the HTTP response returns fast. A Stop from a
/// conversation the row has left (`/clear` mid-turn) only counts the turn.
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
    ctx: &HookContext,
) -> Result<(), IpcError> {
    let session_id = match &payload.session_id {
        Some(id) => id.clone(),
        None => return Ok(()),
    };
    // Snapshot whether a safe-kill / open task is in flight BEFORE we update
    // status; the follow-ups (pane capture + SSH) run off the hook handler.
    let (row_id, safe_kill_in_flight, task_worker) = {
        let s = lock(store)?;
        let Some((before, binding)) = resolve_and_rebind(&s, payload, ctx, StartSource::Unknown)?
        else {
            return Ok(());
        };
        s.conversation_bump_turns(before.id, &session_id)?;
        if binding != Binding::Current {
            return Ok(());
        }
        let in_flight = before.safe_kill_state.as_deref() == Some("requested");
        remember_transcript_path(&s, before.id, payload, &session_id);
        let after = s.record_stop_hook_for_row(before.id)?;
        let detail: Option<String> = payload
            .last_assistant_message
            .as_deref()
            .map(|m| m.trim().chars().take(200).collect::<String>())
            .filter(|d| !d.is_empty());
        best_effort_event_for(
            &s,
            before.id,
            Some(&session_id),
            "turn_done",
            detail.as_deref(),
        );
        let has_open_tasks = s
            .open_tasks_for_worker(before.id)
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        (before.id, in_flight, after.filter(|_| has_open_tasks))
    };
    if safe_kill_in_flight {
        let store = Arc::clone(store);
        let ssh = Arc::clone(ssh);
        crate::rt::spawn(async move {
            crate::service::safe_kill::handle_stop_marker_check(store, ssh, row_id).await;
        });
    }
    if let Some(worker) = task_worker {
        let store = Arc::clone(store);
        let ssh = Arc::clone(ssh);
        let cwd = payload.cwd.clone();
        crate::rt::spawn(async move {
            crate::service::tasks::handle_stop_for_worker(store, ssh, worker, cwd).await;
        });
    }
    spawn_refresh_context(store, ssh, row_id);
    Ok(())
}

/// The UserPromptSubmit hook: a turn is starting. Marks the session
/// `working` so an idle-looking pane between the submit and the first
/// spinner frame is not mistaken for "still idle" — and so `wait_for_session
/// { until: "idle" }` after a `send_prompt` does not return before the turn
/// even begins. A new id rebinds the row first (this covers hosts where the
/// SessionStart hook is missing, and a SessionStart still in flight): source
/// `clear` / `resume` after the matching SessionEnd, else `unknown`. The
/// prompt's first 200 chars become the conversation's `first_prompt` (never
/// logged).
fn apply_prompt_submit_hook(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    ctx: &HookContext,
) -> Result<(), IpcError> {
    let Some(session_id) = payload.session_id.as_deref() else {
        return Ok(());
    };
    let s = lock(store)?;
    let Some((row, binding)) = resolve_and_rebind(&s, payload, ctx, StartSource::Unknown)? else {
        return Ok(());
    };
    match binding {
        Binding::Stale => return Ok(()),
        Binding::Current => remember_transcript_path(&s, row.id, payload, session_id),
        Binding::Rebound => {}
    }
    s.record_prompt_submit_hook_for_row(row.id)?;
    if let Some(p) = payload.prompt.as_deref().filter(|p| !p.trim().is_empty()) {
        s.conversation_set_first_prompt(row.id, session_id, p)?;
    }
    Ok(())
}

/// The SessionEnd hook: a conversation ended. The conversation is closed
/// with the reason in every case.
///
/// - `clear` / `resume`: the process lives on under a new id, so the row's
///   status is untouched; the row is marked awaiting a rebind (resolution
///   step 3) and `conversation_ended` is recorded.
/// - Any other reason: the Claude process exited — the row goes `stopped`
///   and `session_end` is recorded with the reason.
///
/// A SessionEnd for a conversation the row has already left only closes
/// that conversation. A reason that is not a short token is read as
/// `other`.
fn apply_session_end_hook(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    ctx: &HookContext,
) -> Result<(), IpcError> {
    let (Some(session_id), Some(reason)) = (&payload.session_id, payload.reason.as_deref()) else {
        return Ok(());
    };
    let reason = hook_token(Some(reason)).unwrap_or("other");
    let s = lock(store)?;
    let Some((row, binding)) = resolve_and_rebind(&s, payload, ctx, StartSource::Unknown)? else {
        return Ok(());
    };
    s.close_conversation(row.id, session_id, reason)?;
    if binding != Binding::Current {
        best_effort_event_for(
            &s,
            row.id,
            Some(session_id),
            "conversation_ended",
            Some(reason),
        );
        return Ok(());
    }
    remember_transcript_path(&s, row.id, payload, session_id);
    if matches!(reason, "clear" | "resume") {
        s.mark_awaiting_rebind(row.id)?;
        best_effort_event_for(
            &s,
            row.id,
            Some(session_id),
            "conversation_ended",
            Some(reason),
        );
    } else if let Some(row) = s.record_session_end_hook_for_row(row.id)? {
        best_effort_event_for(&s, row.id, Some(session_id), "session_end", Some(reason));
    }
    Ok(())
}

/// The StopFailure hook: the turn ended in an API error (rate limit, auth,
/// overloaded, …). Counts the turn on the conversation it names; for the
/// current conversation it ends the turn exactly like `Stop` — waiters
/// return and read the error from the transcript — records `stop_failure`
/// with the error type (and detail when present) and refreshes the context.
fn apply_stop_failure_hook(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    payload: &HookPayload,
    ctx: &HookContext,
) -> Result<(), IpcError> {
    let Some(session_id) = &payload.session_id else {
        return Ok(());
    };
    let row_id = {
        let s = lock(store)?;
        let Some((row, binding)) = resolve_and_rebind(&s, payload, ctx, StartSource::Unknown)?
        else {
            return Ok(());
        };
        s.conversation_bump_turns(row.id, session_id)?;
        if binding != Binding::Current {
            return Ok(());
        }
        remember_transcript_path(&s, row.id, payload, session_id);
        if let Some(row) = s.record_stop_failure_hook_for_row(row.id)? {
            let error = payload.error.as_deref().unwrap_or("unknown");
            let detail = match payload.error_details.as_deref() {
                Some(d) if !d.trim().is_empty() => format!("{error}: {}", d.trim()),
                _ => error.to_string(),
            };
            best_effort_event_for(&s, row.id, Some(session_id), "stop_failure", Some(&detail));
        }
        row.id
    };
    spawn_refresh_context(store, ssh, row_id);
    Ok(())
}

/// What a `Notification` type means for the row: the mapped status and
/// `Some(Some(kind))` to set / `Some(None)` to clear / `None` to leave the
/// stuck fields alone. `None` overall = not a type fleet acts on (the
/// installed matcher never sends one, but a hand-posted body might).
pub(crate) fn notification_effect(
    notification_type: &str,
) -> Option<(ClaudeStatus, Option<Option<StuckKind>>)> {
    Some(match notification_type {
        "permission_prompt" | "elicitation_dialog" | "elicitation_url_dialog" => {
            (ClaudeStatus::Blocked, None)
        }
        // Claude Code waits for Enter after a long sleep: the existing
        // press_enter playbook resolves it.
        "quota_auto_resume_stale" => (ClaudeStatus::Blocked, Some(Some(StuckKind::PressEnter))),
        "quota_auto_resume_disabled" => (ClaudeStatus::Blocked, None),
        "quota_auto_resume_fired" => (ClaudeStatus::Working, Some(None)),
        _ => return None,
    })
}

/// The Notification hook: Claude is waiting on a human (or just stopped
/// waiting). Applies [`notification_effect`] and records `notification`
/// with the type. `message` / `title` are never stored. Only the row's
/// current conversation may change its status.
fn apply_notification_hook(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    ctx: &HookContext,
) -> Result<(), IpcError> {
    let (Some(session_id), Some(kind)) =
        (&payload.session_id, payload.notification_type.as_deref())
    else {
        return Ok(());
    };
    let Some((status, stuck)) = notification_effect(kind) else {
        return Ok(());
    };
    let s = lock(store)?;
    let Some((row, Binding::Current)) = resolve_and_rebind(&s, payload, ctx, StartSource::Unknown)?
    else {
        return Ok(());
    };
    remember_transcript_path(&s, row.id, payload, session_id);
    if let Some(row) = s.record_notification_hook_for_row(row.id, status, stuck)? {
        best_effort_event_for(&s, row.id, Some(session_id), "notification", Some(kind));
    }
    Ok(())
}

/// Timeline writes never fail the hook that produced them. Hook events carry
/// the conversation they belong to.
fn best_effort_event_for(
    s: &Store,
    session_id: i64,
    claude_id: Option<&str>,
    kind: &str,
    detail: Option<&str>,
) {
    if let Err(e) = s.insert_session_event_for(session_id, claude_id, kind, detail) {
        tracing::warn!(session_id, kind, error = %e, "[hook] session_event insert failed");
    }
}

/// Validate a `worktree_path` from a hook body before it becomes a row:
/// absolute, no `..` component, no control characters, and its basename a
/// safe path component. The hook body is network input signed only by a
/// host token, so it gets the same scrutiny as a frontend value.
///
/// Delegates the rule itself to [`crate::validate::remote_worktree_path`] —
/// the same check a worktree row's stored `path` gets on the session-lifecycle
/// side (`service::sessions`) — and only re-maps the error code: a hook
/// handler answers 400 with `E_VALIDATE`, not the frontend-facing `E_INVALID`
/// `validate.rs` uses everywhere else.
pub fn validate_worktree_path(path: &str) -> Result<(), IpcError> {
    crate::validate::remote_worktree_path("worktree_path", path)
        .map_err(|e| IpcError::new(codes::E_VALIDATE, e.message))
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
        crate::service::hub::ensure_local_allowed(host)?;
        canonical_str(&path)
    } else {
        path
    };
    let s = lock(store)?;
    s.delete_worktrees_at(host, &path)?;
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
        crate::validate::git_ref(b).map_err(|e| IpcError::new(codes::E_VALIDATE, e.message))?;
    }

    let host = caller_host(caller);
    if host != LOCAL_HOST {
        let s = lock(store)?;
        let projects = s.list_projects()?;
        let paths = HostPaths::for_host(&s, host);
        let Some(project_id) = crate::service::sessions::find_project_id_for_path(
            &projects,
            host,
            Path::new(&path),
            &paths,
        ) else {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                format!("worktree_path {path} is not under a known project on host {host}"),
            ));
        };
        let name = Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed")
            .to_string();
        s.upsert_worktree_on(host, project_id, &name, &path, branch)?;
        return Ok(());
    }

    // Local: resolve symlinks (off-lock; it is filesystem IO), then validate
    // the physical form too, since that is what gets stored. Not on a hub
    // without a local host: the path would be resolved on the hub itself.
    crate::service::hub::ensure_local_allowed(host)?;
    let path = canonical_str(&path);
    validate_worktree_path(&path)?;
    let name = Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .to_string();
    let projects = {
        let s = lock(store)?;
        s.list_projects()?
    };
    let Some(project_id) = find_project_id_for_path(&projects, &path) else {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!("worktree_path {path} is not under any known project base"),
        ));
    };
    let s = lock(store)?;
    s.upsert_worktree(project_id, &name, &path, branch)?;
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
            ..Default::default()
        }
    }

    #[test]
    fn stop_hook_on_unknown_session_is_noop() {
        let store = make_store();
        let payload = make_payload("Stop", "no-such-id");
        assert!(apply_hook(&store, &make_ssh(), &payload, &ctx(&Caller::master(), None)).is_ok());
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
            &ctx(&Caller::master(), None),
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
            client: None,
            mode: crate::mcp::TokenMode::Full,
        };
        let err = apply_hook(
            &store,
            &make_ssh(),
            &make_payload("Stop", "uuid-b"),
            &ctx(&host_a, None),
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
            client: None,
            mode: crate::mcp::TokenMode::Readonly,
        };
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("Stop", "uuid-b"),
            &ctx(&host_b, None),
        )
        .unwrap();
        let s = store.lock().unwrap();
        let row = s.get_session("sess", "hostb").unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
        // An unknown session stays a no-op for any caller.
        drop(s);
        assert!(apply_hook(
            &store,
            &make_ssh(),
            &make_payload("Stop", "nope"),
            &ctx(&host_a, None)
        )
        .is_ok());
    }

    #[test]
    fn unknown_event_is_noop() {
        let store = make_store();
        let payload = make_payload("SubagentStop", "s1");
        assert!(apply_hook(&store, &make_ssh(), &payload, &ctx(&Caller::master(), None)).is_ok());
        // A known event about an unknown session is a no-op too.
        let payload = make_payload("SessionStart", "s1");
        assert!(apply_hook(&store, &make_ssh(), &payload, &ctx(&Caller::master(), None)).is_ok());
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
                &ctx(&Caller::master(), None),
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
            client: None,
            mode: crate::mcp::TokenMode::Full,
        };
        let err = apply_hook(
            &store,
            &make_ssh(),
            &make_payload("UserPromptSubmit", "uuid-b"),
            &ctx(&host_a, None),
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
            client: None,
            mode: crate::mcp::TokenMode::Readonly,
        };
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("UserPromptSubmit", "uuid-b"),
            &ctx(&host_b, None),
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
            &ctx(&host_a, None)
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
            ..Default::default()
        };
        assert!(apply_hook(&store, &make_ssh(), &payload, &ctx(&Caller::master(), None)).is_ok());
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
            ..Default::default()
        }
    }

    // The rule itself (empty / too long / relative / `..` / control chars /
    // bad basename / valid) is tested once, on `crate::validate::remote_worktree_path`,
    // in `validate.rs`. What's specific to this wrapper — and so worth testing
    // here — is that a rejection surfaces as `E_VALIDATE` (a hook handler's
    // 400) rather than the `E_INVALID` `validate.rs` uses everywhere else.
    #[test]
    fn validate_worktree_path_accepts_a_clean_path() {
        assert!(validate_worktree_path("/home/u/proj/.worktrees/feat").is_ok());
    }

    #[test]
    fn validate_worktree_path_maps_the_shared_rule_s_rejection_to_e_validate() {
        for bad in ["", "relative/path", "/home/u/proj/.worktrees/bad\nname"] {
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
            &ctx(&Caller::master(), None),
        )
        .unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
        let err = apply_hook(
            &store,
            &make_ssh(),
            &worktree_payload("../../etc/passwd", None),
            &ctx(&Caller::master(), None),
        )
        .unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
        // A branch that looks like a git option is refused too.
        let err = apply_hook(
            &store,
            &make_ssh(),
            &worktree_payload("/home/u/proj/.worktrees/feat", Some("--upload-pack=x")),
            &ctx(&Caller::master(), None),
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
            &ctx(&Caller::master(), None),
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
            client: None,
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
            ..Default::default()
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
        apply_hook(
            &store,
            &make_ssh(),
            &exit_payload("keep", Some(wt)),
            &ctx(&mef, None),
        )
        .unwrap();
        apply_hook(
            &store,
            &make_ssh(),
            &exit_payload("remove", None),
            &ctx(&mef, None),
        )
        .unwrap();
        assert_eq!(on("mefistos"), 1);
        // `remove` drops the caller's row, and only that one.
        apply_hook(
            &store,
            &make_ssh(),
            &exit_payload("remove", Some(wt)),
            &ctx(&mef, None),
        )
        .unwrap();
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
            &ctx(&Caller::master(), None),
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
            &ctx(&host_caller("mefistos"), None),
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
            &ctx(&mef, None),
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
            &ctx(&mef, None),
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
        apply_hook(&store, &make_ssh(), &custom, &ctx(&mef, None)).unwrap();
        // The same path under the master token is judged against the LOCAL
        // bases, where it belongs to nothing.
        let err =
            apply_hook(&store, &make_ssh(), &custom, &ctx(&Caller::master(), None)).unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
        // A path that fails validation is refused before any matching.
        let err = apply_hook(
            &store,
            &make_ssh(),
            &worktree_payload("/home/m/code/r/../../etc", None),
            &ctx(&mef, None),
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
            &ctx(&host_caller("local"), None),
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
        assert!(apply_hook(&store, &make_ssh(), &p, &ctx(&Caller::master(), None)).is_ok());
        // A hand-posted legacy `WorktreeCreate` body is still validated.
        let mut legacy = worktree_payload("../../etc", None);
        legacy.tool_name = Some("WorktreeCreate".into());
        let err =
            apply_hook(&store, &make_ssh(), &legacy, &ctx(&Caller::master(), None)).unwrap_err();
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
        apply_hook(&store, &make_ssh(), &p, &ctx(&Caller::master(), None)).unwrap();
        assert_eq!(
            store.lock().unwrap().session_transcript_path(id).unwrap(),
            None
        );
        let good = "/home/u/.claude/projects/-home-u-p/uuid-1.jsonl";
        let mut p = make_payload("Stop", "uuid-1");
        p.transcript_path = Some(good.into());
        apply_hook(&store, &make_ssh(), &p, &ctx(&Caller::master(), None)).unwrap();
        let s = store.lock().unwrap();
        assert_eq!(
            s.session_transcript_path(id).unwrap().as_deref(),
            Some(good)
        );
        // ...and on the conversation row, so an earlier conversation stays
        // readable after the session moves on.
        let convs = s.list_conversations(id, 10).unwrap();
        assert_eq!(convs[0].transcript_path.as_deref(), Some(good));
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
                adopted: false,
                system: false,
            },
            ProjectRow {
                id: 2,
                owner: "o".into(),
                repo: "r2".into(),
                base_path: "/home/u/proj/sub".into(),
                last_session_at: None,
                adopted: false,
                system: false,
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
            adopted: false,
            system: false,
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

    // ---- SessionEnd / StopFailure / Notification ----

    fn hooked(store: &Arc<Mutex<Store>>) -> i64 {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 0, 0, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "uuid-1").unwrap();
        id
    }

    fn events(store: &Arc<Mutex<Store>>, id: i64) -> Vec<(String, Option<String>)> {
        store
            .lock()
            .unwrap()
            .list_session_events(id, 50)
            .unwrap()
            .into_iter()
            .map(|e| (e.kind, e.detail))
            .collect()
    }

    fn status_of(store: &Arc<Mutex<Store>>, id: i64) -> SessionRow {
        store
            .lock()
            .unwrap()
            .get_session_by_id(id)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn session_end_marks_stopped_and_records_the_reason() {
        let store = make_store();
        let id = hooked(&store);
        let mut p = make_payload("SessionEnd", "uuid-1");
        p.reason = Some("prompt_input_exit".into());
        apply_hook(&store, &make_ssh(), &p, &ctx(&Caller::master(), None)).unwrap();
        let row = status_of(&store, id);
        assert_eq!(row.claude_status.as_deref(), Some("stopped"));
        assert!(events(&store, id).contains(&(
            "session_end".to_string(),
            Some("prompt_input_exit".to_string())
        )));
    }

    #[test]
    fn session_end_clear_and_resume_close_the_conversation_and_keep_status() {
        for reason in ["clear", "resume"] {
            let store = make_store();
            let id = hooked(&store);
            let mut p = make_payload("SessionEnd", "uuid-1");
            p.reason = Some(reason.into());
            apply_hook(&store, &make_ssh(), &p, &ctx(&Caller::master(), None)).unwrap();
            let row = status_of(&store, id);
            assert_ne!(row.claude_status.as_deref(), Some("stopped"), "{reason}");
            assert!(events(&store, id)
                .contains(&("conversation_ended".to_string(), Some(reason.to_string()))));
            let s = store.lock().unwrap();
            let conv = &s.list_conversations(id, 5).unwrap()[0];
            assert_eq!(conv.end_reason.as_deref(), Some(reason));
            assert!(conv.ended_at.is_some());
            // Marked for a rebind by the next SessionStart / UserPromptSubmit.
            assert_eq!(s.sessions_awaiting_rebind("local").unwrap().len(), 1);
        }
    }

    #[test]
    fn session_end_exit_also_closes_the_conversation() {
        let store = make_store();
        let id = hooked(&store);
        let mut p = make_payload("SessionEnd", "uuid-1");
        p.reason = Some("logout".into());
        apply_hook(&store, &make_ssh(), &p, &ctx(&Caller::master(), None)).unwrap();
        let s = store.lock().unwrap();
        let conv = &s.list_conversations(id, 5).unwrap()[0];
        assert_eq!(conv.end_reason.as_deref(), Some("logout"));
        assert!(s.sessions_awaiting_rebind("local").unwrap().is_empty());
    }

    #[test]
    fn stop_failure_ends_the_turn_and_records_the_error() {
        let store = make_store();
        let id = hooked(&store);
        let mut p = make_payload("StopFailure", "uuid-1");
        p.error = Some("rate_limit".into());
        p.error_details = Some("429 Too Many Requests".into());
        apply_hook(&store, &make_ssh(), &p, &ctx(&Caller::master(), None)).unwrap();
        let row = status_of(&store, id);
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
        assert_eq!(row.turn_seq, 1);
        assert!(row.last_stop_at.is_some());
        assert!(events(&store, id).contains(&(
            "stop_failure".to_string(),
            Some("rate_limit: 429 Too Many Requests".to_string())
        )));
    }

    #[test]
    fn notification_effect_maps_the_installed_types() {
        use crate::service::pane_intel::{ClaudeStatus, StuckKind};
        assert_eq!(
            notification_effect("permission_prompt"),
            Some((ClaudeStatus::Blocked, None))
        );
        assert_eq!(
            notification_effect("elicitation_dialog"),
            Some((ClaudeStatus::Blocked, None))
        );
        assert_eq!(
            notification_effect("elicitation_url_dialog"),
            Some((ClaudeStatus::Blocked, None))
        );
        assert_eq!(
            notification_effect("quota_auto_resume_stale"),
            Some((ClaudeStatus::Blocked, Some(Some(StuckKind::PressEnter))))
        );
        assert_eq!(
            notification_effect("quota_auto_resume_disabled"),
            Some((ClaudeStatus::Blocked, None))
        );
        assert_eq!(
            notification_effect("quota_auto_resume_fired"),
            Some((ClaudeStatus::Working, Some(None)))
        );
        assert_eq!(notification_effect("idle_prompt"), None);
        assert_eq!(notification_effect("agent_needs_input"), None);
    }

    #[test]
    fn notification_hook_blocks_then_resumes() {
        let store = make_store();
        let id = hooked(&store);
        let mut p = make_payload("Notification", "uuid-1");
        p.notification_type = Some("quota_auto_resume_stale".into());
        apply_hook(&store, &make_ssh(), &p, &ctx(&Caller::master(), None)).unwrap();
        let row = status_of(&store, id);
        assert_eq!(row.claude_status.as_deref(), Some("blocked"));
        assert_eq!(row.stuck_kind.as_deref(), Some("press_enter"));

        p.notification_type = Some("quota_auto_resume_fired".into());
        apply_hook(&store, &make_ssh(), &p, &ctx(&Caller::master(), None)).unwrap();
        let row = status_of(&store, id);
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        assert!(row.stuck_kind.is_none());

        // An unmapped type (hand-posted; the matcher never sends it) is a no-op.
        p.notification_type = Some("idle_prompt".into());
        apply_hook(&store, &make_ssh(), &p, &ctx(&Caller::master(), None)).unwrap();
        let row = status_of(&store, id);
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        let ev = events(&store, id);
        assert_eq!(
            ev.iter().filter(|(k, _)| k == "notification").count(),
            2,
            "{ev:?}"
        );
    }

    #[test]
    fn new_events_respect_the_caller_host_binding() {
        let store = make_store();
        hooked(&store);
        let other = Caller {
            host_alias: Some("hostb".into()),
            client: None,
            mode: crate::mcp::TokenMode::Full,
        };
        for (event, field) in [
            ("SessionEnd", "reason"),
            ("StopFailure", "error"),
            ("Notification", "notification_type"),
        ] {
            let mut p = make_payload(event, "uuid-1");
            match field {
                "reason" => p.reason = Some("other".into()),
                "error" => p.error = Some("unknown".into()),
                _ => p.notification_type = Some("permission_prompt".into()),
            }
            let e = apply_hook(&store, &make_ssh(), &p, &ctx(&other, None)).unwrap_err();
            assert_eq!(e.code, "E_FORBIDDEN", "{event}");
        }
    }

    // ---- Pane routing and conversations ----

    const OLD: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    const NEW: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";

    fn ctx<'a>(caller: &'a Caller, pane: Option<&str>) -> HookContext<'a> {
        HookContext {
            caller,
            pane_id: pane.map(String::from),
        }
    }

    fn pane_session(store: &Arc<Mutex<Store>>, name: &str, pane: &str) -> i64 {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session(name, "local", None, None, 0, 0, "running", None)
            .unwrap();
        s.set_claude_session_id(id, OLD).unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET tmux_pane_id=?1 WHERE id=?2",
                rusqlite::params![pane, id],
            )
            .unwrap();
        id
    }

    fn claude_id(store: &Arc<Mutex<Store>>, id: i64) -> Option<String> {
        status_of(store, id).claude_session_id
    }

    #[test]
    fn session_start_clear_rebinds_by_pane_and_zeroes_context() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        store
            .lock()
            .unwrap()
            .set_context(id, OLD, 120_000, 200_000, "transcript", None)
            .unwrap();
        let mut p = make_payload("SessionStart", NEW);
        p.source = Some("clear".into());
        p.model = Some("claude-opus-5".into());
        let host = host_caller("local");
        apply_hook(&store, &make_ssh(), &p, &ctx(&host, Some("%3"))).unwrap();
        let row = status_of(&store, id);
        assert_eq!(row.claude_session_id.as_deref(), Some(NEW));
        assert_eq!(row.context.context_tokens, Some(0));
        let ev = events(&store, id);
        assert!(
            ev.contains(&(
                "conversation_started".to_string(),
                Some("clear".to_string())
            )),
            "{ev:?}"
        );
        let s = store.lock().unwrap();
        let convs = s.list_conversations(id, 5).unwrap();
        assert_eq!(convs.len(), 2);
        let new = convs.iter().find(|c| c.claude_session_id == NEW).unwrap();
        assert!(new.current);
        assert_eq!(new.start_source, "clear");
        assert_eq!(new.model.as_deref(), Some("claude-opus-5"));
        let ev = s.list_session_events(id, 5).unwrap();
        let started = ev
            .iter()
            .find(|e| e.kind == "conversation_started")
            .unwrap();
        assert_eq!(started.claude_session_id.as_deref(), Some(NEW));
    }

    #[test]
    fn old_stop_after_clear_does_not_rebind_back() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let host = host_caller("local");
        let mut start = make_payload("SessionStart", NEW);
        start.source = Some("clear".into());
        apply_hook(&store, &make_ssh(), &start, &ctx(&host, Some("%3"))).unwrap();
        let before = status_of(&store, id);
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("Stop", OLD),
            &ctx(&host, Some("%3")),
        )
        .unwrap();
        let after = status_of(&store, id);
        assert_eq!(after.claude_session_id.as_deref(), Some(NEW));
        // The old turn is counted on its own conversation only.
        assert_eq!(after.turn_seq, before.turn_seq);
        let s = store.lock().unwrap();
        let old = s
            .list_conversations(id, 5)
            .unwrap()
            .into_iter()
            .find(|c| c.claude_session_id == OLD)
            .unwrap();
        assert_eq!(old.turns, 1);
    }

    #[test]
    fn prompt_submit_rebinds_via_awaiting_mark_when_no_pane_header() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let host = host_caller("local");
        let mut end = make_payload("SessionEnd", OLD);
        end.reason = Some("clear".into());
        apply_hook(&store, &make_ssh(), &end, &ctx(&host, None)).unwrap();
        // Status stays; conversation closed with reason clear.
        assert_ne!(
            status_of(&store, id).claude_status.as_deref(),
            Some("stopped")
        );
        // One awaiting row on the host, no cwd on either side → rebinds.
        let mut prompt = make_payload("UserPromptSubmit", NEW);
        prompt.cwd = None;
        prompt.prompt = Some("  fix the flaky test ".repeat(20));
        apply_hook(&store, &make_ssh(), &prompt, &ctx(&host, None)).unwrap();
        let row = status_of(&store, id);
        assert_eq!(row.claude_session_id.as_deref(), Some(NEW));
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        let s = store.lock().unwrap();
        let convs = s.list_conversations(id, 5).unwrap();
        assert_eq!(
            convs
                .iter()
                .find(|c| c.claude_session_id == OLD)
                .unwrap()
                .end_reason
                .as_deref(),
            Some("clear")
        );
        let new = convs.iter().find(|c| c.claude_session_id == NEW).unwrap();
        // The just-closed conversation's end reason says how this one began.
        assert_eq!(new.start_source, "clear");
        assert_eq!(
            new.first_prompt.as_deref().map(|p| p.chars().count()),
            Some(200)
        );
        assert!(s.sessions_awaiting_rebind("local").unwrap().is_empty());
    }

    #[test]
    fn awaiting_rebind_requires_a_matching_cwd() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        {
            let s = store.lock().unwrap();
            let pid = s.upsert_project("o", "r", "/home/u/proj").unwrap();
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET project_id=?1 WHERE id=?2",
                    rusqlite::params![pid, id],
                )
                .unwrap();
            s.mark_awaiting_rebind(id).unwrap();
        }
        let host = host_caller("local");
        let mut elsewhere = make_payload("UserPromptSubmit", NEW);
        elsewhere.cwd = Some("/home/u/other".into());
        apply_hook(&store, &make_ssh(), &elsewhere, &ctx(&host, None)).unwrap();
        assert_eq!(claude_id(&store, id).as_deref(), Some(OLD));
        let mut here = make_payload("UserPromptSubmit", NEW);
        here.cwd = Some("/home/u/proj".into());
        apply_hook(&store, &make_ssh(), &here, &ctx(&host, None)).unwrap();
        assert_eq!(claude_id(&store, id).as_deref(), Some(NEW));
    }

    #[test]
    fn awaiting_rebind_is_ambiguous_with_two_candidates() {
        let store = make_store();
        let a = pane_session(&store, "a", "%3");
        let b = pane_session(&store, "b", "%4");
        {
            let s = store.lock().unwrap();
            s.mark_awaiting_rebind(a).unwrap();
            s.mark_awaiting_rebind(b).unwrap();
        }
        let host = host_caller("local");
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("UserPromptSubmit", NEW),
            &ctx(&host, None),
        )
        .unwrap();
        assert_eq!(claude_id(&store, a).as_deref(), Some(OLD));
        assert_eq!(claude_id(&store, b).as_deref(), Some(OLD));
    }

    #[test]
    fn awaiting_rebind_never_makes_a_third_holder_of_a_shared_id() {
        let store = make_store();
        let a = pane_session(&store, "a", "%3");
        let b = pane_session(&store, "b", "%4");
        let c = pane_session(&store, "c", "%5");
        {
            let s = store.lock().unwrap();
            // Two rows already hold NEW (step 2 abstains on them) ...
            for id in [a, b] {
                s.set_claude_session_id(id, NEW).unwrap();
            }
            // ... and a third row is awaiting a rebind.
            s.mark_awaiting_rebind(c).unwrap();
        }
        let host = host_caller("local");
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("UserPromptSubmit", NEW),
            &ctx(&host, None),
        )
        .unwrap();
        assert_eq!(claude_id(&store, c).as_deref(), Some(OLD));
        assert_eq!(
            store
                .lock()
                .unwrap()
                .sessions_by_claude_id(NEW)
                .unwrap()
                .len(),
            2
        );
    }

    fn last_hook_at(store: &Arc<Mutex<Store>>, id: i64) -> Option<i64> {
        store
            .lock()
            .unwrap()
            .conn_ref()
            .query_row("SELECT last_hook_at FROM sessions WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
    }

    #[test]
    fn hook_rebinds_and_the_awaiting_mark_stamp_last_hook_at() {
        // The reconcile in-flight guard keys on `last_hook_at`: a pass that
        // probed before these hooks must not write the old id back.
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        assert_eq!(last_hook_at(&store, id), None);
        let host = host_caller("local");
        let mut end = make_payload("SessionEnd", OLD);
        end.reason = Some("clear".into());
        apply_hook(&store, &make_ssh(), &end, &ctx(&host, None)).unwrap();
        assert!(last_hook_at(&store, id).is_some());

        let id2 = pane_session(&store, "t", "%4");
        let mut start = make_payload("SessionStart", NEW);
        start.source = Some("clear".into());
        apply_hook(&store, &make_ssh(), &start, &ctx(&host, Some("%4"))).unwrap();
        assert_eq!(claude_id(&store, id2).as_deref(), Some(NEW));
        assert!(last_hook_at(&store, id2).is_some());
    }

    #[test]
    fn non_rebinding_events_never_use_the_awaiting_mark() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        store.lock().unwrap().mark_awaiting_rebind(id).unwrap();
        let host = host_caller("local");
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("Stop", NEW),
            &ctx(&host, None),
        )
        .unwrap();
        let row = status_of(&store, id);
        assert_eq!(row.claude_session_id.as_deref(), Some(OLD));
        assert_eq!(row.turn_seq, 0);
    }

    #[test]
    fn master_caller_never_rebinds_through_the_awaiting_mark() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        store.lock().unwrap().mark_awaiting_rebind(id).unwrap();
        let master = Caller::master();
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("UserPromptSubmit", NEW),
            &ctx(&master, None),
        )
        .unwrap();
        assert_eq!(claude_id(&store, id).as_deref(), Some(OLD));
        // Nor through a pane header: the master token has no host.
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("UserPromptSubmit", NEW),
            &ctx(&master, Some("%3")),
        )
        .unwrap();
        assert_eq!(claude_id(&store, id).as_deref(), Some(OLD));
    }

    #[test]
    fn a_rebind_validates_the_new_id() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let host = host_caller("local");
        let mut p = make_payload("SessionStart", "not a uuid; rm -rf");
        p.source = Some("clear".into());
        let e = apply_hook(&store, &make_ssh(), &p, &ctx(&host, Some("%3"))).unwrap_err();
        assert_eq!(e.code, "E_VALIDATE");
        assert_eq!(claude_id(&store, id).as_deref(), Some(OLD));
    }

    #[test]
    fn session_start_with_the_current_id_resets_and_reopens() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        store
            .lock()
            .unwrap()
            .set_context(id, OLD, 50_000, 200_000, "transcript", None)
            .unwrap();
        let host = host_caller("local");
        let mut p = make_payload("SessionStart", OLD);
        p.source = Some("startup".into());
        apply_hook(&store, &make_ssh(), &p, &ctx(&host, Some("%3"))).unwrap();
        let row = status_of(&store, id);
        assert_eq!(row.claude_session_id.as_deref(), Some(OLD));
        assert_eq!(row.context.context_tokens, Some(0));
        // `/resume` back to the current conversation after SessionEnd(resume)
        // reopens it and clears the awaiting mark.
        let mut end = make_payload("SessionEnd", OLD);
        end.reason = Some("resume".into());
        apply_hook(&store, &make_ssh(), &end, &ctx(&host, Some("%3"))).unwrap();
        let mut resume = make_payload("SessionStart", OLD);
        resume.source = Some("resume".into());
        apply_hook(&store, &make_ssh(), &resume, &ctx(&host, Some("%3"))).unwrap();
        let s = store.lock().unwrap();
        let convs = s.list_conversations(id, 5).unwrap();
        assert_eq!(convs.len(), 1);
        assert!(convs[0].current && convs[0].ended_at.is_none());
        assert!(s.sessions_awaiting_rebind("local").unwrap().is_empty());
    }

    #[test]
    fn pre_and_post_compact_record_one_compaction() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let host = host_caller("local");
        let mut pre = make_payload("PreCompact", OLD);
        pre.trigger = Some("auto".into());
        apply_hook(&store, &make_ssh(), &pre, &ctx(&host, Some("%3"))).unwrap();
        assert_eq!(
            status_of(&store, id).current_activity.as_deref(),
            Some("compacting")
        );
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("PostCompact", OLD),
            &ctx(&host, Some("%3")),
        )
        .unwrap();
        assert_eq!(status_of(&store, id).current_activity, None);
        let mut again = make_payload("SessionStart", OLD);
        again.source = Some("compact".into());
        apply_hook(&store, &make_ssh(), &again, &ctx(&host, Some("%3"))).unwrap();
        let ev = events(&store, id);
        assert!(ev.contains(&("compact_started".to_string(), Some("auto".to_string()))));
        assert_eq!(ev.iter().filter(|(k, _)| k == "compact_done").count(), 1);
        let s = store.lock().unwrap();
        assert_eq!(s.list_conversations(id, 1).unwrap()[0].compactions, 1);
        assert!(
            s.get_session_by_id(id)
                .unwrap()
                .unwrap()
                .context
                .context_stale
        );
    }

    #[test]
    fn a_late_compact_start_never_rebinds_back() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let host = host_caller("local");
        let mut clear = make_payload("SessionStart", NEW);
        clear.source = Some("clear".into());
        apply_hook(&store, &make_ssh(), &clear, &ctx(&host, Some("%3"))).unwrap();
        let mut compact = make_payload("SessionStart", OLD);
        compact.source = Some("compact".into());
        apply_hook(&store, &make_ssh(), &compact, &ctx(&host, Some("%3"))).unwrap();
        let row = status_of(&store, id);
        assert_eq!(row.claude_session_id.as_deref(), Some(NEW));
        // Counted on the old conversation; the new one's context stays fresh.
        assert!(!row.context.context_stale);
        let s = store.lock().unwrap();
        let old = s
            .list_conversations(id, 5)
            .unwrap()
            .into_iter()
            .find(|c| c.claude_session_id == OLD)
            .unwrap();
        assert_eq!(old.compactions, 1);
    }

    #[test]
    fn prompt_submit_and_stop_end_a_compacting_activity() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let host = host_caller("local");
        for event in ["UserPromptSubmit", "Stop"] {
            store
                .lock()
                .unwrap()
                .set_current_activity(id, Some("compacting"))
                .unwrap();
            apply_hook(
                &store,
                &make_ssh(),
                &make_payload(event, OLD),
                &ctx(&host, Some("%3")),
            )
            .unwrap();
            assert_eq!(status_of(&store, id).current_activity, None, "{event}");
        }
    }

    #[test]
    fn stop_records_turn_done_with_a_capped_message() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let host = host_caller("local");
        let mut stop = make_payload("Stop", OLD);
        stop.last_assistant_message = Some("x".repeat(500));
        apply_hook(&store, &make_ssh(), &stop, &ctx(&host, Some("%3"))).unwrap();
        let s = store.lock().unwrap();
        let ev = s.list_session_events(id, 5).unwrap();
        let done = ev.iter().find(|e| e.kind == "turn_done").unwrap();
        assert_eq!(done.detail.as_deref().map(str::len), Some(200));
        assert_eq!(done.claude_session_id.as_deref(), Some(OLD));
        assert_eq!(s.list_conversations(id, 1).unwrap()[0].turns, 1);
    }

    #[test]
    fn a_stale_notification_does_not_change_status() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let host = host_caller("local");
        let mut clear = make_payload("SessionStart", NEW);
        clear.source = Some("clear".into());
        apply_hook(&store, &make_ssh(), &clear, &ctx(&host, Some("%3"))).unwrap();
        let mut n = make_payload("Notification", OLD);
        n.notification_type = Some("permission_prompt".into());
        apply_hook(&store, &make_ssh(), &n, &ctx(&host, Some("%3"))).unwrap();
        assert_ne!(
            status_of(&store, id).claude_status.as_deref(),
            Some("blocked")
        );
    }

    #[test]
    fn pane_on_another_host_is_not_resolved() {
        let store = make_store();
        pane_session(&store, "s", "%3");
        store.lock().unwrap().upsert_host("other").unwrap();
        let other = host_caller("other");
        let mut p = make_payload("SessionStart", NEW);
        p.source = Some("clear".into());
        apply_hook(&store, &make_ssh(), &p, &ctx(&other, Some("%3"))).unwrap();
        let s = store.lock().unwrap();
        assert_eq!(
            s.get_session("s", "local")
                .unwrap()
                .unwrap()
                .claude_session_id
                .as_deref(),
            Some(OLD)
        );
    }

    #[test]
    fn an_id_shared_by_two_rows_resolves_only_by_pane() {
        let store = make_store();
        let a = pane_session(&store, "a", "%3");
        let b = pane_session(&store, "b", "%4"); // both bound to OLD
        let host = host_caller("local");
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("UserPromptSubmit", OLD),
            &ctx(&host, None),
        )
        .unwrap();
        for id in [a, b] {
            assert_ne!(
                status_of(&store, id).claude_status.as_deref(),
                Some("working")
            );
        }
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("UserPromptSubmit", OLD),
            &ctx(&host, Some("%4")),
        )
        .unwrap();
        assert_eq!(
            status_of(&store, b).claude_status.as_deref(),
            Some("working")
        );
        assert_ne!(
            status_of(&store, a).claude_status.as_deref(),
            Some("working")
        );
    }

    // ---- Rebind eligibility (nested `claude` in the pane) ----

    const CHILD: &str = "cccccccc-cccc-cccc-cccc-cccccccccccc";

    /// A sweep instant just after now (well inside the task TTL).
    fn soon() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 10
    }

    fn working_parent_with_task(store: &Arc<Mutex<Store>>) -> (i64, i64) {
        let id = pane_session(store, "s", "%3");
        let s = store.lock().unwrap();
        s.set_context(id, OLD, 120_000, 200_000, "transcript", None)
            .unwrap();
        s.set_last_prompt(id, "run the tests").unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET claude_status='working' WHERE id=?1",
                [id],
            )
            .unwrap();
        let t = crate::service::tasks::create_task(&s, None, Some(id), "x").unwrap();
        s.set_task_worker_claude_id(t.id, OLD).unwrap();
        (id, t.id)
    }

    #[test]
    fn a_nested_claude_in_the_pane_never_touches_the_parent_row() {
        let store = make_store();
        let (id, task) = working_parent_with_task(&store);
        let before = status_of(&store, id);
        let convs_before = store.lock().unwrap().list_conversations(id, 10).unwrap();
        let events_before = events(&store, id);
        let host = host_caller("local");
        let mut start = make_payload("SessionStart", CHILD);
        start.source = Some("startup".into());
        let mut prompt = make_payload("UserPromptSubmit", CHILD);
        prompt.prompt = Some("summarise".into());
        let mut end = make_payload("SessionEnd", CHILD);
        end.reason = Some("other".into());
        for p in [start, prompt, make_payload("Stop", CHILD), end] {
            apply_hook(&store, &make_ssh(), &p, &ctx(&host, Some("%3"))).unwrap();
        }
        let after = status_of(&store, id);
        assert_eq!(after.claude_session_id.as_deref(), Some(OLD));
        assert_eq!(after.claude_status.as_deref(), Some("working"));
        assert_eq!(after.turn_seq, before.turn_seq);
        assert_eq!(after.context.context_tokens, Some(120_000));
        assert_eq!(after.last_prompt.as_deref(), Some("run the tests"));
        let s = store.lock().unwrap();
        assert_eq!(s.list_conversations(id, 10).unwrap(), convs_before);
        drop(s);
        assert_eq!(events(&store, id), events_before);
        let s = store.lock().unwrap();
        assert!(crate::service::tasks::sweep_open_tasks(&s, soon())
            .unwrap()
            .is_empty());
        let t = s.get_task(task).unwrap().unwrap();
        assert!(t.finished_at.is_none());
        assert_eq!(t.worker_claude_session_id.as_deref(), Some(OLD));
    }

    #[test]
    fn a_pane_rebind_needs_an_eligible_row_or_a_clear_or_resume_start() {
        // (d) SessionStart(clear) from the pane still rebinds; an
        // interactive /resume rebinds through (b) after SessionEnd(resume).
        for source in ["clear", "resume"] {
            let store = make_store();
            let id = pane_session(&store, "s", "%3");
            if source == "resume" {
                let mut end = make_payload("SessionEnd", OLD);
                end.reason = Some("resume".into());
                apply_hook(
                    &store,
                    &make_ssh(),
                    &end,
                    &ctx(&host_caller("local"), Some("%3")),
                )
                .unwrap();
            }
            let mut p = make_payload("SessionStart", NEW);
            p.source = Some(source.into());
            apply_hook(
                &store,
                &make_ssh(),
                &p,
                &ctx(&host_caller("local"), Some("%3")),
            )
            .unwrap();
            assert_eq!(claude_id(&store, id).as_deref(), Some(NEW), "{source}");
        }
        // (b) UserPromptSubmit after SessionEnd(clear) rebinds by pane.
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let host = host_caller("local");
        let mut end = make_payload("SessionEnd", OLD);
        end.reason = Some("clear".into());
        apply_hook(&store, &make_ssh(), &end, &ctx(&host, Some("%3"))).unwrap();
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("UserPromptSubmit", NEW),
            &ctx(&host, Some("%3")),
        )
        .unwrap();
        assert_eq!(claude_id(&store, id).as_deref(), Some(NEW));
        // (c) the current conversation ended: a fresh `claude` starts over.
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let mut exit = make_payload("SessionEnd", OLD);
        exit.reason = Some("prompt_input_exit".into());
        apply_hook(&store, &make_ssh(), &exit, &ctx(&host, Some("%3"))).unwrap();
        let mut start = make_payload("SessionStart", NEW);
        start.source = Some("startup".into());
        apply_hook(&store, &make_ssh(), &start, &ctx(&host, Some("%3"))).unwrap();
        assert_eq!(claude_id(&store, id).as_deref(), Some(NEW));
        // (a) a row with no id yet binds.
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "UPDATE sessions SET claude_session_id=NULL WHERE id=?1",
                [id],
            )
            .unwrap();
        let mut start = make_payload("SessionStart", NEW);
        start.source = Some("startup".into());
        apply_hook(&store, &make_ssh(), &start, &ctx(&host, Some("%3"))).unwrap();
        assert_eq!(claude_id(&store, id).as_deref(), Some(NEW));
    }

    // ---- SessionStart arriving after the first UserPromptSubmit ----

    #[test]
    fn a_prompt_rebind_after_session_end_takes_the_source_from_the_end_reason() {
        for (reason, source) in [("clear", "clear"), ("resume", "resume")] {
            let store = make_store();
            let (id, _) = working_parent_with_task(&store);
            let host = host_caller("local");
            let mut end = make_payload("SessionEnd", OLD);
            end.reason = Some(reason.into());
            apply_hook(&store, &make_ssh(), &end, &ctx(&host, Some("%3"))).unwrap();
            apply_hook(
                &store,
                &make_ssh(),
                &make_payload("UserPromptSubmit", NEW),
                &ctx(&host, Some("%3")),
            )
            .unwrap();
            let s = store.lock().unwrap();
            let new = s.get_conversation(id, NEW).unwrap().unwrap();
            assert_eq!(new.start_source, source, "{reason}");
            // The worker's tolerated switch keeps its task.
            assert!(
                crate::service::tasks::sweep_open_tasks(&s, soon())
                    .unwrap()
                    .is_empty(),
                "{reason}"
            );
            // The prompt just sent survives the rebind.
            let row = s.get_session_by_id(id).unwrap().unwrap();
            assert_eq!(
                row.last_prompt.as_deref(),
                Some("run the tests"),
                "{reason}"
            );
        }
    }

    #[test]
    fn a_same_id_session_start_upgrades_an_unknown_source() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let host = host_caller("local");
        // No SessionEnd first (hook missing): the prompt rebinds as unknown.
        store.lock().unwrap().mark_awaiting_rebind(id).unwrap();
        apply_hook(
            &store,
            &make_ssh(),
            &make_payload("UserPromptSubmit", NEW),
            &ctx(&host, Some("%3")),
        )
        .unwrap();
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get_conversation(id, NEW)
                .unwrap()
                .unwrap()
                .start_source,
            "unknown"
        );
        let mut start = make_payload("SessionStart", NEW);
        start.source = Some("clear".into());
        apply_hook(&store, &make_ssh(), &start, &ctx(&host, Some("%3"))).unwrap();
        let s = store.lock().unwrap();
        assert_eq!(
            s.get_conversation(id, NEW).unwrap().unwrap().start_source,
            "clear"
        );
        // A known source is never overwritten.
        drop(s);
        let mut again = make_payload("SessionStart", NEW);
        again.source = Some("startup".into());
        apply_hook(&store, &make_ssh(), &again, &ctx(&host, Some("%3"))).unwrap();
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get_conversation(id, NEW)
                .unwrap()
                .unwrap()
                .start_source,
            "clear"
        );
    }

    #[test]
    fn a_late_session_start_after_the_turn_began_keeps_context_and_prompt() {
        let store = make_store();
        let (id, _) = working_parent_with_task(&store);
        let host = host_caller("local");
        let mut end = make_payload("SessionEnd", OLD);
        end.reason = Some("clear".into());
        apply_hook(&store, &make_ssh(), &end, &ctx(&host, Some("%3"))).unwrap();
        let mut prompt = make_payload("UserPromptSubmit", NEW);
        prompt.prompt = Some("next task".into());
        apply_hook(&store, &make_ssh(), &prompt, &ctx(&host, Some("%3"))).unwrap();
        // The turn's own context measurement lands before the late start.
        store
            .lock()
            .unwrap()
            .set_context(id, NEW, 30_000, 200_000, "transcript", None)
            .unwrap();
        let mut start = make_payload("SessionStart", NEW);
        start.source = Some("clear".into());
        apply_hook(&store, &make_ssh(), &start, &ctx(&host, Some("%3"))).unwrap();
        let row = status_of(&store, id);
        assert_eq!(row.context.context_tokens, Some(30_000));
        assert_eq!(row.last_prompt.as_deref(), Some("run the tests"));
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        let s = store.lock().unwrap();
        let conv = s.get_conversation(id, NEW).unwrap().unwrap();
        assert!(conv.current && conv.ended_at.is_none());
        assert_eq!(conv.start_source, "clear");
    }

    #[test]
    fn a_nested_resume_in_the_pane_never_touches_the_parent_row() {
        // `claude -p --resume <id>` / `-c` from the parent's Bash tool starts
        // with source `resume` and an id the parent never had.
        let store = make_store();
        let (id, task) = working_parent_with_task(&store);
        let before = status_of(&store, id);
        let convs_before = store.lock().unwrap().list_conversations(id, 10).unwrap();
        let host = host_caller("local");
        let mut start = make_payload("SessionStart", CHILD);
        start.source = Some("resume".into());
        apply_hook(&store, &make_ssh(), &start, &ctx(&host, Some("%3"))).unwrap();
        let after = status_of(&store, id);
        assert_eq!(after.claude_session_id.as_deref(), Some(OLD));
        assert_eq!(after.claude_status, before.claude_status);
        assert_eq!(after.context.context_tokens, Some(120_000));
        let s = store.lock().unwrap();
        assert_eq!(s.list_conversations(id, 10).unwrap(), convs_before);
        assert!(crate::service::tasks::sweep_open_tasks(&s, soon())
            .unwrap()
            .is_empty());
        let t = s.get_task(task).unwrap().unwrap();
        assert!(t.finished_at.is_none());
        assert_eq!(t.worker_claude_session_id.as_deref(), Some(OLD));
    }
}
