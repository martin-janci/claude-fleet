//! What a finished routine run came to (Orbit Fleet redesign step 8.10,
//! N6 in the AI plan): did work, nothing to do, failed, or needs a person.
//! `state` says only whether the run ran; this says whether anyone should
//! look at it, so a morning sweep that found nothing stays out of the Inbox.
//!
//! Three sources answer, the strongest standing
//! ([`ROUTINE_RUN_OUTCOME_SOURCES`](crate::store::ROUTINE_RUN_OUTCOME_SOURCES)):
//!
//! - **exit**: the run failed (an error ending its turn, its session lost or
//!   removed, a budget, no turn in 6 hours). Always `failed`, and nothing
//!   replaces it.
//! - **rule**: a fact on the session when the turn ended. An open question
//!   or a wedged REPL is `needs_person`; a pull request is `did_work`.
//! - **jev**: the rest (a turn that ended with neither), read from the pane
//!   tail by the routine_run_outcome use case, which shares J2's reading
//!   (step 5.11) and is off until its setting is
//!   (`service::decide::routine_run_outcome`). Until it answers the run has
//!   no outcome and shows as it does today.
//!
//! A `nothing` answer marks the run's session seen, so its finished turn
//! does not land in the Inbox as unread (`done_unread`, step 2.3).

use crate::ipc_error::IpcError;
use crate::service::attention::{needs_attention, Reason};
use crate::store::{RoutineRunRow, SessionRow, Store};

/// What a run came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    DidWork,
    Nothing,
    Failed,
    NeedsPerson,
}

impl RunOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            RunOutcome::DidWork => "did_work",
            RunOutcome::Nothing => "nothing",
            RunOutcome::Failed => "failed",
            RunOutcome::NeedsPerson => "needs_person",
        }
    }

    pub fn parse(s: &str) -> Option<RunOutcome> {
        [
            RunOutcome::DidWork,
            RunOutcome::Nothing,
            RunOutcome::Failed,
            RunOutcome::NeedsPerson,
        ]
        .into_iter()
        .find(|o| o.as_str() == s)
    }
}

/// Who answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeSource {
    Exit,
    Rule,
    Jev,
}

impl OutcomeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            OutcomeSource::Exit => "exit",
            OutcomeSource::Rule => "rule",
            OutcomeSource::Jev => "jev",
        }
    }
}

/// The answer the run's own facts give, without Jev: `None` when they say
/// nothing (a done run whose turn just ended), which is Jev's to read.
/// `row` is the run's session, if it still exists.
pub fn rule_outcome(
    run: &RoutineRunRow,
    row: Option<&SessionRow>,
) -> Option<(RunOutcome, OutcomeSource)> {
    match run.state.as_str() {
        "failed" => return Some((RunOutcome::Failed, OutcomeSource::Exit)),
        "done" => {}
        // Running has no outcome yet; skipped never ran.
        _ => return None,
    }
    let row = row?;
    if let Some(a) = needs_attention(row) {
        if matches!(a.reason, Reason::Waiting | Reason::Stuck) {
            return Some((RunOutcome::NeedsPerson, OutcomeSource::Rule));
        }
    }
    // J2's answer, when it has one (step 5.11): a turn that ended asking,
    // or wedged, waits on a person whatever the hooks missed.
    if matches!(row.turn_outcome.as_deref(), Some("asked" | "stuck")) {
        return Some((RunOutcome::NeedsPerson, OutcomeSource::Rule));
    }
    // A routine's session is its own (one per run), so a pull request on it
    // is this run's work.
    if row.pr_url.is_some() {
        return Some((RunOutcome::DidWork, OutcomeSource::Rule));
    }
    None
}

/// Record `outcome` from `source` for `run`, unless a stronger source
/// already answered. A `nothing` that lands marks the run's session seen,
/// so it stays out of the Inbox; an exit or rule answer that replaces
/// Jev's marks Jev's decision run. Answers whether the run changed.
pub fn record(
    s: &Store,
    run: &RoutineRunRow,
    outcome: RunOutcome,
    source: OutcomeSource,
    now: i64,
) -> Result<bool, IpcError> {
    let changed = s.set_routine_run_outcome(run.id, outcome.as_str(), source.as_str())?;
    // The exit or a rule replaced Jev's answer: that is its follow-up.
    if changed
        && source != OutcomeSource::Jev
        && run.outcome_source.as_deref() == Some(OutcomeSource::Jev.as_str())
    {
        crate::service::decide::routine_run_outcome::record_rule(s, run.id, outcome.as_str(), now)?;
    }
    if changed && outcome == RunOutcome::Nothing {
        if let Some(sid) = run.session_id {
            s.touch_session_viewed(sid, now)?;
        }
    }
    Ok(changed)
}

/// Apply [`rule_outcome`] to a run that just closed (the scheduler's
/// settle pass calls it after each close).
pub fn on_close(s: &Store, run_id: i64, now: i64) -> Result<(), IpcError> {
    let Some(run) = s.get_routine_run(run_id)? else {
        return Ok(());
    };
    let row = match run.session_id {
        Some(sid) => s.get_session_by_id(sid)?,
        None => None,
    };
    if let Some((o, src)) = rule_outcome(&run, row.as_ref()) {
        record(s, &run, o, src, now)?;
    }
    Ok(())
}
