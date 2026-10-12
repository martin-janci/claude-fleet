//! `start:progress` (redesign step 5.13): the three real steps a
//! `new_session` goes through — worktree, tmux, agent — reported as they
//! happen, so the Pulse sequence a client shows advances on the start's own
//! events rather than on a guess.
//!
//! The frames are keyed by the caller's `start_token` (an opaque string it
//! minted) and carry nothing else: no host, no session name, no person. A
//! caller that passes no token gets no frames — the MCP agents and the
//! internal create paths do not ask for them.

use super::*;
use crate::events::{MoveStepState, StartProgress, StartStep};
use crate::ipc_error::codes;
use std::sync::atomic::{AtomicU8, Ordering};

/// Longest `start_token` accepted. A client mints something like
/// `st-<time>-<random>`; anything longer is not a token.
pub const START_TOKEN_MAX: usize = 64;

/// A `start_token` is 1–64 ASCII letters, digits, `-` or `_`: it rides an
/// event frame to every stream that subscribes to `start`, so it is held to
/// a shape that cannot carry anything but an id.
pub fn validate_start_token(token: &str) -> Result<(), IpcError> {
    let ok = !token.is_empty()
        && token.len() <= START_TOKEN_MAX
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            format!("start_token must be 1–{START_TOKEN_MAX} letters, digits, '-' or '_'"),
        ))
    }
}

/// Reports one start's step boundaries on the store's bus. `advance` closes
/// the step in flight (`done`) and opens the next (`started`); `finish`
/// closes the last one; `fail` marks the step in flight `failed`. Without a
/// token every call is a no-op.
pub(crate) struct StartReporter<'a> {
    store: &'a Mutex<Store>,
    token: Option<String>,
    /// 0 = no step in flight, else `StartStep::index()`.
    at: AtomicU8,
}

impl<'a> StartReporter<'a> {
    pub(crate) fn new(store: &'a Mutex<Store>, token: Option<String>) -> Self {
        Self {
            store,
            token,
            at: AtomicU8::new(0),
        }
    }

    fn current(&self) -> Option<StartStep> {
        let at = self.at.load(Ordering::Relaxed);
        StartStep::ALL.into_iter().find(|s| s.index() == at)
    }

    fn emit(&self, step: StartStep, state: MoveStepState) {
        let Some(token) = self.token.as_deref() else {
            return;
        };
        // A poisoned lock loses a progress frame, never the start.
        if let Ok(s) = self.store.lock() {
            s.bus_start_progress(&StartProgress {
                token: token.to_string(),
                step,
                index: step.index(),
                total: StartStep::ALL.len() as u8,
                state,
            });
        }
    }

    /// Close the step in flight and open `step`.
    pub(crate) fn advance(&self, step: StartStep) {
        if let Some(prev) = self.current() {
            if prev == step {
                return;
            }
            self.emit(prev, MoveStepState::Done);
        }
        self.at.store(step.index(), Ordering::Relaxed);
        self.emit(step, MoveStepState::Started);
    }

    /// Close the step in flight: the start succeeded.
    pub(crate) fn finish(&self) {
        if let Some(step) = self.current() {
            self.emit(step, MoveStepState::Done);
        }
        self.at.store(0, Ordering::Relaxed);
    }

    /// Mark the step in flight failed: the start returned an error.
    pub(crate) fn fail(&self) {
        if let Some(step) = self.current() {
            self.emit(step, MoveStepState::Failed);
        }
        self.at.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::RecordingEventBus;

    fn store_with_bus() -> (Mutex<Store>, Arc<RecordingEventBus>) {
        let bus = Arc::new(RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        bus.take();
        (Mutex::new(s), bus)
    }

    #[test]
    fn a_start_reports_each_step_once_in_order() {
        let (store, bus) = store_with_bus();
        let r = StartReporter::new(&store, Some("st-1".into()));
        r.advance(StartStep::Worktree);
        r.advance(StartStep::Worktree);
        r.advance(StartStep::Tmux);
        r.advance(StartStep::Agent);
        r.finish();
        r.finish();
        assert_eq!(
            bus.take(),
            vec![
                "start:progress:st-1:worktree:started",
                "start:progress:st-1:worktree:done",
                "start:progress:st-1:tmux:started",
                "start:progress:st-1:tmux:done",
                "start:progress:st-1:agent:started",
                "start:progress:st-1:agent:done",
            ]
        );
    }

    #[test]
    fn a_failure_marks_the_step_in_flight() {
        let (store, bus) = store_with_bus();
        let r = StartReporter::new(&store, Some("st-2".into()));
        r.advance(StartStep::Worktree);
        r.advance(StartStep::Tmux);
        r.fail();
        assert_eq!(
            bus.take().last().map(String::as_str),
            Some("start:progress:st-2:tmux:failed")
        );
    }

    #[test]
    fn without_a_token_nothing_is_emitted() {
        let (store, bus) = store_with_bus();
        let r = StartReporter::new(&store, None);
        r.advance(StartStep::Worktree);
        r.finish();
        assert!(bus.take().is_empty());
    }

    #[test]
    fn a_token_is_an_id_and_nothing_else() {
        validate_start_token("st-1700000000000-a9_x").unwrap();
        for bad in [
            "",
            "has space",
            "a/b",
            "ü",
            &"x".repeat(START_TOKEN_MAX + 1),
        ] {
            assert_eq!(
                validate_start_token(bad).unwrap_err().code,
                "E_INVALID",
                "{bad:?}"
            );
        }
    }
}
