//! An agent job's steps in its run trace (`service::run_trace`, migration
//! 193): when it was picked up, each try, how each ended and whether it
//! will be tried again. A job retries in its own row, so each try is an
//! `attempt` of the one run `agent:<job id>`.

use super::RunOutcome;
use crate::service::run_trace::{agent_ref, record, RunRef, Step};
use crate::store::{AgentJobEnd, AgentJobRow, Store};

fn at<'a>(id: &'a str, job: &AgentJobRow) -> RunRef<'a> {
    RunRef {
        run_ref: id,
        attempt: i64::from(job.attempts.max(1)),
        parent_ref: None,
    }
}

/// A try of `job` started: on its first, what found it; on each, the try.
pub fn started(s: &Store, job: &AgentJobRow, now: i64) {
    let id = agent_ref(job.id);
    let run = at(&id, job);
    if job.attempts <= 1 {
        record(
            s,
            &run,
            Step::new(
                now,
                "trigger",
                "trigger",
                "scheduler",
                format!("{} picked up {}", job.agent, job.subject),
            )
            .data(serde_json::json!({
                "agent": job.agent,
                "rule_id": job.rule_id,
                "subject": job.subject,
                "trigger": job.trigger,
                "found_at": job.created_at,
                "waited_secs": (now - job.created_at).max(0),
            })),
        );
    }
    record(
        s,
        &run,
        Step::new(
            now,
            "spawn",
            "spawn",
            "hub",
            format!("Try {} started", job.attempts.max(1)),
        ),
    );
}

/// A try of `job` ended in `outcome`, recorded as `end`, after `ms`.
pub fn ended(
    s: &Store,
    job: &AgentJobRow,
    outcome: &RunOutcome,
    end: &AgentJobEnd,
    ms: i64,
    now: i64,
) {
    let id = agent_ref(job.id);
    let run = at(&id, job);
    let (kind, summary, cost) = match outcome {
        RunOutcome::Done { cost_micros, .. } => ("settle", "Finished".to_string(), *cost_micros),
        RunOutcome::Skipped { reason } => ("settle", format!("Nothing to do: {reason}"), 0),
        RunOutcome::NeedsPerson {
            reason,
            cost_micros,
            ..
        } => ("waiting", format!("Needs a person: {reason}"), *cost_micros),
        RunOutcome::Failed {
            kind,
            message,
            cost_micros,
        } => (
            "error",
            format!("Failed ({}): {message}", kind.as_str()),
            *cost_micros,
        ),
    };
    record(
        s,
        &run,
        Step::new(now, "settle", kind, "hub", summary)
            .ms(ms)
            .cost(cost),
    );
    let step = match end {
        AgentJobEnd::Retry { at, error_kind, .. } => Step::new(
            now,
            "outcome",
            "retry",
            "scheduler",
            format!("Will try again in {}s", (at - now).max(0)),
        )
        .data(serde_json::json!({ "next_attempt_at": at, "error_kind": error_kind })),
        AgentJobEnd::Final { state, .. } => Step::new(
            now,
            "outcome",
            "decision",
            "hub",
            format!("Ended {}", state.replace('_', " ")),
        )
        .data(serde_json::json!({ "state": state })),
    };
    record(s, &run, step);
}
