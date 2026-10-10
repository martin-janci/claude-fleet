//! J5 `quick_answer` (redesign step 10.9): which option of an agent's
//! question, or of a chat form's choice, a person is likely to pick. The UI
//! moves that option first and says "Proposed by Jev"; a person still
//! answers. This adapter asks the decision model one Choice over the
//! options it may propose — and nothing more:
//!
//! * **Never on a push, a permission or a risky option** (the plan's "Where
//!   AI never decides", `src/lib/ai_proposal.ts`'s `NEVER_DECIDES`). A
//!   permission dialog is never asked about at all; an option whose words
//!   name a push, a force, a delete or another step that is hard to undo
//!   ([`risky`]) is left out of the question, so no answer can name it. The
//!   UI checks the same words again before it moves anything.
//! * **Shadow / assist only.** Asked off every path a person waits on
//!   (spawned from the reconcile tick, or after `ask { form }` opened its
//!   form), so a question shows at once and a proposal appears when ready
//!   or not at all. In `shadow` the run is only recorded, with the option
//!   under the cursor as the baseline; in `assist` the run is the proposal:
//!   the session row carries it (`proposals`, step 2.8), a form's view
//!   carries it as `proposal`.
//! * **What is sent.** The question's text and the options' labels, through
//!   the envelope's redaction. Nothing from the pane or the repository.
//! * **What is recorded.** A run about subject `session` `<id>` (an agent's
//!   question) or `form` `<form_id>` (a form's first choice); the options are
//!   `o<n>` (the option's number as the card shows it, from 1) and `unsure`.
//! * **Asked once per question.** The trigger remembers each session's
//!   question by a digest and asks again only when it changes; a decided run
//!   on the same subject, input fingerprint, version, mode and model is
//!   reused, not asked again. A question that changed or went away
//!   withdraws its proposal ([`withdraw`]), so an answer never outlives the
//!   question it is about.

use super::{
    decide, fingerprint, gate_at, DecideCtx, DecideRequest, Feature, FeatureMode, JevRequest, Mode,
    Question,
};
use crate::ipc_error::{lock, IpcError};
use crate::service::pane_intel::PendingInput;
use crate::service::settings;
use crate::store::{DecisionRunRow, Store};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "quick_answer.v1";
/// An agent's question, on its session (the subject 2.8's row proposals read).
pub const SUBJECT_SESSION: &str = "session";
/// A chat form's first choice.
pub const SUBJECT_FORM: &str = "form";
/// Below this confidence an answer is recorded, not proposed.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// The option that proposes nothing.
pub const UNSURE: &str = "unsure";
/// The most options a question may have and still be asked: what 1–9 answer.
pub const MAX_OPTIONS: usize = 9;
/// Characters of the question sent, at most.
pub const QUESTION_CHARS: usize = 600;
/// A run decided this recently, on the same input, is reused.
pub const REASK_DAYS: i64 = 7;

/// The words that make an option one AI never proposes: a push, a
/// permission, or a step that is hard to undo. Matched as whole words,
/// ignoring case. `src/lib/quick_answer.ts` checks the same list (its test
/// reads this one).
pub const RISKY_WORDS: &[&str] = &[
    "push",
    "force",
    "delete",
    "remove",
    "drop",
    "destroy",
    "wipe",
    "reset",
    "overwrite",
    "rm",
    "kill",
    "deploy",
    "publish",
    "release",
    "merge",
    "rebase",
    "revert",
    "truncate",
    "purge",
    "production",
    "prod",
    "allow",
    "always",
    "bypass",
    "permission",
    "permissions",
    "sudo",
    "approve",
];

static RISKY: LazyLock<Regex> = LazyLock::new(|| {
    let words = RISKY_WORDS.join("|");
    Regex::new(&format!(r"(?i)\b(?:{words})\b|don['’]?t ask again")).expect("risky words")
});

/// PURE: an option AI never proposes (see [`RISKY_WORDS`]).
pub fn risky(label: &str) -> bool {
    RISKY.is_match(label)
}

/// PURE: a question that names a risky action ("Push the 3 commits to
/// origin/main now?"), whose options AI never orders: its "Yes, go ahead"
/// is a push even though its own words name none. The same words as
/// [`risky`]; `src/lib/quick_answer.ts`'s `riskyQuestion` checks them too.
pub fn risky_question(question: &str) -> bool {
    RISKY.is_match(question)
}

/// PURE: the free-text entry Claude Code adds to a question ("Type
/// something."): not a choice anyone can propose.
pub fn free_text(label: &str) -> bool {
    let l = label.trim().trim_end_matches('.').to_ascii_lowercase();
    l == "type something" || l == "chat about this"
}

/// PURE: an option's word.
pub fn option_of(n: u8) -> String {
    format!("o{n}")
}

/// PURE: the number an option word names; `None` for `unsure` or anything else.
pub fn n_of(option: &str) -> Option<u8> {
    option.strip_prefix('o')?.parse().ok()
}

/// One choice the model may propose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// Its number as shown, from 1.
    pub n: u8,
    pub label: String,
}

/// What the adapter asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickInput {
    /// [`SUBJECT_SESSION`] or [`SUBJECT_FORM`].
    pub subject_kind: &'static str,
    pub subject_id: String,
    /// The subject's org (its consent is checked); `None` when it has none.
    pub org_id: Option<i64>,
    pub question: String,
    /// Only the choices it may propose: never a risky or free-text one.
    pub choices: Vec<Choice>,
    /// The option the rule would take today (the cursor's), for the shadow
    /// comparison.
    pub baseline: Option<u8>,
}

/// PURE: the choices of `labels` (numbered from 1) AI may propose, or none
/// when the question may not be asked: fewer than two safe choices, or more
/// than [`MAX_OPTIONS`] options in all.
pub fn safe_choices<'a>(labels: impl IntoIterator<Item = &'a str>) -> Vec<Choice> {
    let all: Vec<&str> = labels.into_iter().collect();
    if all.len() > MAX_OPTIONS {
        return Vec::new();
    }
    let safe: Vec<Choice> = all
        .iter()
        .enumerate()
        .filter(|(_, l)| !risky(l) && !free_text(l) && !l.trim().is_empty())
        .map(|(i, l)| Choice {
            n: (i + 1) as u8,
            label: (*l).to_string(),
        })
        .collect();
    if safe.len() < 2 {
        Vec::new()
    } else {
        safe
    }
}

/// PURE: the input for an agent's question on session `session_id`, or
/// `None` when it may not be asked: a permission dialog, a multi-select
/// question (a digit only ticks a box there), a question that names a risky
/// action ([`risky_question`]), or too few safe choices.
pub fn session_input(
    session_id: i64,
    org_id: Option<i64>,
    pending: &PendingInput,
) -> Option<QuickInput> {
    if pending.kind != "input" || pending.multi {
        return None;
    }
    if pending.question.as_deref().is_some_and(risky_question) {
        return None;
    }
    let mut labels: Vec<(u8, &str)> = pending
        .options
        .iter()
        .map(|o| (o.n, o.label.as_str()))
        .collect();
    labels.sort_by_key(|(n, _)| *n);
    // The card numbers the options as the pane does; a gap would make "o3"
    // name another option than the third shown.
    if labels
        .iter()
        .enumerate()
        .any(|(i, (n, _))| *n as usize != i + 1)
    {
        return None;
    }
    let choices = safe_choices(labels.iter().map(|(_, l)| *l));
    if choices.is_empty() {
        return None;
    }
    let baseline = pending
        .options
        .iter()
        .find(|o| o.selected)
        .map(|o| o.n)
        .filter(|n| choices.iter().any(|c| c.n == *n));
    Some(QuickInput {
        subject_kind: SUBJECT_SESSION,
        subject_id: session_id.to_string(),
        org_id,
        question: pending.question.clone().unwrap_or_default(),
        choices,
        baseline,
    })
}

/// The form field a form's proposal is about: the first step's only
/// numbered choice (a select of 1–9 options), as the form card draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormChoice {
    pub field: String,
    /// The options' values, in the spec's order.
    pub values: Vec<String>,
    pub input: QuickInput,
}

/// PURE: the choice of form `form_id` the model may be asked about, or
/// `None`: no first step, not exactly one select there, a step with a
/// condition, or too few safe options.
pub fn form_choice(
    form_id: &str,
    org_id: Option<i64>,
    title: &str,
    why: Option<&str>,
    spec: &Value,
) -> Option<FormChoice> {
    let step = spec.get("steps")?.as_array()?.first()?;
    if step.get("when").is_some() {
        return None;
    }
    let choices: Vec<&Value> = step
        .get("fields")?
        .as_array()?
        .iter()
        .filter(|f| {
            matches!(
                f.get("type").and_then(Value::as_str),
                Some("select") | Some("multiselect")
            ) && f
                .get("options")
                .and_then(Value::as_array)
                .is_some_and(|o| !o.is_empty() && o.len() <= MAX_OPTIONS)
        })
        .collect();
    let [f] = choices.as_slice() else {
        return None;
    };
    if f.get("type").and_then(Value::as_str) != Some("select") || f.get("when").is_some() {
        return None;
    }
    let field = f.get("name")?.as_str()?.to_string();
    let label = f.get("label").and_then(Value::as_str).unwrap_or(&field);
    let opts: Vec<(String, String)> = f
        .get("options")?
        .as_array()?
        .iter()
        .filter_map(|o| {
            let a = o.as_array()?;
            Some((
                a.first()?.as_str()?.to_string(),
                a.get(1)?.as_str()?.to_string(),
            ))
        })
        .collect();
    let mut safe = safe_choices(opts.iter().map(|(_, l)| l.as_str()));
    // A value that is itself risky is left out as well as a label.
    safe.retain(|c| !risky(&opts[c.n as usize - 1].0));
    if safe.len() < 2 {
        return None;
    }
    let default = f.get("value").and_then(Value::as_str);
    let baseline = default
        .and_then(|d| opts.iter().position(|(v, _)| v == d))
        .map(|i| (i + 1) as u8)
        .filter(|n| safe.iter().any(|c| c.n == *n));
    let question = [title, why.unwrap_or(""), label]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    Some(FormChoice {
        field,
        values: opts.into_iter().map(|(v, _)| v).collect(),
        input: QuickInput {
            subject_kind: SUBJECT_FORM,
            subject_id: form_id.to_string(),
            org_id,
            question,
            choices: safe,
            baseline,
        },
    })
}

/// The instruction, read literally: the exact condition and what to use.
pub const INSTRUCTIONS: &str = "state.question is a question an AI coding agent asked the \
     person it works for, or a form it asked them to fill in. Each option except unsure is one \
     of the answers offered, in the agent's words. Decide which answer the person is most \
     likely to choose, using only the question's text and the options' words. Choose unsure \
     when the text does not make one answer clearly the likely one.";

/// PURE: the question over `choices`.
pub fn question(choices: &[Choice]) -> Question {
    let mut criteria: Vec<(String, Option<Value>)> = choices
        .iter()
        .map(|c| {
            (
                option_of(c.n),
                Some(Value::String(format!("The person answers: {}", c.label))),
            )
        })
        .collect();
    criteria.push((
        UNSURE.to_string(),
        Some(Value::String(
            "The text does not make one of these answers clearly the likely one.".into(),
        )),
    ));
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: criteria.into_iter().collect(),
    }
}

/// PURE: the request for `input`.
pub fn question_for(input: &QuickInput) -> JevRequest {
    let text: String = input.question.chars().take(QUESTION_CHARS).collect();
    JevRequest {
        state: json!({ "question": text }),
        question: question(&input.choices),
    }
}

/// The option the model proposes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickAnswer {
    /// `o<n>`: the option's number as shown, from 1.
    pub value: String,
    /// The model's confidence, in whole percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_pct: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
}

fn pct(c: Option<f64>) -> Option<u8> {
    c.map(|c| (c.clamp(0.0, 1.0) * 100.0).round() as u8)
}

/// A run that sent a request and got an answer the envelope checked.
fn decided(r: &DecisionRunRow) -> bool {
    r.called
        && matches!(
            r.fallback.as_deref(),
            None | Some("low_confidence") | Some("invalid_answer")
        )
}

/// PURE: what a run proposes: a live answer naming one of `choices`.
fn proposed(r: &DecisionRunRow, choices: &[Choice]) -> Option<QuickAnswer> {
    if r.fallback.is_some() || r.followup.is_some() {
        return None;
    }
    let answer = r.answer.as_deref()?;
    let n = n_of(answer)?;
    choices.iter().any(|c| c.n == n).then(|| QuickAnswer {
        value: answer.to_string(),
        confidence_pct: pct(r.confidence),
        run_id: Some(r.id),
    })
}

enum Plan {
    Skip,
    Reuse(Option<QuickAnswer>),
    Ask(JevRequest),
}

fn plan(s: &Store, input: &QuickInput, now: i64) -> Result<Plan, IpcError> {
    if input.choices.len() < 2 {
        return Ok(Plan::Skip);
    }
    let Ok(mode) = gate_at(s, Feature::QuickAnswer, input.org_id, now) else {
        return Ok(Plan::Skip);
    };
    let fp_key = s.decision_fp_key()?;
    let request = question_for(input);
    let fp = fingerprint(&fp_key, &request.redacted());
    let model = settings::get_string(s, settings::DECIDE_JEV_MODEL);
    let since = now - REASK_DAYS * 86_400;
    let runs = s.decision_runs_for_subjects(
        Feature::QuickAnswer.as_str(),
        input.subject_kind,
        &input.subject_id,
        Some(since),
    )?;
    let recent = runs.iter().find(|r| {
        r.subject_id == input.subject_id
            && r.input_fp.as_deref() == Some(fp.as_str())
            && r.question_version == QUESTION_VERSION
            && r.mode == mode.as_str()
            && decided(r)
            && (model == "jev-latest" || r.model_version.as_deref() == Some(model.as_str()))
    });
    if let Some(r) = recent {
        // Only the latest run is the subject's proposal (2.8): a reused
        // older one would stay hidden behind a newer one, and a withdrawn
        // one (the question went away and came back) shows nothing.
        let latest = runs.iter().find(|x| x.subject_id == input.subject_id);
        if latest.map(|x| x.id) == Some(r.id) && r.followup.is_none() {
            let reuse = (mode == Mode::Assist)
                .then(|| proposed(r, &input.choices))
                .flatten();
            return Ok(Plan::Reuse(reuse));
        }
    }
    Ok(Plan::Ask(request))
}

/// Ask (or reuse) the decision model's answer for `input`. `Some` only in
/// `assist`, with a usable answer naming a safe choice. Every failure is a
/// recorded fallback or nothing at all, never an error. Holds the store lock
/// only for reads, never across the call.
pub async fn ask(ctx: &DecideCtx, input: &QuickInput) -> Option<QuickAnswer> {
    let now = ctx.now();
    let planned = {
        let s = lock(&ctx.store).ok()?;
        match plan(&s, input, now) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("[decide] quick_answer not asked: {}", e.message);
                return None;
            }
        }
    };
    let request = match planned {
        Plan::Skip => return None,
        Plan::Reuse(a) => return a,
        Plan::Ask(r) => r,
    };
    let out = decide(
        ctx,
        DecideRequest {
            feature: Feature::QuickAnswer,
            subject_kind: input.subject_kind.into(),
            subject_id: input.subject_id.clone(),
            org_id: input.org_id,
            request,
            baseline: input.baseline.map(option_of),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    let answer = out.proposal()?;
    let n = n_of(&answer.value)?;
    if !input.choices.iter().any(|c| c.n == n) {
        return None;
    }
    if let (Some(id), Ok(s)) = (out.run_id, lock(&ctx.store)) {
        if let Err(e) = s.supersede_decision_runs(
            Feature::QuickAnswer.as_str(),
            input.subject_kind,
            &input.subject_id,
            Mode::Assist.as_str(),
            id,
            now,
        ) {
            tracing::warn!("[decide] ignored follow-up not recorded: {}", e.message);
        }
    }
    Some(QuickAnswer {
        value: answer.value.clone(),
        confidence_pct: pct(answer.confidence),
        run_id: out.run_id,
    })
}

/// The question about `subject` changed or went away: its proposal nobody
/// decided is marked `ignored`, so the row stops carrying it. Returns the
/// runs marked.
pub fn withdraw(s: &Store, subject_kind: &str, subject_id: &str, now: i64) -> usize {
    s.supersede_decision_runs(
        Feature::QuickAnswer.as_str(),
        subject_kind,
        subject_id,
        Mode::Assist.as_str(),
        i64::MAX,
        now,
    )
    .unwrap_or_else(|e| {
        tracing::warn!("[decide] quick_answer withdraw not recorded: {}", e.message);
        0
    })
}

/// The live proposal for form `form_id`'s choice `fc`, as the form's view
/// carries it: the latest assist run, decided by nobody, naming a safe
/// choice. `None` otherwise.
pub fn form_proposal(s: &Store, fc: &FormChoice) -> Option<FormProposal> {
    let runs = s
        .decision_runs_for_subjects(
            Feature::QuickAnswer.as_str(),
            SUBJECT_FORM,
            &fc.input.subject_id,
            None,
        )
        .ok()?;
    let r = runs.iter().find(|r| r.subject_id == fc.input.subject_id)?;
    if r.mode != Mode::Assist.as_str() || r.confidence.is_some_and(|c| c < MIN_CONFIDENCE) {
        return None;
    }
    let a = proposed(r, &fc.input.choices)?;
    let n = n_of(&a.value)? as usize;
    Some(FormProposal {
        field: fc.field.clone(),
        value: fc.values.get(n - 1)?.clone(),
        source: "jev".into(),
        confidence_pct: a.confidence_pct,
        run_id: a.run_id,
    })
}

/// A form's proposal on its view (`FormView.proposal`): the field, and the
/// option's VALUE, as the form card compares it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormProposal {
    pub field: String,
    pub value: String,
    /// `jev`: who proposed it, as `ai_proposal.ts` reads a source.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_pct: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
}

/// After `ask { form }` opened a form: ask about its choice in a task of its
/// own when the feature is on. Never on the form's path, never failing it.
pub fn spawn_for_form(
    ctx: DecideCtx,
    form_id: &str,
    org_id: Option<i64>,
    title: &str,
    why: Option<&str>,
    spec: &Value,
) -> Option<tokio::task::JoinHandle<()>> {
    let fc = form_choice(form_id, org_id, title, why, spec)?;
    {
        let s = lock(&ctx.store).ok()?;
        gate_at(&s, Feature::QuickAnswer, org_id, ctx.now()).ok()?;
    }
    Some(crate::rt::spawn(async move {
        ask(&ctx, &fc.input).await;
    }))
}

/// PURE: a digest of what a session's question asks (its kind, text and
/// labels; not the cursor or the ticks), to tell a new question from the
/// same one read again.
pub fn digest(pending: &PendingInput) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    pending.kind.hash(&mut h);
    pending.question.hash(&mut h);
    pending.multi.hash(&mut h);
    for o in &pending.options {
        o.n.hash(&mut h);
        o.label.hash(&mut h);
    }
    h.finish()
}

/// The reconcile tick's hook: after a pass, each session whose question is
/// new gets its old proposal withdrawn and, when it may be asked, a run, in
/// one task of its own — never on the tick's path, one task at a time.
pub struct QuickAnswerTrigger {
    ctx: DecideCtx,
    /// Session → the digest of the question last seen on it.
    seen: Mutex<HashMap<i64, u64>>,
    running: Arc<AtomicBool>,
}

impl QuickAnswerTrigger {
    pub fn new(ctx: DecideCtx) -> Arc<Self> {
        Arc::new(QuickAnswerTrigger {
            ctx,
            seen: Mutex::new(HashMap::new()),
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    /// After a reconcile pass. One short lock when the feature is off (and
    /// nothing remembered); otherwise withdraws what moved and spawns one
    /// task over the new questions. `None`: nothing to ask, or a task is
    /// still going (its questions stay new for the next pass).
    pub fn after_pass(self: &Arc<Self>) -> Option<tokio::task::JoinHandle<()>> {
        let now = self.ctx.now();
        let mut seen = self.seen.lock().ok()?;
        let inputs: Vec<QuickInput> = {
            let s = lock(&self.ctx.store).ok()?;
            let on = settings::get_bool(&s, settings::DECIDE_JEV_ENABLED)
                && FeatureMode::of(&s, Feature::QuickAnswer) != FeatureMode::Off;
            if !on && seen.is_empty() {
                return None;
            }
            let rows = s.list_all_sessions().ok()?;
            let mut live: HashMap<i64, u64> = HashMap::new();
            let mut fresh = Vec::new();
            for r in &rows {
                let Some(p) = r.pending_input.as_ref() else {
                    continue;
                };
                if r.lost_at.is_some() {
                    continue;
                }
                let d = digest(p);
                live.insert(r.id, d);
                if seen.get(&r.id) == Some(&d) {
                    continue;
                }
                if seen.contains_key(&r.id) {
                    withdraw(&s, SUBJECT_SESSION, &r.id.to_string(), now);
                }
                if on {
                    fresh.extend(session_input(r.id, r.org_id, p));
                }
            }
            // A question that went away takes its proposal with it.
            for id in seen.keys().filter(|id| !live.contains_key(id)) {
                withdraw(&s, SUBJECT_SESSION, &id.to_string(), now);
            }
            if !fresh.is_empty() && self.running.swap(true, Ordering::AcqRel) {
                // A task is going: remember only what was already asked, so
                // this pass's new questions are new again on the next one.
                let asked: Vec<i64> = fresh
                    .iter()
                    .filter_map(|i| i.subject_id.parse().ok())
                    .collect();
                live.retain(|id, _| !asked.contains(id));
                *seen = live;
                return None;
            }
            *seen = live;
            fresh
        };
        drop(seen);
        if inputs.is_empty() {
            return None;
        }
        let ctx = self.ctx.clone();
        let running = Arc::clone(&self.running);
        Some(crate::rt::spawn(async move {
            let _flight = crate::rt::ClearOnDrop::of_shared(running);
            for input in &inputs {
                ask(&ctx, input).await;
            }
        }))
    }
}
