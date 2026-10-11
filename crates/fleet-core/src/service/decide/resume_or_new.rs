//! N2 `resume_or_new`: resume a past session of the work key a new session
//! plans, or start fresh (gap plan G7.10; the transition plan's "Resume or
//! start fresh"). The New session dialog reads the key from the branch it
//! will run on and, when that key has only PAST work, says so: *Has previous
//! work: Resume pd-2412 · Proposed by Jev · Start fresh instead*.
//!
//! * **Rule first.** Exactly one resumable past session, ended within
//!   `work.recent_days`, is that session ([`rule`]): nobody asks Jev. Only
//!   when the rule does not decide (two or more past sessions, or one that
//!   ended long ago) does this adapter ask one Choice over them — resume
//!   which ONE (`l<link id>`), `new`, or `unsure`. It mirrors J10/N4
//!   [`super::lost_target`].
//! * **Shadow / assist only.** In `shadow` the question is asked off the
//!   dialog's path and only recorded, with `unsure` (today's plain notice) as
//!   the baseline. In `assist` the dialog waits for the one bounded call
//!   (`decide.jev.timeout_ms`) and a usable answer becomes the notice's
//!   proposal. Nothing resumes or starts until a person presses Resume (the
//!   existing resume flow) or Create.
//! * **What is sent.** The key, and each past session's name, branch, the
//!   days since it ended, its role and whether it opened a pull request —
//!   through the envelope's redaction. Nothing from the conversations.
//! * **What is recorded.** A run about subject `work_resume`
//!   `key:<HMAC of the key>` (a key is never recorded as is); the options
//!   are the past sessions' work links (`l<id>`: a past session's row may be
//!   gone, its link is what the resume flow re-opens), `new` and `unsure`.
//! * **Asked once per input**, as K1: a decided run on the same subject,
//!   input fingerprint, question version, mode and pinned model in the last
//!   [`REASK_DAYS`] days is reused.
//! * **Follow-up.** The person's Resume or "Start fresh instead"
//!   ([`record_choice`]) marks the latest assist proposal nobody decided
//!   `confirmed` (the same option) or `corrected` (to theirs). A shadow
//!   answer nobody saw, an `unsure` one and a rule's are never marked.

use super::start_project::{decided, pct};
use super::{
    decide, fingerprint, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode, Question,
};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::settings;
use crate::service::view_scope::ViewScope;
use crate::store::{DecisionRunRow, Secret, Store, WorkLinkRow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use super::start_project::{MIN_CONFIDENCE, REASK_DAYS, UNSURE};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "resume_or_new.v1";
/// What a run is about: one work key's next session.
pub const SUBJECT_KIND: &str = "work_resume";
/// The option that starts fresh.
pub const NEW: &str = "new";
/// The most past sessions one question carries (newest first).
pub const MAX_CANDIDATES: usize = 8;
/// Characters of a past session's name or branch sent, at most.
pub const NAME_CHARS: usize = 80;

/// The instruction, read literally: the exact condition and what to use.
pub const INSTRUCTIONS: &str = "state.key is the work key (a ticket or workstream) a person is \
     about to start a new coding session on. state.past lists the earlier sessions on that key \
     that have ended: each one's name, git branch, days since it ended, role, and whether it \
     opened a pull request. Each option l<id> resumes one of those past sessions, continuing \
     its conversation in its worktree; new starts a fresh session with nothing carried. Decide \
     which, using only those facts. Choose unsure when they do not show it.";

/// One past session of the key, as a candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PastSession {
    /// Its work link: what the resume flow re-opens.
    pub link_id: i64,
    /// The session's row, while it still exists.
    pub session_id: Option<i64>,
    pub name: String,
    pub branch: Option<String>,
    pub ended_at: i64,
    /// `work` | `review` | `worker`.
    pub role: String,
    pub has_pr: bool,
}

/// What the adapter asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeInput {
    /// The work key, normalised.
    pub key: String,
    /// The run's subject ([`subject_id`]).
    pub subject: String,
    /// The key's org (its consent is checked); `None` when it has none.
    pub org_id: Option<i64>,
    /// `work.recent_days`: how recent the rule's one past session must be.
    pub recent_days: i64,
    /// Newest first, resumable ones only.
    pub past: Vec<PastSession>,
}

/// What the New session dialog's past-work notice shows: resume one past
/// session, or start fresh, who proposed it and why. Every field empty
/// when nothing is proposed (the notice stays the plain one).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
pub struct ResumeOrNew {
    /// `l<link id>` (resume that past session) or `new`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The past session's work link, for a resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_id: Option<i64>,
    /// The past session's row, while it still exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    /// The past session's name, for a resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `rule` (the only recent past session) or `jev`.
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
    /// Jev was asked and proposed nothing usable: the notice stays plain.
    #[serde(default)]
    pub unsure: bool,
}

/// PURE: a past session's option word.
pub fn option_of(link_id: i64) -> String {
    format!("l{link_id}")
}

/// PURE: the link an option names; `None` for `new`, `unsure` or anything
/// else.
pub fn link_of(option: &str) -> Option<i64> {
    option.strip_prefix('l')?.parse().ok()
}

/// PURE: whether `option` is an answer that proposes something.
fn names_something(option: &str) -> bool {
    option == NEW || link_of(option).is_some()
}

fn clip(s: &str) -> String {
    s.chars().take(NAME_CHARS).collect()
}

/// The resumable past sessions among `links` (a key's ended links, as the
/// caller may see them), newest first, at most [`MAX_CANDIDATES`].
pub fn past_sessions(s: &Store, links: &[WorkLinkRow]) -> Result<Vec<PastSession>, IpcError> {
    let mut out: Vec<PastSession> = Vec::new();
    for l in links {
        let Some(ended_at) = l.ended_at else { continue };
        if l.state != "confirmed" || !l.resumable {
            continue;
        }
        let name = [&l.snap_name, &l.snap_tmux, &l.snap_branch]
            .into_iter()
            .flatten()
            .map(|v| v.trim())
            .find(|v| !v.is_empty())
            .map(clip)
            .unwrap_or_else(|| format!("session {}", l.id));
        out.push(PastSession {
            link_id: l.id,
            session_id: s.link_session_id(l)?,
            name,
            branch: l.snap_branch.as_deref().map(clip),
            ended_at,
            role: l.role.clone(),
            has_pr: l
                .snap_pr_url
                .as_deref()
                .is_some_and(|u| !u.trim().is_empty()),
        });
    }
    out.sort_by(|a, b| b.ended_at.cmp(&a.ended_at).then(b.link_id.cmp(&a.link_id)));
    out.truncate(MAX_CANDIDATES);
    Ok(out)
}

/// PURE: the run's subject id: 16 hex digits of an HMAC of the key under
/// the local fingerprint key (a key is free text, never recorded as is).
pub fn subject_id(fp_key: &Secret, key: &str) -> String {
    let mac = super::hmac_sha256_hex(
        fp_key.expose().as_bytes(),
        format!("resume_or_new.key\n{key}").as_bytes(),
    );
    format!("key:{}", &mac[..16])
}

/// PURE: whole days between `at` and `now`.
fn days_ago(at: i64, now: i64) -> i64 {
    (now - at).max(0) / 86_400
}

fn ended_words(days: i64) -> String {
    match days {
        0 => "ended today".to_string(),
        1 => "ended yesterday".to_string(),
        n => format!("ended {n} days ago"),
    }
}

fn resume_of(p: &PastSession) -> ResumeOrNew {
    ResumeOrNew {
        value: Some(option_of(p.link_id)),
        link_id: Some(p.link_id),
        session_id: p.session_id,
        name: Some(p.name.clone()),
        ..Default::default()
    }
}

/// PURE: the rule. Exactly one resumable past session, ended within
/// `recent_days`, is the one to resume; anything else is left to Jev
/// (`None`).
pub fn rule(input: &ResumeInput, now: i64) -> Option<ResumeOrNew> {
    let [only] = input.past.as_slice() else {
        return None;
    };
    let days = days_ago(only.ended_at, now);
    if days > input.recent_days.max(0) {
        return None;
    }
    Some(ResumeOrNew {
        source: Some("rule".into()),
        reason: Some(format!("the only past session, {}", ended_words(days))),
        ..resume_of(only)
    })
}

/// PURE: the words under a Jev proposal: what it went on.
pub fn reason_for(value: &str, input: &ResumeInput, now: i64) -> String {
    match link_of(value).and_then(|id| input.past.iter().find(|p| p.link_id == id)) {
        Some(p) => format!(
            "its name, branch and {}",
            ended_words(days_ago(p.ended_at, now))
        ),
        None => "the past sessions' names, branches and when they ended".to_string(),
    }
}

/// PURE: the question over `input`'s past sessions.
pub fn question(input: &ResumeInput) -> Question {
    let mut criteria: Vec<(String, Option<Value>)> = input
        .past
        .iter()
        .map(|p| {
            (
                option_of(p.link_id),
                Some(Value::String(format!(
                    "Resume the past session {} and continue its conversation.",
                    p.name
                ))),
            )
        })
        .collect();
    criteria.push((
        NEW.to_string(),
        Some(Value::String(
            "Start a fresh session; none of the past sessions should be continued.".into(),
        )),
    ));
    criteria.push((
        UNSURE.to_string(),
        Some(Value::String(
            "The names, branches and times do not show whether to resume or which one.".into(),
        )),
    ));
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: criteria.into_iter().collect(),
    }
}

/// PURE: the state: the key and the past sessions' facts, no content.
pub fn state(input: &ResumeInput, now: i64) -> Value {
    json!({
        "key": input.key,
        "past": input
            .past
            .iter()
            .map(|p| json!({
                "option": option_of(p.link_id),
                "name": p.name,
                "branch": p.branch.as_deref().unwrap_or(""),
                "ended_days_ago": days_ago(p.ended_at, now),
                "role": p.role,
                "pull_request": p.has_pr,
            }))
            .collect::<Vec<_>>(),
    })
}

/// PURE: the request for `input`.
pub fn question_for(input: &ResumeInput, now: i64) -> JevRequest {
    JevRequest {
        state: state(input, now),
        question: question(input),
    }
}

/// PURE: what an answer proposes for `input`, or `None` (unsure, or an
/// option not among the past sessions).
fn proposal_of(
    value: &str,
    input: &ResumeInput,
    confidence: Option<f64>,
    run_id: Option<i64>,
    now: i64,
) -> Option<ResumeOrNew> {
    let base = if value == NEW {
        ResumeOrNew {
            value: Some(NEW.into()),
            ..Default::default()
        }
    } else {
        let id = link_of(value)?;
        resume_of(input.past.iter().find(|p| p.link_id == id)?)
    };
    Some(ResumeOrNew {
        source: Some("jev".into()),
        reason: Some(reason_for(value, input, now)),
        confidence_pct: pct(confidence),
        run_id,
        unsure: false,
        ..base
    })
}

fn unsure(run_id: Option<i64>) -> ResumeOrNew {
    ResumeOrNew {
        unsure: true,
        run_id,
        ..Default::default()
    }
}

/// PURE: what a decided run proposes for `input`.
fn proposed(r: &DecisionRunRow, input: &ResumeInput, now: i64) -> ResumeOrNew {
    if r.fallback.is_some() || r.followup.as_deref() == Some("rejected") {
        return unsure(Some(r.id));
    }
    r.answer
        .as_deref()
        .and_then(|a| proposal_of(a, input, r.confidence, Some(r.id), now))
        .unwrap_or_else(|| unsure(Some(r.id)))
}

/// What [`ask`] did before any call: the mode, and the run to reuse.
enum Plan {
    Skip,
    Reuse(ResumeOrNew),
    Ask(JevRequest),
}

fn plan(s: &Store, input: &ResumeInput, now: i64) -> Result<Plan, IpcError> {
    if input.past.is_empty() {
        return Ok(Plan::Skip);
    }
    let feature = Feature::ResumeOrNew;
    let Ok(mode) = gate_at(s, feature, input.org_id, now) else {
        return Ok(Plan::Skip);
    };
    let fp_key = s.decision_fp_key()?;
    let request = question_for(input, now);
    let fp = fingerprint(&fp_key, &request.redacted());
    let model = settings::get_string(s, settings::DECIDE_JEV_MODEL);
    let since = now - REASK_DAYS * 86_400;
    let runs =
        s.decision_runs_for_subjects(feature.as_str(), SUBJECT_KIND, &input.subject, Some(since))?;
    let recent = runs.iter().find(|r| {
        r.subject_id == input.subject
            && r.input_fp.as_deref() == Some(fp.as_str())
            && r.question_version == QUESTION_VERSION
            && r.mode == mode.as_str()
            && decided(r)
            && (model == "jev-latest" || r.model_version.as_deref() == Some(model.as_str()))
    });
    if let Some(r) = recent {
        return Ok(Plan::Reuse(if mode == Mode::Assist {
            proposed(r, input, now)
        } else {
            ResumeOrNew::default()
        }));
    }
    Ok(Plan::Ask(request))
}

/// Ask (or reuse) the decision model's answer for `input`. In `assist`, a
/// usable answer naming a past session or `new` is proposed; any other
/// answer is `unsure`. In `shadow`, and on every failure, nothing: the
/// notice never depends on this. Holds the store lock only for reads,
/// never across the call.
pub async fn ask(ctx: &DecideCtx, input: &ResumeInput) -> ResumeOrNew {
    let now = ctx.now();
    let feature = Feature::ResumeOrNew;
    let planned = {
        let Ok(s) = lock(&ctx.store) else {
            return ResumeOrNew::default();
        };
        match plan(&s, input, now) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("[decide] {} not asked: {}", feature.as_str(), e.message);
                return ResumeOrNew::default();
            }
        }
    };
    let request = match planned {
        Plan::Skip => return ResumeOrNew::default(),
        Plan::Reuse(t) => return t,
        Plan::Ask(request) => request,
    };
    let out = decide(
        ctx,
        DecideRequest {
            feature,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: input.subject.clone(),
            org_id: input.org_id,
            request,
            baseline: Some(UNSURE.into()),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    if out.mode != Some(Mode::Assist) {
        return ResumeOrNew::default();
    }
    let Some(answer) = out.proposal() else {
        return unsure(out.run_id);
    };
    let Some(proposal) = proposal_of(&answer.value, input, answer.confidence, out.run_id, now)
    else {
        return unsure(out.run_id);
    };
    // A newer proposal takes the place of an older one nobody decided.
    if let (Some(id), Ok(s)) = (out.run_id, lock(&ctx.store)) {
        if let Err(e) = s.supersede_decision_runs(
            feature.as_str(),
            SUBJECT_KIND,
            &input.subject,
            Mode::Assist.as_str(),
            id,
            now,
        ) {
            tracing::warn!("[decide] ignored follow-up not recorded: {}", e.message);
        }
    }
    proposal
}

/// The mode the dialog would ask `input` in, under one short lock: `None`
/// when there is nothing to ask or the gate refuses, so it pays nothing.
pub fn mode_for(ctx: &DecideCtx, input: &ResumeInput) -> Option<Mode> {
    if input.past.is_empty() {
        return None;
    }
    let s = lock(&ctx.store).ok()?;
    gate_at(&s, Feature::ResumeOrNew, input.org_id, ctx.now()).ok()
}

/// What the notice shows for `input`: the rule's answer when it decides
/// (whatever the mode: a rule is not AI), else [`ask`] as the dialog calls
/// it — `assist` awaited, `shadow` spawned and only recorded, anything
/// else nothing.
pub async fn propose(ctx: &DecideCtx, input: ResumeInput) -> ResumeOrNew {
    if let Some(r) = rule(&input, ctx.now()) {
        return r;
    }
    match mode_for(ctx, &input) {
        Some(Mode::Assist) => ask(ctx, &input).await,
        Some(Mode::Shadow) => {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                ask(&ctx, &input).await;
            });
            ResumeOrNew::default()
        }
        None => ResumeOrNew::default(),
    }
}

/// The input for `key` from its ended links as `view` may see them: `None`
/// when no resumable past session is left (nothing to propose).
pub fn input_for(
    s: &Store,
    key: &str,
    links: &[WorkLinkRow],
) -> Result<Option<ResumeInput>, IpcError> {
    let past = past_sessions(s, links)?;
    if past.is_empty() {
        return Ok(None);
    }
    let org_id = match s.work_item_by_key(key)? {
        Some(item) => s.item_org(item.id)?,
        None => links.iter().find_map(|l| l.org_id),
    };
    let recent_days = settings::get_string(s, settings::WORK_RECENT_DAYS)
        .parse::<i64>()
        .unwrap_or(14);
    Ok(Some(ResumeInput {
        key: key.to_string(),
        subject: subject_id(&s.decision_fp_key()?, key),
        org_id,
        recent_days,
        past,
    }))
}

/// `key`'s ended links as `view` may see them (the same read as `work
/// {key}`), and the key normalised.
fn visible_links(
    store: &std::sync::Mutex<Store>,
    view: &ViewScope,
    key: &str,
) -> Result<(String, Vec<WorkLinkRow>), IpcError> {
    let key = crate::store::normalize_work_ref(key)?;
    let args = crate::service::work::WorkArgs {
        key: Some(key.clone()),
        ..Default::default()
    };
    Ok((key, crate::service::work::work(&args, store, view)?))
}

/// The New session dialog's proposal for `key` (the `resume_or_new` tool's
/// `propose`): the rule's, else Jev's at assist, else nothing.
pub async fn propose_for_key(
    ctx: &DecideCtx,
    view: &ViewScope,
    key: &str,
) -> Result<ResumeOrNew, IpcError> {
    let (key, links) = visible_links(&ctx.store, view, key)?;
    let input = {
        let s = lock(&ctx.store)?;
        input_for(&s, &key, &links)?
    };
    match input {
        Some(input) => Ok(propose(ctx, input).await),
        None => Ok(ResumeOrNew::default()),
    }
}

/// After a PERSON's choice for `subject`: `chosen` is `l<id>` (Resume) or
/// `new` ("Start fresh instead"). Marks the latest assist proposal nobody
/// decided `confirmed` (the same option) or `corrected` (to theirs).
/// Returns whether a run was marked. Never marks a shadow answer, an
/// `unsure` one, or a run that already has a follow-up.
pub fn record_choice(s: &Store, subject: &str, chosen: &str, now: i64) -> Result<bool, IpcError> {
    let feature = Feature::ResumeOrNew;
    let runs = s.decision_runs_for_subjects(feature.as_str(), SUBJECT_KIND, subject, None)?;
    let Some(r) = runs.iter().find(|r| {
        r.subject_id == subject
            && r.mode == Mode::Assist.as_str()
            && r.fallback.is_none()
            && r.answer.as_deref().is_some_and(names_something)
    }) else {
        return Ok(false);
    };
    if r.followup.is_some() {
        return Ok(false);
    }
    if r.answer.as_deref() == Some(chosen) {
        s.set_decision_followup(r.id, "confirmed", None, now)
    } else {
        s.set_decision_followup(r.id, "corrected", Some(chosen), now)
    }
}

/// [`record_choice`] for `key` as the `resume_or_new` tool's `follow`
/// takes it: `chosen` must be `new` or one of the key's past sessions
/// `view` may see (`E_INVALID` otherwise).
pub fn follow_for_key(
    store: &std::sync::Mutex<Store>,
    view: &ViewScope,
    key: &str,
    chosen: &str,
    now: i64,
) -> Result<bool, IpcError> {
    let (key, links) = visible_links(store, view, key)?;
    let known = chosen == NEW || link_of(chosen).is_some_and(|id| links.iter().any(|l| l.id == id));
    if !known {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("{chosen:?} is neither new nor one of this key's past sessions"),
        ));
    }
    if links.is_empty() {
        return Ok(false);
    }
    let s = lock(store)?;
    let subject = subject_id(&s.decision_fp_key()?, &key);
    record_choice(&s, &subject, chosen, now)
}
