//! The result view of a work item: report, git evidence, gate, PR and follow-ups in one answer.
//!
//! Design 2026-10-13 D1 / D6: an item's result is its newest `done` task
//! that carries a report (`Store::latest_item_result`), any role — a run,
//! a headless check or a "Finish task" wrap-up. This module joins what the
//! panel shows beside it, read on every call and never stored:
//!
//! * the **report** — what the worker SAID (`tasks.result_json`);
//! * the **evidence** — what git said when it finished (`tasks.evidence_json`);
//! * the **gate** — `gate::completion_gate` as of now, fenced by scope;
//! * the **PR** of the worker session, as the PR probe last read it;
//! * the **follow-ups** the report proposed, each with the item already
//!   made from it (`Store::followups_of`), if any.
//!
//! Trust (design §5): a per-host token reads every piece of worker-written
//! text fenced (`fence_untrusted`), as the task page's last outcome is; a
//! person reads it as is. An item or session outside the scope answers
//! exactly as an unknown id.

use super::gate::{self, Gate};
use crate::ipc_error::{lock, IpcError};
use crate::mcp::guard::fence_untrusted;
use crate::service::orgs::{self, OrgScope};
use crate::store::{
    followup_norm, Store, TaskEvidence, TaskReport, TaskRow, REPORT_ENTRY_MAX_CHARS,
    REPORT_LIST_MAX,
};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Longest report summary served, in characters.
pub const RESULT_SUMMARY_MAX_CHARS: usize = 2000;

/// The pull request of the session that produced the result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultPr {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<i64>,
    /// OPEN | CLOSED | MERGED; `None` before the PR probe recorded the PR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// passing | failing | pending; `None` without checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_decision: Option<String>,
}

/// One follow-up the report proposed, and the item made from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultFollowup {
    pub text: String,
    /// The follow-up item already created from this line; `None` until a
    /// person (or an agent) creates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
}

/// A work item's result as the panel shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskResultView {
    pub task_id: i64,
    pub item_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    /// implement | review | test | research | integrate | wrapup.
    pub role: String,
    /// When the task finished.
    pub at: i64,
    /// The done marker the report followed; the conversation matches the
    /// turn that carries the result on it. Served only for a finished task,
    /// whose marker resolves nothing any more.
    pub marker: String,
    pub report: TaskReport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<TaskEvidence>,
    pub prs: Vec<ResultPr>,
    pub gate: Gate,
    pub followups: Vec<ResultFollowup>,
}

/// `work { action: result, item_id }`: the item's result, `None` when no
/// finished task of it left a report.
pub fn result_for_item(
    store: &Mutex<Store>,
    scope: &OrgScope,
    item_id: i64,
) -> Result<Option<TaskResultView>, IpcError> {
    let s = lock(store)?;
    view_for_item(&s, scope, item_id)
}

/// `work { action: result, session_id }`: the result of the session's
/// primary work item, `None` when it has none or the item has no result.
///
/// This is the ORG half of the session fence (the read of
/// `wrapup::request`); the person half is the caller's
/// `resolve_row_person_gated(.., Reach::Read, ..)`.
pub fn result_for_session(
    store: &Mutex<Store>,
    scope: &OrgScope,
    session_id: i64,
) -> Result<Option<TaskResultView>, IpcError> {
    let s = lock(store)?;
    let mut row = s
        .get_session_by_id(session_id)?
        .ok_or_else(|| orgs::not_found("session", session_id))?;
    let visible = match scope.host() {
        Some(h) => row.host_alias == h && scope.sees_row_org_only(&row),
        None => scope.sees_row_org_only(&row),
    };
    if !visible {
        return Err(orgs::not_found("session", session_id));
    }
    // The item is read off the redacted row, so work outside the caller's
    // org answers as no work at all.
    scope.redact_row_org_only(&mut row);
    match row.work.as_ref().and_then(|w| w.item_id) {
        Some(item_id) => view_for_item(&s, scope, item_id),
        None => Ok(None),
    }
}

fn view_for_item(
    s: &Store,
    scope: &OrgScope,
    item_id: i64,
) -> Result<Option<TaskResultView>, IpcError> {
    let Some(item) = s.get_work_item(item_id)? else {
        return Err(orgs::not_found("work item", item_id));
    };
    if !super::status::item_visible(s, scope, &item)? {
        return Err(orgs::not_found("work item", item_id));
    }
    let Some(task) = s.latest_item_result(item_id)? else {
        return Ok(None);
    };
    // `result_json IS NOT NULL` but not a report fleet wrote: no result.
    let Some(report) = task.report.clone() else {
        return Ok(None);
    };
    let now = crate::service::catalog::now_secs();
    let gate = gate::for_scope(gate::completion_gate(s, &item, now)?, s, scope)?;
    let prs = result_prs(s, &task);
    let made = s.followups_of(task.id)?;
    // A follow-up is stored under the title `followup::create` made of the
    // line (cut to a title's length), so the line is matched the same way.
    let followups = report
        .followups
        .iter()
        .map(|t| {
            let as_title = followup_norm(&super::followup::cut_title(t).0);
            ResultFollowup {
                text: t.clone(),
                item_id: made
                    .iter()
                    .find(|(title, _)| followup_norm(title) == as_title)
                    .map(|(_, id)| *id),
            }
        })
        .collect();
    let mut view = TaskResultView {
        task_id: task.id,
        item_id,
        session_id: task.worker_session_id,
        role: task.role.clone().unwrap_or_else(|| "implement".into()),
        at: task.finished_at.unwrap_or(0),
        marker: crate::service::tasks::done_marker(&task.nonce),
        report,
        evidence: task.evidence.clone(),
        prs,
        gate,
        followups,
    };
    if matches!(scope, OrgScope::Host { .. }) {
        fence_view(&mut view);
    }
    Ok(Some(view))
}

/// The worker session's PR: the PR probe's row when it has one, else the
/// session's own reading. Empty when the session is gone or has no PR.
fn result_prs(s: &Store, task: &TaskRow) -> Vec<ResultPr> {
    let Some(row) = task
        .worker_session_id
        .and_then(|sid| s.get_session_by_id(sid).ok().flatten())
    else {
        return Vec::new();
    };
    let Some(url) = row.pr_url.clone() else {
        return Vec::new();
    };
    let pr = match s.pull_request_by_url(&url).ok().flatten() {
        Some(p) => ResultPr {
            url: p.url,
            number: p.number,
            state: Some(p.state),
            ci_status: p.ci_status,
            review_decision: p.review_decision,
        },
        None => ResultPr {
            url,
            number: None,
            state: None,
            ci_status: row.ci_status.clone(),
            review_decision: None,
        },
    };
    vec![pr]
}

/// Fence every piece of worker-written text for an agent reader: the
/// report's summary and lists, the follow-up lines, the commit subjects and
/// the file paths (a worker names its files), as the origin brief fences
/// them.
fn fence_view(v: &mut TaskResultView) {
    let r = &mut v.report;
    r.summary = fence_untrusted(&r.summary, "a task report", RESULT_SUMMARY_MAX_CHARS);
    for list in [
        &mut r.done,
        &mut r.decisions,
        &mut r.remaining,
        &mut r.context,
        &mut r.tests_run,
        &mut r.warnings,
        &mut r.blockers,
        &mut r.followups,
    ] {
        list.truncate(REPORT_LIST_MAX);
        for e in list.iter_mut() {
            *e = fence_untrusted(e, "a task report", REPORT_ENTRY_MAX_CHARS);
        }
    }
    if let Some(c) = r.confidence.as_mut() {
        *c = fence_untrusted(c, "a task report", REPORT_ENTRY_MAX_CHARS);
    }
    for f in &mut v.followups {
        f.text = fence_untrusted(&f.text, "a task report", REPORT_ENTRY_MAX_CHARS);
    }
    if let Some(ev) = v.evidence.as_mut() {
        for c in &mut ev.commits {
            c.subject = fence_untrusted(&c.subject, "a commit subject", REPORT_ENTRY_MAX_CHARS);
        }
        for f in &mut ev.files {
            f.path = fence_untrusted(&f.path, "a file path", REPORT_ENTRY_MAX_CHARS);
        }
    }
}

#[cfg(test)]
mod tests;
