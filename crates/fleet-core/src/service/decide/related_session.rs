//! N1 `related_session` (redesign step 6.9, part 2): whether another live
//! session of the same person is working on the same thing as this one.
//! Two sessions on one problem waste a quota and race each other's
//! commits; nothing today notices it unless they share a worktree.
//!
//! * **When.** On a person's prompt in a session's current conversation
//!   (the hook [`crate::service::hooks::work_link_subject`] answers),
//!   once that conversation has had [`NUDGE_AFTER_TURNS`] turns and kept a
//!   first prompt. Never for the operator. Off the hook's path (spawned).
//! * **Candidates.** The same person's other live sessions in the same org
//!   ([`candidates`]): never another person's (their prompts are theirs),
//!   never one sharing this session's worktree (Related sessions lists those
//!   already), each with a kept first prompt; most recently active first,
//!   at most [`MAX_CANDIDATES`]. With none, nothing is asked.
//! * **What is sent.** Each session's first prompt, cut to
//!   [`PROMPT_CHARS`] characters and redacted by the envelope; options are
//!   the sessions' ids (`s<id>`) and `none`. No name, host or path.
//! * **Shadow** records only. **Assist** leaves a usable answer (at or
//!   above [`MIN_CONFIDENCE`], a session) on the row as its
//!   `related_session` proposal: in the New layout its Details list the
//!   other session under Related sessions as *Same work? · Proposed by
//!   Jev*. Nothing is stopped, merged or moved.
//! * **Asked once per input.** A decided run about the same session on the
//!   same input and question version is reused; a new candidate (another
//!   session starts) is a new input.
//! * **Tidy › Duplicates.** The planner (`service::gc::tidy`) turns a
//!   running pair into a `same_work` candidate for the idler of the two:
//!   never ticked, never automatic.
//! * **Follow-up.** None yet: tidying that candidate is not recorded
//!   against the run. A shadow answer is never marked.

use super::{
    decide, fingerprint, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode, Question,
};
use crate::ipc_error::{lock, IpcError};
use crate::service::work::nudge::NUDGE_AFTER_TURNS;
use crate::store::{DecisionRunRow, SessionRow, Store, PROPOSAL_SUBJECT_SESSION};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "related_session.v1";
/// What a run is about: one session (its row's id), the subject the row's
/// proposals are read by.
pub const SUBJECT_KIND: &str = PROPOSAL_SUBJECT_SESSION;
/// The option that means "none of these".
pub const NONE_OPTION: &str = "none";
/// Below this confidence an answer is recorded, not proposed.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// The most other sessions one question offers.
pub const MAX_CANDIDATES: usize = 8;
/// Characters of each first prompt sent, at most.
pub const PROMPT_CHARS: usize = 400;

/// The instruction, read literally.
pub const INSTRUCTIONS: &str = "state.session is the first prompt a person typed into a coding \
    session. Each option except none is another session of the same person, described by its \
    own first prompt. Decide whether one of them works on the SAME problem or change as \
    state.session, so that the two would do the same work twice. Choose none when no option \
    does: sessions in the same repository or on related topics are not the same work.";

const NONE_MEANS: &str = "No other session works on the same thing.";

/// Statuses of a session that is gone, or not a session at all.
const GONE: &[&str] = &["ghost", "stopped", "failed", "dead", "archived"];

/// PURE: a session's option word.
pub fn option_of(session_id: i64) -> String {
    format!("s{session_id}")
}

/// PURE: the session an option names; `None` for `none` or anything else.
pub fn session_of(option: &str) -> Option<i64> {
    option.strip_prefix('s')?.parse().ok()
}

/// PURE: a first prompt as sent: trimmed, cut to [`PROMPT_CHARS`].
pub fn cut(prompt: &str) -> String {
    prompt.trim().chars().take(PROMPT_CHARS).collect()
}

/// PURE: whether `other` may be offered for `row`: another live session of
/// the same owner in the same org, not on the same worktree.
pub fn eligible(row: &SessionRow, other: &SessionRow) -> bool {
    other.id != row.id
        && other.lost_at.is_none()
        && !GONE.contains(&other.status.as_str())
        && other.owner_person_id.is_some()
        && other.owner_person_id == row.owner_person_id
        && other.org_id == row.org_id
        && !(other.project_id.is_some()
            && other.project_id == row.project_id
            && other.worktree_key.is_some()
            && other.worktree_key == row.worktree_key)
}

/// The current conversation's first prompt of `row`, once that
/// conversation has had [`NUDGE_AFTER_TURNS`] turns (`min_turns`).
fn first_prompt(s: &Store, row: &SessionRow, min_turns: i64) -> Result<Option<String>, IpcError> {
    let Some(current) = row.claude_session_id.as_deref() else {
        return Ok(None);
    };
    let Some(conv) = s.get_conversation(row.id, current)? else {
        return Ok(None);
    };
    if conv.turns < min_turns {
        return Ok(None);
    }
    Ok(conv
        .first_prompt
        .filter(|p| !p.trim().is_empty())
        .map(|p| cut(&p)))
}

/// The other sessions `row` could duplicate, with their first prompts.
pub fn candidates(s: &Store, row: &SessionRow) -> Result<Vec<(i64, String)>, IpcError> {
    let mut out = Vec::new();
    // Newest activity first (`list_all_sessions`' order).
    for other in s.list_all_sessions()? {
        if out.len() >= MAX_CANDIDATES {
            break;
        }
        if !eligible(row, &other)
            || crate::service::operator::is_operator_session(s, &other.host_alias, &other.tmux_name)
        {
            continue;
        }
        if let Some(p) = first_prompt(s, &other, 1)? {
            out.push((other.id, p));
        }
    }
    Ok(out)
}

/// PURE: the request about a session whose first prompt is `prompt`.
pub fn request(prompt: &str, candidates: &[(i64, String)]) -> JevRequest {
    let mut criteria: Vec<(String, Option<Value>)> = candidates
        .iter()
        .map(|(id, p)| {
            (
                option_of(*id),
                Some(Value::String(format!(
                    "Another session whose first prompt is: {p}"
                ))),
            )
        })
        .collect();
    criteria.push((NONE_OPTION.into(), Some(Value::String(NONE_MEANS.into()))));
    JevRequest {
        state: json!({ "session": { "first_prompt": prompt } }),
        question: Question::Choice {
            instructions: Value::String(INSTRUCTIONS.into()),
            criteria: criteria.into_iter().collect(),
        },
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

struct Planned {
    org_id: Option<i64>,
    request: JevRequest,
    ids: Vec<i64>,
}

fn plan(s: &Store, session_id: i64, now: i64) -> Result<Option<Planned>, IpcError> {
    let Some(row) = s.get_session_by_id(session_id)? else {
        return Ok(None);
    };
    if row.owner_person_id.is_none()
        || crate::service::operator::is_operator_session(s, &row.host_alias, &row.tmux_name)
    {
        return Ok(None);
    }
    let Ok(mode) = gate_at(s, Feature::RelatedSession, row.org_id, now) else {
        return Ok(None);
    };
    let Some(prompt) = first_prompt(s, &row, NUDGE_AFTER_TURNS)? else {
        return Ok(None);
    };
    let found = candidates(s, &row)?;
    if found.is_empty() {
        return Ok(None);
    }
    let request = request(&prompt, &found);
    let fp = fingerprint(&s.decision_fp_key()?, &request.redacted());
    let subject = session_id.to_string();
    let asked = s
        .decision_runs_for_subjects(
            Feature::RelatedSession.as_str(),
            SUBJECT_KIND,
            &subject,
            None,
        )?
        .iter()
        .any(|r| {
            r.subject_id == subject
                && r.input_fp.as_deref() == Some(fp.as_str())
                && r.question_version == QUESTION_VERSION
                && r.mode == mode.as_str()
                && decided(r)
        });
    if asked {
        return Ok(None);
    }
    Ok(Some(Planned {
        org_id: row.org_id,
        request,
        ids: found.into_iter().map(|(id, _)| id).collect(),
    }))
}

/// Ask whether session `session_id` duplicates another of its person's.
/// `Some(other)` only in assist with a usable answer naming a candidate.
/// Every failure is a recorded fallback or nothing at all, never an error.
/// Holds the store lock only for reads, never across the call.
pub async fn ask(ctx: &DecideCtx, session_id: i64) -> Option<i64> {
    let now = ctx.now();
    let planned = {
        let s = lock(&ctx.store).ok()?;
        match plan(&s, session_id, now) {
            Ok(p) => p?,
            Err(e) => {
                tracing::warn!("[decide] related_session not asked: {}", e.message);
                return None;
            }
        }
    };
    let subject = session_id.to_string();
    let out = decide(
        ctx,
        DecideRequest {
            feature: Feature::RelatedSession,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: subject.clone(),
            org_id: planned.org_id,
            request: planned.request,
            // Today nothing notices two sessions on one thing.
            baseline: Some(NONE_OPTION.into()),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    let other = session_of(&out.proposal()?.value)?;
    if !planned.ids.contains(&other) {
        return None;
    }
    if let (Some(id), Ok(s)) = (out.run_id, lock(&ctx.store)) {
        if let Err(e) = s.supersede_decision_runs(
            Feature::RelatedSession.as_str(),
            SUBJECT_KIND,
            &subject,
            Mode::Assist.as_str(),
            id,
            now,
        ) {
            tracing::warn!("[decide] ignored follow-up not recorded: {}", e.message);
        }
    }
    Some(other)
}

/// [`ask`] off the caller's path, when the gate could let it through at
/// all: with the defaults (the feature `off`) it costs one short lock and
/// spawns nothing.
pub fn spawn_ask(store: &Arc<Mutex<Store>>, session_id: i64) {
    let open = lock(store).ok().is_some_and(|s| {
        s.get_session_by_id(session_id)
            .ok()
            .flatten()
            .is_some_and(|r| {
                gate_at(
                    &s,
                    Feature::RelatedSession,
                    r.org_id,
                    crate::service::catalog::now_secs(),
                )
                .is_ok()
            })
    });
    if !open {
        return;
    }
    let ctx = DecideCtx::jev(Arc::clone(store));
    let _ = crate::rt::try_spawn(async move {
        ask(&ctx, session_id).await;
    });
}

/// A person's answer to a session's `related_session` proposal (M15 G4.3,
/// the SessionDetails board's "Link / Not related").
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DecideRelatedSessionArgs {
    /// The session whose Details carry the proposal.
    pub session_id: i64,
    /// The proposal's run (`DecisionProposal::run_id`).
    pub run_id: i64,
    /// `true`: Link (same work); `false`: Not related.
    pub linked: bool,
}

/// Mark the proposal's run `confirmed` (Link: the other session stays
/// listed under Related sessions as linked) or `rejected` (Not related: it
/// leaves, and the same answer is not proposed again on the same input).
/// Only an undecided assist run of this feature about `session_id` that
/// names a session is marked; answers whether one was. Nothing is stopped,
/// merged or moved.
pub fn decide_proposal(
    s: &Store,
    args: &DecideRelatedSessionArgs,
    now: i64,
) -> Result<bool, IpcError> {
    let Some(r) = s.get_decision_run(args.run_id)? else {
        return Ok(false);
    };
    if r.feature != Feature::RelatedSession.as_str()
        || r.subject_kind != SUBJECT_KIND
        || r.subject_id != args.session_id.to_string()
        || r.mode != Mode::Assist.as_str()
        || r.followup.is_some()
        || r.answer.as_deref().and_then(session_of).is_none()
    {
        return Ok(false);
    }
    let mark = if args.linked { "confirmed" } else { "rejected" };
    s.set_decision_followup(r.id, mark, None, now)
}

/// [`decide_proposal`] behind the store lock, answering the session's row
/// as it reads now (its proposals re-read). `E_NOTFOUND` for a session that
/// is not there; a proposal already decided or withdrawn changes nothing.
pub fn decide_related_session(
    args: DecideRelatedSessionArgs,
    store: &Mutex<Store>,
) -> Result<SessionRow, IpcError> {
    let s = lock(store)?;
    decide_proposal(&s, &args, crate::service::catalog::now_secs())?;
    s.get_session_by_id(args.session_id)?.ok_or_else(|| {
        IpcError::new(
            crate::ipc_error::codes::E_NOTFOUND,
            format!("session {} not found", args.session_id),
        )
    })
}
