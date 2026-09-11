//! Peer-to-peer messaging between Claude sessions.
//!
//! The `session_messages` table (migration 015) is the source of truth — every
//! send writes a row, and the recipient pulls when ready via `list_inbox`. An
//! optional real-time pane delivery is layered on top: when the sender sets
//! `deliver: true`, the message is *also* typed into the recipient's tmux
//! pane with a `[msg #id from name@host]:` header. Pane delivery is
//! best-effort — the inbox row lands regardless of what the SSH side does, so
//! callers can rely on the inbox.

use crate::ipc_error::IpcError;
use crate::service::sessions;
use crate::ssh::SshClient;
use crate::store::{SessionMessage, Store};
use std::sync::{Arc, Mutex};

#[derive(serde::Deserialize)]
pub struct SendMessageArgs {
    pub from_session_id: i64,
    pub to_session_id: i64,
    pub body: String,
    /// Tag the message; defaults to `"message"`. Receivers can filter on it
    /// (e.g. `"task"`, `"reply"`, `"alert"`).
    pub kind: Option<String>,
    /// When true, also type the message into the recipient's tmux pane.
    /// Defaults to false — inbox-only.
    #[serde(default)]
    pub deliver: bool,
    /// When `deliver`, whether to press Enter after the literal text.
    /// Defaults to true.
    #[serde(default = "default_true")]
    pub submit: bool,
    /// Id of the inbox message this one answers (migration 020). Must exist
    /// and involve the sender (`E_NOTFOUND` / `E_INVALID` otherwise).
    #[serde(default)]
    pub reply_to: Option<i64>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, serde::Serialize)]
pub struct SendMessageResult {
    pub id: i64,
    pub delivered_to_pane: bool,
    /// Pane delivery failure (if any). The inbox row landed regardless — the
    /// recipient will still see it on the next `inbox` call.
    pub deliver_error: Option<String>,
}

/// Maximum number of characters of a message body recorded in the
/// `message_sent` / `message_received` timeline events. The full body lives
/// in `session_messages`; the timeline only needs enough to be recognisable.
pub(crate) const TIMELINE_DETAIL_CHARS: usize = 120;

/// PURE: the body excerpt stored in the timeline events — the first
/// [`TIMELINE_DETAIL_CHARS`] characters (not bytes, so a multi-byte
/// character is never split).
pub(crate) fn timeline_detail(body: &str) -> String {
    body.chars().take(TIMELINE_DETAIL_CHARS).collect()
}

/// PURE: the line typed into the recipient's pane when `deliver` is set. The
/// header makes the source visible to the recipient; the id lets the
/// receiver correlate the pane line with the inbox entry.
pub(crate) fn pane_header(
    id: i64,
    from_tmux_name: &str,
    from_host_alias: &str,
    body: &str,
) -> String {
    format!("[msg #{id} from {from_tmux_name}@{from_host_alias}]: {body}")
}

/// Send one message from one session to another. The inbox row and both
/// timeline events are written in ONE transaction under ONE store lock
/// window (see [`Store::atomically`]) — either all three rows land or none
/// do, and nothing else can interleave between the existence checks and the
/// writes. Pane delivery (when requested) follows and never undoes it.
///
/// Design note: elsewhere in the codebase timeline events are best-effort
/// (`let _ = insert_session_event(..)`) because they are recorded *after* a
/// remote side effect has already happened, and failing there would
/// misreport a prompt that was in fact sent. Here nothing external has
/// happened yet when the events are written, so all-or-nothing is safe and
/// simpler to reason about: an event insert failure rolls the message back
/// and surfaces as the error.
pub async fn send_message(
    args: SendMessageArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SendMessageResult, IpcError> {
    if args.body.is_empty() {
        return Err(IpcError::new(
            "E_VALIDATE",
            "message body must be non-empty",
        ));
    }
    if args.from_session_id == args.to_session_id {
        return Err(IpcError::new(
            "E_SELF_TARGET",
            "from_session_id and to_session_id must differ",
        ));
    }
    let kind = args.kind.as_deref().unwrap_or("message");
    let detail = timeline_detail(&args.body);

    // Resolve both ends and write the row + events under one lock window and
    // in one transaction. We need the sender's name/host for the pane header,
    // and unknown ids must fail before anything is written.
    let (id, from_row, to_row) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.atomically(|s| {
            let from = s.get_session_by_id(args.from_session_id)?.ok_or_else(|| {
                IpcError::new(
                    "E_NOTFOUND",
                    format!("from session {} not found", args.from_session_id),
                )
            })?;
            let to = s.get_session_by_id(args.to_session_id)?.ok_or_else(|| {
                IpcError::new(
                    "E_NOTFOUND",
                    format!("to session {} not found", args.to_session_id),
                )
            })?;
            // A reply must point at a real message the sender took part in;
            // an arbitrary id would let an agent forge a thread.
            if let Some(parent_id) = args.reply_to {
                let parent = s.get_message(parent_id)?.ok_or_else(|| {
                    IpcError::new(
                        "E_NOTFOUND",
                        format!("reply_to message {parent_id} not found"),
                    )
                })?;
                if parent.from_session_id != args.from_session_id
                    && parent.to_session_id != args.from_session_id
                {
                    return Err(IpcError::new(
                        "E_INVALID",
                        format!(
                            "reply_to message {parent_id} does not involve session {}",
                            args.from_session_id
                        ),
                    ));
                }
            }
            // Inbox row — the source of truth.
            let id = s.insert_message(
                args.from_session_id,
                args.to_session_id,
                &args.body,
                kind,
                args.reply_to,
            )?;
            // Timeline events on both ends.
            s.insert_session_event(
                args.from_session_id,
                "message_sent",
                Some(&format!("to={} {}", args.to_session_id, detail)),
            )?;
            s.insert_session_event(
                args.to_session_id,
                "message_received",
                Some(&format!("from={} {}", args.from_session_id, detail)),
            )?;
            Ok((id, from, to))
        })?
    };

    let mut delivered_to_pane = false;
    let mut deliver_error: Option<String> = None;
    if args.deliver {
        let header = pane_header(id, &from_row.tmux_name, &from_row.host_alias, &args.body);
        match sessions::send_prompt(
            sessions::SendPromptArgs {
                host_alias: to_row.host_alias.clone(),
                tmux_name: to_row.tmux_name.clone(),
                prompt: header,
                submit: args.submit,
            },
            store,
            ssh,
        )
        .await
        {
            Ok(()) => delivered_to_pane = true,
            Err(e) => deliver_error = Some(e.message),
        }
    }

    Ok(SendMessageResult {
        id,
        delivered_to_pane,
        deliver_error,
    })
}

/// Return inbox messages for `session_id`. When `mark_read`, unread rows in
/// the returned set are flipped to read.
pub fn list_inbox(
    session_id: i64,
    unread_only: bool,
    limit: i64,
    mark_read: bool,
    store: &Mutex<Store>,
) -> Result<Vec<SessionMessage>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let msgs = s.list_inbox(session_id, unread_only, limit)?;
    if mark_read && !msgs.is_empty() {
        let ids: Vec<i64> = msgs
            .iter()
            .filter(|m| m.read_at.is_none())
            .map(|m| m.id)
            .collect();
        if !ids.is_empty() {
            let _ = s.mark_messages_read(&ids, session_id);
        }
    }
    Ok(msgs)
}

/// What a peer is doing right now — projected from the auto-populated
/// reconcile fields (`current_activity`, `claude_status`, `stuck_kind`,
/// `context_pct`). No new schema; the caller-facing answer to "what's
/// session N working on?"
#[derive(Debug, serde::Serialize)]
pub struct PeerStatus {
    pub session_id: i64,
    pub host_alias: String,
    pub tmux_name: String,
    pub status: String,
    pub claude_status: Option<String>,
    pub current_activity: Option<String>,
    pub stuck_kind: Option<String>,
    pub context_pct: Option<f64>,
    pub last_activity_at: i64,
}

pub fn peer_status(session_id: i64, store: &Mutex<Store>) -> Result<PeerStatus, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let row = s
        .get_session_by_id(session_id)?
        .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("session {} not found", session_id)))?;
    Ok(PeerStatus {
        session_id: row.id,
        host_alias: row.host_alias,
        tmux_name: row.tmux_name,
        status: row.status,
        claude_status: row.claude_status,
        current_activity: row.current_activity,
        stuck_kind: row.stuck_kind,
        context_pct: row.context_pct,
        last_activity_at: row.last_activity_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::sessions::build_send_commands;

    fn seed(s: &Store, name: &str) -> i64 {
        s.upsert_host("local").unwrap();
        s.upsert_session(name, "local", None, None, 0, 0, "running", None)
            .unwrap()
    }

    fn args(from: i64, to: i64, body: &str) -> SendMessageArgs {
        SendMessageArgs {
            from_session_id: from,
            to_session_id: to,
            body: body.to_string(),
            kind: None,
            deliver: false,
            submit: true,
            reply_to: None,
        }
    }

    fn fixture() -> (Mutex<Store>, Arc<SshClient>, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        (Mutex::new(s), Arc::new(SshClient::new()), a, b)
    }

    // ---- pure helpers ----

    #[test]
    fn timeline_detail_truncates_at_the_char_boundary() {
        let under: String = "x".repeat(TIMELINE_DETAIL_CHARS - 1);
        let exact: String = "x".repeat(TIMELINE_DETAIL_CHARS);
        let over: String = "x".repeat(TIMELINE_DETAIL_CHARS + 1);
        assert_eq!(timeline_detail(""), "");
        assert_eq!(timeline_detail(&under), under);
        assert_eq!(timeline_detail(&exact), exact);
        assert_eq!(timeline_detail(&over), exact);
    }

    #[test]
    fn timeline_detail_counts_chars_not_bytes() {
        // 121 multi-byte chars: byte-based truncation would either split a
        // codepoint or keep far fewer than 120 characters.
        let body: String = "🦀".repeat(TIMELINE_DETAIL_CHARS + 1);
        let detail = timeline_detail(&body);
        assert_eq!(detail.chars().count(), TIMELINE_DETAIL_CHARS);
        assert!(detail.chars().all(|c| c == '🦀'));
    }

    #[test]
    fn pane_header_names_id_sender_and_host() {
        assert_eq!(
            pane_header(42, "dev-foo", "hetzner", "hello there"),
            "[msg #42 from dev-foo@hetzner]: hello there"
        );
    }

    #[test]
    fn deliver_payload_is_the_header_as_a_literal_send_keys_then_enter() {
        // The transport (`send_prompt` → bash/ssh + tmux) is not exercised
        // here; this pins the tmux payload the deliver path hands to it.
        let header = pane_header(7, "alpha", "local", "it's done; $HOME `ok`");
        let cmds = build_send_commands("beta", &header, true);
        assert_eq!(cmds.len(), 3);
        assert_eq!(
            cmds[0],
            "tmux send-keys -t 'beta' -l '[msg #7 from alpha@local]: it'\\''s done; $HOME `ok`'"
        );
        assert_eq!(cmds[2], "tmux send-keys -t 'beta' Enter");
        // submit=false stages the text without pressing Enter.
        assert_eq!(build_send_commands("beta", &header, false).len(), 1);
    }

    // ---- send_message validation ----

    #[tokio::test]
    async fn empty_body_is_rejected_before_any_lookup() {
        let (store, ssh, a, b) = fixture();
        let err = send_message(args(a, b, ""), &store, &ssh)
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
        assert!(store
            .lock()
            .unwrap()
            .list_inbox(b, false, 10)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn message_to_self_is_rejected_with_e_self_target() {
        let (store, ssh, a, _) = fixture();
        let err = send_message(args(a, a, "hi me"), &store, &ssh)
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_SELF_TARGET");
        let s = store.lock().unwrap();
        assert!(s.list_inbox(a, false, 10).unwrap().is_empty());
        assert!(s.list_session_events(a, 10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn unknown_target_is_e_notfound_and_writes_nothing() {
        let (store, ssh, a, _) = fixture();
        let err = send_message(args(a, 9999, "anyone?"), &store, &ssh)
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
        assert!(err.message.contains("to session 9999"));
        let s = store.lock().unwrap();
        assert!(s.list_inbox(9999, false, 10).unwrap().is_empty());
        // No orphaned `message_sent` on the sender either.
        assert!(s.list_session_events(a, 10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn unknown_sender_is_e_notfound() {
        let (store, ssh, _, b) = fixture();
        let err = send_message(args(9999, b, "ghost"), &store, &ssh)
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
        assert!(err.message.contains("from session 9999"));
        assert!(store
            .lock()
            .unwrap()
            .list_inbox(b, false, 10)
            .unwrap()
            .is_empty());
    }

    // ---- send_message happy path ----

    #[tokio::test]
    async fn send_writes_inbox_row_and_both_timeline_events() {
        let (store, ssh, a, b) = fixture();
        let res = send_message(args(a, b, "ping"), &store, &ssh)
            .await
            .unwrap();
        assert!(!res.delivered_to_pane);
        assert_eq!(res.deliver_error, None);

        let s = store.lock().unwrap();
        let inbox = s.list_inbox(b, false, 10).unwrap();
        assert_eq!(inbox.len(), 1);
        let m = &inbox[0];
        assert_eq!(m.id, res.id);
        assert_eq!((m.from_session_id, m.to_session_id), (a, b));
        assert_eq!(m.body, "ping");
        assert_eq!(m.kind, "message");
        assert_eq!(m.read_at, None);
        // The sender's inbox is untouched.
        assert!(s.list_inbox(a, false, 10).unwrap().is_empty());

        let sent = s.list_session_events(a, 10).unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].kind, "message_sent");
        assert_eq!(
            sent[0].detail.as_deref(),
            Some(format!("to={b} ping").as_str())
        );
        let received = s.list_session_events(b, 10).unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].kind, "message_received");
        assert_eq!(
            received[0].detail.as_deref(),
            Some(format!("from={a} ping").as_str())
        );
    }

    #[tokio::test]
    async fn send_honours_custom_kind_and_truncates_timeline_detail() {
        let (store, ssh, a, b) = fixture();
        let body = "y".repeat(TIMELINE_DETAIL_CHARS + 40);
        let mut a_args = args(a, b, &body);
        a_args.kind = Some("task".into());
        send_message(a_args, &store, &ssh).await.unwrap();

        let s = store.lock().unwrap();
        let inbox = s.list_inbox(b, true, 10).unwrap();
        assert_eq!(inbox[0].kind, "task");
        assert_eq!(inbox[0].body, body, "inbox keeps the full body");
        let ev = &s.list_session_events(b, 10).unwrap()[0];
        let detail = ev.detail.as_deref().unwrap();
        let prefix = format!("from={a} ");
        assert!(detail.starts_with(&prefix));
        assert_eq!(detail.len() - prefix.len(), TIMELINE_DETAIL_CHARS);
    }

    // ---- inbox ----

    #[tokio::test]
    async fn inbox_marks_read_only_when_asked() {
        let (store, ssh, a, b) = fixture();
        send_message(args(a, b, "one"), &store, &ssh).await.unwrap();
        send_message(args(a, b, "two"), &store, &ssh).await.unwrap();

        // Peek without marking: both still unread afterwards.
        let peek = list_inbox(b, true, 10, false, &store).unwrap();
        assert_eq!(peek.len(), 2);
        assert_eq!(list_inbox(b, true, 10, false, &store).unwrap().len(), 2);

        // Read with mark_read: the returned rows are the pre-flip snapshot,
        // but a later unread-only call is empty and read_at is stamped.
        let read = list_inbox(b, true, 10, true, &store).unwrap();
        assert_eq!(read.len(), 2);
        assert!(list_inbox(b, true, 10, false, &store).unwrap().is_empty());
        let all = list_inbox(b, false, 10, false, &store).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|m| m.read_at.is_some()));
        // Newest first.
        assert_eq!(all[0].body, "two");
    }

    #[tokio::test]
    async fn inbox_limit_caps_the_result() {
        let (store, ssh, a, b) = fixture();
        for i in 0..5 {
            send_message(args(a, b, &format!("m{i}")), &store, &ssh)
                .await
                .unwrap();
        }
        assert_eq!(list_inbox(b, false, 3, false, &store).unwrap().len(), 3);
    }

    // ---- reply_to ----

    #[tokio::test]
    async fn reply_to_is_recorded_and_must_reference_a_message_the_sender_took_part_in() {
        let (store, ssh, a, b) = fixture();
        let first = send_message(args(a, b, "question?"), &store, &ssh)
            .await
            .unwrap();
        let mut reply = args(b, a, "answer.");
        reply.reply_to = Some(first.id);
        let res = send_message(reply, &store, &ssh).await.unwrap();
        let inbox = list_inbox(a, false, 10, false, &store).unwrap();
        assert_eq!(inbox[0].id, res.id);
        assert_eq!(inbox[0].reply_to, Some(first.id));
        // The original carries no reply_to.
        let b_inbox = list_inbox(b, false, 10, false, &store).unwrap();
        assert_eq!(b_inbox[0].reply_to, None);

        // Unknown parent → E_NOTFOUND, nothing written.
        let mut bad = args(b, a, "to nowhere");
        bad.reply_to = Some(9999);
        let err = send_message(bad, &store, &ssh).await.unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
        assert_eq!(list_inbox(a, false, 10, false, &store).unwrap().len(), 1);

        // A third session cannot reply to a thread it is not part of.
        let c = {
            let s = store.lock().unwrap();
            seed(&s, "gamma")
        };
        let mut forged = args(c, a, "me too");
        forged.reply_to = Some(first.id);
        let err = send_message(forged, &store, &ssh).await.unwrap_err();
        assert_eq!(err.code, "E_INVALID");
    }

    // ---- atomicity ----

    #[test]
    fn atomically_rolls_back_every_write_when_the_closure_fails() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let err = s
            .atomically(|s| {
                s.insert_message(a, b, "half", "message", None)?;
                s.insert_session_event(a, "message_sent", Some("half"))?;
                Err::<(), _>(IpcError::new("E_TEST", "boom"))
            })
            .unwrap_err();
        assert_eq!(err.code, "E_TEST");
        assert!(s.list_inbox(b, false, 10).unwrap().is_empty());
        assert!(s.list_session_events(a, 10).unwrap().is_empty());
        // The connection is usable again after the rollback.
        let id = s
            .atomically(|s| s.insert_message(a, b, "whole", "message", None))
            .unwrap();
        assert_eq!(s.list_inbox(b, false, 10).unwrap()[0].id, id);
    }

    // ---- peer_status ----

    #[test]
    fn peer_status_projects_the_row_or_e_notfound() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let store = Mutex::new(s);
        let p = peer_status(a, &store).unwrap();
        assert_eq!((p.session_id, p.tmux_name.as_str()), (a, "alpha"));
        assert_eq!(p.host_alias, "local");
        assert_eq!(p.status, "running");
        assert_eq!(peer_status(4242, &store).unwrap_err().code, "E_NOTFOUND");
    }
}
