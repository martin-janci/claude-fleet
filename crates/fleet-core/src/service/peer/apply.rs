//! Applying what a peer sent: inbound items become marked inbox rows, results
//! settle our outbox. Used by both sides of a link.
//!
//! A remote message's body never reaches a pane. The only text this module
//! types into one is [`wake_nudge`], which names the local message id and
//! nothing the peer controls; the test at the bottom pins that by source.

use super::validate::{check_inbound, Checked};
use super::wire::{ResultStatus, WireMessage, WireRef, WireResult};
use crate::ipc_error::{codes, lock, IpcError};
use crate::mcp::guard;
use crate::service::messages::{timeline_detail, wake_action, WakeAction};
use crate::ssh::SshClient;
use crate::store::{Inbound, OutboxRow, PeerLinkRow, SessionRow, Store};
use std::sync::{Arc, Mutex};

/// The ONLY text a remote message may type into a pane.
pub fn wake_nudge(local_id: i64) -> String {
    format!("[fleet] message #{local_id} from another fleet is in your inbox")
}

/// How a local message is named on the wire: by the peer's id if it came
/// from the peer, else by ours.
pub fn wire_ref_for(s: &Store, own_fleet: &str, local_id: i64) -> Result<WireRef, IpcError> {
    Ok(match s.remote_ref_of(local_id)? {
        Some((fleet, id)) => WireRef { fleet, id },
        None => WireRef {
            fleet: own_fleet.to_string(),
            id: local_id,
        },
    })
}

/// Our outbox rows as the peer receives them. The body goes WITHOUT this
/// hub's untrusted marker: the receiving hub marks it itself, with the
/// sender's address, and a marker naming one of our own sessions or clients
/// would mean nothing there.
pub fn outbox_to_wire(
    s: &Store,
    own_fleet: &str,
    rows: Vec<OutboxRow>,
) -> Result<Vec<WireMessage>, IpcError> {
    rows.into_iter()
        .map(|r| {
            Ok(WireMessage {
                id: r.id,
                from_addr: r.from_addr,
                to_addr: r.to_addr,
                body: strip_markers(&r.body).to_string(),
                kind: r.kind,
                reply_to: match r.reply_to {
                    Some(p) => Some(wire_ref_for(s, own_fleet, p)?),
                    None => None,
                },
                sent_at: r.sent_at,
                wake: r.wake,
            })
        })
        .collect()
}

/// Settle our outbox from a peer's per-item results: `accepted` rows leave
/// `pending`, a `rejected` row becomes `undeliverable` with a
/// `message_undeliverable` event for its sender. Applying one twice is a
/// no-op (both only touch `pending` rows).
pub fn apply_results(store: &Mutex<Store>, results: &[WireResult]) -> Result<(), IpcError> {
    let s = lock(store)?;
    let accepted: Vec<i64> = results
        .iter()
        .filter(|r| r.status == ResultStatus::Accepted)
        .map(|r| r.id)
        .collect();
    s.mark_peer_accepted(&accepted)?;
    for r in results
        .iter()
        .filter(|r| r.status == ResultStatus::Rejected)
    {
        let reason = format!(
            "{}: {}",
            r.code.as_deref().unwrap_or(codes::E_INTERNAL),
            r.message.as_deref().unwrap_or("refused by the peer hub")
        );
        s.mark_peer_undeliverable(r.id, &reason)?;
    }
    Ok(())
}

/// Insert each item the peer sent, one transaction per item, and wake an
/// idle recipient with [`wake_nudge`] after the store lock is released.
/// Returns one result per item, in order; a failed item is a `rejected`
/// result, never a failed exchange.
pub async fn apply_inbound(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    link: &PeerLinkRow,
    own_fleet: &str,
    items: &[WireMessage],
) -> Vec<WireResult> {
    let peer_fleet = link.fleet_id.clone().unwrap_or_default();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        // `apply_one` is sync and returns its guard with it: nothing below
        // runs under the store lock.
        match apply_one(store, link, &peer_fleet, own_fleet, item) {
            Ok(Applied::Inserted { local_id, to }) => {
                let status = to.claude_status.as_deref();
                if item.wake
                    && wake_action(false, status, to.stuck_kind.is_some()) == WakeAction::Paste
                {
                    let nudge = wake_nudge(local_id);
                    // Best-effort: the message is already in the inbox, and
                    // the recipient's next hook carries it either way.
                    let _ = crate::service::sessions::send_system_prompt(
                        &to.host_alias,
                        &to.tmux_name,
                        &nudge,
                        true,
                        store,
                        ssh,
                    )
                    .await;
                }
                out.push(WireResult::accepted(item.id));
            }
            Ok(Applied::Duplicate) => out.push(WireResult::accepted(item.id)),
            Err((code, message)) => out.push(WireResult::rejected(item.id, code, message)),
        }
    }
    out
}

/// What one item came to. A duplicate never wakes: its first arrival did.
enum Applied {
    Inserted { local_id: i64, to: Box<SessionRow> },
    Duplicate,
}

type Refusal = (&'static str, String);

fn internal(e: IpcError) -> Refusal {
    (codes::E_INTERNAL, e.message)
}

/// Every leading untrusted-marker line, not only the first: a peer that
/// stacked two would otherwise keep one of its own under ours.
fn strip_markers(mut text: &str) -> &str {
    loop {
        let next = guard::strip_marker(text);
        if next.len() == text.len() {
            return text;
        }
        text = next;
    }
}

fn apply_one(
    store: &Mutex<Store>,
    link: &PeerLinkRow,
    peer_fleet: &str,
    own_fleet: &str,
    item: &WireMessage,
) -> Result<Applied, Refusal> {
    let Checked {
        from_addr,
        to_host,
        to_name,
    } = check_inbound(item, peer_fleet, own_fleet).map_err(|r| (r.code, r.message))?;
    let s = lock(store).map_err(internal)?;
    // A resend of an item we already hold is accepted again before any
    // other check: its first arrival passed them, and a recipient retired
    // since must not turn a delivered message into an `undeliverable` one.
    if s.local_id_for_remote(peer_fleet, item.id)
        .map_err(internal)?
        .is_some()
    {
        return Ok(Applied::Duplicate);
    }
    let row = s
        .get_session(&to_name, &to_host)
        .map_err(|e| internal(e.into()))?
        .ok_or_else(|| {
            (
                codes::E_PARTICIPANT_UNKNOWN,
                format!("no session {to_name} on {to_host}"),
            )
        })?;
    if let Some(p) = s.participant_for_session(row.id).map_err(internal)? {
        if p.retired_at.is_some() {
            return Err((
                codes::E_PARTICIPANT_RETIRED,
                format!("session {to_name} on {to_host} is gone"),
            ));
        }
    }
    let reply_to = match &item.reply_to {
        None => None,
        Some(r) => Some(map_reply_to(&s, r, peer_fleet, own_fleet, row.id)?),
    };
    let text = strip_markers(&item.body);
    let body = guard::mark_untrusted(text, &format!("{from_addr} over a hub link"));
    // The excerpt is taken from the text, not the marked body: the marker
    // alone would fill it. `from=` names the address the text came from.
    let detail = format!("from={from_addr} {}", timeline_detail(text));
    let outcome = s
        .atomically(|s| {
            let from_p = s.ensure_remote_participant(link.id, &from_addr)?;
            let got = s.insert_inbound_remote(
                peer_fleet, item.id, from_p, row.id, &body, &item.kind, reply_to,
            )?;
            if let Inbound::Inserted(_) = got {
                s.insert_session_event(row.id, "message_received", Some(&detail))?;
            }
            Ok(got)
        })
        .map_err(internal)?;
    Ok(match outcome {
        Inbound::Inserted(local_id) => Applied::Inserted {
            local_id,
            to: Box::new(row),
        },
        Inbound::Duplicate(_) => Applied::Duplicate,
    })
}

/// A `reply_to` names a message on one of the link's two fleets; anything
/// else, or a parent the recipient took no part in, is `E_INVALID`.
fn map_reply_to(
    s: &Store,
    r: &WireRef,
    peer_fleet: &str,
    own_fleet: &str,
    recipient: i64,
) -> Result<i64, Refusal> {
    let local = if r.fleet == own_fleet {
        s.get_message(r.id).map_err(internal)?.map(|m| m.id)
    } else if r.fleet == peer_fleet {
        s.local_id_for_remote(peer_fleet, r.id).map_err(internal)?
    } else {
        None
    };
    let local = local.ok_or_else(|| {
        (
            codes::E_INVALID,
            "reply_to names no message this hub has".to_string(),
        )
    })?;
    let involved = match s.participant_for_session(recipient).map_err(internal)? {
        Some(p) => s
            .message_involves_participant(local, p.id)
            .map_err(internal)?,
        None => false,
    };
    if !involved {
        return Err((
            codes::E_INVALID,
            "reply_to does not involve the recipient".into(),
        ));
    }
    Ok(local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::peer::testkit::*;
    use crate::service::peer::wire::*;

    #[test]
    fn the_nudge_names_only_the_local_id() {
        assert_eq!(
            wake_nudge(42),
            "[fleet] message #42 from another fleet is in your inbox"
        );
    }

    /// The wake path types into a real pane over SSH, which a unit test
    /// cannot reach (nor can the e2e: it needs a live, idle Claude). So the
    /// one wake call in this file is pinned by source: it is handed the
    /// nudge, never a body. `wake_nudge` above pins the nudge's text, and
    /// cycle 1's `wake_action` tests pin the guard.
    #[test]
    fn apply_never_hands_a_body_to_the_pane() {
        let src = include_str!("apply.rs");
        let call = src.find("send_system_prompt(").expect("the wake call");
        let args = &src[call..call + src[call..].find(')').unwrap()];
        assert!(args.contains("nudge"), "{args}");
        assert!(!args.contains("body"), "{args}");
    }

    #[tokio::test]
    async fn a_reply_to_maps_both_ways_and_an_unrelated_parent_is_rejected() {
        use crate::service::peer::listen::exchange;
        let (store, ssh) = hub("fleet-b");
        let b1 = session(&store, "b1");
        let b2 = session(&store, "b2");
        let b3 = session(&store, "b3");
        let c = peer_client(&store, "hub-a");
        let msg = |id: i64, reply_to: Option<WireRef>| WireMessage {
            id,
            from_addr: "fleet-a/session/h/a1".into(),
            to_addr: "fleet-b/session/local/b1".into(),
            body: format!("m{id}"),
            kind: "message".into(),
            reply_to,
            sent_at: 0,
            wake: false,
        };
        let req = |send: Vec<WireMessage>| ExchangeRequest {
            proto: PROTO,
            fleet_id: "fleet-a".into(),
            send,
            after: 0,
            results: vec![],
            wait_ms: 0,
        };
        // An inbound message from fleet-a (its id 5) to b1, and an unrelated
        // local message b2 -> b3.
        exchange(&store, &ssh, c, req(vec![msg(5, None)]))
            .await
            .unwrap();
        let first = store
            .lock()
            .unwrap()
            .local_id_for_remote("fleet-a", 5)
            .unwrap()
            .unwrap();
        let unrelated = store
            .lock()
            .unwrap()
            .insert_message(b2, b3, "x", "message", None)
            .unwrap();

        let resp = exchange(
            &store,
            &ssh,
            c,
            req(vec![
                msg(
                    6,
                    Some(WireRef {
                        fleet: "fleet-a".into(),
                        id: 5,
                    }),
                ),
                msg(
                    7,
                    Some(WireRef {
                        fleet: "fleet-b".into(),
                        id: unrelated,
                    }),
                ),
                msg(
                    8,
                    Some(WireRef {
                        fleet: "fleet-x".into(),
                        id: 1,
                    }),
                ),
            ]),
        )
        .await
        .unwrap();

        assert_eq!(resp.results[0], WireResult::accepted(6));
        let second = store
            .lock()
            .unwrap()
            .local_id_for_remote("fleet-a", 6)
            .unwrap()
            .unwrap();
        let row = store.lock().unwrap().get_message(second).unwrap().unwrap();
        assert_eq!(
            row.reply_to,
            Some(first),
            "the peer's id 5 maps to our own copy"
        );
        assert_eq!(
            resp.results[1].code.as_deref(),
            Some("E_INVALID"),
            "does not involve b1"
        );
        assert_eq!(
            resp.results[2].code.as_deref(),
            Some("E_INVALID"),
            "a third fleet"
        );
        let _ = b1;
    }
}
