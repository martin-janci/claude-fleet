//! Work graph (migration 046): work items, and the links that say which work
//! a session is doing. See `docs/superpowers/specs/2026-09-24-work-graph-design.md`
//! §0 and the migration for the model; the rules that matter here:
//!
//! * A link is anchored on the session's PARTICIPANT, never on the reusable
//!   `sessions.id`, so a move (which re-points the participant) carries it.
//! * A link names an item OR a bare key (`ref_key`): work exists before any
//!   tracker knows it.
//! * A user's decision is final: `confirmed` makes the link primary,
//!   `rejected` is sticky (later detection must not re-propose it), and only
//!   a later explicit decision changes either.
//! * Nothing here deletes history: a session's links END (with a snapshot)
//!   when its participant retires — that is the migration's trigger — and
//!   only an explicit unlink removes a live one.

use super::{now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use std::collections::HashMap;

/// Why a link exists, as far as this slice can set it. Detection sources
/// (branch, pr, url, prompt …) arrive with roadmap M4. `agent_started` is
/// a ticket start an agent made (D34): the same start as `started`, but
/// not a person's decision.
pub const WORK_LINK_SOURCES: &[&str] = &["manual", "started", "agent", "agent_started"];

/// The sources that record a PERSON's decision: `manual` (a person linked,
/// confirmed or rejected it) and `started` (a person started the session
/// for it). Every other source is fleet's or an agent's (`agent`,
/// `agent_started`). Only a person may overturn a person's rejection, and
/// only these count as a person's in usage, auto-trust and write-back.
pub const PERSON_SOURCES: &[&str] = &["manual", "started"];

/// The state signals a person's unlink holds a target against (R9u,
/// migration 070): `branch` (a branch name: the session's, or its pull
/// request's head) and `pr` (a pull request, for its closing references).
pub const WORK_UNLINK_SIGNALS: &[&str] = &["branch", "pr"];

/// The most `work_unlinks` rows one participant keeps (the newest).
pub const WORK_UNLINKS_MAX: i64 = 50;

/// Who makes a link decision. The decider, not what a caller claims,
/// decides the `source` a decision records: work-link labels must say
/// whether a person or an agent decided. The default is a person: the
/// desktop's commands are always a person's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Decider {
    /// A person: the desktop, the master token, a paired person's client.
    /// Records `manual`.
    #[default]
    Person,
    /// An agent: a per-host token (the host's own Claude) or the operator
    /// (the UX agent's client). Records `agent`.
    Agent,
}

impl Decider {
    /// The `source` a decision by this decider records.
    pub fn source(self) -> &'static str {
        match self {
            Decider::Person => "manual",
            Decider::Agent => "agent",
        }
    }

    /// The `source` a ticket start by this decider records: `started` for
    /// a person, `agent_started` for an agent — the same start, but never a
    /// person's decision (usage, auto-trust, write-back).
    pub fn start_source(self) -> &'static str {
        match self {
            Decider::Person => "started",
            Decider::Agent => "agent_started",
        }
    }
}

/// What [`Store::decide_session_work_in_tx`] did with the target's link.
enum Decided {
    /// Nothing was written: a person's decision stands as it is.
    Kept(i64),
    /// The link was written (inserted or updated).
    Wrote(i64),
}

/// What an agent's decision does to a live link a decision already settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentOver {
    /// Nothing a person decided is in the way: write the agent's decision.
    Write,
    /// A person already decided the same way: keep their decision (its
    /// source and time) — a confirm only makes it primary again.
    KeepPersons,
}

/// An AGENT deciding `new_state` over the live link `link_id` (now
/// `old_state` by `old_source`): `E_FORBIDDEN` when that would turn a
/// person's rejection ("Not this") into a link, [`AgentOver::KeepPersons`]
/// when a person already decided the same way. The one rule both decision
/// paths share (`decide_session_work`, `decide_work_link`).
pub(super) fn agent_over_decision(
    session_id: i64,
    link_id: i64,
    old_state: &str,
    old_source: &str,
    new_state: &str,
) -> Result<AgentOver, IpcError> {
    if !PERSON_SOURCES.contains(&old_source) {
        return Ok(AgentOver::Write);
    }
    if old_state == "rejected" && new_state == "confirmed" {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "a person rejected this work for session {session_id} (work link {link_id}); \
                 an agent cannot overturn a person's rejection — ask the person to link it"
            ),
        )
        .with_details(serde_json::json!({ "link_id": link_id, "reason": "rejected_by_person" })));
    }
    Ok(if old_state == new_state {
        AgentOver::KeepPersons
    } else {
        AgentOver::Write
    })
}

/// Longest accepted work key / free-form work reference.
pub const WORK_REF_MAX_CHARS: usize = 64;

/// Longest accepted local work item title.
pub const WORK_TITLE_MAX_CHARS: usize = 200;

/// A unit of work fleet knows by more than a key.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkItemRow {
    pub id: i64,
    /// `local` today; a tracker provider's name once M3 lands.
    pub source: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: Option<String>,
    /// `todo` | `in_progress` | `done` (tracker items; a local item stays
    /// `todo` until someone says otherwise).
    #[serde(default)]
    pub status_category: String,
    /// Who decided `status_category`: `None` (the sync's, or the default),
    /// `"person"` (explicit, final) or `"derived"` (stamped from a merged PR).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_set_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_set_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    // --- tracker attributes (migration 048, work graph M3); all default, so
    // an older hub's row still reads. Identity is (tracker_id, external_id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    /// Former keys (a moved or renamed issue), upper case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// The tracker's type name (Story, Bug, Epic …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Jira `issuetype.hierarchyLevel`: 1 epic, 0 standard, -1 subtask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hierarchy_level: Option<i64>,
    /// The tracker's own status name ("In Review").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_name: Option<String>,
    /// completed | not_planned | duplicate, once resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<i64>,
    /// Display names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignees: Vec<String>,
    /// The current sprint's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration: Option<String>,
    /// The tracker's own `updated`, unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_ext: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_changed_at: Option<i64>,
    /// When fleet last read it from the tracker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<i64>,
    /// Missing is not gone (C25): the tracker stopped answering for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_at: Option<i64>,
    /// not_found_or_no_permission | tracker_removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    // --- shared work context (migration 086); all default, so an older
    // hub's row still reads.
    /// manual | proposed | agent | detected (`None` reads as detected).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// The dispatched job an `agent` item mirrors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<i64>,
    /// proposed | accepted | rejected (origin `proposed` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_why: Option<String>,
    /// When a person put the item on hold (orchestration O2): never READY
    /// while set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_at: Option<i64>,
    /// Its acceptance condition lines (orchestration O3), typed by prefix:
    /// see `service::work::verify`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done_when: Vec<String>,
    /// The date the work is due, `YYYY-MM-DD` (migration 154): a person's
    /// on a native item, the tracker's (Jira `duedate`) on a ticket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<String>,
}

/// One session ↔ work link. `participant_id` is `None` once the retired
/// participant was swept; `ended_at` says the session is gone and `snap_*`
/// say what it was.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkLinkRow {
    pub id: i64,
    #[serde(default)]
    pub item_id: Option<i64>,
    #[serde(default)]
    pub ref_key: Option<String>,
    #[serde(default)]
    pub participant_id: Option<i64>,
    /// `confirmed` | `suggested` | `rejected`.
    pub state: String,
    pub source: String,
    #[serde(default)]
    pub is_primary: bool,
    pub created_at: i64,
    #[serde(default)]
    pub decided_at: Option<i64>,
    #[serde(default)]
    pub ended_at: Option<i64>,
    #[serde(default)]
    pub snap_host: Option<String>,
    #[serde(default)]
    pub snap_tmux: Option<String>,
    #[serde(default)]
    pub snap_name: Option<String>,
    #[serde(default)]
    pub snap_project_id: Option<i64>,
    #[serde(default)]
    pub snap_worktree: Option<String>,
    #[serde(default)]
    pub snap_branch: Option<String>,
    #[serde(default)]
    pub snap_pr_url: Option<String>,
    /// JSON array of every Claude conversation id the session ran.
    #[serde(default)]
    pub snap_claude_ids: Option<String>,
    /// `work` | `review` | `worker` (a link inherited from a parent).
    #[serde(default = "default_role")]
    pub role: String,
    /// `false` once `purge_project` removed the transcripts the link's
    /// conversations would resume from.
    #[serde(default = "default_true")]
    pub resumable: bool,
    // --- detection (migration 049, work graph M4); all default, so an older
    // hub's row still reads.
    /// The conversation the link was decided in (a suggestion: last seen
    /// in); `None` for links older than M4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    /// explicit | strong | weak; `None` for links older than M4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<String>,
    /// The resolver rule that made it (R3, R5 …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Why: what was seen, oldest first (`service::work::resolve::Evidence`
    /// objects: signal, rule, text, snippet?, at, conversation?, note?).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<serde_json::Value>,
    /// A suggestion shown pre-selected.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preselected: bool,
    /// Why a live session's link ended (`branch_changed`, `pr_changed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<String>,
    /// The link's org (work graph M5): its tracker item's, else its
    /// session's (live) or the session's org when it ended (past work).
    /// Filled by [`Store::fill_link_orgs`]; `None` = unassigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
}

fn default_role() -> String {
    "work".into()
}

fn default_true() -> bool {
    true
}

/// A live session's primary work, for the session row and the sidebar.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkSummary {
    pub link_id: i64,
    #[serde(default)]
    pub item_id: Option<i64>,
    /// The item's key, else the link's `ref_key`.
    #[serde(default)]
    pub key: Option<String>,
    /// The item's title (empty for a bare key).
    #[serde(default)]
    pub title: String,
    pub source: String,
    /// `tracker` | `local` | `ref` (native item status, task 4): which kind
    /// of task this is, the same vocabulary as `WorkTask.kind`. Empty for a
    /// hub older than this column.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    // --- the tracker item's status (work graph M3), absent for a bare key,
    // a local item, or a hub older than M3.
    //
    // Deliberately NOT the effective/live status (native item status task
    // 4, fix round 2): fleet-mobile's `WorkSummary.isLocal` derives "this is
    // a local item, not a ticket" from `itemId != null && statusCategory ==
    // null && url == null` — the ONLY signal it has, since it predates
    // `kind` and has no other way to tell. Making a local item's status
    // non-null here would silently disable "Rename work" on every paired
    // phone, shipped builds included, which cannot be patched by anything
    // this repo ships. See `effective_status` below for the live value.
    /// todo | in_progress | done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_category: Option<String>,
    /// The item's status with the live precedence applied (design
    /// 2026-09-28 §2, fix round 2): a person's setting or a stamped `done`
    /// is final; otherwise a confirmed link whose session is presently
    /// working lifts a LOCAL item to `in_progress`; otherwise the stored
    /// value — for BOTH a tracker item and a local item alike, unlike
    /// `status_category` above. This is the field a reader wanting "the
    /// real answer" should use: the sidebar chip, the status filter, the
    /// Today view's staleness check and the handover summary all read this,
    /// not `status_category`. `None` for a bare key (no item at all), or a
    /// hub older than this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_status: Option<String>,
    /// The tracker's own status name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The tracker no longer answers for the item (C25).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unavailable: bool,
    // --- explanation (work graph M4); absent from an older hub.
    /// confirmed | suggested.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub state: String,
    /// explicit | strong | weak; absent for links older than M4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<String>,
    /// The resolver rule that made it (R3, R5 …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// A suggestion shown pre-selected.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preselected: bool,
    /// Live link suggestions the session has, still to decide.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub suggestions: u32,
    /// The link's org (work graph M5): its tracker's, else the session's.
    /// `None` = unassigned. Absent from an older hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    // --- lifecycle (work graph M7); absent from an older hub.
    /// The live session was archived from the UI (collapsed into its
    /// group's Done; tmux keeps running) at this unix second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<i64>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// What to link a session to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkTarget<'a> {
    /// An existing work item.
    Item(i64),
    /// A key or free-form work reference (normalised by [`normalize_work_ref`]).
    Key(&'a str),
    /// A key linked as a bare reference, never resolved to an item (work
    /// graph M5): what a per-host token gets when it names a key another
    /// org's tracker owns — exactly what an unknown key gives it, so the
    /// answer says nothing about the other org.
    Ref(&'a str),
}

/// Normalise a work key / free-form work reference.
///
/// A ticket-shaped key (`abc-123`, `ENG2-7`) is upper-cased, so the same
/// ticket typed two ways is one link; anything else ("billing migration") is
/// kept as trimmed. Refused: empty, longer than [`WORK_REF_MAX_CHARS`], or
/// containing a control character.
pub fn normalize_work_ref(raw: &str) -> Result<String, IpcError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "work key must not be empty",
        ));
    }
    if t.chars().count() > WORK_REF_MAX_CHARS {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("work key longer than {WORK_REF_MAX_CHARS} characters"),
        ));
    }
    if t.chars().any(char::is_control) {
        return Err(IpcError::new(
            codes::E_INVALID,
            "work key must not contain control characters",
        ));
    }
    Ok(canonical_key(t))
}

/// The one spelling of a reference, as items and links store it: a ticket
/// key upper-cased (`ABC-12`), a GitHub `owner/repo#n` lower-cased, anything
/// else (`asana:<gid>`, free text) as trimmed.
pub fn canonical_key(raw: &str) -> String {
    let t = raw.trim();
    if is_ticket_key(t) {
        t.to_ascii_uppercase()
    } else if github_ref(t).is_some() {
        t.to_ascii_lowercase()
    } else {
        t.to_string()
    }
}

/// `owner/repo#n` → `(owner/repo, n)`, both names `[A-Za-z0-9_.-]`; a
/// GitHub Enterprise Server issue's `host/owner/repo#n` (work graph M11.4)
/// → `(host/owner/repo, n)`, the host a DNS name
/// ([`super::trackers::ghes_host_ok`], lower case, no port).
pub fn github_ref(s: &str) -> Option<(&str, u64)> {
    let (repo, n) = s.rsplit_once('#')?;
    split_github_repo(repo)?;
    if n.is_empty() || n.len() > 9 {
        return None;
    }
    Some((repo, n.parse().ok()?))
}

/// A GitHub reference's repository part → `(enterprise host, owner/repo)`:
/// `owner/repo` is github.com's (`None`), `host/owner/repo` an enterprise
/// instance's. `None` when it is neither.
pub fn split_github_repo(repo: &str) -> Option<(Option<&str>, &str)> {
    let name_ok = |x: &str| {
        !x.is_empty()
            && !x.starts_with(['.', '-'])
            && x.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    };
    let parts: Vec<&str> = repo.split('/').collect();
    match parts.as_slice() {
        [o, r] if name_ok(o) && name_ok(r) => Some((None, repo)),
        [h, o, r]
            if super::trackers::ghes_host_ok(&h.to_ascii_lowercase())
                && name_ok(o)
                && name_ok(r) =>
        {
            Some((Some(h), &repo[h.len() + 1..]))
        }
        _ => None,
    }
}

/// `PREFIX-123`: a letter, then 1–9 of `[A-Za-z0-9_]`, a dash, 1–7 digits —
/// the shape the frontend's `extractWorkKey` recognises.
fn is_ticket_key(s: &str) -> bool {
    let Some((prefix, num)) = s.split_once('-') else {
        return false;
    };
    let mut chars = prefix.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    first_ok
        && (2..=10).contains(&prefix.len())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && (1..=7).contains(&num.len())
        && num.chars().all(|c| c.is_ascii_digit())
}

const LINK_COLUMNS: &str = "id, item_id, ref_key, participant_id, state, source, is_primary, \
     created_at, decided_at, ended_at, snap_host, snap_tmux, snap_name, snap_project_id, \
     snap_worktree, snap_branch, snap_pr_url, snap_claude_ids, role, resumable, \
     claude_session_id, strength, rule, evidence, preselected, end_reason, snap_org_id";

/// Columns of [`LINK_COLUMNS`] (a join's `l.` prefix is added by callers).
pub(super) const LINK_COLUMN_COUNT: usize = 27;

/// [`LINK_COLUMNS`] with every column prefixed by `alias.`.
pub(super) fn link_columns_prefixed(alias: &str) -> String {
    LINK_COLUMNS
        .split(", ")
        .map(|c| format!("{alias}.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn map_link(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorkLinkRow> {
    Ok(WorkLinkRow {
        id: r.get(0)?,
        item_id: r.get(1)?,
        ref_key: r.get(2)?,
        participant_id: r.get(3)?,
        state: r.get(4)?,
        source: r.get(5)?,
        is_primary: r.get::<_, i64>(6)? != 0,
        created_at: r.get(7)?,
        decided_at: r.get(8)?,
        ended_at: r.get(9)?,
        snap_host: r.get(10)?,
        snap_tmux: r.get(11)?,
        snap_name: r.get(12)?,
        snap_project_id: r.get(13)?,
        snap_worktree: r.get(14)?,
        snap_branch: r.get(15)?,
        snap_pr_url: r.get(16)?,
        snap_claude_ids: r.get(17)?,
        role: r.get(18)?,
        resumable: r.get::<_, i64>(19)? != 0,
        claude_session_id: r.get(20)?,
        strength: r.get(21)?,
        rule: r.get(22)?,
        evidence: r
            .get::<_, Option<String>>(23)?
            .and_then(|e| serde_json::from_str(&e).ok())
            .unwrap_or_default(),
        preselected: r.get::<_, i64>(24)? != 0,
        end_reason: r.get(25)?,
        // The snapshot's org; `Store::fill_link_orgs` resolves the rest.
        org_id: r.get(26)?,
    })
}

/// [`Store::work_item_by_key`]'s preference among items sharing a key: a
/// removed tracker's rows last, then tracker items before local ones, then
/// the oldest.
const ITEMS_BY_KEY_ORDER: &str = "ORDER BY (tracker_id IS NOT NULL \
                                    AND tracker_id NOT IN (SELECT id FROM trackers)) ASC, \
                                  (source = 'local') ASC, id ASC";

pub(super) const ITEM_COLUMNS: &str =
    "id, source, key, title, url, status_category, created_at, updated_at, \
     tracker_id, external_id, aliases, kind, hierarchy_level, status_name, resolution, parent_id, \
     assignees, iteration, updated_ext, status_changed_at, fetched_at, unavailable_at, \
     unavailable_reason, status_set_by, status_set_at, origin, project_id, notes, task_id, \
     proposal_state, proposed_by, proposal_why, held_at, done_when, due_at";

/// How many columns [`ITEM_COLUMNS`] names. A query that appends its own
/// columns after the list indexes them as `ITEM_COLUMN_COUNT + n` — never a
/// literal, because a literal silently shifts when a column is added here.
pub(super) const ITEM_COLUMN_COUNT: usize = 35;

/// A JSON array column as a list; anything unreadable is empty.
fn json_list(raw: Option<String>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub(super) fn map_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorkItemRow> {
    Ok(WorkItemRow {
        id: r.get(0)?,
        source: r.get(1)?,
        key: r.get(2)?,
        title: r.get(3)?,
        url: r.get(4)?,
        status_category: r.get(5)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
        tracker_id: r.get(8)?,
        external_id: r.get(9)?,
        aliases: json_list(r.get(10)?),
        kind: r.get(11)?,
        hierarchy_level: r.get(12)?,
        status_name: r.get(13)?,
        resolution: r.get(14)?,
        parent_id: r.get(15)?,
        assignees: json_list(r.get(16)?),
        iteration: r.get(17)?,
        updated_ext: r.get(18)?,
        status_changed_at: r.get(19)?,
        fetched_at: r.get(20)?,
        unavailable_at: r.get(21)?,
        unavailable_reason: r.get(22)?,
        status_set_by: r.get(23)?,
        status_set_at: r.get(24)?,
        origin: r.get(25)?,
        project_id: r.get(26)?,
        notes: r.get(27)?,
        task_id: r.get(28)?,
        proposal_state: r.get(29)?,
        proposed_by: r.get(30)?,
        proposal_why: r.get(31)?,
        held_at: r.get(32)?,
        done_when: json_list(r.get(33)?),
        due_at: r.get(34)?,
    })
}

impl Store {
    /// Where shipped work came from (gap plan G3.2, Today's "from Morning
    /// PR sweep"): the routine whose newest run started the participant's
    /// session, else `mission <name>` when the item is a mission's task.
    /// `None` for work a person started.
    pub fn shipped_from(
        &self,
        participant_id: Option<i64>,
        item_id: Option<i64>,
    ) -> rusqlite::Result<Option<String>> {
        use rusqlite::OptionalExtension;
        if let Some(p) = participant_id {
            let routine: Option<String> = self
                .conn
                .prepare_cached(
                    "SELECT r.name FROM participants p \
                     JOIN routine_runs rr ON rr.session_id = p.session_id \
                     JOIN routines r ON r.id = rr.routine_id \
                     WHERE p.id = ?1 ORDER BY rr.id DESC LIMIT 1",
                )?
                .query_row([p], |r| r.get(0))
                .optional()?;
            if routine.is_some() {
                return Ok(routine);
            }
        }
        let Some(item) = item_id else {
            return Ok(None);
        };
        let mission: Option<String> = self
            .conn
            .prepare_cached(
                "SELECT m.name FROM work_items i \
                 JOIN orchestration_projects m ON m.id = i.orchestration_project_id \
                 WHERE i.id = ?1",
            )?
            .query_row([item], |r| r.get(0))
            .optional()?;
        Ok(mission.map(|m| format!("mission {m}")))
    }

    /// Keyed local work items changed since `since` (unix seconds), newest
    /// first — the classification nudge's (work graph M4.6) local
    /// candidates. A keyless item cannot be named back by key, so it is not
    /// one.
    pub fn recent_local_work_items(
        &self,
        since: i64,
        limit: usize,
    ) -> Result<Vec<WorkItemRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM work_items \
             WHERE source = 'local' AND key IS NOT NULL AND updated_at >= ?1 \
             ORDER BY updated_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(rusqlite::params![since, limit as i64], map_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Open work items touched since `since`, newest first, at most `limit`:
    /// not done, not resolved, still answering, and not a proposal nobody
    /// decided (nor a rejected one). Where Jev's `duplicate` question picks
    /// its candidates from ([`crate::service::decide::duplicate`]).
    pub fn open_work_items_since(
        &self,
        since: i64,
        limit: usize,
    ) -> Result<Vec<WorkItemRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM work_items \
             WHERE status_category <> 'done' AND resolution IS NULL \
               AND unavailable_at IS NULL \
               AND (proposal_state IS NULL OR proposal_state = 'accepted') \
               AND updated_at >= ?1 \
             ORDER BY updated_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(rusqlite::params![since, limit as i64], map_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Create a local work item, or return the local item that already has
    /// `key` (updating its title when a non-empty one is given). A local item
    /// with no key is always new.
    pub fn create_local_work_item(
        &self,
        key: Option<&str>,
        title: &str,
    ) -> Result<WorkItemRow, IpcError> {
        let key = key.map(normalize_work_ref).transpose()?;
        let title = title.trim();
        if title.chars().count() > WORK_TITLE_MAX_CHARS {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("work title longer than {WORK_TITLE_MAX_CHARS} characters"),
            ));
        }
        if key.is_none() && title.is_empty() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "a work item needs a key or a title",
            ));
        }
        let now = now_unix();
        if let Some(k) = key.as_deref() {
            if let Some(existing) = self.local_work_item_by_key(k)? {
                if !title.is_empty() && title != existing.title {
                    self.conn.execute(
                        "UPDATE work_items SET title = ?1, updated_at = ?2 WHERE id = ?3",
                        rusqlite::params![title, now, existing.id],
                    )?;
                }
                return self.get_work_item(existing.id)?.ok_or_else(|| {
                    IpcError::new(codes::E_INTERNAL, "work item vanished after update")
                });
            }
        }
        self.conn.execute(
            "INSERT INTO work_items (source, key, title, origin, created_at, updated_at) \
             VALUES ('local', ?1, ?2, 'manual', ?3, ?3)",
            rusqlite::params![key, title, now],
        )?;
        let id = self.conn.last_insert_rowid();
        self.get_work_item(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work item vanished after insert"))
    }

    pub fn get_work_item(&self, id: i64) -> Result<Option<WorkItemRow>, IpcError> {
        self.conn
            .query_row(
                &format!("SELECT {ITEM_COLUMNS} FROM work_items WHERE id = ?1"),
                rusqlite::params![id],
                map_item,
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// The local item that carries `key` (normalised), if any.
    pub fn local_work_item_by_key(&self, key: &str) -> Result<Option<WorkItemRow>, IpcError> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {ITEM_COLUMNS} FROM work_items WHERE source = 'local' AND key = ?1"
                ),
                rusqlite::params![key],
                map_item,
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// Resolve a target to `(item_id, ref_key)`: an item must exist; a key
    /// that a local item carries links to that item, any other key stays a
    /// bare reference.
    fn resolve_work_target(
        &self,
        target: WorkTarget<'_>,
    ) -> Result<(Option<i64>, Option<String>), IpcError> {
        match target {
            WorkTarget::Item(id) => {
                if self.get_work_item(id)?.is_none() {
                    return Err(IpcError::new(
                        codes::E_NOTFOUND,
                        format!("work item {id} not found"),
                    ));
                }
                Ok((Some(id), None))
            }
            WorkTarget::Key(raw) => self.resolve_work_key(&normalize_work_ref(raw)?),
            WorkTarget::Ref(raw) => Ok((None, Some(normalize_work_ref(raw)?))),
        }
    }

    /// `(item_id, ref_key)` for a normalised key: a tracker item exactly one
    /// tracker has (by key or alias) wins; else a local item; else a bare key
    /// a later sync binds. The key is kept on a tracker link too, for history
    /// and for a tracker that is later removed.
    pub(super) fn resolve_work_key(
        &self,
        key: &str,
    ) -> Result<(Option<i64>, Option<String>), IpcError> {
        if let Some(item) = self.tracker_item_for_key(key)? {
            return Ok((Some(item.id), Some(key.to_string())));
        }
        match self.local_work_item_by_key(key)? {
            Some(item) => Ok((Some(item.id), None)),
            None => Ok((None, Some(key.to_string()))),
        }
    }

    /// The live participant of `session_id`, minting one if it has none
    /// (rows from before migration 045). `E_NOTFOUND` for a session row that
    /// does not exist — an identity is never minted for a dead id.
    pub(super) fn work_participant(&self, session_id: i64) -> Result<i64, IpcError> {
        let exists: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
                rusqlite::params![session_id],
                |r| r.get(0),
            )
            .map_err(IpcError::from)?;
        if !exists {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {session_id} not found"),
            ));
        }
        self.ensure_participant_for_session(session_id)
    }

    /// Write a decision about `target` for `session_id`: `confirmed` (the
    /// link becomes the session's primary work) or `rejected` (sticky, never
    /// primary). Idempotent per (session, target): re-deciding updates the
    /// one live link instead of adding another. Emits `session_updated`, so
    /// the row's `work` follows.
    ///
    /// A person's rejection is overturned only by a person: confirming a
    /// target whose live link a person rejected, with a source outside
    /// [`PERSON_SOURCES`] (an agent's `link`), is `E_FORBIDDEN` and writes
    /// nothing. Such a decision the same way a person already decided keeps
    /// the person's (a confirm only makes it primary again).
    fn decide_session_work(
        &self,
        session_id: i64,
        target: WorkTarget<'_>,
        state: &str,
        source: &str,
    ) -> Result<WorkLinkRow, IpcError> {
        self.decide_session_work_as(session_id, target, state, source, true, None)
    }

    /// [`Self::decide_session_work`], choosing whether a confirmed link
    /// takes the primary (work graph M14.1c): `take_primary: false` adds a
    /// secondary link, which still becomes primary when the session has
    /// none. Changing the primary never removes or ends another link.
    /// `expected` is the version of the session's live link to `target` the
    /// person saw (`Some(0)`: none); another change meanwhile answers
    /// `E_CONFLICT` and writes nothing. `None` checks nothing (older
    /// clients).
    fn decide_session_work_as(
        &self,
        session_id: i64,
        target: WorkTarget<'_>,
        state: &str,
        source: &str,
        take_primary: bool,
        expected: Option<i64>,
    ) -> Result<WorkLinkRow, IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        let id = match self.decide_session_work_in_tx(
            session_id,
            target,
            state,
            source,
            take_primary,
            expected,
        )? {
            Decided::Kept(id) => {
                return self
                    .get_work_link(id)?
                    .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work link vanished"))
            }
            Decided::Wrote(id) => id,
        };
        self.bump_session_for_work(session_id)?;
        tx.commit()?;
        self.emit_session(session_id)?;
        self.get_work_link(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work link vanished after write"))
    }

    /// The writes of [`Self::decide_session_work_as`], inside a transaction
    /// the caller holds (the switch's, P-2): no commit, no bump, no emit.
    fn decide_session_work_in_tx(
        &self,
        session_id: i64,
        target: WorkTarget<'_>,
        state: &str,
        source: &str,
        take_primary: bool,
        expected: Option<i64>,
    ) -> Result<Decided, IpcError> {
        if !WORK_LINK_SOURCES.contains(&source) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "unknown work link source {source:?}; one of {}",
                    WORK_LINK_SOURCES.join(", ")
                ),
            ));
        }
        let (item_id, ref_key) = self.resolve_work_target(target)?;
        let participant = self.work_participant(session_id)?;
        let now = now_unix();
        let has_primary: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_links WHERE participant_id = ?1 \
               AND ended_at IS NULL AND is_primary = 1 AND state = 'confirmed')",
            rusqlite::params![participant],
            |r| r.get(0),
        )?;
        let primary = state == "confirmed" && (take_primary || !has_primary);

        let existing: Option<(i64, String, String, i64)> = self
            .conn
            .query_row(
                "SELECT id, state, source, version FROM work_links \
                 WHERE participant_id = ?1 AND ended_at IS NULL \
                   AND ((item_id IS ?2 AND ref_key IS ?3) OR (?2 IS NOT NULL AND item_id = ?2)) \
                 ORDER BY id LIMIT 1",
                rusqlite::params![participant, item_id, ref_key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if let Some(want) = expected {
            let have = existing.as_ref().map_or(0, |(_, _, _, v)| *v);
            if have != want {
                return Err(match &existing {
                    Some((id, _, _, _)) => self.link_conflict(*id)?,
                    None => super::work_view::version_conflict(
                        "this session's link to that work",
                        0,
                        serde_json::json!({ "link_id": null, "version": 0 }),
                    ),
                });
            }
        }
        let mut keep = AgentOver::Write;
        if let Some((id, old_state, old_source, _)) = &existing {
            if !PERSON_SOURCES.contains(&source) {
                keep = agent_over_decision(session_id, *id, old_state, old_source, state)?;
            }
        }
        let existing = existing.map(|(id, _, _, _)| id);
        if let (AgentOver::KeepPersons, Some(id), false) = (keep, existing, primary) {
            // A person rejected it already (or confirmed it, and this
            // confirm takes no primary): nothing to write.
            return Ok(Decided::Kept(id));
        }
        if primary {
            self.conn.execute(
                "UPDATE work_links SET is_primary = 0 \
                 WHERE participant_id = ?1 AND ended_at IS NULL",
                rusqlite::params![participant],
            )?;
        }
        let id = match existing {
            // A person confirmed it already: it becomes primary again, and
            // stays the person's decision.
            Some(id) if keep == AgentOver::KeepPersons => {
                self.conn.execute(
                    "UPDATE work_links SET is_primary = 1 WHERE id = ?1",
                    rusqlite::params![id],
                )?;
                id
            }
            // A decision over a suggestion keeps its evidence and rule, so
            // the link can still say what proposed it. Re-linking as a
            // secondary leaves `is_primary` as it is: a secondary stays one,
            // and the primary stays the primary.
            Some(id) if state == "confirmed" && !primary => {
                self.conn.execute(
                    "UPDATE work_links SET state = ?1, source = ?2, \
                     decided_at = ?3, strength = 'explicit', preselected = 0, \
                     claude_session_id = COALESCE((SELECT claude_session_id FROM sessions \
                                                   WHERE id = ?5), claude_session_id) \
                     WHERE id = ?4",
                    rusqlite::params![state, source, now, id, session_id],
                )?;
                id
            }
            Some(id) => {
                self.conn.execute(
                    "UPDATE work_links SET state = ?1, source = ?2, is_primary = ?3, \
                     decided_at = ?4, strength = 'explicit', preselected = 0, \
                     claude_session_id = COALESCE((SELECT claude_session_id FROM sessions \
                                                   WHERE id = ?6), claude_session_id) \
                     WHERE id = ?5",
                    rusqlite::params![state, source, primary as i64, now, id, session_id],
                )?;
                id
            }
            None => {
                self.conn.execute(
                    "INSERT INTO work_links (item_id, ref_key, participant_id, state, source, \
                                             is_primary, created_at, decided_at, strength, \
                                             claude_session_id) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 'explicit', \
                             (SELECT claude_session_id FROM sessions WHERE id = ?8))",
                    rusqlite::params![
                        item_id,
                        ref_key,
                        participant,
                        state,
                        source,
                        primary as i64,
                        now,
                        session_id
                    ],
                )?;
                self.conn.last_insert_rowid()
            }
        };
        Ok(Decided::Wrote(id))
    }

    /// Say that `session_id` works on `target`; it becomes the session's
    /// primary work. `source`: `manual` (a person), `started` (a person
    /// created the session for it), `agent` (an agent declared it),
    /// `agent_started` (an agent created the session for it). An agent's
    /// link never overturns a person's rejection of the same target
    /// (`E_FORBIDDEN`).
    pub fn link_session_work(
        &self,
        session_id: i64,
        target: WorkTarget<'_>,
        source: &str,
    ) -> Result<WorkLinkRow, IpcError> {
        self.decide_session_work(session_id, target, "confirmed", source)
    }

    /// [`Self::link_session_work`] as a secondary link when `primary` is
    /// false (work graph M14.1c) — the session's primary stays where it is,
    /// unless it has none — and as a compare-and-set on the version of the
    /// session's live link to `target` when `expected` is given.
    pub fn link_session_work_as(
        &self,
        session_id: i64,
        target: WorkTarget<'_>,
        source: &str,
        primary: bool,
        expected: Option<i64>,
    ) -> Result<WorkLinkRow, IpcError> {
        self.decide_session_work_as(session_id, target, "confirmed", source, primary, expected)
    }

    /// [`Self::reject_session_work`] recorded as `decider`'s decision and as
    /// a compare-and-set (M14.1c) when `expected` is given.
    pub fn reject_session_work_as(
        &self,
        session_id: i64,
        target: WorkTarget<'_>,
        decider: Decider,
        expected: Option<i64>,
    ) -> Result<WorkLinkRow, IpcError> {
        self.decide_session_work_as(
            session_id,
            target,
            "rejected",
            decider.source(),
            true,
            expected,
        )
    }

    /// `E_CONFLICT` naming link `link_id`'s current state (work graph
    /// M14.1c). Only ever built for a link the caller was already allowed
    /// to name: the service checks visibility first.
    pub(super) fn link_conflict(&self, link_id: i64) -> Result<IpcError, IpcError> {
        let (v, state, primary, ended): (i64, String, i64, Option<i64>) = self.conn.query_row(
            "SELECT version, state, is_primary, ended_at FROM work_links WHERE id = ?1",
            rusqlite::params![link_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        Ok(super::work_view::version_conflict(
            &format!("work link {link_id}"),
            v,
            serde_json::json!({
                "link_id": link_id, "version": v, "state": state,
                "primary": primary != 0, "ended": ended.is_some(),
            }),
        ))
    }

    /// Refuse with `E_CONFLICT` when link `link_id` is no longer at
    /// `expected` (work graph M14.1c: a person decided on a state someone
    /// else changed since). `None` expects nothing (an older client). A
    /// link that is gone passes: the action itself answers `E_NOTFOUND`.
    /// The caller has checked that the link is one it may name.
    pub fn check_link_version(&self, link_id: i64, expected: Option<i64>) -> Result<(), IpcError> {
        let Some(expected) = expected else {
            return Ok(());
        };
        let have: Option<i64> = self
            .conn
            .query_row(
                "SELECT version FROM work_links WHERE id = ?1",
                rusqlite::params![link_id],
                |r| r.get(0),
            )
            .optional()?;
        match have {
            Some(v) if v != expected => Err(self.link_conflict(link_id)?),
            _ => Ok(()),
        }
    }

    /// A link's current `version` (migration 066), `None` when it is gone.
    pub fn work_link_version(&self, link_id: i64) -> Result<Option<i64>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT version FROM work_links WHERE id = ?1",
                rusqlite::params![link_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The session's current primary link (live, confirmed), if any.
    pub fn current_primary_link(&self, session_id: i64) -> Result<Option<i64>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT l.id FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 WHERE p.session_id = ?1 AND l.ended_at IS NULL AND l.is_primary = 1 \
                   AND l.state = 'confirmed' ORDER BY l.id LIMIT 1",
                rusqlite::params![session_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Make live confirmed link `link_id` the session's primary work, as a
    /// compare-and-set (work graph M14.1c): `expected_primary` names the
    /// primary the caller saw (`Some(0)`: none; `None`: no check). Another
    /// device's change meanwhile answers `E_CONFLICT` with the current
    /// primary. Setting the link that is already primary changes nothing.
    /// Every other link stays as it is (only `is_primary` moves). The link
    /// becomes `explicit`: a person chose it, so the resolver keeps it (R1).
    pub fn set_primary_work_link(
        &self,
        session_id: i64,
        link_id: i64,
        expected_primary: Option<i64>,
    ) -> Result<(), IpcError> {
        let participant = self.work_participant(session_id)?;
        let tx = self.conn.unchecked_transaction()?;
        let current: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM work_links WHERE participant_id = ?1 AND ended_at IS NULL \
                   AND is_primary = 1 AND state = 'confirmed' ORDER BY id LIMIT 1",
                rusqlite::params![participant],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(expected) = expected_primary {
            if current.unwrap_or(0) != expected {
                return Err(primary_conflict(session_id, current));
            }
        }
        let target: Option<String> = self
            .conn
            .query_row(
                "SELECT state FROM work_links WHERE id = ?1 AND participant_id = ?2 \
                   AND ended_at IS NULL",
                rusqlite::params![link_id, participant],
                |r| r.get(0),
            )
            .optional()?;
        match target.as_deref() {
            None => {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {session_id} has no live work link {link_id}"),
                ))
            }
            Some("confirmed") => {}
            Some(other) => {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!(
                        "work link {link_id} is {other}: only a confirmed link can be primary \
                         (confirm it first)"
                    ),
                ))
            }
        }
        if current == Some(link_id) {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE work_links SET is_primary = (id = ?2), \
               strength = CASE WHEN id = ?2 THEN 'explicit' ELSE strength END \
             WHERE participant_id = ?1 AND ended_at IS NULL \
               AND (is_primary = 1 OR id = ?2)",
            rusqlite::params![participant, link_id],
        )?;
        self.bump_session_for_work(session_id)?;
        tx.commit()?;
        self.emit_session(session_id)?;
        Ok(())
    }

    /// Move session `session_id` from the work of its live link `from` to
    /// `to` in one step (task → session P-2): `from` ENDS (`end_reason =
    /// 'switched'`, with the snapshot an ended link keeps, so Continue on the
    /// old task resumes this session's conversation) and `to` becomes the
    /// session's primary, as `link` would make it. A compare-and-set on the
    /// primary the caller saw (`expected_primary`; `Some(0)`: none; `None`:
    /// no check), so the link → set_primary → unlink sequence it replaces
    /// can no longer half-fail, and two devices cannot both switch. `to`
    /// already being `from`'s target is `E_INVALID`; `from` not a live
    /// confirmed link of this session is `E_NOTFOUND`. The agent's rules are
    /// [`Self::link_session_work`]'s: an agent never turns a person's
    /// rejection of `to` into a link.
    pub fn switch_session_work(
        &self,
        session_id: i64,
        from: i64,
        to: WorkTarget<'_>,
        source: &str,
        expected_primary: Option<i64>,
    ) -> Result<WorkLinkRow, IpcError> {
        let participant = self.work_participant(session_id)?;
        let (to_item, to_ref) = self.resolve_work_target(to)?;
        let tx = self.conn.unchecked_transaction()?;
        let current: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM work_links WHERE participant_id = ?1 AND ended_at IS NULL \
                   AND is_primary = 1 AND state = 'confirmed' ORDER BY id LIMIT 1",
                rusqlite::params![participant],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(expected) = expected_primary {
            if current.unwrap_or(0) != expected {
                return Err(primary_conflict(session_id, current));
            }
        }
        let old: Option<(Option<i64>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT item_id, ref_key FROM work_links WHERE id = ?1 AND participant_id = ?2 \
                   AND ended_at IS NULL AND state = 'confirmed'",
                rusqlite::params![from, participant],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((old_item, old_ref)) = old else {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {session_id} has no live confirmed work link {from}"),
            ));
        };
        let same = |a: &Option<i64>, b: &Option<i64>| a.is_some() && a == b;
        if same(&old_item, &to_item) || (old_ref.is_some() && old_ref == to_ref) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("work link {from} is already on that work: nothing to switch"),
            ));
        }
        self.end_live_link(from, session_id, "switched", now_unix())?;
        let id = match self.decide_session_work_in_tx(
            session_id,
            to,
            "confirmed",
            source,
            true,
            None,
        )? {
            Decided::Kept(id) | Decided::Wrote(id) => id,
        };
        self.bump_session_for_work(session_id)?;
        tx.commit()?;
        self.emit_session(session_id)?;
        self.get_work_link(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work link vanished after write"))
    }

    /// The sessions with a live confirmed link to `target`, other than
    /// `except` (task → session P-3: "this task is already open in …").
    /// Ids only; the caller decides which of them it may name.
    pub fn live_sessions_on_target(
        &self,
        target: WorkTarget<'_>,
        except: i64,
    ) -> Result<Vec<i64>, IpcError> {
        let (item_id, ref_key) = self.resolve_work_target(target)?;
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT p.session_id FROM work_links l \
               JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
             WHERE l.ended_at IS NULL AND l.state = 'confirmed' AND p.session_id IS NOT NULL \
               AND p.session_id != ?3 \
               AND ((?1 IS NOT NULL AND l.item_id = ?1) \
                    OR (?2 IS NOT NULL AND l.item_id IS NULL AND l.ref_key = ?2)) \
             ORDER BY p.session_id",
        )?;
        let rows = stmt.query_map(rusqlite::params![item_id, ref_key, except], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<i64>>>()?)
    }

    /// Undo a person's decision (work graph M14.1c): a live confirmed or
    /// rejected link that detection had proposed goes back to a suggestion,
    /// keeping its evidence. A link made by hand, with nothing that proposed
    /// it, is refused — remove it instead (`unlink`). A suggestion is left
    /// as it is (idempotent). An agent never undoes a person's decision
    /// (D34): undo, then confirm, would overturn a person's rejection in two
    /// steps.
    pub fn reconsider_work_link(
        &self,
        session_id: i64,
        link_id: i64,
        decider: Decider,
    ) -> Result<(), IpcError> {
        let participant = self.work_participant(session_id)?;
        let row: Option<(String, String, Option<String>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT state, source, rule, evidence FROM work_links \
                 WHERE id = ?1 AND participant_id = ?2 AND ended_at IS NULL",
                rusqlite::params![link_id, participant],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((state, source, rule, evidence)) = row else {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {session_id} has no live work link {link_id}"),
            ));
        };
        if state == "suggested" {
            return Ok(());
        }
        let proposed = rule.is_some() || evidence.as_deref().is_some_and(|e| e != "[]");
        if !proposed {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "work link {link_id} was made by hand, not proposed: remove it (unlink) \
                     instead of undoing it"
                ),
            ));
        }
        if decider == Decider::Agent && PERSON_SOURCES.contains(&source.as_str()) {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!(
                    "a person decided work link {link_id} for session {session_id}; an agent \
                     cannot undo a person's decision — ask the person"
                ),
            )
            .with_details(
                serde_json::json!({ "link_id": link_id, "reason": "decided_by_person" }),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "UPDATE work_links SET state = 'suggested', is_primary = 0, decided_at = NULL, \
               strength = CASE WHEN rule IS NULL THEN 'weak' ELSE 'strong' END, \
               preselected = 0, review_ack_at = NULL \
             WHERE id = ?1",
            rusqlite::params![link_id],
        )?;
        self.bump_session_for_work(session_id)?;
        tx.commit()?;
        self.emit_session(session_id)?;
        Ok(())
    }

    /// A person keeps a conflict on purpose (work graph M14.1c, D32): a
    /// live confirmed link to another org's task or to an unavailable
    /// ticket leaves the review inbox. Idempotent: the first ack's time
    /// stays.
    pub fn ack_work_link(&self, session_id: i64, link_id: i64) -> Result<(), IpcError> {
        let participant = self.work_participant(session_id)?;
        let n = self.conn.execute(
            "UPDATE work_links SET review_ack_at = COALESCE(review_ack_at, ?3) \
             WHERE id = ?1 AND participant_id = ?2 AND ended_at IS NULL \
               AND state = 'confirmed'",
            rusqlite::params![link_id, participant, now_unix()],
        )?;
        if n == 0 {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {session_id} has no live confirmed work link {link_id}"),
            ));
        }
        self.bump_session_for_work(session_id)?;
        self.emit_session(session_id)?;
        Ok(())
    }

    /// Say that `session_id` does NOT work on `target` (a person's "Not
    /// this"). Sticky: detection must never re-propose it; only a later
    /// explicit link by a person overrides it. Test shorthand for a
    /// person's reject: every production path names its decider
    /// ([`Self::reject_session_work_as`]).
    #[cfg(test)]
    pub fn reject_session_work(
        &self,
        session_id: i64,
        target: WorkTarget<'_>,
    ) -> Result<WorkLinkRow, IpcError> {
        self.reject_session_work_by(session_id, target, Decider::Person)
    }

    /// [`Self::reject_session_work`] recorded as `decider`'s decision.
    pub fn reject_session_work_by(
        &self,
        session_id: i64,
        target: WorkTarget<'_>,
        decider: Decider,
    ) -> Result<WorkLinkRow, IpcError> {
        self.decide_session_work(session_id, target, "rejected", decider.source())
    }

    /// Remove one live link of `session_id` (a mistaken link, not a
    /// rejection: the target may be proposed again). `false` when the link
    /// does not exist, is not this session's, or has already ended — ended
    /// links are history and are never removed here. An agent's unlink; a
    /// person's goes through [`Self::unlink_session_work_held`].
    pub fn unlink_session_work(&self, session_id: i64, link_id: i64) -> Result<bool, IpcError> {
        self.unlink_session_work_held(session_id, link_id, &[])
    }

    /// [`Self::unlink_session_work`] by a PERSON whose correction must hold
    /// against the unchanged state signal that named the link's target
    /// (R9u, D34): with the delete, in one savepoint, write one
    /// `work_unlinks` row per `(signal, value)` in `holds` (`branch` or
    /// `pr`, see [`WORK_UNLINK_SIGNALS`]) for the link's target. Detection
    /// then drops a state candidate for that target while the signal's
    /// value is the same. Nothing is written when the link is not removed.
    /// A participant keeps its newest [`WORK_UNLINKS_MAX`] rows.
    pub fn unlink_session_work_held(
        &self,
        session_id: i64,
        link_id: i64,
        holds: &[(&str, String)],
    ) -> Result<bool, IpcError> {
        if let Some((signal, _)) = holds.iter().find(|(s, _)| !WORK_UNLINK_SIGNALS.contains(s)) {
            return Err(IpcError::new(
                codes::E_INTERNAL,
                format!("unknown unlink signal {signal:?}"),
            ));
        }
        let n = self.in_savepoint("unlink_session_work", |c| -> Result<usize, IpcError> {
            let link: Option<(i64, Option<i64>, Option<String>)> = c
                .query_row(
                    "SELECT participant_id, item_id, ref_key FROM work_links \
                     WHERE id = ?1 AND ended_at IS NULL AND participant_id = \
                       (SELECT id FROM participants WHERE session_id = ?2 \
                          AND retired_at IS NULL)",
                    rusqlite::params![link_id, session_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((participant, item_id, ref_key)) = link else {
                return Ok(0);
            };
            let now = now_unix();
            for (signal, value) in holds {
                c.execute(
                    "INSERT INTO work_unlinks (participant_id, item_id, ref_key, signal, value, at) \
                     SELECT ?1, ?2, ?3, ?4, ?5, ?6 WHERE NOT EXISTS ( \
                       SELECT 1 FROM work_unlinks WHERE participant_id = ?1 \
                         AND item_id IS ?2 AND ref_key IS ?3 AND signal = ?4 AND value = ?5)",
                    rusqlite::params![participant, item_id, ref_key, signal, value, now],
                )?;
            }
            if !holds.is_empty() {
                c.execute(
                    "DELETE FROM work_unlinks WHERE participant_id = ?1 AND id NOT IN \
                       (SELECT id FROM work_unlinks WHERE participant_id = ?1 \
                         ORDER BY id DESC LIMIT ?2)",
                    rusqlite::params![participant, WORK_UNLINKS_MAX],
                )?;
            }
            Ok(c.execute(
                "DELETE FROM work_links WHERE id = ?1",
                rusqlite::params![link_id],
            )?)
        })?;
        if n > 0 {
            self.bump_session_for_work(session_id)?;
            self.emit_session(session_id)?;
        }
        Ok(n > 0)
    }

    /// A link write changes the row's `work` without touching `sessions`, so
    /// bump `row_version` by hand: the frontend's merge guard then orders the
    /// `session_updated` this emits after any older payload of the row.
    pub(super) fn bump_session_for_work(&self, session_id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE sessions SET row_version = row_version + 1 WHERE id = ?1",
            rusqlite::params![session_id],
        )?;
        Ok(())
    }

    pub fn get_work_link(&self, id: i64) -> Result<Option<WorkLinkRow>, IpcError> {
        self.conn
            .query_row(
                &format!("SELECT {LINK_COLUMNS} FROM work_links WHERE id = ?1"),
                rusqlite::params![id],
                map_link,
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// Every live link of `session_id` (confirmed and rejected), primary
    /// first, then newest decision first.
    pub fn session_work_links(&self, session_id: i64) -> Result<Vec<WorkLinkRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {LINK_COLUMNS} FROM work_links \
             WHERE ended_at IS NULL AND participant_id = \
               (SELECT id FROM participants WHERE session_id = ?1 AND retired_at IS NULL) \
             ORDER BY is_primary DESC, COALESCE(decided_at, created_at) DESC, id DESC"
        ))?;
        let rows = stmt.query_map(rusqlite::params![session_id], map_link)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Ended links to `key` (past work: its sessions are gone, their
    /// snapshots remain), newest first. Matches the link's own key or its
    /// item's key.
    pub fn ended_work_links_for_key(&self, key: &str) -> Result<Vec<WorkLinkRow>, IpcError> {
        let key = normalize_work_ref(key)?;
        let cols = LINK_COLUMNS
            .split(", ")
            .map(|c| format!("l.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {cols} FROM work_links l LEFT JOIN work_items i ON i.id = l.item_id \
             WHERE l.ended_at IS NOT NULL AND l.state = 'confirmed' \
               AND (l.ref_key = ?1 OR i.key = ?1) \
             ORDER BY l.ended_at DESC, l.id DESC"
        ))?;
        let rows = stmt.query_map(rusqlite::params![key], map_link)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Give `participant` a live confirmed link to `(item_id, ref_key)`
    /// because of something fleet did (`resumed`, `forked`, `inherited`),
    /// unless it already has a live link to that target — a confirmed one
    /// means the work is carried already, a rejected one is sticky. It
    /// becomes primary only when the participant has no primary work yet.
    /// `true` when a link was written. `session_id` is the participant's
    /// session, which a settled suggestion's timeline event goes to.
    fn carry_link(
        &self,
        session_id: i64,
        participant: i64,
        item_id: Option<i64>,
        ref_key: Option<&str>,
        source: &str,
        role: &str,
    ) -> Result<bool, IpcError> {
        // A carry is a decision fleet makes for the person: it settles a
        // live suggestion of the same target rather than sitting beside it.
        // The same target is the same item however it was spelled (by id,
        // or by its key), as `decide_session_work` matches it. The
        // suggestion's row goes, but its outcome stays as a
        // `work_suggestion_withdrawn` event, reason `carried` (D34).
        let settled: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM work_links WHERE participant_id = ?1 AND ended_at IS NULL \
                   AND state = 'suggested' \
                   AND ((item_id IS ?2 AND ref_key IS ?3) OR (?2 IS NOT NULL AND item_id = ?2)) \
                 ORDER BY id",
            )?;
            let rows = stmt.query_map(rusqlite::params![participant, item_id, ref_key], |r| {
                r.get(0)
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if !settled.is_empty() {
            let conversation: Option<String> = self
                .conn
                .query_row(
                    "SELECT claude_session_id FROM sessions WHERE id = ?1",
                    rusqlite::params![session_id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            for link_id in settled {
                self.withdraw_suggestion(
                    session_id,
                    conversation.as_deref(),
                    link_id,
                    super::work_detect::WITHDRAWN_CARRIED,
                )?;
            }
        }
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_links WHERE participant_id = ?1 \
               AND ended_at IS NULL \
               AND ((item_id IS ?2 AND ref_key IS ?3) OR (?2 IS NOT NULL AND item_id = ?2)))",
            rusqlite::params![participant, item_id, ref_key],
            |r| r.get(0),
        )?;
        if exists {
            return Ok(false);
        }
        let has_primary: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_links WHERE participant_id = ?1 \
               AND ended_at IS NULL AND is_primary = 1)",
            rusqlite::params![participant],
            |r| r.get(0),
        )?;
        let now = now_unix();
        self.conn.execute(
            "INSERT INTO work_links (item_id, ref_key, participant_id, state, source, role, \
                                     is_primary, created_at, decided_at, strength) \
             VALUES (?1, ?2, ?3, 'confirmed', ?4, ?5, ?6, ?7, ?7, 'explicit')",
            rusqlite::params![
                item_id,
                ref_key,
                participant,
                source,
                role,
                (!has_primary) as i64,
                now
            ],
        )?;
        Ok(true)
    }

    /// Resume auto-carry (review C7): `session_id` now runs Claude
    /// conversation `claude_session_id`; every ENDED confirmed link whose
    /// snapshot names that conversation is carried onto the session with
    /// source `resumed`. Called by the rebind (the one writer of a row's
    /// conversation id) inside its transaction; the caller emits the row.
    /// Returns how many links were written.
    pub(crate) fn carry_resumed_work(
        &self,
        session_id: i64,
        claude_session_id: &str,
    ) -> Result<usize, IpcError> {
        let targets: Vec<(Option<i64>, Option<String>, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT l.item_id, l.ref_key, l.role FROM work_links l, \
                        json_each(l.snap_claude_ids) j \
                 WHERE l.ended_at IS NOT NULL AND l.state = 'confirmed' \
                   AND l.snap_claude_ids IS NOT NULL AND j.value = ?1 \
                 GROUP BY l.item_id, l.ref_key ORDER BY MAX(l.ended_at) DESC",
            )?;
            let rows = stmt.query_map(rusqlite::params![claude_session_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if targets.is_empty() {
            return Ok(0);
        }
        let participant = self.ensure_participant_for_session(session_id)?;
        let mut n = 0;
        for (item_id, ref_key, role) in targets {
            if self.carry_link(
                session_id,
                participant,
                item_id,
                ref_key.as_deref(),
                "resumed",
                &role,
            )? {
                n += 1;
            }
        }
        if n > 0 {
            self.bump_session_for_work(session_id)?;
        }
        Ok(n)
    }

    /// `move_session { keep_source }` forks the session: copy the source's
    /// live confirmed links onto the target with source `forked`. Emits the
    /// target's row when anything was copied.
    pub fn copy_work_links(&self, from_session: i64, to_session: i64) -> Result<usize, IpcError> {
        let links: Vec<WorkLinkRow> = self
            .session_work_links(from_session)?
            .into_iter()
            .filter(|l| l.state == "confirmed")
            .collect();
        if links.is_empty() {
            return Ok(0);
        }
        let participant = self.work_participant(to_session)?;
        let tx = self.conn.unchecked_transaction()?;
        let mut n = 0;
        // Primary first, so the fork's primary is the source's.
        for l in &links {
            if self.carry_link(
                to_session,
                participant,
                l.item_id,
                l.ref_key.as_deref(),
                "forked",
                &l.role,
            )? {
                n += 1;
            }
        }
        if n > 0 {
            self.bump_session_for_work(to_session)?;
        }
        tx.commit()?;
        if n > 0 {
            self.emit_session(to_session)?;
        }
        Ok(n)
    }

    /// A resume started `session_id` for work `key`: link it with source
    /// `resumed` (a no-op when the rebind's carry already did). Emits the row.
    pub fn link_resumed_work(&self, session_id: i64, key: &str) -> Result<bool, IpcError> {
        let (item_id, ref_key) = self.resolve_work_target(WorkTarget::Key(key))?;
        let participant = self.work_participant(session_id)?;
        let wrote = self.carry_link(
            session_id,
            participant,
            item_id,
            ref_key.as_deref(),
            "resumed",
            "work",
        )?;
        if wrote {
            self.bump_session_for_work(session_id)?;
            self.emit_session(session_id)?;
        }
        Ok(wrote)
    }

    /// [`Self::inherit_work`] for a task worker (`dispatch_task`, a
    /// background agent's requester), emitting the worker's row when it
    /// gained a link. Not part of `set_parent_session_id`: a move also sets
    /// that column (to its source), and a move carries work its own way.
    pub fn inherit_worker_work(&self, worker: i64, requester: i64) -> Result<bool, IpcError> {
        let wrote = self.inherit_work(worker, requester, "worker")?;
        if wrote {
            self.emit_session(worker)?;
        }
        Ok(wrote)
    }

    /// A review session (`role` `review`) or a task worker (`worker`)
    /// inherits its parent's primary work, source `inherited` — only when it
    /// has no live link of its own. The caller emits the child's row.
    pub(crate) fn inherit_work(
        &self,
        child_session: i64,
        parent_session: i64,
        role: &str,
    ) -> Result<bool, IpcError> {
        let Some(primary) = self
            .session_work_links(parent_session)?
            .into_iter()
            .find(|l| l.is_primary && l.state == "confirmed")
        else {
            return Ok(false);
        };
        if self
            .session_work_links(child_session)?
            .iter()
            .any(|l| l.state != "suggested")
        {
            return Ok(false);
        }
        let participant = self.work_participant(child_session)?;
        let wrote = self.carry_link(
            child_session,
            participant,
            primary.item_id,
            primary.ref_key.as_deref(),
            "inherited",
            role,
        )?;
        if wrote {
            self.bump_session_for_work(child_session)?;
        }
        Ok(wrote)
    }

    /// Work keys whose links would lose resumable conversations when
    /// `project_id` is purged on `hosts`: ended links snapshotted there, and
    /// live links of that project's sessions there. Sorted, distinct.
    pub fn work_keys_for_purge(
        &self,
        project_id: i64,
        hosts: &[String],
    ) -> Result<Vec<String>, IpcError> {
        let hosts = serde_json::to_string(hosts)
            .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?;
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT COALESCE(i.key, l.ref_key, i.title) AS k FROM work_links l \
             LEFT JOIN work_items i ON i.id = l.item_id \
             LEFT JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
             LEFT JOIN sessions s ON s.id = p.session_id \
             WHERE l.state = 'confirmed' AND l.resumable = 1 AND ( \
               (l.ended_at IS NOT NULL AND l.snap_project_id = ?1 \
                  AND l.snap_host IN (SELECT value FROM json_each(?2))) \
               OR (l.ended_at IS NULL AND s.project_id = ?1 \
                  AND s.host_alias IN (SELECT value FROM json_each(?2)))) \
               AND k IS NOT NULL \
             ORDER BY k",
        )?;
        let rows = stmt.query_map(rusqlite::params![project_id, hosts], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// After a purge of `project_id` on `hosts`: mark the ended links
    /// snapshotted there `resumable = 0` (their transcripts are gone), so
    /// *continue* is disabled with a reason and *fresh with brief* remains.
    /// Live links end with the project's sessions right after, and are
    /// marked by the same rule once they have.
    pub fn mark_purged_work_unresumable(
        &self,
        project_id: i64,
        hosts: &[String],
    ) -> Result<usize, IpcError> {
        let hosts = serde_json::to_string(hosts)
            .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?;
        Ok(self.conn.execute(
            "UPDATE work_links SET resumable = 0 \
             WHERE ended_at IS NOT NULL AND resumable = 1 AND snap_project_id = ?1 \
               AND snap_host IN (SELECT value FROM json_each(?2))",
            rusqlite::params![project_id, hosts],
        )?)
    }

    /// The work item that carries `key` (normalised): a tracker's item when
    /// one exists, else the local one. A removed tracker's rows (kept for
    /// the links that point at them) come last: they must not shadow the
    /// same site re-added, whose fresh row carries the live title and org.
    pub fn work_item_by_key(&self, key: &str) -> Result<Option<WorkItemRow>, IpcError> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {ITEM_COLUMNS} FROM work_items WHERE key = ?1 {ITEMS_BY_KEY_ORDER} \
                     LIMIT 1"
                ),
                rusqlite::params![key],
                map_item,
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// Every work item that carries `key`, in [`Store::work_item_by_key`]'s
    /// order (its first row is that function's answer). Two trackers can
    /// hold the same key — two Jira sites, one per org, both with `PAY` — so
    /// a scoped reader that must answer with the item IT may see walks this
    /// list rather than taking the first row and refusing the caller over
    /// another org's ticket.
    pub fn work_items_by_key(&self, key: &str) -> Result<Vec<WorkItemRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM work_items WHERE key = ?1 {ITEMS_BY_KEY_ORDER}"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params![key], map_item)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Live confirmed links to `key` with the session each is on, newest
    /// decision first.
    pub fn live_work_sessions_for_key(
        &self,
        key: &str,
    ) -> Result<Vec<(WorkLinkRow, super::SessionRow)>, IpcError> {
        let key = normalize_work_ref(key)?;
        let cols = LINK_COLUMNS
            .split(", ")
            .map(|c| format!("l.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        // `item_id IN (…)`, not a join on the item's key: both arms of the
        // OR then have an index (`ref_key`, `item_id`), where the join made
        // SQLite walk every live link per call — and `work { tickets }`
        // makes one call per ticket (work graph M12.2).
        let pairs: Vec<(WorkLinkRow, i64)> = {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT {cols}, p.session_id FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 WHERE l.ended_at IS NULL AND l.state = 'confirmed' \
                   AND p.session_id IS NOT NULL \
                   AND (l.ref_key = ?1 OR l.item_id IN (SELECT id FROM work_items WHERE key = ?1)) \
                 ORDER BY COALESCE(l.decided_at, l.created_at) DESC, l.id DESC"
            ))?;
            let rows = stmt.query_map(rusqlite::params![key], |r| {
                Ok((map_link(r)?, r.get::<_, i64>(LINK_COLUMN_COUNT)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut out = Vec::with_capacity(pairs.len());
        for (link, sid) in pairs {
            if let Some(row) = self.get_session_by_id(sid)? {
                out.push((link, row));
            }
        }
        Ok(out)
    }

    /// Every work item some confirmed link points at, live or ended (the
    /// Today view's "shipped" reads a done ticket only when it is work).
    pub fn linked_work_item_ids(&self) -> Result<std::collections::HashSet<i64>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT item_id FROM work_links \
             WHERE item_id IS NOT NULL AND state = 'confirmed'",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Confirmed links that ended at or after `since` (past work, for the
    /// sidebar's past-only groups), newest first, at most `limit`.
    pub fn recent_ended_work_links(
        &self,
        since: i64,
        limit: i64,
    ) -> Result<Vec<WorkLinkRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {LINK_COLUMNS} FROM work_links \
             WHERE ended_at IS NOT NULL AND ended_at >= ?1 AND state = 'confirmed' \
             ORDER BY ended_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(rusqlite::params![since, limit], map_link)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// session id → its primary work, for every live session that has one.
    ///
    /// `kind` is unconditional (native item status task 4, fix round 1):
    /// this had the same tracker-only `status_category` `CASE` that hid a
    /// local item's status on the session row, before that was fixed. Zero
    /// non-test callers today, so nothing downstream depended on the old
    /// hiding — but a latent copy of a just-fixed bug is exactly what waits
    /// for its first caller.
    ///
    /// `status_category` itself is back to tracker-only (fix round 2): a
    /// shipped fleet-mobile build derives "this is a local item" from
    /// `status_category == null` on the identically-shaped `WorkSummary`
    /// `rows.rs` stamps on the session row, and this method feeds the same
    /// type. `effective_status` carries the live-lifted value instead — see
    /// its doc on `WorkSummary`.
    pub fn primary_work_by_session(&self) -> Result<HashMap<i64, WorkSummary>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT p.session_id, l.id, l.item_id, COALESCE(i.key, l.ref_key), \
                    COALESCE(i.title, ''), l.source, \
                    CASE WHEN i.id IS NULL THEN 'ref' \
                         WHEN i.tracker_id IS NOT NULL THEN 'tracker' \
                         ELSE 'local' END, \
                    CASE WHEN i.tracker_id IS NOT NULL THEN i.status_category END, \
                    {effective}, \
                    i.status_name, i.url, i.unavailable_at IS NOT NULL \
             FROM work_links l \
             JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
             LEFT JOIN work_items i ON i.id = l.item_id \
             WHERE l.ended_at IS NULL AND l.is_primary = 1 AND l.state = 'confirmed' \
               AND p.session_id IS NOT NULL",
            effective = crate::effective_status_sql!(),
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                WorkSummary {
                    link_id: r.get(1)?,
                    item_id: r.get(2)?,
                    key: r.get(3)?,
                    title: r.get(4)?,
                    source: r.get(5)?,
                    kind: r.get(6)?,
                    status_category: r.get(7)?,
                    effective_status: r.get(8)?,
                    status_name: r.get(9)?,
                    url: r.get(10)?,
                    unavailable: r.get::<_, Option<bool>>(11)?.unwrap_or(false),
                    state: "confirmed".into(),
                    ..Default::default()
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }
}

/// `E_CONFLICT` for a `set_primary` whose expected primary is no longer the
/// session's (work graph M14.1c). `current` is the primary the caller may
/// be told about (`None`: none, or one it does not see).
pub fn primary_conflict(session_id: i64, current: Option<i64>) -> IpcError {
    IpcError::new(
        codes::E_CONFLICT,
        format!(
            "session {session_id}'s primary work changed meanwhile (now {}); \
             reload it and decide again",
            current.map_or("none".to_string(), |c| format!("link {c}"))
        ),
    )
    .with_details(serde_json::json!({
        "session_id": session_id, "primary_link_id": current,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(s: &Store, name: &str) -> i64 {
        s.upsert_host("h").unwrap();
        s.upsert_session(name, "h", None, None, 1, 1, "running", None)
            .unwrap()
    }

    #[test]
    fn normalizes_ticket_keys_and_keeps_free_form_refs() {
        assert_eq!(normalize_work_ref(" abc-123 ").unwrap(), "ABC-123");
        assert_eq!(normalize_work_ref("ENG2-7").unwrap(), "ENG2-7");
        assert_eq!(
            normalize_work_ref("billing migration").unwrap(),
            "billing migration"
        );
        let long = "x".repeat(WORK_REF_MAX_CHARS + 1);
        for bad in ["", "   ", "a\u{7}b", long.as_str()] {
            let err = normalize_work_ref(bad).unwrap_err();
            assert_eq!(err.code, codes::E_INVALID, "{bad:?}");
        }
    }

    #[test]
    fn linking_makes_the_newest_decision_primary() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev");
        let a = s
            .link_session_work(sid, WorkTarget::Key("abc-1"), "manual")
            .unwrap();
        assert_eq!(a.ref_key.as_deref(), Some("ABC-1"));
        assert!(a.is_primary);
        assert_eq!(a.state, "confirmed");

        let b = s
            .link_session_work(sid, WorkTarget::Key("DEF-2"), "agent")
            .unwrap();
        let links = s.session_work_links(sid).unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].id, b.id, "primary first");
        assert!(links[0].is_primary);
        assert!(!links[1].is_primary);

        // Re-linking the first target updates its one link, not a second.
        let again = s
            .link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        assert_eq!(again.id, a.id);
        assert_eq!(s.session_work_links(sid).unwrap().len(), 2);
        let summary = s.primary_work_by_session().unwrap();
        assert_eq!(summary[&sid].key.as_deref(), Some("ABC-1"));
    }

    #[test]
    fn work_rev_moves_on_a_secondary_link_change_that_work_does_not_show() {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let dyn_bus: std::sync::Arc<dyn crate::events::EventBus> = bus.clone();
        let s = Store::open_with_bus_in_memory(dyn_bus).unwrap();
        let sid = seed(&s, "dev");
        let row = || s.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(row().work_rev, 0, "no live link: omitted");
        s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        let primary = row().work;
        let first = row().work_rev;
        assert_ne!(first, 0, "a live link");
        let mut seen = vec![first];
        let mut step = |what: &str, r: crate::store::SessionRow| {
            assert_eq!(r.work, primary, "{what}: the primary did not move");
            // Only a change from the value before matters to a client (the
            // same set of links reads the same again: removing what was
            // added gives back the first value).
            let rev = r.work_rev;
            assert_ne!(Some(&rev), seen.last(), "{what}: work_rev did not move");
            seen.push(rev);
        };
        // Added as a secondary; the row goes out (`emit_session` re-reads
        // it, so the event carries the new digest).
        bus.take();
        let b = s
            .link_session_work_as(sid, WorkTarget::Key("DEF-2"), "manual", false, None)
            .unwrap();
        assert_eq!(bus.names(), vec!["session:updated"]);
        step("add", row());
        // A secondary rejected (a new rejected link), then confirmed.
        let c = s
            .reject_session_work(sid, WorkTarget::Key("GHI-3"))
            .unwrap();
        step("reject", row());
        s.link_session_work_as(sid, WorkTarget::Key("GHI-3"), "manual", false, None)
            .unwrap();
        step("confirm", row());
        // Archived, then ended by the R7 path's column.
        s.conn
            .execute(
                "UPDATE work_links SET archived_at = 5 WHERE id = ?1",
                [b.id],
            )
            .unwrap();
        step("archive", row());
        s.conn
            .execute("UPDATE work_links SET ended_at = 9 WHERE id = ?1", [c.id])
            .unwrap();
        step("end", row());
        // Removed.
        bus.take();
        assert!(s.unlink_session_work(sid, b.id).unwrap());
        assert_eq!(bus.names(), vec!["session:updated"]);
        step("unlink", row());
    }

    #[test]
    fn a_rejection_is_sticky_and_never_primary() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev");
        s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        let r = s
            .reject_session_work(sid, WorkTarget::Key("abc-1"))
            .unwrap();
        assert_eq!(r.state, "rejected");
        assert!(!r.is_primary);
        assert!(s.primary_work_by_session().unwrap().is_empty());
        // Still listed, so the UI can show and undo it.
        assert_eq!(s.session_work_links(sid).unwrap()[0].state, "rejected");
        // Only an explicit link overrides it.
        let back = s
            .link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        assert_eq!(back.id, r.id);
        assert_eq!(back.state, "confirmed");
    }

    /// D34 label hygiene: an in-session agent's `link` must not turn a
    /// person's "Not this" into the session's primary work. A person (and
    /// only a person) may still correct their own rejection.
    #[test]
    fn an_agent_cannot_overturn_a_persons_rejection() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev");
        let r = s
            .reject_session_work(sid, WorkTarget::Key("ABC-1"))
            .unwrap();

        let err = s
            .link_session_work(sid, WorkTarget::Key("abc-1"), "agent")
            .unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN, "{}", err.message);
        assert!(err.message.contains("person rejected"), "{}", err.message);
        let links = s.session_work_links(sid).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(
            (
                links[0].id,
                links[0].state.as_str(),
                links[0].source.as_str()
            ),
            (r.id, "rejected", "manual"),
            "nothing written"
        );
        assert!(!links[0].is_primary);

        // An agent's link to OTHER work is unaffected, and its refusal left
        // no primary cleared behind it.
        let other = s
            .link_session_work(sid, WorkTarget::Key("DEF-2"), "agent")
            .unwrap();
        assert!(other.is_primary);
        assert!(s
            .link_session_work(sid, WorkTarget::Key("ABC-1"), "agent")
            .is_err());
        assert!(s.get_work_link(other.id).unwrap().unwrap().is_primary);

        // The person's correction still wins.
        let back = s
            .link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        assert_eq!((back.id, back.state.as_str()), (r.id, "confirmed"));
    }

    #[test]
    fn a_local_item_owns_its_key() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev");
        let item = s
            .create_local_work_item(Some("pay-7"), "Retry payments")
            .unwrap();
        assert_eq!(item.key.as_deref(), Some("PAY-7"));
        assert_eq!(
            s.create_local_work_item(Some("PAY-7"), "").unwrap().id,
            item.id,
            "one local item per key"
        );
        let renamed = s.create_local_work_item(Some("PAY-7"), "Retry v2").unwrap();
        assert_eq!(renamed.title, "Retry v2");
        // A key a local item carries links to the item.
        let l = s
            .link_session_work(sid, WorkTarget::Key("pay-7"), "manual")
            .unwrap();
        assert_eq!(l.item_id, Some(item.id));
        assert_eq!(l.ref_key, None);
        let summary = &s.primary_work_by_session().unwrap()[&sid];
        assert_eq!(summary.title, "Retry v2");
        assert_eq!(summary.key.as_deref(), Some("PAY-7"));
        // Keyless items are fine with a title; nothing at all is not.
        assert!(s.create_local_work_item(None, "Spike: search").is_ok());
        assert_eq!(
            s.create_local_work_item(None, " ").unwrap_err().code,
            codes::E_INVALID
        );
    }

    #[test]
    fn unknown_sessions_items_and_sources_are_refused() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev");
        let err = s
            .link_session_work(4242, WorkTarget::Key("ABC-1"), "manual")
            .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        assert!(
            s.participant_for_session(4242).unwrap().is_none(),
            "no identity minted for a dead id"
        );
        let err = s
            .link_session_work(sid, WorkTarget::Item(99), "manual")
            .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        let err = s
            .link_session_work(sid, WorkTarget::Key("ABC-1"), "branch")
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
    }

    #[test]
    fn a_link_follows_the_session_through_a_move() {
        let s = Store::open_in_memory().unwrap();
        let src = seed(&s, "src");
        let dst = seed(&s, "dst");
        let l = s
            .link_session_work(src, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        // What move_session's finalise does: re-point the identity, then the
        // source row is deleted.
        let p = s.participant_for_session(src).unwrap().unwrap().id;
        s.repoint_participant(p, dst).unwrap();
        s.delete_session(src).unwrap();
        let links = s.session_work_links(dst).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].id, l.id);
        assert_eq!(links[0].ended_at, None, "a move does not end the work");
        assert_eq!(
            s.primary_work_by_session().unwrap()[&dst].key.as_deref(),
            Some("ABC-1")
        );
    }

    #[test]
    fn a_killed_sessions_link_ends_with_a_snapshot_instead_of_vanishing() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev-login");
        s.set_friendly_name("h", "dev-login", Some("Fix login"))
            .unwrap();
        s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.delete_session(sid).unwrap();

        assert!(s.session_work_links(sid).unwrap().is_empty());
        assert!(s.primary_work_by_session().unwrap().is_empty());
        let past = s.ended_work_links_for_key("abc-1").unwrap();
        assert_eq!(past.len(), 1);
        let p = &past[0];
        assert!(p.ended_at.is_some());
        assert!(!p.is_primary);
        assert_eq!(p.snap_host.as_deref(), Some("h"));
        assert_eq!(p.snap_tmux.as_deref(), Some("dev-login"));
        assert_eq!(p.snap_name.as_deref(), Some("Fix login"));
    }

    #[test]
    fn recently_ended_links_are_listed_newest_first() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "a");
        let b = seed(&s, "b");
        let live = seed(&s, "live");
        for (sid, key) in [(a, "ABC-1"), (b, "DEF-2"), (live, "GHI-3")] {
            s.link_session_work(sid, WorkTarget::Key(key), "manual")
                .unwrap();
        }
        s.delete_session(a).unwrap();
        s.delete_session(b).unwrap();
        let recent = s.recent_ended_work_links(0, 10).unwrap();
        assert_eq!(recent.len(), 2, "live links are not past work");
        assert!(recent.iter().all(|l| l.ended_at.is_some()));
        assert!(s
            .recent_ended_work_links(now_unix() + 60, 10)
            .unwrap()
            .is_empty());
        assert_eq!(s.recent_ended_work_links(0, 1).unwrap().len(), 1);
    }

    #[test]
    fn unlink_removes_only_this_sessions_live_link() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "a");
        let b = seed(&s, "b");
        let la = s
            .link_session_work(a, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        assert!(!s.unlink_session_work(b, la.id).unwrap(), "not b's link");
        assert!(s.unlink_session_work(a, la.id).unwrap());
        assert!(s.session_work_links(a).unwrap().is_empty());
        assert!(!s.unlink_session_work(a, la.id).unwrap(), "already gone");
    }

    #[test]
    fn the_session_row_carries_its_primary_work_and_every_change_is_emitted() {
        let (s, bus) = super::super::test_support::store_with_recorder();
        let sid = seed(&s, "dev");
        let before = s.get_session("dev", "h").unwrap().unwrap();
        assert_eq!(before.work, None);
        bus.take();

        let item = s.create_local_work_item(Some("abc-9"), "Login").unwrap();
        let link = s
            .link_session_work(sid, WorkTarget::Key("ABC-9"), "manual")
            .unwrap();
        let row = s.get_session("dev", "h").unwrap().unwrap();
        assert_eq!(
            row.work,
            Some(WorkSummary {
                link_id: link.id,
                item_id: Some(item.id),
                key: Some("ABC-9".into()),
                title: "Login".into(),
                source: "manual".into(),
                kind: "local".into(),
                // `status_category` stays tracker-only (fix round 2, wire
                // compat with a shipped phone build's `isLocal`); the live
                // value shows through `effective_status` instead —
                // `create_local_work_item` leaves it `todo`,
                // `status_set_by` NULL, no working session.
                effective_status: Some("todo".into()),
                state: "confirmed".into(),
                strength: Some("explicit".into()),
                ..Default::default()
            })
        );
        assert!(row.row_version > before.row_version);
        assert_eq!(bus.take(), vec![format!("session:updated:{sid}")]);

        assert!(row.work_rejected.is_empty());
        // A rejection of the primary leaves the row without work, and names
        // the key so a client's own recognition does not show it either.
        s.reject_session_work(sid, WorkTarget::Item(item.id))
            .unwrap();
        let row = s.get_session("dev", "h").unwrap().unwrap();
        assert_eq!(row.work, None);
        assert_eq!(row.work_rejected, vec!["ABC-9".to_string()]);
        assert_eq!(bus.take().len(), 1);

        let bare = s
            .link_session_work(sid, WorkTarget::Key("billing"), "agent")
            .unwrap();
        let w = s.get_session("dev", "h").unwrap().unwrap().work.unwrap();
        assert_eq!((w.key.as_deref(), w.title.as_str()), (Some("billing"), ""));
        bus.take();
        assert!(s.unlink_session_work(sid, bare.id).unwrap());
        assert_eq!(s.get_session("dev", "h").unwrap().unwrap().work, None);
        assert_eq!(bus.take().len(), 1);
        // A no-op unlink emits nothing.
        assert!(!s.unlink_session_work(sid, bare.id).unwrap());
        assert!(bus.take().is_empty());
    }

    fn with_conversation(s: &Store, name: &str, claude: &str) -> i64 {
        let id = seed(s, name);
        s.rebind_conversation(id, claude, crate::store::StartSource::Startup, None, None)
            .unwrap();
        id
    }

    #[test]
    fn a_resumed_conversation_carries_its_ended_work() {
        let (s, bus) = super::super::test_support::store_with_recorder();
        let old = with_conversation(&s, "old", "c-1");
        s.link_session_work(old, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.delete_session(old).unwrap();

        let new = with_conversation(&s, "new", "c-fresh");
        assert!(
            s.session_work_links(new).unwrap().is_empty(),
            "a fresh id carries nothing"
        );
        bus.take();
        s.rebind_conversation(new, "c-1", crate::store::StartSource::Resume, None, None)
            .unwrap();
        let links = s.session_work_links(new).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(
            (
                links[0].ref_key.as_deref(),
                links[0].source.as_str(),
                links[0].is_primary
            ),
            (Some("ABC-1"), "resumed", true)
        );
        let row = s.get_session_by_id(new).unwrap().unwrap();
        assert_eq!(row.work.unwrap().key.as_deref(), Some("ABC-1"));
        assert!(bus.take().contains(&format!("session:updated:{new}")));

        // Resuming again (a /resume back) adds nothing.
        s.rebind_conversation(new, "c-2", crate::store::StartSource::Clear, None, None)
            .unwrap();
        s.rebind_conversation(new, "c-1", crate::store::StartSource::Resume, None, None)
            .unwrap();
        assert_eq!(s.session_work_links(new).unwrap().len(), 1);
    }

    #[test]
    fn a_rejection_or_other_primary_is_respected_by_the_carry() {
        let s = Store::open_in_memory().unwrap();
        let old = with_conversation(&s, "old", "c-1");
        s.link_session_work(old, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.delete_session(old).unwrap();
        let rejecting = with_conversation(&s, "rej", "c-x");
        s.reject_session_work(rejecting, WorkTarget::Key("ABC-1"))
            .unwrap();
        s.rebind_conversation(
            rejecting,
            "c-1",
            crate::store::StartSource::Resume,
            None,
            None,
        )
        .unwrap();
        let links = s.session_work_links(rejecting).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].state, "rejected", "sticky");

        let busy = with_conversation(&s, "busy", "c-y");
        s.link_session_work(busy, WorkTarget::Key("DEF-2"), "manual")
            .unwrap();
        s.rebind_conversation(busy, "c-1", crate::store::StartSource::Resume, None, None)
            .unwrap();
        let links = s.session_work_links(busy).unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].ref_key.as_deref(), Some("DEF-2"), "primary stays");
        assert!(!links[1].is_primary);
    }

    /// A tracker item is one target however it is spelled: a rejection made
    /// by item id blocks a carry that names its key, and a repoint merge
    /// does not stack a by-id link beside a by-key one.
    #[test]
    fn a_carry_and_a_repoint_match_the_item_under_either_spelling() {
        let s = Store::open_in_memory().unwrap();
        let t = s
            .add_tracker("jira", "Acme", "https://acme.atlassian.net")
            .unwrap();
        let item = super::super::test_support::tracker_item(&s, t.id, "10001", "ABC-1", "Pay");

        let rejecting = with_conversation(&s, "rej", "c-x");
        s.reject_session_work(rejecting, WorkTarget::Item(item))
            .unwrap();
        assert!(
            !s.link_resumed_work(rejecting, "ABC-1").unwrap(),
            "a carry by key is blocked by the rejection by id"
        );
        let links = s.session_work_links(rejecting).unwrap();
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].state, "rejected");
        let row = s.get_session_by_id(rejecting).unwrap().unwrap();
        assert_eq!(row.work, None);

        let src = seed(&s, "src");
        let dst = seed(&s, "dst");
        s.link_session_work(src, WorkTarget::Item(item), "manual")
            .unwrap();
        s.link_session_work(dst, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        let p = s.participant_for_session(src).unwrap().unwrap().id;
        s.repoint_participant(p, dst).unwrap();
        s.delete_session(src).unwrap();
        let links = s.session_work_links(dst).unwrap();
        assert_eq!(links.len(), 1, "one link to the item: {links:?}");
        assert_eq!(links[0].item_id, Some(item));
        assert!(links[0].is_primary);
    }

    #[test]
    fn a_fork_copies_confirmed_links_only() {
        let s = Store::open_in_memory().unwrap();
        let src = seed(&s, "src");
        let dst = seed(&s, "dst");
        s.link_session_work(src, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.reject_session_work(src, WorkTarget::Key("NOPE-1"))
            .unwrap();
        s.link_session_work(src, WorkTarget::Key("DEF-2"), "manual")
            .unwrap();
        assert_eq!(s.copy_work_links(src, dst).unwrap(), 2);
        let links = s.session_work_links(dst).unwrap();
        assert_eq!(links.len(), 2);
        assert!(links
            .iter()
            .all(|l| l.source == "forked" && l.state == "confirmed"));
        assert_eq!(
            links[0].ref_key.as_deref(),
            Some("DEF-2"),
            "the source's primary"
        );
        assert_eq!(s.copy_work_links(src, dst).unwrap(), 0, "idempotent");
        assert_eq!(
            s.session_work_links(src).unwrap().len(),
            3,
            "source untouched"
        );
    }

    #[test]
    fn reviews_and_workers_inherit_the_parents_primary_work() {
        let s = Store::open_in_memory().unwrap();
        let parent = seed(&s, "parent");
        let review = seed(&s, "review");
        let worker = seed(&s, "worker");
        let own = seed(&s, "own");
        s.link_session_work(parent, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.link_session_work(own, WorkTarget::Key("XYZ-9"), "manual")
            .unwrap();
        s.set_session_kind(review, "review", Some(parent)).unwrap();
        s.inherit_worker_work(worker, parent).unwrap();
        s.inherit_worker_work(own, parent).unwrap();
        for (sid, role) in [(review, "review"), (worker, "worker")] {
            let links = s.session_work_links(sid).unwrap();
            assert_eq!(links.len(), 1, "{role}");
            assert_eq!(
                (
                    links[0].ref_key.as_deref(),
                    links[0].source.as_str(),
                    links[0].role.as_str()
                ),
                (Some("ABC-1"), "inherited", role)
            );
            assert!(links[0].is_primary);
        }
        let own_links = s.session_work_links(own).unwrap();
        assert_eq!(own_links.len(), 1, "a session with its own work keeps it");
        assert_eq!(own_links[0].ref_key.as_deref(), Some("XYZ-9"));
        // A parent without work gives nothing.
        let orphan = seed(&s, "orphan");
        let lone = seed(&s, "lone");
        assert!(!s.inherit_worker_work(orphan, lone).unwrap());
        assert!(s.session_work_links(orphan).unwrap().is_empty());
    }

    #[test]
    fn a_purge_names_and_marks_the_work_that_loses_its_transcripts() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        s.upsert_host("g").unwrap();
        let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
        let mk = |name: &str, host: &str| {
            let id = s
                .upsert_session(name, host, None, None, 1, 1, "running", None)
                .unwrap();
            s.conn
                .execute(
                    "UPDATE sessions SET project_id = ?1 WHERE id = ?2",
                    rusqlite::params![pid, id],
                )
                .unwrap();
            id
        };
        let ended = mk("ended", "h");
        let live = mk("live", "h");
        let elsewhere = mk("elsewhere", "g");
        s.link_session_work(ended, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.link_session_work(live, WorkTarget::Key("DEF-2"), "manual")
            .unwrap();
        s.link_session_work(elsewhere, WorkTarget::Key("GHI-3"), "manual")
            .unwrap();
        s.delete_session(ended).unwrap();
        let hosts = vec!["h".to_string()];
        assert_eq!(
            s.work_keys_for_purge(pid, &hosts).unwrap(),
            vec!["ABC-1".to_string(), "DEF-2".to_string()]
        );
        s.delete_session(live).unwrap();
        assert_eq!(s.mark_purged_work_unresumable(pid, &hosts).unwrap(), 2);
        assert!(s
            .ended_work_links_for_key("ABC-1")
            .unwrap()
            .iter()
            .all(|l| !l.resumable));
        assert!(s.session_work_links(elsewhere).unwrap()[0].resumable);
        assert!(s.work_keys_for_purge(pid, &hosts).unwrap().is_empty());
    }

    #[test]
    fn a_repoint_collision_moves_the_targets_live_links_to_the_survivor() {
        let s = Store::open_in_memory().unwrap();
        let src = seed(&s, "src");
        let dst = seed(&s, "dst");
        s.link_session_work(src, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.link_session_work(dst, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.link_session_work(dst, WorkTarget::Key("DEF-2"), "manual")
            .unwrap();
        let p = s.participant_for_session(src).unwrap().unwrap().id;
        s.repoint_participant(p, dst).unwrap();
        s.delete_session(src).unwrap();
        let links = s.session_work_links(dst).unwrap();
        let keys: Vec<_> = links.iter().map(|l| l.ref_key.clone().unwrap()).collect();
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert!(keys.contains(&"ABC-1".to_string()) && keys.contains(&"DEF-2".to_string()));
        assert_eq!(links.iter().filter(|l| l.is_primary).count(), 1);
        assert_eq!(
            links[0].ref_key.as_deref(),
            Some("ABC-1"),
            "the survivor's primary"
        );
    }

    #[test]
    fn an_items_status_provenance_round_trips_and_defaults_to_none() {
        let s = Store::open_in_memory().unwrap();
        let id = s.create_local_work_item(None, "auth refactor").unwrap().id;
        let row = s.get_work_item(id).unwrap().unwrap();
        assert_eq!(row.status_category, "todo");
        assert_eq!(
            row.status_set_by, None,
            "a fresh item has no decision on it"
        );
        assert_eq!(row.status_set_at, None);

        s.conn
            .execute(
                "UPDATE work_items SET status_category = 'done', status_set_by = 'person', \
                 status_set_at = 1700 WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
        let row = s.get_work_item(id).unwrap().unwrap();
        assert_eq!(row.status_category, "done");
        assert_eq!(row.status_set_by.as_deref(), Some("person"));
        assert_eq!(row.status_set_at, Some(1700));
    }

    #[test]
    fn item_column_count_matches_the_column_list() {
        assert_eq!(
            ITEM_COLUMNS.split(',').count(),
            ITEM_COLUMN_COUNT,
            "ITEM_COLUMN_COUNT must equal the columns ITEM_COLUMNS names, or every \
             query that appends its own columns decodes the wrong index"
        );
    }
}

#[cfg(test)]
mod shipped_from_tests {
    use super::*;

    /// Gap plan G3.2: Today's shipped line says which routine or mission
    /// the work came from; a person's own work says nothing.
    #[test]
    fn shipped_work_names_its_routine_or_mission() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", None).unwrap();
        let sid = s
            .upsert_session("sweep", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let c = s.conn_ref();
        c.execute(
            "INSERT INTO routines (name, trigger, host_alias, project_id, prompt, created_at, updated_at) \
             VALUES ('Morning PR sweep', 'cron', 'h', 1, 'p', 1, 1)",
            [],
        )
        .unwrap();
        let rid = c.last_insert_rowid();
        c.execute(
            "INSERT INTO routine_runs (routine_id, trigger, state, session_id, started_at) \
             VALUES (?1, 'cron', 'done', ?2, 1)",
            rusqlite::params![rid, sid],
        )
        .unwrap();
        let pid: i64 = c
            .query_row(
                "SELECT id FROM participants WHERE session_id = ?1",
                [sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            s.shipped_from(Some(pid), None).unwrap().as_deref(),
            Some("Morning PR sweep")
        );

        let item = s.create_local_work_item(None, "Federation").unwrap();
        c.execute(
            "INSERT INTO orchestration_projects (name, goal, mode, state, created_at, updated_at) \
             VALUES ('Hub federation v2', 'g', 'manual', 'active', 1, 1)",
            [],
        )
        .unwrap();
        let mid = c.last_insert_rowid();
        c.execute(
            "UPDATE work_items SET orchestration_project_id = ?1 WHERE id = ?2",
            rusqlite::params![mid, item.id],
        )
        .unwrap();
        assert_eq!(
            s.shipped_from(None, Some(item.id)).unwrap().as_deref(),
            Some("mission Hub federation v2")
        );
        let plain = s.create_local_work_item(None, "Mine").unwrap();
        assert_eq!(s.shipped_from(None, Some(plain.id)).unwrap(), None);
    }
}
