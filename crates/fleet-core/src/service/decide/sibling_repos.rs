//! N3 `sibling_repos`: which other repository a ticket start also needs
//! (redesign step 3.12). A ticket's start in the New session dialog offers
//! "Also start in <repo>" for the projects the key ran in before (work graph
//! M9.6, D11; [`crate::service::trackers::tickets::sibling_candidates`]).
//! This adapter asks the decision model one Choice over those candidates —
//! which ONE of them the same task also needs changes in — and nothing more.
//! It mirrors K1 [`super::start_project`] closely:
//!
//! * **Shadow / assist only.** In `shadow` the question is asked off the
//!   preview's path (spawned) and only recorded, with `none` (today's
//!   nothing ticked) as the baseline. In `assist` the preview waits for the
//!   one bounded call (`decide.jev.timeout_ms`) and a usable answer becomes
//!   `suggested_sibling`: the dialog pre-ticks it, and a person still
//!   presses Start. Nothing here starts, links or moves anything.
//! * **What is sent.** The task's key and title, the first
//!   [`DESCRIPTION_CHARS`] characters of its cached description, the chosen
//!   repository's `owner/repo` and each candidate's — through the envelope's
//!   redaction. Nothing from the repositories themselves.
//! * **What is recorded.** A run about subject `work_start_siblings`
//!   `item:<id>` (or `key:<HMAC of the key>` for a key no tracker knows);
//!   the options are project ids (`p<id>`), `none` and `unsure`.
//! * **Asked once per input.** A decided run on the same subject, input
//!   fingerprint, question version, mode and pinned model in the last
//!   [`REASK_DAYS`] days is reused, not asked again.
//! * **Follow-up.** When a PERSON's start of the same subject lands
//!   ([`record_start`]), the latest assist proposal nobody decided is marked
//!   `confirmed` (the sibling it proposed was started too) or `corrected`
//!   (to the sibling they did start, or `none`). A shadow answer nobody saw
//!   is never marked (D34, D37), and an agent's start marks nothing.

use super::start_project::{decided, option_of, pct, project_of, subject_id, Candidate};
use super::{
    decide, fingerprint, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode, Question,
};
use crate::ipc_error::{lock, IpcError};
use crate::service::settings;
use crate::store::{DecisionRunRow, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use super::start_project::{DESCRIPTION_CHARS, MIN_CONFIDENCE, REASK_DAYS, UNSURE};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "sibling_repos.v1";
/// What a run is about: one task's start, and the siblings it offers.
pub const SUBJECT_KIND: &str = "work_start_siblings";
/// The option that proposes no sibling (and the baseline: today nothing is
/// pre-ticked).
pub const NONE: &str = "none";

/// The instruction, read literally: the exact condition and what to use.
pub const INSTRUCTIONS: &str = "state.task is one task (a ticket) a person is about to start \
     working on in the git repository state.chosen (owner/repo): its key, its title and the \
     start of its description. state.candidates are other repositories this task's key was \
     worked on in before. Each option except none and unsure is one of those repositories. \
     Decide which ONE of them the same task also needs code changes in, using only the task's \
     text and the repository names. Choose none when the task's changes belong in \
     state.chosen alone. Choose unsure when the text does not show whether another repository \
     is needed.";

/// What the adapter asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiblingInput {
    pub key: String,
    pub title: String,
    pub item_id: Option<i64>,
    /// The task's org (its consent is checked); `None` when it has none.
    pub org_id: Option<i64>,
    /// The cached description (third-party text), when there is one.
    pub description: Option<String>,
    /// The repository the start is planned in.
    pub chosen: Candidate,
    /// The other projects the key ran in before, newest first.
    pub candidates: Vec<Candidate>,
}

/// The sibling the model proposes, for the dialog to pre-tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedSibling {
    pub project_id: i64,
    /// The model's confidence, in whole percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_pct: Option<u8>,
    /// The recorded run (what a follow-up marks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
}

fn label(c: &Candidate) -> String {
    format!("{}/{}", c.owner, c.repo)
}

/// PURE: the question over `candidates`.
pub fn question(candidates: &[Candidate]) -> Question {
    let mut criteria: Vec<(String, Option<Value>)> = candidates
        .iter()
        .map(|c| {
            (
                option_of(c.project_id),
                Some(Value::String(format!(
                    "The task also needs code changes in the repository {}.",
                    label(c)
                ))),
            )
        })
        .collect();
    criteria.push((
        NONE.to_string(),
        Some(Value::String(
            "The task's changes belong in the chosen repository alone.".into(),
        )),
    ));
    criteria.push((
        UNSURE.to_string(),
        Some(Value::String(
            "The task's text does not show whether another repository is needed.".into(),
        )),
    ));
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: criteria.into_iter().collect(),
    }
}

/// PURE: the state: the task's text and the repositories' names only.
pub fn state(input: &SiblingInput) -> Value {
    let description: String = input
        .description
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(DESCRIPTION_CHARS)
        .collect();
    json!({
        "task": {
            "key": input.key,
            "title": input.title,
            "description": description,
        },
        "chosen": label(&input.chosen),
        "candidates": input.candidates.iter().map(label).collect::<Vec<_>>(),
    })
}

/// PURE: the request for `input`.
pub fn question_for(input: &SiblingInput) -> JevRequest {
    JevRequest {
        state: state(input),
        question: question(&input.candidates),
    }
}

/// PURE: what a run proposes: an answered run naming one of `candidates`.
fn proposed(r: &DecisionRunRow, candidates: &[Candidate]) -> Option<SuggestedSibling> {
    if r.fallback.is_some() || r.followup.as_deref() == Some("rejected") {
        return None;
    }
    let pid = project_of(r.answer.as_deref()?)?;
    candidates
        .iter()
        .any(|c| c.project_id == pid)
        .then_some(SuggestedSibling {
            project_id: pid,
            confidence_pct: pct(r.confidence),
            run_id: Some(r.id),
        })
}

/// What [`ask`] did before any call: the mode, and the run to reuse.
enum Plan {
    Skip,
    Reuse(Option<SuggestedSibling>),
    Ask {
        subject: String,
        request: JevRequest,
    },
}

fn plan(s: &Store, input: &SiblingInput, now: i64) -> Result<Plan, IpcError> {
    if input.candidates.is_empty() {
        return Ok(Plan::Skip);
    }
    let Ok(mode) = gate_at(s, Feature::SiblingRepos, input.org_id, now) else {
        return Ok(Plan::Skip);
    };
    let fp_key = s.decision_fp_key()?;
    let subject = subject_id(&fp_key, input.item_id, &input.key);
    let request = question_for(input);
    let fp = fingerprint(&fp_key, &request.redacted());
    let model = settings::get_string(s, settings::DECIDE_JEV_MODEL);
    let since = now - REASK_DAYS * 86_400;
    let runs = s.decision_runs_for_subjects(
        Feature::SiblingRepos.as_str(),
        SUBJECT_KIND,
        &subject,
        Some(since),
    )?;
    let recent = runs.iter().find(|r| {
        r.subject_id == subject
            && r.input_fp.as_deref() == Some(fp.as_str())
            && r.question_version == QUESTION_VERSION
            && r.mode == mode.as_str()
            && decided(r)
            && (model == "jev-latest" || r.model_version.as_deref() == Some(model.as_str()))
    });
    if let Some(r) = recent {
        let reuse = (mode == Mode::Assist)
            .then(|| proposed(r, &input.candidates))
            .flatten();
        return Ok(Plan::Reuse(reuse));
    }
    Ok(Plan::Ask { subject, request })
}

/// Ask (or reuse) the decision model's answer for `input`. `Some` only in
/// `assist`, with a usable answer naming a candidate. Every failure is a
/// recorded fallback or nothing at all, never an error: a start never
/// depends on this. Holds the store lock only for reads, never across the
/// call.
pub async fn ask(ctx: &DecideCtx, input: &SiblingInput) -> Option<SuggestedSibling> {
    let now = ctx.now();
    let planned = {
        let s = lock(&ctx.store).ok()?;
        match plan(&s, input, now) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("[decide] sibling_repos not asked: {}", e.message);
                return None;
            }
        }
    };
    let (subject, request) = match planned {
        Plan::Skip => return None,
        Plan::Reuse(s) => return s,
        Plan::Ask { subject, request } => (subject, request),
    };
    let out = decide(
        ctx,
        DecideRequest {
            feature: Feature::SiblingRepos,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: subject.clone(),
            org_id: input.org_id,
            request,
            baseline: Some(NONE.into()),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    let answer = out.proposal()?;
    let pid = project_of(&answer.value)?;
    if !input.candidates.iter().any(|c| c.project_id == pid) {
        return None;
    }
    // A newer proposal takes the place of an older one nobody decided.
    if let (Some(id), Ok(s)) = (out.run_id, lock(&ctx.store)) {
        if let Err(e) = s.supersede_decision_runs(
            Feature::SiblingRepos.as_str(),
            SUBJECT_KIND,
            &subject,
            Mode::Assist.as_str(),
            id,
            now,
        ) {
            tracing::warn!("[decide] ignored follow-up not recorded: {}", e.message);
        }
    }
    Some(SuggestedSibling {
        project_id: pid,
        confidence_pct: pct(answer.confidence),
        run_id: out.run_id,
    })
}

/// The mode a preview would ask `input` in, under one short lock: `None`
/// when there is nothing to ask or the gate refuses, so the preview pays
/// nothing.
pub fn mode_for(ctx: &DecideCtx, input: &SiblingInput) -> Option<Mode> {
    if input.candidates.is_empty() {
        return None;
    }
    let s = lock(&ctx.store).ok()?;
    gate_at(&s, Feature::SiblingRepos, input.org_id, ctx.now()).ok()
}

/// After a PERSON's start of the task (`item_id` / `key`) in `primary`, with
/// `started` every project they started it in (the primary and any ticked
/// siblings): mark the latest assist proposal nobody decided `confirmed`
/// (the sibling it proposed is among `started`) or `corrected` (to the
/// first sibling they did start, else `none`). Returns whether a run was
/// marked. Never marks a shadow answer, and never a run that already has a
/// follow-up.
pub fn record_start(
    s: &Store,
    item_id: Option<i64>,
    key: &str,
    primary: i64,
    started: &[i64],
    now: i64,
) -> Result<bool, IpcError> {
    let fp_key = s.decision_fp_key()?;
    let subject = subject_id(&fp_key, item_id, key);
    let runs =
        s.decision_runs_for_subjects(Feature::SiblingRepos.as_str(), SUBJECT_KIND, &subject, None)?;
    let Some(r) = runs.iter().find(|r| {
        r.subject_id == subject
            && r.mode == Mode::Assist.as_str()
            && r.fallback.is_none()
            && r.answer.as_deref().and_then(project_of).is_some()
    }) else {
        return Ok(false);
    };
    if r.followup.is_some() {
        return Ok(false);
    }
    let proposed = r.answer.as_deref().and_then(project_of);
    if proposed.is_some_and(|p| started.contains(&p)) {
        return s.set_decision_followup(r.id, "confirmed", None, now);
    }
    let theirs = started
        .iter()
        .find(|&&p| p != primary)
        .map(|&p| option_of(p))
        .unwrap_or_else(|| NONE.to_string());
    s.set_decision_followup(r.id, "corrected", Some(&theirs), now)
}
