//! J10 `restore_target` and N4 `adopt_target`: the project a lost and found
//! entry belongs to (redesign step 4.12). Host detail's Lost and found lists
//! two kinds of entry fleet can bring back:
//!
//! * **N4, a live pane fleet did not start** (`adopt_session`, step 4.8):
//!   Adopt asks which project it is, prefilled.
//! * **J10, a conversation whose pane is gone** (a transcript
//!   `discover_lost_sessions` found): Restore asks which project to resume
//!   it in, prefilled, and offers the ticket its branch names
//!   ([`LostTicket`], a rule in `sessions::lost_found`; Jev is not asked
//!   about tickets).
//!
//! The rule goes first: a working directory inside a fleet project is that
//! project, and nobody asks Jev. Only when no rule places it does this
//! adapter ask one Choice over the host's projects — which ONE of them the
//! conversation's work belongs to — or `unsure`. It mirrors K1
//! [`super::start_project`]:
//!
//! * **Shadow / assist only.** In `shadow` the question is asked off the
//!   caller's path and only recorded, with `unsure` (today's blank form) as
//!   the baseline. In `assist` the caller waits for the one bounded call
//!   (`decide.jev.timeout_ms`) and a usable answer prefills the form. A
//!   person still presses Adopt or Restore, and confirms.
//! * **What is sent.** The working directory, the git branch when the
//!   transcript names one, the tmux session's name for a pane, and each
//!   candidate's `owner/repo` — through the envelope's redaction. Nothing
//!   from the conversation itself.
//! * **What is recorded.** A run about subject `lost_pane` `session:<id>`
//!   or `lost_transcript` `t:<HMAC of the transcript id>`; the options are
//!   project ids (`p<id>`) and `unsure`.
//! * **Asked once per input**, as K1: a decided run on the same subject,
//!   input fingerprint, question version, mode and pinned model in the last
//!   [`REASK_DAYS`] days is reused.
//! * **Follow-up.** A PERSON's Adopt or Restore ([`record_choice`]) marks
//!   the latest assist proposal nobody decided `confirmed` (same project),
//!   `corrected` (to theirs) or `rejected` (they chose none). A shadow
//!   answer nobody saw is never marked.

use super::start_project::{decided, option_of, pct, project_of, Candidate};
use super::{
    decide, fingerprint, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode, Question,
};
use crate::ipc_error::{lock, IpcError};
use crate::service::settings;
use crate::store::{DecisionRunRow, Secret, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use super::start_project::{MIN_CONFIDENCE, REASK_DAYS, UNSURE};

/// The most candidates one question carries (the host's projects, most
/// recently used first).
pub const MAX_CANDIDATES: usize = 20;

/// The instruction, read literally: the exact condition and what to use.
pub const INSTRUCTIONS: &str = "state.place is where one Claude Code conversation ran on a \
     host: its working directory, its git branch when known, and its tmux session's name when \
     it is a live pane. Each option except unsure is one git repository fleet knows \
     (owner/repo). Decide which ONE of them this conversation's work belongs to, using only \
     the directory, the branch and the name. Choose unsure when they do not show it.";

/// Which Lost and found entry is asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LostKind {
    /// A live tmux pane fleet did not start (N4, Adopt).
    Pane,
    /// A conversation whose pane is gone (J10, Restore).
    Transcript,
}

impl LostKind {
    pub fn feature(self) -> Feature {
        match self {
            LostKind::Pane => Feature::AdoptTarget,
            LostKind::Transcript => Feature::RestoreTarget,
        }
    }

    pub fn subject_kind(self) -> &'static str {
        match self {
            LostKind::Pane => "lost_pane",
            LostKind::Transcript => "lost_transcript",
        }
    }

    /// The question's version: bump it when the question changes.
    pub fn question_version(self) -> &'static str {
        match self {
            LostKind::Pane => "adopt_target.v1",
            LostKind::Transcript => "restore_target.v1",
        }
    }
}

/// What the adapter asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostInput {
    pub kind: LostKind,
    /// The run's subject ([`pane_subject`] / [`transcript_subject`]).
    pub subject: String,
    /// The entry's org (its consent is checked); `None` when it has none.
    pub org_id: Option<i64>,
    pub cwd: String,
    pub git_branch: Option<String>,
    /// The tmux session's name, for a pane.
    pub name: Option<String>,
    pub candidates: Vec<Candidate>,
}

/// What a Lost and found form is prefilled with: the project, who proposed
/// it and why. Every field empty when nothing is proposed.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
pub struct LostTarget {
    /// The project to prefill; `None` leaves the form blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// `rule` (the working directory is inside the project) or `jev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// What the proposal went on, in words a person reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The model's confidence, in whole percent (a rule has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_pct: Option<u8>,
    /// The recorded run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
    /// Jev was asked and named no project (or too unsure of one): the form
    /// says so, and stays blank.
    #[serde(default)]
    pub unsure: bool,
    /// J10's other half, for a found conversation only: the ticket its git
    /// branch names, proposed by the rule. The form offers to link the
    /// restored session to it; nothing links until a person confirms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<LostTicket>,
}

/// The ticket a found conversation's branch names (J10, redesign 4.12).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
pub struct LostTicket {
    /// The work key: `PD-2412`.
    pub key: String,
    /// The ticket's title, when a tracker's cache holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `rule`: the branch names the key.
    pub source: String,
    /// What the proposal went on, in words a person reads.
    pub reason: String,
}

impl LostTarget {
    /// The rule's answer: the directory is inside `project_id`.
    pub fn rule(project_id: i64) -> Self {
        LostTarget {
            project_id: Some(project_id),
            source: Some("rule".into()),
            reason: Some("its directory is in this project".into()),
            ..Default::default()
        }
    }
}

/// PURE: a pane's subject.
pub fn pane_subject(session_id: i64) -> String {
    format!("session:{session_id}")
}

/// PURE: a transcript's subject: 16 hex digits of an HMAC of its id under
/// the local fingerprint key (an id is never recorded as is).
pub fn transcript_subject(fp_key: &Secret, claude_session_id: &str) -> String {
    let mac = super::hmac_sha256_hex(
        fp_key.expose().as_bytes(),
        format!("restore_target.transcript\n{claude_session_id}").as_bytes(),
    );
    format!("t:{}", &mac[..16])
}

/// PURE: the words under the form: what the proposal went on.
pub fn reason_for(input: &LostInput) -> String {
    match (&input.git_branch, &input.name) {
        (Some(b), _) => format!("directory and branch {b}"),
        (None, Some(n)) => format!("directory and the name {n}"),
        (None, None) => "its directory".to_string(),
    }
}

/// PURE: the question over `candidates`.
pub fn question(candidates: &[Candidate]) -> Question {
    let mut criteria: Vec<(String, Option<Value>)> = candidates
        .iter()
        .map(|c| {
            (
                option_of(c.project_id),
                Some(Value::String(format!(
                    "The conversation's work belongs to the repository {}/{}.",
                    c.owner, c.repo
                ))),
            )
        })
        .collect();
    criteria.push((
        UNSURE.to_string(),
        Some(Value::String(
            "The directory, branch and name do not show which repository it belongs to.".into(),
        )),
    ));
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: criteria.into_iter().collect(),
    }
}

/// PURE: the state: where the conversation ran, and the repositories' names.
pub fn state(input: &LostInput) -> Value {
    json!({
        "place": {
            "cwd": input.cwd,
            "branch": input.git_branch.as_deref().unwrap_or(""),
            "name": input.name.as_deref().unwrap_or(""),
        },
        "candidates": input
            .candidates
            .iter()
            .map(|c| format!("{}/{}", c.owner, c.repo))
            .collect::<Vec<_>>(),
    })
}

/// PURE: the request for `input`.
pub fn question_for(input: &LostInput) -> JevRequest {
    JevRequest {
        state: state(input),
        question: question(&input.candidates),
    }
}

/// PURE: what a decided run proposes for `input`: a project among the
/// candidates, or `unsure` (asked, and nothing to prefill).
fn proposed(r: &DecisionRunRow, input: &LostInput) -> LostTarget {
    let unsure = LostTarget {
        unsure: true,
        run_id: Some(r.id),
        ..Default::default()
    };
    if r.fallback.is_some() || r.followup.as_deref() == Some("rejected") {
        return unsure;
    }
    let Some(pid) = r.answer.as_deref().and_then(project_of) else {
        return unsure;
    };
    if !input.candidates.iter().any(|c| c.project_id == pid) {
        return unsure;
    }
    LostTarget {
        project_id: Some(pid),
        source: Some("jev".into()),
        reason: Some(reason_for(input)),
        confidence_pct: pct(r.confidence),
        run_id: Some(r.id),
        unsure: false,
        ticket: None,
    }
}

/// What [`ask`] did before any call: the mode, and the run to reuse.
enum Plan {
    Skip,
    Reuse(LostTarget),
    Ask(JevRequest),
}

fn plan(s: &Store, input: &LostInput, now: i64) -> Result<Plan, IpcError> {
    if input.candidates.is_empty() {
        return Ok(Plan::Skip);
    }
    let feature = input.kind.feature();
    let Ok(mode) = gate_at(s, feature, input.org_id, now) else {
        return Ok(Plan::Skip);
    };
    let fp_key = s.decision_fp_key()?;
    let request = question_for(input);
    let fp = fingerprint(&fp_key, &request.redacted());
    let model = settings::get_string(s, settings::DECIDE_JEV_MODEL);
    let since = now - REASK_DAYS * 86_400;
    let runs = s.decision_runs_for_subjects(
        feature.as_str(),
        input.kind.subject_kind(),
        &input.subject,
        Some(since),
    )?;
    let recent = runs.iter().find(|r| {
        r.subject_id == input.subject
            && r.input_fp.as_deref() == Some(fp.as_str())
            && r.question_version == input.kind.question_version()
            && r.mode == mode.as_str()
            && decided(r)
            && (model == "jev-latest" || r.model_version.as_deref() == Some(model.as_str()))
    });
    if let Some(r) = recent {
        return Ok(Plan::Reuse(if mode == Mode::Assist {
            proposed(r, input)
        } else {
            LostTarget::default()
        }));
    }
    Ok(Plan::Ask(request))
}

/// Ask (or reuse) the decision model's answer for `input`. In `assist`, a
/// usable answer naming a candidate prefills; any other answer is
/// `unsure`. In `shadow`, and on every failure, nothing (an empty target):
/// a form never depends on this. Holds the store lock only for reads,
/// never across the call.
pub async fn ask(ctx: &DecideCtx, input: &LostInput) -> LostTarget {
    let now = ctx.now();
    let feature = input.kind.feature();
    let planned = {
        let Ok(s) = lock(&ctx.store) else {
            return LostTarget::default();
        };
        match plan(&s, input, now) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("[decide] {} not asked: {}", feature.as_str(), e.message);
                return LostTarget::default();
            }
        }
    };
    let request = match planned {
        Plan::Skip => return LostTarget::default(),
        Plan::Reuse(t) => return t,
        Plan::Ask(request) => request,
    };
    let out = decide(
        ctx,
        DecideRequest {
            feature,
            subject_kind: input.kind.subject_kind().into(),
            subject_id: input.subject.clone(),
            org_id: input.org_id,
            request,
            baseline: Some(UNSURE.into()),
            question_version: input.kind.question_version().into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    if out.mode != Some(Mode::Assist) {
        return LostTarget::default();
    }
    let unsure = LostTarget {
        unsure: true,
        run_id: out.run_id,
        ..Default::default()
    };
    let Some(answer) = out.proposal() else {
        return unsure;
    };
    let Some(pid) = project_of(&answer.value) else {
        return unsure;
    };
    if !input.candidates.iter().any(|c| c.project_id == pid) {
        return unsure;
    }
    // A newer proposal takes the place of an older one nobody decided.
    if let (Some(id), Ok(s)) = (out.run_id, lock(&ctx.store)) {
        if let Err(e) = s.supersede_decision_runs(
            feature.as_str(),
            input.kind.subject_kind(),
            &input.subject,
            Mode::Assist.as_str(),
            id,
            now,
        ) {
            tracing::warn!("[decide] ignored follow-up not recorded: {}", e.message);
        }
    }
    LostTarget {
        project_id: Some(pid),
        source: Some("jev".into()),
        reason: Some(reason_for(input)),
        confidence_pct: pct(answer.confidence),
        run_id: out.run_id,
        unsure: false,
        ticket: None,
    }
}

/// The mode a form would ask `input` in, under one short lock: `None` when
/// there is nothing to ask or the gate refuses, so the form pays nothing.
pub fn mode_for(ctx: &DecideCtx, input: &LostInput) -> Option<Mode> {
    if input.candidates.is_empty() {
        return None;
    }
    let s = lock(&ctx.store).ok()?;
    gate_at(&s, input.kind.feature(), input.org_id, ctx.now()).ok()
}

/// [`ask`] as a form calls it: `assist` awaited, `shadow` spawned and only
/// recorded, anything else nothing.
pub async fn propose(ctx: &DecideCtx, input: LostInput) -> LostTarget {
    match mode_for(ctx, &input) {
        Some(Mode::Assist) => ask(ctx, &input).await,
        Some(Mode::Shadow) => {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                ask(&ctx, &input).await;
            });
            LostTarget::default()
        }
        None => LostTarget::default(),
    }
}

/// After a PERSON's Adopt or Restore of `subject` into `chosen` (`None`: no
/// project): mark the latest assist proposal nobody decided `confirmed`
/// (same project), `corrected` (to theirs) or `rejected` (they chose none).
/// Returns whether a run was marked. Never marks a shadow answer, an
/// `unsure` one, or a run that already has a follow-up.
pub fn record_choice(
    s: &Store,
    kind: LostKind,
    subject: &str,
    chosen: Option<i64>,
    now: i64,
) -> Result<bool, IpcError> {
    let feature = kind.feature();
    let runs =
        s.decision_runs_for_subjects(feature.as_str(), kind.subject_kind(), subject, None)?;
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
    match chosen {
        Some(p) if proposed == Some(p) => s.set_decision_followup(r.id, "confirmed", None, now),
        Some(p) => s.set_decision_followup(r.id, "corrected", Some(&option_of(p)), now),
        None => s.set_decision_followup(r.id, "rejected", None, now),
    }
}
