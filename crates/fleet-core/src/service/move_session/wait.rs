//! Wait, in-process, for a busy source session to go idle, then run the real
//! move — Transfer slice 3c ("transfer when the session finishes").
//!
//! The wait is cancellable and bounded (`move.wait_max_mins`, default
//! [`DEFAULT_WAIT_MAX_MINS`]); it never holds the [`Store`] mutex across its
//! sleep, since it can run for hours. A pending wait is tracked two ways: an
//! in-process [`CancellationToken`] registry (this process's own waiters, so
//! `cancel_wait` can stop one immediately) and a `session_move_waiting`
//! timeline event opened by `begin_wait` and closed by `run_wait` (or, when
//! the process restarted mid-wait and nothing is left to close it,
//! [`sweep_unresolved_waits`] at the next startup).
//!
//! Everything here takes borrowed handles (`&Mutex<Store>`, `&dyn SshExec`,
//! `&dyn MoveHooks`) and is `await`ed directly, so it runs end-to-end against
//! the same fixtures as the rest of `move_session` — only Task 3's public
//! entry point spawns a `'static` task around [`run_wait`].

use super::{
    move_session_with, moves_in_flight, MoveHooks, MoveOptions, MoveOutcome, MoveSessionArgs,
    SOURCE_NOT_IDLE,
};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::settings;
use crate::service::tasks::{wait_for_session_with, WaitCond};
use crate::ssh::SshExec;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Mutex;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Timeline event recorded when a wait begins.
pub const EVENT_MOVE_WAITING: &str = "session_move_waiting";
/// Timeline event recorded when a wait ends, for any reason.
pub const EVENT_MOVE_WAIT_ENDED: &str = "session_move_wait_ended";

/// `settings` key [`crate::service::settings::MOVE_WAIT_MAX_MINS`]: absent /
/// unparseable / 0 → this default; the setting itself caps at
/// `MOVE_WAIT_MAX_MINS_MAX`.
pub const DEFAULT_WAIT_MAX_MINS: u64 = 240;

/// Why a wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitEnd {
    /// The source went idle and the move completed.
    Moved,
    /// The source went idle, the move ran, and it partially completed
    /// (`E_MOVE_PARTIAL`) — both sessions are left alive.
    Partial,
    /// The source went idle but the move was refused for a reason other than
    /// the source going busy again (which instead resumes the wait).
    Refused,
    /// `cancel_wait` fired this wait's token.
    Cancelled,
    /// The deadline passed with the source never going idle.
    TimedOut,
    /// The source session row was deleted while the wait was pending.
    SessionGone,
    /// Nothing was left to honour this wait: the hub restarted while it was
    /// pending (closed by [`sweep_unresolved_waits`] at the next startup), or
    /// its waiter was dropped or panicked before recording an end (closed by
    /// [`WaitGuard`]'s drop at once — the same state the sweep would find,
    /// recorded one restart earlier, so no new reason for the UI to learn).
    HubRestarted,
}

/// What a pending wait reports back to its caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveWaiting {
    pub session_id: i64,
    pub to_host: String,
    pub deadline_unix: i64,
}

// ── registry ─────────────────────────────────────────────────────────────

/// One entry per pending wait, keyed the same way as
/// [`super::moves_in_flight`] — `(store address, session id)` — so parallel
/// tests on separate in-memory stores cannot collide.
type WaitKey = (usize, i64);
type WaitRegistry = Mutex<HashMap<WaitKey, WaitEntry>>;

/// One pending wait's registry entry.
struct WaitEntry {
    token: CancellationToken,
    /// The waiter has started the real move: there is no wait left to
    /// cancel (a running move is not cancellable), so `cancel_wait` answers
    /// `false` — but the entry stays, so a second `begin_wait` is still
    /// refused while the move runs. Cleared again if the move is refused
    /// because the source went busy (the wait resumes).
    moving: bool,
    /// `session_move_wait_ended` was recorded for this wait — the guard's
    /// drop must not record a second one.
    ended: bool,
}

fn waits() -> &'static WaitRegistry {
    static WAITS: std::sync::OnceLock<WaitRegistry> = std::sync::OnceLock::new();
    WAITS.get_or_init(Default::default)
}

fn key_for(store: &Mutex<Store>, session_id: i64) -> WaitKey {
    (store as *const Mutex<Store> as usize, session_id)
}

/// Held for as long as a wait is pending; dropping it deregisters — the
/// startup sweep relies on this to tell a live wait from an orphaned one.
///
/// Holds the store (a borrow in tests, an `Arc` in the spawned waiter) so a
/// waiter dropped or panicking before [`run_wait`] recorded its end still
/// closes its `session_move_waiting`, as [`WaitEnd::HubRestarted`].
pub(super) struct WaitGuard<S: Deref<Target = Mutex<Store>>> {
    key: WaitKey,
    session_id: i64,
    token: CancellationToken,
    store: S,
}

impl<S: Deref<Target = Mutex<Store>>> WaitGuard<S> {
    pub(super) fn token(&self) -> &CancellationToken {
        &self.token
    }
}

impl<S: Deref<Target = Mutex<Store>>> Drop for WaitGuard<S> {
    fn drop(&mut self) {
        // A poisoned registry cannot tell us whether the end was recorded;
        // leave it to the startup sweep rather than risk writing it twice.
        let Ok(mut reg) = waits().lock() else { return };
        let entry = reg.remove(&self.key);
        drop(reg);
        if entry.is_some_and(|e| !e.ended) {
            // Synchronous and short: one insert under the store's sync lock.
            write_wait_ended(&self.store, self.session_id, WaitEnd::HubRestarted, None);
        }
    }
}

/// Flag this session's registry entry (if any) as mid-move or not.
fn set_moving(key: WaitKey, moving: bool) {
    if let Ok(mut reg) = waits().lock() {
        if let Some(e) = reg.get_mut(&key) {
            e.moving = moving;
        }
    }
}

pub(super) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Effective `move.wait_max_mins`: stored value if positive and registered
/// (capped at `MOVE_WAIT_MAX_MINS_MAX`), else [`DEFAULT_WAIT_MAX_MINS`] — the
/// same "get_string, parse, positive, else default" shape the move uses for
/// its other numeric settings (`SETTING_MAX_TRANSCRIPT_MB` and friends).
fn wait_max_mins(s: &Store) -> u64 {
    settings::get_string(s, settings::MOVE_WAIT_MAX_MINS)
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .map(|n| n.min(settings::MOVE_WAIT_MAX_MINS_MAX))
        .unwrap_or(DEFAULT_WAIT_MAX_MINS)
}

/// Register a wait for `args.session_id`, record `session_move_waiting`, and
/// return the guard, the deadline, and the `MoveWaiting` to answer with.
///
/// Refuses (`E_INVALID_STATE`) a session already waiting, or one with a move
/// currently in flight (queried, per [`super::moves_in_flight`] — the claim
/// itself is never taken here; the real move takes it on each retry inside
/// [`run_wait`]'s loop).
pub(super) fn begin_wait<S: Deref<Target = Mutex<Store>>>(
    args: &MoveSessionArgs,
    store: S,
) -> Result<(WaitGuard<S>, i64, MoveWaiting), IpcError> {
    let key = key_for(&store, args.session_id);

    {
        let flight = moves_in_flight().lock().map_err(|_| IpcError::lock())?;
        if flight.contains(&key) {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "a move of session {} is already in progress",
                    args.session_id
                ),
            ));
        }
    }

    let token = CancellationToken::new();
    {
        let mut reg = waits().lock().map_err(|_| IpcError::lock())?;
        if reg.contains_key(&key) {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("session {} is already waiting to move", args.session_id),
            ));
        }
        reg.insert(
            key,
            WaitEntry {
                token: token.clone(),
                moving: false,
                ended: false,
            },
        );
    }

    let mins = match lock(&store) {
        Ok(s) => wait_max_mins(&s),
        Err(e) => {
            if let Ok(mut reg) = waits().lock() {
                reg.remove(&key);
            }
            return Err(e);
        }
    };
    let deadline_unix = now_unix() + (mins as i64) * 60;

    let waiting = MoveWaiting {
        session_id: args.session_id,
        to_host: args.target_host_alias.clone(),
        deadline_unix,
    };
    let detail = serde_json::json!({
        "to_host": args.target_host_alias,
        "keep_source": args.keep_source,
        "strict": args.strict,
        "clean_target": args.clean_target,
        "deadline_unix": deadline_unix,
    })
    .to_string();
    if let Ok(s) = store.lock() {
        if let Err(err) = s.insert_session_event(args.session_id, EVENT_MOVE_WAITING, Some(&detail))
        {
            tracing::warn!(
                kind = EVENT_MOVE_WAITING,
                session_id = args.session_id,
                error = %err,
                "[event] insert failed"
            );
        }
    }
    let guard = WaitGuard {
        key,
        session_id: args.session_id,
        token,
        store,
    };
    Ok((guard, deadline_unix, waiting))
}

/// Cancel a pending wait. `true` if there was one registered and still
/// waiting — `false` once its waiter has started the real move (which a
/// cancel does not stop) or recorded its end.
///
/// Only fires the token — the registry entry is removed by the waiting
/// [`WaitGuard`]'s own drop once `run_wait` observes the cancellation and
/// returns, so a second call for the same session (once that drop has
/// happened) correctly reports nothing left to cancel.
pub(super) fn cancel_wait(store: &Mutex<Store>, session_id: i64) -> bool {
    let key = key_for(store, session_id);
    let Ok(reg) = waits().lock() else {
        return false;
    };
    match reg.get(&key) {
        Some(e) if !e.moving && !e.ended => {
            e.token.cancel();
            true
        }
        _ => false,
    }
}

/// The wait loop. `args` must carry `when: Now` and `dry_run: false` — this
/// is the move it will run once the source is idle, not a preview and not
/// another wait; the public `move_session` forces both when it spawns this.
/// Records `session_move_wait_ended` with the reason before returning it.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_wait(
    args: MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    opts: MoveOptions,
    token: &CancellationToken,
    deadline_unix: i64,
    poll: Duration,
) -> WaitEnd {
    run_wait_with(
        args,
        store,
        ssh,
        hooks,
        opts,
        token,
        deadline_unix,
        poll,
        WAIT_SLICE,
        &now_unix,
    )
    .await
}

/// Longest single idle poll between two wall-clock deadline checks.
pub(super) const WAIT_SLICE: Duration = Duration::from_secs(60);

/// [`run_wait`] with its wall clock (unix seconds) and poll slice injected,
/// so a test can move the wall clock without sleeping.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_wait_with(
    args: MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    opts: MoveOptions,
    token: &CancellationToken,
    deadline_unix: i64,
    poll: Duration,
    slice: Duration,
    clock: &(dyn Fn() -> i64 + Sync),
) -> WaitEnd {
    let mut refusal: Option<IpcError> = None;
    let end = loop {
        // The deadline is wall-clock (I3): a monotonic `Instant` stops while
        // a Mac sleeps, so a Friday wait would fire on Monday. Poll in slices
        // of at most `slice` and re-read the wall clock between them, so a
        // wake past the deadline ends the wait within one slice.
        let now = clock();
        if now >= deadline_unix {
            break WaitEnd::TimedOut;
        }
        let left = Duration::from_secs((deadline_unix - now) as u64).min(slice);
        let idle = tokio::select! {
            _ = token.cancelled() => break WaitEnd::Cancelled,
            r = wait_for_session_with(store, args.session_id, WaitCond::Idle, left, poll) => r,
        };
        match idle {
            Err(e) if e.code == codes::E_NOTFOUND => break WaitEnd::SessionGone,
            Err(e) => {
                refusal = Some(e);
                break WaitEnd::Refused;
            }
            // Not idle within this slice: back to the wall-clock check.
            Ok(o) if !o.satisfied => continue,
            Ok(_) => {}
        }
        // Idle: run the real move. A cancel from here on lets it finish —
        // cancelling a running move is out of scope — so the entry is marked
        // moving first, and `cancel_wait` answers `was_waiting: false`.
        let key = key_for(store, args.session_id);
        set_moving(key, true);
        let moved = move_session_with(args.clone(), store, ssh, hooks, opts).await;
        set_moving(key, false);
        match moved {
            Ok(MoveOutcome::Moved(_)) => break WaitEnd::Moved,
            // `args` is forced to `when: Now, dry_run: false` by the only
            // caller that spawns this loop, so `move_session_with` can only
            // ever answer `Moved` or an `Err` here — a `Preview` needs
            // `dry_run: true`, and `Waiting` / `WaitCancelled` need a `when`
            // other than `Now`. Kept as a match arm (not `unreachable!()`) so
            // a future caller that breaks that invariant gets a recorded
            // refusal instead of a panicked task.
            Ok(other) => {
                refusal = Some(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!(
                        "a wait retry got an outcome it cannot use: {other:?} — run_wait \
                         requires when: Now and dry_run: false"
                    ),
                ));
                break WaitEnd::Refused;
            }
            Err(e) if e.code == codes::E_MOVE_PARTIAL => break WaitEnd::Partial,
            Err(e) if e.message.contains(SOURCE_NOT_IDLE) => continue,
            Err(e) => {
                refusal = Some(e);
                break WaitEnd::Refused;
            }
        }
    };
    record_wait_ended(store, args.session_id, end, refusal.as_ref());
    end
}

/// Best-effort: record why a wait ended, and flag its registry entry so the
/// guard's drop does not record it again. Never `?` — the wait itself is
/// already over by the time this runs, so a store failure here must not
/// change what the caller gets back.
fn record_wait_ended(
    store: &Mutex<Store>,
    session_id: i64,
    end: WaitEnd,
    refusal: Option<&IpcError>,
) {
    if let Ok(mut reg) = waits().lock() {
        if let Some(e) = reg.get_mut(&key_for(store, session_id)) {
            e.ended = true;
        }
    }
    write_wait_ended(store, session_id, end, refusal);
}

/// Insert one `session_move_wait_ended`; logs, never fails.
fn write_wait_ended(
    store: &Mutex<Store>,
    session_id: i64,
    end: WaitEnd,
    refusal: Option<&IpcError>,
) {
    let mut detail = serde_json::json!({ "reason": end });
    if end == WaitEnd::Refused {
        if let Some(e) = refusal {
            detail["code"] = serde_json::Value::String(e.code.clone());
            detail["message"] = serde_json::Value::String(e.message.clone());
        }
    }
    let Ok(s) = store.lock() else {
        tracing::warn!(
            kind = EVENT_MOVE_WAIT_ENDED,
            session_id,
            reason = ?end,
            "[event] store lock poisoned; wait end not recorded"
        );
        return;
    };
    if let Err(err) =
        s.insert_session_event(session_id, EVENT_MOVE_WAIT_ENDED, Some(&detail.to_string()))
    {
        tracing::warn!(
            kind = EVENT_MOVE_WAIT_ENDED,
            session_id,
            error = %err,
            "[event] insert failed"
        );
    }
}

/// Close every unresolved `session_move_waiting` as `hub_restarted` — called
/// once at startup (Task 5), before anything re-registers a live wait. A
/// wait still registered in this process (a wait `begin_wait`'d and not yet
/// ended) is skipped: it is not orphaned, and must never be closed out from
/// under its own `run_wait`. Returns how many it closed.
pub fn sweep_unresolved_waits(store: &Mutex<Store>) -> Result<usize, IpcError> {
    let unresolved = {
        let s = lock(store)?;
        s.unresolved_events(EVENT_MOVE_WAITING, EVENT_MOVE_WAIT_ENDED)?
    };
    let mut closed = 0usize;
    for (session_id, _event_id, _detail) in unresolved {
        let key = key_for(store, session_id);
        let live = waits()
            .lock()
            .map(|reg| reg.contains_key(&key))
            .unwrap_or(false);
        if live {
            continue;
        }
        let detail = serde_json::json!({ "reason": WaitEnd::HubRestarted }).to_string();
        let s = lock(store)?;
        s.insert_session_event(session_id, EVENT_MOVE_WAIT_ENDED, Some(&detail))?;
        closed += 1;
    }
    Ok(closed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two defaults must never drift apart: `MOVE_WAIT_MAX_MINS`'s spec
    /// default (`settings.rs`) and this module's fallback for a missing /
    /// malformed stored value.
    #[test]
    fn the_setting_default_matches_the_fallback_constant() {
        assert_eq!(
            settings::resolve(settings::MOVE_WAIT_MAX_MINS, None),
            DEFAULT_WAIT_MAX_MINS.to_string()
        );
    }
}
