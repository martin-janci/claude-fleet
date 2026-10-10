//! J7 `tracker_duplicate` (redesign step 6.8, test map J7 "local item vs
//! tracker item"): whether a local task a person or an agent just created
//! is the same work as an open ticket of the org's tracker. Two names for
//! one piece of work split its sessions, its links and its history.
//!
//! * **Rule first** ([`rule`]). A local title that names a tracker key the
//!   candidates hold is that ticket: detection reads the key itself, and
//!   nobody is asked. No open ticket of the same org shares a telling
//!   title word ([`super::duplicate::rank`], the K4 candidates): `none`,
//!   and nobody is asked. A subtask or an agent proposal (K4 asks about
//!   those) is never asked about.
//! * **When.** Right after a standalone local task is created (`work_link
//!   { action: create }`), off the caller's path ([`spawn_ask`]).
//! * **What is sent.** The task's title, redacted; each candidate ticket's
//!   key and title. Options are `i<item id>`, `none` and `unsure`.
//! * **Shadow** records only, with `none` (today nothing notices) as the
//!   baseline. **Assist** leaves a usable answer (at or above
//!   [`MIN_CONFIDENCE`], a ticket) on the task as its `tracker_duplicate`
//!   proposal. Review shows it on any suggestion of that task: *May
//!   duplicate PAY-88 · Proposed by Jev · N% · Change*, with *Link PAY-88
//!   instead* — a person's click, never automatic. Nothing is merged.
//! * **Asked once per input.**
//! * **What is recorded.** Subject `work_item` `<local id>`.

use super::duplicate::{option_id, overlap, rank, title_words};
use super::{
    decide, fingerprint, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode, Question,
};
use crate::ipc_error::{lock, IpcError};
use crate::store::{DecisionRunRow, Store, WorkItemRow, PROPOSAL_SUBJECT_WORK_ITEM};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

pub use super::duplicate::item_of;

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "tracker_duplicate.v1";
/// What a run is about: the local task.
pub const SUBJECT_KIND: &str = PROPOSAL_SUBJECT_WORK_ITEM;
/// No ticket is the same work.
pub const NONE_OPTION: &str = "none";
/// The model cannot tell.
pub const UNSURE: &str = "unsure";
/// Below this confidence an answer is recorded, not proposed.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// Tracker tickets updated within this many days are candidates.
pub const RECENT_DAYS: i64 = 90;

/// The instruction, read literally.
pub const INSTRUCTIONS: &str = "state.local is a task a person or an agent just created in a \
     local task list: its title and why. Each option i<id> is an open ticket of the team's \
     issue tracker, with its key and title. Decide whether one ticket is the SAME piece of work \
     as state.local, so that the two would be done twice: a ticket that is only related, \
     overlaps in part or touches the same area is not the same work. Choose none when no ticket \
     is, and unsure when the titles do not show it.";

/// PURE: a ticket as the question describes it.
fn described(i: &WorkItemRow) -> String {
    match i.key.as_deref() {
        Some(k) => format!("The tracker ticket {k}: {}", i.title),
        None => format!("The tracker ticket: {}", i.title),
    }
}

/// PURE: whether an item is a tracker's (not fleet's own list).
pub fn is_tracker_item(i: &WorkItemRow) -> bool {
    i.tracker_id.is_some() || (i.source != "local" && i.key.is_some())
}

/// PURE: whether `key` stands in `text` as a whole token (case aside).
fn names(text: &str, key: &str) -> bool {
    let words: Vec<String> = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .map(|w| w.trim_matches('-').to_ascii_uppercase())
        .collect();
    let k = key.to_ascii_uppercase();
    words.contains(&k)
}

/// PURE: the rule's answer for `local` among `tickets` (already ranked):
/// a ticket whose key the title names; `none` with no candidate; `None`
/// when the model may be asked.
pub fn rule(local: &WorkItemRow, tickets: &[WorkItemRow]) -> Option<String> {
    let named: Vec<&WorkItemRow> = tickets
        .iter()
        .filter(|t| t.key.as_deref().is_some_and(|k| names(&local.title, k)))
        .collect();
    if named.len() == 1 {
        return Some(option_id(named[0].id));
    }
    if tickets.is_empty() {
        return Some(NONE_OPTION.into());
    }
    None
}

/// PURE: the candidate tickets for `local` among `open`: tracker tickets
/// only, the K4 ranking (same org, a shared telling title word, most alike
/// first, at most ten). A ticket whose key the title names is kept first
/// even when no word is shared.
pub fn candidates(
    local: &WorkItemRow,
    open: Vec<WorkItemRow>,
    mut same_org: impl FnMut(&WorkItemRow) -> bool,
) -> Vec<WorkItemRow> {
    let tickets: Vec<WorkItemRow> = open.into_iter().filter(is_tracker_item).collect();
    let keyed: Vec<WorkItemRow> = tickets
        .iter()
        .filter(|t| t.key.as_deref().is_some_and(|k| names(&local.title, k)))
        .filter(|t| same_org(t))
        .cloned()
        .collect();
    let mut out = keyed;
    for t in rank(local, tickets, same_org) {
        if !out.iter().any(|o| o.id == t.id) {
            out.push(t);
        }
    }
    out.truncate(super::duplicate::MAX_CANDIDATES);
    out
}

/// PURE: how alike two titles are (the K4 measure), for a reader that
/// wants to show it.
pub fn alike(a: &str, b: &str) -> f64 {
    overlap(&title_words(a), &title_words(b))
}

/// PURE: the question over `tickets`.
pub fn question(tickets: &[WorkItemRow]) -> Question {
    let mut criteria: Vec<(String, Option<Value>)> = tickets
        .iter()
        .map(|i| (option_id(i.id), Some(Value::String(described(i)))))
        .collect();
    criteria.push((
        NONE_OPTION.into(),
        Some(Value::String(
            "No ticket is the same work as the local task.".into(),
        )),
    ));
    criteria.push((
        UNSURE.into(),
        Some(Value::String(
            "The titles do not show whether one ticket is the same work.".into(),
        )),
    ));
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: criteria.into_iter().collect(),
    }
}

/// PURE: the request for `local` over `tickets`.
pub fn request(local: &WorkItemRow, tickets: &[WorkItemRow]) -> JevRequest {
    let why: String = local
        .proposal_why
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(super::duplicate::WHY_CHARS)
        .collect();
    JevRequest {
        state: json!({ "local": { "title": local.title, "why": why } }),
        question: question(tickets),
    }
}

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

fn plan(s: &Store, item_id: i64, now: i64) -> Result<Option<Planned>, IpcError> {
    let Some(item) = s.get_work_item(item_id)? else {
        return Ok(None);
    };
    if is_tracker_item(&item) || item.parent_id.is_some() || item.proposal_state.is_some() {
        return Ok(None);
    }
    let org_id = s.item_org(item_id)?;
    let Ok(mode) = gate_at(s, Feature::TrackerDuplicate, org_id, now) else {
        return Ok(None);
    };
    let open = s.open_work_items_since(now - RECENT_DAYS * 86_400, super::duplicate::SCAN_LIMIT)?;
    let found = candidates(&item, open, |i| s.item_org(i.id).ok().flatten() == org_id);
    if rule(&item, &found).is_some() {
        return Ok(None);
    }
    let request = request(&item, &found);
    let fp = fingerprint(&s.decision_fp_key()?, &request.redacted());
    let subject = item_id.to_string();
    let asked = s
        .decision_runs_for_subjects(
            Feature::TrackerDuplicate.as_str(),
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
        org_id,
        request,
        ids: found.iter().map(|i| i.id).collect(),
    }))
}

/// Ask whether local task `item_id` duplicates a tracker ticket. `Some(id)`
/// only in assist with a usable answer naming a candidate ticket. Every
/// failure is a recorded fallback or nothing at all, never an error. Holds
/// the store lock only for reads, never across the call.
pub async fn ask(ctx: &DecideCtx, item_id: i64) -> Option<i64> {
    let now = ctx.now();
    let planned = {
        let s = lock(&ctx.store).ok()?;
        match plan(&s, item_id, now) {
            Ok(p) => p?,
            Err(e) => {
                tracing::warn!("[decide] tracker_duplicate not asked: {}", e.message);
                return None;
            }
        }
    };
    let subject = item_id.to_string();
    let out = decide(
        ctx,
        DecideRequest {
            feature: Feature::TrackerDuplicate,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: subject.clone(),
            org_id: planned.org_id,
            request: planned.request,
            // Today nothing notices a local task that repeats a ticket.
            baseline: Some(NONE_OPTION.into()),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    let ticket = item_of(&out.proposal()?.value)?;
    if !planned.ids.contains(&ticket) {
        return None;
    }
    if let (Some(id), Ok(s)) = (out.run_id, lock(&ctx.store)) {
        if let Err(e) = s.supersede_decision_runs(
            Feature::TrackerDuplicate.as_str(),
            SUBJECT_KIND,
            &subject,
            Mode::Assist.as_str(),
            id,
            now,
        ) {
            tracing::warn!("[decide] ignored follow-up not recorded: {}", e.message);
        }
    }
    Some(ticket)
}

/// [`ask`] off the caller's path, when the gate could let it through at
/// all: with the defaults (the feature `off`) it costs one short lock and
/// spawns nothing.
pub fn spawn_ask(store: &Arc<Mutex<Store>>, item_id: i64) {
    let open = lock(store).ok().is_some_and(|s| {
        let org = s.item_org(item_id).ok().flatten();
        gate_at(
            &s,
            Feature::TrackerDuplicate,
            org,
            crate::service::catalog::now_secs(),
        )
        .is_ok()
    });
    if !open {
        return;
    }
    let ctx = DecideCtx::jev(Arc::clone(store));
    let _ = crate::rt::try_spawn(async move {
        ask(&ctx, item_id).await;
    });
}
