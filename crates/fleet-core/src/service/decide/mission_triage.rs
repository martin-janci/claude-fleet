//! K3 `mission_triage` (redesign step 9.10): what a stuck mission's outcome
//! is so far and what to do next. The canvas board's Nudge card: "Partial ·
//! Proposed by Jev · Retry / Split / Give up / Ask".
//!
//! * **When.** A person opens a stuck mission (Missions, or Today's Nudge)
//!   and the view asks; nothing asks on a timer. A mission is stuck
//!   ([`stuck`]) when its loop braked (budget or no progress) and it is
//!   still paused, when a member item failed, or when nothing is ready
//!   while something is blocked.
//! * **What is sent.** Fleet's own stuck reason, the mission's goal and
//!   done-when lines, the counts of done, failed and blocked items, and the
//!   start of the last failure ([`FAILURE_CHARS`]), through the envelope's
//!   redaction. When that failure is the worker's own summary (Claude's
//!   text) it is sent only with the org's reply-text consent (D48). Two closed questions: the outcome ([`OUTCOMES`]) and the
//!   next step ([`NEXT_STEPS`]), each with `unsure`.
//! * **Shadow** records the answers. **Assist** returns a usable answer (at
//!   or above [`MIN_CONFIDENCE`], not `unsure`) as a proposal the card
//!   shows; a person picks the step through the action they already have.
//! * **Never decides completion.** Triage reads the store and records
//!   decision runs, nothing else: it never completes a mission, never
//!   changes its state and never records a verification, whatever the
//!   outcome says. An outcome of `done` is a reading; Verified stays proof.
//! * **Asked once a week.** A decided run on the same input and question
//!   version is reused for [`REUSE_SECS`] rather than asked again.

use super::{decide, fingerprint, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode};
use super::{DecisionOutcome, Question};
use crate::ipc_error::lock;
use crate::service::work::missions::MissionDetail;
use crate::store::{DecisionProposal, DecisionRunRow, MissionRow, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The question's version: bump it when a question below changes.
pub const QUESTION_VERSION: &str = "mission_triage.v1";
/// What a run is about: a mission, `<id>:outcome` or `<id>:next`.
pub const SUBJECT_KIND: &str = "mission";
/// The outcome so far.
pub const OUTCOMES: [&str; 4] = ["done", "partial", "blocked", "failed"];
/// The next step.
pub const NEXT_STEPS: [&str; 4] = ["retry", "split", "give_up", "ask"];
/// The option that proposes nothing.
pub const UNSURE: &str = "unsure";
/// Below this confidence an answer is recorded, not proposed.
pub const MIN_CONFIDENCE: f64 = crate::store::PROPOSAL_MIN_CONFIDENCE;
/// A decided run on the same input is reused this long.
pub const REUSE_SECS: i64 = 7 * 86_400;
/// Characters of the last failure sent, at most.
pub const FAILURE_CHARS: usize = 600;

const OUTCOME_INSTRUCTIONS: &str = "state describes a mission that has stopped making progress: \
    why fleet thinks it is stuck, its goal and the lines that say when it is done, how many of \
    its tasks are done, failed and blocked, and the last failure. Decide what the mission's \
    outcome is so far. Choose unsure when the state does not say.";

const NEXT_INSTRUCTIONS: &str = "state describes a mission that has stopped making progress: \
    why fleet thinks it is stuck, its goal and the lines that say when it is done, how many of \
    its tasks are done, failed and blocked, and the last failure. Decide the one next step most \
    likely to get it moving. Choose unsure when the state does not say.";

/// PURE: what each outcome means, as the question says it.
fn outcome_means(o: &str) -> &'static str {
    match o {
        "done" => "The goal looks met: what is left does not matter to it.",
        "partial" => "Some of the goal is met and the rest can still be done.",
        "blocked" => "It waits on something outside it: a person, another task, access.",
        "failed" => "The work that was tried does not work and will not as it stands.",
        _ => "The state does not say.",
    }
}

/// PURE: what each next step means, as the question says it.
fn next_means(n: &str) -> &'static str {
    match n {
        "retry" => "Run the failed or stalled task again as it is.",
        "split" => "Break the stuck task into smaller tasks and plan again.",
        "give_up" => "Stop the mission: what it wants is not worth more runs.",
        "ask" => "Ask a person a question the mission cannot answer itself.",
        _ => "The state does not say.",
    }
}

/// Why fleet calls a mission stuck, in its own words and counts: what the
/// card shows and what the questions are asked about.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stuck {
    /// `budget`, `no_progress`, `failed` or `blocked`.
    pub reason: String,
    /// Fleet's words for it ("the workers spent $4.00 of the grant's $4.00").
    pub why: String,
    pub done: usize,
    pub failed: usize,
    pub blocked: usize,
    pub total: usize,
    /// The start of the newest failed attempt's error or summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
    /// `last_failure` is the worker's own summary, which is Claude's text:
    /// it reaches Jev only with the org's reply-text consent (D48).
    #[serde(skip)]
    pub last_failure_is_reply: bool,
}

/// What a person sees: the outcome and the next step Jev proposes, each
/// `None` when nothing may be proposed (off, shadow, unsure, too unsure).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposals {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<DecisionProposal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<DecisionProposal>,
}

/// PURE: a count in words, "1 task" / "3 tasks".
fn tasks(n: usize) -> String {
    format!("{n} task{}", if n == 1 { "" } else { "s" })
}

/// PURE: whether `detail`'s mission is stuck, and why. Only a mission that
/// is active or paused can be; a braked one only while it stays paused
/// since the brake.
pub fn stuck(detail: &MissionDetail) -> Option<Stuck> {
    let m = &detail.mission;
    if m.state != "active" && m.state != "paused" {
        return None;
    }
    let count = |s: &str| detail.graph.nodes.iter().filter(|n| n.state == s).count();
    let (done, failed, blocked) = (count("done"), count("failed"), count("blocked"));
    let newest = detail
        .graph
        .nodes
        .iter()
        .filter(|n| n.state == "failed")
        .filter_map(|n| n.attempt.as_ref())
        .max_by_key(|a| a.task_id);
    let last_failure_is_reply = newest.is_some_and(|a| a.error.is_none() && a.summary.is_some());
    let last_failure = newest
        .and_then(|a| a.error.clone().or_else(|| a.summary.clone()))
        .map(|t| t.chars().take(FAILURE_CHARS).collect());
    // Events come newest first; a brake still holds while nothing changed
    // the mission after it.
    let brake = detail
        .events
        .iter()
        .find(|e| e.kind == "budget" || e.kind == "no_progress")
        .filter(|e| m.state == "paused" && e.at >= m.updated_at);
    let (reason, why) = if let Some(e) = brake {
        let said = e
            .payload
            .as_ref()
            .and_then(|p| p.get("why"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let why = match e.kind.as_str() {
            "budget" => format!("Paused on its budget: {said}"),
            _ => format!("Paused for no progress: {said}"),
        };
        (e.kind.clone(), why)
    } else if failed > 0 {
        ("failed".to_string(), format!("{} failed", tasks(failed)))
    } else if m.state == "active" && blocked > 0 && detail.graph.phase() == "blocked" {
        (
            "blocked".to_string(),
            format!("Nothing is ready; {} blocked", tasks(blocked)),
        )
    } else {
        return None;
    };
    Some(Stuck {
        reason,
        why: why.trim_end_matches([':', ' ']).to_string(),
        done,
        failed,
        blocked,
        total: detail.graph.nodes.len(),
        last_failure,
        last_failure_is_reply,
    })
}

/// PURE: the state both questions are asked about.
pub fn state(m: &MissionRow, s: &Stuck) -> Value {
    json!({
        "stuck": { "reason": s.reason, "why": s.why },
        "goal": m.goal,
        "done_when": m.done_when,
        "tasks": { "done": s.done, "failed": s.failed, "blocked": s.blocked, "total": s.total },
        "last_failure": s.last_failure.clone().unwrap_or_default(),
    })
}

/// PURE: one choice over `options` and `unsure`.
fn choice(instructions: &str, options: &[&str], means: fn(&str) -> &'static str) -> Question {
    let criteria = options
        .iter()
        .chain(std::iter::once(&UNSURE))
        .map(|o| (o.to_string(), Some(Value::String(means(o).into()))))
        .collect();
    Question::Choice {
        instructions: Value::String(instructions.into()),
        criteria,
    }
}

/// PURE: the outcome question's request.
pub fn outcome_request(m: &MissionRow, s: &Stuck) -> JevRequest {
    JevRequest {
        state: state(m, s),
        question: choice(OUTCOME_INSTRUCTIONS, &OUTCOMES, outcome_means),
    }
}

/// PURE: the next-step question's request.
pub fn next_request(m: &MissionRow, s: &Stuck) -> JevRequest {
    JevRequest {
        state: state(m, s),
        question: choice(NEXT_INSTRUCTIONS, &NEXT_STEPS, next_means),
    }
}

/// A run that sent a request and got an answer the envelope checked (or
/// found wanting): asking again on the same input would repeat it.
fn decided(r: &DecisionRunRow) -> bool {
    r.called
        && matches!(
            r.fallback.as_deref(),
            None | Some("low_confidence") | Some("invalid_answer")
        )
}

/// PURE: the proposal a run makes, if any: a live assist answer, sure
/// enough, not `unsure`, not yet followed up.
fn proposal_of(r: &DecisionRunRow, why: &str) -> Option<DecisionProposal> {
    let value = r.answer.clone().filter(|a| a != UNSURE)?;
    let sure = r.confidence.is_some_and(|c| c >= MIN_CONFIDENCE);
    if r.mode != Mode::Assist.as_str() || r.fallback.is_some() || r.followup.is_some() || !sure {
        return None;
    }
    Some(DecisionProposal {
        feature: Feature::MissionTriage.as_str().into(),
        value,
        source: crate::store::proposal_source(&r.provider).into(),
        reason: Some(why.to_string()),
        confidence_pct: r
            .confidence
            .map(|c| (c * 100.0).round().clamp(0.0, 100.0) as u8),
        run_id: Some(r.id),
        at: Some(r.at),
        linked: None,
    })
}

/// PURE: [`proposal_of`] for a fresh outcome.
fn proposal_from(out: &DecisionOutcome, why: &str, now: i64) -> Option<DecisionProposal> {
    let a = out.proposal().filter(|a| a.value != UNSURE)?;
    Some(DecisionProposal {
        feature: Feature::MissionTriage.as_str().into(),
        value: a.value.clone(),
        source: "jev".into(),
        reason: Some(why.to_string()),
        confidence_pct: a
            .confidence
            .map(|c| (c * 100.0).round().clamp(0.0, 100.0) as u8),
        run_id: out.run_id,
        at: Some(now),
        linked: None,
    })
}

/// What to do about one question.
enum Plan {
    /// The gate refuses: nothing asked, nothing proposed.
    Skip,
    /// A decided run on this input is recent enough: its proposal, if any.
    Reuse(Option<DecisionProposal>),
    /// Ask.
    Ask,
}

fn plan(s: &Store, m: &MissionRow, subject: &str, req: &JevRequest, why: &str, now: i64) -> Plan {
    let Ok(mode) = gate_at(s, Feature::MissionTriage, m.org_id, now) else {
        return Plan::Skip;
    };
    let Ok(fp_key) = s.decision_fp_key() else {
        return Plan::Skip;
    };
    let fp = fingerprint(&fp_key, &req.redacted());
    let runs = s
        .decision_runs_for_subjects(
            Feature::MissionTriage.as_str(),
            SUBJECT_KIND,
            subject,
            Some(now - REUSE_SECS),
        )
        .unwrap_or_default();
    match runs.iter().find(|r| {
        r.subject_id == subject
            && r.input_fp.as_deref() == Some(fp.as_str())
            && r.question_version == QUESTION_VERSION
            && r.mode == mode.as_str()
            && decided(r)
    }) {
        Some(r) => Plan::Reuse(proposal_of(r, why)),
        None => Plan::Ask,
    }
}

async fn one(
    ctx: &DecideCtx,
    m: &MissionRow,
    subject: String,
    req: JevRequest,
    why: &str,
) -> Option<DecisionProposal> {
    let now = ctx.now();
    let planned = {
        let s = lock(&ctx.store).ok()?;
        plan(&s, m, &subject, &req, why, now)
    };
    match planned {
        Plan::Skip => None,
        Plan::Reuse(p) => p,
        Plan::Ask => {
            let out = decide(
                ctx,
                DecideRequest {
                    feature: Feature::MissionTriage,
                    subject_kind: SUBJECT_KIND.into(),
                    subject_id: subject,
                    org_id: m.org_id,
                    request: req,
                    // Today no rule triages a mission.
                    baseline: None,
                    question_version: QUESTION_VERSION.into(),
                    min_confidence: Some(MIN_CONFIDENCE),
                },
            )
            .await;
            proposal_from(&out, why, now)
        }
    }
}

/// Ask both questions about stuck mission `m`. Every failure is a recorded
/// fallback or nothing at all, never an error. Reads the store and records
/// decision runs; changes nothing else. Holds the store lock only for
/// reads, never across a call.
pub async fn ask(ctx: &DecideCtx, m: &MissionRow, s: &Stuck) -> Proposals {
    // The worker's summary is Claude's text: without the org's reply-text
    // consent (D48, off by default) Jev is asked without it.
    let reply_ok = !s.last_failure_is_reply
        || lock(&ctx.store).is_ok_and(|st| super::reply_text_allowed(&st, m.org_id));
    let held;
    let s = if reply_ok {
        s
    } else {
        held = Stuck {
            last_failure: None,
            last_failure_is_reply: false,
            ..s.clone()
        };
        &held
    };
    let outcome = one(
        ctx,
        m,
        format!("{}:outcome", m.id),
        outcome_request(m, s),
        &s.why,
    )
    .await;
    let next = one(ctx, m, format!("{}:next", m.id), next_request(m, s), &s.why).await;
    Proposals { outcome, next }
}
