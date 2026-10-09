//! K5 `work_placement` (redesign step 6.9, part 2): which of a person's
//! Work-view groups a new task belongs in, when no person and no rule
//! placed it (test map K5).
//!
//! * **When.** Right after a person creates a standalone task (`work_link
//!   { action: create }` with no parent), off the caller's path
//!   ([`spawn_ask`]). A subtask sits under its parent and is never asked
//!   about, nor is a task a person or a rule already placed.
//! * **What is sent.** The task's title and key, and one Choice over the
//!   groups people use ([`groups`]: the labels of people's placements, most
//!   used first, then the enabled rules' groups; at most
//!   [`MAX_GROUPS`]), plus `none` and `unsure`. With no group in use
//!   nothing is asked.
//! * **What is recorded.** Subject `work_item` `<id>`; the options are
//!   `g` + 12 hex digits of an HMAC of the group's label under the local
//!   fingerprint key ([`option_of`]), never the label itself.
//! * **Shadow** records only. **Assist** leaves a usable answer (at or
//!   above [`MIN_CONFIDENCE`], a group) on the task as its
//!   `work_placement` proposal; the Work view reads it with the label put
//!   back ([`label_proposals`]), and the task's Group line in the New layout
//!   offers *Place in X*. Jev never places a task and never writes a rule.
//! * **Asked once.** A decided run about the same task, input and question
//!   version is never asked again.
//! * **Follow-up.** A PERSON's placement of the task ([`record_place`])
//!   marks the latest assist answer nobody decided `confirmed` (the same
//!   group) or `corrected` (another one). A shadow answer nobody saw is
//!   never marked (D34, D37).

use super::{
    decide, fingerprint, gate_at, hmac_sha256_hex, DecideCtx, DecideRequest, Feature, JevRequest,
    Mode, Question,
};
use crate::ipc_error::{lock, IpcError};
use crate::store::{
    DecisionProposal, DecisionRunRow, Placement, Secret, Store, WorkRule,
    PROPOSAL_SUBJECT_WORK_ITEM,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "work_placement.v1";
/// What a run is about: the task's work item (its id as text).
pub const SUBJECT_KIND: &str = PROPOSAL_SUBJECT_WORK_ITEM;
/// The option that places the task in no group.
pub const NONE_OPTION: &str = "none";
/// The option that proposes nothing.
pub const UNSURE: &str = "unsure";
/// Below this confidence an answer is recorded, not proposed.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// The most groups one question offers.
pub const MAX_GROUPS: usize = 20;

/// The instruction, read literally.
pub const INSTRUCTIONS: &str = "state.task is a task a person just created: its title and key. \
    Each option except none and unsure is one group of a person's Work view, named by its \
    label; people put tasks about the same area, product or project in the same group. Decide \
    which group this task belongs in, using only the task's text and the group labels. Choose \
    none when the task fits no group, and unsure when the text does not show which.";

/// PURE: a group's option word under the fingerprint key: `g` and 12 hex
/// digits, so the label (a person's text) is never recorded.
pub fn option_of(fp_key: &Secret, label: &str) -> String {
    let mac = hmac_sha256_hex(
        fp_key.expose().as_bytes(),
        format!("work_placement.group\n{label}").as_bytes(),
    );
    format!("g{}", &mac[..12])
}

/// PURE: the groups people use: placement labels, most used first (then
/// by label), then the enabled rules' groups not already listed; trimmed,
/// non-empty, at most [`MAX_GROUPS`].
pub fn groups<'a>(
    placements: impl IntoIterator<Item = &'a Placement>,
    rules: &[WorkRule],
) -> Vec<String> {
    let mut count: HashMap<String, usize> = HashMap::new();
    for p in placements {
        if let Some(l) = p.group.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
            *count.entry(l.to_string()).or_default() += 1;
        }
    }
    let mut out: Vec<(String, usize)> = count.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut out: Vec<String> = out.into_iter().map(|(l, _)| l).collect();
    for r in rules.iter().filter(|r| r.enabled) {
        let l = r.group.trim();
        if !l.is_empty() && !out.iter().any(|o| o == l) {
            out.push(l.to_string());
        }
    }
    out.truncate(MAX_GROUPS);
    out
}

/// PURE: the question over `labels`, each with its option word.
pub fn question(options: &[(String, String)]) -> Question {
    let mut criteria: Vec<(String, Option<Value>)> = options
        .iter()
        .map(|(id, label)| {
            (
                id.clone(),
                Some(Value::String(format!(
                    "The task belongs in the group \"{label}\"."
                ))),
            )
        })
        .collect();
    criteria.push((
        NONE_OPTION.into(),
        Some(Value::String("The task fits none of these groups.".into())),
    ));
    criteria.push((
        UNSURE.into(),
        Some(Value::String(
            "The task's text does not show which group it belongs in.".into(),
        )),
    ));
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: criteria.into_iter().collect(),
    }
}

/// PURE: the request about a task titled `title` (key `key`).
pub fn request(title: &str, key: Option<&str>, options: &[(String, String)]) -> JevRequest {
    JevRequest {
        state: json!({ "task": { "title": title, "key": key.unwrap_or("") } }),
        question: question(options),
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
    labels: Vec<(String, String)>,
}

/// What to ask about task `item_id`, or `None` (not a standalone task,
/// placed already, no group in use, the gate refuses, or asked already).
fn plan(s: &Store, item_id: i64, now: i64) -> Result<Option<Planned>, IpcError> {
    let Some(item) = s.get_work_item(item_id)? else {
        return Ok(None);
    };
    if item.parent_id.is_some() || item.proposal_state.is_some() {
        return Ok(None);
    }
    let task_id = format!("item:{item_id}");
    let org_id = s.item_org(item_id)?;
    let Ok(mode) = gate_at(s, Feature::WorkPlacement, org_id, now) else {
        return Ok(None);
    };
    let placements = s.work_placements()?;
    if placements
        .iter()
        .any(|p| p.task_id == task_id && p.group.is_some())
    {
        return Ok(None);
    }
    let rules = s.work_rules()?;
    // A rule that places it decides: Jev is asked only where nothing does.
    let g = crate::service::work::view::Graph::load(s, &crate::service::orgs::OrgScope::All)?;
    if let Ok((t, _)) = crate::service::work::view::find_task(
        &g,
        &crate::service::orgs::OrgScope::All,
        &task_id,
        false,
    ) {
        if matches!(t.group.source.as_str(), "manual" | "rule") {
            return Ok(None);
        }
    }
    let labels = groups(&placements, &rules);
    if labels.is_empty() {
        return Ok(None);
    }
    let fp_key = s.decision_fp_key()?;
    let options: Vec<(String, String)> = labels
        .into_iter()
        .map(|l| (option_of(&fp_key, &l), l))
        .collect();
    let request = request(&item.title, item.key.as_deref(), &options);
    let fp = fingerprint(&fp_key, &request.redacted());
    let subject = item_id.to_string();
    let runs = s.decision_runs_for_subjects(
        Feature::WorkPlacement.as_str(),
        SUBJECT_KIND,
        &subject,
        None,
    )?;
    if runs.iter().any(|r| {
        r.subject_id == subject
            && r.input_fp.as_deref() == Some(fp.as_str())
            && r.question_version == QUESTION_VERSION
            && r.mode == mode.as_str()
            && decided(r)
    }) {
        return Ok(None);
    }
    Ok(Some(Planned {
        org_id,
        request,
        labels: options,
    }))
}

/// Ask which group task `item_id` belongs in. `Some(label)` only in
/// assist with a usable answer naming a group. Every failure is a recorded
/// fallback or nothing at all, never an error. Holds the store lock only
/// for reads, never across the call.
pub async fn ask(ctx: &DecideCtx, item_id: i64) -> Option<String> {
    let now = ctx.now();
    let planned = {
        let s = lock(&ctx.store).ok()?;
        match plan(&s, item_id, now) {
            Ok(p) => p?,
            Err(e) => {
                tracing::warn!("[decide] work_placement not asked: {}", e.message);
                return None;
            }
        }
    };
    let subject = item_id.to_string();
    let out = decide(
        ctx,
        DecideRequest {
            feature: Feature::WorkPlacement,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: subject.clone(),
            org_id: planned.org_id,
            request: planned.request,
            // Today a task nobody placed sits in no group of a person's.
            baseline: Some(NONE_OPTION.into()),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    let answer = out.proposal()?;
    let label = planned
        .labels
        .iter()
        .find(|(id, _)| *id == answer.value)
        .map(|(_, l)| l.clone())?;
    if let (Some(id), Ok(s)) = (out.run_id, lock(&ctx.store)) {
        if let Err(e) = s.supersede_decision_runs(
            Feature::WorkPlacement.as_str(),
            SUBJECT_KIND,
            &subject,
            Mode::Assist.as_str(),
            id,
            now,
        ) {
            tracing::warn!("[decide] ignored follow-up not recorded: {}", e.message);
        }
    }
    Some(label)
}

/// [`ask`] off the caller's path, when the gate could let it through at
/// all: with the defaults (the feature `off`) it costs one short lock and
/// spawns nothing.
pub fn spawn_ask(store: &Arc<Mutex<Store>>, item_id: i64) {
    let open = lock(store).ok().is_some_and(|s| {
        let org = s.item_org(item_id).ok().flatten();
        gate_at(
            &s,
            Feature::WorkPlacement,
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

/// The Work view's read of `work_placement` proposals: each value (a
/// group's option word) replaced by the label of a group still in use, or
/// the proposal dropped when no group answers to it. Other features'
/// proposals pass through untouched. Reads the fingerprint key only when a
/// `work_placement` proposal is there (it exists then: a run was recorded).
pub fn label_proposals(
    s: &Store,
    mut proposals: HashMap<i64, Vec<DecisionProposal>>,
    placements: &HashMap<String, Placement>,
    rules: &[WorkRule],
) -> Result<HashMap<i64, Vec<DecisionProposal>>, IpcError> {
    let feature = Feature::WorkPlacement.as_str();
    if !proposals
        .values()
        .any(|v| v.iter().any(|p| p.feature == feature))
    {
        return Ok(proposals);
    }
    let fp_key = s.decision_fp_key()?;
    let by_option: HashMap<String, String> = groups(placements.values(), rules)
        .into_iter()
        .map(|l| (option_of(&fp_key, &l), l))
        .collect();
    for v in proposals.values_mut() {
        v.retain_mut(|p| {
            if p.feature != feature {
                return true;
            }
            match by_option.get(&p.value) {
                Some(l) => {
                    p.value = l.clone();
                    true
                }
                None => false,
            }
        });
    }
    proposals.retain(|_, v| !v.is_empty());
    Ok(proposals)
}

/// A PERSON placed task `item_id` in `label`: mark the latest assist answer
/// nobody decided `confirmed` (the same group) or `corrected` (to this
/// group's option). Returns whether a run was marked. Never marks a shadow
/// answer, nor a run that already has a follow-up.
pub fn record_place(s: &Store, item_id: i64, label: &str, now: i64) -> Result<bool, IpcError> {
    let subject = item_id.to_string();
    let runs = s.decision_runs_for_subjects(
        Feature::WorkPlacement.as_str(),
        SUBJECT_KIND,
        &subject,
        None,
    )?;
    let Some(r) = runs.iter().find(|r| {
        r.subject_id == subject
            && r.mode == Mode::Assist.as_str()
            && r.fallback.is_none()
            && r.answer.as_deref().is_some_and(|a| a.starts_with('g'))
    }) else {
        return Ok(false);
    };
    if r.followup.is_some() {
        return Ok(false);
    }
    let theirs = option_of(&s.decision_fp_key()?, label.trim());
    if r.answer.as_deref() == Some(theirs.as_str()) {
        s.set_decision_followup(r.id, "confirmed", None, now)
    } else {
        s.set_decision_followup(r.id, "corrected", Some(&theirs), now)
    }
}
