//! Peer-to-peer messaging between Claude sessions.
//!
//! The `session_messages` table (migration 015) is the source of truth — every
//! send writes a row, and the recipient pulls when ready via `list_inbox`. An
//! optional real-time pane delivery is layered on top: when the sender sets
//! `deliver: true`, the message is *also* typed into the recipient's tmux
//! pane with a `[msg #id from name@host]:` header. Pane delivery is
//! best-effort — the inbox row lands regardless of what the SSH side does, so
//! callers can rely on the inbox.

use crate::ipc_error::lock;
use crate::ipc_error::{codes, IpcError};
use crate::service::pane_intel::ClaudeStatus;
use crate::service::sessions;
use crate::ssh::SshClient;
use crate::store::{SessionMessage, Store};
use std::sync::{Arc, Mutex};

#[derive(serde::Deserialize)]
pub struct SendMessageArgs {
    pub from_session_id: i64,
    pub to_session_id: i64,
    /// Fleet address of the recipient — `<fleet>/session/<host>/<name>`,
    /// `<fleet>/client/<name>` or `<fleet>/hub`. Alternative to
    /// `to_session_id`; when set, this wins. `Client` and `Hub` addresses
    /// parse but are refused as recipients this cycle (`E_UNSUPPORTED`) —
    /// the participant rows exist so a later cycle can route to them.
    #[serde(default)]
    pub to_addr: Option<String>,
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
    /// Nudge an IDLE recipient so it notices now instead of at its next
    /// turn. Only a recipient reported plainly `idle` (`claude_status`) is
    /// nudged: a `working` one gets the message from its own `Stop` hook; a
    /// `blocked` or otherwise stuck one, or one with no reported status at
    /// all (never hooked — often sitting on the first-run trust prompt), is
    /// never typed into. Skipped when `deliver` already pasted this same
    /// message. Defaults to false. See [`wake_action`] for the exact rule.
    #[serde(default)]
    pub wake: bool,
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
    /// Whether `wake` actually nudged the recipient's pane (only possible
    /// for an idle, unprompted session).
    pub woke: bool,
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
            codes::E_VALIDATE,
            "message body must be non-empty",
        ));
    }
    // Resolved BEFORE the self-target check and every downstream lookup: a
    // session addressing ITSELF by `to_addr` must not sail past
    // `E_SELF_TARGET` just because `args.to_session_id` (unused in the
    // address case) happens to be 0.
    let target = resolve_target(&args, store)?;
    let to_session_id = match target {
        Target::Local(id) => id,
        Target::Remote {
            link_id,
            addr,
            fleet,
        } => {
            return send_remote(args, link_id, &addr, &fleet, store);
        }
    };
    if args.from_session_id == to_session_id {
        return Err(IpcError::new(
            codes::E_SELF_TARGET,
            "from_session_id and to_session_id must differ",
        ));
    }
    let kind = args.kind.as_deref().unwrap_or("message");
    let detail = timeline_detail(&args.body);

    // Resolve both ends and write the row + events under one lock window and
    // in one transaction. We need the sender's name/host for the pane header,
    // and unknown ids must fail before anything is written.
    let (id, from_row, to_row) = {
        let s = lock(store)?;
        s.atomically(|s| {
            let from = s.get_session_by_id(args.from_session_id)?.ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("from session {} not found", args.from_session_id),
                )
            })?;
            let to = s.get_session_by_id(to_session_id)?.ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("to session {to_session_id} not found"),
                )
            })?;
            // A reply must point at a real message the sender took part in;
            // an arbitrary id would let an agent forge a thread.
            if let Some(parent_id) = args.reply_to {
                if s.get_message(parent_id)?.is_none() {
                    return Err(IpcError::new(
                        codes::E_NOTFOUND,
                        format!("reply_to message {parent_id} not found"),
                    ));
                }
                // By PARTICIPANT, not by raw session id (final review,
                // Important 3): a move creates a new `sessions` row and
                // kills the source, so keying on `parent.from_session_id` /
                // `parent.to_session_id` meant a moved session could not
                // reply to anything it had sent or received before the
                // move. The participant is the durable end of a thread.
                // A sender with no participant at all has never sent or
                // received anything, so it cannot be part of any thread.
                let mine = s.participant_for_session(args.from_session_id)?;
                let involved = match mine {
                    Some(p) => s.message_involves_participant(parent_id, p.id)?,
                    None => false,
                };
                if !involved {
                    return Err(IpcError::new(
                        codes::E_INVALID,
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
                to_session_id,
                &args.body,
                kind,
                args.reply_to,
            )?;
            // Timeline events on both ends.
            s.insert_session_event(
                args.from_session_id,
                "message_sent",
                Some(&format!("to={to_session_id} {detail}")),
            )?;
            s.insert_session_event(
                to_session_id,
                "message_received",
                Some(&format!("from={} {detail}", args.from_session_id)),
            )?;
            Ok((id, from, to))
        })?
    };

    let mut delivered_to_pane = false;
    let mut deliver_error: Option<String> = None;
    if args.deliver {
        if to_row.claude_status.as_deref() == Some(ClaudeStatus::Blocked.as_str())
            || to_row.stuck_kind.is_some()
        {
            deliver_error = Some(format!(
                "session {} is waiting on {}; the message is in its inbox but was not typed into the dialog",
                to_row.id,
                to_row.stuck_kind.as_deref().unwrap_or("a dialog")
            ));
        } else {
            let header = pane_header(id, &from_row.tmux_name, &from_row.host_alias, &args.body);
            match sessions::send_system_prompt(
                &to_row.host_alias,
                &to_row.tmux_name,
                &header,
                args.submit,
                store,
                ssh,
            )
            .await
            {
                Ok(()) => delivered_to_pane = true,
                Err(e) => deliver_error = Some(e.message),
            }
        }
    }

    // Wake-up is the ONLY remaining use of the paste primitive. Delivery
    // proper rides the hook response; this exists because an idle,
    // unprompted session never fires a hook and would otherwise not notice
    // at all. Skipped entirely when `deliver` already pasted this same
    // message — `deliver` and `wake` both existing to type into the pane
    // is not a reason to type it in twice.
    //
    // The guard mirrors the `deliver` branch above EXACTLY:
    // `claude_status == blocked` OR `stuck_kind.is_some()` refuses. Checking
    // `claude_status` alone is not enough — a Stop hook's `idle` is
    // preserved over a later pane read while `stuck_kind` is COALESCEd from
    // that pane read, so `claude_status: idle` with `stuck_kind:
    // Some(trust_prompt)` is reachable and real: wake would paste and press
    // Enter on a live trust prompt, approving something the operator never
    // approved. `None` (never hooked) is refused too, not treated as idle:
    // a never-hooked session is frequently sitting on the first-run trust
    // prompt with no pane read yet, so unknown is not safely idle either.
    let mut woke = false;
    if args.wake {
        match wake_action(
            delivered_to_pane,
            to_row.claude_status.as_deref(),
            to_row.stuck_kind.is_some(),
        ) {
            WakeAction::Skip => {}
            WakeAction::Refuse => {
                deliver_error = Some(merge_error(
                    deliver_error,
                    format!(
                        "recipient is waiting on {}; not typed into — the message is in its inbox",
                        to_row.stuck_kind.as_deref().unwrap_or("a dialog")
                    ),
                ));
            }
            WakeAction::RefuseUnknown => {
                deliver_error = Some(merge_error(
                    deliver_error,
                    "recipient's status is unknown (never hooked); not typed into — the message is in its inbox"
                        .to_string(),
                ));
            }
            WakeAction::Paste => {
                let header = pane_header(id, &from_row.tmux_name, &from_row.host_alias, &args.body);
                match sessions::send_system_prompt(
                    &to_row.host_alias,
                    &to_row.tmux_name,
                    &header,
                    true,
                    store,
                    ssh,
                )
                .await
                {
                    Ok(()) => woke = true,
                    Err(e) => deliver_error = Some(merge_error(deliver_error, e.message)),
                }
            }
        }
    }

    Ok(SendMessageResult {
        id,
        delivered_to_pane,
        deliver_error,
        woke,
    })
}

/// What `wake` should do, given the recipient's pane-delivery outcome and
/// reported status. PURE — no SSH, no store — so every branch is testable
/// without a real tmux server.
///
/// - `already_delivered` (this same call's `deliver` already pasted the
///   message) always wins: `deliver` and `wake` both existing to type into
///   the pane is not a reason to type it in twice.
/// - Otherwise this mirrors the `deliver` branch's own guard EXACTLY:
///   `claude_status == blocked` OR `stuck` refuses. `claude_status` alone is
///   not enough — a Stop hook's `idle` is preserved over a later pane read
///   while `stuck_kind` is COALESCEd from that pane read, so `idle` with a
///   `stuck_kind` is reachable and real: without this check wake would paste
///   and press Enter into a live dialog (e.g. approve a trust prompt the
///   operator never approved).
/// - `None` (never hooked) refuses too, not treated as idle: a never-hooked
///   session is often sitting on the first-run trust prompt with no pane
///   read yet, so unknown is not safely idle either.
/// - `working` / `completed` / `failed` / `stopped` are left alone: a
///   working session's own Stop hook will carry the message, and the
///   others are not usefully nudgeable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeAction {
    /// Nothing to do — already delivered, or a status this call does not
    /// act on (working / completed / failed / stopped).
    Skip,
    /// Refuse and report why: blocked, or stuck on any dialog.
    Refuse,
    /// Refuse and report why: status has never been reported at all.
    RefuseUnknown,
    /// Safe to paste: reported idle, not stuck, not already delivered.
    Paste,
}

pub(crate) fn wake_action(
    already_delivered: bool,
    claude_status: Option<&str>,
    stuck: bool,
) -> WakeAction {
    if already_delivered {
        return WakeAction::Skip;
    }
    if claude_status == Some(ClaudeStatus::Blocked.as_str()) || stuck {
        return WakeAction::Refuse;
    }
    match claude_status {
        Some(s) if s == ClaudeStatus::Idle.as_str() => WakeAction::Paste,
        Some(_) => WakeAction::Skip,
        None => WakeAction::RefuseUnknown,
    }
}

/// PURE: append a second error to a possibly-already-set one, so `deliver`
/// and `wake` failures on the same call are both visible rather than one
/// silently overwriting the other.
fn merge_error(existing: Option<String>, new: String) -> String {
    match existing {
        Some(prev) => format!("{prev}; {new}"),
        None => new,
    }
}

/// Where a `send_message` goes: a local session, or a peer hub's outbox.
enum Target {
    Local(i64),
    Remote {
        link_id: i64,
        addr: String,
        fleet: String,
    },
}

/// Resolve `args.to_session_id` / `args.to_addr` to where the message
/// actually goes. `to_addr` wins when both are set.
///
/// `Client` and `Hub` addresses parse but are refused as recipients this
/// cycle (`E_UNSUPPORTED`): the participant rows exist so a later cycle can
/// route to them, and refusing is honest about what is built. A foreign
/// fleet address with no live hub link is refused the same way, naming the
/// missing link; with a live link it resolves to `Target::Remote`.
fn resolve_target(args: &SendMessageArgs, store: &Mutex<Store>) -> Result<Target, IpcError> {
    let Some(raw) = args.to_addr.as_deref() else {
        return Ok(Target::Local(args.to_session_id));
    };
    let addr = crate::service::address::parse(raw)?;
    // The one place an address genuinely has to be compared against this
    // fleet's identity, so the one place that may mint it.
    let fleet = crate::service::address::ensure_local_fleet_id(store)?;
    if crate::service::address::is_foreign(&addr, &fleet) {
        let crate::service::address::Addr::Session {
            fleet: Some(peer), ..
        } = &addr
        else {
            return Err(IpcError::new(
                codes::E_UNSUPPORTED,
                "only session addresses can receive a message across a hub link",
            ));
        };
        let link = lock(store)?.live_peer_link_for_fleet(peer)?;
        return match link {
            Some(l) => Ok(Target::Remote {
                link_id: l.id,
                addr: crate::service::address::render(&addr),
                fleet: peer.clone(),
            }),
            None => Err(IpcError::new(
                codes::E_UNSUPPORTED,
                format!(
                    "that address names fleet {peer}, which this hub has no link to \
                     (an operator links hubs with `fleet-hub peer add`)"
                ),
            )),
        };
    }
    match addr {
        crate::service::address::Addr::Session { host, name, .. } => {
            let s = lock(store)?;
            let row = s.get_session(&name, &host)?.ok_or_else(|| {
                IpcError::new(
                    codes::E_PARTICIPANT_UNKNOWN,
                    format!("no session {name} on {host}"),
                )
            })?;
            if let Some(p) = s.participant_for_session(row.id)? {
                if p.retired_at.is_some() {
                    return Err(IpcError::new(
                        codes::E_PARTICIPANT_RETIRED,
                        format!("session {name} on {host} is gone"),
                    ));
                }
            }
            Ok(Target::Local(row.id))
        }
        crate::service::address::Addr::Client { .. }
        | crate::service::address::Addr::Hub { .. } => Err(IpcError::new(
            codes::E_UNSUPPORTED,
            "only session addresses can receive a message today",
        )),
    }
}

/// Queue a message for a peer hub. Nothing is typed into any pane: `deliver`
/// is refused, and the recipient's hub decides the wake (a fixed nudge,
/// never the body). The exchange loop picks the row up from the outbox.
/// `peer_fleet` is the fleet `to_addr` names (the link's).
fn send_remote(
    args: SendMessageArgs,
    link_id: i64,
    to_addr: &str,
    peer_fleet: &str,
    store: &Mutex<Store>,
) -> Result<SendMessageResult, IpcError> {
    if args.deliver {
        return Err(IpcError::new(
            codes::E_UNSUPPORTED,
            "deliver types into a pane; a hub never types into another fleet's panes",
        ));
    }
    if args.body.len() > crate::service::peer::wire::PEER_BODY_MAX {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!(
                "a message to another fleet is at most {} bytes",
                crate::service::peer::wire::PEER_BODY_MAX
            ),
        ));
    }
    let kind = args.kind.as_deref().unwrap_or("message");
    // The peer's own rules: refused here, not a week later as undeliverable.
    crate::service::peer::validate::check_kind_for_link(kind)
        .map_err(|r| IpcError::new(r.code, format!("to another fleet, {}", r.message)))?;
    crate::service::peer::validate::check_addr_len("the recipient address", to_addr)
        .map_err(|r| IpcError::new(r.code, format!("to another fleet, {}", r.message)))?;
    // Must be called before `lock(store)` below: it takes the store lock
    // internally, and the store mutex must never be locked while already
    // held.
    let fleet = crate::service::address::ensure_local_fleet_id(store)?;
    let detail = timeline_detail(&args.body);
    let s = lock(store)?;
    let id = s.atomically(|s| {
        // G8(b) (review): `client revoke` revokes only the `client_tokens`
        // row, never the link itself (a re-pair keeps pending rows
        // attached — Ruling 6), so a listener whose paired hub was revoked
        // kept queuing new outbound rows for that fleet indefinitely; a
        // parked exchange handler on the other side would keep serving them
        // for up to its own 25s window, and every message after that piled
        // up undeliverable. Refuse up front instead.
        if let Some(link) = s.peer_link(link_id)? {
            let listener_revoked = link.role == crate::store::LINK_ROLE_LISTENER
                && match link.client_id {
                    Some(client_id) => !s.client_token_is_live(client_id)?,
                    None => true,
                };
            if listener_revoked {
                return Err(IpcError::new(
                    codes::E_UNSUPPORTED,
                    format!("the link to fleet {peer_fleet} has been revoked on this hub"),
                ));
            }
        }
        let from = s.get_session_by_id(args.from_session_id)?.ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                format!("from session {} not found", args.from_session_id),
            )
        })?;
        let from_addr = crate::service::address::render(&crate::service::address::Addr::Session {
            fleet: Some(fleet.clone()),
            host: from.host_alias.clone(),
            name: from.tmux_name.clone(),
        });
        // G4 (review): only `to_addr` was capped here; an oversized
        // `from_addr` (an overlong host/tmux name) would queue fine and
        // only be refused a round trip later, at the peer's own
        // `check_addr_len` on receipt — this becomes the recipient's
        // hook-delivery sender label, which has the same hard budget.
        crate::service::peer::validate::check_addr_len("the sender address", &from_addr)
            .map_err(|r| IpcError::new(r.code, format!("to another fleet, {}", r.message)))?;
        let to_p = s.ensure_remote_participant(link_id, to_addr)?;
        if let Some(parent_id) = args.reply_to {
            // Sender-participation (cycle 1, unchanged by G2): "a reply
            // must point at a real message the sender took part in; an
            // arbitrary id would let an agent forge a thread." G2 ADDS a
            // condition on top of this — it does not replace it. A sender
            // with no participant at all has never sent or received
            // anything, so it cannot be part of any thread.
            let mine = s.participant_for_session(args.from_session_id)?;
            let involved = match mine {
                Some(p) => s.message_involves_participant(parent_id, p.id)?,
                None => false,
            };
            if !involved {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!(
                        "reply_to message {parent_id} does not involve session {}",
                        args.from_session_id
                    ),
                ));
            }
            // G2 (review): the sender-participation check above is
            // necessary but not sufficient — it says nothing about what the
            // RECEIVING peer actually validates. `wire_ref_for` sends a
            // local parent as `{own_fleet, id}`; the peer's own
            // `map_reply_to` accepts that shape only when the parent
            // involves ITS OWN recipient participant on this link. A parent
            // the sender took part in but never exchanged with `to_addr`
            // (e.g. a purely local thread) sailed past the sender check
            // alone, queued, and only came back a round trip later as
            // `message_undeliverable`.
            //
            // So ALSO require what the peer will recognise: a parent
            // exchanged with the SAME remote participant. G2b: "received
            // from the target fleet" is not enough — the peer requires the
            // parent to involve the specific recipient, so a reply to b1
            // onto b2's message would be rejected there. A parent the
            // recipient sent us already involves `to_p` (its
            // from_participant IS `to_p`), so this one test covers both the
            // received and the previously-sent parent.
            if !s.message_involves_participant(parent_id, to_p)? {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("reply_to must be a message exchanged with {to_addr}"),
                ));
            }
        }
        let id = s.insert_outbound_remote(
            args.from_session_id,
            &from_addr,
            to_p,
            &args.body,
            kind,
            args.reply_to,
            args.wake,
        )?;
        s.insert_session_event(
            args.from_session_id,
            "message_sent",
            Some(&format!("to={to_addr} {detail}")),
        )?;
        Ok(id)
    })?;
    Ok(SendMessageResult {
        id,
        delivered_to_pane: false,
        deliver_error: None,
        woke: false,
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
    let s = lock(store)?;
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
    let s = lock(store)?;
    let row = s.get_session_by_id(session_id)?.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("session {} not found", session_id),
        )
    })?;
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

/// Safety floor: a waiter re-reads at least this often even if no
/// notification arrives, so a missed signal costs latency, never the wait.
const REPLY_POLL_FLOOR: std::time::Duration = std::time::Duration::from_millis(500);

/// Bounded wait for the next message addressed to `session_id`, newer than
/// `after_message_id`. `Ok(None)` on timeout — a timeout is an outcome, not an
/// error, matching `wait_for_session`.
pub async fn wait_for_reply(
    store: &Mutex<Store>,
    session_id: i64,
    after_message_id: Option<i64>,
    timeout: std::time::Duration,
) -> Result<Option<SessionMessage>, IpcError> {
    let deadline = tokio::time::Instant::now() + timeout;
    // Take the handle (and validate the session) under one short lock window,
    // never across an await.
    let notify = {
        let s = lock(store)?;
        if s.get_session_by_id(session_id)?.is_none() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {session_id} not found"),
            ));
        }
        s.message_notify()
    };
    loop {
        {
            let s = lock(store)?;
            // Id-ordered, not `sent_at`-ordered (`list_inbox`'s order): a
            // waiter's job is "is there anything newer than the last id I
            // saw", and only `id` (an `INTEGER PRIMARY KEY`, monotonic by
            // construction) can answer that without depending on the wall
            // clock — a clock regression must never hide a genuinely newer
            // message for the rest of the timeout.
            if let Some(m) =
                s.newest_inbox_message_after(session_id, after_message_id.unwrap_or(0))?
            {
                return Ok(Some(m));
            }
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        // Wake on arrival; the floor bounds a missed signal.
        let _ = tokio::time::timeout(REPLY_POLL_FLOOR.min(deadline - now), notify.notified()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::sessions::{build_send_script, normalize_prompt_body};
    use std::time::Duration;

    fn seed(s: &Store, name: &str) -> i64 {
        s.upsert_host("local").unwrap();
        s.upsert_session(name, "local", None, None, 0, 0, "running", None)
            .unwrap()
    }

    fn args(from: i64, to: i64, body: &str) -> SendMessageArgs {
        SendMessageArgs {
            from_session_id: from,
            to_session_id: to,
            to_addr: None,
            body: body.to_string(),
            kind: None,
            deliver: false,
            submit: true,
            reply_to: None,
            wake: false,
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
    fn deliver_payload_is_the_header_pasted_via_load_buffer_then_enter() {
        // The transport (`send_prompt` → bash/ssh + tmux) is not exercised
        // here; this pins the tmux payload the deliver path hands to it.
        let header = pane_header(7, "alpha", "local", "it's done; $HOME `ok`");
        let body = normalize_prompt_body(&header).unwrap();
        let s = build_send_script("beta", None, &body, "buf", true);
        assert!(s.starts_with("set -o pipefail; t='=beta:'; "), "{s}");
        assert!(s.contains("load-buffer -b 'buf' -"), "{s}");
        assert!(s.contains("paste-buffer -p -d -b 'buf' -t \"$t\""), "{s}");
        assert!(
            s.trim_end().ends_with("tmux send-keys -t \"$t\" Enter"),
            "{s}"
        );
        // submit=false stages the text without pressing Enter.
        assert!(!build_send_script("beta", None, &body, "buf", false).contains("Enter"));
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
    async fn deliver_to_a_blocked_recipient_lands_in_the_inbox_but_is_not_typed() {
        let (store, ssh, a, b) = fixture();
        {
            let s = store.lock().unwrap();
            s.record_notification_hook_for_row(
                b,
                crate::service::pane_intel::ClaudeStatus::Blocked,
                None,
            )
            .unwrap();
        }
        let mut a_args = args(a, b, "ping");
        a_args.deliver = true;
        let res = send_message(a_args, &store, &ssh).await.unwrap();

        assert!(!res.delivered_to_pane);
        let err = res.deliver_error.expect("blocked recipient reports why");
        assert!(err.contains("inbox"), "{err}");

        let s = store.lock().unwrap();
        let inbox = s.list_inbox(b, false, 10).unwrap();
        assert_eq!(inbox.len(), 1, "the message still lands in the inbox");
        assert_eq!(inbox[0].body, "ping");
    }

    // ---- to_addr ----

    #[tokio::test]
    async fn send_accepts_an_address_instead_of_a_session_id() {
        let (store, ssh, a, b) = fixture();
        let mut m = args(a, 0, "by address");
        m.to_session_id = 0;
        m.to_addr = Some("/session/local/beta".into());
        let res = send_message(m, &store, &ssh).await.unwrap();
        let inbox = list_inbox(b, false, 10, false, &store).unwrap();
        assert_eq!(inbox[0].id, res.id);
        assert_eq!(inbox[0].body, "by address");
    }

    #[tokio::test]
    async fn an_address_naming_nothing_is_e_participant_unknown() {
        let (store, ssh, a, _b) = fixture();
        let mut m = args(a, 0, "nowhere");
        m.to_session_id = 0;
        m.to_addr = Some("/session/local/ghost".into());
        let err = send_message(m, &store, &ssh).await.unwrap_err();
        assert_eq!(err.code, "E_PARTICIPANT_UNKNOWN");
    }

    #[tokio::test]
    async fn a_malformed_address_is_e_validate_and_writes_nothing() {
        let (store, ssh, a, b) = fixture();
        let mut m = args(a, 0, "bad");
        m.to_session_id = 0;
        m.to_addr = Some("nonsense".into());
        let err = send_message(m, &store, &ssh).await.unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
        assert!(list_inbox(b, false, 10, false, &store).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_foreign_fleet_without_a_link_is_refused_naming_the_link() {
        let (store, ssh, a, _b) = fixture();
        let mut m = args(a, 0, "hi");
        m.to_session_id = 0;
        m.to_addr = Some("fleet-zz/session/h/b1".into());
        let e = send_message(m, &store, &ssh).await.unwrap_err();
        assert_eq!(e.code, codes::E_UNSUPPORTED);
        assert!(e.message.contains("fleet-hub peer add"), "{}", e.message);
    }

    #[tokio::test]
    async fn a_linked_foreign_address_queues_an_outbox_row() {
        let (store, ssh, a, _b) = fixture();
        let link = {
            let s = store.lock().unwrap();
            let id = s.insert_dialer_link("https://b.example", "t").unwrap();
            s.adopt_dialer_fleet(id, "fleet-b").unwrap()
        };
        let mut m = args(a, 0, "hi");
        m.to_session_id = 0;
        m.to_addr = Some("fleet-b/session/h/b1".into());
        m.wake = true;
        let r = send_message(m, &store, &ssh).await.unwrap();
        assert!(!r.delivered_to_pane && !r.woke);
        let s = store.lock().unwrap();
        let page = s.pending_outbox(link, 0, 50).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, r.id);
        assert_eq!(page[0].to_addr, "fleet-b/session/h/b1");
        assert!(page[0].wake);
        let fleet = s.get_setting("fleet.id").unwrap().unwrap();
        assert_eq!(page[0].from_addr, format!("{fleet}/session/local/alpha"));
        let ev = s.list_session_events(a, 10).unwrap();
        let sent = ev.iter().find(|e| e.kind == "message_sent").unwrap();
        assert!(sent
            .detail
            .as_deref()
            .unwrap()
            .starts_with("to=fleet-b/session/h/b1 "));
        let zero: i64 = s
            .conn_ref()
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id = 0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(zero, 0);
    }

    #[tokio::test]
    async fn deliver_to_a_remote_address_is_refused_and_a_big_body_too() {
        let (store, ssh, a, _b) = fixture();
        {
            let s = store.lock().unwrap();
            let id = s.insert_dialer_link("https://b.example", "t").unwrap();
            s.adopt_dialer_fleet(id, "fleet-b").unwrap();
        }
        let mut m = args(a, 0, "hi");
        m.to_session_id = 0;
        m.to_addr = Some("fleet-b/session/h/b1".into());
        m.deliver = true;
        assert_eq!(
            send_message(m, &store, &ssh).await.unwrap_err().code,
            codes::E_UNSUPPORTED
        );
        let mut big = args(a, 0, "hi");
        big.to_session_id = 0;
        big.to_addr = Some("fleet-b/session/h/b1".into());
        big.body = "x".repeat(crate::service::peer::wire::PEER_BODY_MAX + 1);
        assert_eq!(
            send_message(big, &store, &ssh).await.unwrap_err().code,
            codes::E_VALIDATE
        );
    }

    /// A kind the peer's `check_inbound` would reject is refused at the
    /// sender, before it queues; the default kind passes.
    #[tokio::test]
    async fn a_kind_the_peer_would_reject_is_refused_before_it_queues() {
        let (store, ssh, a, _b) = fixture();
        let link = {
            let s = store.lock().unwrap();
            let id = s.insert_dialer_link("https://b.example", "t").unwrap();
            s.adopt_dialer_fleet(id, "fleet-b").unwrap()
        };
        for bad in ["Task", "a b", "kind\nx", &"k".repeat(33)] {
            let mut m = args(a, 0, "hi");
            m.to_addr = Some("fleet-b/session/h/b1".into());
            m.kind = Some(bad.to_string());
            assert_eq!(
                send_message(m, &store, &ssh).await.unwrap_err().code,
                codes::E_VALIDATE,
                "{bad:?}"
            );
        }
        assert!(store
            .lock()
            .unwrap()
            .pending_outbox(link, 0, 50)
            .unwrap()
            .is_empty());
        let mut ok = args(a, 0, "hi");
        ok.to_addr = Some("fleet-b/session/h/b1".into());
        send_message(ok, &store, &ssh).await.unwrap();
        let mut result = args(a, 0, "done");
        result.to_addr = Some("fleet-b/session/h/b1".into());
        result.kind = Some("task_result".into());
        send_message(result, &store, &ssh).await.unwrap();
        let s = store.lock().unwrap();
        let kinds: Vec<String> = s
            .pending_outbox(link, 0, 50)
            .unwrap()
            .into_iter()
            .map(|r| r.kind)
            .collect();
        assert_eq!(kinds, vec!["message", "task_result"]);
    }

    /// I3 + M4: a recipient address over `PEER_ADDR_MAX`, and the kind
    /// `question` (which holds a session's stop), are refused before they
    /// queue — the peer would refuse both.
    #[tokio::test]
    async fn an_overlong_address_or_a_question_is_refused_before_it_queues() {
        let (store, ssh, a, _b) = fixture();
        let link = {
            let s = store.lock().unwrap();
            let id = s.insert_dialer_link("https://b.example", "t").unwrap();
            s.adopt_dialer_fleet(id, "fleet-b").unwrap()
        };
        let mut long = args(a, 0, "hi");
        long.to_addr = Some(format!(
            "fleet-b/session/h/{}",
            "n".repeat(crate::service::peer::wire::PEER_ADDR_MAX)
        ));
        let e = send_message(long, &store, &ssh).await.unwrap_err();
        assert_eq!(e.code, codes::E_VALIDATE, "{}", e.message);
        let mut q = args(a, 0, "hi");
        q.to_addr = Some("fleet-b/session/h/b1".into());
        q.kind = Some("question".into());
        let e = send_message(q, &store, &ssh).await.unwrap_err();
        assert_eq!(e.code, codes::E_VALIDATE, "{}", e.message);
        assert!(
            e.message.contains("cannot hold a session's stop"),
            "{}",
            e.message
        );
        assert!(store
            .lock()
            .unwrap()
            .pending_outbox(link, 0, 50)
            .unwrap()
            .is_empty());
    }

    /// M5: a reply across a link may thread onto a local message or one
    /// from the target fleet, never onto one from a third fleet — that would
    /// hand the peer a third fleet's id and message id.
    #[tokio::test]
    async fn a_reply_to_a_third_fleets_message_is_refused() {
        let (store, ssh, a, _b) = fixture();
        let (link_b, from_b, from_c) = {
            let s = store.lock().unwrap();
            let link_b = s.insert_dialer_link("https://b.example", "t").unwrap();
            s.adopt_dialer_fleet(link_b, "fleet-b").unwrap();
            let link_c = s.insert_dialer_link("https://c.example", "t2").unwrap();
            s.adopt_dialer_fleet(link_c, "fleet-c").unwrap();
            let pb = s
                .ensure_remote_participant(link_b, "fleet-b/session/h/b1")
                .unwrap();
            let pc = s
                .ensure_remote_participant(link_c, "fleet-c/session/h/c1")
                .unwrap();
            let crate::store::Inbound::Inserted(from_b) = s
                .insert_inbound_remote("fleet-b", 7, pb, a, "from b", "message", None)
                .unwrap()
            else {
                panic!("inserted")
            };
            let crate::store::Inbound::Inserted(from_c) = s
                .insert_inbound_remote("fleet-c", 7, pc, a, "from c", "message", None)
                .unwrap()
            else {
                panic!("inserted")
            };
            (link_b, from_b, from_c)
        };
        let reply = |parent: i64| {
            let mut m = args(a, 0, "re");
            m.to_addr = Some("fleet-b/session/h/b1".into());
            m.reply_to = Some(parent);
            m
        };
        let e = send_message(reply(from_c), &store, &ssh).await.unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{}", e.message);
        assert!(store
            .lock()
            .unwrap()
            .pending_outbox(link_b, 0, 50)
            .unwrap()
            .is_empty());
        send_message(reply(from_b), &store, &ssh).await.unwrap();
    }

    /// G2 (review): the OLD check asked only whether the parent involved the
    /// SENDER's own participant — never whether it was ever exchanged with
    /// `to_addr`, which is what the RECEIVING peer actually validates
    /// (`map_reply_to`). A purely local parent (sender's own thread, never
    /// touching the target fleet) sailed past that check, queued, and only
    /// bounced back a round trip later as `message_undeliverable`. It must
    /// now be refused before anything is queued.
    #[tokio::test]
    async fn a_reply_to_a_purely_local_parent_is_refused_before_it_queues() {
        let (store, ssh, a, b) = fixture();
        let parent = send_message(args(a, b, "purely local"), &store, &ssh)
            .await
            .unwrap()
            .id;
        let link = {
            let s = store.lock().unwrap();
            let id = s.insert_dialer_link("https://b.example", "t").unwrap();
            s.adopt_dialer_fleet(id, "fleet-b").unwrap()
        };
        let mut reply = args(a, 0, "re");
        reply.to_addr = Some("fleet-b/session/h/b1".into());
        reply.reply_to = Some(parent);
        let e = send_message(reply, &store, &ssh).await.unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{}", e.message);
        assert!(
            e.message
                .contains("reply_to must be a message exchanged with"),
            "{}",
            e.message
        );
        assert!(store
            .lock()
            .unwrap()
            .pending_outbox(link, 0, 50)
            .unwrap()
            .is_empty());
    }

    /// A parent this hub RECEIVED from the target fleet is accepted: the
    /// peer will recognise its own `{peer_fleet, id}` on the wire.
    #[tokio::test]
    async fn a_reply_to_a_parent_received_from_the_target_fleet_is_accepted() {
        let (store, ssh, a, _b) = fixture();
        let (link, from_b) = {
            let s = store.lock().unwrap();
            let link = s.insert_dialer_link("https://b.example", "t").unwrap();
            s.adopt_dialer_fleet(link, "fleet-b").unwrap();
            let pb = s
                .ensure_remote_participant(link, "fleet-b/session/h/b1")
                .unwrap();
            let crate::store::Inbound::Inserted(from_b) = s
                .insert_inbound_remote("fleet-b", 1, pb, a, "from b", "message", None)
                .unwrap()
            else {
                panic!("inserted")
            };
            (link, from_b)
        };
        let mut reply = args(a, 0, "re");
        reply.to_addr = Some("fleet-b/session/h/b1".into());
        reply.reply_to = Some(from_b);
        send_message(reply, &store, &ssh).await.unwrap();
        assert_eq!(
            store
                .lock()
                .unwrap()
                .pending_outbox(link, 0, 50)
                .unwrap()
                .len(),
            1
        );
    }

    /// G2b (review): a parent received from the target FLEET is not enough —
    /// the receiver requires the parent to involve the specific recipient.
    /// `a` replying to b1 onto a message b2 sent would queue, then come back
    /// `message_undeliverable` a round trip later. Refused at send instead.
    #[tokio::test]
    async fn a_reply_to_b1_onto_a_message_from_b2_is_refused_before_it_queues() {
        let (store, ssh, a, _b) = fixture();
        let (link, from_b2) = {
            let s = store.lock().unwrap();
            let link = s.insert_dialer_link("https://b.example", "t").unwrap();
            s.adopt_dialer_fleet(link, "fleet-b").unwrap();
            let pb2 = s
                .ensure_remote_participant(link, "fleet-b/session/h/b2")
                .unwrap();
            let crate::store::Inbound::Inserted(from_b2) = s
                .insert_inbound_remote("fleet-b", 1, pb2, a, "from b2", "message", None)
                .unwrap()
            else {
                panic!("inserted")
            };
            (link, from_b2)
        };
        let mut reply = args(a, 0, "re");
        reply.to_addr = Some("fleet-b/session/h/b1".into());
        reply.reply_to = Some(from_b2);
        let e = send_message(reply, &store, &ssh).await.unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{}", e.message);
        assert!(
            e.message
                .contains("reply_to must be a message exchanged with"),
            "{}",
            e.message
        );
        assert!(store
            .lock()
            .unwrap()
            .pending_outbox(link, 0, 50)
            .unwrap()
            .is_empty());
    }

    /// A parent this hub already SENT to the same remote recipient is
    /// accepted too — the peer will recognise its own `{own_fleet, id}`
    /// against a row it received on this link, whoever locally sends the
    /// follow-up.
    #[tokio::test]
    async fn a_reply_to_a_parent_previously_sent_to_the_same_remote_recipient_is_accepted() {
        let (store, ssh, a, _b) = fixture();
        let link = {
            let s = store.lock().unwrap();
            let id = s.insert_dialer_link("https://b.example", "t").unwrap();
            s.adopt_dialer_fleet(id, "fleet-b").unwrap()
        };
        let mut first = args(a, 0, "hi b");
        first.to_addr = Some("fleet-b/session/h/b1".into());
        let parent = send_message(first, &store, &ssh).await.unwrap().id;
        let mut reply = args(a, 0, "re");
        reply.to_addr = Some("fleet-b/session/h/b1".into());
        reply.reply_to = Some(parent);
        send_message(reply, &store, &ssh).await.unwrap();
        assert_eq!(
            store
                .lock()
                .unwrap()
                .pending_outbox(link, 0, 50)
                .unwrap()
                .len(),
            2
        );
    }

    /// G2 correction: the sender-participation check from cycle 1 ("a reply
    /// must point at a real message the sender took part in; an arbitrary
    /// id would let an agent forge a thread") is NOT replaced by the
    /// target-recipient check above — G2 ADDS a condition, it does not
    /// substitute one. A session that never took part in a thread must
    /// stay refused even when that thread happens to satisfy the new
    /// target check (it was exchanged with the same remote recipient):
    /// otherwise any local session could forge a reply onto any OTHER
    /// session's remote conversation just by naming its id.
    #[tokio::test]
    async fn a_session_cannot_thread_onto_a_message_it_took_no_part_in_even_when_the_target_matches(
    ) {
        let (store, ssh, a, b) = fixture();
        let link = {
            let s = store.lock().unwrap();
            let id = s.insert_dialer_link("https://b.example", "t").unwrap();
            s.adopt_dialer_fleet(id, "fleet-b").unwrap()
        };
        // `a` exchanges with the remote recipient — this is the parent `b`
        // will try to forge a reply onto.
        let mut first = args(a, 0, "hi b");
        first.to_addr = Some("fleet-b/session/h/b1".into());
        let parent = send_message(first, &store, &ssh).await.unwrap().id;
        // Give `b` a participant row of its own (an unrelated local
        // message), so the refusal below is specifically about `b` not
        // being part of THIS thread — not about `b` having no participant
        // at all.
        send_message(args(b, a, "unrelated"), &store, &ssh)
            .await
            .unwrap();
        // `b` never sent or received `parent`; it must be refused even
        // though `parent` WAS exchanged with the same remote recipient
        // `b` is now addressing.
        let mut forged = args(b, 0, "sneaky reply");
        forged.to_addr = Some("fleet-b/session/h/b1".into());
        forged.reply_to = Some(parent);
        let e = send_message(forged, &store, &ssh).await.unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{}", e.message);
        assert!(
            e.message.contains(&format!("does not involve session {b}")),
            "{}",
            e.message
        );
        // Only `a`'s own send queued; `b`'s forged reply must not have.
        assert_eq!(
            store
                .lock()
                .unwrap()
                .pending_outbox(link, 0, 50)
                .unwrap()
                .len(),
            1
        );
    }

    /// G4 (review): only `to_addr` was capped; an oversized `from_addr` (an
    /// overlong tmux name) queued fine and would only be refused a round
    /// trip later, at the peer's own `check_addr_len` on receipt.
    #[tokio::test]
    async fn an_oversized_from_addr_is_refused_before_it_queues() {
        let (store, ssh, _a, _b) = fixture();
        let long_name = "n".repeat(crate::service::peer::wire::PEER_ADDR_MAX);
        let long_sender = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_session(&long_name, "local", None, None, 0, 0, "running", None)
                .unwrap()
        };
        let link = {
            let s = store.lock().unwrap();
            let id = s.insert_dialer_link("https://b.example", "t").unwrap();
            s.adopt_dialer_fleet(id, "fleet-b").unwrap()
        };
        let mut m = args(long_sender, 0, "hi");
        m.to_addr = Some("fleet-b/session/h/b1".into());
        let e = send_message(m, &store, &ssh).await.unwrap_err();
        assert_eq!(e.code, codes::E_VALIDATE, "{}", e.message);
        assert!(store
            .lock()
            .unwrap()
            .pending_outbox(link, 0, 50)
            .unwrap()
            .is_empty());
    }

    /// G8(b) (review): a listener link survives `client revoke` — only the
    /// `client_tokens` row is revoked, never the link itself, because a
    /// re-pair keeps pending rows attached. Without this check a send
    /// through the now-revoked link would keep queuing forever, with no way
    /// for the far end (which will never poll again) to ever see it.
    #[tokio::test]
    async fn a_send_through_a_revoked_listener_link_is_refused() {
        let (store, ssh, a, _b) = fixture();
        let link = {
            let s = store.lock().unwrap();
            let client = s.insert_client_token("hub-b", "sha-hub-b", "peer").unwrap();
            let row = s.ensure_listener_link(client.id, "fleet-b").unwrap();
            s.revoke_client_token("hub-b").unwrap();
            row.id
        };
        let mut m = args(a, 0, "hi");
        m.to_addr = Some("fleet-b/session/h/b1".into());
        let e = send_message(m, &store, &ssh).await.unwrap_err();
        assert_eq!(e.code, codes::E_UNSUPPORTED, "{}", e.message);
        assert!(e.message.contains("revoked"), "{}", e.message);
        assert!(store
            .lock()
            .unwrap()
            .pending_outbox(link, 0, 50)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn addressing_yourself_by_address_is_e_self_target() {
        let (store, ssh, a, _b) = fixture();
        let mut m = args(a, 0, "hi me");
        m.to_session_id = 0;
        m.to_addr = Some("/session/local/alpha".into());
        let err = send_message(m, &store, &ssh).await.unwrap_err();
        assert_eq!(err.code, "E_SELF_TARGET");
    }

    // ---- wake ----

    #[tokio::test]
    async fn wake_is_skipped_for_a_working_recipient_because_the_hook_will_carry_it() {
        let (store, ssh, a, b) = fixture();
        {
            let s = store.lock().unwrap();
            s.record_notification_hook_for_row(
                b,
                crate::service::pane_intel::ClaudeStatus::Working,
                None,
            )
            .unwrap();
        }
        let mut m = args(a, b, "later");
        m.wake = true;
        let res = send_message(m, &store, &ssh).await.unwrap();
        assert!(
            !res.woke,
            "a working session gets the message from its Stop hook"
        );
        assert_eq!(list_inbox(b, true, 10, false, &store).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn wake_refuses_a_blocked_recipient_and_says_why() {
        let (store, ssh, a, b) = fixture();
        {
            let s = store.lock().unwrap();
            s.record_notification_hook_for_row(
                b,
                crate::service::pane_intel::ClaudeStatus::Blocked,
                None,
            )
            .unwrap();
        }
        let mut m = args(a, b, "do not answer the dialog");
        m.wake = true;
        let res = send_message(m, &store, &ssh).await.unwrap();
        assert!(!res.woke);
        let err = res.deliver_error.expect("a blocked recipient reports why");
        assert!(err.contains("inbox"), "{err}");
        assert_eq!(
            list_inbox(b, true, 10, false, &store).unwrap().len(),
            1,
            "the message still lands in the inbox"
        );
    }

    #[tokio::test]
    async fn wake_is_not_attempted_when_wake_is_false() {
        let (store, ssh, a, b) = fixture();
        let res = send_message(args(a, b, "quiet"), &store, &ssh)
            .await
            .unwrap();
        assert!(!res.woke);
        assert_eq!(res.deliver_error, None);
    }

    /// Fix round 1 / CRITICAL 2: `claude_status: idle` with a `stuck_kind`
    /// set is reachable (a Stop hook's `idle` is preserved over a later
    /// pane read that COALESCEs `stuck_kind` from it) — checking
    /// `claude_status` alone would paste into, and press Enter on, a live
    /// trust prompt. This is the exact scenario the fix must refuse.
    #[tokio::test]
    async fn wake_refuses_a_stuck_but_not_blocked_recipient_and_says_why() {
        let (store, ssh, a, b) = fixture();
        {
            let s = store.lock().unwrap();
            s.record_notification_hook_for_row(
                b,
                crate::service::pane_intel::ClaudeStatus::Idle,
                Some(Some(crate::service::pane_intel::StuckKind::TrustPrompt)),
            )
            .unwrap();
        }
        let mut m = args(a, b, "do not approve anything for me");
        m.wake = true;
        let res = send_message(m, &store, &ssh).await.unwrap();
        assert!(
            !res.woke,
            "idle claude_status must not override a set stuck_kind"
        );
        let err = res
            .deliver_error
            .expect("a stuck-but-not-blocked recipient reports why");
        assert!(err.contains("inbox"), "{err}");
        assert_eq!(
            list_inbox(b, true, 10, false, &store).unwrap().len(),
            1,
            "the message still lands in the inbox"
        );
    }

    /// Fix round 1 / Important: a never-hooked session (`claude_status:
    /// None`) is frequently sitting on the first-run trust prompt with no
    /// pane read yet, so unknown must not be treated as safely idle.
    #[tokio::test]
    async fn wake_refuses_a_recipient_with_unknown_status_and_says_why() {
        let (store, ssh, a, b) = fixture();
        // No hook has ever landed for `b`: claude_status stays None.
        let mut m = args(a, b, "hello?");
        m.wake = true;
        let res = send_message(m, &store, &ssh).await.unwrap();
        assert!(!res.woke, "unknown status must not be treated as idle");
        let err = res
            .deliver_error
            .expect("an unknown-status recipient reports why");
        assert!(err.contains("inbox"), "{err}");
        assert_eq!(
            list_inbox(b, true, 10, false, &store).unwrap().len(),
            1,
            "the message still lands in the inbox"
        );
    }

    // ---- wake_action (pure decision table) ----

    #[test]
    fn wake_action_never_pastes_twice_when_deliver_already_did() {
        // `already_delivered` wins over every status, including one that
        // would otherwise Paste.
        for status in [None, Some("idle"), Some("blocked"), Some("working")] {
            for stuck in [false, true] {
                assert_eq!(
                    wake_action(true, status, stuck),
                    WakeAction::Skip,
                    "status={status:?} stuck={stuck}"
                );
            }
        }
    }

    #[test]
    fn wake_action_refuses_blocked_or_stuck_over_pastes_idle() {
        assert_eq!(
            wake_action(false, Some("blocked"), false),
            WakeAction::Refuse
        );
        // idle + stuck: `stuck` must win, not the idle claude_status.
        assert_eq!(wake_action(false, Some("idle"), true), WakeAction::Refuse);
        assert_eq!(
            wake_action(false, Some("blocked"), true),
            WakeAction::Refuse
        );
    }

    #[test]
    fn wake_action_pastes_only_plain_idle() {
        assert_eq!(wake_action(false, Some("idle"), false), WakeAction::Paste);
    }

    #[test]
    fn wake_action_refuses_unknown_status_rather_than_treating_it_as_idle() {
        assert_eq!(wake_action(false, None, false), WakeAction::RefuseUnknown);
        // Unknown status is refused even if (incoherently) stuck were also
        // set — RefuseUnknown, not the generic Refuse, so the caller sees
        // the more specific reason.
        assert_eq!(wake_action(false, None, true), WakeAction::Refuse);
    }

    #[test]
    fn wake_action_skips_a_working_or_terminal_status() {
        for status in ["working", "completed", "failed", "stopped"] {
            assert_eq!(wake_action(false, Some(status), false), WakeAction::Skip);
        }
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

    /// Final review, Important 3. `reply_to` validation used to key on the
    /// parent's raw `from_session_id` / `to_session_id`, but a move creates a
    /// NEW row and kills the source, so after a move a session could not
    /// reply to anything it had sent or received before it —
    /// `E_INVALID "reply_to message N does not involve session M"`. The
    /// durable identity is the participant, and this was the one read the
    /// participant conversion missed.
    #[tokio::test]
    async fn a_reply_still_works_after_the_replying_session_moved() {
        let (store, ssh, a, b) = fixture();
        let first = send_message(args(a, b, "question?"), &store, &ssh)
            .await
            .unwrap();
        // The move: a new row for the same endpoint, and the participant
        // re-pointed onto it exactly as `move_session::finalise` does.
        let moved = {
            let s = store.lock().unwrap();
            let moved = seed(&s, "beta-on-the-other-host");
            let p = s.participant_for_session(b).unwrap().unwrap();
            s.repoint_participant(p.id, moved).unwrap();
            moved
        };
        let mut reply = args(moved, a, "answer.");
        reply.reply_to = Some(first.id);
        let res = send_message(reply, &store, &ssh)
            .await
            .expect("a moved session must still be able to reply to its own thread");
        let inbox = list_inbox(a, false, 10, false, &store).unwrap();
        assert_eq!(inbox[0].id, res.id);
        assert_eq!(inbox[0].reply_to, Some(first.id));

        // The forgery guard still holds: a session that never took part in
        // the thread is refused, by participant just as by row id.
        let outsider = {
            let s = store.lock().unwrap();
            seed(&s, "delta")
        };
        let mut forged = args(outsider, a, "me too");
        forged.reply_to = Some(first.id);
        assert_eq!(
            send_message(forged, &store, &ssh).await.unwrap_err().code,
            "E_INVALID"
        );
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

    /// Timeline inserts push `session:event`; inside `atomically` those are
    /// held until COMMIT, so a rolled-back write announces nothing and a
    /// committed one announces everything, in order.
    #[test]
    fn atomically_emits_only_after_commit() {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let _ = bus.take();
        s.atomically(|s| {
            s.insert_session_event(a, "message_sent", Some("half"))?;
            assert!(bus.names().is_empty(), "held until commit");
            Err::<(), _>(IpcError::new("E_TEST", "boom"))
        })
        .unwrap_err();
        assert!(bus.take().is_empty(), "a rollback emits nothing");

        s.atomically(|s| {
            s.insert_session_event(a, "message_sent", None)?;
            s.insert_session_event(b, "message_received", None)
        })
        .unwrap();
        assert_eq!(
            bus.take(),
            vec![
                format!("session:event:{a}:message_sent"),
                format!("session:event:{b}:message_received"),
            ]
        );
        // Outside `atomically` emits go straight through again.
        s.insert_session_event(a, "killed", None).unwrap();
        assert_eq!(bus.names(), vec!["session:event"]);
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

    // ---- wait_for_reply ----

    #[tokio::test]
    async fn wait_for_reply_returns_immediately_when_one_already_waits() {
        let (store, ssh, a, b) = fixture();
        send_message(args(a, b, "early"), &store, &ssh)
            .await
            .unwrap();
        let got = wait_for_reply(&store, b, None, Duration::from_secs(5))
            .await
            .unwrap()
            .expect("the already-waiting message");
        assert_eq!(got.body, "early");
    }

    #[tokio::test]
    async fn wait_for_reply_wakes_on_a_message_that_arrives_while_waiting() {
        let (store, ssh, a, b) = fixture();
        let store = std::sync::Arc::new(store);
        let (s2, ssh2) = (store.clone(), ssh.clone());
        let sender = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            send_message(args(a, b, "late"), &s2, &ssh2).await.unwrap();
        });
        let started = std::time::Instant::now();
        let got = wait_for_reply(&store, b, None, Duration::from_secs(5))
            .await
            .unwrap()
            .expect("the message that arrived during the wait");
        sender.await.unwrap();
        assert_eq!(got.body, "late");
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "the wake must be event-driven, not a 500 ms poll: took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn wait_for_reply_times_out_with_none_rather_than_an_error() {
        let (store, _ssh, _a, b) = fixture();
        let got = wait_for_reply(&store, b, None, Duration::from_millis(150))
            .await
            .unwrap();
        assert!(got.is_none(), "a timeout is Ok(None), not an error");
    }

    #[tokio::test]
    async fn after_message_id_ignores_messages_the_caller_already_saw() {
        let (store, ssh, a, b) = fixture();
        let first = send_message(args(a, b, "one"), &store, &ssh).await.unwrap();
        let got = wait_for_reply(&store, b, Some(first.id), Duration::from_millis(150))
            .await
            .unwrap();
        assert!(
            got.is_none(),
            "the already-seen message must not satisfy the wait"
        );
        let second = send_message(args(a, b, "two"), &store, &ssh).await.unwrap();
        let got = wait_for_reply(&store, b, Some(first.id), Duration::from_secs(5))
            .await
            .unwrap()
            .expect("the newer message");
        assert_eq!(got.id, second.id);
    }

    #[tokio::test]
    async fn wait_for_reply_rejects_an_unknown_session() {
        let (store, _ssh, _a, _b) = fixture();
        let err = wait_for_reply(&store, 9999, None, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
    }
}
