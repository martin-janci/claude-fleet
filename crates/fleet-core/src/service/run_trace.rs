//! Run traces (migration 194): what happened inside one run, step by step,
//! so the Automation screen can draw it while it goes and a person or Jev
//! can read afterwards why it went that way.
//!
//! Writers call [`record`] at each step; a failure to record is logged and
//! never fails the run. Readers call [`read_in`] (`runs { trace }`), which
//! answers only a run the caller may see: the same reach as `runs { list }`
//! (`service::runs::reach`).

use crate::ipc_error::{codes, IpcError};
use crate::service::view_scope::ViewScope;
use crate::store::{NewRunTrace, RunTraceRow, RunsReach, Store};
use serde::{Deserialize, Serialize};

/// Steps one read answers by default.
pub const TRACE_DEFAULT_LIMIT: i64 = 500;
/// The most steps one read answers.
pub const TRACE_MAX_LIMIT: i64 = 2000;

/// A routine run's id in `runs { list }` and in its trace.
pub fn routine_ref(run_id: i64) -> String {
    format!("routine:{run_id}")
}

/// An agent job's id in `runs { list }` and in its trace.
pub fn agent_ref(job_id: i64) -> String {
    format!("agent:{job_id}")
}

/// One step to record. Build it with [`Step::new`] and the setters.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub at: i64,
    pub phase: &'static str,
    pub kind: &'static str,
    pub actor: &'static str,
    pub summary: String,
    pub data: Option<serde_json::Value>,
    pub duration_ms: Option<i64>,
    pub cost_micros: Option<i64>,
    pub decision_id: Option<i64>,
}

impl Step {
    pub fn new(
        at: i64,
        phase: &'static str,
        kind: &'static str,
        actor: &'static str,
        summary: impl Into<String>,
    ) -> Self {
        Step {
            at,
            phase,
            kind,
            actor,
            summary: summary.into(),
            data: None,
            duration_ms: None,
            cost_micros: None,
            decision_id: None,
        }
    }
    pub fn data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
    pub fn ms(mut self, ms: i64) -> Self {
        self.duration_ms = Some(ms);
        self
    }
    pub fn cost(mut self, micros: i64) -> Self {
        self.cost_micros = Some(micros);
        self
    }
    pub fn decision(mut self, id: Option<i64>) -> Self {
        self.decision_id = id;
        self
    }
}

/// Which run, and which try of it, a step belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRef<'a> {
    pub run_ref: &'a str,
    pub attempt: i64,
    /// The run this one retries, if it does.
    pub parent_ref: Option<&'a str>,
}

impl<'a> RunRef<'a> {
    pub fn first(run_ref: &'a str) -> Self {
        RunRef {
            run_ref,
            attempt: 1,
            parent_ref: None,
        }
    }
}

/// Record `step` of `run`. A failure is logged, never answered: the trace
/// is for reading a run, and a run never fails because it could not be
/// written down.
pub fn record(s: &Store, run: &RunRef<'_>, step: Step) {
    let res = s.insert_run_trace(&NewRunTrace {
        run_ref: run.run_ref,
        attempt: run.attempt,
        parent_ref: run.parent_ref,
        at: step.at,
        phase: step.phase,
        kind: step.kind,
        actor: step.actor,
        summary: &step.summary,
        data: step.data.as_ref(),
        duration_ms: step.duration_ms,
        cost_micros: step.cost_micros,
        decision_id: step.decision_id,
    });
    if let Err(e) = res {
        tracing::warn!(run = run.run_ref, error = %e.message, "[run_trace] recording a step failed");
    }
}

/// [`record`] each of `steps`, in order.
pub fn record_all(s: &Store, run: &RunRef<'_>, steps: Vec<Step>) {
    for step in steps {
        record(s, run, step);
    }
}

/// `runs { action: trace, run_id, after_seq?, limit? }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceArgs {
    /// The run's id from `runs { list }`: `routine:418`, `agent:91`.
    pub run_id: String,
    /// Only the steps after this one (a live view reads on from its last).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_seq: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

/// What `runs { trace }` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTrace {
    pub run_id: String,
    /// Oldest first.
    pub steps: Vec<RunTraceRow>,
    /// More steps follow the last one answered: read on from its `seq`.
    pub more: bool,
}

/// Whether a caller with `reach` may read run `run_id`'s trace. A run of a
/// source that keeps no trace is not found.
fn visible(s: &Store, reach: &RunsReach, run_id: &str) -> Result<bool, IpcError> {
    let Some((source, id)) = run_id.split_once(':') else {
        return Ok(false);
    };
    let Ok(id) = id.parse::<i64>() else {
        return Ok(false);
    };
    if !matches!(source, "routine" | "agent") {
        return Ok(false);
    }
    let RunsReach::Scoped {
        routines, spend, ..
    } = reach
    else {
        return Ok(true);
    };
    Ok(match source {
        "routine" => s
            .get_routine_run(id)?
            .is_some_and(|run| routines.contains(&run.routine_id)),
        // `store::runs`: an agent's job belongs to no session or mission,
        // so only whole-fleet spend sees it.
        _ => *spend,
    })
}

/// Run `args.run_id`'s steps, when `scope` may see the run.
pub fn read_in(s: &Store, scope: &ViewScope, args: &TraceArgs) -> Result<RunTrace, IpcError> {
    let reach = crate::service::runs::reach(s, scope)?;
    if !visible(s, &reach, &args.run_id)? {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("no run {:?} with a trace", args.run_id),
        ));
    }
    let limit = args
        .limit
        .unwrap_or(TRACE_DEFAULT_LIMIT)
        .clamp(1, TRACE_MAX_LIMIT);
    let mut steps = s.run_trace(&args.run_id, args.after_seq.unwrap_or(0), limit + 1)?;
    let more = steps.len() as i64 > limit;
    steps.truncate(limit as usize);
    Ok(RunTrace {
        run_id: args.run_id.clone(),
        steps,
        more,
    })
}

/// [`read_in`] on a store behind its lock.
pub fn read(
    store: &std::sync::Mutex<Store>,
    scope: &ViewScope,
    args: &TraceArgs,
) -> Result<RunTrace, IpcError> {
    let s = crate::ipc_error::lock(store)?;
    read_in(&s, scope, args)
}
