//! Routines (Orbit Fleet redesign step 8.5): a saved prompt that starts a
//! session on a schedule ([`cron`]), on a fleet event, or when a person
//! presses Run now. Every run lands in a session whose origin is the
//! routine (migration 124), owned by the routine's owner, on the routine's
//! host and project, billing its profile; the prompt is queued as the
//! session's handover and typed into it once Claude's REPL is ready
//! (`sessions::seed::seed_routine`), and a run whose prompt could not be
//! typed fails with `E_PROMPT_NOT_DELIVERED`.
//!
//! **Who may read a routine:** its org's boundary first
//! (`OrgScope::sees_org`), then its owner, the hub's own reader and the one
//! person of a single-person hub for an unowned one
//! ([`ViewScope::may_own_person_row`]), and a member of its org. **Who may
//! change it:** the same, except that of the org's members only an admin
//! may. Anyone else gets exactly the answer of an id that does not exist.
//! A routine's org is its host's, read when it is saved. A per-host token
//! is refused at the tool layer: a session does not schedule sessions.
//!
//! **What stops a run** ([`tick`]): `automation.paused` (Pause all) stops
//! the scheduler, never a person's Run now; the overlap rule `skip` records
//! a fire that finds a run still open as skipped; the day budget records a
//! fire after the routine's runs spent it today as skipped; and a run whose
//! session passes the run budget is failed and the routine paused, so a
//! person looks before it spends again. Checking the account's usage before
//! a run is step 8.7's.
//!
//! **Event triggers** read the session timeline (`session_events`) past a
//! per-routine cursor: a routine fires on an [`EVENTS`] kind written for a
//! session its owner owns, never for a session a routine started (so two
//! routines cannot keep each other going). A fire that finds the routine
//! busy or over budget is dropped, not recorded: a timeline event is far
//! too frequent for a row each.
//!
//! **Pull request triggers** (M15 step G2.4) are [`PR_EVENTS`]: reconcile
//! writes one on the timeline of the session that opened a PR when its
//! `pull_requests` row gains a review, its checks turn failing or passing,
//! or it merges. Such a routine may narrow to one repo (`event_repo`) and
//! widen from its owner's PRs to its org's (`event_author: anyone`). Any
//! event routine may hold a rate (`event_rate_secs`): at most one run per
//! PR, or per session, in that window; a fire inside it is dropped.
//!
//! **Guards** (M15 step G3.8, migration 159): the fleet's daily budget
//! (`automation.daily_budget`, every routine's runs in one UTC day) stops
//! new runs like a routine's own day budget; a time cap (`run_max_secs`)
//! stops a run's turn and fails it; a fallback host (`fallback_host`)
//! takes a run its host cannot (unreachable, its login past the line, or a
//! start that failed); `retry_once` starts a failed run again on the next
//! pass, once, and Pause all stops that too; `autonomy` adds a line to the
//! prompt saying how far the run may go (report, ask before push, push). A
//! failed run carries an error code, read as a named fix ([`fix`]).

pub mod cron;
pub mod fix;
pub mod outcome;
pub mod tick;

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs;
use crate::service::view_scope::ViewScope;
use crate::store::{
    NewRoutineRun, RoutineFields, RoutineRow, RoutineRunRow, Store, PR_EVENT_CI_FAILED,
    PR_EVENT_CI_PASSED, PR_EVENT_MERGED, PR_EVENT_REVIEW, ROUTINE_OVERLAPS, ROUTINE_TRIGGERS,
};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

pub use tick::{spawn_routine_tick, tick_once, Deps, LiveSpawn, Spawn};

/// The session timeline kinds an event routine may fire on.
pub const EVENTS: [&str; 12] = [
    "turn_done",
    "stop_failure",
    "stuck",
    "lost",
    "task_done",
    "task_failed",
    "session_restore_failed",
    "workspace_repair_failed",
    PR_EVENT_REVIEW,
    PR_EVENT_CI_FAILED,
    PR_EVENT_CI_PASSED,
    PR_EVENT_MERGED,
];

/// The [`EVENTS`] about a pull request: their detail is its URL, and only
/// they take a repo filter or `event_author: anyone`.
pub const PR_EVENTS: [&str; 4] = [
    PR_EVENT_REVIEW,
    PR_EVENT_CI_FAILED,
    PR_EVENT_CI_PASSED,
    PR_EVENT_MERGED,
];

/// `event_author` values: `me` (the same as absent) | `anyone`.
pub const EVENT_AUTHORS: [&str; 2] = ["me", "anyone"];
/// The longest event rate: a week.
pub const EVENT_RATE_MAX_SECS: i64 = 7 * 24 * 3600;
/// A repo filter, in characters.
pub const EVENT_REPO_MAX_CHARS: usize = 200;

/// Runs a `routines { get }` carries.
pub const RUNS_SHOWN: i64 = 20;
/// The most runs one `runs` read returns.
pub const RUNS_MAX: i64 = 200;
/// A routine's name, in characters.
pub const NAME_MAX_CHARS: usize = 80;

/// What a person writes (`routines { action: save, routine }`). On a change
/// every field is written again: send the whole routine.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct RoutineInput {
    pub name: String,
    /// Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// cron | event | manual
    pub trigger: String,
    /// For cron: `minute hour day month weekday`, or @hourly, @daily, @weekly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// Minutes east of UTC the cron line is read at (the device's offset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc_offset_min: Option<i64>,
    /// For event: a timeline kind (turn_done, stuck, lost, pr_review,
    /// pr_ci_failed, pr_ci_passed, pr_merged, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    pub host_alias: String,
    pub project_id: i64,
    /// Credential profile (the account it bills); absent = the host's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_run_micros: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_day_micros: Option<i64>,
    /// skip (default) | parallel
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlap: Option<String>,
    /// pr_* events: only this repo, owner/name or name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_repo: Option<String>,
    /// pr_* events: me (default) | anyone in the routine's org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_author: Option<String>,
    /// At most one run per PR (or session) in this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_rate_secs: Option<i64>,
    // The guards (G3.8): plain comments, not doc comments, so they stay
    // out of the served schema's budget; docs/control-api.md says them.
    // The IANA zone shown with the schedule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    // Stop the turn and fail the run after this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_max_secs: Option<i64>,
    // The host for a run its own host cannot take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_host: Option<String>,
    // Start a failed run again, once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_once: Option<bool>,
    // 0 report only | 1 ask before push | 2 push (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<i64>,
}

/// The shortest time cap: a minute.
pub const RUN_MAX_SECS_MIN: i64 = 60;
/// A time zone name, in characters.
pub const TIME_ZONE_MAX_CHARS: usize = 64;
/// `autonomy` levels: what the run is told it may do.
pub const AUTONOMY_LEVELS: [&str; 3] = ["report only", "ask before push", "push and open PRs"];

/// The line `autonomy` adds under a run's prompt; `None` for level 2 (or
/// none), which says nothing beyond the prompt.
pub fn autonomy_line(level: Option<i64>) -> Option<&'static str> {
    match level {
        Some(0) => Some(
            "Autonomy L0, report only: change no files, commit nothing and push nothing. \
             Report what you found and what you would do.",
        ),
        Some(1) => Some(
            "Autonomy L1, ask before push: you may change files and commit, but stop and ask \
             before you push, open a pull request or merge.",
        ),
        _ => None,
    }
}

/// `routines { action: get }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutineDetail {
    pub routine: RoutineRow,
    /// Newest first, at most [`RUNS_SHOWN`].
    #[serde(default)]
    pub runs: Vec<RoutineRunRow>,
    /// Whether this caller may change it: the UI's buttons, not a fence.
    #[serde(default)]
    pub may_change: bool,
    /// The account its runs bill, "runs as … on mac" (redesign 8.7);
    /// `None` when its login is on no known account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<crate::service::account_limits::LoginAccount>,
    /// The named fix of each failed run in `runs` (G3.8).
    #[serde(default)]
    pub fixes: Vec<fix::RunFix>,
}

fn not_found(id: i64) -> IpcError {
    orgs::not_found("routine", id)
}

fn host_not_found(alias: &str) -> IpcError {
    IpcError::new(codes::E_NOTFOUND, format!("host {alias} not found"))
}

fn role_in(s: &Store, scope: &ViewScope, org: Option<i64>) -> Result<Option<String>, IpcError> {
    match (org, scope.person) {
        (Some(o), Some(p)) => s.org_role(o, p),
        _ => Ok(None),
    }
}

/// May `scope` read `r`?
pub fn sees_routine(s: &Store, scope: &ViewScope, r: &RoutineRow) -> Result<bool, IpcError> {
    if scope.may_own_person_row(r.org_id, r.owner_person_id) {
        return Ok(true);
    }
    Ok(scope.org.sees_org(r.org_id) && role_in(s, scope, r.org_id)?.is_some())
}

/// May `scope` change `r`?
pub fn may_change_routine(s: &Store, scope: &ViewScope, r: &RoutineRow) -> Result<bool, IpcError> {
    if scope.may_own_person_row(r.org_id, r.owner_person_id) {
        return Ok(true);
    }
    Ok(scope.org.sees_org(r.org_id)
        && role_in(s, scope, r.org_id)?.as_deref() == Some(crate::store::ROLE_ADMIN))
}

/// The routine, if `scope` may read it.
pub fn visible(s: &Store, scope: &ViewScope, id: i64) -> Result<RoutineRow, IpcError> {
    match s.get_routine(id)? {
        Some(r) if sees_routine(s, scope, &r)? => Ok(r),
        _ => Err(not_found(id)),
    }
}

/// The routine, if `scope` may change it. One it may read but not change
/// is `E_FORBIDDEN`; one it may not read is unknown.
pub fn changeable(s: &Store, scope: &ViewScope, id: i64) -> Result<RoutineRow, IpcError> {
    let r = visible(s, scope, id)?;
    if may_change_routine(s, scope, &r)? {
        Ok(r)
    } else {
        Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "{} is its owner's; an admin of its organisation may change it too",
                r.name
            ),
        ))
    }
}

/// `routines { action: list }`: the routines this caller may read.
pub fn list(store: &Mutex<Store>, scope: &ViewScope) -> Result<Vec<RoutineRow>, IpcError> {
    let s = lock(store)?;
    let mut out = Vec::new();
    for r in s.list_routines()? {
        if sees_routine(&s, scope, &r)? {
            out.push(r);
        }
    }
    Ok(out)
}

/// `routines { action: get, routine_id }`.
pub fn get(store: &Mutex<Store>, scope: &ViewScope, id: i64) -> Result<RoutineDetail, IpcError> {
    let s = lock(store)?;
    let routine = visible(&s, scope, id)?;
    let runs = s.routine_runs(id, RUNS_SHOWN)?;
    Ok(RoutineDetail {
        fixes: runs
            .iter()
            .filter_map(|run| fix::fix(&routine, run))
            .collect(),
        runs,
        may_change: may_change_routine(&s, scope, &routine)?,
        account: crate::service::account_limits::login_account(
            &s,
            &routine.host_alias,
            routine.profile.as_deref(),
            crate::store::now_unix(),
        )?,
        routine,
    })
}

/// `routines { action: runs, routine_id, limit? }`: newest first.
pub fn runs(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: i64,
    limit: Option<i64>,
) -> Result<Vec<RoutineRunRow>, IpcError> {
    let s = lock(store)?;
    visible(&s, scope, id)?;
    s.routine_runs(id, limit.unwrap_or(RUNS_SHOWN).clamp(1, RUNS_MAX))
}

/// A routine whose newest run failed, as the Inbox shows it (redesign 8.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailingRoutine {
    pub routine: RoutineRow,
    pub run: RoutineRunRow,
    /// Whether this caller may Retry or Pause it: the UI's buttons.
    #[serde(default)]
    pub may_change: bool,
    /// Its named fix (G3.8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<fix::RunFix>,
}

/// Whether a run failed: its state, or what it came to (8.10).
fn run_failed(r: &RoutineRunRow) -> bool {
    fix::failed(r)
}

/// `routines { action: failing }`: each routine this caller may read whose
/// newest run failed, newest failure first. A newer run (Retry) takes it
/// out, and so does a person switching it off after the failure (Pause);
/// a routine the scheduler paused, over its budget, stays, since that
/// pause is news too.
pub fn failing(store: &Mutex<Store>, scope: &ViewScope) -> Result<Vec<FailingRoutine>, IpcError> {
    let s = lock(store)?;
    let mut out = Vec::new();
    for r in s.list_routines()? {
        if !sees_routine(&s, scope, &r)? {
            continue;
        }
        let Some(run) = s.routine_runs(r.id, 1)?.into_iter().next() else {
            continue;
        };
        if !run_failed(&run) {
            continue;
        }
        let failed_at = run.finished_at.unwrap_or(run.started_at);
        if !r.enabled && r.paused_reason.is_none() && r.updated_at >= failed_at {
            continue;
        }
        out.push(FailingRoutine {
            may_change: may_change_routine(&s, scope, &r)?,
            fix: fix::fix(&r, &run),
            routine: r,
            run,
        });
    }
    out.sort_by_key(|f| {
        std::cmp::Reverse((f.run.finished_at.unwrap_or(f.run.started_at), f.run.id))
    });
    Ok(out)
}

/// The fields `input` writes, checked; and the routine's org, its host's.
fn check(
    s: &Store,
    scope: &ViewScope,
    input: &RoutineInput,
) -> Result<(RoutineFields, Option<i64>), IpcError> {
    let invalid = |m: String| IpcError::new(codes::E_INVALID, m);
    let name = input.name.trim();
    if name.is_empty() {
        return Err(invalid("a routine needs a name".into()));
    }
    if name.chars().count() > NAME_MAX_CHARS || name.chars().any(char::is_control) {
        return Err(invalid(format!(
            "a routine's name is one line of at most {NAME_MAX_CHARS} characters"
        )));
    }
    let prompt = input.prompt.trim();
    if prompt.is_empty() {
        return Err(invalid("a routine needs a prompt".into()));
    }
    let max = crate::service::work::handover::BRIEF_MAX_CHARS;
    if prompt.chars().count() > max {
        return Err(invalid(format!(
            "a routine's prompt is at most {max} characters"
        )));
    }
    if !ROUTINE_TRIGGERS.contains(&input.trigger.as_str()) {
        return Err(invalid(format!(
            "trigger must be cron | event | manual, got {:?}",
            input.trigger
        )));
    }
    let offset = input.utc_offset_min.unwrap_or(0);
    if offset.abs() > cron::MAX_OFFSET_MIN {
        return Err(invalid(format!(
            "utc_offset_min must be within ±{}",
            cron::MAX_OFFSET_MIN
        )));
    }
    let cron_line = match input.trigger.as_str() {
        "cron" => {
            let line = input
                .cron
                .as_deref()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .ok_or_else(|| invalid("a cron routine needs cron".into()))?;
            let c = cron::Cron::parse(line).map_err(invalid)?;
            if c.next_after(crate::store::now_unix(), offset).is_none() {
                return Err(invalid(format!("{line:?} never fires")));
            }
            Some(line.to_string())
        }
        _ => None,
    };
    let event = match input.trigger.as_str() {
        "event" => {
            let e = input
                .event
                .as_deref()
                .ok_or_else(|| invalid("an event routine needs event".into()))?;
            if !EVENTS.contains(&e) {
                return Err(invalid(format!(
                    "event must be one of {}, got {e:?}",
                    EVENTS.join(" | ")
                )));
            }
            Some(e.to_string())
        }
        _ => None,
    };
    let (event_repo, event_author, event_rate_secs) = match event.as_deref() {
        Some(e) => event_filters(input, e)?,
        None => (None, None, None),
    };
    crate::validate::host_alias(&input.host_alias)?;
    if s.get_host_row(&input.host_alias)?.is_none() {
        return Err(host_not_found(&input.host_alias));
    }
    let org = s.host_org(&input.host_alias)?;
    if !scope.org.sees_org(org) {
        return Err(host_not_found(&input.host_alias));
    }
    if s.get_project(input.project_id)?.is_none() {
        return Err(orgs::not_found("project", input.project_id));
    }
    let profile = input
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| crate::validate::claude_profile(p).map(|()| p.to_string()))
        .transpose()?;
    for (what, v) in [
        ("budget_run_micros", input.budget_run_micros),
        ("budget_day_micros", input.budget_day_micros),
    ] {
        if v.is_some_and(|v| v <= 0) {
            return Err(invalid(format!("{what} must be above 0 (or absent)")));
        }
    }
    let guards = guards(s, scope, input, org)?;
    let overlap = input.overlap.as_deref().unwrap_or("skip");
    if !ROUTINE_OVERLAPS.contains(&overlap) {
        return Err(invalid(format!(
            "overlap must be skip | parallel, got {overlap:?}"
        )));
    }
    Ok((
        RoutineFields {
            name: name.to_string(),
            enabled: input.enabled.unwrap_or(true),
            trigger: input.trigger.clone(),
            cron: cron_line,
            utc_offset_min: offset,
            event,
            host_alias: input.host_alias.clone(),
            project_id: input.project_id,
            profile,
            prompt: prompt.to_string(),
            budget_run_micros: input.budget_run_micros,
            budget_day_micros: input.budget_day_micros,
            overlap: overlap.to_string(),
            event_repo,
            event_author,
            event_rate_secs,
            time_zone: guards.time_zone,
            run_max_secs: guards.run_max_secs,
            fallback_host: guards.fallback_host,
            retry_once: guards.retry_once,
            autonomy: guards.autonomy,
        },
        org,
    ))
}

/// A routine's guards (G3.8), checked.
struct Guards {
    time_zone: Option<String>,
    run_max_secs: Option<i64>,
    fallback_host: Option<String>,
    retry_once: bool,
    autonomy: Option<i64>,
}

/// The guards `input` writes: a zone name of letters, digits and `/_+-`; a
/// time cap from a minute to the six hours after which any run fails; a
/// fallback host this caller sees, in the routine's org and not its own
/// host; an autonomy level 0 to 2.
fn guards(
    s: &Store,
    scope: &ViewScope,
    input: &RoutineInput,
    org: Option<i64>,
) -> Result<Guards, IpcError> {
    let invalid = |m: String| IpcError::new(codes::E_INVALID, m);
    let time_zone = input
        .time_zone
        .as_deref()
        .map(str::trim)
        .filter(|z| !z.is_empty());
    if let Some(z) = time_zone {
        let ok = z.chars().count() <= TIME_ZONE_MAX_CHARS
            && z.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '+' | '-'));
        if !ok {
            return Err(invalid(format!(
                "time_zone is a zone name such as Europe/Bratislava, got {z:?}"
            )));
        }
    }
    if let Some(cap) = input.run_max_secs {
        if !(RUN_MAX_SECS_MIN..=tick::RUN_STALE_SECS).contains(&cap) {
            return Err(invalid(format!(
                "run_max_secs must be {RUN_MAX_SECS_MIN} to {} (six hours), or absent",
                tick::RUN_STALE_SECS
            )));
        }
    }
    let fallback = input
        .fallback_host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty());
    if let Some(h) = fallback {
        crate::validate::host_alias(h)?;
        if h == input.host_alias {
            return Err(invalid(
                "the fallback host is another host than the routine's".into(),
            ));
        }
        if s.get_host_row(h)?.is_none() || !scope.org.sees_org(s.host_org(h)?) {
            return Err(host_not_found(h));
        }
        if s.host_org(h)? != org {
            return Err(invalid(format!(
                "the fallback host {h} is in another organisation than {}",
                input.host_alias
            )));
        }
    }
    if let Some(a) = input.autonomy {
        if !(0..=2).contains(&a) {
            return Err(invalid(format!(
                "autonomy must be 0 (report only), 1 (ask before push) or 2 (push), got {a}"
            )));
        }
    }
    Ok(Guards {
        time_zone: time_zone.map(str::to_string),
        run_max_secs: input.run_max_secs,
        fallback_host: fallback.map(str::to_string),
        retry_once: input.retry_once.unwrap_or(false),
        autonomy: input.autonomy.filter(|a| *a < 2),
    })
}

/// `routines { action: budget }` (G3.8): what every routine's runs spent
/// in this UTC day, against `automation.daily_budget`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetBudget {
    /// Micro-USD, every routine's runs since `since`.
    pub spent_micros: i64,
    /// `automation.daily_budget` in micro-USD; absent = none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_micros: Option<i64>,
    /// The UTC day's start, unix seconds.
    pub since: i64,
}

/// The UTC day's start at `now`.
pub fn utc_day_start(now: i64) -> i64 {
    now - now.rem_euclid(86_400)
}

/// The fleet's routine spend today and its budget. Every routine counts,
/// whoever's: the budget is the fleet's. Any caller that reads routines may
/// read it.
pub fn fleet_budget(s: &Store, now: i64) -> Result<FleetBudget, IpcError> {
    let since = utc_day_start(now);
    let usd =
        crate::service::settings::get_secs(s, crate::service::settings::AUTOMATION_DAILY_BUDGET);
    Ok(FleetBudget {
        spent_micros: s.routines_cost_since(since)?,
        budget_micros: (usd > 0).then(|| (usd as i64).saturating_mul(1_000_000)),
        since,
    })
}

/// `routines { action: budget }`.
pub fn budget(store: &Mutex<Store>, now: i64) -> Result<FleetBudget, IpcError> {
    fleet_budget(&*lock(store)?, now)
}

/// An event routine's filters, checked: the repo (trimmed, `owner/name` or
/// `name`), the author (`None` for me) and the rate. A repo or `anyone` on
/// a session event is refused: a session event has no repo, and is only
/// ever its owner's.
#[allow(clippy::type_complexity)]
fn event_filters(
    input: &RoutineInput,
    event: &str,
) -> Result<(Option<String>, Option<String>, Option<i64>), IpcError> {
    let invalid = |m: String| IpcError::new(codes::E_INVALID, m);
    let on_pr = PR_EVENTS.contains(&event);
    let repo = input
        .event_repo
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    if let Some(r) = repo {
        if !on_pr {
            return Err(invalid(format!(
                "a repo filter is for a pull request event, not {event}"
            )));
        }
        let parts: Vec<&str> = r.split('/').collect();
        let ok = r.chars().count() <= EVENT_REPO_MAX_CHARS
            && parts.len() <= 2
            && parts.iter().all(|p| {
                !p.is_empty()
                    && p.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            });
        if !ok {
            return Err(invalid(format!(
                "event_repo is owner/name or name, got {r:?}"
            )));
        }
    }
    let author = match input.event_author.as_deref().map(str::trim) {
        None | Some("") | Some("me") => None,
        Some("anyone") if on_pr => Some("anyone".to_string()),
        Some("anyone") => {
            return Err(invalid(format!(
                "a session event is its owner's; anyone is for a pull request event, not {event}"
            )))
        }
        Some(a) => {
            return Err(invalid(format!(
                "event_author must be {}, got {a:?}",
                EVENT_AUTHORS.join(" | ")
            )))
        }
    };
    if let Some(r) = input.event_rate_secs {
        if !(1..=EVENT_RATE_MAX_SECS).contains(&r) {
            return Err(invalid(format!(
                "event_rate_secs must be 1 to {EVENT_RATE_MAX_SECS} (a week), or absent"
            )));
        }
    }
    Ok((repo.map(str::to_string), author, input.event_rate_secs))
}

/// Whether a PR's `owner/name` passes a routine's repo filter: the same
/// `owner/name`, or the same `name` when the filter names no owner,
/// without case. A PR whose repo is unknown passes no filter.
pub fn repo_matches(filter: &str, repo: Option<&str>) -> bool {
    let Some(repo) = repo else { return false };
    if filter.contains('/') {
        return filter.eq_ignore_ascii_case(repo);
    }
    repo.rsplit('/')
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case(filter))
}

/// The next cron fire of `f` after `now`; `None` for any other trigger, or
/// while it is off.
fn next_fire(f: &RoutineFields, now: i64) -> Option<i64> {
    if !f.enabled || f.trigger != "cron" {
        return None;
    }
    let c = cron::Cron::parse(f.cron.as_deref()?).ok()?;
    c.next_after(now, f.utc_offset_min)
}

/// The next fire of a stored routine after `now` (see [`next_fire`]).
pub(crate) fn next_fire_of(r: &RoutineRow, now: i64) -> Option<i64> {
    if r.trigger != "cron" {
        return None;
    }
    cron::Cron::parse(r.cron.as_deref()?)
        .ok()?
        .next_after(now, r.utc_offset_min)
}

/// Why a change that moves a routine to another organisation's host is
/// refused.
const STAYS_IN_ORG: &str = "a routine stays in its organisation: pick a host of the same one";

/// How many next fires a [`preview`] lists.
pub const PREVIEW_RUNS: usize = 5;

/// `routines { action: preview, routine, routine_id? }` (gap plan G2.3):
/// the editor's dry run of a routine not saved yet. Nothing is written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutinePreview {
    /// Its schedule's next fires after now (unix seconds), at most
    /// [`PREVIEW_RUNS`], read at `utc_offset_min` as the scheduler reads
    /// them; empty for an event or manual routine and for a line that does
    /// not parse. A fixed offset does not follow a daylight-saving change
    /// (see [`cron`]): the editor shows these in the device's zone, so a
    /// fire past the change shows the hour it moved by.
    pub next_runs: Vec<i64>,
    /// The offset `next_runs` were read at, minutes east of UTC.
    pub utc_offset_min: i64,
    /// Why `save` would refuse it, in its words; absent when it would save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// The account its login bills, when the login is on a known account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<crate::service::account_limits::LoginAccount>,
    /// Every login on its host that is on a known account, the host's own
    /// first: the editor's Account picker. Empty for a host this caller
    /// may not see.
    #[serde(default)]
    pub logins: Vec<crate::service::account_limits::LoginAccount>,
}

/// The dry run of `input`: what `save` would say, its next fires and the
/// account it would bill. With `routine_id`, as a change of that routine,
/// which this caller must be able to change.
pub fn preview(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: Option<i64>,
    input: &RoutineInput,
    now: i64,
) -> Result<RoutinePreview, IpcError> {
    let s = lock(store)?;
    let before = id.map(|id| changeable(&s, scope, id)).transpose()?;
    let problem = match check(&s, scope, input) {
        Ok((_, org)) if before.as_ref().is_some_and(|b| b.org_id != org) => {
            Some(STAYS_IN_ORG.to_string())
        }
        Ok(_) => None,
        Err(e) => Some(e.message),
    };
    let offset = input
        .utc_offset_min
        .unwrap_or(0)
        .clamp(-cron::MAX_OFFSET_MIN, cron::MAX_OFFSET_MIN);
    let mut next_runs = Vec::new();
    if input.trigger == "cron" {
        if let Some(c) = input
            .cron
            .as_deref()
            .and_then(|l| cron::Cron::parse(l).ok())
        {
            let mut at = now;
            while next_runs.len() < PREVIEW_RUNS {
                let Some(next) = c.next_after(at, offset) else {
                    break;
                };
                next_runs.push(next);
                at = next;
            }
        }
    }
    let alias = input.host_alias.as_str();
    let host_seen = crate::validate::host_alias(alias).is_ok()
        && s.get_host_row(alias)?.is_some()
        && scope.org.sees_org(s.host_org(alias)?);
    let logins = if host_seen {
        crate::service::account_limits::login_accounts(&s, alias, now)?
    } else {
        Vec::new()
    };
    let profile = input
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let account = logins
        .iter()
        .find(|l| l.login.profile.as_deref() == profile)
        .cloned();
    Ok(RoutinePreview {
        next_runs,
        utc_offset_min: offset,
        problem,
        account,
        logins,
    })
}

/// `routines { action: save, routine, routine_id? }`: without `routine_id`
/// a new routine, the caller's; with it, a change of every field.
pub fn save(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: Option<i64>,
    input: &RoutineInput,
) -> Result<RoutineRow, IpcError> {
    let s = lock(store)?;
    let now = crate::store::now_unix();
    match id {
        None => {
            // The owner is the caller's person; the hub's own reader (the
            // standalone desktop) writes the hub's personal owner. A caller
            // that proves no person owns nothing.
            let owner = match scope.person {
                Some(p) => Some(p),
                None if scope.is_internal() => s.personal_owner_id()?,
                None => {
                    return Err(IpcError::new(
                        codes::E_FORBIDDEN,
                        "a routine is a person's; this caller proves no person",
                    ))
                }
            };
            let (f, org) = check(&s, scope, input)?;
            let cursor = s.latest_session_event_id()?;
            s.insert_routine(org, owner, &f, next_fire(&f, now), cursor)
        }
        Some(id) => {
            let before = changeable(&s, scope, id)?;
            let (f, org) = check(&s, scope, input)?;
            if org != before.org_id {
                return Err(IpcError::new(codes::E_INVALID, STAYS_IN_ORG));
            }
            // A routine that starts listening for another event, or again
            // after being off, does not fire on what happened meanwhile.
            let cursor = if f.trigger == "event"
                && (before.event != f.event || before.trigger != "event" || !before.enabled)
            {
                s.latest_session_event_id()?
            } else {
                before.event_cursor
            };
            // The same schedule, still on: a fire already due waits for the
            // tick instead of being moved past (an edit to the prompt must
            // not drop it).
            let same_schedule = before.enabled
                && f.enabled
                && before.trigger == f.trigger
                && before.cron == f.cron
                && before.utc_offset_min == f.utc_offset_min;
            let next = if same_schedule && before.next_run_at.is_some() {
                before.next_run_at
            } else {
                next_fire(&f, now)
            };
            s.update_routine(id, &f, next, cursor)?
                .ok_or_else(|| not_found(id))
        }
    }
}

/// `routines { action: delete, routine_id }`: the routine and its runs.
/// The sessions its runs started stay.
pub fn delete(store: &Mutex<Store>, scope: &ViewScope, id: i64) -> Result<bool, IpcError> {
    let s = lock(store)?;
    changeable(&s, scope, id)?;
    s.delete_routine(id)
}

/// `routines { action: set_enabled, routine_id, enabled }`. Turning one on
/// starts its schedule from now and its event cursor at the newest event.
pub fn set_enabled(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: i64,
    enabled: bool,
) -> Result<RoutineRow, IpcError> {
    let s = lock(store)?;
    let r = changeable(&s, scope, id)?;
    let now = crate::store::now_unix();
    if enabled && !r.enabled && r.trigger == "event" {
        s.set_routine_event_cursor(id, s.latest_session_event_id()?)?;
    }
    // Already on: keep the schedule as it is, a due fire included.
    let next = match (enabled, r.enabled) {
        (false, _) => None,
        (true, true) if r.next_run_at.is_some() => r.next_run_at,
        (true, _) => next_fire_of(&r, now),
    };
    s.set_routine_enabled(id, enabled, None, next)?
        .ok_or_else(|| not_found(id))
}

/// `routines { action: skip_next, routine_id, skip? }`: the next scheduled
/// fire is recorded as skipped instead of run (`skip: false` takes that
/// back). Only a cron routine has a next fire.
pub fn skip_next(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: i64,
    skip: bool,
) -> Result<RoutineRow, IpcError> {
    let s = lock(store)?;
    let r = changeable(&s, scope, id)?;
    if r.trigger != "cron" {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("{} has no schedule to skip", r.name),
        ));
    }
    s.set_routine_skip_next(id, skip)?
        .ok_or_else(|| not_found(id))
}

/// `routines { action: run_now, routine_id }`: a person starts a run now,
/// whatever its trigger, and whether or not it is on. Pause all does not
/// stop a person; the overlap rule and the day budget do.
pub async fn run_now(
    deps: &Deps,
    scope: &ViewScope,
    id: i64,
    now: i64,
) -> Result<RoutineRunRow, IpcError> {
    let r = {
        let s = lock(&deps.store)?;
        changeable(&s, scope, id)?
    };
    tick::fire(deps, &r, "run_now", None, None, now).await
}

/// Record a run that did not start.
pub(crate) fn record_skip(
    s: &Store,
    r: &RoutineRow,
    trigger: &str,
    trigger_ref: Option<&str>,
    scheduled_for: Option<i64>,
    reason: &str,
    now: i64,
) -> Result<RoutineRunRow, IpcError> {
    s.insert_routine_run(&NewRoutineRun {
        routine_id: r.id,
        trigger,
        trigger_ref,
        state: "skipped",
        reason: Some(reason),
        session_id: None,
        scheduled_for,
        at: now,
        error_code: None,
        host_alias: None,
    })
}

/// Dollars, for a reason a person reads.
pub(crate) fn usd(micros: i64) -> String {
    format!("${:.2}", micros as f64 / 1e6)
}

#[cfg(test)]
mod tests;
