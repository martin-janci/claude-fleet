//! Progress events for a move: the nine user-facing steps
//! ([`crate::events::MoveStep`]) as `move:progress` on the store's bus.
//!
//! Best-effort by construction. An emit takes the store lock for the length
//! of one bus call and never across an `.await`; a poisoned mutex is logged
//! and skipped. Nothing here can fail or slow a move.

use std::sync::Mutex;

use crate::events::{MoveProgress, MoveStep, MoveStepState};
use crate::store::Store;

/// Tracks the step a move is in and emits its boundaries.
pub(super) struct Progress<'a> {
    store: &'a Mutex<Store>,
    session_id: i64,
    to_host: String,
    current: Option<MoveStep>,
}

impl<'a> Progress<'a> {
    pub(super) fn new(store: &'a Mutex<Store>, session_id: i64, to_host: &str) -> Self {
        Self {
            store,
            session_id,
            to_host: to_host.to_string(),
            current: None,
        }
    }

    /// Begin `step`, closing the current one as done first.
    pub(super) fn start(&mut self, step: MoveStep) {
        self.end(MoveStepState::Done, None);
        self.current = Some(step);
        self.emit(step, MoveStepState::Started, None);
    }

    /// Close the current step as done.
    pub(super) fn done(&mut self, detail: Option<String>) {
        self.end(MoveStepState::Done, detail);
    }

    /// Close a step that cannot fail the move: warned when it could not do
    /// all of its work (and the move goes on), done otherwise.
    pub(super) fn end_soft(&mut self, warned: bool, detail: Option<String>) {
        self.end(
            if warned {
                MoveStepState::Warned
            } else {
                MoveStepState::Done
            },
            detail,
        );
    }

    /// Close the current step as failed. Silent when no step has started —
    /// a refusal before the first step is not a step's failure.
    pub(super) fn fail(&mut self) {
        self.end(MoveStepState::Failed, None);
    }

    fn end(&mut self, state: MoveStepState, detail: Option<String>) {
        if let Some(step) = self.current.take() {
            self.emit(step, state, detail);
        }
    }

    fn emit(&self, step: MoveStep, state: MoveStepState, detail: Option<String>) {
        let p = MoveProgress {
            session_id: self.session_id,
            to_host: self.to_host.clone(),
            step,
            index: step.index(),
            total: MoveStep::ALL.len() as u8,
            state,
            detail,
        };
        match self.store.lock() {
            Ok(s) => s.bus_move_progress(&p),
            Err(_) => tracing::warn!(
                session_id = self.session_id,
                step = step.as_str(),
                "store mutex poisoned; move progress not emitted"
            ),
        }
    }
}

impl Drop for Progress<'_> {
    /// A move whose future is dropped mid-step (the caller went away) still
    /// tells its observers that the step did not finish. After a normal end
    /// `current` is `None`, so this emits nothing.
    fn drop(&mut self) {
        self.fail();
    }
}

/// "3 files" / "1 file".
pub(super) fn count(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

/// The `git` step's detail. ADR 0002 has the move carry uncommitted work as
/// well as unpushed commits, and `CarryReport.commits` counts only the
/// commits — so a move with dirty files and no unpushed commits used to
/// report "0 commits" and hide the thing it actually carried. Whichever half
/// is zero is left out.
pub(super) fn git_detail(commits: u32, dirty: usize) -> String {
    match (commits, dirty) {
        (0, 0) => "nothing to carry".to_string(),
        (0, d) => count(d, "file"),
        (c, 0) => count(c as usize, "commit"),
        (c, d) => format!("{}, {}", count(c as usize, "commit"), count(d, "file")),
    }
}

/// The `claude_state` step's detail.
pub(super) fn state_detail(files: usize, notes: usize) -> String {
    format!("{}, {}", count(files, "file"), count(notes, "note"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{EventBus, RecordingEventBus};
    use std::sync::Arc;

    fn recording() -> (Mutex<Store>, Arc<RecordingEventBus>) {
        let bus = Arc::new(RecordingEventBus::new());
        let dyn_bus: Arc<dyn EventBus> = bus.clone();
        let store = Store::open_with_bus_in_memory(dyn_bus).expect("open");
        (Mutex::new(store), bus)
    }

    #[test]
    fn starting_the_next_step_closes_the_current_one_as_done() {
        let (store, bus) = recording();
        let mut p = Progress::new(&store, 3, "beta");
        p.start(MoveStep::Check);
        p.start(MoveStep::Transcript);
        p.done(None);
        assert_eq!(
            bus.take(),
            vec![
                "move:progress:3:check:started",
                "move:progress:3:check:done",
                "move:progress:3:transcript:started",
                "move:progress:3:transcript:done",
            ]
        );
    }

    #[test]
    fn fail_closes_the_current_step_and_is_silent_without_one() {
        let (store, bus) = recording();
        let mut p = Progress::new(&store, 3, "beta");
        p.fail();
        assert!(bus.take().is_empty(), "nothing started, nothing to fail");
        p.start(MoveStep::Git);
        p.fail();
        p.fail();
        p.done(None);
        assert_eq!(
            bus.take(),
            vec!["move:progress:3:git:started", "move:progress:3:git:failed"]
        );
    }

    #[test]
    fn warned_is_its_own_end_state() {
        let (store, bus) = recording();
        let mut p = Progress::new(&store, 3, "beta");
        p.start(MoveStep::Ignored);
        p.end_soft(true, Some("0 files".into()));
        assert_eq!(
            bus.take(),
            vec![
                "move:progress:3:ignored:started",
                "move:progress:3:ignored:warned"
            ]
        );
    }

    /// F1: a hub client that goes away drops the move's future mid-step. No
    /// `fail()` ever runs on that path, so the last thing every observer of
    /// that move had heard was `…:started` — a step that never ends.
    #[test]
    fn a_dropped_progress_closes_the_step_it_was_in() {
        let (store, bus) = recording();
        {
            let mut p = Progress::new(&store, 3, "beta");
            p.start(MoveStep::Git);
        }
        assert_eq!(
            bus.take(),
            vec!["move:progress:3:git:started", "move:progress:3:git:failed"]
        );
    }

    #[test]
    fn a_progress_that_ended_normally_emits_nothing_more_when_dropped() {
        let (store, bus) = recording();
        {
            let mut p = Progress::new(&store, 3, "beta");
            p.start(MoveStep::Git);
            p.done(Some("2 commits".into()));
        }
        assert_eq!(
            bus.take(),
            vec!["move:progress:3:git:started", "move:progress:3:git:done"]
        );
    }

    #[test]
    fn end_soft_is_warned_when_the_step_could_not_do_all_of_its_work() {
        let (store, bus) = recording();
        let mut p = Progress::new(&store, 3, "beta");
        p.start(MoveStep::Ignored);
        p.end_soft(false, Some("1 file".into()));
        p.start(MoveStep::ClaudeState);
        p.end_soft(true, Some("0 files, 0 notes".into()));
        assert_eq!(
            bus.take(),
            vec![
                "move:progress:3:ignored:started",
                "move:progress:3:ignored:done",
                "move:progress:3:claude_state:started",
                "move:progress:3:claude_state:warned",
            ]
        );
    }

    #[test]
    fn a_poisoned_store_never_panics_the_move() {
        let (store, _bus) = recording();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = store.lock().unwrap();
            panic!("poison");
        }));
        assert!(store.lock().is_err(), "the mutex is poisoned");
        let mut p = Progress::new(&store, 3, "beta");
        p.start(MoveStep::Check);
        p.fail();
    }

    #[test]
    fn details_are_counts() {
        assert_eq!(git_detail(0, 0), "nothing to carry");
        assert_eq!(git_detail(0, 2), "2 files");
        assert_eq!(git_detail(1, 0), "1 commit");
        assert_eq!(git_detail(2, 5), "2 commits, 5 files");
        assert_eq!(git_detail(1, 1), "1 commit, 1 file");
        assert_eq!(count(1, "file"), "1 file");
        assert_eq!(count(3, "file"), "3 files");
        assert_eq!(state_detail(46, 4), "46 files, 4 notes");
        assert_eq!(state_detail(1, 1), "1 file, 1 note");
    }
}
