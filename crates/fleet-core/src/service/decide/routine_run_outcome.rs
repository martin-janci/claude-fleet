//! N6 `routine_run_outcome` (Orbit Fleet redesign step 8.10, the Jev
//! half): what a finished routine run came to when its own facts say
//! nothing. A morning sweep that found nothing ends its turn like one that
//! opened three pull requests; the exit and the rules
//! (`service::routines::outcome`) read a failure, an open question and a
//! pull request, and leave the rest unanswered. This adapter asks the
//! decision model one Choice over the end of the run's screen —
//! `did_work`, `nothing`, `needs_person` or `unsure` — and nothing more:
//!
//! * **Exit and rules always win.** The answer is written with the `jev`
//!   source, the weakest (`ROUTINE_RUN_OUTCOME_SOURCES`): a failed exit or
//!   a rule's answer is never replaced, and a rule that answers later
//!   replaces Jev's and marks it ([`record_rule`]).
//! * **Shadow / assist only.** In `shadow` the answer is only recorded,
//!   with today's reading (every finished run is work to look at,
//!   [`BASELINE`]) as the baseline. In `assist` a usable answer is the
//!   run's outcome; `nothing` marks the run's session seen, so its finished
//!   turn stays out of the Inbox. Nothing is typed, killed or sent to the
//!   session, and the run's session stays where it is for a person to open.
//! * **What is sent.** J2's reading of the screen (step 5.11,
//!   [`turn_outcome::prepare_tail`]): the ANSI-stripped visible pane tail,
//!   chrome left out, fenced code replaced by placeholders, through the
//!   envelope's redaction. It is Claude's reply text, so the org must ALSO
//!   consent to reply text (D48), as for J2.
//! * **When.** The scheduler's pass ([`spawn_pass`]) reads the runs that
//!   finished in the last [`RECENT_SECS`] without an outcome, at most
//!   [`PER_PASS`] a pass, off the scheduler's path. With the setting off it
//!   costs one short lock and spawns nothing.
//! * **What is recorded.** One run per routine run: subject `routine_run`
//!   `<run id>`; a run already decided is never asked again.

use super::start_project::decided;
use super::turn_outcome::prepare_tail;
use super::{decide, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode, Question};
use crate::ipc_error::{lock, IpcError};
use crate::service::routines::outcome::{self, OutcomeSource, RunOutcome};
use crate::ssh::SshClient;
use crate::store::{RoutineRunRow, SessionRow, Store};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "routine_run_outcome.v1";
/// What a decision run is about: one routine run (its id as text).
pub const SUBJECT_KIND: &str = "routine_run";
/// Below this confidence an answer is recorded, not applied.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// The option that says nothing.
pub const UNSURE: &str = "unsure";
/// Today's reading, the shadow baseline: a finished run lands in the Inbox
/// as work to look at.
pub const BASELINE: &str = "did_work";
/// Only runs that finished this recently are read: an older run's screen
/// has moved on, or its pane is gone.
pub const RECENT_SECS: i64 = 3600;
/// Runs read per pass, at most.
pub const PER_PASS: i64 = 5;

/// The instruction, read literally: the exact condition and what to use.
pub const INSTRUCTIONS: &str = "state.screen is the end of a terminal screen of Claude Code, an \
     AI coding assistant, captured after it finished a scheduled task nobody was watching: its \
     last reply and what is below it. Code is replaced by [code: ...] placeholders. Decide what \
     the task came to, using only the screen. Choose did_work when the reply reports something \
     it changed, made, found or reported that a person should read; nothing when it reports \
     there was nothing to do or nothing new (no matching items, no changes, all clear); \
     needs_person when it asks the person a question or for a decision, or says it could not \
     go on without one. Choose unsure when the screen does not show which.";

/// PURE: the question: one Choice over what a run came to.
pub fn question() -> Question {
    let criteria: [(&str, &str); 4] = [
        (
            "did_work",
            "The reply reports something done, made, found or reported that a person should read.",
        ),
        (
            "nothing",
            "The reply reports there was nothing to do or nothing new, and asks nothing.",
        ),
        (
            "needs_person",
            "The reply asks the person a question or for a decision, or says it cannot go on \
             without one.",
        ),
        (
            UNSURE,
            "The screen does not show which of these the task came to.",
        ),
    ];
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: criteria
            .into_iter()
            .map(|(k, v)| (k.to_string(), Some(Value::String(v.into()))))
            .collect(),
    }
}

/// PURE: the request for a screen (already [`prepare_tail`]d).
pub fn request(screen: &str) -> JevRequest {
    JevRequest {
        state: json!({ "screen": screen }),
        question: question(),
    }
}

/// PURE: the outcome an answer word applies; `None` for `unsure` or
/// anything else.
pub fn applied(answer: &str) -> Option<RunOutcome> {
    match answer {
        "did_work" => Some(RunOutcome::DidWork),
        "nothing" => Some(RunOutcome::Nothing),
        "needs_person" => Some(RunOutcome::NeedsPerson),
        _ => None,
    }
}

/// Reads a session's visible pane: tmux on its host in production, a
/// script in tests.
#[async_trait::async_trait]
pub trait PaneReader: Send + Sync {
    async fn tail(&self, row: &SessionRow) -> Result<String, IpcError>;
}

/// [`PaneReader`] through tmux, locally or over SSH.
pub struct TmuxPanes {
    pub ssh: Arc<SshClient>,
}

#[async_trait::async_trait]
impl PaneReader for TmuxPanes {
    async fn tail(&self, row: &SessionRow) -> Result<String, IpcError> {
        let tmux: Box<dyn crate::tmux::TmuxExec> = if row.host_alias == "local" {
            Box::new(crate::tmux::LocalTmux)
        } else {
            Box::new(crate::tmux::RemoteTmux {
                client: Arc::clone(&self.ssh),
                host: row.host_alias.clone(),
            })
        };
        tmux.capture_pane(&row.tmux_name).await
    }
}

/// What the adapter asks about: one finished run and its session.
#[derive(Debug, Clone, PartialEq)]
pub struct RunInput {
    pub run: RoutineRunRow,
    pub session: SessionRow,
    /// The run's org (its session's, else its routine's): both consents
    /// are checked.
    pub org_id: Option<i64>,
}

/// A session whose pane can be read.
fn has_pane(row: &SessionRow) -> bool {
    row.agent == crate::store::AGENT_CLAUDE
        && row.kind != "shell"
        && !crate::store::has_no_pane(&row.kind)
        && row.lost_at.is_none()
}

/// The runs to read now: finished since `now -` [`RECENT_SECS`] with no
/// outcome, whose session still has a pane, the gate open for their org,
/// not decided already. Short: the caller holds the store lock for it.
pub fn pending(s: &Store, now: i64) -> Result<Vec<RunInput>, IpcError> {
    let mut out = Vec::new();
    for run in s.recent_routine_runs_without_outcome(now - RECENT_SECS, PER_PASS)? {
        let Some(sid) = run.session_id else { continue };
        let Some(session) = s.get_session_by_id(sid)? else {
            continue;
        };
        if !has_pane(&session) {
            continue;
        }
        let org_id = match session.org_id {
            Some(o) => Some(o),
            None => s.get_routine(run.routine_id)?.and_then(|r| r.org_id),
        };
        if gate_at(s, Feature::RoutineRunOutcome, org_id, now).is_err() {
            continue;
        }
        let subject = run.id.to_string();
        let runs = s.decision_runs_for_subjects(
            Feature::RoutineRunOutcome.as_str(),
            SUBJECT_KIND,
            &subject,
            None,
        )?;
        if runs.iter().any(|r| r.subject_id == subject && decided(r)) {
            continue;
        }
        out.push(RunInput {
            run,
            session,
            org_id,
        });
    }
    Ok(out)
}

/// What [`ask`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum Asked {
    /// Not asked: the gate refused, or the screen was empty. Nothing
    /// recorded.
    Skipped,
    /// Asked and recorded; nothing applied (shadow, a fallback, `unsure`,
    /// under the floor, or the exit or a rule answered first).
    Recorded,
    /// Assist: the answer is the run's outcome now.
    Applied {
        outcome: String,
        run_id: Option<i64>,
    },
}

/// Ask the decision model what `input`'s run came to from its screen
/// `tail` (raw), and in `assist` apply a usable answer. Every failure is a
/// recorded fallback or nothing at all, never an error. Holds the store
/// lock only briefly, never across the call.
pub async fn ask(ctx: &DecideCtx, input: &RunInput, tail: &str) -> Asked {
    let now = ctx.now();
    let screen = prepare_tail(tail);
    if screen.trim().is_empty() {
        return Asked::Skipped;
    }
    let subject = input.run.id.to_string();
    let out = decide(
        ctx,
        DecideRequest {
            feature: Feature::RoutineRunOutcome,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: subject,
            org_id: input.org_id,
            request: request(&screen),
            baseline: Some(BASELINE.into()),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    let Some(answer) = out.proposal() else {
        return Asked::Recorded;
    };
    let Some(o) = applied(&answer.value) else {
        return Asked::Recorded;
    };
    let Ok(s) = lock(&ctx.store) else {
        return Asked::Recorded;
    };
    // Re-read: the exit or a rule may have answered while Jev was asked.
    let run = match s.get_routine_run(input.run.id) {
        Ok(Some(r)) => r,
        _ => return Asked::Recorded,
    };
    match outcome::record(&s, &run, o, OutcomeSource::Jev, now) {
        Ok(true) => Asked::Applied {
            outcome: o.as_str().to_string(),
            run_id: out.run_id,
        },
        Ok(false) => Asked::Recorded,
        Err(e) => {
            tracing::warn!("[decide] routine_run_outcome not applied: {}", e.message);
            Asked::Recorded
        }
    }
}

/// Read each pending run's screen and [`ask`], one after the other.
pub async fn read_pending(ctx: &DecideCtx, panes: &dyn PaneReader) {
    let now = ctx.now();
    let inputs = {
        let Ok(s) = lock(&ctx.store) else { return };
        match pending(&s, now) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("[decide] routine_run_outcome not read: {}", e.message);
                return;
            }
        }
    };
    for input in inputs {
        match panes.tail(&input.session).await {
            Ok(tail) => {
                ask(ctx, &input, &tail).await;
            }
            Err(e) => tracing::debug!(
                run = input.run.id,
                error = %e.message,
                "[decide] routine_run_outcome: pane not captured"
            ),
        }
    }
}

/// One pass at a time: a slow capture never stacks passes up.
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// The scheduler's pass: when the setting could let a call through at all
/// and a run waits, [`read_pending`] off the scheduler's path. With the
/// defaults (the feature `off`) it costs one short lock and spawns nothing.
pub fn spawn_pass(store: &Arc<Mutex<Store>>, panes: Arc<dyn PaneReader>, now: i64) {
    let waiting = lock(store).ok().is_some_and(|s| {
        super::FeatureMode::of(&s, Feature::RoutineRunOutcome) != super::FeatureMode::Off
            && pending(&s, now).is_ok_and(|p| !p.is_empty())
    });
    if !waiting || IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    let ctx = DecideCtx::jev(Arc::clone(store));
    let spawned = crate::rt::try_spawn(async move {
        read_pending(&ctx, panes.as_ref()).await;
        IN_FLIGHT.store(false, Ordering::SeqCst);
    });
    if spawned.is_none() {
        IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

/// The exit or a rule answered run `run_id` after Jev did, and replaced
/// Jev's answer with `said`: mark the run's assist answer nobody decided
/// `confirmed` (the same) or `corrected` (to `said`). Never a shadow answer
/// (D34, D37). Returns whether a run was marked.
pub fn record_rule(s: &Store, run_id: i64, said: &str, now: i64) -> Result<bool, IpcError> {
    let subject = run_id.to_string();
    let runs = s.decision_runs_for_subjects(
        Feature::RoutineRunOutcome.as_str(),
        SUBJECT_KIND,
        &subject,
        None,
    )?;
    let Some(r) = runs.into_iter().find(|r| {
        r.subject_id == subject
            && r.mode == Mode::Assist.as_str()
            && r.fallback.is_none()
            && r.answer.as_deref().is_some_and(|a| applied(a).is_some())
    }) else {
        return Ok(false);
    };
    if r.followup.is_some() {
        return Ok(false);
    }
    if r.answer.as_deref() == Some(said) {
        s.set_decision_followup(r.id, "confirmed", None, now)
    } else {
        s.set_decision_followup(r.id, "corrected", Some(said), now)
    }
}
