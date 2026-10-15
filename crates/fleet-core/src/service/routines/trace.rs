//! A routine run's steps in its trace (`service::run_trace`, migration
//! 194): what fired it, the checks it passed, where it started, its prompt,
//! how it settled and who judged its outcome. The scheduler (`tick`) and
//! the outcome rules call these; the words are a person's.

use super::debug::RoutineCheck;
use crate::service::run_trace::{record, record_all, routine_ref, RunRef, Step};
use crate::store::{RoutineRow, RoutineRunRow, Store};

/// What fired a run, in words, from its `trigger` and `trigger_ref`.
pub fn trigger_words(trigger: &str, trigger_ref: Option<&str>) -> String {
    if let Some(id) = trigger_ref.and_then(|t| t.strip_prefix("retry:")) {
        return format!("Retry of run #{id}");
    }
    match (trigger, trigger_ref) {
        ("cron", _) => "Fired by its schedule".into(),
        ("run_now", _) => "Started by Run now".into(),
        ("event", Some(r)) if r.starts_with("pr:") => {
            let pr = r.split(':').nth(1).unwrap_or_default();
            format!("Fired by an event on pull request {pr}")
        }
        ("event", Some(r)) if r.starts_with("session:") => {
            let sid = r.split(':').nth(1).unwrap_or_default();
            format!("Fired by an event on session {sid}")
        }
        ("event", _) => "Fired by an event".into(),
        (other, _) => format!("Fired by {other}"),
    }
}

/// The run `run` retries, if it is a retry: `routine:<id>`.
fn parent(run: &RoutineRunRow) -> Option<String> {
    run.trigger_ref
        .as_deref()
        .and_then(|t| t.strip_prefix("retry:"))
        .and_then(|id| id.parse::<i64>().ok())
        .map(routine_ref)
}

/// The first two steps of every run: its trigger and its checks. `skip`
/// is why it did not start, when it did not.
pub fn opening(
    r: &RoutineRow,
    run: &RoutineRunRow,
    checks: &[RoutineCheck],
    skip: Option<&str>,
    now: i64,
) -> Vec<Step> {
    let trigger = Step::new(
        now,
        "trigger",
        "trigger",
        "scheduler",
        trigger_words(&run.trigger, run.trigger_ref.as_deref()),
    )
    .data(serde_json::json!({
        "routine_id": r.id,
        "routine": r.name,
        "trigger": run.trigger,
        "trigger_ref": run.trigger_ref,
        "scheduled_for": run.scheduled_for,
    }));
    let warn = checks.iter().filter(|c| c.state == "warn").count();
    let summary = match skip {
        Some(why) => format!("Skipped: {why}"),
        None if warn > 0 => format!("{} checks passed, {warn} with a caveat", checks.len()),
        None => format!("{} checks passed", checks.len()),
    };
    let checks_step = Step::new(now, "checks", "check", "scheduler", summary)
        .data(serde_json::json!({ "checks": checks, "skipped": skip }));
    vec![trigger, checks_step]
}

/// Record `steps` on routine run `run`.
pub fn write(s: &Store, run: &RoutineRunRow, steps: Vec<Step>) {
    let id = routine_ref(run.id);
    let parent = parent(run);
    let at = RunRef {
        run_ref: &id,
        attempt: 1,
        parent_ref: parent.as_deref(),
    };
    record_all(s, &at, steps);
}

/// Record one step on routine run `run_id`.
pub fn write_one(s: &Store, run_id: i64, step: Step) {
    let id = routine_ref(run_id);
    record(s, &RunRef::first(&id), step);
}

/// The step a start of the run's session wrote: `Ok(session id)` or the
/// error, on `host`, taking `ms`.
pub fn spawn_step(host: &str, result: Result<i64, &str>, ms: i64, now: i64) -> Step {
    match result {
        Ok(sid) => Step::new(
            now,
            "spawn",
            "spawn",
            "hub",
            format!("Session started on {host}"),
        )
        .data(serde_json::json!({ "host": host, "session_id": sid })),
        Err(why) => Step::new(
            now,
            "spawn",
            "error",
            "hub",
            format!("The start on {host} failed: {why}"),
        )
        .data(serde_json::json!({ "host": host, "error": why })),
    }
    .ms(ms)
}

/// Where the run was placed: its host, its login and why a fallback.
pub fn placement_step(r: &RoutineRow, host: &str, note: Option<&str>, now: i64) -> Step {
    let summary = match note {
        Some(n) => format!("Placed {n}"),
        None => format!("Placed on {host}"),
    };
    Step::new(now, "placement", "placement", "scheduler", summary).data(serde_json::json!({
        "host": host,
        "profile": r.profile,
        "fallback_host": r.fallback_host,
        "note": note,
    }))
}

/// How a run closed: `state`, why, its error code and what it cost.
pub fn settle_step(
    state: &str,
    reason: Option<&str>,
    code: Option<&str>,
    cost_micros: i64,
    now: i64,
) -> Step {
    let summary = match (state, reason) {
        ("done", _) => "Its turn finished".to_string(),
        (_, Some(why)) => {
            let mut w = why.to_string();
            if let Some(first) = w.get(..1) {
                w.replace_range(..1, &first.to_uppercase());
            }
            w
        }
        (other, None) => format!("Closed {other}"),
    };
    let kind = if state == "failed" { "error" } else { "settle" };
    Step::new(now, "settle", kind, "scheduler", summary)
        .data(serde_json::json!({ "state": state, "reason": reason, "error_code": code }))
        .cost(cost_micros)
}

/// Who judged a run's outcome, and what: the run's exit, a rule or Jev.
pub fn outcome_step(outcome: &str, source: &str, decision_id: Option<i64>, now: i64) -> Step {
    let words = match outcome {
        "did_work" => "did work",
        "nothing" => "had nothing to do",
        "needs_person" => "needs a person",
        "failed" => "failed",
        other => other,
    };
    let (by, actor) = match source {
        "exit" => (", judged by how it ended", "scheduler"),
        "jev" => (", judged by Jev", "jev"),
        _ => (", judged by a rule", "rule"),
    };
    Step::new(now, "outcome", "decision", actor, format!("It {words}{by}"))
        .data(serde_json::json!({ "outcome": outcome, "source": source }))
        .decision(decision_id)
}
