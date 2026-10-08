//! J1 `work_link` live (redesign step 6.8): which work item a session no
//! rule could link is working on. The offline benchmark
//! (`fleet-hub decide bench work-link`, [`super::bench::work_link`]) asks
//! the same question; acceptance there (D32) is what may move this feature
//! from `shadow` to `assist`. Until then it stays `off` by default.
//!
//! * **When.** On a person's prompt in a session's current conversation
//!   ([`crate::service::hooks::work_link_subject`]), once that conversation
//!   has had [`NUDGE_AFTER_TURNS`] turns and kept a first prompt, while the
//!   session has no live link (confirmed or suggested). Never for the
//!   operator. The call runs off the hook's path (spawned); the hook never
//!   waits for it.
//! * **What is sent.** The conversation's first prompt with every key,
//!   ticket URL and the branch removed ([`state_text`]; a key in it would
//!   have made a rule link already), and one Choice over the candidates
//!   the classification nudge offers ([`candidate_items`], at most
//!   [`MAX_CANDIDATES`]), each described by its title, plus `none`.
//! * **Shadow** records the answer and does nothing else. **Assist** turns
//!   a usable answer (at or above [`MIN_CONFIDENCE`], not `none`) into a
//!   pre-selected suggestion through the resolver (rule R12, source `jev`):
//!   the row's chip, Review and Details show it as "Proposed by Jev", and a
//!   person confirms or rejects it. A pair the person rejected is never
//!   proposed again (R9). Nothing is linked by itself.
//! * **Asked once per input.** A decided run about the same session on the
//!   same input fingerprint, question version and mode is reused, not asked
//!   again: every later prompt of the conversation costs nothing.
//! * **Follow-up.** A person's decision on the suggestion marks the run
//!   `confirmed` or `rejected` ([`record_decision`]); a person confirming a
//!   different link of the same session marks it `corrected`. A shadow
//!   answer nobody saw is never marked (D34, D37).

use super::Question;
use super::{decide, fingerprint, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode};
use crate::ipc_error::{lock, IpcError};
use crate::service::work::nudge::{candidate_items, rejected_keys, NUDGE_AFTER_TURNS};
use crate::store::{DecisionRunRow, SessionRow, Store, WorkItemRow, DECISION_NO_BASELINE};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex};

/// The question's version: bump it when the question below changes (and
/// the benchmark's with it: they are one question).
pub const QUESTION_VERSION: &str = "work_link.v1";
/// What a run is about: one session (its row's id), the subject the row's
/// proposals are read by.
pub const SUBJECT_KIND: &str = "session";
/// The option that means "none of these".
pub const NONE_OPTION: &str = "none";
/// Below this confidence an answer is recorded, not suggested.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// The most candidates one question offers (test map J1: ≤ 50; live, the
/// person's own tickets and recent local items, far fewer).
pub const MAX_CANDIDATES: usize = 20;
/// The resolver rule a suggestion from this adapter carries.
pub const RULE: &str = "R12";

/// The instruction, read literally (the benchmark sends the same).
pub const INSTRUCTIONS: &str = "Which one of these work items is this coding session \
    working on? The state is the first prompt a person typed into the session; ticket keys, \
    links and branch names were removed from it. Answer \"none\" if none of them fits or it \
    cannot be told.";

/// What `none` means, as the question describes it.
pub const NONE_MEANS: &str =
    "None of these: the session works on something else, or it cannot be told";

/// PURE: an item's option word.
pub fn option_id(item_id: i64) -> String {
    format!("i{item_id}")
}

/// PURE: the item an option names; `None` for `none` or anything else.
pub fn item_of(option: &str) -> Option<i64> {
    option.strip_prefix('i')?.parse().ok()
}

/// PURE: the Choice over `candidates` (option id, title), plus
/// [`NONE_OPTION`].
pub fn question<'a>(candidates: impl IntoIterator<Item = (String, &'a str)>) -> Question {
    let mut criteria: BTreeMap<String, Option<Value>> = candidates
        .into_iter()
        .map(|(id, title)| (id, Some(Value::String(title.to_string()))))
        .collect();
    criteria.insert(
        NONE_OPTION.to_string(),
        Some(Value::String(NONE_MEANS.into())),
    );
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria,
    }
}

/// PURE: the request for a first prompt already stripped ([`state_text`]).
pub fn request(state: &str, candidates: &[WorkItemRow]) -> JevRequest {
    JevRequest {
        state: serde_json::json!({ "first_prompt": state }),
        question: question(
            candidates
                .iter()
                .map(|c| (option_id(c.id), c.title.as_str())),
        ),
    }
}

static URL_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?i)(?:\b(?:https?|ftp|ssh|git|wss?)://|\bwww\.)[^\s<>()\[\]{}'"`]+"#)
        .expect("URL pattern compiles")
});

/// Every case-insensitive occurrence of `needle` replaced by `with`,
/// repeated until none is left (a removal can join two halves).
pub(crate) fn remove_ci(text: &str, needle: &str, with: &str) -> String {
    let needle = needle.trim();
    if needle.is_empty() {
        return text.to_string();
    }
    let re = match regex::Regex::new(&format!("(?i){}", regex::escape(needle))) {
        Ok(r) => r,
        Err(_) => return text.to_string(),
    };
    let mut t = text.to_string();
    for _ in 0..16 {
        if !re.is_match(&t) {
            break;
        }
        t = re.replace_all(&t, with).into_owned();
    }
    t
}

/// PURE: `prompt` with every recogniser match (keys, ticket URLs, `#123`),
/// every URL and the branch (whole, and by its path segments) removed —
/// the first three steps of the benchmark's leakage guard, which adds the
/// truth's own words on top. Not yet the envelope's redaction.
pub fn strip_refs(
    prompt: &str,
    ctx: &crate::service::work::recognize::RecognizeCtx,
    branch: Option<&str>,
) -> String {
    let mut t = prompt.to_string();
    let mut spans: Vec<(usize, usize)> = crate::service::work::recognize::recognize(&t, ctx)
        .into_iter()
        .map(|m| m.span)
        .collect();
    spans.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (a, b) in spans {
        match merged.last_mut() {
            Some(last) if a < last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    for (a, b) in merged.into_iter().rev() {
        if t.is_char_boundary(a) && t.is_char_boundary(b) && a <= b && b <= t.len() {
            t.replace_range(a..b, "[ref]");
        }
    }
    t = URL_RE.replace_all(&t, "[url]").into_owned();
    if let Some(b) = branch.map(str::trim).filter(|b| !b.is_empty()) {
        t = remove_ci(&t, b, "[branch]");
        for seg in b.split('/').filter(|s| s.chars().count() >= 3) {
            t = remove_ci(&t, seg, "[branch]");
        }
    }
    t
}

/// The recognition context the stripping uses: no prefixes (so every
/// key-shaped token counts), every configured tracker's host, and a
/// placeholder repository so a bare `#123` is caught too.
pub fn strip_ctx(s: &Store) -> Result<crate::service::work::recognize::RecognizeCtx, IpcError> {
    Ok(crate::service::work::recognize::RecognizeCtx {
        prefixes: Vec::new(),
        trackers: crate::service::work::detect::tracker_hosts(&s.list_trackers()?),
        repo: Some("bench/repo".into()),
    })
}

/// PURE: the state a model is shown: [`strip_refs`], the envelope's
/// redaction, whitespace collapsed.
pub fn state_text(
    prompt: &str,
    ctx: &crate::service::work::recognize::RecognizeCtx,
    branch: Option<&str>,
) -> String {
    let t = super::redact_state(&strip_refs(prompt, ctx, branch));
    t.split_whitespace().collect::<Vec<_>>().join(" ")
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

/// What [`ask`] would ask, worked out under the caller's lock.
struct Planned {
    org_id: Option<i64>,
    request: JevRequest,
    candidates: Vec<WorkItemRow>,
}

/// Whether to ask about session `session_id` now, and what. `None` when
/// the gate refuses, the session is not one to ask about (see the module
/// doc), or the same question was decided already.
fn plan(s: &Store, session_id: i64, now: i64) -> Result<Option<Planned>, IpcError> {
    let Some(row) = s.get_session_by_id(session_id)? else {
        return Ok(None);
    };
    let Ok(mode) = gate_at(s, Feature::WorkLink, row.org_id, now) else {
        return Ok(None);
    };
    let Some((first_prompt, links)) = subject(s, &row)? else {
        return Ok(None);
    };
    let rejected = rejected_keys(s, &links)?;
    let candidates = candidate_items(s, &row, &rejected, MAX_CANDIDATES, now)?;
    if candidates.is_empty() {
        return Ok(None);
    }
    let branch = s.detection_state(session_id)?.and_then(|st| st.branch);
    let state = state_text(&first_prompt, &strip_ctx(s)?, branch.as_deref());
    if state.is_empty() {
        return Ok(None);
    }
    let request = request(&state, &candidates);
    let fp = fingerprint(&s.decision_fp_key()?, &request.redacted());
    let subject = session_id.to_string();
    let asked = s
        .decision_runs_for_subjects(Feature::WorkLink.as_str(), SUBJECT_KIND, &subject, None)?
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
        candidates,
    }))
}

/// The session's first prompt and links, when it is one to ask about: its
/// current conversation has had [`NUDGE_AFTER_TURNS`] turns and kept a
/// first prompt, it has no live link, and it is not the operator.
fn subject(
    s: &Store,
    row: &SessionRow,
) -> Result<Option<(String, Vec<crate::store::WorkLinkRow>)>, IpcError> {
    if crate::service::operator::is_operator_session(s, &row.host_alias, &row.tmux_name) {
        return Ok(None);
    }
    let Some(current) = row.claude_session_id.as_deref() else {
        return Ok(None);
    };
    let Some(conv) = s.get_conversation(row.id, current)? else {
        return Ok(None);
    };
    if conv.turns < NUDGE_AFTER_TURNS {
        return Ok(None);
    }
    let Some(first_prompt) = conv.first_prompt.filter(|p| !p.trim().is_empty()) else {
        return Ok(None);
    };
    let links = s.session_work_links(row.id)?;
    if links
        .iter()
        .any(|l| l.state == "confirmed" || l.state == "suggested")
    {
        return Ok(None);
    }
    Ok(Some((first_prompt, links)))
}

/// What [`ask`] did, for a caller (and a test) to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    /// Nothing was asked: the gate refused, nothing to ask, or asked before.
    Nothing,
    /// Asked and recorded; nothing proposed (shadow, `none`, a weak or
    /// failed answer).
    Recorded,
    /// Asked, and the answer is now a suggestion on the session (assist).
    Proposed { item_id: i64, key: String },
}

/// Ask (or not) the decision model which item session `session_id` works
/// on. Every failure is a recorded fallback or nothing at all, never an
/// error: a prompt never depends on this. Holds the store lock only to plan
/// and to apply, never across the call.
pub async fn ask(ctx: &DecideCtx, session_id: i64) -> Asked {
    let now = ctx.now();
    let planned = {
        let Ok(s) = lock(&ctx.store) else {
            return Asked::Nothing;
        };
        match plan(&s, session_id, now) {
            Ok(Some(p)) => p,
            Ok(None) => return Asked::Nothing,
            Err(e) => {
                tracing::warn!("[decide] work_link not asked: {}", e.message);
                return Asked::Nothing;
            }
        }
    };
    let subject = session_id.to_string();
    let out = decide(
        ctx,
        DecideRequest {
            feature: Feature::WorkLink,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: subject.clone(),
            org_id: planned.org_id,
            request: planned.request,
            // No rule had an answer: that is why it is asked.
            baseline: Some(DECISION_NO_BASELINE.into()),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    let Some(answer) = out.proposal() else {
        return Asked::Recorded;
    };
    let Some(item) = item_of(&answer.value)
        .and_then(|id| planned.candidates.iter().find(|c| c.id == id).cloned())
    else {
        return Asked::Recorded;
    };
    let Some(key) = item.key.clone() else {
        return Asked::Recorded;
    };
    let Ok(s) = lock(&ctx.store) else {
        return Asked::Recorded;
    };
    if let Some(id) = out.run_id {
        if let Err(e) = s.supersede_decision_runs(
            Feature::WorkLink.as_str(),
            SUBJECT_KIND,
            &subject,
            Mode::Assist.as_str(),
            id,
            now,
        ) {
            tracing::warn!("[decide] ignored follow-up not recorded: {}", e.message);
        }
    }
    // The resolver may still have linked it meanwhile (a branch, a person):
    // the R12 event then only touches or is dropped, as any event is.
    let target = match crate::store::normalize_work_ref(&key) {
        Ok(t) => t,
        Err(_) => return Asked::Recorded,
    };
    let pct = answer
        .confidence
        .map(|c| (c.clamp(0.0, 1.0) * 100.0).round() as u8);
    match crate::service::work::detect::on_jev_proposal(
        &s,
        session_id,
        &target,
        item.tracker_id,
        pct,
    ) {
        Ok(_) => Asked::Proposed {
            item_id: item.id,
            key: target,
        },
        Err(e) => {
            tracing::warn!("[decide] work_link proposal not applied: {}", e.message);
            Asked::Recorded
        }
    }
}

/// [`ask`] off the caller's path, when the gate could let it through at
/// all: with the defaults (the feature `off`) it costs one short lock and
/// spawns nothing.
pub fn spawn_ask(store: &Arc<Mutex<Store>>, session_id: i64) {
    let open = lock(store).ok().is_some_and(|s| {
        let org = s
            .get_session_by_id(session_id)
            .ok()
            .flatten()
            .map(|r| r.org_id);
        org.is_some_and(|org| {
            gate_at(
                &s,
                Feature::WorkLink,
                org,
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

/// A PERSON decided a link of session `session_id`: `rule` is the rule
/// the link carries, `target` the item or key it names (as an option word
/// when it is an item, `i<id>`). On an R12 suggestion the latest assist
/// proposal nobody decided is marked `confirmed` or `rejected`; a confirm
/// of another link marks it `corrected` to that link's item (or `none`
/// when it names no item). Returns whether a run was marked. Never marks a
/// shadow answer, nor a run that already has a follow-up.
pub fn record_decision(
    s: &Store,
    session_id: i64,
    rule: Option<&str>,
    item_id: Option<i64>,
    confirm: bool,
    now: i64,
) -> Result<bool, IpcError> {
    let subject = session_id.to_string();
    let runs =
        s.decision_runs_for_subjects(Feature::WorkLink.as_str(), SUBJECT_KIND, &subject, None)?;
    let Some(r) = runs.iter().find(|r| {
        r.subject_id == subject
            && r.mode == Mode::Assist.as_str()
            && r.fallback.is_none()
            && r.answer.as_deref().and_then(item_of).is_some()
    }) else {
        return Ok(false);
    };
    if r.followup.is_some() {
        return Ok(false);
    }
    let ours = rule == Some(RULE) && item_id.map(option_id).as_deref() == r.answer.as_deref();
    match (ours, confirm) {
        (true, true) => s.set_decision_followup(r.id, "confirmed", None, now),
        (true, false) => s.set_decision_followup(r.id, "rejected", None, now),
        (false, true) => {
            let theirs = item_id.map_or_else(|| NONE_OPTION.to_string(), option_id);
            s.set_decision_followup(r.id, "corrected", Some(&theirs), now)
        }
        // Rejecting some other suggestion says nothing about Jev's.
        (false, false) => Ok(false),
    }
}
