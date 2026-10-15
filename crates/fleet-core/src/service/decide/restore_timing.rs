//! `restore_timing`: whether a session the host killed should be resumed
//! now, held back until its host is less loaded, or moved to another host
//! (`service::auto_restore`).
//!
//! * **When.** Only when the rule has a doubt to settle: a restore is due
//!   (past `recovery.grace_secs`, within `recovery.max_attempts`) on a host
//!   that is under load (memory short, CPU saturated), or after an automatic
//!   recovery of the session already failed once. A due restore on a calm
//!   host is the rule's alone and asks nothing.
//! * **Question.** A choice: `recover_now`, `defer`, and `m:<alias>` for
//!   each other active, reachable host whose memory is healthy (the only
//!   hosts a move may name).
//! * **What is sent.** Bucketed numbers only: per host (its own and each
//!   move candidate) CPU busy percent, available memory percent and free
//!   disk percent rounded down to a tenth, and its live sessions; for the
//!   session, why it was lost (a vocabulary word), minutes since its last
//!   activity (bucketed), its last turn outcome word, whether a turn was in
//!   flight when it died, and how many automatic restores already failed.
//!   No names, paths, prompts or transcript.
//! * **Baseline.** The rule: `defer` while the host is loaded, else
//!   `recover_now` on the same host.
//! * **Modes.** Off asks nothing. Shadow records the answer beside the
//!   baseline; the rule acts. Assist lets the answer steer the tick, within
//!   hard bounds the answer cannot cross ([`bounded`]): a `recover_now`
//!   never resumes onto a host under the low-memory floor, and a move
//!   names only a candidate host. This is the owner's call (2026-10-14) for
//!   an automatic loop, a deliberate exception to D35's "assist only
//!   proposes": every action it can pick is one the rule could pick too
//!   (resume, wait), so the model chooses timing, never a new kind of act.
//!   A `move` is honoured as "wait here, and say where it should go": a lost
//!   session has no live source for `move_session` to carry its worktree
//!   from, so the tick defers and records the target for Control and the
//!   person (`auto_restore_deferred` detail `move:<alias>`).
//! * **Asked once per input.** A decided run in the same loss episode on
//!   the same input fingerprint is reused, and any run in the last
//!   [`REASK_SECS`] is too, so a tick every 20 s never turns into a call
//!   every 20 s, also while the backend fails.
//! * **One-line reason.** Jev answers a closed choice with a confidence, no
//!   prose; [`reason_line`] words the answer for the timeline.

use super::{decide, fingerprint, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode};
use super::{Question, QUOTED_DATA};
use crate::ipc_error::lock;
use crate::store::{is_decision_word, DecisionRunRow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "restore_timing.v1";
/// What a run is about: the lost session (its id as text).
pub const SUBJECT_KIND: &str = "session";
/// Resume it now, on its own host.
pub const RECOVER_NOW: &str = "recover_now";
/// Wait until its host is less loaded.
pub const DEFER: &str = "defer";
/// Any run on the same input this recent is reused, decided or not.
pub const REASK_SECS: i64 = 600;
/// Below this confidence an answer is recorded, not followed.
pub const MIN_CONFIDENCE: f64 = 0.5;

/// The instruction, read literally.
pub const INSTRUCTIONS: &str = "A coding session's process was stopped by its machine (killed, \
     for example when memory ran out, or by a reboot) and fleet can resume its conversation. \
     state.host describes the machine it ran on: CPU busy percent, available memory percent, \
     free disk percent and how many sessions run there now. state.session says why it was \
     lost, minutes since its last activity, the outcome word of its last turn, whether a turn \
     was still running when it stopped, and how many automatic restores of it already failed. \
     Each option m:<host> is another machine it could move to, with the same numbers in \
     state.hosts. Choose recover_now to resume it on its own machine now, defer to wait until \
     that machine is less loaded, or a move option when another machine is clearly the better \
     place. Prefer its own machine when it can take the session.";

/// The facts one host carries into the question.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFacts {
    pub alias: String,
    /// CPU busy percent, rounded down to a tenth.
    pub cpu_busy_pct: Option<u8>,
    /// Available memory percent, rounded down to a tenth.
    pub mem_avail_pct: Option<u8>,
    /// Free home disk percent, rounded down to a tenth.
    pub disk_free_pct: Option<u8>,
    pub live_sessions: i64,
}

/// What the tick asks about one lost session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreQuestion {
    pub session_id: i64,
    pub org_id: Option<i64>,
    /// When it was lost: a run in this episode is reused.
    pub lost_at: i64,
    pub lost_reason: String,
    /// Minutes since its last activity, bucketed ([`minutes_bucket`]).
    pub idle_minutes: i64,
    pub turn_outcome: Option<String>,
    pub mid_turn: bool,
    pub failed_attempts: u32,
    /// Its own host.
    pub host: HostFacts,
    /// Hosts a move may name: active, reachable, memory healthy, not its own.
    pub move_to: Vec<HostFacts>,
    /// What the rule decided: [`RECOVER_NOW`] or [`DEFER`].
    pub baseline: &'static str,
}

/// Jev's answer, as the tick reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Advice {
    RecoverNow,
    Defer,
    Move(String),
}

/// What [`ask`] hands back: the advice and whether the tick may follow it.
#[derive(Debug, Clone, PartialEq)]
pub struct Advised {
    pub advice: Advice,
    /// `true` in assist; in shadow the answer is only recorded.
    pub follow: bool,
    pub confidence: Option<f64>,
}

/// PURE: down to a tenth of a percent scale (`37.9` → `30`).
pub fn tenth(v: f64) -> u8 {
    ((v.clamp(0.0, 100.0) / 10.0).floor() * 10.0) as u8
}

/// PURE: minutes in coarse steps (under 5, 15, 60, 240, a day, more), so a
/// question asked a tick later reads the same.
pub fn minutes_bucket(secs: i64) -> i64 {
    let m = secs.max(0) / 60;
    *[5, 15, 60, 240, 1440]
        .iter()
        .find(|&&b| m < b)
        .unwrap_or(&10_080)
}

/// PURE: a move option's word.
pub fn option_of(alias: &str) -> String {
    format!("m:{alias}")
}

/// PURE: the host a move option names.
pub fn host_of(option: &str) -> Option<&str> {
    option.strip_prefix("m:").filter(|a| !a.is_empty())
}

/// PURE: the question over `q`'s options.
pub fn question(q: &RestoreQuestion) -> Question {
    let mut criteria: BTreeMap<String, Option<Value>> = BTreeMap::new();
    criteria.insert(
        RECOVER_NOW.into(),
        Some(Value::String(
            "Resume the session on its own machine now.".into(),
        )),
    );
    criteria.insert(
        DEFER.into(),
        Some(Value::String(
            "Wait until its own machine is less loaded.".into(),
        )),
    );
    for h in &q.move_to {
        let word = option_of(&h.alias);
        if is_decision_word(&word) {
            criteria.insert(
                word,
                Some(Value::String(format!(
                    "Move the session to the machine {}.",
                    h.alias
                ))),
            );
        }
    }
    Question::Choice {
        instructions: Value::String(format!("{INSTRUCTIONS}{QUOTED_DATA}")),
        criteria,
    }
}

fn facts(h: &HostFacts) -> Value {
    json!({
        "cpu_busy_pct": h.cpu_busy_pct,
        "mem_avail_pct": h.mem_avail_pct,
        "disk_free_pct": h.disk_free_pct,
        "live_sessions": h.live_sessions,
    })
}

/// PURE: the request for `q`.
pub fn request(q: &RestoreQuestion) -> JevRequest {
    let hosts: serde_json::Map<String, Value> = q
        .move_to
        .iter()
        .map(|h| (option_of(&h.alias), facts(h)))
        .collect();
    JevRequest {
        state: json!({
            "host": facts(&q.host),
            "session": {
                "lost_reason": q.lost_reason,
                "idle_minutes": q.idle_minutes,
                "turn_outcome": q.turn_outcome,
                "mid_turn": q.mid_turn,
                "failed_restores": q.failed_attempts,
            },
            "hosts": hosts,
        }),
        question: question(q),
        order_salt: None,
    }
}

/// PURE: an option word as advice, `None` for one the question did not
/// offer.
pub fn advice_of(word: &str, q: &RestoreQuestion) -> Option<Advice> {
    match word {
        RECOVER_NOW => Some(Advice::RecoverNow),
        DEFER => Some(Advice::Defer),
        w => {
            let alias = host_of(w)?;
            q.move_to
                .iter()
                .any(|h| h.alias == alias)
                .then(|| Advice::Move(alias.to_string()))
        }
    }
}

/// PURE: the bounds an answer cannot cross. A `recover_now` under the hard
/// low-memory floor becomes `defer`; a move to a host the question did not
/// offer (no longer healthy, never offered) is dropped (`None`: the rule
/// decides).
pub fn bounded(advice: Advice, floor: bool, q: &RestoreQuestion) -> Option<Advice> {
    match advice {
        Advice::RecoverNow if floor => Some(Advice::Defer),
        Advice::Move(h) if !q.move_to.iter().any(|m| m.alias == h) => None,
        a => Some(a),
    }
}

/// PURE: one line for the timeline and the logs.
pub fn reason_line(a: &Advice, confidence: Option<f64>) -> String {
    let pct = confidence
        .map(|c| format!(", {}% sure", (c.clamp(0.0, 1.0) * 100.0).round() as u8))
        .unwrap_or_default();
    match a {
        Advice::RecoverNow => format!("Jev: resume now, the host can take it{pct}"),
        Advice::Defer => format!("Jev: wait for the host's load to drop{pct}"),
        Advice::Move(h) => format!("Jev: better on {h}{pct}"),
    }
}

/// A run that got an answer the envelope checked (or found wanting).
fn decided(r: &DecisionRunRow) -> bool {
    r.called
        && matches!(
            r.fallback.as_deref(),
            None | Some("low_confidence") | Some("invalid_answer")
        )
}

enum Plan {
    Skip,
    Reuse(Option<Advised>),
    Ask(Mode, JevRequest),
}

fn plan(ctx: &DecideCtx, q: &RestoreQuestion, now: i64) -> Plan {
    let Ok(s) = lock(&ctx.store) else {
        return Plan::Skip;
    };
    let Ok(mode) = gate_at(&s, Feature::RestoreTiming, q.org_id, now) else {
        return Plan::Skip;
    };
    let Ok(fp_key) = s.decision_fp_key() else {
        return Plan::Skip;
    };
    let req = request(q);
    let fp = fingerprint(&fp_key, &req.redacted());
    let subject = q.session_id.to_string();
    let since = q.lost_at.min(now - REASK_SECS);
    let runs = s
        .decision_runs_for_subjects(
            Feature::RestoreTiming.as_str(),
            SUBJECT_KIND,
            &subject,
            Some(since),
        )
        .unwrap_or_default();
    let same = |r: &&DecisionRunRow| {
        r.subject_id == subject
            && r.input_fp.as_deref() == Some(fp.as_str())
            && r.question_version == QUESTION_VERSION
            && r.mode == mode.as_str()
    };
    if let Some(r) = runs
        .iter()
        .filter(same)
        .find(|r| (decided(r) && r.at >= q.lost_at) || r.at >= now - REASK_SECS)
    {
        let reuse = (r.fallback.is_none())
            .then(|| r.answer.as_deref().and_then(|w| advice_of(w, q)))
            .flatten()
            .map(|advice| Advised {
                advice,
                follow: mode == Mode::Assist,
                confidence: r.confidence,
            });
        return Plan::Reuse(reuse);
    }
    Plan::Ask(mode, req)
}

/// Ask (or reuse) Jev's timing for `q`. `None` when the gate refuses, the
/// answer is unusable or under [`MIN_CONFIDENCE`]: the rule decides. Never
/// errors, never holds the store lock across the call. The caller still
/// applies [`bounded`].
pub async fn ask(ctx: &DecideCtx, q: &RestoreQuestion) -> Option<Advised> {
    let now = ctx.now();
    let (mode, req) = match plan(ctx, q, now) {
        Plan::Skip => return None,
        Plan::Reuse(a) => return a,
        Plan::Ask(mode, req) => (mode, req),
    };
    let out = decide(
        ctx,
        DecideRequest {
            feature: Feature::RestoreTiming,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: q.session_id.to_string(),
            org_id: q.org_id,
            request: req,
            baseline: Some(q.baseline.to_string()),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    let a = out.usable()?;
    Some(Advised {
        advice: advice_of(&a.value, q)?,
        follow: mode == Mode::Assist,
        confidence: a.confidence,
    })
}
