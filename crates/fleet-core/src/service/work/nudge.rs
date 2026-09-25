//! The opt-in classification nudge (work graph M4.6).
//!
//! When detection found nothing — three turns in, no link and no suggestion
//! — and the person has only a handful of open tickets, one prompt of the
//! conversation carries a short note naming them and asking Claude to say
//! which one it is on: `work_link { action: link, source: agent_inferred }`.
//! The answer is only ever a pre-selected suggestion (rule R11 in
//! [`super::resolve`]); a person decides. Nothing here blocks a Stop or types
//! into the pane.
//!
//! When it fires — all of:
//! - `work.classify_nudge` is on (off by default);
//! - the conversation has had at least [`NUDGE_AFTER_TURNS`] turns;
//! - the session has no live link, confirmed or suggested;
//! - there are 1..=[`MAX_CANDIDATES`] candidates: the person's *My work*
//!   tickets inside the host's scope, and keyed local items changed in the
//!   last [`RECENT_DAYS`] days, less any the session rejected;
//! - it has not fired in this conversation.
//!
//! The candidates' titles are the tracker's text and ride inside one
//! untrusted fence; the keys and the instruction are fleet's own. The whole
//! note is at most [`NUDGE_MAX_CHARS`].

use crate::ipc_error::IpcError;
use crate::service::orgs::OrgScope;
use crate::service::trackers::tickets::{tickets_in, RECENT_DAYS};
use crate::store::{SessionRow, Store};
use std::collections::BTreeSet;

/// Turns a conversation has had, with no link, before the nudge may fire.
pub const NUDGE_AFTER_TURNS: i64 = 3;

/// More candidates than this and the nudge stays quiet: a list that long is
/// a guessing game, and the agent's answer would be worth little.
pub const MAX_CANDIDATES: usize = 5;

/// The most characters the note may take of the prompt's context.
pub const NUDGE_MAX_CHARS: usize = 400;

/// Where the candidates' titles come from, as the untrusted fence names it.
const TITLES_FROM: &str = "the tracker";

/// One work item the note offers: its key (fleet's) and title (third-party).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NudgeCandidate {
    pub key: String,
    pub title: String,
}

/// The note for session `session_id` offering `candidates`, at most
/// [`NUDGE_MAX_CHARS`] characters. The instruction and the keys are fleet's
/// own; the titles ride in one untrusted fence, cut to fit — and dropped
/// whole when not even the fence fits, since the keys alone are enough to
/// answer.
pub fn nudge_text(session_id: i64, candidates: &[NudgeCandidate]) -> String {
    use crate::mcp::guard::{defuse, fence_untrusted};
    let keys: Vec<String> = candidates.iter().map(|c| defuse(&c.key)).collect();
    let head = format!(
        "[claude-fleet: work] If this session works on one of these tickets, call work_link \
         {{action: link, session_id: {session_id}, key: <KEY>, source: agent_inferred}}; \
         else ignore this, don't ask the user.\nTickets: {}",
        keys.join(", ")
    );
    let flat = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
    let titles: Vec<String> = candidates
        .iter()
        .filter(|c| !c.title.trim().is_empty())
        .map(|c| format!("{}: {}", c.key, flat(&c.title)))
        .collect();
    let fits = |s: &str| s.chars().count() <= NUDGE_MAX_CHARS;
    if titles.is_empty() {
        return cut(&head);
    }
    // The fence with an empty body is the fixed cost; what is left of the
    // budget is what the titles may take.
    let overhead = 1 + fence_untrusted("", TITLES_FROM, 0).chars().count();
    let room = NUDGE_MAX_CHARS.saturating_sub(head.chars().count() + overhead);
    if room < MIN_TITLE_ROOM {
        return cut(&head);
    }
    let out = format!(
        "{head}\n{}",
        fence_untrusted(&titles.join("\n"), TITLES_FROM, room)
    );
    if fits(&out) {
        out
    } else {
        cut(&head)
    }
}

/// Fewer characters than this for the titles is not worth a fence.
const MIN_TITLE_ROOM: usize = 16;

/// The note never exceeds its budget, even with many long keys.
fn cut(s: &str) -> String {
    s.chars().take(NUDGE_MAX_CHARS).collect()
}

/// The note for this prompt of `row`'s conversation `conversation`, or
/// `None` when it should not fire (see the module doc). Reads under the
/// caller's store guard; stamps nothing — the caller stamps only a note it
/// actually delivered ([`Store::mark_conversation_nudged`]).
pub fn classify_nudge(
    s: &Store,
    row: &SessionRow,
    conversation: &str,
    now: i64,
) -> Result<Option<String>, IpcError> {
    if !crate::service::settings::get_bool(s, crate::service::settings::WORK_CLASSIFY_NUDGE) {
        return Ok(None);
    }
    if s.conversation_nudged(row.id, conversation)? {
        return Ok(None);
    }
    let Some(conv) = s.get_conversation(row.id, conversation)? else {
        return Ok(None);
    };
    if conv.turns < NUDGE_AFTER_TURNS {
        return Ok(None);
    }
    let links = s.session_work_links(row.id)?;
    if links
        .iter()
        .any(|l| l.state == "confirmed" || l.state == "suggested")
    {
        return Ok(None);
    }
    let mut rejected: BTreeSet<String> = BTreeSet::new();
    for l in links.iter().filter(|l| l.state == "rejected") {
        let key = match (&l.ref_key, l.item_id) {
            (Some(k), _) => Some(k.clone()),
            (None, Some(id)) => s.get_work_item(id)?.and_then(|i| i.key),
            _ => None,
        };
        if let Some(k) = key {
            rejected.insert(k.to_uppercase());
        }
    }
    // The note is read by the Claude on the row's host, so it offers only
    // what that host may read (work graph M5, and M3's host fence on
    // tracker items) — exactly the SessionStart context's scope.
    let scope = OrgScope::for_host(s, &row.host_alias)?;
    let want = MAX_CANDIDATES + 1 + rejected.len();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<NudgeCandidate> = Vec::new();
    let mut offer = |key: Option<String>, title: String| {
        let Some(key) = key.map(|k| k.to_uppercase()) else {
            return;
        };
        if rejected.contains(&key) || !seen.insert(key.clone()) {
            return;
        }
        out.push(NudgeCandidate { key, title });
    };
    for t in tickets_in(s, None, Some("mine"), None, Some(want), &scope)? {
        offer(t.item.key, t.item.title);
    }
    // Local items belong to no org, which every scope sees (M5).
    for item in s.recent_local_work_items(now - RECENT_DAYS * 86_400, want)? {
        offer(item.key, item.title);
    }
    if out.is_empty() || out.len() > MAX_CANDIDATES {
        return Ok(None);
    }
    Ok(Some(nudge_text(row.id, &out)))
}

#[cfg(test)]
mod tests;
