//! Finish or undo a **partial** move (Task 6): one that stopped after the
//! target session was already started, so both sessions were left alive
//! (`E_MOVE_PARTIAL`, [`super::EVENT_MOVE_PARTIAL`]).
//!
//! The handle a caller resolves against is the target session's own
//! `session_move_partial` timeline event, not `move_session`'s run-time
//! state ([`super::PartialCtx`], the in-flight guard, the carry's temp
//! files): all of that is gone the moment the async call that hit
//! `E_MOVE_PARTIAL` returns, and this recovery typically runs long after —
//! a different process, a different day. The event is the only durable
//! record of what the move had done and seen at the moment it stopped, which
//! is exactly why [`super::partial`] copies every fact a resolution needs
//! into its detail rather than leaving them to be re-derived.
//!
//! Two actions, both of which kill a live tmux session, so every refusal
//! below is load-bearing:
//!
//! - **Finish**: the move is accepted. The source is killed and the move is
//!   recorded complete ([`super::EVENT_MOVED`]), via [`finalise_source`] —
//!   the exact step a clean `move_session` ends on — so the transcript
//!   re-check that would refuse a source that kept working is not
//!   reimplemented here. Refused when the target row is not `running`, or
//!   when the partial never recorded where the source transcript was (no
//!   `source_transcript_size` / `_mtime`): a defensive refusal, since the
//!   transcript copy precedes every `partial(…)` call site in `move_session`
//!   — every real partial has these — but a resolution never guesses them.
//! - **Undo**: the move is discarded. Only the target is killed
//!   ([`super::EVENT_MOVE_UNDONE`] on both rows); the source is never
//!   touched. Refused when the target's `turn_seq` (or, when the partial
//!   recorded one, `last_turn_at`) has moved past what the partial saw — it
//!   took a turn undo would discard — or when its `claude_status` is not
//!   `idle` (`blocked` is not idle: it is waiting on a question, mid-turn).
//!   Undo never touches the target's disk: no worktree, transcript, carried
//!   file or script of any kind — nothing but the kill and the two event
//!   inserts (best-effort, like `finalise_source`'s own post-kill
//!   bookkeeping — a kill that already succeeded is never turned back into a
//!   hard error by a store that cannot be locked afterward).
//!
//! Every refusal returns before any kill. The identity of the handle itself
//! is checked first of all: `session_id` must be the partial's own recorded
//! target, and the other identifying fields (host, tmux name, source id)
//! must agree with what the store says now — a disagreement refuses rather
//! than warns, since it means this event does not belong to this row.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::ipc_error::{codes, lock, IpcError};
use crate::ssh::{SshClient, SshExec};
use crate::store::{SessionRow, Store};

use super::finalise::{finalise_source, FinaliseArgs};
use super::{
    carry, Located, MoveHooks, RealHooks, EVENT_MOVED, EVENT_MOVE_PARTIAL, EVENT_MOVE_UNDONE,
};

/// How many of the target's newest timeline events `resolve_move` scans for
/// the handle. Generous: a partial's own record-keeping (reconcile
/// transitions, hook events) can interleave, but a resolution is expected to
/// run soon after the partial, not after thousands of unrelated events.
const EVENT_SCAN_LIMIT: i64 = 500;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResolveMoveAction {
    Finish,
    Undo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveMoveArgs {
    /// The TARGET session of the partial move.
    pub session_id: i64,
    pub action: ResolveMoveAction,
}

/// What a completed `resolve_move` did. A wire type: every field is
/// required, so a rename is caught at the boundary rather than silently
/// defaulting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveMoveReport {
    pub action: ResolveMoveAction,
    pub source_session_id: i64,
    pub target_session_id: i64,
    pub from_host: String,
    pub to_host: String,
    pub source_killed: bool,
    pub target_killed: bool,
    pub warnings: Vec<String>,
}

/// The newest unresolved `session_move_partial`, parsed. Every field is an
/// `Option` exactly as the JSON detail has it (see [`super::partial`] /
/// [`super::record_partial`]) — a resolution refuses rather than guesses
/// when one it needs is missing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct PartialRecord {
    step: Option<String>,
    to_host: Option<String>,
    from_host: Option<String>,
    from_session_id: Option<i64>,
    to_session_id: Option<i64>,
    cause_code: Option<String>,
    to_tmux_name: Option<String>,
    claude_session_id: Option<String>,
    branch: Option<String>,
    source_transcript_size: Option<u64>,
    source_transcript_mtime: Option<i64>,
    source_transcript_path: Option<String>,
    to_turn_seq: Option<i64>,
    to_last_turn_at: Option<i64>,
}

/// Walk `events` newest-first and return the newest unresolved partial:
/// `None` as soon as a [`EVENT_MOVED`] or [`EVENT_MOVE_UNDONE`] is seen
/// before a [`EVENT_MOVE_PARTIAL`] (the move was already resolved, one way
/// or the other), and `None` if no partial is found at all. Pure over the
/// slice — no store access — so it is testable on its own.
fn newest_unresolved(events: &[crate::store::SessionEvent]) -> Option<PartialRecord> {
    for e in events {
        if e.kind == EVENT_MOVED || e.kind == EVENT_MOVE_UNDONE {
            return None;
        }
        if e.kind == EVENT_MOVE_PARTIAL {
            return e
                .detail
                .as_deref()
                .and_then(|d| serde_json::from_str(d).ok());
        }
    }
    None
}

/// Refuse (never just log) a mismatch between what the partial recorded for
/// an identifying field and what the store says now.
///
/// Fix round 1 / Finding 2: `host_alias` / `tmux_name` are the session's
/// unique key and are never `UPDATE`d once set, so for a partial that
/// genuinely belongs to this row the two always agree. A disagreement is
/// therefore not corruption to shrug off — it is the very signal that this
/// event does not belong to this row (a hand-edited event, or one orphaned
/// by a deleted-and-reused row id: `sessions.id` has no `AUTOINCREMENT`, and
/// deleting a session does not delete its `session_events`). `None`
/// (never recorded) is not a disagreement — it is simply absent, and is left
/// to whatever check actually requires the value.
fn refuse_if_differs(
    session_id: i64,
    field: &'static str,
    recorded: Option<&str>,
    actual: &str,
) -> Result<(), IpcError> {
    if let Some(r) = recorded {
        if r != actual {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "session {session_id}'s partial recorded {field} as {r:?}, but the store's current value is {actual:?}; expected them to agree — this event does not belong to this row"
                ),
            ));
        }
    }
    Ok(())
}

/// Turn a [`finalise_source`] error into the recovery's own: always
/// `E_INVALID_STATE`, whatever the underlying step's code was (its own
/// `details.step` travels into the message, per `finalise_source`'s own
/// contract), since — unlike a fresh `move_session` — there is no partial
/// left to fall back on here; the caller must look at the target and decide.
fn wrap_finalise_error(e: IpcError) -> IpcError {
    let step = e
        .details
        .as_ref()
        .and_then(|d| d.get("step"))
        .and_then(|v| v.as_str())
        .unwrap_or("finishing the source")
        .to_string();
    let mut out = IpcError::new(
        codes::E_INVALID_STATE,
        format!(
            "finishing this partial move stopped at {step}: {} ({}); the target is untouched — check the target session, then retry finish or undo instead",
            e.message, e.code
        ),
    );
    out.details = e.details;
    out
}

/// [`resolve_move`] over any transport and hooks.
pub(super) async fn resolve_move_with(
    args: ResolveMoveArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
) -> Result<ResolveMoveReport, IpcError> {
    let target = {
        let s = lock(store)?;
        s.get_session_by_id(args.session_id)?
    }
    .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no session {}", args.session_id)))?;

    let events = {
        let s = lock(store)?;
        s.list_session_events(target.id, EVENT_SCAN_LIMIT)?
    };
    let Some(partial) = newest_unresolved(&events) else {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "session {}'s timeline has no unresolved session_move_partial event; expected an unresolved partial move to finish or undo — it is not a partial move",
                target.id
            ),
        ));
    };
    tracing::info!(
        step = partial.step.as_deref().unwrap_or("?"),
        cause_code = partial.cause_code.as_deref().unwrap_or("?"),
        action = ?args.action,
        session_id = target.id,
        "[resolve_move] resolving a partial move"
    );
    // Fix round 1 / Finding 1 (Critical): `record_partial` writes the
    // identical `session_move_partial` on BOTH rows, so the source's own
    // timeline also carries an unresolved partial until a later successful
    // move resolves it. Without this check, calling `resolve_move` with the
    // source's id would silently treat the source as the target — Undo
    // would then kill the SOURCE (comparing its turn counters against the
    // TARGET's recorded ones) while leaving the target alive. This is a
    // precondition, not a log line: the caller must be handed the right id
    // back, not merely warned.
    let recorded_target_id = partial.to_session_id.ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "the partial on session {}'s timeline recorded no to_session_id; expected the target's id, to confirm this is the session it should be resolved against",
                target.id
            ),
        )
    })?;
    if recorded_target_id != target.id {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "session {} is the SOURCE of this partial move, not its target; the target is session {recorded_target_id} on {}; retry resolve_move against session {recorded_target_id} instead",
                target.id,
                partial.to_host.as_deref().unwrap_or("an unrecorded host"),
            ),
        ));
    }
    // Finding 2, same root: a disagreement on the other identifying fields
    // is the same signal, so it gets the same refusal.
    refuse_if_differs(
        target.id,
        "to_host",
        partial.to_host.as_deref(),
        &target.host_alias,
    )?;
    refuse_if_differs(
        target.id,
        "to_tmux_name",
        partial.to_tmux_name.as_deref(),
        &target.tmux_name,
    )?;

    match args.action {
        ResolveMoveAction::Finish => finish(store, ssh, hooks, target, partial).await,
        ResolveMoveAction::Undo => undo(store, hooks, target, partial).await,
    }
}

/// The source row for `target`, found through the persisted
/// `parent_session_id` set the moment the target row was registered (see
/// `move_session_with`) — the durable, store-verified link — falling back to
/// the partial's own `from_session_id` only when that is unset. Fix round 1
/// / Finding 2: when BOTH are present and disagree, that disagreement is
/// refused outright rather than silently picking one — the same signal as
/// Finding 1, that this event does not belong to this row.
fn source_id_for(target: &SessionRow, partial: &PartialRecord) -> Result<i64, IpcError> {
    match (target.parent_session_id, partial.from_session_id) {
        (Some(p), Some(f)) if p != f => Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "session {}'s parent_session_id is {p} but its partial recorded from_session_id {f}; expected them to agree — this event does not belong to this row",
                target.id
            ),
        )),
        (Some(p), _) => Ok(p),
        (None, Some(f)) => Ok(f),
        (None, None) => Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "session {} has no parent_session_id and its partial recorded no from_session_id; expected one to identify the source session",
                target.id
            ),
        )),
    }
}

fn fetch_source(store: &Mutex<Store>, source_id: i64) -> Result<SessionRow, IpcError> {
    let row = {
        let s = lock(store)?;
        s.get_session_by_id(source_id)?
    };
    row.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("the source session {source_id} no longer exists"),
        )
    })
}

/// Accept the move: kill the source, record it complete. See the module
/// docs.
async fn finish(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    target: SessionRow,
    partial: PartialRecord,
) -> Result<ResolveMoveReport, IpcError> {
    if target.status != "running" {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "the target session {}'s row status is {:?}; expected running to finish a partial move against it",
                target.id, target.status
            ),
        ));
    }
    let (Some(size), Some(mtime)) = (
        partial.source_transcript_size,
        partial.source_transcript_mtime,
    ) else {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "the partial recorded no source_transcript_size/_mtime for session {} (saw {:?}/{:?}); expected both, to know exactly what was copied — refusing rather than guessing",
                target.id, partial.source_transcript_size, partial.source_transcript_mtime
            ),
        ));
    };
    let branch = partial.branch.clone().ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "the partial recorded no branch for session {}; expected one to finish",
                target.id
            ),
        )
    })?;

    let source_id = source_id_for(&target, &partial)?;
    let source = fetch_source(store, source_id)?;
    refuse_if_differs(
        target.id,
        "from_host",
        partial.from_host.as_deref(),
        &source.host_alias,
    )?;

    let claude_id = target
        .claude_session_id
        .clone()
        .or_else(|| partial.claude_session_id.clone())
        .ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "neither the target row nor the partial recorded a claude_session_id for session {}; expected one to finish",
                    target.id
                ),
            )
        })?;
    let path = partial.source_transcript_path.clone().unwrap_or_default();

    let outcome = finalise_source(
        FinaliseArgs {
            source_row_id: source.id,
            source_host: &source.host_alias,
            source_tmux_name: &source.tmux_name,
            target_row_id: target.id,
            target_host: &target.host_alias,
            target_tmux_name: &target.tmux_name,
            claude_id: &claude_id,
            branch: &branch,
            stored_transcript: partial.source_transcript_path.as_deref(),
            copied: Located { size, mtime, path },
            transcript_bytes: size,
            keep_source: false,
            extra_detail: serde_json::json!({ "finished_from_partial": true }),
        },
        store,
        ssh,
        hooks,
        &carry::CarryReport::default(),
    )
    .await
    .map_err(wrap_finalise_error)?;

    Ok(ResolveMoveReport {
        action: ResolveMoveAction::Finish,
        source_session_id: source.id,
        target_session_id: target.id,
        from_host: source.host_alias,
        to_host: target.host_alias,
        source_killed: outcome.source_killed,
        target_killed: false,
        warnings: outcome.warnings,
    })
}

/// Discard the move: kill the target, leave the source running. Nothing on
/// the target's disk is ever touched — no script runs against it. See the
/// module docs.
async fn undo(
    store: &Mutex<Store>,
    hooks: &dyn MoveHooks,
    target: SessionRow,
    partial: PartialRecord,
) -> Result<ResolveMoveReport, IpcError> {
    let expected_turn = partial.to_turn_seq.ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "the partial recorded no to_turn_seq for session {} (saw null); expected it, to verify the target has not since taken a turn — refusing rather than guessing",
                target.id
            ),
        )
    })?;
    if target.turn_seq != expected_turn {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "the target session {}'s turn_seq is {}; expected {expected_turn} (what the partial recorded) — it has taken a turn since the move stopped, so undoing now would discard it",
                target.id, target.turn_seq
            ),
        ));
    }
    if let (Some(expected_at), Some(actual_at)) = (partial.to_last_turn_at, target.last_turn_at) {
        if actual_at != expected_at {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "the target session {}'s last_turn_at is {actual_at}; expected {expected_at} (what the partial recorded) — it has taken a turn since the move stopped, so undoing now would discard it",
                    target.id
                ),
            ));
        }
    }
    let status = target.claude_status.as_deref();
    if status != Some("idle") {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "the target session {}'s claude_status is {:?}; expected idle to undo — a turn in progress (or a question it is waiting on) would be discarded",
                target.id, status
            ),
        ));
    }

    let source_id = source_id_for(&target, &partial)?;
    let source = fetch_source(store, source_id)?;
    refuse_if_differs(
        target.id,
        "from_host",
        partial.from_host.as_deref(),
        &source.host_alias,
    )?;

    // The kill only — no worktree, transcript, carried file or script of any
    // kind touches the target.
    hooks
        .kill_tmux_session(store, &target.host_alias, &target.tmux_name)
        .await?;

    // Fix round 1 / Finding 3: the kill already happened by this point, so a
    // poisoned store mutex here must not turn a completed kill into a hard
    // error — best-effort, exactly like `finalise_source`'s own post-kill
    // bookkeeping at the equivalent point. The module docs' "every refusal
    // returns before any kill" stays literally true: this is no longer a
    // refusal path at all.
    let detail = serde_json::json!({
        "from_host": source.host_alias,
        "to_host": target.host_alias,
        "from_session_id": source.id,
        "to_session_id": target.id,
        "claude_session_id": target.claude_session_id,
        "branch": partial.branch,
    })
    .to_string();
    if let Ok(s) = store.lock() {
        for sid in [source.id, target.id] {
            if let Err(e) = s.insert_session_event(sid, EVENT_MOVE_UNDONE, Some(&detail)) {
                tracing::warn!(
                    kind = EVENT_MOVE_UNDONE,
                    session_id = sid,
                    error = %e,
                    "[event] insert failed"
                );
            }
        }
    } else {
        tracing::warn!(
            source_id = source.id,
            target_id = target.id,
            "[resolve_move] store mutex poisoned after killing the target; session_move_undone was not recorded"
        );
    }

    Ok(ResolveMoveReport {
        action: ResolveMoveAction::Undo,
        source_session_id: source.id,
        target_session_id: target.id,
        from_host: source.host_alias,
        to_host: target.host_alias,
        source_killed: false,
        target_killed: true,
        warnings: Vec::new(),
    })
}

/// Finish or undo a partial move (see the module docs) over the real ssh
/// client.
pub async fn resolve_move(
    args: ResolveMoveArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<ResolveMoveReport, IpcError> {
    let hooks = RealHooks { ssh };
    resolve_move_with(args, store, &**ssh, &hooks).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::move_session::TargetWorkspace;
    use crate::ssh_fake::{FakeSsh, Match, Reply};

    // Pre-flight ruling R3: `mod.rs`'s `mod tests` is private and
    // `#[cfg(test)]`, so this module defines its own constants rather than
    // widening that one to share three values.
    const SID: &str = "550e8400-e29b-41d4-a716-446655440000";
    const TRANSCRIPT_LEN: usize = 128;
    const MTIME: i64 = 1_700_000_000;
    const SRC_TRANSCRIPT_PATH: &str = "/home/a/.claude/projects/p/c.jsonl";

    /// Two rows and a `session_move_partial` event between them: what the
    /// store looks like after a move stopped with the target running. With
    /// `unresolved = false` a later `session_moved` marks it already resolved.
    /// Uses the same `Store` calls as `mod.rs`'s `fixture_on`.
    fn partial_fixture(unresolved: bool) -> (Mutex<Store>, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        for h in ["alpha", "beta"] {
            s.insert_host(h, None).unwrap();
            s.update_host_probe(h, true, None, None, 1).unwrap();
            s.set_host_provisioned(h, true).unwrap();
        }
        let pid = s.upsert_project("o", "r", "/local/o/r").unwrap();
        let wid = s
            .upsert_worktree(
                pid,
                "feat",
                "/local/o/r/.claude/worktrees/feat",
                Some("feat"),
            )
            .unwrap();
        let source = s
            .upsert_session(
                "dev-o-r--feat",
                "alpha",
                Some(pid),
                Some(wid),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        let target = s
            .upsert_session(
                "dev-o-r--feat",
                "beta",
                Some(pid),
                Some(wid),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        s.set_claude_session_id(source, SID).unwrap();
        s.set_claude_session_id(target, SID).unwrap();
        s.set_claude_status_by_session_id(SID, "idle").unwrap();
        s.set_parent_session_id(target, Some(source)).unwrap();
        let detail = serde_json::json!({
            "step": "killing the source dev-o-r--feat on alpha",
            "to_host": "beta",
            "from_host": "alpha",
            "from_session_id": source,
            "to_session_id": target,
            "cause_code": "E_SSH",
            "to_tmux_name": "dev-o-r--feat",
            "claude_session_id": SID,
            "branch": "feat",
            "source_transcript_size": TRANSCRIPT_LEN,
            "source_transcript_mtime": MTIME,
            "source_transcript_path": SRC_TRANSCRIPT_PATH,
            "to_turn_seq": 0,
            "to_last_turn_at": serde_json::Value::Null,
        })
        .to_string();
        for id in [source, target] {
            s.insert_session_event(id, EVENT_MOVE_PARTIAL, Some(&detail))
                .unwrap();
        }
        if !unresolved {
            for id in [source, target] {
                s.insert_session_event(id, EVENT_MOVED, Some(&detail))
                    .unwrap();
            }
        }
        (Mutex::new(s), source, target)
    }

    /// Like [`partial_fixture`], but the target's `claude_status` is left at
    /// its default `NULL` (never reconciled) instead of `idle`. Fix round 1 /
    /// Finding 4: a target `resolve_move` has never seen a hook for is not
    /// idle either. `set_claude_status_by_session_id` is keyed by the shared
    /// `claude_session_id`, so it would set both rows — the target here is
    /// simply never given one, leaving its `claude_status` column untouched.
    fn partial_fixture_target_status_none() -> (Mutex<Store>, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        for h in ["alpha", "beta"] {
            s.insert_host(h, None).unwrap();
            s.update_host_probe(h, true, None, None, 1).unwrap();
            s.set_host_provisioned(h, true).unwrap();
        }
        let pid = s.upsert_project("o", "r", "/local/o/r").unwrap();
        let wid = s
            .upsert_worktree(
                pid,
                "feat",
                "/local/o/r/.claude/worktrees/feat",
                Some("feat"),
            )
            .unwrap();
        let source = s
            .upsert_session(
                "dev-o-r--feat",
                "alpha",
                Some(pid),
                Some(wid),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        let target = s
            .upsert_session(
                "dev-o-r--feat",
                "beta",
                Some(pid),
                Some(wid),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        s.set_claude_session_id(source, SID).unwrap();
        s.set_claude_status_by_session_id(SID, "idle").unwrap();
        s.set_parent_session_id(target, Some(source)).unwrap();
        let detail = serde_json::json!({
            "step": "killing the source dev-o-r--feat on alpha",
            "to_host": "beta",
            "from_host": "alpha",
            "from_session_id": source,
            "to_session_id": target,
            "cause_code": "E_SSH",
            "to_tmux_name": "dev-o-r--feat",
            "claude_session_id": SID,
            "branch": "feat",
            "source_transcript_size": TRANSCRIPT_LEN,
            "source_transcript_mtime": MTIME,
            "source_transcript_path": SRC_TRANSCRIPT_PATH,
            "to_turn_seq": 0,
            "to_last_turn_at": serde_json::Value::Null,
        })
        .to_string();
        for id in [source, target] {
            s.insert_session_event(id, EVENT_MOVE_PARTIAL, Some(&detail))
                .unwrap();
        }
        (Mutex::new(s), source, target)
    }

    /// Minimal `MoveHooks`: `resolve_move` never calls
    /// `ensure_target_workspace` / `start_target` / `refresh_host` /
    /// `source_cwd_hint`, so those are `unreachable!` — a call to one would
    /// mean this task grew a dependency it should not have.
    struct FakeHooks {
        killed: Mutex<Vec<(String, String)>>,
        /// Fix round 1 / Finding 3: poison the store's mutex right after a
        /// successful kill, to prove the post-kill bookkeeping can no longer
        /// turn a completed kill into a hard error.
        poison_after_kill: bool,
    }

    impl FakeHooks {
        fn new(_fake: &FakeSsh) -> Self {
            Self {
                killed: Mutex::new(Vec::new()),
                poison_after_kill: false,
            }
        }

        fn poisoning(_fake: &FakeSsh) -> Self {
            Self {
                killed: Mutex::new(Vec::new()),
                poison_after_kill: true,
            }
        }

        fn killed_any(&self) -> bool {
            !self.killed.lock().unwrap().is_empty()
        }
    }

    #[async_trait::async_trait]
    impl MoveHooks for FakeHooks {
        async fn source_cwd_hint(&self, _: &Mutex<Store>, _: &SessionRow) -> Option<String> {
            unreachable!("resolve_move never derives a workspace cwd")
        }
        async fn ensure_target_workspace(
            &self,
            _: &Mutex<Store>,
            _: TargetWorkspace<'_>,
        ) -> Result<String, IpcError> {
            unreachable!("resolve_move never touches the workspace")
        }
        async fn start_target(
            &self,
            _host: &str,
            _tmux_name: &str,
            _cwd: &str,
            _pane_cmd: &str,
        ) -> Result<(), IpcError> {
            unreachable!("resolve_move never starts a session")
        }
        async fn refresh_host(&self, _: &Mutex<Store>, _: &str) -> Result<(), IpcError> {
            unreachable!("resolve_move never reconciles a host")
        }
        async fn kill_tmux_session(
            &self,
            store: &Mutex<Store>,
            host: &str,
            tmux_name: &str,
        ) -> Result<(), IpcError> {
            self.killed
                .lock()
                .unwrap()
                .push((host.to_string(), tmux_name.to_string()));
            if self.poison_after_kill {
                // A thread that panics while holding a `std::sync::Mutex`
                // poisons it on unwind; `catch_unwind` stops that unwind
                // from also taking down the test. This simulates some
                // unrelated panic landing between the kill and the
                // bookkeeping that follows it.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _guard = store.lock().unwrap();
                    panic!("poisoning the store mutex for a test");
                }));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn finish_kills_the_source_and_records_the_move_as_complete() {
        let (store, source_id, target_id) = partial_fixture(true);
        let fake = FakeSsh::new();
        // `finalise_source`'s own re-check locates the source transcript
        // again before killing it: the brief's sketch omits this, but a real
        // `SshExec` call happens here (that recheck is exactly what refuses
        // a source that kept working — see the next test), so the fake needs
        // a matching reply for the happy path. Sizes/mtime/path match what
        // the partial recorded, i.e. nothing changed since the copy.
        fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:locate"),
            Reply::ok(&format!(
                "{TRANSCRIPT_LEN}\t{MTIME}\t{SRC_TRANSCRIPT_PATH}\n"
            )),
        );
        let hooks = FakeHooks::new(&fake);
        let rep = resolve_move_with(
            ResolveMoveArgs {
                session_id: target_id,
                action: ResolveMoveAction::Finish,
            },
            &store,
            &fake,
            &hooks,
        )
        .await
        .expect("finish");
        assert!(rep.source_killed);
        assert!(!rep.target_killed);
        assert_eq!(rep.source_session_id, source_id);
        assert_eq!(rep.target_session_id, target_id);
        for id in [source_id, target_id] {
            let ev = store.lock().unwrap().list_session_events(id, 50).unwrap();
            let e = ev
                .iter()
                .find(|e| e.kind == EVENT_MOVED)
                .expect("session_moved");
            let d: serde_json::Value = serde_json::from_str(e.detail.as_deref().unwrap()).unwrap();
            assert_eq!(d["finished_from_partial"], true);
        }
    }

    #[tokio::test]
    async fn finish_refuses_a_source_that_took_a_turn_after_the_partial() {
        let (store, _source_id, target_id) = partial_fixture(true);
        let fake = FakeSsh::new();
        // The source transcript is now bigger than the partial recorded.
        fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:locate"),
            Reply::ok("99999\t1700009999\t/p/c.jsonl\n"),
        );
        let hooks = FakeHooks::new(&fake);
        let err = resolve_move_with(
            ResolveMoveArgs {
                session_id: target_id,
                action: ResolveMoveAction::Finish,
            },
            &store,
            &fake,
            &hooks,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(err.message.contains("99999"), "{}", err.message);
        assert!(!hooks.killed_any(), "nothing is killed on a refusal");
    }

    #[tokio::test]
    async fn undo_kills_the_target_and_leaves_its_files_alone() {
        let (store, source_id, target_id) = partial_fixture(true);
        let fake = FakeSsh::new();
        let hooks = FakeHooks::new(&fake);
        let rep = resolve_move_with(
            ResolveMoveArgs {
                session_id: target_id,
                action: ResolveMoveAction::Undo,
            },
            &store,
            &fake,
            &hooks,
        )
        .await
        .expect("undo");
        assert!(rep.target_killed);
        assert!(!rep.source_killed);
        for id in [source_id, target_id] {
            let ev = store.lock().unwrap().list_session_events(id, 50).unwrap();
            assert!(ev.iter().any(|e| e.kind == EVENT_MOVE_UNDONE), "{id}");
            assert!(!ev.iter().any(|e| e.kind == EVENT_MOVED), "{id}");
        }
        // Not one call reached the target host at all: no worktree,
        // transcript or carried file is ever touched by an undo. Stronger
        // than checking `Call::script()` (which only recognizes `bash -lc`
        // argv and would miss a stray `upload_file` or direct-argv exec) —
        // every call is recorded regardless of shape, so this is exact.
        assert!(
            fake.calls_for("beta").is_empty(),
            "undo made a call against the target host"
        );
        assert!(hooks
            .killed
            .lock()
            .unwrap()
            .contains(&("beta".to_string(), "dev-o-r--feat".to_string())));
    }

    #[tokio::test]
    async fn undo_refuses_a_target_that_has_taken_a_turn_or_is_not_idle() {
        fn bump_turn(s: &Store, id: i64) {
            // The Stop hook's own write: bumps `turn_seq`, sets
            // `last_turn_at` — the real API a resolution has to respect,
            // not a test-only setter.
            s.record_stop_hook_for_row(id).unwrap();
        }
        fn make_busy(s: &Store, id: i64) {
            // The UserPromptSubmit hook's own write: `claude_status` becomes
            // `working` — again the real API, not a test-only setter.
            s.record_prompt_submit_hook_for_row(id).unwrap();
        }
        fn make_blocked(s: &Store, id: i64) {
            // The Notification hook's own write: `claude_status` becomes
            // `blocked` — the spec names this explicitly as "not idle" (fix
            // round 1 / Finding 4), so it gets its own case.
            s.record_notification_hook_for_row(
                id,
                crate::service::pane_intel::ClaudeStatus::Blocked,
                None,
            )
            .unwrap();
        }
        for (label, edit) in [
            ("turn", bump_turn as fn(&Store, i64)),
            ("busy", make_busy as fn(&Store, i64)),
            ("blocked", make_blocked as fn(&Store, i64)),
        ] {
            let (store, _src, target_id) = partial_fixture(true);
            {
                let s = store.lock().unwrap();
                edit(&s, target_id);
            }
            let fake = FakeSsh::new();
            let hooks = FakeHooks::new(&fake);
            let err = resolve_move_with(
                ResolveMoveArgs {
                    session_id: target_id,
                    action: ResolveMoveAction::Undo,
                },
                &store,
                &fake,
                &hooks,
            )
            .await
            .unwrap_err();
            assert_eq!(err.code, codes::E_INVALID_STATE, "{label}");
            assert!(!hooks.killed_any(), "{label}: nothing killed on a refusal");
        }

        // A target `resolve_move` has never seen a hook for (claude_status
        // still NULL) is not idle either.
        let (store, _src, target_id) = partial_fixture_target_status_none();
        let fake = FakeSsh::new();
        let hooks = FakeHooks::new(&fake);
        let err = resolve_move_with(
            ResolveMoveArgs {
                session_id: target_id,
                action: ResolveMoveAction::Undo,
            },
            &store,
            &fake,
            &hooks,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE, "none");
        assert!(!hooks.killed_any(), "none: nothing killed on a refusal");
    }

    #[tokio::test]
    async fn resolve_move_refuses_when_given_the_source_session_instead_of_the_target() {
        // Finding 1: `record_partial` writes the identical
        // `session_move_partial` on BOTH rows, so the source's own timeline
        // also carries an unresolved partial. Calling `resolve_move` with
        // the source's id must not silently treat the source as the target.
        let (store, source_id, target_id) = partial_fixture(true);
        let fake = FakeSsh::new();
        let hooks = FakeHooks::new(&fake);
        for action in [ResolveMoveAction::Finish, ResolveMoveAction::Undo] {
            let err = resolve_move_with(
                ResolveMoveArgs {
                    session_id: source_id,
                    action,
                },
                &store,
                &fake,
                &hooks,
            )
            .await
            .unwrap_err();
            assert_eq!(err.code, codes::E_INVALID_STATE, "{action:?}");
            assert!(err.message.contains("SOURCE"), "{}", err.message);
            assert!(
                err.message.contains(&target_id.to_string()),
                "{}",
                err.message
            );
            assert!(err.message.contains("beta"), "{}", err.message);
        }
        assert!(!hooks.killed_any(), "nothing killed on a refusal");
    }

    #[tokio::test]
    async fn resolve_move_refuses_when_recorded_identity_fields_disagree_with_the_live_row() {
        // Finding 2: `to_host` / `to_tmux_name` are part of the session's
        // unique key and are never rewritten once set, so for a genuine
        // handle they always agree with the live row. A disagreement (a
        // corrupted event, or one orphaned by a deleted-and-reused row id)
        // must refuse, not warn-and-proceed.
        for bad_field in ["to_host", "to_tmux_name"] {
            let (store, source_id, target_id) = partial_fixture(true);
            let mut detail = serde_json::json!({
                "step": "x",
                "to_host": "beta",
                "from_host": "alpha",
                "from_session_id": source_id,
                "to_session_id": target_id,
                "to_tmux_name": "dev-o-r--feat",
                "claude_session_id": SID,
                "branch": "feat",
                "source_transcript_size": TRANSCRIPT_LEN,
                "source_transcript_mtime": MTIME,
                "source_transcript_path": SRC_TRANSCRIPT_PATH,
                "to_turn_seq": 0,
                "to_last_turn_at": serde_json::Value::Null,
            });
            detail[bad_field] = serde_json::Value::String("wrong".to_string());
            {
                let s = store.lock().unwrap();
                s.insert_session_event(target_id, EVENT_MOVE_PARTIAL, Some(&detail.to_string()))
                    .unwrap();
            }
            let fake = FakeSsh::new();
            // Without the identity check, Finish would sail through to a
            // real (successful) kill — this happy-path locate reply is what
            // makes that failure mode visible instead of being masked by an
            // unrelated ssh-recheck parse error.
            fake.on_host(
                "alpha",
                Match::script_contains("# cf-move:locate"),
                Reply::ok(&format!(
                    "{TRANSCRIPT_LEN}\t{MTIME}\t{SRC_TRANSCRIPT_PATH}\n"
                )),
            );
            let hooks = FakeHooks::new(&fake);
            let err = resolve_move_with(
                ResolveMoveArgs {
                    session_id: target_id,
                    action: ResolveMoveAction::Finish,
                },
                &store,
                &fake,
                &hooks,
            )
            .await
            .unwrap_err();
            assert_eq!(err.code, codes::E_INVALID_STATE, "{bad_field}");
            assert!(
                !hooks.killed_any(),
                "{bad_field}: nothing killed on a refusal"
            );
        }
    }

    #[tokio::test]
    async fn resolve_move_refuses_when_parent_session_id_and_recorded_from_session_id_disagree() {
        // Finding 2's other half: `source_id_for` must refuse a disagreement
        // rather than silently pick one of the two.
        let (store, source_id, target_id) = partial_fixture(true);
        {
            let s = store.lock().unwrap();
            s.set_parent_session_id(target_id, Some(source_id + 999))
                .unwrap();
        }
        let fake = FakeSsh::new();
        let hooks = FakeHooks::new(&fake);
        let err = resolve_move_with(
            ResolveMoveArgs {
                session_id: target_id,
                action: ResolveMoveAction::Finish,
            },
            &store,
            &fake,
            &hooks,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(!hooks.killed_any());
    }

    #[tokio::test]
    async fn undo_records_the_kill_even_when_the_post_kill_bookkeeping_cannot_lock_the_store() {
        // Finding 3: the kill already happened by the time the bookkeeping
        // runs, so a poisoned mutex there must not turn a completed kill
        // into a hard error (mirrors `finalise_source`'s own post-kill
        // bookkeeping, which is best-effort for the same reason).
        let (store, _source_id, target_id) = partial_fixture(true);
        let fake = FakeSsh::new();
        let hooks = FakeHooks::poisoning(&fake);
        let rep = resolve_move_with(
            ResolveMoveArgs {
                session_id: target_id,
                action: ResolveMoveAction::Undo,
            },
            &store,
            &fake,
            &hooks,
        )
        .await
        .expect("the kill must be reported even if bookkeeping can't lock the store");
        assert!(rep.target_killed);
        assert!(hooks.killed_any());
    }

    #[tokio::test]
    async fn a_session_with_no_unresolved_partial_is_refused() {
        // `unresolved: false` records a `session_moved` after the partial.
        let (store, _src, target_id) = partial_fixture(false);
        let fake = FakeSsh::new();
        let hooks = FakeHooks::new(&fake);
        for action in [ResolveMoveAction::Finish, ResolveMoveAction::Undo] {
            let err = resolve_move_with(
                ResolveMoveArgs {
                    session_id: target_id,
                    action,
                },
                &store,
                &fake,
                &hooks,
            )
            .await
            .unwrap_err();
            assert_eq!(err.code, codes::E_INVALID_STATE);
            assert!(err.message.contains("not a partial"), "{}", err.message);
        }
    }

    #[tokio::test]
    async fn a_partial_missing_the_fact_an_action_needs_is_refused_not_guessed() {
        let (store, source_id, target_id) = partial_fixture(true);
        // Rewrite the partial detail with a null source_transcript_size.
        // (Insert a newer `session_move_partial` whose detail lacks it.)
        // Every identifying field here matches the fixture's real rows —
        // this test isolates the missing-transcript-facts refusal, not the
        // identity checks (fix round 1 / Findings 1-2) added after it.
        {
            let s = store.lock().unwrap();
            let detail = format!(
                r#"{{"step":"x","to_host":"beta","from_host":"alpha","from_session_id":{source_id},"to_session_id":{target_id},"source_transcript_size":null,"source_transcript_mtime":null,"to_turn_seq":null,"to_last_turn_at":null,"claude_session_id":"c","branch":"feat","to_tmux_name":"dev-o-r--feat"}}"#
            );
            s.insert_session_event(target_id, EVENT_MOVE_PARTIAL, Some(&detail))
                .unwrap();
        }
        let fake = FakeSsh::new();
        let hooks = FakeHooks::new(&fake);
        let err = resolve_move_with(
            ResolveMoveArgs {
                session_id: target_id,
                action: ResolveMoveAction::Finish,
            },
            &store,
            &fake,
            &hooks,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(!hooks.killed_any());
    }

    // ── `newest_unresolved` is pure: exercised directly, no store ──────────

    fn ev(id: i64, kind: &str, detail: Option<&str>) -> crate::store::SessionEvent {
        crate::store::SessionEvent {
            id,
            session_id: 1,
            at: id,
            kind: kind.to_string(),
            detail: detail.map(String::from),
            claude_session_id: None,
        }
    }

    #[test]
    fn newest_unresolved_stops_at_the_first_resolving_event() {
        let partial_detail = r#"{"branch":"feat"}"#;
        // Newest-first: a session_moved after the partial means resolved.
        let resolved = [
            ev(2, EVENT_MOVED, Some(partial_detail)),
            ev(1, EVENT_MOVE_PARTIAL, Some(partial_detail)),
        ];
        assert!(newest_unresolved(&resolved).is_none());

        let undone = [
            ev(2, EVENT_MOVE_UNDONE, Some(partial_detail)),
            ev(1, EVENT_MOVE_PARTIAL, Some(partial_detail)),
        ];
        assert!(newest_unresolved(&undone).is_none());

        // No partial at all.
        assert!(newest_unresolved(&[ev(1, "status_change", None)]).is_none());

        // An unresolved partial, behind unrelated events: found, and parsed.
        let unresolved = [
            ev(3, "status_change", None),
            ev(2, EVENT_MOVE_PARTIAL, Some(partial_detail)),
            ev(1, "status_change", None),
        ];
        let found = newest_unresolved(&unresolved).expect("found");
        assert_eq!(found.branch.as_deref(), Some("feat"));
    }
}
