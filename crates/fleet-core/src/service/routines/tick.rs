//! The routines' scheduler: one pass every [`TICK_EVERY`] that settles the
//! open runs, then fires every due cron routine and every event routine
//! whose event was written, each under its lease, so a hub and a desktop on
//! one store never both fire one routine. A restart loses nothing: a fire
//! missed while the process was down runs once at the next pass, and the
//! next one is counted from then.

use super::{record_skip, usd, EVENTS};
use crate::cancel::CancellationRegistry;
use crate::ipc_error::{lock, IpcError};
use crate::service::sessions::NewSessionArgs;
use crate::ssh::SshClient;
use crate::store::{NewRoutineRun, RoutineRow, RoutineRunRow, SessionOrigin, SessionRow, Store};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the scheduler looks.
pub const TICK_EVERY: Duration = Duration::from_secs(20);
/// A run whose session finished no turn in this long is failed.
pub const RUN_STALE_SECS: i64 = 6 * 3600;
/// Session events one event routine reads per pass.
pub const EVENTS_PER_PASS: i64 = 50;

/// Starts a run's session: the real start path in production, a fake in
/// tests.
#[async_trait::async_trait]
pub trait Spawn: Send + Sync {
    async fn spawn(&self, args: NewSessionArgs) -> Result<SessionRow, IpcError>;
}

/// [`Spawn`] through `service::sessions::new_session`.
pub struct LiveSpawn {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
    pub reg: Arc<CancellationRegistry>,
}

#[async_trait::async_trait]
impl Spawn for LiveSpawn {
    async fn spawn(&self, args: NewSessionArgs) -> Result<SessionRow, IpcError> {
        crate::service::sessions::new_session(args, &self.store, &self.ssh, &self.reg).await
    }
}

/// What the scheduler needs.
#[derive(Clone)]
pub struct Deps {
    pub store: Arc<Mutex<Store>>,
    pub spawn: Arc<dyn Spawn>,
}

impl Deps {
    /// The production dependencies: runs start through the real start path.
    pub fn live(
        store: Arc<Mutex<Store>>,
        ssh: Arc<SshClient>,
        reg: Arc<CancellationRegistry>,
    ) -> Self {
        let spawn = Arc::new(LiveSpawn {
            store: Arc::clone(&store),
            ssh,
            reg,
        });
        Deps { store, spawn }
    }
}

/// The session a run of `r` starts.
fn session_args(r: &RoutineRow, owner: Option<i64>) -> NewSessionArgs {
    NewSessionArgs {
        host_alias: r.host_alias.clone(),
        project_id: r.project_id,
        worktree_id: None,
        name: String::new(),
        call_id: None,
        new_worktree: None,
        base_branch: None,
        kind: None,
        start_command: None,
        friendly_name: crate::validate::friendly_name(&r.name)
            .is_ok()
            .then(|| r.name.clone()),
        resume_claude_session_id: None,
        model: None,
        effort: None,
        profile: r.profile.clone(),
        agent: None,
        origin: Some(SessionOrigin::routine(r.id)),
        owner_person_id: owner,
    }
}

/// Fire `r` once: record why it does not start (overlap, day budget), or
/// start its session, queue its prompt and record the run `running`. A
/// spawn that fails records the run `failed` and answers it, not an error:
/// the run is what a person reads.
pub async fn fire(
    deps: &Deps,
    r: &RoutineRow,
    trigger: &str,
    trigger_ref: Option<&str>,
    scheduled_for: Option<i64>,
    now: i64,
) -> Result<RoutineRunRow, IpcError> {
    let owner = {
        let s = lock(&deps.store)?;
        if let Some(why) = refusal(&s, r, now)? {
            return record_skip(&s, r, trigger, trigger_ref, scheduled_for, &why, now);
        }
        match r.owner_person_id {
            Some(p) => Some(p),
            None => s.personal_owner_id()?,
        }
    };
    let spawned = deps.spawn.spawn(session_args(r, owner)).await;
    let s = lock(&deps.store)?;
    let (state, reason, session_id) = match spawned {
        Ok(row) => {
            let meta = serde_json::json!({ "routine_id": r.id, "source": "routine" }).to_string();
            match s.enqueue_handover(row.id, &r.prompt, Some(&meta)) {
                Ok(_) => ("running", None, Some(row.id)),
                Err(e) => (
                    "failed",
                    Some(format!("its prompt was not queued: {}", e.message)),
                    Some(row.id),
                ),
            }
        }
        Err(e) => ("failed", Some(e.message), None),
    };
    s.insert_routine_run(&NewRoutineRun {
        routine_id: r.id,
        trigger,
        trigger_ref,
        state,
        reason: reason.as_deref(),
        session_id,
        scheduled_for,
        at: now,
    })
}

/// Why `r` may not start a run now, in words: its last run is still open
/// (overlap `skip`), or its runs spent its day budget.
fn refusal(s: &Store, r: &RoutineRow, now: i64) -> Result<Option<String>, IpcError> {
    if r.overlap == "skip" && s.routine_run_open(r.id)? {
        return Ok(Some("its last run is still going".into()));
    }
    if let Some(budget) = r.budget_day_micros {
        let since = super::cron::local_day_start(now, r.utc_offset_min);
        let spent = s.routine_cost_since(r.id, since)?;
        if spent >= budget {
            return Ok(Some(format!(
                "its runs spent {} of today's {}",
                usd(spent),
                usd(budget)
            )));
        }
    }
    Ok(None)
}

/// Close a run and record what its own facts say it came to (step 8.10).
fn close(
    s: &Store,
    id: i64,
    state: &str,
    reason: Option<&str>,
    cost_micros: i64,
    now: i64,
) -> Result<(), IpcError> {
    s.finish_routine_run(id, state, reason, cost_micros, now)?;
    super::outcome::on_close(s, id, now)
}

/// Close the open runs whose session finished a turn, failed, went away or
/// went quiet for [`RUN_STALE_SECS`]; refresh the others' cost, and fail
/// one past its run budget and pause its routine.
pub fn settle(s: &Store, now: i64) -> Result<(), IpcError> {
    for run in s.running_routine_runs()? {
        let Some(sid) = run.session_id else {
            close(s, run.id, "failed", Some("it started no session"), 0, now)?;
            continue;
        };
        let Some(row) = s.get_session_by_id(sid)? else {
            close(
                s,
                run.id,
                "failed",
                Some("its session was removed before its turn finished"),
                run.cost_micros,
                now,
            )?;
            continue;
        };
        let cost = row.usage.usage_cost_micros;
        if s.session_event_since(sid, "turn_done", 0)? {
            close(s, run.id, "done", None, cost, now)?;
        } else if s.session_event_since(sid, "stop_failure", 0)? {
            close(
                s,
                run.id,
                "failed",
                Some("its turn ended in an error"),
                cost,
                now,
            )?;
        } else if row.lost_at.is_some() {
            close(s, run.id, "failed", Some("its session was lost"), cost, now)?;
        } else if let Some(budget) = s
            .get_routine(run.routine_id)?
            .and_then(|r| r.budget_run_micros)
            .filter(|b| cost >= *b)
        {
            let why = format!(
                "its session spent {} of the run's {}",
                usd(cost),
                usd(budget)
            );
            close(s, run.id, "failed", Some(&why), cost, now)?;
            s.set_routine_enabled(
                run.routine_id,
                false,
                Some(&format!("paused: a run went over its budget ({why})")),
                None,
            )?;
        } else if now - run.started_at > RUN_STALE_SECS {
            close(
                s,
                run.id,
                "failed",
                Some("no turn finished in 6 hours"),
                cost,
                now,
            )?;
        } else if cost != run.cost_micros {
            s.set_routine_run_cost(run.id, cost)?;
        }
    }
    Ok(())
}

/// Whether an event on session `sid` may fire `r`: a session its owner
/// owns, never one a routine started.
fn event_fires(s: &Store, r: &RoutineRow, sid: i64) -> Result<bool, IpcError> {
    let Some(row) = s.get_session_by_id(sid)? else {
        return Ok(false);
    };
    if row.origin.as_deref() == Some("routine") {
        return Ok(false);
    }
    Ok(match (row.owner_person_id, r.owner_person_id) {
        (Some(session_owner), Some(owner)) => session_owner == owner,
        // An unowned routine exists only on a hub with no people, where
        // every session is the hub's own.
        (_, None) => s.personal_owner_id()?.is_none(),
        (None, Some(_)) => false,
    })
}

/// One cron routine, under its lease: skip it when a person asked, else
/// fire it; then count its next fire from `now`.
async fn tick_cron(deps: &Deps, id: i64, now: i64) -> Result<(), IpcError> {
    let r = {
        let s = lock(&deps.store)?;
        if !s.take_routine_lease(id, now)? {
            return Ok(());
        }
        match s.get_routine(id)? {
            Some(r) if r.enabled && r.next_run_at.is_some_and(|at| at <= now) => r,
            _ => {
                s.release_routine_lease(id)?;
                return Ok(());
            }
        }
    };
    let scheduled = r.next_run_at;
    let res = if r.skip_next {
        let s = lock(&deps.store)?;
        record_skip(&s, &r, "cron", None, scheduled, "a person skipped it", now).map(|_| ())
    } else {
        fire(deps, &r, "cron", None, scheduled, now)
            .await
            .map(|_| ())
    };
    let s = lock(&deps.store)?;
    s.advance_routine(id, super::next_fire_of(&r, now))?;
    s.release_routine_lease(id)?;
    res
}

/// One event routine, under its lease: fire once per new event of its
/// kind that it may see, then move its cursor past what it read.
async fn tick_event(deps: &Deps, r: &RoutineRow, now: i64) -> Result<(), IpcError> {
    let Some(kind) = r.event.as_deref().filter(|k| EVENTS.contains(k)) else {
        return Ok(());
    };
    let fires = {
        let s = lock(&deps.store)?;
        if !s.take_routine_lease(r.id, now)? {
            return Ok(());
        }
        let events = s.events_of_kind_after(r.event_cursor, kind, EVENTS_PER_PASS)?;
        let Some(&(last, _)) = events.last() else {
            s.release_routine_lease(r.id)?;
            return Ok(());
        };
        s.set_routine_event_cursor(r.id, last)?;
        let mut fires = Vec::new();
        for (eid, sid) in events {
            if event_fires(&s, r, sid)? {
                fires.push(format!("session:{sid}:{eid}"));
            }
        }
        fires
    };
    let mut res = Ok(());
    for reference in fires {
        let busy = {
            let s = lock(&deps.store)?;
            refusal(&s, r, now)?.is_some()
        };
        if busy {
            continue;
        }
        if let Err(e) = fire(deps, r, "event", Some(&reference), None, now).await {
            res = Err(e);
        }
    }
    let s = lock(&deps.store)?;
    s.release_routine_lease(r.id)?;
    res
}

/// One pass of the scheduler.
pub async fn tick_once(deps: &Deps, now: i64) {
    let next = Some(TICK_EVERY);
    let (due, events) = match deps.store.lock() {
        Ok(s) => {
            // Settling only reads what happened, so it runs while paused too.
            if let Err(e) = settle(&s, now) {
                crate::service::loops::report("routines", Err(e.message), next);
                return;
            }
            if !crate::service::loops::gate_in("routines", &s, next) {
                return;
            }
            match (s.routines_due(now), s.event_routines()) {
                (Ok(d), Ok(e)) => (d, e),
                (Err(e), _) | (_, Err(e)) => {
                    crate::service::loops::report("routines", Err(e.message), next);
                    return;
                }
            }
        }
        Err(e) => {
            crate::service::loops::report("routines", Err(e.to_string()), next);
            return;
        }
    };
    let mut failed: Option<String> = None;
    for id in due {
        if let Err(e) = tick_cron(deps, id, now).await {
            tracing::warn!(routine = id, error = %e.message, "[routines] fire failed");
            failed = Some(format!("routine {id}: {}", e.message));
        }
    }
    for r in &events {
        if let Err(e) = tick_event(deps, r, now).await {
            tracing::warn!(routine = r.id, error = %e.message, "[routines] event fire failed");
            failed = Some(format!("routine {}: {}", r.id, e.message));
        }
    }
    crate::service::loops::report("routines", failed.map_or(Ok(()), Err), next);
}

/// The scheduler's periodic task, on the hub and on a standalone desktop.
pub fn spawn_routine_tick(
    deps: Deps,
    token: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    crate::rt::spawn(async move {
        let mut every = tokio::time::interval(TICK_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = token.cancelled() => return,
                _ = every.tick() => tick_once(&deps, crate::store::now_unix()).await,
            }
        }
    })
}
