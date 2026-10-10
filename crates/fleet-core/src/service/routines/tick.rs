//! The routines' scheduler: one pass every [`TICK_EVERY`] that settles the
//! open runs, then fires every due cron routine and every event routine
//! whose event was written, each under its lease, so a hub and a desktop on
//! one store never both fire one routine. A restart loses nothing: a fire
//! missed while the process was down runs once at the next pass, and the
//! next one is counted from then.

use super::fix::{
    turn_code, E_NO_SESSION, E_PROMPT_NOT_DELIVERED, E_PROMPT_NOT_QUEUED, E_RUN_BUDGET,
    E_RUN_STALE, E_RUN_TIME_CAP, E_SESSION_LOST, E_SESSION_REMOVED,
};
use super::{record_skip, repo_matches, usd, EVENTS, PR_EVENTS};
use crate::cancel::CancellationRegistry;
use crate::ipc_error::{lock, IpcError};
use crate::service::decide::routine_run_outcome::{self, PaneReader, TmuxPanes};
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
/// A failed run older than this is not retried (`retry_once`): a pass
/// missed for longer, a restart, does not start yesterday's work.
pub const RETRY_WITHIN_SECS: i64 = 3600;

/// Starts a run's session: the real start path in production, a fake in
/// tests.
#[async_trait::async_trait]
pub trait Spawn: Send + Sync {
    async fn spawn(&self, args: NewSessionArgs) -> Result<SessionRow, IpcError>;
    /// Reads a run's screen for Jev's outcome (step 8.10); `None` where no
    /// pane can be read (tests).
    fn panes(&self) -> Option<Arc<dyn PaneReader>> {
        None
    }
    /// Stops the turn of a run past its time cap (G3.8): Escape in its
    /// pane, so the session stays for a person to read. Nothing in tests.
    async fn stop_turn(&self, _host_alias: &str, _tmux_name: &str) -> Result<(), IpcError> {
        Ok(())
    }
    /// Types a run's prompt into its fresh session once Claude is ready
    /// (`handover` is the row queued with the same text). `Err` says why it
    /// was not typed. Nothing in tests.
    async fn deliver(
        &self,
        _row: &SessionRow,
        _prompt: &str,
        _handover: i64,
    ) -> Result<(), String> {
        Ok(())
    }
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
    fn panes(&self) -> Option<Arc<dyn PaneReader>> {
        Some(Arc::new(TmuxPanes {
            ssh: Arc::clone(&self.ssh),
        }))
    }
    async fn stop_turn(&self, host_alias: &str, tmux_name: &str) -> Result<(), IpcError> {
        crate::service::sessions::send_keys(
            host_alias,
            tmux_name,
            crate::tmux::NamedKey::Escape,
            &self.store,
            &self.ssh,
        )
        .await
    }
    async fn deliver(&self, row: &SessionRow, prompt: &str, handover: i64) -> Result<(), String> {
        crate::service::sessions::seed::seed_routine(&self.store, &self.ssh, row, prompt, handover)
            .await
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

/// The session a run of `r` starts on `host`.
fn session_args(r: &RoutineRow, host: &str, owner: Option<i64>) -> NewSessionArgs {
    NewSessionArgs {
        host_alias: host.to_string(),
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
        over_limit_ok: false,
        owner_person_id: owner,
        start_token: None,
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
    fire_with(deps, r, trigger, trigger_ref, scheduled_for, None, now).await
}

/// [`fire`], with a line on what fired it added under the prompt (a pull
/// request event names its PR, so the run knows which one).
pub async fn fire_with(
    deps: &Deps,
    r: &RoutineRow,
    trigger: &str,
    trigger_ref: Option<&str>,
    scheduled_for: Option<i64>,
    context: Option<&str>,
    now: i64,
) -> Result<RoutineRunRow, IpcError> {
    let (owner, _starting, plan) = {
        let s = lock(&deps.store)?;
        let (host, note) = match plan(&s, r, now)? {
            Plan::Skip(why) => {
                return record_skip(&s, r, trigger, trigger_ref, scheduled_for, &why, now)
            }
            Plan::Start { host, note } => (host, note),
        };
        // Its run row is written only once the session exists, so a second
        // fire while this one spawns (Run now during a scheduled fire, a
        // double click) would pass `routine_run_open` too: the claim is
        // taken under the same lock as the check (review r06 F1).
        let starting = match claim_start(&deps.store, r) {
            Some(c) => c,
            None => {
                let why = "its last run is still starting";
                return record_skip(&s, r, trigger, trigger_ref, scheduled_for, why, now);
            }
        };
        let owner = match r.owner_person_id {
            Some(p) => Some(p),
            None => s.personal_owner_id()?,
        };
        (owner, starting, (host, note))
    };
    let (mut host, mut note) = plan;
    let mut spawned = deps.spawn.spawn(session_args(r, &host, owner)).await;
    // The start failed on the routine's own host: its fallback takes the
    // run, once (G3.8).
    if let (Err(e), Some(fb)) = (&spawned, r.fallback_host.as_deref()) {
        if host == r.host_alias {
            note = Some(format!(
                "on {fb}: the start on {} failed ({})",
                r.host_alias, e.message
            ));
            host = fb.to_string();
            spawned = deps.spawn.spawn(session_args(r, &host, owner)).await;
        }
    }
    let s = lock(&deps.store)?;
    let mut typing = None;
    let (state, reason, code, session_id) = match spawned {
        Ok(row) => {
            let meta = serde_json::json!({ "routine_id": r.id, "source": "routine" }).to_string();
            let prompt = run_prompt(r, context);
            match s.enqueue_handover(row.id, &prompt, Some(&meta)) {
                Ok(handover) => {
                    let id = row.id;
                    typing = Some((row, prompt, handover));
                    ("running", note, None, Some(id))
                }
                Err(e) => (
                    "failed",
                    Some(format!("its prompt was not queued: {}", e.message)),
                    Some(E_PROMPT_NOT_QUEUED.to_string()),
                    Some(row.id),
                ),
            }
        }
        Err(e) => ("failed", Some(e.message), Some(e.code), None),
    };
    let run = s.insert_routine_run(&NewRoutineRun {
        routine_id: r.id,
        trigger,
        trigger_ref,
        state,
        reason: reason.as_deref(),
        session_id,
        scheduled_for,
        at: now,
        error_code: code.as_deref(),
        host_alias: Some(&host),
    })?;
    // A fresh session takes no turn until something is typed into it: the
    // queued prompt alone sat there unread. Typed in the background (the
    // REPL takes seconds, a trust dialog waits for a person); a run whose
    // prompt could not be typed fails, so it shows and `retry_once` can
    // take it, rather than staying `running` for six quiet hours.
    if let Some((row, prompt, handover)) = typing {
        let store = Arc::clone(&deps.store);
        let spawn = Arc::clone(&deps.spawn);
        let run_id = run.id;
        crate::rt::spawn(async move {
            if let Err(why) = spawn.deliver(&row, &prompt, handover).await {
                undelivered(&store, run_id, &why, crate::store::now_unix());
            }
        });
    }
    Ok(run)
}

/// Fail run `run_id` whose prompt was not typed, unless it has already
/// closed.
pub(super) fn undelivered(store: &Mutex<Store>, run_id: i64, why: &str, now: i64) {
    let res = lock(store).and_then(|s| match s.get_routine_run(run_id)? {
        Some(run) if run.state == "running" => fail(
            &s,
            run.id,
            E_PROMPT_NOT_DELIVERED,
            &format!("its prompt was not delivered: {why}"),
            run.cost_micros,
            now,
        ),
        _ => Ok(()),
    });
    if let Err(e) = res {
        tracing::warn!(run = run_id, error = %e.message, "[routines] failing an undelivered run failed");
    }
}

/// The prompt a run of `r` queues: its own, the autonomy line (G3.8) and
/// the line on what fired it.
pub(super) fn run_prompt(r: &RoutineRow, context: Option<&str>) -> String {
    let mut prompt = r.prompt.clone();
    for line in [super::autonomy_line(r.autonomy), context]
        .into_iter()
        .flatten()
    {
        prompt.push_str("\n\n");
        prompt.push_str(line);
    }
    prompt
}

/// The `overlap: skip` routines whose run is between its check and its row.
static STARTING: crate::rt::KeySet = std::sync::Mutex::new(None);

/// A skip routine's claim on starting its one run, or `None` when another
/// fire of it is already starting. A routine whose runs may overlap claims
/// nothing.
fn claim_start(store: &Arc<Mutex<Store>>, r: &RoutineRow) -> Option<Option<crate::rt::KeyedClaim>> {
    if r.overlap != "skip" {
        return Some(None);
    }
    crate::rt::claim(&STARTING, Arc::as_ptr(store) as usize, r.id.to_string()).map(Some)
}

/// Whether `r` starts a run now, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Plan {
    /// It does not, in words.
    Skip(String),
    /// On `host`: its own, or its fallback with `note` saying why.
    Start { host: String, note: Option<String> },
}

/// Why `r` may not start a run now, in words: its last run is still open
/// (overlap `skip`), its runs spent its day budget, the fleet's routines
/// spent `automation.daily_budget`, or its login is past the line and no
/// fallback host can take it.
fn refusal(s: &Store, r: &RoutineRow, now: i64) -> Result<Option<String>, IpcError> {
    Ok(match plan(s, r, now)? {
        Plan::Skip(why) => Some(why),
        Plan::Start { .. } => None,
    })
}

/// Where a run of `r` starts now, if it does (see [`refusal`]). Its own
/// host, unless that host is unreachable or its login is past
/// `accounts.pause_at` and its fallback host is neither (G3.8); the
/// fallback bills the same profile there. A host the last probe could not
/// reach, with no fallback that can, is tried anyway: the probe may be
/// stale, and a failed start says why.
pub(super) fn plan(s: &Store, r: &RoutineRow, now: i64) -> Result<Plan, IpcError> {
    if r.overlap == "skip" && s.routine_run_open(r.id)? {
        return Ok(Plan::Skip("its last run is still going".into()));
    }
    if let Some(budget) = r.budget_day_micros {
        let since = super::cron::local_day_start(now, r.utc_offset_min);
        let spent = s.routine_cost_since(r.id, since)?;
        if spent >= budget {
            return Ok(Plan::Skip(format!(
                "its runs spent {} of today's {}",
                usd(spent),
                usd(budget)
            )));
        }
    }
    let fleet = super::fleet_budget(s, now)?;
    if let Some(budget) = fleet.budget_micros {
        if fleet.spent_micros >= budget {
            return Ok(Plan::Skip(format!(
                "the fleet's routines spent {} of today's {} (automation.daily_budget)",
                usd(fleet.spent_micros),
                usd(budget)
            )));
        }
    }
    // Account-aware automation (redesign 8.7): a run on an account past
    // `accounts.pause_at` would stall at the limit, so the routine skips it
    // and fires again at its next time, unless its fallback can take it.
    let profile = r.profile.as_deref();
    let over = crate::service::account_limits::over_limit(s, &r.host_alias, profile, now)?;
    let reachable = |h: &str| -> Result<bool, IpcError> {
        Ok(s.get_host_row(h)?.is_some_and(|row| row.reachable))
    };
    let down = !reachable(&r.host_alias)?;
    if over.is_none() && !down {
        return Ok(Plan::Start {
            host: r.host_alias.clone(),
            note: None,
        });
    }
    if let Some(fb) = r.fallback_host.as_deref() {
        let fb_over = crate::service::account_limits::over_limit(s, fb, profile, now)?;
        if fb_over.is_none() && reachable(fb)? {
            let why = match &over {
                Some(o) => o.reason(),
                None => format!("{} was unreachable", r.host_alias),
            };
            return Ok(Plan::Start {
                host: fb.to_string(),
                note: Some(format!("on {fb}: {why}")),
            });
        }
    }
    Ok(match over {
        Some(o) => Plan::Skip(o.reason()),
        None => Plan::Start {
            host: r.host_alias.clone(),
            note: None,
        },
    })
}

/// Close a run and record what its own facts say it came to (step 8.10).
/// A failed close records `code`, why it failed (G3.8).
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

/// [`close`] a run failed, with its error code.
fn fail(
    s: &Store,
    id: i64,
    code: &str,
    reason: &str,
    cost_micros: i64,
    now: i64,
) -> Result<(), IpcError> {
    s.set_routine_run_error_code(id, code)?;
    close(s, id, "failed", Some(reason), cost_micros, now)
}

/// A turn to stop: a run past its time cap (G3.8), `(host, tmux name)`.
pub type StopTurn = (String, String);

/// How a time cap reads: "20 min", "2 h", "1 h 30 min".
pub(super) fn cap_words(secs: i64) -> String {
    let m = secs / 60;
    match (m / 60, m % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// Close the open runs whose session finished a turn, failed, went away or
/// went quiet for [`RUN_STALE_SECS`]; refresh the others' cost, fail one
/// past its run budget and pause its routine, and fail one past its time
/// cap. Answers the turns to stop: the time-capped runs' (G3.8).
pub fn settle(s: &Store, now: i64) -> Result<Vec<StopTurn>, IpcError> {
    let mut stop = Vec::new();
    for run in s.running_routine_runs()? {
        let Some(sid) = run.session_id else {
            fail(s, run.id, E_NO_SESSION, "it started no session", 0, now)?;
            continue;
        };
        let Some(row) = s.get_session_by_id(sid)? else {
            fail(
                s,
                run.id,
                E_SESSION_REMOVED,
                "its session was removed before its turn finished",
                run.cost_micros,
                now,
            )?;
            continue;
        };
        let cost = row.usage.usage_cost_micros;
        let routine = s.get_routine(run.routine_id)?;
        if s.session_event_since(sid, "turn_done", 0)? {
            close(s, run.id, "done", None, cost, now)?;
        } else if s.session_event_since(sid, "stop_failure", 0)? {
            let detail = s
                .newest_session_event_of(sid, &["stop_failure"])?
                .and_then(|e| e.detail);
            let why = match detail.as_deref() {
                Some(d) => format!("its turn ended in an error: {d}"),
                None => "its turn ended in an error".to_string(),
            };
            fail(s, run.id, turn_code(detail.as_deref()), &why, cost, now)?;
        } else if row.lost_at.is_some() {
            fail(s, run.id, E_SESSION_LOST, "its session was lost", cost, now)?;
        } else if let Some(budget) = routine
            .as_ref()
            .and_then(|r| r.budget_run_micros)
            .filter(|b| cost >= *b)
        {
            let why = format!(
                "its session spent {} of the run's {}",
                usd(cost),
                usd(budget)
            );
            fail(s, run.id, E_RUN_BUDGET, &why, cost, now)?;
            s.set_routine_enabled(
                run.routine_id,
                false,
                Some(&format!("paused: a run went over its budget ({why})")),
                None,
            )?;
        } else if let Some(cap) = routine
            .as_ref()
            .and_then(|r| r.run_max_secs)
            .filter(|c| now - run.started_at > *c)
        {
            let why = format!(
                "it ran past its {} time cap; its turn was stopped",
                cap_words(cap)
            );
            fail(s, run.id, E_RUN_TIME_CAP, &why, cost, now)?;
            stop.push((row.host_alias.clone(), row.tmux_name.clone()));
        } else if now - run.started_at > RUN_STALE_SECS {
            fail(
                s,
                run.id,
                E_RUN_STALE,
                "no turn finished in 6 hours",
                cost,
                now,
            )?;
        } else if cost != run.cost_micros {
            s.set_routine_run_cost(run.id, cost)?;
        }
    }
    Ok(stop)
}

/// Start each failed run of a `retry_once` routine again, once (G3.8): the
/// run's own trigger, `trigger_ref` `retry:<run id>`. Only while automation
/// runs (the caller is past Pause all's gate).
async fn retry_failed(deps: &Deps, now: i64) -> Result<(), IpcError> {
    let runs = lock(&deps.store)?.routine_runs_to_retry(now - RETRY_WITHIN_SECS)?;
    for run in runs {
        let Some(r) = lock(&deps.store)?.get_routine(run.routine_id)? else {
            continue;
        };
        let reference = format!("retry:{}", run.id);
        fire(
            deps,
            &r,
            &run.trigger,
            Some(&reference),
            run.scheduled_for,
            now,
        )
        .await?;
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

/// One event that may fire an event routine: the run's `trigger_ref`, the
/// subject its rate counts (`pr:<id>:` or `session:<id>:`), and the line
/// added under its prompt.
struct EventFire {
    reference: String,
    subject: String,
    context: Option<String>,
}

/// What a pull request event says, for the line under the prompt.
fn pr_event_words(kind: &str, pr: &crate::store::PullRequestRow) -> String {
    match kind {
        crate::store::PR_EVENT_REVIEW => match pr.review_decision.as_deref() {
            Some("CHANGES_REQUESTED") => "got a review asking for changes".into(),
            _ => "was approved".into(),
        },
        crate::store::PR_EVENT_CI_FAILED => "has failing checks".into(),
        crate::store::PR_EVENT_CI_PASSED => "has its checks passing".into(),
        crate::store::PR_EVENT_MERGED => "was merged".into(),
        other => other.replace('_', " "),
    }
}

/// Whether event `eid` of `kind` on session `sid` may fire `r`, and how.
/// A session event: [`event_fires`]. A pull request event (its detail is
/// the PR's URL): the PR passes the routine's repo filter, and is its
/// owner's ([`event_fires`] on the session that opened it) or, with
/// `event_author: anyone`, any PR seen on a host of the routine's org,
/// never one a routine's session opened. An unassigned routine has no org
/// to widen to: anyone is its owner's.
fn event_fire(
    s: &Store,
    r: &RoutineRow,
    kind: &str,
    eid: i64,
    sid: i64,
    detail: Option<&str>,
) -> Result<Option<EventFire>, IpcError> {
    if !PR_EVENTS.contains(&kind) {
        return Ok(event_fires(s, r, sid)?.then(|| EventFire {
            reference: format!("session:{sid}:{eid}"),
            subject: format!("session:{sid}:"),
            context: None,
        }));
    }
    let Some(pr) = detail
        .map(|u| s.pull_request_by_url(u))
        .transpose()?
        .flatten()
    else {
        return Ok(None);
    };
    if let Some(f) = r.event_repo.as_deref() {
        if !repo_matches(f, pr.repo.as_deref()) {
            return Ok(None);
        }
    }
    let anyone = r.event_author.as_deref() == Some("anyone") && r.org_id.is_some();
    let passes = if anyone {
        let in_org = match pr.host_alias.as_deref() {
            Some(h) => s.host_org(h)? == r.org_id,
            None => false,
        };
        let by_routine = s
            .get_session_by_id(sid)?
            .is_some_and(|row| row.origin.as_deref() == Some("routine"));
        in_org && !by_routine
    } else {
        event_fires(s, r, sid)?
    };
    Ok(passes.then(|| EventFire {
        reference: format!("pr:{}:{eid}", pr.id),
        subject: format!("pr:{}:", pr.id),
        context: Some(format!(
            "Started because pull request {} {}.",
            pr.url,
            pr_event_words(kind, &pr)
        )),
    }))
}

/// One cron routine, under its lease: skip it when a person asked, else
/// fire it; then count its next fire from `now`.
async fn tick_cron(deps: &Deps, id: i64, now: i64) -> Result<(), IpcError> {
    let r = {
        let s = lock(&deps.store)?;
        if !s.take_routine_lease(id, crate::service::tick::lease_clock(now))? {
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
    // The routine as it is now: a person may have changed its schedule or
    // set Skip next while it fired, and the next fire follows what they
    // saved, not what this pass read (review r06 F3).
    if let Some(cur) = s.get_routine(id)? {
        s.advance_routine(id, super::next_fire_of(&cur, now), r.skip_next)?;
    }
    s.release_routine_lease(id)?;
    res
}

/// One event routine, under its lease: fire once per new event of its
/// kind that it may see, then move its cursor past what it read.
pub(super) async fn tick_event(deps: &Deps, r: &RoutineRow, now: i64) -> Result<(), IpcError> {
    let (r, fires) = {
        let s = lock(&deps.store)?;
        if !s.take_routine_lease(r.id, crate::service::tick::lease_clock(now))? {
            return Ok(());
        }
        // The row as it is now, under the lease: the pass read it before
        // earlier fires ran, and meanwhile it may have been turned off or
        // back on (a new cursor), or changed.
        let fresh = s
            .get_routine(r.id)?
            .filter(|f| f.enabled && f.trigger == "event");
        let Some((r, kind)) = fresh.and_then(|f| {
            let k = f.event.clone().filter(|k| EVENTS.contains(&k.as_str()))?;
            Some((f, k))
        }) else {
            s.release_routine_lease(r.id)?;
            return Ok(());
        };
        let kind = kind.as_str();
        let events = s.events_of_kind_after(r.event_cursor, kind, EVENTS_PER_PASS)?;
        let Some(&(last, _, _)) = events.last() else {
            // Nothing of this kind since the cursor: move it to the newest
            // event anyway (same lock, so nothing slipped in between), or a
            // rare kind rescans every event since the routine was made, on
            // every 20 s pass (review r16).
            let newest = s.latest_session_event_id()?;
            if newest > r.event_cursor {
                s.set_routine_event_cursor(r.id, newest)?;
            }
            s.release_routine_lease(r.id)?;
            return Ok(());
        };
        s.set_routine_event_cursor(r.id, last)?;
        let mut fires = Vec::new();
        for (eid, sid, detail) in events {
            if let Some(f) = event_fire(&s, &r, kind, eid, sid, detail.as_deref())? {
                fires.push(f);
            }
        }
        (r, fires)
    };
    let r = &r;
    let mut res = Ok(());
    for f in fires {
        // An error here ends the pass, not the lease: it is released below,
        // so the routine is not held for the lease's two minutes. A fire
        // inside the rate is dropped like a busy one; it reads the runs this
        // pass already started, so two events of one PR fire once.
        let busy = lock(&deps.store).and_then(|s| {
            if let Some(rate) = r.event_rate_secs {
                if s.routine_fired_on_since(r.id, &f.subject, now - rate)? {
                    return Ok(true);
                }
            }
            refusal(&s, r, now).map(|w| w.is_some())
        });
        match busy {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                res = Err(e);
                break;
            }
        }
        let ctx = f.context.as_deref();
        if let Err(e) = fire_with(deps, r, "event", Some(&f.reference), None, ctx, now).await {
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
    // Settling only reads what happened (and stops a run past its time
    // cap), so it runs while paused too. Nothing below Pause all's gate
    // starts: no fire, no retry.
    let (stop, work) = match deps.store.lock() {
        Ok(s) => match settle(&s, now) {
            Err(e) => (Vec::new(), Err(e.message)),
            Ok(stop) => {
                let work = if !crate::service::loops::gate_in("routines", &s, next) {
                    Ok(None)
                } else {
                    match (s.routines_due(now), s.event_routines()) {
                        (Ok(d), Ok(e)) => Ok(Some((d, e))),
                        (Err(e), _) | (_, Err(e)) => Err(e.message),
                    }
                };
                (stop, work)
            }
        },
        Err(e) => (Vec::new(), Err(e.to_string())),
    };
    stop_turns(deps, stop).await;
    let (due, events) = match work {
        Ok(Some(w)) => w,
        Ok(None) => return,
        Err(e) => {
            crate::service::loops::report("routines", Err(e), next);
            return;
        }
    };
    let mut failed: Option<String> = None;
    if let Err(e) = retry_failed(deps, now).await {
        tracing::warn!(error = %e.message, "[routines] retry failed");
        failed = Some(format!("retry: {}", e.message));
    }
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
    // Jev reads what the runs that just finished came to, off this path.
    if let Some(panes) = deps.spawn.panes() {
        routine_run_outcome::spawn_pass(&deps.store, panes, now);
    }
    crate::service::loops::report("routines", failed.map_or(Ok(()), Err), next);
}

/// Stop the turns of the runs past their time cap; a failure is logged,
/// the run is failed either way.
async fn stop_turns(deps: &Deps, stop: Vec<StopTurn>) {
    for (host, name) in stop {
        if let Err(e) = deps.spawn.stop_turn(&host, &name).await {
            tracing::warn!(host = %host, session = %name, error = %e.message, "[routines] stop turn failed");
        }
    }
}

/// The scheduler's periodic task, on the hub and on a standalone desktop.
pub fn spawn_routine_tick(
    deps: Deps,
    token: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    crate::rt::spawn(async move {
        let mut every = tokio::time::interval(TICK_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // A pass that panics is logged and the next tick runs (review r06 F7).
        crate::service::tick::run_cancellable_tick(every, token, || {
            tick_once(&deps, crate::store::now_unix())
        })
        .await;
    })
}
