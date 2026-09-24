//! PURE checks on what a peer sends. A failed check rejects one item, never
//! the whole exchange (except `check_batch`, which refuses the request).

use super::wire::{WireMessage, PEER_BATCH_MAX, PEER_BODY_MAX};
use crate::service::address::{self, Addr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub code: &'static str,
    pub message: String,
}

fn reject(code: &'static str, message: impl Into<String>) -> Rejection {
    Rejection {
        code,
        message: message.into(),
    }
}

/// The longest `kind` a peer may send. Cycle 1's kinds (`message`,
/// `task_result`) are well under it.
const PEER_KIND_MAX: usize = 32;

/// An item that passed: the recipient in our fleet, the sender's address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub from_addr: String,
    pub to_host: String,
    pub to_name: String,
}

pub fn check_fleet_id(id: &str) -> Result<(), Rejection> {
    match address::parse(&format!("{id}/hub")) {
        Ok(Addr::Hub { fleet: Some(f) }) if f == id => Ok(()),
        _ => Err(reject("E_VALIDATE", "fleet_id is not a fleet id")),
    }
}

pub fn check_batch(send: usize, results: usize) -> Result<(), Rejection> {
    if send > PEER_BATCH_MAX || results > PEER_BATCH_MAX {
        return Err(reject(
            "E_VALIDATE",
            format!("at most {PEER_BATCH_MAX} messages and {PEER_BATCH_MAX} results per exchange"),
        ));
    }
    Ok(())
}

/// A `kind` that may cross a hub link: 1 to 32 of `[a-z0-9_-]`. The sender
/// checks it too (`send_remote`), so a kind the peer would refuse is refused
/// up front rather than coming back as `message_undeliverable`.
pub fn check_kind(kind: &str) -> Result<(), Rejection> {
    if kind.is_empty()
        || kind.len() > PEER_KIND_MAX
        || !kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(reject(
            "E_VALIDATE",
            format!("kind must be 1 to {PEER_KIND_MAX} of [a-z0-9_-]"),
        ));
    }
    Ok(())
}

/// `peer_fleet` is the link's pinned fleet; `own_fleet` is ours.
pub fn check_inbound(
    m: &WireMessage,
    peer_fleet: &str,
    own_fleet: &str,
) -> Result<Checked, Rejection> {
    let from = address::parse(&m.from_addr)
        .map_err(|e| reject("E_VALIDATE", format!("from_addr: {}", e.message)))?;
    match &from {
        Addr::Session { fleet: Some(f), .. } if f == peer_fleet && f != own_fleet => {}
        _ => {
            return Err(reject(
                "E_FORBIDDEN",
                "from_addr must be a session of the linked fleet",
            ))
        }
    }
    let to = address::parse(&m.to_addr)
        .map_err(|e| reject("E_VALIDATE", format!("to_addr: {}", e.message)))?;
    let (to_host, to_name) = match to {
        Addr::Session {
            fleet: Some(f),
            host,
            name,
        } if f == own_fleet => (host, name),
        Addr::Session { .. } => {
            return Err(reject(
                "E_FORBIDDEN",
                "to_addr is not a session of this fleet",
            ))
        }
        _ => {
            return Err(reject(
                "E_VALIDATE",
                "only a session address can receive a message",
            ))
        }
    };
    check_kind(&m.kind)?;
    if m.body.is_empty() {
        return Err(reject("E_VALIDATE", "body is empty"));
    }
    if m.body.len() > PEER_BODY_MAX {
        return Err(reject(
            "E_VALIDATE",
            format!("body is over {PEER_BODY_MAX} bytes"),
        ));
    }
    Ok(Checked {
        from_addr: address::render(&from),
        to_host,
        to_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::peer::wire::WireMessage;

    fn m(from: &str, to: &str, body: &str) -> WireMessage {
        WireMessage {
            id: 1,
            from_addr: from.into(),
            to_addr: to.into(),
            body: body.into(),
            kind: "message".into(),
            reply_to: None,
            sent_at: 0,
            wake: false,
        }
    }

    #[test]
    fn a_good_item_checks_out() {
        let c = check_inbound(
            &m("fleet-a/session/h/a1", "fleet-b/session/h/b1", "hi"),
            "fleet-a",
            "fleet-b",
        )
        .unwrap();
        assert_eq!((c.to_host.as_str(), c.to_name.as_str()), ("h", "b1"));
        assert_eq!(c.from_addr, "fleet-a/session/h/a1");
    }

    #[test]
    fn a_peer_cannot_speak_for_another_fleet_or_ours() {
        for from in [
            "fleet-x/session/h/a1",
            "fleet-b/session/h/a1",
            "/session/h/a1",
        ] {
            let e = check_inbound(&m(from, "fleet-b/session/h/b1", "hi"), "fleet-a", "fleet-b")
                .unwrap_err();
            assert_eq!(e.code, "E_FORBIDDEN", "{from}");
        }
    }

    #[test]
    fn only_a_session_of_ours_receives() {
        for to in [
            "fleet-a/session/h/b1",
            "fleet-b/client/phone",
            "fleet-b/hub",
            "/session/h/b1",
        ] {
            let e = check_inbound(&m("fleet-a/session/h/a1", to, "hi"), "fleet-a", "fleet-b")
                .unwrap_err();
            assert!(
                e.code == "E_VALIDATE" || e.code == "E_FORBIDDEN",
                "{to}: {}",
                e.code
            );
        }
    }

    #[test]
    fn the_body_must_be_non_empty_and_capped() {
        let ok = "x".repeat(PEER_BODY_MAX);
        assert!(check_inbound(
            &m("fleet-a/session/h/a1", "fleet-b/session/h/b1", &ok),
            "fleet-a",
            "fleet-b"
        )
        .is_ok());
        for body in [String::new(), "x".repeat(PEER_BODY_MAX + 1)] {
            let e = check_inbound(
                &m("fleet-a/session/h/a1", "fleet-b/session/h/b1", &body),
                "fleet-a",
                "fleet-b",
            )
            .unwrap_err();
            assert_eq!(e.code, "E_VALIDATE");
        }
    }

    /// M1: `kind` is short, non-empty and `[a-z0-9_-]` — it is stored and
    /// rendered as-is, so a peer cannot stuff text or a line break into it.
    #[test]
    fn the_kind_must_be_a_short_token() {
        let mut ok = m("fleet-a/session/h/a1", "fleet-b/session/h/b1", "hi");
        for kind in ["message", "task_result", "a-b", &"k".repeat(32)] {
            ok.kind = kind.to_string();
            assert!(check_inbound(&ok, "fleet-a", "fleet-b").is_ok(), "{kind}");
        }
        for kind in ["", "Message", "a b", "kind\nx", "é", &"k".repeat(33)] {
            ok.kind = kind.to_string();
            let e = check_inbound(&ok, "fleet-a", "fleet-b").unwrap_err();
            assert_eq!(e.code, "E_VALIDATE", "{kind:?}");
        }
    }

    #[test]
    fn a_batch_over_the_cap_is_refused_whole() {
        assert!(check_batch(PEER_BATCH_MAX, PEER_BATCH_MAX).is_ok());
        assert!(check_batch(PEER_BATCH_MAX + 1, 0).is_err());
        assert!(check_batch(0, PEER_BATCH_MAX + 1).is_err());
    }

    #[test]
    fn a_fleet_id_must_be_a_fleet_segment() {
        assert!(check_fleet_id("0b8e7f2a-1c2d-4e5f-8a9b-0c1d2e3f4a5b").is_ok());
        for bad in ["", "a/b", "x".repeat(37).as_str(), "has space"] {
            assert!(check_fleet_id(bad).is_err(), "{bad:?}");
        }
    }
}
