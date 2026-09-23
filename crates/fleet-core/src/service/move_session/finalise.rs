//! The move's final source step (module docs step 4): re-check the source
//! transcript, kill the source (unless `keep_source`), take one more look,
//! carry the usage totals and cursor, and record `session_moved` on both
//! rows.
//!
//! Extracted as its own entry point so a later recovery (finishing a partial
//! move) can run exactly this code: [`finalise_source`] returns plain errors
//! rather than `E_MOVE_PARTIAL` ones, so each caller decides for itself what
//! an error here means — `move_session` wraps them as partial, a recovery
//! wraps them as `E_INVALID_STATE`. The step it failed at travels in
//! `details.step` on the returned error either way, exactly as `partial(…)`
//! itself records it, so a caller need not guess it back out of the message.

use std::sync::Mutex;

use crate::ipc_error::{codes, lock, IpcError};
use crate::store::Store;

use super::{carry, locate_on, Located, MoveHooks, EVENT_MOVED};
use crate::ssh::SshExec;

/// What [`finalise_source`] needs. Every field is a plain value or reference
/// the move already has in scope by the time the target session exists —
/// nothing here is re-derived.
pub(super) struct FinaliseArgs<'a> {
    pub source_row_id: i64,
    pub source_host: &'a str,
    pub source_tmux_name: &'a str,
    pub target_row_id: i64,
    pub target_host: &'a str,
    pub target_tmux_name: &'a str,
    pub claude_id: &'a str,
    pub branch: &'a str,
    pub stored_transcript: Option<&'a str>,
    /// The transcript as the copy was taken: the kill is refused if the
    /// source has moved past it.
    pub copied: Located,
    pub transcript_bytes: u64,
    pub keep_source: bool,
    /// Extra keys merged into the `session_moved` detail (a later recovery
    /// passes `{"finished_from_partial": true}`).
    pub extra_detail: serde_json::Value,
}

/// What the step did.
pub(super) struct FinaliseOutcome {
    pub source_killed: bool,
    pub warnings: Vec<String>,
}

/// Tag a plain error with the step it failed at, the same way `partial(…)`
/// itself records `details.step` — the caller reads it back out to build its
/// own wrapped error, so nothing here has to guess or duplicate the wrapping.
fn tag_step(step: &str, e: IpcError) -> IpcError {
    IpcError {
        details: Some(serde_json::json!({ "step": step })),
        ..e
    }
}

/// The final source step (see the module docs). Returns plain errors — the
/// caller decides how to wrap them (`E_MOVE_PARTIAL` for a fresh move,
/// `E_INVALID_STATE` for a recovery finishing a partial one).
///
/// `carried` is `None` for a caller that did not do the carrying — a
/// recovery finishing a partial move runs from the recorded event alone,
/// hours later, and has no carry report at all. The `session_moved` detail
/// then omits the `carried` key rather than writing a default one, which
/// would read as fact (zero commits, zero dirty entries, nothing carried)
/// and contradict the `session_move_partial` above it on the same timeline.
/// The timeline is the durable record recovery leans on; a gap in it is
/// honest, a fabricated zero is not.
pub(super) async fn finalise_source(
    a: FinaliseArgs<'_>,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    carried: Option<&carry::CarryReport>,
) -> Result<FinaliseOutcome, IpcError> {
    let mut warnings = Vec::new();
    let mut source_killed = false;
    if !a.keep_source {
        // The source kept running through the copy: if its transcript moved
        // on since, it took a turn the target does not have. Killing it would
        // lose that turn, so stop here with both sessions alive.
        let recheck = locate_on(ssh, a.source_host, a.stored_transcript, a.claude_id)
            .await
            .map_err(|e| tag_step("re-checking the source transcript", e))?;
        if recheck.size != a.copied.size || recheck.mtime != a.copied.mtime {
            return Err(tag_step(
                "source transcript changed after copy",
                IpcError::new(
                    codes::E_INVALID_STATE,
                    format!(
                        "the source transcript went from {} to {} bytes (mtime {} -> {}) after it was copied, so the source took a turn the target does not have; kill the target session {} on {} and retry move_session once the source is idle",
                        a.copied.size, recheck.size, a.copied.mtime, recheck.mtime, a.target_tmux_name, a.target_host
                    ),
                ),
            ));
        }
        // Snapshot the source's lifetime usage and usage cursor before the
        // kill drops its row.
        let (source_usage, source_cursor) = store
            .lock()
            .ok()
            .map(|s| {
                (
                    s.get_session_by_id(a.source_row_id)
                        .ok()
                        .flatten()
                        .map(|r| r.usage),
                    s.usage_cursor(a.source_row_id).ok().flatten(),
                )
            })
            .unwrap_or((None, None));
        // The address embeds the host alias and the row id changes on a
        // move, so the durable thing is the participant: re-point it BEFORE
        // the source is killed, and every message already addressed to this
        // session follows to the target instead of being tombstoned with the
        // source row (see `store::sessions::delete_session`). No participant
        // exists when nothing was ever addressed to the source — nothing to
        // carry, so this is a no-op then.
        let repointed: Option<i64> = {
            let s = lock(store)?;
            match s.participant_for_session(a.source_row_id)? {
                Some(p) => {
                    s.repoint_participant(p.id, a.target_row_id)
                        .map_err(|e| tag_step("re-pointing the participant", e))?;
                    Some(p.id)
                }
                None => None,
            }
        };
        if let Err(e) = hooks
            .kill_tmux_session(store, a.source_host, a.source_tmux_name)
            .await
        {
            // The kill failed, so BOTH sessions are alive and the caller is
            // about to report `E_MOVE_PARTIAL` — a state that can last hours,
            // until `resolve_move` runs. The re-point above must not stand
            // through it: with the identity on the target, the still-running
            // source has no participant at all, its `list_inbox` reads empty,
            // and every message addressed to it is delivered to the target
            // instead. Put it back. (A collision merge the re-point may have
            // performed is not undone — the target's own mail has already
            // been folded into this identity, and re-splitting it would be
            // guesswork; it follows the identity back to the source, which is
            // where the live endpoint is.) A failure here can only be logged:
            // the kill error is the one that has to reach the caller.
            if let Some(pid) = repointed {
                match lock(store).and_then(|s| s.repoint_participant(pid, a.source_row_id)) {
                    Ok(()) => tracing::info!(
                        participant = pid,
                        session_id = a.source_row_id,
                        "move_session: the kill failed, so the participant is back on the source"
                    ),
                    Err(re) => tracing::warn!(
                        participant = pid,
                        session_id = a.source_row_id,
                        error = %re.message,
                        "move_session: the kill failed AND the participant could not be put back; mail for the source lands on the target until resolve_move runs"
                    ),
                }
            }
            return Err(tag_step(
                &format!(
                    "killing the source {} on {}",
                    a.source_tmux_name, a.source_host
                ),
                e,
            ));
        }
        source_killed = true;
        // One last look: a write between the final check and the kill means
        // the target may lack the source's last turn. Nothing is left to
        // undo, so say so in the report.
        match locate_on(ssh, a.source_host, a.stored_transcript, a.claude_id).await {
            Ok(after) if after.size != recheck.size || after.mtime != recheck.mtime => {
                warnings.push(format!(
                    "the source wrote after the final check and before the kill (transcript {} -> {} bytes, mtime {} -> {}); the target may be missing its last turn — compare the two with session_transcript",
                    recheck.size, after.size, recheck.mtime, after.mtime
                ));
            }
            Ok(_) => {}
            Err(e) => warnings.push(format!(
                "could not re-check the source transcript after the kill ({}: {}); the target may be missing its last turn",
                e.code, e.message
            )),
        }
        // The spend follows the session. Never under keep_source: both rows
        // stay live there and would report it twice.
        if let Ok(s) = store.lock() {
            if let Some(u) = source_usage.as_ref() {
                if let Err(e) = s.add_usage_totals(a.target_row_id, u) {
                    tracing::warn!(
                        "move_session: carrying usage totals to {} failed: {e}",
                        a.target_row_id
                    );
                }
            }
            // A source usage pass between the inherit and the kill counted
            // lines the target's cursor still points before: catch up.
            if let Some(c) = source_cursor.as_ref() {
                if let Err(e) = s.raise_usage_cursor(a.target_row_id, c) {
                    tracing::warn!(
                        "move_session: raising the usage cursor on {} failed: {e}",
                        a.target_row_id
                    );
                }
            }
        }
    }

    // The move is complete: record it on both rows.
    let mut detail = serde_json::json!({
        "from_host": a.source_host,
        "to_host": a.target_host,
        "from_session_id": a.source_row_id,
        "to_session_id": a.target_row_id,
        "claude_session_id": a.claude_id,
        "branch": a.branch,
        "bytes": a.transcript_bytes,
        "kept_source": a.keep_source,
        "source_killed": source_killed,
    });
    if let (Some(base), Some(c)) = (detail.as_object_mut(), carried) {
        base.insert("carried".into(), serde_json::json!(c));
    }
    if let (Some(base), serde_json::Value::Object(extra)) = (detail.as_object_mut(), a.extra_detail)
    {
        // `Map::extend` would silently overwrite a canonical key
        // (`source_killed`, `from_session_id`, …) if a future caller's
        // `extra_detail` ever collided with one; skip the key instead and
        // flag it in debug builds, so a collision is loud in tests rather
        // than a silently rewritten timeline event in production.
        for (k, v) in extra {
            if base.contains_key(&k) {
                debug_assert!(
                    false,
                    "finalise_source: extra_detail key {k:?} collides with a canonical session_moved field and was ignored"
                );
                continue;
            }
            base.insert(k, v);
        }
    }
    let detail = detail.to_string();
    if let Ok(s) = store.lock() {
        for sid in [a.source_row_id, a.target_row_id] {
            if let Err(e) = s.insert_session_event(sid, EVENT_MOVED, Some(&detail)) {
                tracing::warn!(
                    kind = EVENT_MOVED,
                    session_id = sid,
                    error = %e,
                    "[event] insert failed"
                );
            }
        }
    }

    Ok(FinaliseOutcome {
        source_killed,
        warnings,
    })
}
