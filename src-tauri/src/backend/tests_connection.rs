//! Tests for [`super`] (`backend::connection`).

use super::*;
use serde_json::{json, Value};
use std::sync::Mutex as StdMutex;

#[derive(Default)]
struct Recorder {
    seen: StdMutex<Vec<(&'static str, Value)>>,
}

impl RemoteEventSink for Recorder {
    fn emit_remote(&self, name: &'static str, payload: Value) {
        self.seen.lock().unwrap().push((name, payload));
    }
}

fn status(token: &str) -> (HubConnectionStatus, Arc<Recorder>) {
    let sink = Arc::new(Recorder::default());
    (HubConnectionStatus::remote(sink.clone(), token), sink)
}

#[test]
fn a_hub_client_starts_connecting_and_a_standalone_app_says_so() {
    let (s, _) = status("cl_t");
    assert_eq!(s.current(), HubConnection::Connecting);
    assert_eq!(
        HubConnectionStatus::standalone().current(),
        HubConnection::Standalone
    );
}

/// The banner needs the event; a window that mounts late needs `current`.
#[test]
fn a_report_is_remembered_and_emitted_as_the_connection_event() {
    let (s, sink) = status("cl_t");
    let state = HubConnection::Reconnecting {
        attempt: 2,
        retry_in_secs: 4,
        reason: "the hub closed the event stream".into(),
    };
    s.report(state.clone());
    assert_eq!(s.current(), state);
    assert_eq!(
        sink.seen.lock().unwrap().clone(),
        vec![(
            CONNECTION_EVENT,
            json!({
                "state": "reconnecting", "attempt": 2, "retry_in_secs": 4,
                "reason": "the hub closed the event stream"
            })
        )],
        "the frontend store reads exactly this shape"
    );
}

/// The reason crosses to the webview. It is built from transport errors and
/// the hub's own error bodies, which is where a proxy echoing the
/// Authorization header back would put the token.
#[test]
fn a_reason_never_carries_the_token_and_stays_one_capped_line() {
    let (s, sink) = status("cl_s3cret");
    let long = "x".repeat(5 * MAX_REASON);
    s.report(HubConnection::Offline {
        attempt: 1,
        retry_in_secs: 1,
        reason: format!("401 from proxy:\nAuthorization: Bearer cl_s3cret\r\n{long}"),
    });
    let HubConnection::Offline { reason, .. } = s.current() else {
        panic!("{:?}", s.current())
    };
    assert!(!reason.contains("cl_s3cret"), "{reason}");
    assert!(reason.contains("<redacted>"), "{reason}");
    assert!(
        !reason.contains('\n') && !reason.contains('\r'),
        "{reason:?}"
    );
    assert!(reason.chars().count() <= MAX_REASON + 1, "{}", reason.len());
    let emitted = sink.seen.lock().unwrap()[0].1.to_string();
    assert!(!emitted.contains("cl_s3cret"), "{emitted}");
}

/// The frontend's `HubConnection` type in `hub_connection.ts` reads these
/// two variants by their exact `state` tag and field names.
#[test]
fn the_skew_states_serialise_as_the_frontend_expects() {
    let (s, sink) = status("cl_t");
    s.report(HubConnection::HubTooOld {
        hub_contract: 0,
        min_contract: 2,
    });
    assert_eq!(
        sink.seen.lock().unwrap().last().unwrap(),
        &(
            CONNECTION_EVENT,
            json!({ "state": "hub_too_old", "hub_contract": 0, "min_contract": 2 })
        )
    );
    s.report(HubConnection::HubTooNew {
        hub_contract: 5,
        max_contract: 1,
    });
    assert_eq!(
        sink.seen.lock().unwrap().last().unwrap(),
        &(
            CONNECTION_EVENT,
            json!({ "state": "hub_too_new", "hub_contract": 5, "max_contract": 1 })
        )
    );
}

/// The contract verdict is a second, slower hand on the same value: what a
/// `ready` frame last said about the hub's row shapes, which a socket drop
/// does not re-judge. The call gate in `backend::remote` reads this, not
/// `current`.
#[test]
fn the_contract_verdict_outlives_the_state_and_only_a_hello_moves_it() {
    let (s, _) = status("cl_t");
    assert_eq!(s.contract_verdict(), None, "nothing judged yet");

    let skew = HubConnection::HubTooNew {
        hub_contract: 5,
        max_contract: 1,
    };
    s.report(skew.clone());
    assert_eq!(s.contract_verdict(), Some(skew.clone()));

    // The bridge loops after a skew, and its next iteration reports one of
    // these. Neither has read a `ready` frame, so neither may clear it.
    for state in [
        HubConnection::Offline {
            attempt: 1,
            retry_in_secs: 1,
            reason: "connection refused".into(),
        },
        HubConnection::Reconnecting {
            attempt: 2,
            retry_in_secs: 4,
            reason: "the hub closed the event stream".into(),
        },
        HubConnection::Connecting,
    ] {
        s.report(state.clone());
        assert_eq!(s.current(), state);
        assert_eq!(s.contract_verdict(), Some(skew.clone()), "{state:?}");
    }

    // `Connected` is reported exactly when a `ready` frame classifies the
    // hub in range, and is the only thing that clears the verdict.
    s.report(HubConnection::Connected);
    assert_eq!(s.contract_verdict(), None);
}

#[test]
fn a_standalone_status_emits_nothing() {
    let s = HubConnectionStatus::standalone();
    s.report(HubConnection::Connected);
    assert_eq!(
        s.current(),
        HubConnection::Standalone,
        "no bridge runs standalone, so nothing may move it"
    );
}

#[test]
fn the_status_debug_hides_the_token() {
    let (s, _) = status("cl_s3cret");
    assert!(!format!("{s:?}").contains("cl_s3cret"));
}
