//! The opt-in classification nudge (work graph M4.6): when a session has
//! worked a few turns and nothing linked it to any work, hand its Claude a
//! short list of the few items it could be working on, and let it say which
//! — as a guess a person still decides.
//!
//! It fires on a UserPromptSubmit only when ALL of these hold:
//!
//! - `work.classify_nudge` is on (off by default);
//! - the prompt belongs to the row's current conversation, and that
//!   conversation has finished at least [`NUDGE_AFTER_TURNS`] turns;
//! - the session has no live link in any state but `rejected`;
//! - the conversation has not been nudged yet;
//! - there are 1 to [`NUDGE_MAX_CANDIDATES`] candidates: open items assigned
//!   to the user ("My work") in a tracker the session's repository maps to,
//!   plus local items touched in the last [`LOCAL_RECENT_SECS`].
//!
//! The text is at most [`NUDGE_MAX_CHARS`] and rides the same
//! `additionalContext` as the inbox, AFTER it. The answer, `work_link
//! { action: link, source: agent_inferred }`, is a pre-selected suggestion
//! (rule R11, tier `inferred`), never a confirmed link. Never a Stop
//! `block`, never a prompt typed into the pane.

use crate::ipc_error::IpcError;
use crate::mcp::guard::defuse;
use crate::service::orgs::OrgScope;
use crate::service::settings;
use crate::store::{SessionRow, Store};
use std::collections::BTreeSet;

/// Most characters of the nudge's text.
pub const NUDGE_MAX_CHARS: usize = 400;
/// Finished turns of a conversation before it may be nudged.
pub const NUDGE_AFTER_TURNS: i64 = 3;
/// More candidates than this and a guess is not worth asking for.
pub const NUDGE_MAX_CANDIDATES: usize = 5;
/// How recently a local item must have been touched to be a candidate.
pub const LOCAL_RECENT_SECS: i64 = 14 * 86_400;

const HEAD: &str = "[claude-fleet: work] If your task is one of: ";

/// One item the nudge offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NudgeCandidate {
    pub item_id: i64,
    /// The item's key; `None` for a local item without one (offered by id).
    pub key: Option<String>,
    pub title: String,
}

/// The nudge's text for `session_id` over `candidates`, or `None` when there
/// are none, too many, or they cannot fit [`NUDGE_MAX_CHARS`] even without
/// titles. Pure. Titles are third-party text: flattened, defused and cut
/// short, and they shrink before anything else does.
pub fn compose(session_id: i64, candidates: &[NudgeCandidate]) -> Option<String> {
    if candidates.is_empty() || candidates.len() > NUDGE_MAX_CANDIDATES {
        return None;
    }
    let tail = format!(
        ". If it is, call work_link {{action: link, session_id: {session_id}, source: \
         agent_inferred}} with that key or item_id. Otherwise ignore this; don't ask the user."
    );
    for title_max in [40, 24, 12, 0] {
        let list = candidates
            .iter()
            .map(|c| entry(c, title_max))
            .collect::<Vec<_>>()
            .join("; ");
        let text = format!("{HEAD}{list}{tail}");
        if text.chars().count() <= NUDGE_MAX_CHARS {
            return Some(text);
        }
    }
    None
}

fn entry(c: &NudgeCandidate, title_max: usize) -> String {
    let id = match &c.key {
        Some(k) => format!("key {}", clean(k, 40)),
        None => format!("item_id {}", c.item_id),
    };
    let title = clean(&c.title, title_max);
    if title.is_empty() {
        id
    } else {
        format!("{id} ({title})")
    }
}

/// One line, no marker, none of the characters the sentence around it uses,
/// at most `max` characters (an ellipsis marks a cut).
fn clean(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let flat = defuse(text)
        .chars()
        .map(|c| match c {
            '(' | ')' | '{' | '}' | ';' | '"' | '`' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The nudge for a UserPromptSubmit of `row` in `conversation`, or `None`
/// when any condition in the module doc does not hold. Reads only; the
/// caller stamps [`Store::mark_conversation_nudged`] when the text rides.
pub fn for_prompt(
    s: &Store,
    row: &SessionRow,
    conversation: &str,
    now: i64,
) -> Result<Option<String>, IpcError> {
    if !settings::get_bool(s, settings::WORK_CLASSIFY_NUDGE) {
        return Ok(None);
    }
    if matches!(row.kind.as_str(), "shell" | "external") {
        return Ok(None);
    }
    let Some(conv) = s.get_conversation(row.id, conversation)? else {
        return Ok(None);
    };
    if conv.turns < NUDGE_AFTER_TURNS || s.conversation_nudged(row.id, conversation)? {
        return Ok(None);
    }
    let links = s.session_work_links(row.id)?;
    if links.iter().any(|l| l.state != "rejected") {
        return Ok(None);
    }
    let rejected: BTreeSet<(Option<i64>, Option<String>)> =
        links.into_iter().map(|l| (l.item_id, l.ref_key)).collect();
    let found = candidates(s, row, &rejected, now)?;
    Ok(compose(row.id, &found))
}

/// Open items this session could be working on (see the module doc),
/// tracker items first, at most one more than [`NUDGE_MAX_CANDIDATES`] (so
/// "too many" is still seen). A guess never crosses orgs, and a per-host
/// token's org fence holds: an item of another org is never offered.
pub fn candidates(
    s: &Store,
    row: &SessionRow,
    rejected: &BTreeSet<(Option<i64>, Option<String>)>,
    now: i64,
) -> Result<Vec<NudgeCandidate>, IpcError> {
    let limit = NUDGE_MAX_CANDIDATES + 1;
    let scope = OrgScope::for_host(s, &row.host_alias)?;
    let session_org = s.session_org(row.id)?;
    let fits = |item_id: i64, key: Option<&String>| -> Result<bool, IpcError> {
        if rejected.iter().any(|(i, k)| {
            *i == Some(item_id) || (k.is_some() && k.as_ref() == key && key.is_some())
        }) {
            return Ok(false);
        }
        let org = s.item_org(item_id)?;
        let crosses = matches!((org, session_org), (Some(a), Some(b)) if a != b);
        Ok(!crosses && scope.sees_org(org))
    };
    let mut out: Vec<NudgeCandidate> = Vec::new();

    let st = s.detection_state(row.id)?;
    let repo = st.as_ref().and_then(|d| d.repo.clone());
    let worked = match st.as_ref().and_then(|d| d.project_id) {
        Some(p) => s.trackers_worked_in_project(p)?,
        None => BTreeSet::new(),
    };
    for t in s.list_trackers()? {
        let github = t.provider == "github"
            && repo
                .as_deref()
                .is_some_and(|r| crate::store::github_covers(&t, &r.to_ascii_lowercase()));
        if !github && !worked.contains(&t.id) {
            continue;
        }
        let Some(me) = t.config.account_id.as_deref() else {
            continue;
        };
        for (item, meta) in s.tracker_items(Some(t.id))? {
            if item.status_category == "done"
                || item.unavailable_at.is_some()
                || meta.assignee_id.as_deref() != Some(me)
                || !fits(item.id, item.key.as_ref())?
            {
                continue;
            }
            out.push(NudgeCandidate {
                item_id: item.id,
                key: item.key,
                title: item.title,
            });
            if out.len() >= limit {
                return Ok(out);
            }
        }
    }
    for item in s.recent_open_local_items(now - LOCAL_RECENT_SECS, limit)? {
        if out.iter().any(|c| c.item_id == item.id) || !fits(item.id, item.key.as_ref())? {
            continue;
        }
        out.push(NudgeCandidate {
            item_id: item.id,
            key: item.key,
            title: item.title,
        });
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "nudge/tests.rs"]
mod tests;
