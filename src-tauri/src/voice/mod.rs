//! The desktop as a voice source: the microphone (`capture`) converted to the
//! relay's PCM (`resample`), and the one claim this window holds.
//!
//! Standalone, a claim registers a `CpalSource` with fleet-core's
//! `registry()`, which the embedded server's `/voice/capture` reads. Paired
//! to a hub, it holds the hub's `/voice/source` websocket open
//! (`hub_source`): that open socket is the claim. One PTY is attached at a
//! time, so there is one claim at a time; claiming another session releases
//! the first.

pub mod capture;
pub mod hub_source;
// Only the cpal capture (macOS, Windows) converts; elsewhere it is unused.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
mod resample;

use fleet_core::service::voice::{registry, RevokeReason, VoiceSource};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio_util::sync::CancellationToken;

/// The event the UI follows: `{ session_id, state, error? }`, `state` one of
/// `claimed` / `capturing` / `stopped` / `released` / `error`. `stopped` is a
/// capture's end and says nothing about the claim (it can follow a
/// `released`); a `released` with an `error` is a claim taken elsewhere or
/// lapsed idle.
pub const STATE_EVENT: &str = "voice:state";

/// The owner a standalone claim is registered under.
const OWNER: &str = "desktop";

/// Sends one `STATE_EVENT` payload to the UI. The command builds it from
/// `tauri::Emitter::emit`; tests pass a closure.
pub type Emit = Arc<dyn Fn(serde_json::Value) + Send + Sync>;

pub fn payload(session_id: i64, state: &str, error: Option<String>) -> serde_json::Value {
    let mut v = serde_json::json!({ "session_id": session_id, "state": state });
    if let Some(e) = error {
        v["error"] = serde_json::Value::String(e);
    }
    v
}

/// The microphone of this machine, reporting its state for `session_id`.
pub fn cpal_source(session_id: i64, emit: Emit) -> Arc<dyn VoiceSource> {
    Arc::new(capture::CpalSource {
        on_state: Arc::new(move |state, error| emit(payload(session_id, state, error))),
    })
}

/// What the UI is told when a hub socket ends without this window
/// releasing it; `None` for a release (whoever released already told it).
fn hub_ended_report(
    ended: Result<hub_source::Ended, String>,
) -> Option<(&'static str, Option<String>)> {
    let text = |s: &str| Some(s.to_string());
    Some(match ended {
        // Unreachable: `run` answers Released only once `stop` is cancelled.
        Ok(hub_source::Ended::Released) => return None,
        Ok(hub_source::Ended::ClaimedElsewhere) => {
            ("released", text(RevokeReason::Replaced.text()))
        }
        Ok(hub_source::Ended::Idle) => ("released", text(RevokeReason::Expired.text())),
        Ok(hub_source::Ended::HubClosed) => {
            ("released", text("the hub closed the microphone connection"))
        }
        Err(e) => ("error", Some(e)),
    })
}

enum Active {
    Local {
        session_id: i64,
        claim_id: u64,
    },
    /// Cancelled exactly when this claim is released or replaced, always
    /// under `VoiceState`'s lock — so a hub socket that ends on its own can
    /// tell, under the same lock, whether it is still the active claim.
    Hub {
        session_id: i64,
        stop: CancellationToken,
    },
}

/// This window's one microphone claim (managed by Tauri).
#[derive(Default)]
pub struct VoiceState {
    inner: Arc<Mutex<Option<Active>>>,
}

fn lock(m: &Mutex<Option<Active>>) -> MutexGuard<'_, Option<Active>> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// End `active`'s claim and tell the UI. Called under the lock.
fn end(active: Active, emit: &Emit) {
    let session_id = match active {
        Active::Local {
            session_id,
            claim_id,
        } => {
            registry().release(session_id, claim_id);
            session_id
        }
        Active::Hub { session_id, stop } => {
            stop.cancel();
            session_id
        }
    };
    emit(payload(session_id, "released", None));
}

impl VoiceState {
    /// Standalone: make `source` the session's microphone in this process's
    /// registry, releasing whatever this window claimed before.
    pub fn claim_local(&self, session_id: i64, source: Arc<dyn VoiceSource>, emit: &Emit) {
        let mut slot = lock(&self.inner);
        if let Some(prev) = slot.take() {
            end(prev, emit);
        }
        let claim_id = registry().claim(session_id, OWNER, source);
        *slot = Some(Active::Local {
            session_id,
            claim_id,
        });
        emit(payload(session_id, "claimed", None));
    }

    /// Paired: hold the hub's `/voice/source` open for the session, releasing
    /// whatever this window claimed before. Must be called inside a tokio
    /// runtime. When the hub ends the socket (another device claimed the
    /// microphone, the hub went away, an error), the claim is cleared and
    /// the UI told why.
    pub fn claim_hub(
        &self,
        base_url: &str,
        token: &str,
        session_id: i64,
        source: Arc<dyn VoiceSource>,
        emit: &Emit,
    ) {
        let mut slot = lock(&self.inner);
        if let Some(prev) = slot.take() {
            end(prev, emit);
        }
        let stop = CancellationToken::new();
        *slot = Some(Active::Hub {
            session_id,
            stop: stop.clone(),
        });
        emit(payload(session_id, "claimed", None));
        drop(slot);

        let inner = Arc::clone(&self.inner);
        let emit = Arc::clone(emit);
        let (base_url, token) = (base_url.to_string(), token.to_string());
        tokio::spawn(async move {
            let ended = hub_source::run(base_url, token, session_id, source, stop.clone()).await;
            let mut slot = lock(&inner);
            // Released or replaced: whoever did that already told the UI.
            if stop.is_cancelled() {
                return;
            }
            *slot = None;
            if let Some((state, error)) = hub_ended_report(ended) {
                emit(payload(session_id, state, error));
            }
        });
    }

    /// Release this window's claim, whichever session it is on. A no-op
    /// when there is none.
    pub fn release(&self, emit: &Emit) {
        if let Some(prev) = lock(&self.inner).take() {
            end(prev, emit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core::service::voice::PcmTx;

    struct Silent;
    impl VoiceSource for Silent {
        fn start(&self, _tx: PcmTx) -> Result<Box<dyn Send>, String> {
            Ok(Box::new(()))
        }
    }

    #[test]
    fn claiming_a_second_session_releases_the_first() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        let emit: Emit = Arc::new(move |v| seen.lock().unwrap().push(v));
        let state = VoiceState::default();
        let src: Arc<dyn VoiceSource> = Arc::new(Silent);

        state.claim_local(1, Arc::clone(&src), &emit);
        assert_eq!(registry().owner(1).as_deref(), Some("desktop"));
        state.claim_local(2, Arc::clone(&src), &emit);
        assert_eq!(registry().owner(1), None);
        assert_eq!(registry().owner(2).as_deref(), Some("desktop"));

        state.release(&emit);
        assert_eq!(registry().owner(2), None);
        state.release(&emit); // nothing left: no event

        let states: Vec<(i64, String)> = events
            .lock()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v["session_id"].as_i64().unwrap(),
                    v["state"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            states,
            [
                (1, "claimed".into()),
                (1, "released".into()),
                (2, "claimed".into()),
                (2, "released".into()),
            ]
        );
    }

    #[test]
    fn a_standalone_claim_taken_by_another_device_turns_the_mic_off_saying_why() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        let emit: Emit = Arc::new(move |v| seen.lock().unwrap().push(v));
        let state = VoiceState::default();
        state.claim_local(31, cpal_source(31, Arc::clone(&emit)), &emit);
        registry().claim(31, "client:phone", Arc::new(Silent));
        let last = events.lock().unwrap().last().cloned().unwrap();
        assert_eq!(
            last,
            payload(31, "released", Some("microphone claimed elsewhere".into()))
        );
        // This window's later release is a no-op on the registry: the
        // phone's claim stays.
        state.release(&emit);
        assert_eq!(registry().owner(31).as_deref(), Some("client:phone"));
    }

    #[test]
    fn a_hub_socket_that_ends_on_its_own_tells_the_ui_why() {
        use hub_source::Ended;
        let r = |e| hub_ended_report(e).map(|(s, err)| (s, err.unwrap_or_default()));
        assert_eq!(r(Ok(Ended::Released)), None);
        assert_eq!(
            r(Ok(Ended::ClaimedElsewhere)),
            Some(("released", "microphone claimed elsewhere".into()))
        );
        assert_eq!(
            r(Ok(Ended::Idle)),
            Some(("released", "microphone idle — turn 🎤 on again".into()))
        );
        assert_eq!(
            r(Ok(Ended::HubClosed)),
            Some((
                "released",
                "the hub closed the microphone connection".into()
            ))
        );
        assert_eq!(r(Err("boom".into())), Some(("error", "boom".into())));
    }
}
