//! LLM drafts in Control (Orbit Fleet redesign step 9.11): a finished
//! mission's release note and Today's morning brief.
//!
//! Both are the plan's "LLM for writing" engine and follow its rules:
//!
//! * **On demand only.** A draft runs when a person presses Draft or
//!   Regenerate (Refresh, for the brief). Opening Today shows the brief it
//!   drafted last, with its time ([`brief`] with `refresh: false` never
//!   runs anything).
//! * **An editable draft.** The answer is text the UI puts in a
//!   `DraftField` that names the model and the host it ran on; fleet acts on
//!   none of it.
//! * **On a host of the same org.** A release note runs on the mission's
//!   planner host; a brief covers one org's work and runs on the host of
//!   that org's most recent session.
//! * **Booked with its origin.** Every run is an `aux_usage` row,
//!   [`crate::store::AUX_ORIGIN_RELEASE_NOTE`] or
//!   [`crate::store::AUX_ORIGIN_MORNING_BRIEF`], with its cost when `claude` reported
//!   one (redesign 8.2).
//! * **Locked down and bounded.** The run is the planner's `claude -p`
//!   ([`super::planner::planner_script`]: no tools, no MCP, no hooks); its
//!   answer is redacted and capped at [`DRAFT_MAX_CHARS`]. One draft per
//!   host at a time.

use super::planner::{self, PlannerOutput};
use super::{changeable, host_sees_org, mission_id, planner_host, Deps};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::claude_print;
use crate::service::settings;
use crate::service::view_scope::ViewScope;
use crate::service::work::today::{Today, TodayGroup, TodaySession, BUCKET_IN_PROGRESS};
use crate::service::work::WorkLinkArgs;
use crate::store::{MissionRow, Store};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// The most of a draft fleet keeps, in characters.
pub const DRAFT_MAX_CHARS: usize = 4_000;

/// Items a release note or a brief lists, at most.
const ITEMS_MAX: usize = 40;

/// The release note's instruction. The mission's facts follow it.
pub const RELEASE_NOTE_PROMPT: &str = "Write a short release note for the finished mission \
described below, for the people who use what it changed. Plain text, at most 15 lines: one \
opening sentence, then one line per notable change in plain words, then any follow-up still \
open. Use only the facts below; name no person; do not invent changes. Do not use tools.";

/// The morning brief's instruction. Today's digest follows it.
pub const BRIEF_PROMPT: &str = "Write a morning brief from the work digest below, for the person \
whose day it is. Plain text, at most 10 lines: what needs them first (waiting on them), what is in \
progress, what shipped since yesterday, and what has gone stale. Use only the facts below; do \
not invent work. Do not use tools.";

/// A draft and where it came from: what `DraftField` shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    pub model: String,
    pub host_alias: String,
    /// What it was drafted from, in a few words ("12 tasks and 2 PRs").
    pub from: String,
    /// When it was drafted, unix seconds.
    pub at: i64,
    /// The answer was longer than [`DRAFT_MAX_CHARS`] and was cut.
    #[serde(default)]
    pub truncated: bool,
}

/// Today's brief: the draft, the org it covers, or none yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Brief {
    /// The brief drafted last for this caller; `None` before the first
    /// Refresh (and after a restart of the process that drafted it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<Draft>,
    /// The org whose work it covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
}

/// PURE: a draft's stored form: redacted, trimmed, cut at
/// [`DRAFT_MAX_CHARS`]. The flag says whether it was cut.
pub fn clean_draft(text: &str) -> (String, bool) {
    let redacted = crate::logging::redact(text.trim());
    let n = redacted.chars().count();
    if n <= DRAFT_MAX_CHARS {
        return (redacted.into_owned(), false);
    }
    (redacted.chars().take(DRAFT_MAX_CHARS).collect(), true)
}

/// PURE: "N things" / "1 thing".
fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// What a release note is drafted from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteFacts {
    pub name: String,
    pub goal: String,
    /// `(key or "-", title, status category)` per member, root excluded.
    pub items: Vec<(String, String, String)>,
    pub prs: Vec<String>,
}

/// PURE: the release note's prompt and its "from" words.
pub fn release_note_prompt(f: &NoteFacts) -> (String, String) {
    let mut lines = vec![format!("Mission: {}", f.name)];
    if !f.goal.trim().is_empty() {
        lines.push(format!("Goal: {}", f.goal.trim()));
    }
    lines.push("Tasks:".into());
    for (key, title, status) in f.items.iter().take(ITEMS_MAX) {
        lines.push(format!("- {key} {title} ({status})"));
    }
    if !f.prs.is_empty() {
        lines.push("Pull requests:".into());
        for p in &f.prs {
            lines.push(format!("- {p}"));
        }
    }
    let mut from = count(f.items.len(), "task", "tasks");
    if !f.prs.is_empty() {
        from.push_str(&format!(" and {}", count(f.prs.len(), "PR", "PRs")));
    }
    (
        format!("{RELEASE_NOTE_PROMPT}\n\n{}", lines.join("\n")),
        from,
    )
}

/// PURE: the groups of `today` that belong to `org`, and its shipped work,
/// as the brief's prompt and its "from" words. `None` when the org has
/// nothing today.
pub fn brief_prompt(today: &Today, org: Option<i64>) -> Option<(String, String)> {
    let groups: Vec<TodayGroup> = today
        .groups
        .iter()
        .filter_map(|g| org_part(g, org))
        .take(ITEMS_MAX)
        .collect();
    let shipped: Vec<_> = today
        .shipped
        .iter()
        .filter(|s| s.org_id == org)
        .take(ITEMS_MAX)
        .collect();
    if groups.is_empty() && shipped.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for bucket in ["waiting", BUCKET_IN_PROGRESS, "stale"] {
        let rows: Vec<&TodayGroup> = groups.iter().filter(|g| g.bucket == bucket).collect();
        if rows.is_empty() {
            continue;
        }
        lines.push(format!("{}:", bucket.replace('_', " ")));
        for g in rows {
            let key = g.key.as_deref().unwrap_or("-");
            let sessions: Vec<String> = g
                .sessions
                .iter()
                .map(|s| match s.attention.as_deref() {
                    Some(a) => format!("{} ({a})", s.name),
                    None => s.name.clone(),
                })
                .collect();
            lines.push(format!(
                "- {key} {} [{}] sessions: {}",
                g.title,
                g.status_name.as_deref().unwrap_or("-"),
                if sessions.is_empty() {
                    "none".to_string()
                } else {
                    sessions.join(", ")
                }
            ));
        }
    }
    if !shipped.is_empty() {
        lines.push("shipped:".into());
        for s in &shipped {
            lines.push(format!(
                "- {} {} ({})",
                s.key.as_deref().unwrap_or("-"),
                s.title,
                s.how
            ));
        }
    }
    let from = format!(
        "{} and {} shipped",
        count(groups.len(), "item", "items"),
        shipped.len()
    );
    Some((format!("{BRIEF_PROMPT}\n\n{}", lines.join("\n")), from))
}

/// The org a session of `g` counts for in a brief: its own, else its
/// group's. The one rule [`brief_target`] (which org, which host) and
/// [`brief_prompt`] (what the prompt carries) share, so a brief never
/// targets one org and describes another (review r15).
fn brief_org(g: &TodayGroup, s: &TodaySession) -> Option<i64> {
    s.org_id.or(g.org_id)
}

/// The part of `g` a brief for `org` covers: the group with only its
/// sessions that count for `org` (the no-work group mixes orgs), or `None`
/// when none does. A group with no session belongs to its own org.
fn org_part(g: &TodayGroup, org: Option<i64>) -> Option<TodayGroup> {
    if g.sessions.is_empty() {
        return (g.org_id == org).then(|| g.clone());
    }
    let sessions: Vec<TodaySession> = g
        .sessions
        .iter()
        .filter(|s| brief_org(g, s) == org)
        .cloned()
        .collect();
    (!sessions.is_empty()).then(|| TodayGroup {
        sessions,
        ..g.clone()
    })
}

/// PURE: the org a brief covers and the host it runs on: the org of the
/// most recently active session in today's digest, and that session's host.
/// `wanted` narrows it to one org; `host_sees(host, org)` keeps only a
/// session whose host may be sent that org's text, so a brief never runs on
/// another org's host (transition plan, risk "Org text leaves the org").
pub fn brief_target(
    today: &Today,
    wanted: Option<i64>,
    host_sees: impl Fn(&str, Option<i64>) -> bool,
) -> Option<(Option<i64>, String)> {
    today
        .groups
        .iter()
        .flat_map(|g| g.sessions.iter().map(move |s| (g, s)))
        .filter(|(g, s)| wanted.is_none() || brief_org(g, s) == wanted)
        .filter(|(g, s)| host_sees(&s.host_alias, brief_org(g, s)))
        .max_by_key(|(_, s)| s.last_activity_at)
        .map(|(g, s)| (brief_org(g, s), s.host_alias.clone()))
}

/// The hosts with a draft running now.
static RUNNING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Holds a host's slot while its draft runs.
struct HostSlot(String);

impl HostSlot {
    fn take(host: &str) -> Result<Self, IpcError> {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if !running.insert(host.to_string()) {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("a draft is already running on {host}; try again when it ends"),
            ));
        }
        Ok(Self(host.to_string()))
    }
}

impl Drop for HostSlot {
    fn drop(&mut self) {
        RUNNING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// Who asked, for the brief's cache: one brief per person (or the fleet).
fn reader_key(scope: &ViewScope) -> String {
    match scope.person {
        Some(p) => format!("person:{p}"),
        None => "fleet".into(),
    }
}

/// The brief drafted last, per reader and org.
static BRIEFS: LazyLock<Mutex<HashMap<String, Brief>>> = LazyLock::new(Default::default);

/// One booked run: the origin, where, and what it cost.
pub(super) struct Run<'a> {
    pub origin: &'static str,
    pub host: &'a str,
    pub model: &'a str,
    pub mission_id: Option<i64>,
    pub org_id: Option<i64>,
}

/// Run `prompt` on `run.host` and book it. The store is not locked across
/// the call.
pub(super) async fn run_draft(
    deps: &Deps,
    run: &Run<'_>,
    prompt: &str,
) -> Result<(String, bool), IpcError> {
    let _slot = HostSlot::take(run.host)?;
    let script = planner::planner_script(run.model, prompt);
    let out = crate::ssh::run_shell_bounded(
        deps.ssh.as_ref(),
        run.host,
        &script,
        Duration::from_secs(10),
        Duration::from_secs(planner::PLANNER_HOST_TIMEOUT_SECS + 20),
    )
    .await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = match planner::parse_planner_output(&stdout) {
        PlannerOutput::Ran(ran) => {
            let (answer, usage) = planner::planner_answer(ran);
            book(deps, run, usage.as_ref());
            if claude_print::run_signed_out(usage.as_ref(), &answer, &out.stderr) {
                return Err(claude_print::signed_out_error(run.host, None));
            }
            if usage.as_ref().is_some_and(|u| u.is_error) {
                return Err(IpcError::new(
                    codes::E_CLAUDE_CLI,
                    "the draft failed: claude answered with an error",
                ));
            }
            answer
        }
        PlannerOutput::NoClaude => {
            return Err(IpcError::new(
                codes::E_CLAUDE_CLI,
                format!("claude is not on {}'s login PATH", run.host),
            ))
        }
        PlannerOutput::Nothing if claude_print::run_signed_out(None, "", &out.stderr) => {
            return Err(claude_print::signed_out_error(run.host, None))
        }
        PlannerOutput::Nothing => {
            return Err(IpcError::new(
                codes::E_CLAUDE_CLI,
                format!(
                    "the draft on {} gave no answer: {}",
                    run.host,
                    crate::service::work::summary::last_error_line(&out.stderr)
                ),
            ))
        }
    };
    if text.trim().is_empty() {
        return Err(IpcError::new(
            codes::E_CLAUDE_CLI,
            "the draft came back empty",
        ));
    }
    Ok(clean_draft(&text))
}

/// Book a draft run in `aux_usage`. A failed write is logged, never the
/// draft's error.
fn book(deps: &Deps, run: &Run<'_>, usage: Option<&crate::service::claude_print::Envelope>) {
    let row = crate::store::NewAuxUsage {
        origin: run.origin,
        host_alias: run.host.to_string(),
        model: run.model.to_string(),
        mission_id: run.mission_id,
        org_id: run.org_id,
        claude_session_id: None,
        input_tokens: usage.and_then(|u| u.input_tokens),
        output_tokens: usage.and_then(|u| u.output_tokens),
        cost_micros: usage.and_then(|u| u.cost_microusd).unwrap_or(0),
        at: crate::store::now_unix(),
    };
    if let Err(e) = lock(&deps.store).and_then(|s| s.insert_aux_usage(&row)) {
        tracing::warn!(origin = run.origin, error = %e.message, "[drafts] cost not booked");
    }
}

/// What a release note reads from the store.
fn note_facts(s: &Store, m: &MissionRow) -> Result<NoteFacts, IpcError> {
    let mut items = Vec::new();
    let mut prs: Vec<String> = Vec::new();
    for i in s.mission_items(m.id)? {
        if Some(i.id) == m.root_item_id {
            continue;
        }
        for t in s.tasks_for_item(i.id)? {
            if let Some(url) = t
                .worker_session_id
                .and_then(|w| s.get_session_by_id(w).ok().flatten())
                .and_then(|w| w.pr_url)
            {
                if !prs.contains(&url) {
                    prs.push(url);
                }
            }
        }
        items.push((
            i.key.clone().unwrap_or_else(|| "-".into()),
            i.title.clone(),
            i.status_category.clone(),
        ));
    }
    Ok(NoteFacts {
        name: m.name.clone(),
        goal: m.goal.clone(),
        items,
        prs,
    })
}

/// `work_link { action: mission_release_note, mission_id }`: draft the
/// release note of a completed mission. Its owner's or an org admin's.
pub async fn release_note(
    args: &WorkLinkArgs,
    deps: &Deps,
    scope: &ViewScope,
) -> Result<Draft, IpcError> {
    let id = mission_id(args)?;
    let (m, host, model, prompt, from) = {
        let s = lock(&deps.store)?;
        // Scope first: another org's mission is E_NOTFOUND, whatever the
        // setting says.
        let m = changeable(&s, scope, id)?;
        settings::require_writing_help(&s, settings::WORK_DRAFT_RELEASE_NOTES)?;
        if m.state != "completed" {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "{} is {}; a release note is for a completed mission",
                    m.name, m.state
                ),
            ));
        }
        let host = planner_host(&s, &m, None)?;
        let model = settings::get_string_for(&s, settings::WORK_SUMMARY_MODEL, m.org_id);
        let (prompt, from) = release_note_prompt(&note_facts(&s, &m)?);
        (m, host, model, prompt, from)
    };
    let run = Run {
        origin: crate::store::AUX_ORIGIN_RELEASE_NOTE,
        host: &host,
        model: &model,
        mission_id: Some(m.id),
        org_id: m.org_id,
    };
    let (text, truncated) = run_draft(deps, &run, &prompt).await?;
    Ok(Draft {
        text,
        model,
        host_alias: host,
        from,
        at: crate::store::now_unix(),
        truncated,
    })
}

/// `work_link { action: today_brief, refresh?, org_id?, since? }`: Today's
/// morning brief. Without `refresh` it answers the brief drafted last (or
/// none) and runs nothing; with it, it drafts a new one from today's digest
/// under the caller's view and keeps it.
pub async fn brief(args: &WorkLinkArgs, deps: &Deps, scope: &ViewScope) -> Result<Brief, IpcError> {
    let key = reader_key(scope);
    if !args.refresh.unwrap_or(false) {
        let briefs = BRIEFS.lock().unwrap_or_else(|e| e.into_inner());
        return Ok(briefs.get(&key).cloned().unwrap_or_default());
    }
    let today = crate::service::work::today::today(&deps.store, args.since, scope)?;
    let target = {
        let s = lock(&deps.store)?;
        brief_target(&today, args.org_id, |host, org| {
            host_sees_org(&s, host, org).unwrap_or(false)
        })
    };
    let (org_id, host) = match target {
        Some(t) => t,
        None if brief_target(&today, args.org_id, |_, _| true).is_some() => {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                "today's sessions all run on hosts of another organisation, so the \
                 brief has no host it may be drafted on",
            ))
        }
        None => {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                "no session is running today, so there is no host to draft the brief on",
            ))
        }
    };
    let (prompt, from) = brief_prompt(&today, org_id).ok_or_else(|| {
        IpcError::new(codes::E_INVALID_STATE, "nothing in today's digest to brief")
    })?;
    let model = {
        let s = lock(&deps.store)?;
        settings::get_string_for(&s, settings::WORK_SUMMARY_MODEL, org_id)
    };
    let run = Run {
        origin: crate::store::AUX_ORIGIN_MORNING_BRIEF,
        host: &host,
        model: &model,
        mission_id: None,
        org_id,
    };
    let (text, truncated) = run_draft(deps, &run, &prompt).await?;
    let brief = Brief {
        draft: Some(Draft {
            text,
            model,
            host_alias: host,
            from,
            at: crate::store::now_unix(),
            truncated,
        }),
        org_id,
    };
    BRIEFS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, brief.clone());
    Ok(brief)
}

#[cfg(test)]
mod tests;
