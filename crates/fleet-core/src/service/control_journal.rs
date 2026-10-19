//! Control's journal (Control review F12): "the model decides, Fleet owns
//! the state".
//!
//! Control's agent kept what it was doing only in its conversation, so a
//! move to another host (`auto_restore`'s `operator_moved_note`), a
//! restart, a compaction or `/clear` started it from nothing. The hub now
//! keeps a small journal for it and hands it over as the first `[fleet]`
//! note of every new conversation:
//!
//! * **what it follows**: the sessions it started or briefed and the
//!   missions it handed off, read live (`service::control_follow`), each
//!   with Control's last brief;
//! * **what it asked the person** and is still waiting on (`ask` rows,
//!   resolved when the person next speaks);
//! * **the last thing the person said** to it (`said`, written by the
//!   UserPromptSubmit hook from the person's own words only, never from a
//!   fleet note or Claude Code's own turns: [`on_prompt`]);
//! * **its own notes** (`note`).
//!
//! Everything but the notes and asks comes from events fleet already
//! records; the model writes nothing to keep it. `control_journal` (MCP)
//! reads it whole and lets Control add a note or an ask, and the person or
//! Control mute a thread.
//!
//! **When.** The `control_journal` loop reads the operator's own timeline
//! for `conversation_started` (a start, a restart, a move's new birth,
//! `/clear`, `/resume`) and `compact_done`, and queues the journal at the
//! operator's next idle moment (`sessions::deferred`), once per pass and
//! never while one is still queued. An empty journal is not sent. Off with
//! `control.journal`; stops while automation is paused.
//!
//! The cursor lives in one internal setting, [`STATE_KEY`].

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::control_follow::{self, Follow, FollowState};
use crate::service::settings;
use crate::ssh::SshClient;
use crate::store::{journal_text, JournalEntry, SessionRow, Store, JOURNAL_SAID_MAX};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// What a journal note opens with. It starts with `[fleet]`
/// (`prompt_origin::FLEET_PROMPT_MARKS`), so it is typed and drawn as
/// fleet's, never as the person.
pub const JOURNAL_MARK: &str = "[fleet] Control journal";
/// The internal setting holding [`JournalState`]. No `SPECS` row.
pub const STATE_KEY: &str = "control.journal_state";
/// The operator timeline kinds that mean a new conversation.
pub const NEW_CONVERSATION_KINDS: [&str; 2] = ["conversation_started", "compact_done"];
/// Lines of each list a note shows; the rest are counted.
pub const NOTE_LINES: usize = 8;
/// Notes a journal note carries, newest first.
pub const NOTES_SHOWN: i64 = 5;

/// The operator's standing instructions about the journal (one section of
/// its CLAUDE.md, `operator::CLAUDE_MD_SECTIONS`).
pub const SECTION: &str = "**Your journal.** Fleet keeps your state, not your conversation: \
what you follow, what you asked the person, what they last said. A message that opens with \
`[fleet] Control journal` is it, handed to you at the start of a new conversation (a restart, \
a move, a compaction, a new chat): pick up from it without asking the person to repeat \
themselves, and check live state before you act on it. `control_journal` reads it whole; \
`control_journal {action: note, text}` keeps something you must not forget, and `{action: \
ask, text}` records a question you put to the person (it clears when they answer). Fleet \
stops telling you about a session once it is done, killed or merged, or quiet for a day, \
and about a thread the person muted.\n";

/// The journal as `control_journal { action: read }` answers it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalView {
    /// Every thread Control started or was handed, with whether it is
    /// followed (`state`) and muted: the MCP-visible flag.
    pub follows: Vec<FollowLine>,
    /// Questions Control asked the person, still open, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<JournalEntry>,
    /// The last thing the person said to Control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<JournalEntry>,
    /// Control's own notes, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<JournalEntry>,
}

/// One thread of the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowLine {
    #[serde(flatten)]
    pub follow: Follow,
    /// A session Control started (rather than only briefed).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub started_by_control: bool,
    /// A session's `claude_status`, a mission's state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Control's last brief to it: when, and its first line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
    /// A mission's items done and in all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<(i64, i64)>,
}

/// The operator's row, when one was ever made and still has one.
pub fn operator_row(s: &Store) -> Option<SessionRow> {
    let r = crate::service::operator::operator_ref(s)?;
    s.find_sessions_by_tmux_name(&r.tmux_name, Some(&r.host_alias))
        .ok()?
        .into_iter()
        .next()
}

/// The journal now.
pub fn view(s: &Store, now: i64) -> JournalView {
    let op = operator_row(s).map(|r| r.id);
    let mut follows = Vec::new();
    for f in control_follow::follows(s, op, now) {
        let mut line = FollowLine {
            follow: f,
            started_by_control: false,
            status: None,
            brief_at: None,
            brief: None,
            progress: None,
        };
        if line.follow.kind == "session" {
            if let Ok(Some(row)) = s.get_session_by_id(line.follow.id) {
                line.started_by_control = row.origin.as_deref() == Some("operator");
                line.status = row.claude_status.clone();
            }
            if let Ok(Some((at, preview))) = s.latest_brief_of(line.follow.id) {
                line.brief_at = Some(at);
                line.brief = preview;
            }
        } else if let Ok(Some(m)) = s.get_mission(line.follow.id) {
            line.status = Some(m.state.clone());
            line.progress = Some((m.done, m.total));
        }
        follows.push(line);
    }
    JournalView {
        follows,
        asks: s.open_journal_asks().unwrap_or_default(),
        said: s.last_journal_said().ok().flatten(),
        notes: s.journal_entries("note", NOTES_SHOWN).unwrap_or_default(),
    }
}

/// PURE: "4 min ago", "3 h ago", "2 d ago".
pub fn ago(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 2 * 86_400 {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} d ago", secs / 86_400)
    }
}

fn state_word(s: FollowState) -> &'static str {
    match s {
        FollowState::Following => "following",
        FollowState::Muted => "muted",
        FollowState::Killed => "killed",
        FollowState::Merged => "merged",
        FollowState::Done => "done",
        FollowState::Quiet => "quiet",
    }
}

/// PURE: the journal note for `v`, `None` when there is nothing in it to
/// pick up from (nothing followed, asked, said or noted).
pub fn render(v: &JournalView, now: i64) -> Option<String> {
    let following: Vec<&FollowLine> = v
        .follows
        .iter()
        .filter(|l| l.follow.state.following())
        .collect();
    if following.is_empty() && v.asks.is_empty() && v.said.is_none() && v.notes.is_empty() {
        return None;
    }
    let mut out = format!(
        "{JOURNAL_MARK}. This is a new conversation, so here is what fleet kept for you. It is \
         state, not a request, and nothing here asks for a reply; check live state before you \
         act on it. `control_journal` has all of it.\n"
    );
    if !following.is_empty() {
        out.push_str("\nYou follow:\n");
        for l in following.iter().take(NOTE_LINES) {
            let f = &l.follow;
            let mut line = if f.kind == "session" {
                let host = f.host_alias.as_deref().unwrap_or("?");
                let who = if l.started_by_control {
                    ", you started it"
                } else {
                    ""
                };
                format!("- session {} on {host} (#{}{who})", f.name, f.id)
            } else {
                format!("- mission #{} \"{}\"", f.id, f.name)
            };
            if let Some(st) = &l.status {
                line.push_str(&format!(": {st}"));
            }
            if let Some((done, total)) = l.progress.filter(|(_, t)| *t > 0) {
                line.push_str(&format!(", {done} of {total} done"));
            }
            if let Some(at) = l.brief_at {
                line.push_str(&format!("; your last brief {}", ago(now - at)));
                if let Some(b) = &l.brief {
                    line.push_str(&format!(": \"{b}\""));
                }
            }
            out.push_str(&line);
            out.push('\n');
        }
        if following.len() > NOTE_LINES {
            out.push_str(&format!("- … and {} more.\n", following.len() - NOTE_LINES));
        }
    }
    let stopped: Vec<&FollowLine> = v
        .follows
        .iter()
        .filter(|l| !l.follow.state.following())
        .collect();
    if !stopped.is_empty() {
        let mut by: std::collections::BTreeMap<&str, usize> = Default::default();
        for l in &stopped {
            *by.entry(state_word(l.follow.state)).or_default() += 1;
        }
        let why = by
            .iter()
            .map(|(w, n)| format!("{n} {w}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("No longer followed: {why}.\n"));
    }
    if !v.asks.is_empty() {
        out.push_str("\nYou asked the person and they have not answered yet:\n");
        for a in v.asks.iter().take(NOTE_LINES) {
            out.push_str(&format!("- {} ({})\n", a.text, ago(now - a.at)));
        }
    }
    if let Some(said) = &v.said {
        out.push_str(&format!(
            "\nThe person last said ({}): \"{}\"\n",
            ago(now - said.at),
            said.text
        ));
    }
    if !v.notes.is_empty() {
        out.push_str("\nYour notes:\n");
        for n in &v.notes {
            out.push_str(&format!("- {} ({})\n", n.text, ago(now - n.at)));
        }
    }
    Some(out.trim_end().to_string())
}

/// The UserPromptSubmit hook's call with the person's own words (`human`
/// is `prompt_origin::human_part`'s answer, so a fleet note or a harness
/// turn never gets here): on the operator's row, the journal's `said`.
pub fn on_prompt(s: &Store, row: &SessionRow, human: &str, now: i64) {
    if !crate::service::operator::is_operator_session(s, &row.host_alias, &row.tmux_name) {
        return;
    }
    if !settings::get_bool(s, settings::CONTROL_JOURNAL) {
        return;
    }
    let Some(text) = journal_text(human, JOURNAL_SAID_MAX) else {
        return;
    };
    if let Err(e) = s.insert_journal_entry("said", &text, now) {
        tracing::debug!(error = %e, "[control-journal] said not recorded");
    }
}

/// `control_journal { action: note | ask, text }`: one entry of Control's.
pub fn add(store: &Mutex<Store>, kind: &str, text: &str, now: i64) -> Result<i64, IpcError> {
    if !matches!(kind, "note" | "ask") {
        return Err(IpcError::new(
            codes::E_INVALID,
            "a journal entry is a note or an ask",
        ));
    }
    let Some(text) = journal_text(text, crate::store::JOURNAL_TEXT_MAX) else {
        return Err(IpcError::new(codes::E_INVALID, "text is empty"));
    };
    Ok(lock(store)?.insert_journal_entry(kind, &text, now)?)
}

/// `control_journal { action: mute | unmute, thread, id }`. Tells the
/// owner's devices (`handoff:changed`), whose threads column redraws.
pub fn mute(
    store: &Mutex<Store>,
    thread: &str,
    id: i64,
    on: bool,
    now: i64,
) -> Result<bool, IpcError> {
    if !crate::store::MUTE_KINDS.contains(&thread) {
        return Err(IpcError::new(
            codes::E_INVALID,
            "thread is session or mission",
        ));
    }
    let s = lock(store)?;
    let changed = s.set_control_mute(thread, id, on, now)?;
    if changed {
        s.bus_handoff_changed();
    }
    Ok(changed)
}

/// `control_journal`'s arguments, for the MCP tool and the desktop alike.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalArgs {
    /// read (default) | note | ask | mute | unmute
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
}

/// `control_journal`: `read` answers [`view`]; `note` / `ask` answer
/// `{id}`; `mute` / `unmute` answer `{changed}`.
pub fn call(
    store: &Mutex<Store>,
    a: &JournalArgs,
    now: i64,
) -> Result<serde_json::Value, IpcError> {
    let action = a.action.as_deref().unwrap_or("read");
    match action {
        "read" => Ok(serde_json::to_value(view(&*lock(store)?, now)).unwrap_or_default()),
        "note" | "ask" => {
            let id = add(store, action, a.text.as_deref().unwrap_or(""), now)?;
            Ok(serde_json::json!({ "id": id }))
        }
        "mute" | "unmute" => {
            let (Some(thread), Some(id)) = (a.thread.as_deref(), a.id) else {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "mute and unmute name thread and id",
                ));
            };
            let changed = mute(store, thread, id, action == "mute", now)?;
            Ok(serde_json::json!({ "changed": changed }))
        }
        other => Err(IpcError::new(
            codes::E_INVALID,
            format!("unknown action {other}: read, note, ask, mute or unmute"),
        )),
    }
}

/// What the loop remembers between passes ([`STATE_KEY`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalState {
    /// The newest timeline id already looked at; `None` until the first
    /// pass, which starts from now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<i64>,
    /// When the last journal note was queued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<i64>,
}

impl JournalState {
    fn load(s: &Store) -> Self {
        s.get_setting(STATE_KEY)
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }

    fn save(&self, s: &Store) {
        let j = serde_json::to_string(self).unwrap_or_default();
        if let Err(e) = s.set_setting(STATE_KEY, &j) {
            tracing::debug!(error = %e, "[control-journal] state not saved");
        }
    }
}

/// Side effects of a pass, injected for tests.
#[async_trait]
pub trait JournalExec: Send + Sync {
    /// Queue `body` for the session's next idle moment.
    async fn queue(&self, session_id: i64, body: String) -> Result<(), IpcError>;
}

/// Production executor over the deferred queue.
pub struct RealJournalExec {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
}

#[async_trait]
impl JournalExec for RealJournalExec {
    async fn queue(&self, session_id: i64, body: String) -> Result<(), IpcError> {
        crate::service::sessions::deferred::queue_prompt(
            crate::service::sessions::deferred::QueuePromptArgs {
                session_id,
                prompt: body,
                ..Default::default()
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map(|_| ())
    }
}

/// Whether a journal note is still waiting in the operator's queue.
fn note_pending(s: &Store, session_id: i64) -> bool {
    s.list_deferred_prompts(Some(session_id))
        .unwrap_or_default()
        .iter()
        .any(|p| {
            p.delivered_at.is_none()
                && p.failed_at.is_none()
                && p.cancelled_at.is_none()
                && p.skipped_at.is_none()
                && p.body.starts_with(JOURNAL_MARK)
        })
}

/// What a pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JournalReport {
    pub sent: bool,
    pub held: Option<&'static str>,
}

/// One pass: when the operator started a new conversation since the
/// cursor, queue its journal.
pub async fn run_with(store: &Mutex<Store>, exec: &dyn JournalExec, now: i64) -> JournalReport {
    let (op_id, body, mut state, newest) = {
        let Ok(s) = store.lock() else {
            return JournalReport::default();
        };
        if crate::service::loops::paused(&s) {
            return JournalReport {
                held: Some("paused"),
                ..Default::default()
            };
        }
        let mut state = JournalState::load(&s);
        let op = operator_row(&s);
        if !settings::get_bool(&s, settings::CONTROL_JOURNAL) || op.is_none() {
            if state != JournalState::default() {
                JournalState::default().save(&s);
            }
            return JournalReport::default();
        }
        let op = op.expect("checked");
        // A read error leaves the cursor where it is: falling back to 0
        // would rewind it onto every past new conversation, and an empty
        // list would skip a real one.
        let Ok(newest) = s.newest_session_event_id() else {
            return JournalReport::default();
        };
        let Some(cursor) = state.cursor else {
            state.cursor = Some(newest);
            state.save(&s);
            return JournalReport::default();
        };
        let Ok(started) = s.session_events_of_kinds_after(op.id, &NEW_CONVERSATION_KINDS, cursor)
        else {
            return JournalReport::default();
        };
        if started.is_empty() {
            if newest != cursor {
                state.cursor = Some(newest);
                state.save(&s);
            }
            return JournalReport::default();
        }
        if note_pending(&s, op.id) {
            state.cursor = Some(newest);
            state.save(&s);
            return JournalReport {
                held: Some("note_pending"),
                ..Default::default()
            };
        }
        let body = render(&view(&s, now), now);
        if body.is_none() {
            state.cursor = Some(newest);
            state.save(&s);
            return JournalReport {
                held: Some("empty"),
                ..Default::default()
            };
        }
        (op.id, body.expect("checked"), state, newest)
    };
    match exec.queue(op_id, body).await {
        Ok(()) => {
            state.cursor = Some(newest);
            state.sent_at = Some(now);
            if let Ok(s) = store.lock() {
                state.save(&s);
            }
            JournalReport {
                sent: true,
                held: None,
            }
        }
        Err(e) => {
            // The cursor stays: the next pass tries again.
            tracing::debug!(error = %e.message, "[control-journal] note not queued");
            JournalReport {
                held: Some("queue_failed"),
                ..Default::default()
            }
        }
    }
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// The tick's entry: gated (`control_journal`, stops while paused), cheap
/// when off, detached and single-flight.
pub fn maybe_run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) {
    {
        let Ok(s) = store.lock() else {
            return;
        };
        if !crate::service::loops::gate_in("control_journal", &s, None) {
            return;
        }
    }
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    crate::rt::spawn(async move {
        let _flight = crate::rt::ClearOnDrop::of_static(&RUNNING);
        let exec = RealJournalExec {
            store: Arc::clone(&store),
            ssh,
        };
        let r = run_with(&store, &exec, crate::store::now_unix()).await;
        crate::service::loops::report("control_journal", Ok::<_, String>(()), None);
        if r.sent {
            tracing::info!("[control-journal] journal queued for a new Control conversation");
        }
    });
}

#[cfg(test)]
#[path = "control_journal_tests.rs"]
mod tests;
