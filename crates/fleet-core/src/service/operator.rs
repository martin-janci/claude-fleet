//! The UX agent's operator session: who it is, and the one rule that keeps it
//! from acting on itself.
//!
//! This is the ONLY module that knows the operator is special. Everything
//! else treats it as an ordinary session — which is the point: it gets the
//! transcript, the conversation view, restart, reboot survival and the
//! sidebar for free, and removing the feature means deleting this file.

use crate::ipc_error::{codes, IpcError};
use crate::store::Store;

/// `settings` key holding `"<host_alias>/<tmux_name>"` for the live operator
/// session. Absent until `ensure_operator` has run once.
pub const SETTING_OPERATOR_SESSION: &str = "operator.session";

/// Where the operator session lives. Identity is `(host, tmux name)` rather
/// than a row id because ids churn on re-discovery, exactly as the quick
/// switcher's MRU key does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorRef {
    pub host_alias: String,
    pub tmux_name: String,
}

/// PURE: render a reference for storage.
pub fn format_ref(r: &OperatorRef) -> String {
    format!("{}/{}", r.host_alias, r.tmux_name)
}

/// PURE: read a stored reference. Both halves must be non-empty — a
/// half-written value must resolve to "no operator", never to a reference
/// that guards the wrong session (or every session on a host).
pub fn parse_ref(raw: &str) -> Option<OperatorRef> {
    let (host, tmux) = raw.split_once('/')?;
    if host.is_empty() || tmux.is_empty() {
        return None;
    }
    Some(OperatorRef {
        host_alias: host.to_string(),
        tmux_name: tmux.to_string(),
    })
}

/// The recorded operator, if one has ever been created. A store error reads
/// as "none": the guard's job is to refuse acting on a KNOWN operator, and a
/// database that cannot answer has not named one.
pub fn operator_ref(store: &Store) -> Option<OperatorRef> {
    store
        .get_setting(SETTING_OPERATOR_SESSION)
        .ok()
        .flatten()
        .as_deref()
        .and_then(parse_ref)
}

/// Record the operator's whereabouts. Called once by `ensure_operator`.
pub fn set_operator_ref(store: &Store, r: &OperatorRef) -> Result<(), IpcError> {
    store
        .set_setting(SETTING_OPERATOR_SESSION, &format_ref(r))
        .map_err(|e| IpcError::new(codes::E_SQLITE, format!("record the operator session: {e}")))
}

/// Refuse a session-addressed operation aimed at the operator itself.
///
/// Without this, "tidy up the zombie sessions" ends the conversation that
/// asked for it — mid-sentence, with no one left to say what happened.
pub fn refuse_if_operator(
    store: &Store,
    host_alias: &str,
    tmux_name: &str,
    what: &str,
) -> Result<(), IpcError> {
    match operator_ref(store) {
        Some(r) if r.host_alias == host_alias && r.tmux_name == tmux_name => Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "{what} refused: {tmux_name} on {host_alias} is the UX agent's own session. \
                 Close the agent panel and act on it from the sidebar if you mean it."
            ),
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn a_reference_round_trips_and_a_malformed_one_is_no_reference() {
        let r = OperatorRef {
            host_alias: "mefistos".into(),
            tmux_name: "fleet-operator".into(),
        };
        assert_eq!(format_ref(&r), "mefistos/fleet-operator");
        assert_eq!(parse_ref("mefistos/fleet-operator"), Some(r));
        // A tmux name may not contain '/', so the first separator is the only
        // one — but a value with no separator, or an empty half, is not a
        // reference and must not resolve to one.
        assert_eq!(parse_ref("mefistos"), None);
        assert_eq!(parse_ref("/fleet-operator"), None);
        assert_eq!(parse_ref("mefistos/"), None);
        assert_eq!(parse_ref(""), None);
    }

    #[test]
    fn an_unrecorded_operator_guards_nothing() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(operator_ref(&s), None);
        refuse_if_operator(&s, "local", "anything", "kill_session")
            .expect("with no operator recorded, nothing is refused");
    }

    #[test]
    fn the_operator_refuses_to_be_acted_on_and_its_neighbours_do_not() {
        let s = Store::open_in_memory().unwrap();
        let r = OperatorRef {
            host_alias: "local".into(),
            tmux_name: "fleet-operator".into(),
        };
        set_operator_ref(&s, &r).unwrap();
        assert_eq!(operator_ref(&s), Some(r));

        let err = refuse_if_operator(&s, "local", "fleet-operator", "kill_session")
            .expect_err("the operator must refuse to be killed");
        assert_eq!(err.code, crate::ipc_error::codes::E_FORBIDDEN);
        assert!(
            err.message.contains("kill_session"),
            "the refusal names what was attempted: {}",
            err.message
        );

        // Same name on another host, and another name on the same host, are
        // ordinary sessions.
        refuse_if_operator(&s, "mefistos", "fleet-operator", "kill_session").unwrap();
        refuse_if_operator(&s, "local", "blue-sirius", "kill_session").unwrap();
    }
}
