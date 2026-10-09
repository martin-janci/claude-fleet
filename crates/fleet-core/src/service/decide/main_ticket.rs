//! J6 `main_ticket` (redesign step 6.8, test map J6 "the subject among
//! several keys"): which ONE of the ticket keys a session's prompt names is
//! the session's main ticket. A prompt that names several keys ("fix
//! PAY-12; PAY-9 was the first try, see also OPS-3") leaves detection with
//! one weak suggestion per key, and Review shows them side by side.
//!
//! * **Rule first** ([`rule`]). One key is that key. A branch that names
//!   exactly one of the keys is that key. More than [`MAX_KEYS`] keys is a
//!   pasted list of references (the dump guard): nothing is pre-selected.
//!   A session that already has a confirmed primary is the person's. In
//!   each of these nobody is asked and nothing is recorded.
//! * **When.** On a person's prompt in the session's current conversation
//!   (beside J1, off the hook's path), once detection has left two or more
//!   suggested keys.
//! * **What is sent.** The conversation's first prompt, its first
//!   [`PROMPT_CHARS`] characters, with every candidate key replaced by a
//!   placeholder (`[K1]`, `[K2]` …) so the answer is a choice between
//!   placeholders, and each candidate's title, redacted by the envelope.
//!   Options are `k<link id>` and `unsure`.
//! * **Shadow** records only, with `unsure` (today nothing picks one) as
//!   the baseline. **Assist** leaves a usable answer (at or above
//!   [`MIN_CONFIDENCE`], a key) on the session row as its `main_ticket`
//!   proposal; Review marks that suggestion *Proposed by Jev · main ticket
//!   among N keys*. Nothing is confirmed by itself: Confirm, Reject and
//!   Change… stay a person's.
//! * **Asked once per input.** A decided run about the same session on the
//!   same input and question version is reused.
//! * **Follow-up.** A person's Confirm or Reject of a suggestion the run
//!   named marks it `confirmed` or `rejected` ([`record_decision`]); a
//!   shadow answer is never marked.

use super::{
    decide, fingerprint, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode, Question,
};
use crate::ipc_error::{lock, IpcError};
use crate::store::{DecisionRunRow, Store, PROPOSAL_SUBJECT_SESSION};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "main_ticket.v1";
/// What a run is about: one session (its row's id), so the row's
/// proposals carry the answer.
pub const SUBJECT_KIND: &str = PROPOSAL_SUBJECT_SESSION;
/// The option that proposes nothing.
pub const UNSURE: &str = "unsure";
/// Below this confidence an answer is recorded, not proposed.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// More keys than this is a list of references, never asked about.
pub const MAX_KEYS: usize = 8;
/// Characters of the prompt sent, at most.
pub const PROMPT_CHARS: usize = 1_000;

/// The instruction, read literally.
pub const INSTRUCTIONS: &str = "state.prompt is the first prompt a person typed into a coding \
     session. It names several tickets, each replaced by a placeholder such as [K1] or [K2]; \
     each option except unsure is one of those tickets, with its title. Decide which ONE ticket \
     the session works on: the one the person asks to be done, not one named only for context, \
     as an earlier attempt, a duplicate, a reference or a follow-up. Choose unsure when the \
     prompt does not show one main ticket.";

/// One key the prompt names: its option word, the key and its title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCandidate {
    pub option: String,
    pub key: String,
    pub title: String,
}

/// PURE: a suggestion's option word.
pub fn option_of(link_id: i64) -> String {
    format!("k{link_id}")
}

/// PURE: the link an option names; `None` for `unsure` or anything else.
pub fn link_of(option: &str) -> Option<i64> {
    option.strip_prefix('k')?.parse().ok()
}

/// PURE: whether `key` stands in `text` as a whole token (case aside): not
/// glued to a letter or digit on either side, so `PAY-1` is not in
/// `PAY-12`.
fn names(text: &str, key: &str) -> bool {
    let hay = text.to_ascii_uppercase();
    let needle = key.to_ascii_uppercase();
    if needle.is_empty() {
        return false;
    }
    let bytes = hay.as_bytes();
    let mut from = 0;
    while let Some(i) = hay[from..].find(&needle) {
        let at = from + i;
        let end = at + needle.len();
        let before = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
        let after = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before && after {
            return true;
        }
        from = at + needle.chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// PURE: the rule's answer, before any model: `Some(option)` when it
/// decides (one key; the branch names exactly one of them), `Some(unsure)`
/// when nothing may be pre-selected (no key, or more than [`MAX_KEYS`]: a
/// pasted list), `None` when the model may be asked.
pub fn rule(keys: &[KeyCandidate], branch: Option<&str>) -> Option<String> {
    match keys.len() {
        0 => return Some(UNSURE.into()),
        1 => return Some(keys[0].option.clone()),
        n if n > MAX_KEYS => return Some(UNSURE.into()),
        _ => {}
    }
    if let Some(b) = branch {
        let named: Vec<&KeyCandidate> = keys.iter().filter(|k| names(b, &k.key)).collect();
        if named.len() == 1 {
            return Some(named[0].option.clone());
        }
    }
    None
}

/// PURE: the placeholder of the `i`-th key (from 0).
fn placeholder(i: usize) -> String {
    format!("[K{}]", i + 1)
}

/// PURE: `prompt` with every key replaced by its placeholder (longest key
/// first, so `PAY-12` is never half of a `PAY-1` replacement), cut to
/// [`PROMPT_CHARS`] characters.
pub fn masked(prompt: &str, keys: &[KeyCandidate]) -> String {
    let mut order: Vec<usize> = (0..keys.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(keys[i].key.len()));
    let mut out = prompt.to_string();
    for i in order {
        let key = keys[i].key.to_ascii_uppercase();
        if key.is_empty() {
            continue;
        }
        let mut next = String::with_capacity(out.len());
        let upper = out.to_ascii_uppercase();
        let bytes = upper.as_bytes();
        let mut from = 0;
        let mut copied = 0;
        while let Some(j) = upper[from..].find(&key) {
            let at = from + j;
            let end = at + key.len();
            let before = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
            let after = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
            if before && after && at >= copied {
                next.push_str(&out[copied..at]);
                next.push_str(&placeholder(i));
                copied = end;
            }
            from = at + key.chars().next().map_or(1, char::len_utf8);
        }
        next.push_str(&out[copied..]);
        out = next;
    }
    out.chars().take(PROMPT_CHARS).collect()
}

/// PURE: the question over `keys`.
pub fn question(keys: &[KeyCandidate]) -> Question {
    let mut criteria: Vec<(String, Option<Value>)> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let title = if k.title.trim().is_empty() {
                "(no title known)".to_string()
            } else {
                k.title.clone()
            };
            (
                k.option.clone(),
                Some(Value::String(format!(
                    "The session's main ticket is {}: {title}",
                    placeholder(i)
                ))),
            )
        })
        .collect();
    criteria.push((
        UNSURE.into(),
        Some(Value::String(
            "The prompt does not show one main ticket.".into(),
        )),
    ));
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: criteria.into_iter().collect(),
    }
}

/// PURE: the request for `prompt` over `keys`.
pub fn request(prompt: &str, keys: &[KeyCandidate]) -> JevRequest {
    JevRequest {
        state: json!({ "prompt": masked(prompt, keys) }),
        question: question(keys),
    }
}

/// PURE: Review's words for the proposal.
pub fn reason(keys: usize) -> String {
    format!("main ticket among {keys} keys")
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

/// The session's suggested keys, one per key (the newest suggestion of a
/// key wins), in the order detection left them.
pub fn suggested_keys(s: &Store, session_id: i64) -> Result<Vec<KeyCandidate>, IpcError> {
    let mut out: Vec<KeyCandidate> = Vec::new();
    for l in s.session_work_links(session_id)? {
        if l.state != "suggested" {
            continue;
        }
        let item = match l.item_id {
            Some(id) => s.get_work_item(id)?,
            None => None,
        };
        let Some(key) = item
            .as_ref()
            .and_then(|i| i.key.clone())
            .or_else(|| l.ref_key.clone())
            .filter(|k| !k.trim().is_empty())
        else {
            continue;
        };
        if out.iter().any(|k| k.key.eq_ignore_ascii_case(&key)) {
            continue;
        }
        out.push(KeyCandidate {
            option: option_of(l.id),
            key,
            title: item.map(|i| i.title).unwrap_or_default(),
        });
    }
    Ok(out)
}

struct Planned {
    org_id: Option<i64>,
    request: JevRequest,
    options: Vec<String>,
}

fn plan(s: &Store, session_id: i64, now: i64) -> Result<Option<Planned>, IpcError> {
    let Some(row) = s.get_session_by_id(session_id)? else {
        return Ok(None);
    };
    if crate::service::operator::is_operator_session(s, &row.host_alias, &row.tmux_name) {
        return Ok(None);
    }
    let Ok(mode) = gate_at(s, Feature::MainTicket, row.org_id, now) else {
        return Ok(None);
    };
    // A confirmed primary is the person's answer already.
    if s.current_primary_link(session_id)?.is_some() {
        return Ok(None);
    }
    let keys = suggested_keys(s, session_id)?;
    let branch = match row.worktree_id {
        Some(w) => s.get_worktree_row(w)?.and_then(|w| w.branch),
        None => None,
    };
    if rule(&keys, branch.as_deref()).is_some() {
        return Ok(None);
    }
    let Some(current) = row.claude_session_id.as_deref() else {
        return Ok(None);
    };
    let Some(prompt) = s
        .get_conversation(row.id, current)?
        .and_then(|c| c.first_prompt)
        .filter(|p| !p.trim().is_empty())
    else {
        return Ok(None);
    };
    let request = request(&prompt, &keys);
    let fp = fingerprint(&s.decision_fp_key()?, &request.redacted());
    let subject = session_id.to_string();
    let asked = s
        .decision_runs_for_subjects(Feature::MainTicket.as_str(), SUBJECT_KIND, &subject, None)?
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
        options: keys.into_iter().map(|k| k.option).collect(),
    }))
}

/// Ask which suggestion of session `session_id` is its main ticket.
/// `Some(link id)` only in assist with a usable answer naming one. Every
/// failure is a recorded fallback or nothing at all, never an error. Holds
/// the store lock only for reads, never across the call.
pub async fn ask(ctx: &DecideCtx, session_id: i64) -> Option<i64> {
    let now = ctx.now();
    let planned = {
        let s = lock(&ctx.store).ok()?;
        match plan(&s, session_id, now) {
            Ok(p) => p?,
            Err(e) => {
                tracing::warn!("[decide] main_ticket not asked: {}", e.message);
                return None;
            }
        }
    };
    let subject = session_id.to_string();
    let out = decide(
        ctx,
        DecideRequest {
            feature: Feature::MainTicket,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: subject.clone(),
            org_id: planned.org_id,
            request: planned.request,
            // Today nothing picks one of the keys.
            baseline: Some(UNSURE.into()),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    let answer = out.proposal()?;
    if !planned.options.contains(&answer.value) {
        return None;
    }
    if let (Some(id), Ok(s)) = (out.run_id, lock(&ctx.store)) {
        if let Err(e) = s.supersede_decision_runs(
            Feature::MainTicket.as_str(),
            SUBJECT_KIND,
            &subject,
            Mode::Assist.as_str(),
            id,
            now,
        ) {
            tracing::warn!("[decide] ignored follow-up not recorded: {}", e.message);
        }
    }
    link_of(&answer.value)
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
                    Feature::MainTicket,
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

/// A person confirmed (`accept`) or rejected the suggestion `link_id` of
/// session `session_id`. The open assist answer about the session (one
/// nobody marked yet) is marked: `confirmed` or `rejected` when it named
/// that suggestion, `corrected` to it when the person confirmed another
/// one. A rejection of another suggestion, and a shadow answer, mark
/// nothing. Returns whether a run was marked.
pub fn record_decision(
    s: &Store,
    session_id: i64,
    link_id: i64,
    accept: bool,
    now: i64,
) -> Result<bool, IpcError> {
    let subject = session_id.to_string();
    let runs =
        s.decision_runs_for_subjects(Feature::MainTicket.as_str(), SUBJECT_KIND, &subject, None)?;
    let Some(r) = runs.into_iter().find(|r| {
        r.subject_id == subject
            && r.mode == Mode::Assist.as_str()
            && r.fallback.is_none()
            && r.followup.is_none()
            && r.answer.as_deref().is_some_and(|a| a != UNSURE)
    }) else {
        return Ok(false);
    };
    let picked = option_of(link_id);
    if r.answer.as_deref() == Some(picked.as_str()) {
        let mark = if accept { "confirmed" } else { "rejected" };
        return s.set_decision_followup(r.id, mark, None, now);
    }
    if accept {
        return s.set_decision_followup(r.id, "corrected", Some(&picked), now);
    }
    Ok(false)
}
