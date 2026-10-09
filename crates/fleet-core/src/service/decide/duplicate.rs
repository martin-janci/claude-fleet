//! K4 `duplicate` (redesign step 6.9, part 1): whether a task an agent or
//! the planner just PROPOSED is the same work as a task that already
//! exists. The canvas board's MCTasks card: "May duplicate TASK-236 ·
//! Proposed by Jev · Merge / Keep both".
//!
//! * **When.** Right after a proposal is stored (`work_link { action:
//!   propose | propose_tree }`, a planner card that proposes a tree),
//!   once per new proposal, off the caller's path ([`spawn_ask`]). A
//!   proposal a person or a card accepts at once is never asked about.
//! * **What is sent.** The proposal's title and its `why` (agent text,
//!   through the envelope's redaction), and one Choice over at most
//!   [`MAX_CANDIDATES`] open tasks of the same org touched in the last
//!   [`RECENT_DAYS`] days that share a title word with it
//!   ([`candidates`]), each described by its key and title, plus `none`.
//!   With no such task nothing is asked.
//! * **Shadow** records the answer and does nothing else. **Assist** leaves
//!   a usable answer (at or above [`MIN_CONFIDENCE`], not `none`) on the
//!   proposal as its `duplicate` proposal (step 2.8): the proposal's card
//!   shows "May duplicate <key>", and a person merges (what hangs on the
//!   proposal moves to that task and the proposal closes as rejected,
//!   `Store::merge_proposal_into`) or keeps both (accepts it). Nothing is
//!   rejected, merged or accepted by itself.
//! * **Asked once.** A decided run about the same proposal on the same
//!   input and question version is never asked again.
//! * **Follow-up.** A person's single decision on the proposal marks the
//!   latest assist answer nobody decided ([`record_decision`]): a reject
//!   (Merge) `confirmed`, an accept (Keep both) `rejected`. A shadow
//!   answer nobody saw is never marked (D34, D37), nor is a bulk accept.

use super::Question;
use super::{decide, fingerprint, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode};
use crate::ipc_error::{lock, IpcError};
use crate::store::{DecisionRunRow, Store, WorkItemRow, PROPOSAL_SUBJECT_WORK_ITEM};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "duplicate.v1";
/// What a run is about: the proposed work item (its id as text), the
/// subject the Work view reads a task's proposals by.
pub const SUBJECT_KIND: &str = PROPOSAL_SUBJECT_WORK_ITEM;
/// The option that means "none of these is the same work".
pub const NONE_OPTION: &str = "none";
/// Below this confidence an answer is recorded, not proposed.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// The most existing tasks one question offers.
pub const MAX_CANDIDATES: usize = 10;
/// Only tasks touched this recently are offered.
pub const RECENT_DAYS: i64 = 90;
/// How many open tasks are read to rank, at most.
pub const SCAN_LIMIT: usize = 500;
/// Characters of the proposal's `why` sent, at most.
pub const WHY_CHARS: usize = 600;

/// The instruction, read literally.
pub const INSTRUCTIONS: &str = "state.proposed is a task an agent just proposed: its title and \
    the agent's reason for it. Each option except none is one task that already exists and is \
    not done, named by its key and title. Decide whether one of them is the SAME piece of work \
    as state.proposed, so that finishing either would finish the other. Choose none when no \
    option is the same work: a task that is only related, overlaps in part, or touches the same \
    area is not the same work.";

const NONE_MEANS: &str = "No existing task is the same work as the proposed one.";

/// Words too common in task titles to say anything.
const STOP: &[&str] = &[
    "and", "the", "for", "with", "from", "into", "onto", "this", "that", "add", "fix", "make",
    "use", "update", "new", "task", "when", "not", "all", "its",
];

/// PURE: an existing task's option word.
pub fn option_id(item_id: i64) -> String {
    format!("i{item_id}")
}

/// PURE: the task an option names; `None` for `none` or anything else.
pub fn item_of(option: &str) -> Option<i64> {
    option.strip_prefix('i')?.parse().ok()
}

/// PURE: a title's words that can say two tasks are alike: lower case,
/// three characters or more, not a [`STOP`] word.
pub fn title_words(title: &str) -> BTreeSet<String> {
    title
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 3 && !STOP.contains(&w.as_str()))
        .collect()
}

/// PURE: how alike two titles' words are: the shared words over the
/// shorter title's, 0 when either has none.
pub fn overlap(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let shorter = a.len().min(b.len());
    if shorter == 0 {
        return 0.0;
    }
    a.intersection(b).count() as f64 / shorter as f64
}

/// PURE: the candidates for `proposed` among `open` (newest first): not
/// itself, not its parent, the same org (`org_of`), sharing a title word,
/// most alike first (newest first among equals), at most
/// [`MAX_CANDIDATES`].
pub fn rank(
    proposed: &WorkItemRow,
    open: Vec<WorkItemRow>,
    mut same_org: impl FnMut(&WorkItemRow) -> bool,
) -> Vec<WorkItemRow> {
    let words = title_words(&proposed.title);
    let mut scored: Vec<(f64, usize, WorkItemRow)> = open
        .into_iter()
        .enumerate()
        .filter(|(_, i)| i.id != proposed.id && Some(i.id) != proposed.parent_id)
        .filter_map(|(n, i)| {
            let o = overlap(&words, &title_words(&i.title));
            (o > 0.0).then_some((o, n, i))
        })
        .filter(|(_, _, i)| same_org(i))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|(_, _, i)| i)
        .collect()
}

/// PURE: an existing task as the question describes it.
fn described(i: &WorkItemRow) -> String {
    match i.key.as_deref() {
        Some(k) => format!("The existing task {k}: {}", i.title),
        None => format!("The existing task: {}", i.title),
    }
}

/// PURE: the question over `candidates`.
pub fn question(candidates: &[WorkItemRow]) -> Question {
    let mut criteria: Vec<(String, Option<Value>)> = candidates
        .iter()
        .map(|i| (option_id(i.id), Some(Value::String(described(i)))))
        .collect();
    criteria.push((NONE_OPTION.into(), Some(Value::String(NONE_MEANS.into()))));
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: criteria.into_iter().collect(),
    }
}

/// PURE: the state: the proposal's title and the start of its `why`.
pub fn state(proposed: &WorkItemRow) -> Value {
    let why: String = proposed
        .proposal_why
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(WHY_CHARS)
        .collect();
    json!({ "proposed": { "title": proposed.title, "why": why } })
}

/// PURE: the request for `proposed` over `candidates`.
pub fn request(proposed: &WorkItemRow, candidates: &[WorkItemRow]) -> JevRequest {
    JevRequest {
        state: state(proposed),
        question: question(candidates),
    }
}

/// The open tasks a proposal could duplicate, read from the store.
pub fn candidates(
    s: &Store,
    proposed: &WorkItemRow,
    now: i64,
) -> Result<Vec<WorkItemRow>, IpcError> {
    let org = s.item_org(proposed.id)?;
    let open = s.open_work_items_since(now - RECENT_DAYS * 86_400, SCAN_LIMIT)?;
    Ok(rank(proposed, open, |i| {
        s.item_org(i.id).ok().flatten() == org
    }))
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
}

/// What to ask about proposal `item_id`, or `None` (no such open
/// proposal, the gate refuses, nothing alike, or asked already).
fn plan(s: &Store, item_id: i64, now: i64) -> Result<Option<Planned>, IpcError> {
    let Some(item) = s.get_work_item(item_id)? else {
        return Ok(None);
    };
    if item.proposal_state.as_deref() != Some("proposed") {
        return Ok(None);
    }
    let org_id = s.item_org(item_id)?;
    let Ok(mode) = gate_at(s, Feature::Duplicate, org_id, now) else {
        return Ok(None);
    };
    let found = candidates(s, &item, now)?;
    if found.is_empty() {
        return Ok(None);
    }
    let request = request(&item, &found);
    let fp_key = s.decision_fp_key()?;
    let fp = fingerprint(&fp_key, &request.redacted());
    let subject = item_id.to_string();
    let runs =
        s.decision_runs_for_subjects(Feature::Duplicate.as_str(), SUBJECT_KIND, &subject, None)?;
    if runs.iter().any(|r| {
        r.subject_id == subject
            && r.input_fp.as_deref() == Some(fp.as_str())
            && r.question_version == QUESTION_VERSION
            && r.mode == mode.as_str()
            && decided(r)
    }) {
        return Ok(None);
    }
    Ok(Some(Planned { org_id, request }))
}

/// What [`ask`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    /// Nothing was asked (gate, nothing alike, asked already).
    Nothing,
    /// Asked and recorded; nothing proposed (shadow, `none`, too unsure, a
    /// fallback).
    Recorded,
    /// In assist: the proposal now carries "may duplicate `item_id`".
    Proposed { item_id: i64 },
}

/// Ask whether proposal `item_id` duplicates an existing task. Every
/// failure is a recorded fallback or nothing at all, never an error. Holds
/// the store lock only for reads, never across the call.
pub async fn ask(ctx: &DecideCtx, item_id: i64) -> Asked {
    let now = ctx.now();
    let planned = {
        let Ok(s) = lock(&ctx.store) else {
            return Asked::Nothing;
        };
        match plan(&s, item_id, now) {
            Ok(Some(p)) => p,
            Ok(None) => return Asked::Nothing,
            Err(e) => {
                tracing::warn!("[decide] duplicate not asked: {}", e.message);
                return Asked::Nothing;
            }
        }
    };
    let subject = item_id.to_string();
    let out = decide(
        ctx,
        DecideRequest {
            feature: Feature::Duplicate,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: subject.clone(),
            org_id: planned.org_id,
            request: planned.request,
            // Today nothing flags a duplicate.
            baseline: Some(NONE_OPTION.into()),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    let Some(dup) = out.proposal().and_then(|a| item_of(&a.value)) else {
        return Asked::Recorded;
    };
    if let (Some(id), Ok(s)) = (out.run_id, lock(&ctx.store)) {
        if let Err(e) = s.supersede_decision_runs(
            Feature::Duplicate.as_str(),
            SUBJECT_KIND,
            &subject,
            Mode::Assist.as_str(),
            id,
            now,
        ) {
            tracing::warn!("[decide] ignored follow-up not recorded: {}", e.message);
        }
    }
    Asked::Proposed { item_id: dup }
}

/// [`ask`] about each of `item_ids` off the caller's path, one after the
/// other, when the gate could let it through at all: with the defaults
/// (the feature `off`) it costs one short lock and spawns nothing.
pub fn spawn_ask(store: &Arc<Mutex<Store>>, item_ids: Vec<i64>) {
    if item_ids.is_empty() {
        return;
    }
    let open = lock(store).ok().is_some_and(|s| {
        let now = crate::service::catalog::now_secs();
        item_ids.iter().any(|id| {
            let org = s.item_org(*id).ok().flatten();
            gate_at(&s, Feature::Duplicate, org, now).is_ok()
        })
    });
    if !open {
        return;
    }
    let ctx = DecideCtx::jev(Arc::clone(store));
    let _ = crate::rt::try_spawn(async move {
        for id in item_ids {
            ask(&ctx, id).await;
        }
    });
}

/// A PERSON decided proposal `item_id`: mark the latest assist answer
/// nobody decided that names a task `confirmed` (they rejected the
/// proposal: Merge) or `rejected` (they accepted it: Keep both). Returns
/// whether a run was marked. Never marks a shadow answer, nor a run that
/// already has a follow-up.
pub fn record_decision(s: &Store, item_id: i64, accept: bool, now: i64) -> Result<bool, IpcError> {
    let subject = item_id.to_string();
    let runs =
        s.decision_runs_for_subjects(Feature::Duplicate.as_str(), SUBJECT_KIND, &subject, None)?;
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
    s.set_decision_followup(
        r.id,
        if accept { "rejected" } else { "confirmed" },
        None,
        now,
    )
}
