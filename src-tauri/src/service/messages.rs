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
}

fn default_true() -> bool {
    true
}

#[derive(serde::Serialize)]
pub struct SendMessageResult {
    pub id: i64,
    pub delivered_to_pane: bool,
    /// Pane delivery failure (if any). The inbox row landed regardless — the
    /// recipient will still see it on the next `inbox` call.
    pub deliver_error: Option<String>,
}

/// Send one message from one session to another. The inbox row is written
/// first; pane delivery (when requested) follows and never undoes it.
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
            "E_VALIDATE",
            "from_session_id and to_session_id must differ",
        ));
    }

    // Resolve both ends up-front: we need the sender's name/host for the pane
    // header, and we want unknown ids to fail before we write the row.
    let (from_row, to_row) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let from = s
            .get_session_by_id(args.from_session_id)
            .map_err(|e| IpcError::new("E_REPO", e.to_string()))?
            .ok_or_else(|| {
                IpcError::new(
                    "E_NOTFOUND",
                    format!("from session {} not found", args.from_session_id),
                )
            })?;
        let to = s
            .get_session_by_id(args.to_session_id)
            .map_err(|e| IpcError::new("E_REPO", e.to_string()))?
            .ok_or_else(|| {
                IpcError::new(
                    "E_NOTFOUND",
                    format!("to session {} not found", args.to_session_id),
                )
            })?;
        (from, to)
    };
    let kind = args.kind.as_deref().unwrap_or("message");

    // Inbox row — the source of truth.
    let id = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.insert_message(args.from_session_id, args.to_session_id, &args.body, kind)?
    };

    // Append timeline events on both ends — best-effort, never blocks the
    // send. Body is truncated to keep the timeline readable.
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let detail: String = args.body.chars().take(120).collect();
        let _ = s.insert_session_event(
            args.from_session_id,
            "message_sent",
            Some(&format!("to={} {}", args.to_session_id, detail)),
        );
        let _ = s.insert_session_event(
            args.to_session_id,
            "message_received",
            Some(&format!("from={} {}", args.from_session_id, detail)),
        );
    }

    let mut delivered_to_pane = false;
    let mut deliver_error: Option<String> = None;
    if args.deliver {
        // Header makes the source visible to the recipient. The id lets the
        // receiver correlate the pane line with the inbox entry.
        let header = format!(
            "[msg #{} from {}@{}]: {}",
            id, from_row.tmux_name, from_row.host_alias, args.body,
        );
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
#[derive(serde::Serialize)]
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
        .get_session_by_id(session_id)
        .map_err(|e| IpcError::new("E_REPO", e.to_string()))?
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
