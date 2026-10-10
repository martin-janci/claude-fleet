//! K1 `start_project`: which repository a task's first start belongs in (the
//! test map's card K1, accepted by the owner on 2026-10-07). When no project
//! has worked on a task's key prefix yet, a start cannot be planned:
//! [`crate::service::trackers::tickets::preview_start`] answers
//! `missing: "project"` with the most recently used projects, and a person
//! picks one in the start popover. This adapter asks the decision model one
//! Choice over those same candidates — and nothing more:
//!
//! * **Shadow / assist only.** In `shadow` the question is asked off the
//!   preview's path (spawned) and only recorded, with the first candidate
//!   (today's first row) as the baseline. In `assist` the preview waits for
//!   the one bounded call (`decide.jev.timeout_ms`) and a usable answer
//!   becomes `suggested_project`: the popover pre-selects it, and a person
//!   still presses Start. Nothing here starts, links or moves anything.
//! * **What is sent.** The task's key and title, the first
//!   [`DESCRIPTION_CHARS`] characters of its cached description, and each
//!   candidate's `owner/repo` — through the envelope's redaction. Nothing
//!   from the repositories.
//! * **What is recorded.** A run about subject `work_start` `item:<id>` (or
//!   `key:<HMAC of the key>` for a key no tracker knows); the options are
//!   project ids (`p<id>`) and `unsure`.
//! * **Asked once per input.** A decided run on the same subject, input
//!   fingerprint, question version, mode and pinned model in the last
//!   [`REASK_DAYS`] days is reused, not asked again: re-opening the popover
//!   costs nothing.
//! * **Follow-up.** When a PERSON's start of the same subject lands in a
//!   project ([`record_start`]), the latest assist proposal nobody decided
//!   is marked `confirmed` (the same project) or `corrected` (to theirs). A
//!   shadow answer nobody saw is never marked (D34, D37), and an agent's
//!   start marks nothing.

use super::{
    decide, fingerprint, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode, Question,
};
use crate::ipc_error::{lock, IpcError};
use crate::service::settings;
use crate::store::{DecisionRunRow, Secret, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "start_project.v1";
/// What a run is about: one task's start.
pub const SUBJECT_KIND: &str = "work_start";
/// Below this confidence an answer is recorded, not suggested.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// The option that suggests nothing.
pub const UNSURE: &str = "unsure";
/// Characters of the cached description sent, at most.
pub const DESCRIPTION_CHARS: usize = 1000;
/// A run decided this recently, on the same input, is reused.
pub const REASK_DAYS: i64 = 14;

/// The instruction, read literally: the exact condition and what to use.
pub const INSTRUCTIONS: &str = "state.task is one task (a ticket) a person is about to start \
     working on: its key, its title and the start of its description. Each option except unsure \
     is one git repository, named owner/repo, that the work could be done in. Decide which \
     repository the task's code changes belong in, using only the task's text and the \
     repository names. Choose unsure when the text does not name or clearly point to one of the \
     repositories.";

/// One candidate repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub project_id: i64,
    pub owner: String,
    pub repo: String,
}

/// Review r15: the candidates the model may read about a subject of
/// `org_id`: a project in that org, or one whose own org consented to
/// `feature` (`decide.jev.unassigned` for a project in no org). A project
/// whose org cannot be read stays home. The person is still offered all.
pub fn fenced(
    s: &crate::store::Store,
    feature: super::Feature,
    org_id: Option<i64>,
    candidates: &[Candidate],
) -> Vec<Candidate> {
    candidates
        .iter()
        .filter(|c| {
            let org = s.get_project(c.project_id).ok().flatten().and_then(|p| {
                super::host_placement::project_org(s, &p.owner, &p.repo, &p.base_path).ok()
            });
            match org {
                Some(org) => org == org_id || super::consents(s, feature, org),
                None => false,
            }
        })
        .cloned()
        .collect()
}

/// What the adapter asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartInput {
    pub key: String,
    pub title: String,
    pub item_id: Option<i64>,
    /// The task's org (its consent is checked); `None` when it has none.
    pub org_id: Option<i64>,
    /// The cached description (third-party text), when there is one.
    pub description: Option<String>,
    /// The preview's candidates, in its order (most recently used first).
    pub candidates: Vec<Candidate>,
}

/// The project the model proposes, for the popover to pre-select.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedProject {
    pub project_id: i64,
    /// The model's confidence, in whole percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_pct: Option<u8>,
    /// The recorded run (what a follow-up marks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
}

impl SuggestedProject {
    /// This pre-selection as a [`DecisionProposal`] (step 2.8): what Jev
    /// proposes for the task's start, `p<id>` (the question's option).
    pub fn proposal(&self) -> crate::store::DecisionProposal {
        crate::store::DecisionProposal {
            feature: Feature::StartProject.as_str().to_string(),
            value: option_of(self.project_id),
            source: "jev".to_string(),
            reason: None,
            confidence_pct: self.confidence_pct,
            run_id: self.run_id,
            at: None,
            linked: None,
        }
    }
}

/// PURE: a confidence in whole percent.
pub(super) fn pct(c: Option<f64>) -> Option<u8> {
    c.map(|c| (c.clamp(0.0, 1.0) * 100.0).round() as u8)
}

/// PURE: a project's option word.
pub fn option_of(project_id: i64) -> String {
    format!("p{project_id}")
}

/// PURE: the project an option names; `None` for `unsure` or anything else.
pub fn project_of(option: &str) -> Option<i64> {
    option.strip_prefix('p')?.parse().ok()
}

/// PURE: the question over `candidates`.
pub fn question(candidates: &[Candidate]) -> Question {
    let mut criteria: Vec<(String, Option<Value>)> = candidates
        .iter()
        .map(|c| {
            (
                option_of(c.project_id),
                Some(Value::String(format!(
                    "The task's code changes belong in the repository {}/{}.",
                    c.owner, c.repo
                ))),
            )
        })
        .collect();
    criteria.push((
        UNSURE.to_string(),
        Some(Value::String(
            "The task's text does not show which of these repositories it belongs in.".into(),
        )),
    ));
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: criteria.into_iter().collect(),
    }
}

/// PURE: the state: the task's text only.
pub fn state(input: &StartInput) -> Value {
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
        }
    })
}

/// PURE: the request for `input`.
pub fn question_for(input: &StartInput) -> JevRequest {
    JevRequest {
        state: state(input),
        question: question(&input.candidates),
    }
}

/// PURE: the run's subject id: the item, else 16 hex digits of an HMAC of
/// the key under the local fingerprint key (a key no tracker knows is free
/// text, never recorded as is).
pub fn subject_id(fp_key: &Secret, item_id: Option<i64>, key: &str) -> String {
    match item_id {
        Some(id) => format!("item:{id}"),
        None => {
            let mac = super::hmac_sha256_hex(
                fp_key.expose().as_bytes(),
                format!("start_project.key\n{key}").as_bytes(),
            );
            format!("key:{}", &mac[..16])
        }
    }
}

/// A run that sent a request and got an answer the envelope checked (or
/// found wanting): asking again on the same input would repeat it.
pub(super) fn decided(r: &DecisionRunRow) -> bool {
    r.called
        && matches!(
            r.fallback.as_deref(),
            None | Some("low_confidence") | Some("invalid_answer")
        )
}

/// PURE: what a run proposes: an answered run naming one of `candidates`.
fn proposed(r: &DecisionRunRow, candidates: &[Candidate]) -> Option<SuggestedProject> {
    if r.fallback.is_some() || r.followup.as_deref() == Some("rejected") {
        return None;
    }
    let pid = project_of(r.answer.as_deref()?)?;
    candidates
        .iter()
        .any(|c| c.project_id == pid)
        .then_some(SuggestedProject {
            project_id: pid,
            confidence_pct: pct(r.confidence),
            run_id: Some(r.id),
        })
}

/// What [`ask`] did before any call: the mode, and the run to reuse.
enum Plan {
    Skip,
    Reuse(Option<SuggestedProject>),
    Ask {
        subject: String,
        request: JevRequest,
    },
}

fn plan(s: &Store, input: &StartInput, now: i64) -> Result<Plan, IpcError> {
    if input.candidates.is_empty() {
        return Ok(Plan::Skip);
    }
    let Ok(mode) = gate_at(s, Feature::StartProject, input.org_id, now) else {
        return Ok(Plan::Skip);
    };
    let fp_key = s.decision_fp_key()?;
    let subject = subject_id(&fp_key, input.item_id, &input.key);
    let request = question_for(input);
    let fp = fingerprint(&fp_key, &request.redacted());
    let model = settings::get_string(s, settings::DECIDE_JEV_MODEL);
    let since = now - REASK_DAYS * 86_400;
    let runs = s.decision_runs_for_subjects(
        Feature::StartProject.as_str(),
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
pub async fn ask(ctx: &DecideCtx, input: &StartInput) -> Option<SuggestedProject> {
    let now = ctx.now();
    let planned = {
        let s = lock(&ctx.store).ok()?;
        match plan(&s, input, now) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("[decide] start_project not asked: {}", e.message);
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
            feature: Feature::StartProject,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: subject.clone(),
            org_id: input.org_id,
            request,
            baseline: input.candidates.first().map(|c| option_of(c.project_id)),
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
            Feature::StartProject.as_str(),
            SUBJECT_KIND,
            &subject,
            Mode::Assist.as_str(),
            id,
            now,
        ) {
            tracing::warn!("[decide] ignored follow-up not recorded: {}", e.message);
        }
    }
    Some(SuggestedProject {
        project_id: pid,
        confidence_pct: pct(answer.confidence),
        run_id: out.run_id,
    })
}

/// The mode a preview would ask `input` in, under one short lock: `None`
/// when the gate refuses (nothing to ask), so the preview pays nothing.
pub fn mode_for(ctx: &DecideCtx, input: &StartInput) -> Option<Mode> {
    if input.candidates.is_empty() {
        return None;
    }
    let s = lock(&ctx.store).ok()?;
    gate_at(&s, Feature::StartProject, input.org_id, ctx.now()).ok()
}

/// After a PERSON's start of the task (`item_id` / `key`) landed in
/// `project_id`: mark the latest assist proposal nobody decided `confirmed`
/// (the same project) or `corrected` (to theirs). Returns whether a run was
/// marked. Never marks a shadow answer, and never a run that already has a
/// follow-up.
pub fn record_start(
    s: &Store,
    item_id: Option<i64>,
    key: &str,
    project_id: i64,
    now: i64,
) -> Result<bool, IpcError> {
    let fp_key = s.decision_fp_key()?;
    let subject = subject_id(&fp_key, item_id, key);
    let runs =
        s.decision_runs_for_subjects(Feature::StartProject.as_str(), SUBJECT_KIND, &subject, None)?;
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
    let theirs = option_of(project_id);
    if r.answer.as_deref() == Some(theirs.as_str()) {
        s.set_decision_followup(r.id, "confirmed", None, now)
    } else {
        s.set_decision_followup(r.id, "corrected", Some(&theirs), now)
    }
}
