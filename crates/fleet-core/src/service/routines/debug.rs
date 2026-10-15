//! Debugging a routine: `explain` says what it does next or why it does
//! nothing, check by check, with the events an event routine would read
//! and the trace of what the scheduler decided (`routine_decisions`,
//! migration 186); `dry_run` says what a fire right now would start and
//! the exact prompt it would type, without starting anything. Both only
//! read: a person debugging a routine never spends a run on it.
//!
//! **What a reader sees of the timeline:** only the events, and the
//! decisions about events, on sessions the caller may read
//! ([`ViewScope::sees_session_row`]): a routine's owner debugging it learns
//! nothing about another person's sessions or pull requests, and an event
//! on one answers `dry_run` exactly as an id that does not exist.

use super::tick::{self, EventFire, Plan};
use super::{fleet_budget, usd, visible, EVENTS, PR_EVENTS};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::loops::{self, LoopHealth};
use crate::service::view_scope::ViewScope;
use crate::store::{RoutineDecisionRow, RoutineRow, Store, ROUTINE_DECISIONS_KEPT};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Decisions an `explain` carries by default.
pub const DECISIONS_SHOWN: i64 = 30;
/// Recent events of its kind an event routine's `explain` judges.
pub const EVENTS_SHOWN: i64 = 10;
/// The newest events of its kind looked through for [`EVENTS_SHOWN`] the
/// caller may read.
pub const EVENTS_SCANNED: i64 = 200;

/// One check a fire passes or fails, in the order the scheduler asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineCheck {
    /// enabled | automation | scheduler | schedule | event | overlap |
    /// day_budget | fleet_budget | account | host
    pub name: String,
    /// `ok` passes, `warn` passes with a caveat, `block` stops a fire.
    pub state: String,
    pub detail: String,
}

impl RoutineCheck {
    fn new(name: &str, state: &str, detail: impl Into<String>) -> Self {
        RoutineCheck {
            name: name.into(),
            state: state.into(),
            detail: detail.into(),
        }
    }
}

/// What one timeline event does to an event routine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventVerdict {
    pub event_id: i64,
    pub kind: String,
    pub session_id: i64,
    pub at: i64,
    /// Whether the routine's cursor is past it: it was read already.
    pub read: bool,
    /// Whether it fires the routine (its filters and its rate).
    pub fires: bool,
    /// Why it does not, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The run's `trigger_ref` it would carry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// An event routine's view of the timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventExplain {
    pub kind: String,
    /// The last event it read.
    pub cursor: i64,
    /// Events of its kind past the cursor: the next pass reads them.
    /// Counted for an unrestricted caller only (it counts every person's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread: Option<i64>,
    /// The newest events of its kind, newest first, at most
    /// [`EVENTS_SHOWN`].
    #[serde(default)]
    pub recent: Vec<EventVerdict>,
}

/// `routines { action: explain, routine_id, limit? }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutineExplain {
    pub routine_id: i64,
    pub name: String,
    pub trigger: String,
    /// One line: what it does next, or the first check that stops it.
    pub summary: String,
    /// `ok` | `warn` | `block`: the worst check.
    pub state: String,
    #[serde(default)]
    pub checks: Vec<RoutineCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<i64>,
    /// The scheduler loop in the process that answered; `None` runs in it
    /// means it has not passed since that process started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<LoopHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<EventExplain>,
    /// The scheduler's decisions, newest first.
    #[serde(default)]
    pub decisions: Vec<RoutineDecisionRow>,
}

/// The session a dry run would start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DryRunSession {
    pub host_alias: String,
    pub project_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_person_id: Option<i64>,
}

/// `routines { action: dry_run, routine_id, event_id? }`: what a fire now
/// would do. Nothing is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineDryRun {
    pub routine_id: i64,
    /// Whether a fire now starts a session (overlap, budgets, account).
    pub starts: bool,
    /// Why it does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Why it starts on its fallback host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The session it would start (on the fallback when that takes it).
    pub session: DryRunSession,
    /// The prompt it would type: its own, the autonomy line and the line
    /// on what fired it.
    pub prompt: String,
    /// The event it was tried on: the one asked for, or an event routine's
    /// newest of its kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<EventVerdict>,
    /// What only a scheduled fire must also pass (on, Pause all, the
    /// schedule); Run now skips them.
    #[serde(default)]
    pub scheduled_only: Vec<RoutineCheck>,
}

/// Whether `scope` may read session `sid`.
fn sees_session(s: &Store, scope: &ViewScope, sid: Option<i64>) -> Result<bool, IpcError> {
    let Some(sid) = sid else {
        return Ok(true);
    };
    Ok(s.get_session_by_id(sid)?
        .is_some_and(|row| scope.sees_session_row(&row).is_visible()))
}

/// `r`'s newest decisions, the ones about a session `scope` may not read
/// left out.
fn visible_decisions(
    s: &Store,
    scope: &ViewScope,
    r: &RoutineRow,
    limit: Option<i64>,
) -> Result<Vec<RoutineDecisionRow>, IpcError> {
    let limit = limit
        .unwrap_or(DECISIONS_SHOWN)
        .clamp(1, ROUTINE_DECISIONS_KEPT);
    let rows = s.routine_decisions(
        r.id,
        if scope.is_unrestricted() {
            limit
        } else {
            ROUTINE_DECISIONS_KEPT
        },
    )?;
    let mut out = Vec::new();
    for d in rows {
        if out.len() as i64 >= limit {
            break;
        }
        if sees_session(s, scope, d.session_id)? {
            out.push(d);
        }
    }
    Ok(out)
}

/// The process's `routines` loop row.
fn scheduler() -> Option<LoopHealth> {
    loops::registry()
        .snapshot()
        .into_iter()
        .find(|l| l.name == "routines")
}

/// The checks only the scheduler asks: on, Pause all, the loop itself,
/// and the trigger's own (the schedule, an event kind).
fn scheduled_checks(
    s: &Store,
    r: &RoutineRow,
    loop_row: Option<&LoopHealth>,
    now: i64,
) -> Result<Vec<RoutineCheck>, IpcError> {
    let mut out = Vec::new();
    out.push(if r.enabled {
        RoutineCheck::new("enabled", "ok", "it is on")
    } else {
        let why = r
            .paused_reason
            .as_deref()
            .unwrap_or("a person turned it off");
        RoutineCheck::new("enabled", "block", format!("it is off: {why}"))
    });
    out.push(if loops::paused(s) {
        RoutineCheck::new(
            "automation",
            "block",
            "Pause all is on (automation.paused): the scheduler starts nothing; Run now still works",
        )
    } else {
        RoutineCheck::new("automation", "ok", "Pause all is off")
    });
    out.push(match loop_row {
        Some(l) if l.result.as_deref() == Some(loops::RESULT_ERROR) => RoutineCheck::new(
            "scheduler",
            "warn",
            format!(
                "its last pass failed: {}",
                l.last_error.as_deref().unwrap_or("no message")
            ),
        ),
        Some(l) if l.last_run_at.is_some() => RoutineCheck::new(
            "scheduler",
            "ok",
            format!(
                "it passes every {} s; {} passes, {} failed",
                tick::TICK_EVERY.as_secs(),
                l.runs,
                l.failures
            ),
        ),
        _ => RoutineCheck::new(
            "scheduler",
            "warn",
            "it has not passed in this process yet (it starts within 20 s, on the hub when paired)",
        ),
    });
    match r.trigger.as_str() {
        "cron" => out.push(match (r.next_run_at, r.skip_next) {
            (None, _) => RoutineCheck::new(
                "schedule",
                "block",
                format!(
                    "it has no next fire (cron {:?})",
                    r.cron.as_deref().unwrap_or("")
                ),
            ),
            (Some(_), true) => RoutineCheck::new(
                "schedule",
                "warn",
                "a person set Skip next: its next fire is recorded as skipped",
            ),
            (Some(at), false) if at <= now => {
                RoutineCheck::new("schedule", "ok", "it is due: the next pass fires it")
            }
            (Some(_), false) => RoutineCheck::new(
                "schedule",
                "ok",
                format!("cron {}", r.cron.as_deref().unwrap_or("")),
            ),
        }),
        "event" => out.push(match r.event.as_deref() {
            Some(k) if EVENTS.contains(&k) => {
                let mut words = format!("it fires on {k}");
                if let Some(repo) = r.event_repo.as_deref() {
                    words.push_str(&format!(" in {repo}"));
                }
                if PR_EVENTS.contains(&k) && r.event_author.as_deref() == Some("anyone") {
                    words.push_str(", anyone's in its organisation");
                }
                if let Some(rate) = r.event_rate_secs {
                    words.push_str(&format!(
                        ", at most once per subject in {}",
                        tick::cap_words(rate)
                    ));
                }
                RoutineCheck::new("event", "ok", words)
            }
            other => RoutineCheck::new(
                "event",
                "block",
                format!("{other:?} is not an event a routine fires on"),
            ),
        }),
        _ => out.push(RoutineCheck::new(
            "schedule",
            "ok",
            "it runs only when a person presses Run now",
        )),
    }
    Ok(out)
}

/// The checks every fire asks, Run now's too: [`tick::plan`]'s, one by one.
pub(super) fn fire_checks(
    s: &Store,
    r: &RoutineRow,
    now: i64,
) -> Result<Vec<RoutineCheck>, IpcError> {
    let mut out = Vec::new();
    out.push(if r.overlap != "skip" {
        RoutineCheck::new("overlap", "ok", "its runs may overlap")
    } else if s.routine_run_open(r.id)? {
        RoutineCheck::new("overlap", "block", "its last run is still going")
    } else {
        RoutineCheck::new("overlap", "ok", "no run of it is open")
    });
    if let Some(budget) = r.budget_day_micros {
        let since = super::cron::local_day_start(now, r.utc_offset_min);
        let spent = s.routine_cost_since(r.id, since)?;
        let words = format!("its runs spent {} of today's {}", usd(spent), usd(budget));
        out.push(RoutineCheck::new(
            "day_budget",
            if spent >= budget { "block" } else { "ok" },
            words,
        ));
    }
    let fleet = fleet_budget(s, now)?;
    if let Some(budget) = fleet.budget_micros {
        let words = format!(
            "the fleet's routines spent {} of today's {} (automation.daily_budget)",
            usd(fleet.spent_micros),
            usd(budget)
        );
        out.push(RoutineCheck::new(
            "fleet_budget",
            if fleet.spent_micros >= budget {
                "block"
            } else {
                "ok"
            },
            words,
        ));
    }
    let profile = r.profile.as_deref();
    out.push(
        match crate::service::account_limits::over_limit(s, &r.host_alias, profile, now)? {
            Some(o) => RoutineCheck::new("account", "block", o.reason()),
            None => RoutineCheck::new("account", "ok", "its login is under accounts.pause_at"),
        },
    );
    let up = s.get_host_row(&r.host_alias)?.is_some_and(|h| h.reachable);
    out.push(if up {
        RoutineCheck::new("host", "ok", format!("{} is reachable", r.host_alias))
    } else {
        RoutineCheck::new(
            "host",
            "warn",
            format!(
                "the last probe could not reach {}; a fire tries it anyway{}",
                r.host_alias,
                match r.fallback_host.as_deref() {
                    Some(fb) => format!(", or its fallback {fb}"),
                    None => String::new(),
                }
            ),
        )
    });
    Ok(out)
}

/// What event `eid` does to event routine `r`: its filters
/// ([`tick::event_fire`]), then its rate.
fn judge_event(
    s: &Store,
    r: &RoutineRow,
    (eid, sid, at, detail): (i64, i64, i64, Option<String>),
    kind: &str,
    now: i64,
) -> Result<(EventVerdict, Option<EventFire>), IpcError> {
    let mut v = EventVerdict {
        event_id: eid,
        kind: kind.to_string(),
        session_id: sid,
        at,
        read: eid <= r.event_cursor,
        fires: false,
        reason: None,
        reference: None,
    };
    if r.event.as_deref() != Some(kind) {
        v.reason = Some(format!(
            "it is {kind}; the routine fires on {}",
            r.event.as_deref().unwrap_or("no event")
        ));
        return Ok((v, None));
    }
    match tick::event_fire(s, r, kind, eid, sid, detail.as_deref())? {
        Err(why) => {
            v.reason = Some(why);
            Ok((v, None))
        }
        Ok(f) => {
            v.reference = Some(f.reference.clone());
            if let Some(rate) = r.event_rate_secs {
                if s.routine_fired_on_since(r.id, &f.subject, now - rate)? {
                    v.reason = Some(format!(
                        "it already ran for {} in the last {}",
                        f.subject.trim_end_matches(':'),
                        tick::cap_words(rate)
                    ));
                    return Ok((v, None));
                }
            }
            v.fires = true;
            Ok((v, Some(f)))
        }
    }
}

fn worst(checks: &[RoutineCheck]) -> &'static str {
    if checks.iter().any(|c| c.state == "block") {
        "block"
    } else if checks.iter().any(|c| c.state == "warn") {
        "warn"
    } else {
        "ok"
    }
}

/// `routines { action: explain, routine_id, limit? }`: the checks, the
/// events it would read and its trace, `limit` decisions (default
/// [`DECISIONS_SHOWN`]).
pub fn explain(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: i64,
    limit: Option<i64>,
    now: i64,
) -> Result<RoutineExplain, IpcError> {
    let loop_row = scheduler();
    let s = lock(store)?;
    let r = visible(&s, scope, id)?;
    let mut checks = scheduled_checks(&s, &r, loop_row.as_ref(), now)?;
    checks.extend(fire_checks(&s, &r, now)?);
    let event = match r.event.as_deref().filter(|_| r.trigger == "event") {
        Some(kind) if EVENTS.contains(&kind) => {
            let mut recent = Vec::new();
            for ev in s.recent_events_of_kind(kind, EVENTS_SCANNED)? {
                if recent.len() as i64 >= EVENTS_SHOWN {
                    break;
                }
                if sees_session(&s, scope, Some(ev.1))? {
                    recent.push(judge_event(&s, &r, ev, kind, now)?.0);
                }
            }
            let unread = if scope.is_unrestricted() {
                Some(s.count_events_of_kind_after(r.event_cursor, kind)?)
            } else {
                None
            };
            Some(EventExplain {
                kind: kind.to_string(),
                cursor: r.event_cursor,
                unread,
                recent,
            })
        }
        _ => None,
    };
    let state = worst(&checks);
    let summary = match checks.iter().find(|c| c.state == "block") {
        Some(c) => format!("Does not fire: {}.", c.detail),
        None => match r.trigger.as_str() {
            "cron" if r.skip_next => "Its next fire will be skipped (Skip next).".to_string(),
            "cron" => "Fires at its next scheduled time.".to_string(),
            "event" => match &event {
                Some(e) if e.unread.is_some_and(|n| n > 0) => format!(
                    "Fires on the next pass if one of its {} unread {} events passes its filters.",
                    e.unread.unwrap_or(0),
                    e.kind
                ),
                Some(e) => format!("Waiting for a {} event that passes its filters.", e.kind),
                None => "Waiting for its event.".to_string(),
            },
            _ => "Runs when a person presses Run now.".to_string(),
        },
    };
    Ok(RoutineExplain {
        routine_id: r.id,
        name: r.name.clone(),
        trigger: r.trigger.clone(),
        summary,
        state: state.into(),
        checks,
        next_run_at: r.next_run_at,
        scheduler: loop_row,
        decisions: visible_decisions(&s, scope, &r, limit)?,
        event,
    })
}

/// `routines { action: decisions, routine_id, limit? }`: its trace alone,
/// newest first.
pub fn decisions(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: i64,
    limit: Option<i64>,
) -> Result<Vec<RoutineDecisionRow>, IpcError> {
    let s = lock(store)?;
    let r = visible(&s, scope, id)?;
    visible_decisions(&s, scope, &r, limit)
}

/// `routines { action: dry_run, routine_id, event_id? }`: what a fire now
/// would start and type. An event routine is tried on `event_id`, or on
/// the newest event of its kind; another routine refuses an `event_id`.
pub fn dry_run(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: i64,
    event_id: Option<i64>,
    now: i64,
) -> Result<RoutineDryRun, IpcError> {
    let loop_row = scheduler();
    let s = lock(store)?;
    let r = visible(&s, scope, id)?;
    if event_id.is_some() && r.trigger != "event" {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "{} is not an event routine: dry_run takes no event_id",
                r.name
            ),
        ));
    }
    let ev = match event_id {
        Some(eid) => {
            let not_found = || IpcError::new(codes::E_NOTFOUND, format!("event {eid} not found"));
            let (sid, kind, at, detail) = s.session_event_by_id(eid)?.ok_or_else(not_found)?;
            if !sees_session(&s, scope, Some(sid))? {
                return Err(not_found());
            }
            Some((kind, (eid, sid, at, detail)))
        }
        None => match r.event.as_deref().filter(|_| r.trigger == "event") {
            Some(kind) => {
                let mut newest = None;
                for e in s.recent_events_of_kind(kind, EVENTS_SCANNED)? {
                    if sees_session(&s, scope, Some(e.1))? {
                        newest = Some((kind.to_string(), e));
                        break;
                    }
                }
                newest
            }
            None => None,
        },
    };
    let (event, fire) = match ev {
        Some((kind, e)) => {
            let (v, f) = judge_event(&s, &r, e, &kind, now)?;
            (Some(v), f)
        }
        None => (None, None),
    };
    let (starts, reason, host, note) = match tick::plan(&s, &r, now)? {
        Plan::Skip(why) => (false, Some(why), r.host_alias.clone(), None),
        Plan::Start { host, note } => (true, None, host, note),
    };
    let owner = match r.owner_person_id {
        Some(p) => Some(p),
        None => s.personal_owner_id()?,
    };
    let scheduled_only = scheduled_checks(&s, &r, loop_row.as_ref(), now)?;
    Ok(RoutineDryRun {
        routine_id: r.id,
        starts,
        reason,
        note,
        session: DryRunSession {
            host_alias: host,
            project_id: r.project_id,
            profile: r.profile.clone(),
            owner_person_id: owner,
        },
        prompt: tick::run_prompt(&r, fire.as_ref().and_then(|f| f.context.as_deref())),
        event,
        scheduled_only,
    })
}
