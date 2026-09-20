# Transfer Sheet Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Moving a session to another host is one visible button in the terminal header, with live step-by-step progress in a closable sheet and a readable result or failure.

**Architecture:** The backend emits a new non-row event, `move:progress`, at nine fixed step boundaries of `move_session`; it travels the existing event bus (and the hub's `/events` stream) unchanged. The frontend keeps one `MoveRun` per session in a store fed by those events and by the command's result; a single `TransferSheet` renders setup / progress / result / failure from that run, and a `TransferChip` in the terminal header opens it and shows the live state.

**Tech Stack:** Rust (`fleet-core`, `src-tauri`), Svelte 5 + TypeScript, Vitest + @testing-library/svelte.

**Spec:** `docs/superpowers/specs/2026-09-20-transfer-sheet-design.md`

## Global Constraints

- No change to `MoveReport`, to `MoveSessionArgs`, or to any `#[tool(...)]` argument or description. The served MCP tool surface is byte-budgeted.
- Wire types derive `Serialize + Deserialize` with **no** `#[serde(default)]`. Field names are `snake_case` on the wire.
- Never hold the `Mutex<Store>` guard across an `.await`. A progress emit locks, emits, drops.
- Progress can never fail or slow a move: a poisoned mutex logs `tracing::warn!` and returns.
- `MoveProgress.detail` carries counts only — never a path, a prompt, stderr or transcript text — and is `None` on `failed`.
- The nine steps, in order, exactly: `check`, `transcript`, `workspace`, `git`, `replay`, `ignored`, `claude_state`, `start`, `handoff`. `index` is 1-based, `total` is 9.
- Existing test ids are kept: `move-from-details`, `move-target`, `move-keep-source`, `confirm-move`, `move-no-targets`, `move-dialog`. New ones: `transfer-chip`, `transfer-live`, `transfer-steps`, `transfer-result`, `transfer-failure`, `transfer-details`, `transfer-close`, `transfer-done`, `transfer-open-target`.
- Do NOT run a dev build of the desktop app (`cargo tauri dev`, `pnpm tauri dev`, `cargo run -p claude-fleet`): it migrates the production database and kills the installed app. Verification is tests only.
- Frontend commands: `npx vitest run <file>` and `npx svelte-check` (the `pnpm test` / `pnpm check` scripts do not find their binaries here). Run `pnpm install --frozen-lockfile` once before the first frontend test.
- Rust commands: `cargo test -p fleet-core <filter>`; before a task is reported done, the task's whole crate suite (`cargo test -p fleet-core`, or `cargo test -p claude-fleet --lib` for `src-tauri`) and `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check`. Never judge a run through `| tail`.
- Git: commit only. Never `pull`, `push`, `rebase`, `checkout`, `stash` or `merge`. All git commands run as `git -C <worktree path>`.
- No attribution lines in commit messages.
- UI copy is sentence case. No emoji; the only glyph is `⇄` (already used by the app).
- One writer per file: each task below owns its files; do not edit a file another task owns.

## File map

| File | Task | Responsibility |
|---|---|---|
| `crates/fleet-core/src/events.rs` | 1 | `MoveStep`, `MoveStepState`, `MoveProgress`, the `RowChange` variant, names/kinds, bus method |
| `crates/fleet-core/src/store/sessions.rs` | 1 | `Store::bus_move_progress` |
| `crates/fleet-core/src/mcp/events_route.rs` | 1 | test: the `move` kind filters |
| `src-tauri/src/backend/tests_events.rs` | 1 | test: the bridge carries `move:progress` |
| `src/lib/moveProgress.ts` (new) | 1 | TS wire types, `MOVE_STEPS`, `stepLabel` |
| `src/lib/events.ts` | 1 | union member, handler, subscription |
| `docs/control-api.md` | 1 | names list mentions `move:progress` |
| `crates/fleet-core/src/service/move_session/progress.rs` (new) | 2 | `Progress` emitter, detail helpers |
| `crates/fleet-core/src/service/move_session/mod.rs` | 2 | plumbing + the nine `start` calls + flow tests |
| `src/lib/moves.ts` (new) + test | 3 | the run store |
| `src/lib/moveEligibility.ts`, `src/lib/moveErrors.ts` (new) + tests | 4 | who can move where; error sentences |
| `src/lib/TransferSheet.svelte` (new) + test | 5 | the sheet |
| `src/lib/TransferChip.svelte` (new) + test, `src/lib/TerminalView.svelte` + its two tests | 6 | header control |
| `src/lib/SessionDetails.svelte` + test, `src/App.svelte`, `docs/adr/0002-move-carries-work-as-is.md`, `CLAUDE.md` | 7 | rewiring, mount, docs |

---

### Task 1: The `move:progress` event, end to end on the wire

**Files:**
- Modify: `crates/fleet-core/src/events.rs`
- Modify: `crates/fleet-core/src/store/sessions.rs` (append one method + one test)
- Modify: `crates/fleet-core/src/mcp/events_route.rs` (one test)
- Modify: `src-tauri/src/backend/tests_events.rs` (one table row + import)
- Create: `src/lib/moveProgress.ts`
- Modify: `src/lib/events.ts`
- Modify: `docs/control-api.md` (the `/events` names list, near "`catalog:loaded`, `sync:progress`, …")

**Interfaces:**
- Consumes: nothing.
- Produces (Rust): `fleet_core::events::{MoveStep, MoveStepState, MoveProgress}`; `MoveStep::ALL: [MoveStep; 9]`; `MoveStep::as_str(self) -> &'static str`; `MoveStep::index(self) -> u8` (1-based); `MoveStepState::as_str(self) -> &'static str`; `RowChange::MoveProgress(MoveProgress)`; `EventBus::move_progress(&self, p: &MoveProgress)`; `Store::bus_move_progress(&self, p: &MoveProgress)`. `RecordingEventBus` records the event as `move:progress:<session_id>:<step>:<state>`.
- Produces (TS): from `src/lib/moveProgress.ts` — `MOVE_STEPS`, `type MoveStep`, `type MoveStepState`, `interface MoveProgress`, `stepLabel(step: MoveStep, toHost: string): string`. From `events.ts` — handler `onMoveProgress?: (p: MoveProgress) => void`.

- [ ] **Step 1: Write the failing Rust tests** in `crates/fleet-core/src/events.rs`, inside `mod tests`:

```rust
    #[test]
    fn move_steps_are_nine_in_order_and_serialize_as_their_names() {
        let names: Vec<&str> = MoveStep::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            names,
            [
                "check",
                "transcript",
                "workspace",
                "git",
                "replay",
                "ignored",
                "claude_state",
                "start",
                "handoff"
            ]
        );
        for (i, step) in MoveStep::ALL.iter().enumerate() {
            assert_eq!(step.index() as usize, i + 1);
            assert_eq!(
                serde_json::to_value(step).unwrap(),
                serde_json::json!(step.as_str())
            );
        }
        for state in [
            MoveStepState::Started,
            MoveStepState::Done,
            MoveStepState::Warned,
            MoveStepState::Failed,
        ] {
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::json!(state.as_str())
            );
        }
    }

    #[test]
    fn move_progress_keeps_its_wire_shape() {
        let p = MoveProgress {
            session_id: 7,
            to_host: "beta".into(),
            step: MoveStep::Git,
            index: 4,
            total: 9,
            state: MoveStepState::Done,
            detail: Some("2 commits".into()),
        };
        let change = RowChange::MoveProgress(p.clone());
        assert_eq!(change.name(), "move:progress");
        assert_eq!(
            change.payload(),
            serde_json::json!({
                "session_id": 7, "to_host": "beta", "step": "git", "index": 4,
                "total": 9, "state": "done", "detail": "2 commits"
            })
        );
        let back: MoveProgress = serde_json::from_value(change.payload()).unwrap();
        assert_eq!(back, p);
    }

    /// The frontend's step list is the same nine names, in the same order.
    /// `moveProgress.ts` brackets the list with two marker comments so this
    /// test reads the list and nothing else.
    #[test]
    fn frontend_declares_the_move_steps_in_order() {
        let ts = include_str!("../../../src/lib/moveProgress.ts");
        let begin = ts.find("// move-steps:begin").expect("begin marker");
        let end = ts.find("// move-steps:end").expect("end marker");
        let quoted: Vec<&str> = ts[begin..end].split('\'').skip(1).step_by(2).collect();
        let want: Vec<&str> = MoveStep::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(quoted, want);
    }
```

Also extend the two existing tests in the same module:
- in `row_change_names_match_frontend_subscriptions`, add to `cases` (build the value above the `vec!`):

```rust
        let moving = MoveProgress {
            session_id: 1,
            to_host: String::new(),
            step: MoveStep::Check,
            index: 1,
            total: 9,
            state: MoveStepState::Started,
            detail: None,
        };
        // … and in the vec:
            (RowChange::MoveProgress(moving), "move:progress"),
```
- in `every_row_change_variant_is_subscribable`, add the arm `RowChange::MoveProgress(_) => pinned_name!("move:progress"),`.

- [ ] **Step 2: Run them and confirm they fail to compile**

Run: `cargo test -p fleet-core events::tests`
Expected: compile errors — `MoveStep`, `MoveStepState`, `MoveProgress`, `RowChange::MoveProgress` not found; `moveProgress.ts` missing for `include_str!`.

- [ ] **Step 3: Implement the Rust types** in `crates/fleet-core/src/events.rs`.

Directly below the `SyncProgress` struct:

```rust
/// The nine user-facing steps of a move, in the order they run. Several of
/// `move_session`'s internal stages fold into one step (seeding is part of
/// `workspace`, the confirm is part of `start`): the user follows these, not
/// the stage numbers.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MoveStep {
    Check,
    Transcript,
    Workspace,
    Git,
    Replay,
    Ignored,
    ClaudeState,
    Start,
    Handoff,
}

impl MoveStep {
    /// Every step, in order. `src/lib/moveProgress.ts` mirrors it
    /// (`frontend_declares_the_move_steps_in_order`).
    pub const ALL: [MoveStep; 9] = [
        MoveStep::Check,
        MoveStep::Transcript,
        MoveStep::Workspace,
        MoveStep::Git,
        MoveStep::Replay,
        MoveStep::Ignored,
        MoveStep::ClaudeState,
        MoveStep::Start,
        MoveStep::Handoff,
    ];

    /// The wire name (what serde writes).
    pub const fn as_str(self) -> &'static str {
        match self {
            MoveStep::Check => "check",
            MoveStep::Transcript => "transcript",
            MoveStep::Workspace => "workspace",
            MoveStep::Git => "git",
            MoveStep::Replay => "replay",
            MoveStep::Ignored => "ignored",
            MoveStep::ClaudeState => "claude_state",
            MoveStep::Start => "start",
            MoveStep::Handoff => "handoff",
        }
    }

    /// 1-based position in [`Self::ALL`].
    pub const fn index(self) -> u8 {
        self as u8 + 1
    }
}

/// How a [`MoveStep`] stands. `Warned` is a step that could not do all of
/// its work but cannot fail the move (ignored files, the Claude-side state).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MoveStepState {
    Started,
    Done,
    Warned,
    Failed,
}

impl MoveStepState {
    pub const fn as_str(self) -> &'static str {
        match self {
            MoveStepState::Started => "started",
            MoveStepState::Done => "done",
            MoveStepState::Warned => "warned",
            MoveStepState::Failed => "failed",
        }
    }
}

/// One step boundary of an in-flight `move_session`. `session_id` is the
/// SOURCE row. `detail` is a short count ("2 commits") — never a path or
/// stderr — and is `None` on `Failed`: the error reaches the caller through
/// the command's result.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MoveProgress {
    pub session_id: i64,
    pub to_host: String,
    pub step: MoveStep,
    pub index: u8,
    pub total: u8,
    pub state: MoveStepState,
    pub detail: Option<String>,
}
```

Then:
- `RowChange`: add, after `SyncProgress(SyncProgress),`:
  ```rust
      /// A step boundary of an in-flight move. Not a store row.
      MoveProgress(MoveProgress),
  ```
- `RowChange::name`: `RowChange::MoveProgress(_) => "move:progress",`
- `RowChange::payload`: `RowChange::MoveProgress(p) => to_value(p),`
- `EventBus`: below `sync_progress`:
  ```rust
      /// See [`RowChange::MoveProgress`].
      fn move_progress(&self, p: &MoveProgress) {
          self.emit(&RowChange::MoveProgress(p.clone()));
      }
  ```
- `EVENT_NAMES`: `[&str; 19]`, append `"move:progress",`.
- `EVENT_KINDS`: `[&str; 11]`, append `"move",`.
- `RecordingEventBus::emit`: add the arm
  ```rust
              RowChange::MoveProgress(p) => {
                  format!("{}:{}:{}", p.session_id, p.step.as_str(), p.state.as_str())
              }
  ```

If any other `match` on `RowChange` in the workspace stops compiling, give it the same treatment as its `SyncProgress` arm.

- [ ] **Step 4: Create `src/lib/moveProgress.ts`**

```ts
// The wire shape of `move:progress` (`fleet_core::events::MoveProgress`) and
// the nine steps of a move. The Rust test
// `frontend_declares_the_move_steps_in_order` reads the list between the two
// markers below: keep nothing but the step names in single quotes there.

// move-steps:begin
export const MOVE_STEPS = [
  'check',
  'transcript',
  'workspace',
  'git',
  'replay',
  'ignored',
  'claude_state',
  'start',
  'handoff',
] as const;
// move-steps:end

export type MoveStep = (typeof MOVE_STEPS)[number];
export type MoveStepState = 'started' | 'done' | 'warned' | 'failed';

export interface MoveProgress {
  /** The SOURCE session's row id. */
  session_id: number;
  to_host: string;
  step: MoveStep;
  /** 1-based position of `step` in `MOVE_STEPS`. */
  index: number;
  total: number;
  state: MoveStepState;
  /** A short count ("2 commits"); null on `failed`. */
  detail: string | null;
}

/** What the sheet calls a step. */
export function stepLabel(step: MoveStep, toHost: string): string {
  switch (step) {
    case 'check':
      return 'Check the source';
    case 'transcript':
      return 'Read the conversation';
    case 'workspace':
      return 'Prepare the target';
    case 'git':
      return 'Carry the git work';
    case 'replay':
      return 'Replay uncommitted work';
    case 'ignored':
      return 'Ignored files';
    case 'claude_state':
      return 'Subagents and memory';
    case 'start':
      return `Start on ${toHost}`;
    case 'handoff':
      return 'Hand over';
  }
}
```

- [ ] **Step 5: Wire `src/lib/events.ts`** — follow `sync:progress` at each of its five sites:
  - import: `import type { MoveProgress } from './moveProgress';`
  - `RowEventHandlers`: `onMoveProgress?: (p: MoveProgress) => void;` below `onSyncProgress`
  - the `RowEvent` union: `| { name: 'move:progress'; payload: MoveProgress }` (the last member currently ends with `;` — move the `;`)
  - the flush `switch`: `case 'move:progress': handlers.onMoveProgress?.(ev.payload); break;`
  - `wanted`: `moveProgress: !!handlers.onMoveProgress,`
  - the `Promise.all` list: `sub('move:progress', wanted.moveProgress),`

- [ ] **Step 6: Store method + test** — append to `crates/fleet-core/src/store/sessions.rs`, inside an `impl Store` block:

```rust
    /// Emit `move:progress` (not a store row).
    pub fn bus_move_progress(&self, p: &crate::events::MoveProgress) {
        self.bus.move_progress(p);
    }
```

and in that file's `mod tests`:

```rust
    #[test]
    fn bus_move_progress_records_expected_event() {
        use crate::events::{MoveProgress, MoveStep, MoveStepState};
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let dyn_bus: std::sync::Arc<dyn crate::events::EventBus> = bus.clone();
        let s = crate::store::Store::open_with_bus_in_memory(dyn_bus).expect("open");
        s.bus_move_progress(&MoveProgress {
            session_id: 7,
            to_host: "beta".into(),
            step: MoveStep::Git,
            index: 4,
            total: 9,
            state: MoveStepState::Done,
            detail: None,
        });
        assert_eq!(bus.take(), vec!["move:progress:7:git:done"]);
    }
```

- [ ] **Step 7: The `/events` kind filter** — in `crates/fleet-core/src/mcp/events_route.rs`, next to `a_filter_matches_the_part_before_the_colon`:

```rust
    #[test]
    fn the_move_kind_is_subscribable() {
        let asked = wanted_kinds(&q(Some("move")));
        assert!(asked.unknown.is_empty(), "{:?}", asked.unknown);
        let kinds = asked.accepted;
        assert!(matches(kinds.as_ref(), &ev("move:progress")));
        assert!(!matches(kinds.as_ref(), &ev("session:updated")));
    }
```

- [ ] **Step 8: The desktop bridge** — in `src-tauri/src/backend/tests_events.rs`, extend the import to `use fleet_core::events::{CatalogSummary, MoveProgress, MoveStep, MoveStepState, RowChange, SyncProgress};` and add after the `RowChange::SyncProgress(…)` row of the `changes` table:

```rust
        RowChange::MoveProgress(MoveProgress {
            session_id: 5,
            to_host: "trn".into(),
            step: MoveStep::Git,
            index: 4,
            total: 9,
            state: MoveStepState::Done,
            detail: Some("2 commits".into()),
        }),
```

If a doc comment in that file counts the variants the table builds ("15 of the 16"), update the numbers.

- [ ] **Step 9: Docs** — in `docs/control-api.md`, the `/events` bullet listing event names: add `` `move:progress` `` after `` `sync:progress` ``.

- [ ] **Step 10: Run everything**

Run: `cargo test -p fleet-core` then `cargo test -p claude-fleet --lib backend` then `npx vitest run src/lib/events` then `npx svelte-check`
Expected: all pass, 0 svelte-check errors. Then `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check`: clean.

- [ ] **Step 11: Commit**

```bash
git -C <worktree> add crates/fleet-core/src/events.rs crates/fleet-core/src/store/sessions.rs crates/fleet-core/src/mcp/events_route.rs src-tauri/src/backend/tests_events.rs src/lib/moveProgress.ts src/lib/events.ts docs/control-api.md
git -C <worktree> commit -m "feat(events): move:progress — the nine steps of a move on the event bus"
```

---

### Task 2: `move_session` emits its progress

**Files:**
- Create: `crates/fleet-core/src/service/move_session/progress.rs`
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` (`mod progress;`, `move_session_steps`, `move_session_inner`, and its `mod tests`)

**Interfaces:**
- Consumes (Task 1): `crate::events::{MoveProgress, MoveStep, MoveStepState}`, `Store::bus_move_progress`, `RecordingEventBus` recording `move:progress:<session_id>:<step>:<state>`.
- Produces: `progress::Progress<'a>` with `new(store: &'a Mutex<Store>, session_id: i64, to_host: &str)`, `start(&mut self, MoveStep)`, `done(&mut self, Option<String>)`, `warned(&mut self, Option<String>)`, `fail(&mut self)`; helpers `progress::git_detail(commits: u32, dirty: usize) -> String`, `progress::count(n: usize, one: &str) -> String`, `progress::state_detail(files: usize, notes: usize) -> String`.

- [ ] **Step 1: Write `progress.rs` with its failing tests first** (tests at the bottom of the new file; write the file with the test module and `todo!()`-free stubs is NOT acceptable — write tests, run, see them fail to compile, then write the code above them):

```rust
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
        p.warned(Some("0 files".into()));
        assert_eq!(
            bus.take(),
            vec![
                "move:progress:3:ignored:started",
                "move:progress:3:ignored:warned"
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
        assert_eq!(git_detail(0, 2), "0 commits");
        assert_eq!(git_detail(1, 0), "1 commit");
        assert_eq!(git_detail(2, 5), "2 commits");
        assert_eq!(count(1, "file"), "1 file");
        assert_eq!(count(3, "file"), "3 files");
        assert_eq!(state_detail(46, 4), "46 files, 4 notes");
        assert_eq!(state_detail(1, 1), "1 file, 1 note");
    }
}
```

Add `mod progress;` beside the other `mod` lines at the top of `mod.rs` (next to `mod carry;` / `mod claude_state;`).

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p fleet-core move_session::progress`
Expected: compile errors — `Progress`, `git_detail`, `count`, `state_detail` not found.

- [ ] **Step 3: Implement `progress.rs`** (above the test module):

```rust
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

    /// Close the current step as warned: it could not do all of its work,
    /// and the move goes on.
    pub(super) fn warned(&mut self, detail: Option<String>) {
        self.end(MoveStepState::Warned, detail);
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

/// "3 files" / "1 file".
pub(super) fn count(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

/// The `git` step's detail.
pub(super) fn git_detail(commits: u32, dirty: usize) -> String {
    if commits == 0 && dirty == 0 {
        "nothing to carry".to_string()
    } else {
        count(commits as usize, "commit")
    }
}

/// The `claude_state` step's detail.
pub(super) fn state_detail(files: usize, notes: usize) -> String {
    format!("{}, {}", count(files, "file"), count(notes, "note"))
}
```

Run: `cargo test -p fleet-core move_session::progress` — Expected: 5 passed.

- [ ] **Step 4: Write the failing flow tests** in `mod.rs` → `mod tests`.

First make the fixture accept a store. Change

```rust
    fn fixture() -> Fixture {
        let s = Store::open_in_memory().unwrap();
```
to
```rust
    fn fixture() -> Fixture {
        fixture_on(Store::open_in_memory().unwrap())
    }

    /// [`fixture`] with every event recorded, for the progress tests.
    fn recorded_fixture() -> (Fixture, std::sync::Arc<crate::events::RecordingEventBus>) {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let dyn_bus: std::sync::Arc<dyn crate::events::EventBus> = bus.clone();
        let f = fixture_on(Store::open_with_bus_in_memory(dyn_bus).unwrap());
        bus.take(); // the fixture's own row events
        (f, bus)
    }

    /// The recorded `move:progress` events as `<step>:<state>`.
    fn progress_of(f: &Fixture, bus: &crate::events::RecordingEventBus) -> Vec<String> {
        let prefix = format!("move:progress:{}:", f.source_id);
        bus.take()
            .into_iter()
            .filter_map(|e| e.strip_prefix(&prefix).map(str::to_string))
            .collect()
    }

    fn fixture_on(s: Store) -> Fixture {
```
(the rest of the old `fixture` body is unchanged).

Then add:

```rust
    #[tokio::test]
    async fn a_clean_move_reports_all_nine_steps_in_order() {
        let (f, bus) = recorded_fixture();
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        run(&f, &hooks, false).await.expect("move");
        assert_eq!(
            progress_of(&f, &bus),
            [
                "check:started",
                "check:done",
                "transcript:started",
                "transcript:done",
                "workspace:started",
                "workspace:done",
                "git:started",
                "git:done",
                "replay:started",
                "replay:done",
                "ignored:started",
                "ignored:done",
                "claude_state:started",
                "claude_state:done",
                "start:started",
                "start:done",
                "handoff:started",
                "handoff:done",
            ]
        );
    }

    #[tokio::test]
    async fn a_refusal_in_the_check_ends_the_stream_at_check_failed() {
        let (f, bus) = recorded_fixture();
        f.fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:inspect"),
            Reply::ok(&inspection(" M a.rs", HEAD, "0")),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let mut a = args(&f, false);
        a.strict = true;
        let err = move_session_with(a, &f.store, &f.fake, &hooks, fast())
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_DIRTY);
        assert_eq!(progress_of(&f, &bus), ["check:started", "check:failed"]);
    }

    #[tokio::test]
    async fn a_carry_failure_ends_the_stream_at_the_step_that_failed() {
        let (f, bus) = recorded_fixture();
        f.fake.on_host(
            "beta",
            Match::script_contains("# cf-carry:fetch"),
            Reply::fail(5, &format!("{} fetch", carry::FAILED)),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_CARRY);
        let seen = progress_of(&f, &bus);
        assert_eq!(seen.last().map(String::as_str), Some("git:failed"), "{seen:?}");
        assert!(!seen.iter().any(|e| e.starts_with("replay:")), "{seen:?}");
    }

    #[tokio::test]
    async fn a_step_that_only_warns_reports_warned_and_the_move_goes_on() {
        let (f, bus) = recorded_fixture();
        let mut listed = out("").into_bytes();
        listed.extend_from_slice(b"4\t.env\0");
        f.fake
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:ignored-list"),
                Reply::Exit {
                    code: 0,
                    stdout: listed,
                    stderr: Vec::new(),
                },
            )
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:ignored-pack"),
                Reply::fail(5, &format!("{} tar", carry::FAILED)),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        run(&f, &hooks, false).await.expect("the move still succeeds");
        let seen = progress_of(&f, &bus);
        assert!(seen.contains(&"ignored:warned".to_string()), "{seen:?}");
        assert_eq!(seen.last().map(String::as_str), Some("handoff:done"));
    }

    #[tokio::test]
    async fn a_refusal_before_the_first_step_reports_nothing() {
        let (f, bus) = recorded_fixture();
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let _claim = MoveClaim::acquire(&f.store, f.source_id).unwrap();
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(progress_of(&f, &bus).is_empty());
    }
```

`# cf-carry:fetch` is `carry::fetch_script`'s first line; it runs on the target in stage 3b.

- [ ] **Step 5: Run and confirm they fail for the right reason**

Run: `cargo test -p fleet-core move_session::tests::a_clean_move_reports_all_nine_steps_in_order move_session::tests::a_refusal`
Expected: FAIL — `progress_of` returns an empty list (left `[]`, right the expected steps). `a_refusal_before_the_first_step_reports_nothing` passes already; that is expected — it guards Step 6 against emitting too early.

- [ ] **Step 6: Plumb `Progress` through the flow** in `mod.rs`.

`move_session_steps` becomes:

```rust
async fn move_session_steps(
    args: MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    opts: MoveOptions,
) -> Result<MoveReport, IpcError> {
    let mut cleanup = CarryCleanup::default();
    let mut progress = progress::Progress::new(store, args.session_id, &args.target_host_alias);
    let result =
        move_session_inner(args, store, ssh, hooks, opts, &mut cleanup, &mut progress).await;
    // The step that was running is the one that failed; a refusal before the
    // first step (validation, the claim) closes nothing.
    match &result {
        Ok(_) => progress.done(None),
        Err(_) => progress.fail(),
    }
    cleanup.run(ssh).await;
    result
}
```

`move_session_inner` gains the last parameter `progress: &mut progress::Progress<'_>,` and these calls (add `use crate::events::MoveStep;` at the top of the file with the other `use crate::` lines):

| Where (the stage comment already in the code) | Insert directly above it |
|---|---|
| `// 0. The source Claude must be idle NOW` | `progress.start(MoveStep::Check);` |
| `// 2. Transcript: locate, cap, read` | `progress.start(MoveStep::Transcript);` |
| `// 3. Target workspace: refresh origin/<branch>` | `progress.start(MoveStep::Workspace);` |
| `// 3b. Carry the git state` | `progress.start(MoveStep::Git);` |
| `// 3c. Replay the uncommitted work` | `progress.done(Some(progress::git_detail(carried.commits, state.dirty.len())));`<br>`progress.start(MoveStep::Replay);` |
| `// 3d. Small git-ignored files.` | `progress.start(MoveStep::Ignored);`<br>`let warned_before = warnings.len();` |
| `// 3e. The Claude-side state` | see below |
| the line `put(ssh, &target, &prep.path, &bytes)` | see below |
| `// 6. The source: check it wrote nothing` | `progress.start(MoveStep::Handoff);` |

Above `// 3e.` (that is, after the ignored-files `match` closes):

```rust
    {
        let detail = Some(progress::count(carried.ignored_carried.len(), "file"));
        if warnings.len() > warned_before {
            progress.warned(detail);
        } else {
            progress.done(detail);
        }
    }
    progress.start(MoveStep::ClaudeState);
    let warned_before = warnings.len();
```

Above `put(ssh, &target, &prep.path, &bytes)` (after the `carry_memory` `match` closes):

```rust
    {
        let detail = Some(progress::state_detail(
            carried.session_state.carried.len(),
            carried.memory.carried.len(),
        ));
        if warnings.len() > warned_before {
            progress.warned(detail);
        } else {
            progress.done(detail);
        }
    }
    progress.start(MoveStep::Start);
```

`carried` and `state` are the names already in scope at those points (`carried.commits = bundle.commits;` is set in stage 3b; `state.dirty` is the inspected porcelain). If `carried` is declared after the `// 3c.` comment in your copy of the file, use the binding that holds the commit count there — the detail string must still come from `git_detail`.

Every other caller of `move_session_inner` (there is one: `move_session_steps`) already passes the new argument.

- [ ] **Step 7: Run the flow tests, then the whole crate**

Run: `cargo test -p fleet-core move_session`
Expected: all pass, including the five new ones.
Run: `cargo test -p fleet-core` — all pass. `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check` — clean.

- [ ] **Step 8: Commit**

```bash
git -C <worktree> add crates/fleet-core/src/service/move_session/progress.rs crates/fleet-core/src/service/move_session/mod.rs
git -C <worktree> commit -m "feat(move): report the nine steps of a move as move:progress"
```

---

### Task 3: The run store, `src/lib/moves.ts`

**Files:**
- Create: `src/lib/moves.ts`
- Test: `src/lib/moves.test.ts`

**Interfaces:**
- Consumes (Task 1): `MOVE_STEPS`, `MoveProgress`, `MoveStep`, `MoveStepState` from `./moveProgress`. Existing: `moveSession`, `MoveReport` from `./moveSession`; `IpcError` from `./result`; `sessions`, `SessionRow` from `./sessions`; `selectedSession`, `selectSession` from `./selection`; `push`, `pushError` from `./toasts`.
- Produces: `StepState`, `MoveStatus`, `MoveRun`, `moves` (readable store of `Map<number, MoveRun>`), `transferSheetFor` (writable `number | null`), `startMove(session, toHost, { keepSource }): void`, `applyMoveProgress(p): void`, `dismissMove(sessionId): void`, `activeMoveFor(sessionId): MoveRun | undefined`, `stepNumber(run): number`, `resetMovesForTest(): void`.

- [ ] **Step 1: Write the failing tests** — `src/lib/moves.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import {
  moves, transferSheetFor, startMove, applyMoveProgress, dismissMove,
  activeMoveFor, stepNumber, resetMovesForTest,
} from './moves';
import type { MoveProgress, MoveStep, MoveStepState } from './moveProgress';
import { MOVE_STEPS } from './moveProgress';
import { sessions, type SessionRow } from './sessions';
import { selectSession, selectedSession } from './selection';
import { toasts, clearToasts } from './toasts';

const mockInvoke = invoke as ReturnType<typeof vi.fn>;

const row = (over: Partial<SessionRow>): SessionRow =>
  ({
    id: 5, tmux_name: 'dev-foo', host_alias: 'mefistos', project_id: 1, worktree_id: 10,
    created_at: 1, last_activity_at: 1, status: 'running', notes: null, account_uuid: null,
    kind: 'work', reviews_session_id: null, worktree_key: null, lost_at: null,
    claude_session_id: '550e8400-e29b-41d4-a716-446655440000', claude_status: null,
    effort_level: null, pr_url: null, current_activity: null, friendly_name: null,
    safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null,
    safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null,
    stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null,
    last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null,
    parent_session_id: null, tags: [], model: null, context_tokens: null,
    context_window: null, context_source: null, context_at: null, context_stale: false,
    tmux_pane_id: null, ...over,
  }) as SessionRow;

const source = row({});
const target = row({ id: 6, host_alias: 'turanga', parent_session_id: 5 });

const report = {
  source_session_id: 5, target_session_id: 6, from_host: 'mefistos', to_host: 'turanga',
  tmux_name: 'dev-foo', claude_session_id: source.claude_session_id, branch: 'feat',
  target_cwd: '/r/.claude/worktrees/feat', transcript_bytes: 10, source_killed: true,
  warnings: [],
  carried: {
    commits: 0, bundle_bytes: 0, dirty_entries: [], ignored_carried: [],
    ignored_left_behind: [], target_seeded: 'existing',
    session_state: { carried: [], kept_target: [], left_behind: [] },
    memory: { carried: [], kept_target: [], identical: 0, index_lines_added: 0, left_behind: [] },
  },
  target,
};

const ev = (step: MoveStep, state: MoveStepState, over: Partial<MoveProgress> = {}): MoveProgress => ({
  session_id: 5, to_host: 'turanga', step, index: MOVE_STEPS.indexOf(step) + 1,
  total: 9, state, detail: null, ...over,
});

const states = (id = 5) => get(moves).get(id)!.steps.map((s) => s.state);

/** A `moveSession` call the test resolves by hand. */
function pending() {
  let resolve!: (v: unknown) => void;
  let reject!: (e: unknown) => void;
  mockInvoke.mockImplementation(
    (cmd: string) =>
      cmd === 'move_session'
        ? new Promise((res, rej) => { resolve = res; reject = rej; })
        : Promise.resolve(undefined),
  );
  return { resolve: (v: unknown) => resolve(v), reject: (e: unknown) => reject(e) };
}
const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  mockInvoke.mockReset();
  resetMovesForTest();
  sessions.set([source]);
  selectSession(null);
  clearToasts();
});

describe('startMove', () => {
  it('creates a running local run with nine pending steps and invokes once', () => {
    pending();
    startMove(source, 'turanga', { keepSource: false });
    const run = get(moves).get(5)!;
    expect(run).toMatchObject({
      sessionId: 5, sessionName: 'dev-foo', fromHost: 'mefistos', toHost: 'turanga',
      keepSource: false, origin: 'local', status: 'running', report: null, error: null,
    });
    expect(run.steps.map((s) => s.step)).toEqual([...MOVE_STEPS]);
    expect(states()).toEqual(Array(9).fill('pending'));
    expect(activeMoveFor(5)).toBe(run);
    expect(mockInvoke).toHaveBeenCalledWith('move_session', {
      args: { session_id: 5, target_host_alias: 'turanga', keep_source: false, strict: false },
    });
  });

  it('refuses a second start while one is running', () => {
    pending();
    startMove(source, 'turanga', { keepSource: false });
    startMove(source, 'turanga', { keepSource: true });
    expect(mockInvoke.mock.calls.filter((c) => c[0] === 'move_session')).toHaveLength(1);
    expect(get(moves).get(5)!.keepSource).toBe(false);
  });

  it('settles done from the result, keeps warned steps, and follows the selection', async () => {
    const p = pending();
    selectSession(source);
    transferSheetFor.set(5);
    startMove(source, 'turanga', { keepSource: false });
    applyMoveProgress(ev('ignored', 'warned', { detail: '0 files' }));
    p.resolve(report);
    await flush();
    const run = get(moves).get(5)!;
    expect(run.status).toBe('done');
    expect(run.report?.target_session_id).toBe(6);
    expect(states()).toEqual(['done', 'done', 'done', 'done', 'done', 'warned', 'done', 'done', 'done']);
    expect(get(selectedSession)?.id).toBe(6);
    expect(get(toasts)).toHaveLength(0); // the sheet is open on this run
    expect(activeMoveFor(5)).toBeUndefined();
  });

  it('toasts when the sheet is closed, and leaves another selection alone', async () => {
    const p = pending();
    const other = row({ id: 9, tmux_name: 'other' });
    sessions.set([source, other]);
    selectSession(other);
    startMove(source, 'turanga', { keepSource: false });
    p.resolve(report);
    await flush();
    expect(get(selectedSession)?.id).toBe(9);
    expect(get(toasts).some((t) => t.kind === 'success' && t.message === 'Moved dev-foo to turanga')).toBe(true);
  });

  it('settles failed and marks the step that was running', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    applyMoveProgress(ev('git', 'started'));
    p.reject({ code: 'E_MOVE_CARRY', message: 'fetch failed', details: { step: 'fetch' } });
    await flush();
    const run = get(moves).get(5)!;
    expect(run.status).toBe('failed');
    expect(run.error?.code).toBe('E_MOVE_CARRY');
    expect(states()).toEqual(['done', 'done', 'done', 'failed', 'pending', 'pending', 'pending', 'pending', 'pending']);
    expect(get(toasts).some((t) => t.kind === 'error')).toBe(true);
  });

  it('settles partial on E_MOVE_PARTIAL', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    p.reject({ code: 'E_MOVE_PARTIAL', message: 'x', details: { target_session_id: 6 } });
    await flush();
    expect(get(moves).get(5)!.status).toBe('partial');
  });
});

describe('applyMoveProgress', () => {
  it('only moves a step forward', () => {
    pending();
    startMove(source, 'turanga', { keepSource: false });
    applyMoveProgress(ev('check', 'done'));
    applyMoveProgress(ev('check', 'started'));
    expect(states()[0]).toBe('done');
  });

  it('fills the gap before a later step', () => {
    pending();
    startMove(source, 'turanga', { keepSource: false });
    applyMoveProgress(ev('git', 'started', { detail: null }));
    expect(states().slice(0, 4)).toEqual(['done', 'done', 'done', 'started']);
    expect(stepNumber(get(moves).get(5)!)).toBe(4);
  });

  it('drops an event whose index and step disagree', () => {
    pending();
    startMove(source, 'turanga', { keepSource: false });
    applyMoveProgress(ev('git', 'done', { index: 2 }));
    applyMoveProgress(ev('git', 'done', { index: 0 }));
    applyMoveProgress(ev('git', 'done', { index: 10 }));
    expect(states()).toEqual(Array(9).fill('pending'));
  });

  it('truncates the detail at 80 characters', () => {
    pending();
    startMove(source, 'turanga', { keepSource: false });
    applyMoveProgress(ev('git', 'done', { detail: 'x'.repeat(200) }));
    expect(get(moves).get(5)!.steps[3].detail).toHaveLength(80);
  });

  it('creates an observed run for a move started elsewhere and settles it from events', () => {
    applyMoveProgress(ev('check', 'started'));
    let run = get(moves).get(5)!;
    expect(run).toMatchObject({
      origin: 'observed', sessionName: 'dev-foo', fromHost: 'mefistos', toHost: 'turanga',
      keepSource: null, status: 'running',
    });
    applyMoveProgress(ev('handoff', 'done'));
    run = get(moves).get(5)!;
    expect(run.status).toBe('done');
    expect(run.report).toBeNull();
  });

  it('an observed run fails on a failed step, with no error object', () => {
    applyMoveProgress(ev('check', 'started'));
    applyMoveProgress(ev('check', 'failed'));
    expect(get(moves).get(5)).toMatchObject({ status: 'failed', error: null });
  });

  it('names an unknown session by its id', () => {
    applyMoveProgress(ev('check', 'started', { session_id: 77 }));
    expect(get(moves).get(77)).toMatchObject({ sessionName: 'session 77', fromHost: '' });
  });

  it('ignores events for a settled run, except a fresh check:started', () => {
    applyMoveProgress(ev('check', 'started'));
    applyMoveProgress(ev('check', 'failed'));
    applyMoveProgress(ev('git', 'done'));
    expect(get(moves).get(5)!.status).toBe('failed');
    applyMoveProgress(ev('check', 'started'));
    expect(get(moves).get(5)).toMatchObject({ status: 'running', origin: 'observed' });
    expect(states()[0]).toBe('started');
  });

  it('a local run never settles from events', () => {
    pending();
    startMove(source, 'turanga', { keepSource: false });
    applyMoveProgress(ev('handoff', 'done'));
    expect(get(moves).get(5)!.status).toBe('running');
  });
});

describe('dismissMove', () => {
  it('removes a settled run and refuses a running one', () => {
    pending();
    startMove(source, 'turanga', { keepSource: false });
    dismissMove(5);
    expect(get(moves).has(5)).toBe(true);
    resetMovesForTest();
    applyMoveProgress(ev('check', 'started'));
    applyMoveProgress(ev('check', 'failed'));
    dismissMove(5);
    expect(get(moves).has(5)).toBe(false);
  });
});
```

`moveSession` resolves an `IpcError` rejection from `invoke` into `{ ok: false, error }` (`invokeCmd` in `result.ts`); the `p.reject({...})` calls above rely on that. Read `invokeCmd` first and, if it expects a different rejection shape, adapt `reject`'s argument — not the assertions.

- [ ] **Step 2: Run and confirm failure**

Run: `npx vitest run src/lib/moves.test.ts`
Expected: FAIL — `Failed to resolve import "./moves"`.

- [ ] **Step 3: Implement `src/lib/moves.ts`**

```ts
// One run per moving session: what the Transfer sheet and the header chip
// render. A run started in this window ('local') is settled by the command's
// result; one seen only through `move:progress` events ('observed' — another
// window, the MCP API, a hub client) is settled by its events. Nothing here
// is persisted: the session timeline already records the move.
import { get, writable, type Readable } from 'svelte/store';
import { MOVE_STEPS, type MoveProgress, type MoveStep, type MoveStepState } from './moveProgress';
import { moveSession, type MoveReport } from './moveSession';
import type { IpcError, Result } from './result';
import { sessions, type SessionRow } from './sessions';
import { selectedSession, selectSession } from './selection';
import { push, pushError } from './toasts';

export type StepState = 'pending' | MoveStepState;
export type MoveStatus = 'running' | 'done' | 'failed' | 'partial';

export interface MoveRunStep {
  step: MoveStep;
  state: StepState;
  detail: string | null;
}

export interface MoveRun {
  sessionId: number;
  /** tmux name at start: the source row may be gone by the end. */
  sessionName: string;
  fromHost: string;
  toHost: string;
  /** null for an observed run: only the starter knows. */
  keepSource: boolean | null;
  origin: 'local' | 'observed';
  /** Always the nine steps, in order. */
  steps: MoveRunStep[];
  status: MoveStatus;
  report: MoveReport | null;
  error: IpcError | null;
  startedAt: number;
}

const DETAIL_MAX = 80;
const RANK: Record<StepState, number> = { pending: 0, started: 1, done: 2, warned: 2, failed: 2 };

const store = writable<Map<number, MoveRun>>(new Map());

export const moves: Readable<Map<number, MoveRun>> = { subscribe: store.subscribe };

/** The session whose Transfer sheet is open, or null. */
export const transferSheetFor = writable<number | null>(null);

function blank(): MoveRunStep[] {
  return MOVE_STEPS.map((step) => ({ step, state: 'pending' as StepState, detail: null }));
}

function put(run: MoveRun): void {
  store.update((m) => new Map(m).set(run.sessionId, run));
}

export function activeMoveFor(sessionId: number): MoveRun | undefined {
  const run = get(store).get(sessionId);
  return run?.status === 'running' ? run : undefined;
}

/** 1-based number of the step the run has reached (at least 1). */
export function stepNumber(run: MoveRun): number {
  let n = 1;
  run.steps.forEach((s, i) => {
    if (s.state !== 'pending') n = i + 1;
  });
  return n;
}

/** Start a move without waiting for it. A no-op while one is running. */
export function startMove(session: SessionRow, toHost: string, opts: { keepSource: boolean }): void {
  if (activeMoveFor(session.id)) return;
  put({
    sessionId: session.id,
    sessionName: session.tmux_name,
    fromHost: session.host_alias,
    toHost,
    keepSource: opts.keepSource,
    origin: 'local',
    steps: blank(),
    status: 'running',
    report: null,
    error: null,
    startedAt: Date.now(),
  });
  void moveSession(session.id, toHost, { keepSource: opts.keepSource }).then((r) =>
    settle(session.id, r),
  );
}

function settle(sessionId: number, r: Result<MoveReport>): void {
  const run = get(store).get(sessionId);
  if (!run || run.origin !== 'local' || run.status !== 'running') return;
  const sheetOpen = get(transferSheetFor) === sessionId;
  if (r.ok) {
    put({
      ...run,
      status: 'done',
      report: r.value,
      // Events may never have arrived (an old hub, a dropped stream): the
      // result is the truth, so everything not warned is done.
      steps: run.steps.map((s) => (s.state === 'warned' ? s : { ...s, state: 'done' as StepState })),
    });
    if (get(selectedSession)?.id === sessionId) selectSession(r.value.target);
    if (!sheetOpen) push({ kind: 'success', message: `Moved ${run.sessionName} to ${run.toHost}` });
    return;
  }
  const hasFailed = run.steps.some((s) => s.state === 'failed');
  put({
    ...run,
    status: r.error.code === 'E_MOVE_PARTIAL' ? 'partial' : 'failed',
    error: r.error,
    steps: hasFailed
      ? run.steps
      : run.steps.map((s) => (s.state === 'started' ? { ...s, state: 'failed' as StepState } : s)),
  });
  if (!sheetOpen) pushError(r.error, `Move of ${run.sessionName} failed`);
}

function observed(p: MoveProgress): MoveRun {
  const row = get(sessions).find((s) => s.id === p.session_id);
  return {
    sessionId: p.session_id,
    sessionName: row?.tmux_name ?? `session ${p.session_id}`,
    fromHost: row?.host_alias ?? '',
    toHost: p.to_host,
    keepSource: null,
    origin: 'observed',
    steps: blank(),
    status: 'running',
    report: null,
    error: null,
    startedAt: Date.now(),
  };
}

/** Patch a run from one `move:progress` event. */
export function applyMoveProgress(p: MoveProgress): void {
  const i = p.index - 1;
  if (!Number.isInteger(i) || i < 0 || i >= MOVE_STEPS.length || MOVE_STEPS[i] !== p.step) return;
  let run = get(store).get(p.session_id);
  if (run && run.status !== 'running') {
    // Settled. Only the first event of a NEW move of this session replaces it.
    if (!(i === 0 && p.state === 'started')) return;
    run = undefined;
  }
  run ??= observed(p);
  const steps = run.steps.map((s, n) => {
    if (n < i) return RANK[s.state] < 2 ? { ...s, state: 'done' as StepState } : s;
    if (n > i || RANK[p.state] <= RANK[s.state]) return s;
    return { ...s, state: p.state, detail: p.detail === null ? null : p.detail.slice(0, DETAIL_MAX) };
  });
  let status: MoveStatus = run.status;
  if (run.origin === 'observed') {
    if (p.state === 'failed') status = 'failed';
    else if (i === MOVE_STEPS.length - 1 && (p.state === 'done' || p.state === 'warned')) status = 'done';
  }
  put({ ...run, steps, status });
}

/** Forget a settled run. A running one cannot be dismissed. */
export function dismissMove(sessionId: number): void {
  if (activeMoveFor(sessionId)) return;
  store.update((m) => {
    const next = new Map(m);
    next.delete(sessionId);
    return next;
  });
}

export function resetMovesForTest(): void {
  store.set(new Map());
  transferSheetFor.set(null);
}
```

- [ ] **Step 4: Run the tests**

Run: `npx vitest run src/lib/moves.test.ts` — Expected: all pass. Then `npx svelte-check` — 0 errors.

- [ ] **Step 5: Commit**

```bash
git -C <worktree> add src/lib/moves.ts src/lib/moves.test.ts
git -C <worktree> commit -m "feat(ui): moves store — one run per moving session, fed by events and the result"
```

---

### Task 4: Eligibility and error sentences

**Files:**
- Create: `src/lib/moveEligibility.ts`, `src/lib/moveErrors.ts`
- Test: `src/lib/moveEligibility.test.ts`, `src/lib/moveErrors.test.ts`

**Interfaces:**
- Consumes: `SessionRow` (`./sessions`), `HostRow` (`./hosts`), `hubActionBlocked`, `HubStatus` (`./hub`), `HubConnection` (`./hub_connection`), `IpcError` (`./result`).
- Produces: `canMoveSession(s: SessionRow): boolean`; `moveTargetsFor(s: SessionRow, hosts: HostRow[]): HostRow[]`; `moveBlockedReason(status: HubStatus, conn: HubConnection): string | null`; `describeMoveError(error: IpcError | null, status: 'failed' | 'partial', toHost: string): { what: string; standing: string }`.

- [ ] **Step 1: Write the failing tests**

`src/lib/moveEligibility.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { canMoveSession, moveTargetsFor } from './moveEligibility';
import type { SessionRow } from './sessions';
import type { HostRow } from './hosts';

const s = (over: Partial<SessionRow>) =>
  ({ id: 1, host_alias: 'a', kind: 'work', worktree_id: 10, claude_session_id: 'x', ...over }) as SessionRow;
const h = (alias: string, over: Partial<HostRow> = {}) =>
  ({ alias, ssh_alias: alias, reachable: true, hidden: false, provisioned: true, transport: 'ssh', ...over }) as HostRow;

describe('canMoveSession', () => {
  it('needs a work session with a worktree and a Claude session id', () => {
    expect(canMoveSession(s({}))).toBe(true);
    expect(canMoveSession(s({ kind: 'shell' }))).toBe(false);
    expect(canMoveSession(s({ worktree_id: null }))).toBe(false);
    expect(canMoveSession(s({ claude_session_id: null }))).toBe(false);
  });
});

describe('moveTargetsFor', () => {
  it('offers other visible, reachable hosts that are provisioned or local', () => {
    const hosts = [
      h('a'), h('b'), h('down', { reachable: false }), h('hid', { hidden: true }),
      h('bare', { provisioned: false }), h('local', { provisioned: false }),
    ];
    expect(moveTargetsFor(s({}), hosts).map((x) => x.alias)).toEqual(['b', 'local']);
  });
});
```

`src/lib/moveErrors.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { describeMoveError } from './moveErrors';

const err = (code: string, details?: unknown, message = 'raw backend text') => ({ code, message, details });

describe('describeMoveError', () => {
  it.each([
    ['E_MOVE_DIRTY', 'uncommitted'],
    ['E_MOVE_UNPUSHED', 'not pushed'],
    ['E_MOVE_MIDOP', 'in the middle of'],
    ['E_MOVE_TARGET_DIRTY', 'turanga already has uncommitted'],
    ['E_MOVE_TOO_LARGE', 'too large'],
    ['E_LOCAL_ONLY', 'hub'],
  ])('%s has its own sentence', (code, fragment) => {
    const d = describeMoveError(err(code), 'failed', 'turanga');
    expect(d.what).toContain(fragment);
    expect(d.what).not.toContain('raw backend text');
  });

  it.each(['seed', 'haves', 'snapshot', 'download', 'upload', 'fetch', 'apply', 'verify', 'target'])(
    'E_MOVE_CARRY at %s has its own sentence',
    (step) => {
      const d = describeMoveError(err('E_MOVE_CARRY', { step }), 'failed', 'turanga');
      expect(d.what).not.toContain('raw backend text');
      expect(d.what.length).toBeGreaterThan(20);
    },
  );

  it('tells a timeout from a failure by the cause code', () => {
    for (const cause_code of ['E_SSH_TIMEOUT', 'E_TIMEOUT']) {
      const d = describeMoveError(err('E_MOVE_CARRY', { step: 'fetch', cause_code }), 'failed', 'turanga');
      expect(d.what).toBe('The target could not take in the carried commits. The host timed out.');
    }
  });

  it('tells busy from already-moving for E_INVALID_STATE', () => {
    expect(describeMoveError(err('E_INVALID_STATE', null, 'a move of session 5 is already in progress'), 'failed', 't').what)
      .toBe('This session is already being moved.');
    expect(describeMoveError(err('E_INVALID_STATE', null, 'the source Claude is working'), 'failed', 't').what)
      .toBe('the source Claude is working');
  });

  it('falls back to the backend message for an unknown code and an unknown carry step', () => {
    expect(describeMoveError(err('E_WHATEVER'), 'failed', 't').what).toBe('raw backend text');
    expect(describeMoveError(err('E_MOVE_CARRY', { step: 'novel' }), 'failed', 't').what).toBe('raw backend text');
  });

  it('says where things stand', () => {
    expect(describeMoveError(err('E_MOVE_CARRY', { step: 'fetch' }), 'failed', 'turanga').standing)
      .toBe('The source session was not touched. Anything copied to turanga was cleaned up.');
    expect(describeMoveError(err('E_MOVE_PARTIAL'), 'partial', 'turanga').standing)
      .toBe('The session is running on turanga, but the source could not be retired. Both sessions were left as they are.');
  });

  it('an observed failure has no error object', () => {
    const d = describeMoveError(null, 'failed', 'turanga');
    expect(d.what).toBe('The move failed. It was started elsewhere, so the reason is in that window or in the session timeline.');
  });
});
```

- [ ] **Step 2: Run and confirm failure**

Run: `npx vitest run src/lib/moveEligibility.test.ts src/lib/moveErrors.test.ts`
Expected: FAIL — both imports unresolved.

- [ ] **Step 3: Implement**

`src/lib/moveEligibility.ts`:

```ts
// Who can move, and where to. One copy, shared by the terminal-header chip,
// the details-panel button and the Transfer sheet.
import type { HostRow } from './hosts';
import { hubActionBlocked, type HubStatus } from './hub';
import type { HubConnection } from './hub_connection';
import type { SessionRow } from './sessions';

/** Only a worktree-backed work session with a Claude id can move: there is a
 *  branch to recreate and a conversation to resume. */
export function canMoveSession(s: SessionRow): boolean {
  return s.kind === 'work' && s.worktree_id !== null && s.claude_session_id !== null;
}

/** Other visible, reachable hosts that can run a session. */
export function moveTargetsFor(s: SessionRow, hosts: HostRow[]): HostRow[] {
  return hosts.filter(
    (h) =>
      h.alias !== s.host_alias && !h.hidden && h.reachable && (h.provisioned || h.alias === 'local'),
  );
}

/** Why a move cannot be started right now (a hub client that is offline), or null. */
export function moveBlockedReason(status: HubStatus, conn: HubConnection): string | null {
  return hubActionBlocked('move_session', status, conn);
}
```

`src/lib/moveErrors.ts`:

```ts
// A failed move, in words. The backend's messages are written for an
// operator reading a log; the sheet needs one sentence on what failed and one
// on where things stand.
import type { IpcError } from './result';

export interface MoveFailure {
  what: string;
  standing: string;
}

const CARRY_STEP: Record<string, string> = {
  seed: 'The target could not be given a clone of the repository to receive the work into.',
  haves: 'The target could not say which commits it already has.',
  snapshot: 'The source could not take a snapshot of its uncommitted work.',
  download: 'The git work could not be read from the source.',
  upload: 'The git work could not be written to the target.',
  fetch: 'The target could not take in the carried commits.',
  apply: 'The uncommitted work could not be replayed on the target.',
  verify: 'The target did not end up with the same uncommitted work as the source.',
  target: 'The target worktree could not be prepared.',
};

/** Both transports report a blown wall clock as `E_SSH_TIMEOUT`; `E_TIMEOUT`
 *  is the older name and costs nothing to keep. */
const TIMEOUTS = new Set(['E_SSH_TIMEOUT', 'E_TIMEOUT']);

function field(details: unknown, key: string): string | null {
  if (typeof details !== 'object' || details === null) return null;
  const v = (details as Record<string, unknown>)[key];
  return typeof v === 'string' ? v : null;
}

function what(error: IpcError, toHost: string): string {
  switch (error.code) {
    case 'E_MOVE_DIRTY':
      return 'The source has uncommitted work, and this move was asked to refuse that.';
    case 'E_MOVE_UNPUSHED':
      return 'The source has commits that are not pushed, and this move was asked to refuse that.';
    case 'E_MOVE_MIDOP':
      return 'The source is in the middle of a merge, rebase or similar. Finish or abort it, then move.';
    case 'E_MOVE_TARGET_DIRTY':
      return `${toHost} already has uncommitted work in this worktree. Clean it up there first.`;
    case 'E_MOVE_TOO_LARGE':
      return 'The work to carry is too large for a move.';
    case 'E_LOCAL_ONLY':
      return 'This desktop is a window onto a hub, and the hub refused the move.';
    case 'E_INVALID_STATE':
      return error.message.includes('already in progress')
        ? 'This session is already being moved.'
        : error.message;
    case 'E_MOVE_PARTIAL':
      return 'The new session started, but the last step of the move failed.';
    case 'E_MOVE_CARRY': {
      const step = field(error.details, 'step');
      const sentence = step === null ? undefined : CARRY_STEP[step];
      if (sentence === undefined) return error.message;
      return TIMEOUTS.has(field(error.details, 'cause_code') ?? '')
        ? `${sentence} The host timed out.`
        : sentence;
    }
    default:
      return error.message;
  }
}

export function describeMoveError(
  error: IpcError | null,
  status: 'failed' | 'partial',
  toHost: string,
): MoveFailure {
  const standing =
    status === 'partial'
      ? `The session is running on ${toHost}, but the source could not be retired. Both sessions were left as they are.`
      : `The source session was not touched. Anything copied to ${toHost} was cleaned up.`;
  if (error === null) {
    return {
      what: 'The move failed. It was started elsewhere, so the reason is in that window or in the session timeline.',
      standing,
    };
  }
  return { what: what(error, toHost), standing };
}
```

- [ ] **Step 4: Run** — `npx vitest run src/lib/moveEligibility.test.ts src/lib/moveErrors.test.ts` → all pass; `npx svelte-check` → 0 errors.

- [ ] **Step 5: Commit**

```bash
git -C <worktree> add src/lib/moveEligibility.ts src/lib/moveEligibility.test.ts src/lib/moveErrors.ts src/lib/moveErrors.test.ts
git -C <worktree> commit -m "feat(ui): move eligibility in one place, and move errors in words"
```

---

### Task 5: `TransferSheet.svelte`

**Files:**
- Create: `src/lib/TransferSheet.svelte`
- Test: `src/lib/TransferSheet.test.ts`

**Interfaces:**
- Consumes: Task 3 (`moves`, `transferSheetFor`, `startMove`, `dismissMove`, `MoveRun`), Task 1 (`stepLabel`), Task 4 (`moveTargetsFor`, `moveBlockedReason`, `describeMoveError`); existing `Modal.svelte` (`title`, `onclose`, `width`, `testid`, children), `sessions`, `hosts`, `hubStatus` (`./hub`), `hubConnection` (`./hub_connection`), `selectSession` (`./selection`).
- Produces: a prop-less component. Mounted once (Task 7); visible while `$transferSheetFor` is a session id that has a run or a live row.

- [ ] **Step 1: Write the failing tests** — `src/lib/TransferSheet.test.ts`:

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import TransferSheet from './TransferSheet.svelte';
import { moves, transferSheetFor, startMove, applyMoveProgress, resetMovesForTest } from './moves';
import { MOVE_STEPS, type MoveProgress, type MoveStep, type MoveStepState } from './moveProgress';
import { sessions, type SessionRow } from './sessions';
import { hosts, type HostRow } from './hosts';
import { selectedSession, selectSession } from './selection';

const mockInvoke = invoke as ReturnType<typeof vi.fn>;
const flush = () => new Promise((r) => setTimeout(r, 0));

const source = {
  id: 5, tmux_name: 'dev-foo', host_alias: 'mefistos', kind: 'work', worktree_id: 10,
  project_id: 1, claude_session_id: '550e8400-e29b-41d4-a716-446655440000',
  parent_session_id: null, tags: [],
} as unknown as SessionRow;
const target = { ...source, id: 6, host_alias: 'turanga', parent_session_id: 5 } as SessionRow;
const host = (alias: string, over: Partial<HostRow> = {}) =>
  ({ alias, ssh_alias: alias, reachable: true, hidden: false, provisioned: true,
     claude_version: null, tmux_version: null, last_pinged_at: 1, account_uuid: null,
     transport: 'ssh', ...over }) as HostRow;

const report = {
  source_session_id: 5, target_session_id: 6, from_host: 'mefistos', to_host: 'turanga',
  tmux_name: 'dev-foo', claude_session_id: source.claude_session_id, branch: 'feat',
  target_cwd: '/r', transcript_bytes: 10, source_killed: true,
  warnings: ['origin was unreachable from turanga', 'replaced an existing 4-byte transcript'],
  carried: {
    commits: 2, bundle_bytes: 100,
    dirty_entries: [{ status: ' M', path: 'a.rs' }],
    ignored_carried: [{ path: '.env', bytes: 4 }],
    ignored_left_behind: [{ path: 'big.bin', bytes: 9000000, reason: 'over_cap' }],
    target_seeded: 'existing',
    session_state: { carried: [{ path: 'subagents/a.jsonl', bytes: 9 }], kept_target: ['custom-title.json'], left_behind: [] },
    memory: { carried: [{ path: 'note.md', bytes: 3 }], kept_target: ['Other.md'], identical: 1, index_lines_added: 1, left_behind: [] },
  },
  target,
};

const ev = (step: MoveStep, state: MoveStepState, detail: string | null = null): MoveProgress => ({
  session_id: 5, to_host: 'turanga', step, index: MOVE_STEPS.indexOf(step) + 1, total: 9, state, detail,
});

function pendingMove() {
  let resolve!: (v: unknown) => void;
  let reject!: (e: unknown) => void;
  mockInvoke.mockImplementation((cmd: string) =>
    cmd === 'move_session'
      ? new Promise((res, rej) => { resolve = res; reject = rej; })
      : Promise.resolve(undefined),
  );
  return { resolve: (v: unknown) => resolve(v), reject: (e: unknown) => reject(e) };
}

beforeEach(() => {
  mockInvoke.mockReset();
  resetMovesForTest();
  sessions.set([source]);
  hosts.set([host('mefistos'), host('turanga'), host('down', { reachable: false })]);
  selectSession(null);
});

describe('TransferSheet', () => {
  it('renders nothing while no sheet is open', () => {
    render(TransferSheet);
    expect(screen.queryByTestId('move-dialog')).toBeNull();
  });

  it('setup: offers eligible targets and starts the move', async () => {
    pendingMove();
    transferSheetFor.set(5);
    render(TransferSheet);
    await tick();
    const select = (await screen.findByTestId('move-target')) as HTMLSelectElement;
    expect(Array.from(select.options, (o) => o.value)).toEqual(['turanga']);
    await fireEvent.click(screen.getByTestId('move-keep-source'));
    await fireEvent.click(screen.getByTestId('confirm-move'));
    expect(mockInvoke).toHaveBeenCalledWith('move_session', {
      args: { session_id: 5, target_host_alias: 'turanga', keep_source: true, strict: false },
    });
    expect(await screen.findByTestId('transfer-steps')).toBeTruthy();
  });

  it('setup: with no eligible target the button is disabled and says why', async () => {
    hosts.set([host('mefistos')]);
    transferSheetFor.set(5);
    render(TransferSheet);
    expect(await screen.findByTestId('move-no-targets')).toBeTruthy();
    expect((screen.getByTestId('confirm-move') as HTMLButtonElement).disabled).toBe(true);
  });

  it('progress: shows the nine steps with their states and details; Close leaves the move running', async () => {
    pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    applyMoveProgress(ev('git', 'done', '2 commits'));
    applyMoveProgress(ev('replay', 'started'));
    await tick();
    const items = (await screen.findByTestId('transfer-steps')).querySelectorAll('li');
    expect(items).toHaveLength(9);
    expect(items[3].getAttribute('data-state')).toBe('done');
    expect(items[3].textContent).toContain('Carry the git work');
    expect(items[3].textContent).toContain('2 commits');
    expect(items[4].getAttribute('data-state')).toBe('started');
    expect(items[7].textContent).toContain('Start on turanga');
    expect(items[8].getAttribute('data-state')).toBe('pending');
    await fireEvent.click(screen.getByTestId('transfer-close'));
    expect(get(transferSheetFor)).toBeNull();
    expect(get(moves).get(5)!.status).toBe('running');
  });

  it('progress: an observed run says it was started elsewhere', async () => {
    applyMoveProgress(ev('check', 'started'));
    transferSheetFor.set(5);
    render(TransferSheet);
    expect((await screen.findByTestId('move-dialog')).textContent).toContain('Started elsewhere');
  });

  it('result: counts, one warning per line, details, open and done', async () => {
    const p = pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    p.resolve(report);
    await flush();
    await tick();
    const result = await screen.findByTestId('transfer-result');
    expect(result.textContent).toContain('2 commits');
    expect(result.textContent).toContain('1 uncommitted entry');
    expect(result.textContent).toContain('1 ignored file');
    expect(result.textContent).toContain('1 left behind');
    expect(result.querySelectorAll('[data-testid="transfer-warning"]')).toHaveLength(2);
    expect(result.textContent).not.toContain('big.bin');
    await fireEvent.click(screen.getByTestId('transfer-details'));
    expect(result.textContent).toContain('big.bin');
    expect(result.textContent).toContain('over the size cap');
    expect(result.textContent).toContain('Other.md');
    await fireEvent.click(screen.getByTestId('transfer-open-target'));
    expect(get(selectedSession)?.id).toBe(6);
    expect(get(transferSheetFor)).toBeNull();
    expect(get(moves).has(5)).toBe(false);
  });

  it('result: a clean pushed branch says there was nothing to carry', async () => {
    const p = pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    p.resolve({ ...report, warnings: [], carried: { ...report.carried, commits: 0, dirty_entries: [] } });
    await flush();
    await tick();
    expect((await screen.findByTestId('transfer-result')).textContent)
      .toContain('Nothing to carry — the branch was pushed and clean');
    expect(screen.queryByTestId('transfer-warning')).toBeNull();
  });

  it('failure: a sentence, where things stand, the failed step, raw details collapsed; Done dismisses', async () => {
    const p = pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    applyMoveProgress(ev('git', 'started'));
    p.reject({ code: 'E_MOVE_CARRY', message: 'raw backend text', details: { step: 'fetch', stderr: 'fatal: bad object' } });
    await flush();
    await tick();
    const failure = await screen.findByTestId('transfer-failure');
    expect(failure.textContent).toContain('The target could not take in the carried commits.');
    expect(failure.textContent).toContain('The source session was not touched.');
    expect(screen.getByTestId('transfer-steps').querySelectorAll('li')[3].getAttribute('data-state')).toBe('failed');
    const raw = failure.querySelector('details')!;
    expect(raw.open).toBe(false);
    expect(raw.textContent).toContain('E_MOVE_CARRY');
    expect(raw.textContent).toContain('fatal: bad object');
    await fireEvent.click(screen.getByTestId('transfer-done'));
    expect(get(moves).has(5)).toBe(false);
    expect(get(transferSheetFor)).toBeNull();
  });

  it('partial: links the new session', async () => {
    sessions.set([source, target]);
    const p = pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    p.reject({ code: 'E_MOVE_PARTIAL', message: 'x', details: { target_session_id: 6 } });
    await flush();
    await tick();
    expect((await screen.findByTestId('transfer-failure')).textContent).toContain('is running on turanga');
    await fireEvent.click(screen.getByTestId('transfer-open-target'));
    expect(get(selectedSession)?.id).toBe(6);
  });

  it('closes itself when the session is gone and there is no run', async () => {
    transferSheetFor.set(99);
    render(TransferSheet);
    await tick();
    expect(get(transferSheetFor)).toBeNull();
  });
});
```

As in Task 3, adapt `p.reject(...)`'s argument to whatever rejection shape `invokeCmd` turns into `{ ok: false, error }` — not the assertions.

- [ ] **Step 2: Run and confirm failure**

Run: `npx vitest run src/lib/TransferSheet.test.ts` — Expected: FAIL, `./TransferSheet.svelte` unresolved.

- [ ] **Step 3: Implement `src/lib/TransferSheet.svelte`**

```svelte
<script lang="ts">
  import Modal from './Modal.svelte';
  import { hosts } from './hosts';
  import { hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { moveBlockedReason, moveTargetsFor } from './moveEligibility';
  import { describeMoveError } from './moveErrors';
  import { stepLabel } from './moveProgress';
  import { dismissMove, moves, startMove, transferSheetFor } from './moves';
  import { selectSession } from './selection';
  import { sessions } from './sessions';

  // One sheet for the whole app. Which view shows is a function of the run:
  // none → setup, running → progress, done → result, failed/partial → failure.
  const id = $derived($transferSheetFor);
  const run = $derived(id === null ? undefined : $moves.get(id));
  const session = $derived(id === null ? undefined : $sessions.find((s) => s.id === id));
  const targets = $derived(session ? moveTargetsFor(session, $hosts) : []);
  const blocked = $derived(moveBlockedReason($hubStatus, $hubConnection));

  let target = $state('');
  let keepSource = $state(false);
  let showDetails = $state(false);

  // A fresh setup each time the sheet opens on a session.
  $effect(() => {
    if (id !== null && !run) {
      if (!targets.some((h) => h.alias === target)) target = targets[0]?.alias ?? '';
    }
  });
  $effect(() => {
    void id;
    keepSource = false;
    showDetails = false;
  });
  // Nothing to show: the row is gone and no run remembers it.
  $effect(() => {
    if (id !== null && !run && !session) transferSheetFor.set(null);
  });

  const failure = $derived(
    run && (run.status === 'failed' || run.status === 'partial')
      ? describeMoveError(run.error, run.status, run.toHost)
      : null,
  );
  const carried = $derived(run?.report?.carried ?? null);

  /** The session a finished move produced, when this window can find it. */
  const newSession = $derived.by(() => {
    if (!run) return undefined;
    if (run.report) return run.report.target;
    const details = run.error?.details;
    const tid =
      typeof details === 'object' && details !== null
        ? (details as Record<string, unknown>).target_session_id
        : undefined;
    if (typeof tid === 'number') return $sessions.find((s) => s.id === tid);
    return $sessions.find((s) => s.parent_session_id === run.sessionId && s.host_alias === run.toHost);
  });

  const n = (count: number, one: string, many = `${one}s`) => `${count} ${count === 1 ? one : many}`;
  const REASON = {
    denylisted: 'never carried (secrets, caches, build output)',
    over_cap: 'over the size cap',
    unsupported_name: 'a file name that cannot be carried safely',
  } as const;

  function close() {
    transferSheetFor.set(null);
  }
  function transfer() {
    if (!session || !target || blocked !== null) return;
    startMove(session, target, { keepSource });
  }
  function done() {
    if (run) dismissMove(run.sessionId);
    close();
  }
  function openTarget() {
    if (newSession) selectSession(newSession);
    done();
  }

  const title = $derived(
    !run
      ? `Transfer ${session?.tmux_name ?? ''}`
      : run.status === 'running'
        ? `Moving to ${run.toHost}`
        : run.status === 'done'
          ? `Moved to ${run.toHost}`
          : 'The move did not finish',
  );
</script>

{#snippet steps()}
  {#if run}
    <ol class="steps" data-testid="transfer-steps">
      {#each run.steps as s (s.step)}
        <li data-state={s.state}>
          <span class="mark" aria-hidden="true"></span>
          <span class="label">{stepLabel(s.step, run.toHost)}</span>
          {#if s.detail}<span class="detail">{s.detail}</span>{/if}
        </li>
      {/each}
    </ol>
  {/if}
{/snippet}

{#if id !== null && (run || session)}
  <Modal {title} onclose={close} width="480px" testid="move-dialog">
    {#if !run && session}
      <div class="field"><span class="key">From</span> {session.host_alias}</div>
      {#if targets.length === 0}
        <p class="note" data-testid="move-no-targets">No other reachable, provisioned host.</p>
      {:else}
        <label class="field">
          <span class="key">To</span>
          <select bind:value={target} data-testid="move-target">
            {#each targets as h (h.alias)}
              <option value={h.alias}>{h.alias}</option>
            {/each}
          </select>
        </label>
        <label class="field">
          <input type="checkbox" bind:checked={keepSource} data-testid="move-keep-source" />
          Keep this session running
        </label>
      {/if}
      <p class="note">
        Uncommitted and unpushed work, small ignored files, subagents and project memory travel
        too. Nothing is pushed or committed.
      </p>
      <div class="buttons">
        <button onclick={close}>Cancel</button>
        <button
          onclick={transfer}
          disabled={!target || blocked !== null}
          title={blocked ?? ''}
          data-testid="confirm-move"
        >
          Transfer
        </button>
      </div>
    {:else if run && run.status === 'running'}
      {#if run.origin === 'observed'}
        <p class="note">Started elsewhere — this window is following along.</p>
      {/if}
      {@render steps()}
      <div class="buttons">
        <button onclick={close} data-testid="transfer-close">Close</button>
      </div>
    {:else if run && run.status === 'done'}
      <div data-testid="transfer-result">
        {#if carried}
          <ul class="summary">
            <li>
              {#if carried.commits === 0 && carried.dirty_entries.length === 0}
                Nothing to carry — the branch was pushed and clean
              {:else}
                {n(carried.commits, 'commit')} · {n(carried.dirty_entries.length, 'uncommitted entry', 'uncommitted entries')}
              {/if}
            </li>
            <li>
              {n(carried.ignored_carried.length, 'ignored file')}
              {#if carried.ignored_left_behind.length > 0}
                <span class="muted">· {carried.ignored_left_behind.length} left behind</span>
              {/if}
            </li>
            <li>
              {n(carried.session_state.carried.length, 'session file')}
              {#if carried.session_state.kept_target.length > 0}
                <span class="muted">· {carried.session_state.kept_target.length} kept on target</span>
              {/if}
              {#if carried.session_state.left_behind.length > 0}
                <span class="muted">· {carried.session_state.left_behind.length} left behind</span>
              {/if}
            </li>
            <li>
              {n(carried.memory.carried.length, 'memory note')}
              {#if carried.memory.kept_target.length + carried.memory.identical > 0}
                <span class="muted">· {carried.memory.kept_target.length + carried.memory.identical} already there</span>
              {/if}
              {#if carried.memory.index_lines_added > 0}
                <span class="muted">· {n(carried.memory.index_lines_added, 'index line')}</span>
              {/if}
            </li>
          </ul>
          {#if run.report && !run.report.source_killed}
            <p class="note">The source session keeps running on {run.fromHost}.</p>
          {/if}
          {#if run.report && run.report.warnings.length > 0}
            <div class="warnings">
              <p class="warn-head">{n(run.report.warnings.length, 'warning')}</p>
              {#each run.report.warnings as w (w)}
                <p class="warn" data-testid="transfer-warning">{w}</p>
              {/each}
            </div>
          {/if}
          {#if showDetails}
            <div class="details">
              {#each [...carried.ignored_left_behind, ...carried.session_state.left_behind, ...carried.memory.left_behind] as f (f.path)}
                <p><code>{f.path}</code> — {REASON[f.reason]}</p>
              {/each}
              {#each carried.session_state.kept_target as p (p)}
                <p><code>{p}</code> — kept the target's copy</p>
              {/each}
              {#each carried.memory.kept_target as p (p)}
                <p><code>{p}</code> — kept the target's note</p>
              {/each}
              {#if carried.memory.identical > 0}
                <p>{n(carried.memory.identical, 'note')} identical on both hosts</p>
              {/if}
            </div>
          {/if}
        {:else}
          <p class="note">Started elsewhere — this window has no report for it.</p>
        {/if}
      </div>
      <div class="buttons">
        {#if carried}
          <button onclick={() => (showDetails = !showDetails)} data-testid="transfer-details">
            {showDetails ? 'Hide details' : 'Details'}
          </button>
        {/if}
        {#if newSession}
          <button onclick={openTarget} data-testid="transfer-open-target">Open on {run.toHost}</button>
        {/if}
        <button onclick={done} data-testid="transfer-done">Done</button>
      </div>
    {:else if run && failure}
      <div data-testid="transfer-failure">
        <p class="what">{failure.what}</p>
        <p class="note">{failure.standing}</p>
        {@render steps()}
        {#if run.error}
          <details>
            <summary>Raw details</summary>
            <pre>{run.error.code}
{run.error.message}{#if typeof run.error.details === 'object' && run.error.details !== null && typeof (run.error.details as Record<string, unknown>).stderr === 'string'}

{(run.error.details as Record<string, unknown>).stderr}{/if}</pre>
          </details>
        {/if}
      </div>
      <div class="buttons">
        {#if run.status === 'partial' && newSession}
          <button onclick={openTarget} data-testid="transfer-open-target">Open on {run.toHost}</button>
        {/if}
        <button onclick={done} data-testid="transfer-done">Done</button>
      </div>
    {/if}
  </Modal>
{/if}

<style>
  .field { display: flex; align-items: center; gap: 0.5rem; margin: 0.35rem 0; font-size: 0.85rem; }
  .key { width: 3rem; color: var(--fg-muted); }
  .field select { flex: 1; }
  .note, .muted { color: var(--fg-muted); font-size: 0.8rem; }
  .what { font-size: 0.9rem; margin: 0 0 0.35rem; }
  .steps, .summary { list-style: none; margin: 0.5rem 0; padding: 0; font-size: 0.85rem; }
  .steps li { display: flex; align-items: baseline; gap: 0.5rem; padding: 0.15rem 0; }
  .steps li[data-state='pending'] { color: var(--fg-muted); }
  .steps li[data-state='failed'] .label { color: var(--danger, #d33); }
  .steps li[data-state='warned'] .label { color: var(--warning, #b80); }
  .mark { width: 1rem; text-align: center; }
  .steps li[data-state='pending'] .mark::before { content: '○'; }
  .steps li[data-state='started'] .mark::before { content: '◌'; }
  .steps li[data-state='done'] .mark::before { content: '✓'; }
  .steps li[data-state='warned'] .mark::before { content: '!'; }
  .steps li[data-state='failed'] .mark::before { content: '✕'; }
  .detail { margin-left: auto; color: var(--fg-muted); font-size: 0.75rem; }
  .summary li { padding: 0.15rem 0; }
  .warnings { border-top: 1px solid var(--border); margin-top: 0.5rem; padding-top: 0.4rem; }
  .warn-head { color: var(--warning, #b80); font-size: 0.85rem; margin: 0; }
  .warn, .details p { font-size: 0.8rem; color: var(--fg-muted); margin: 0.15rem 0; }
  .details { border-top: 1px solid var(--border); margin-top: 0.5rem; padding-top: 0.4rem; }
  pre { white-space: pre-wrap; font-size: 0.75rem; }
  .buttons { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 0.75rem; }
</style>
```

The app defines `--fg`, `--fg-muted`, `--border`, `--accent` and `--bg-pane`; it has no danger or warning variable, which is why those two carry a literal fallback. If `SessionDetails.svelte`'s `<style>` already writes a red or amber for errors, reuse that literal instead of `#d33` / `#b80`.

- [ ] **Step 4: Run** — `npx vitest run src/lib/TransferSheet.test.ts` → all pass; `npx svelte-check` → 0 errors and no new warnings from this file.

- [ ] **Step 5: Commit**

```bash
git -C <worktree> add src/lib/TransferSheet.svelte src/lib/TransferSheet.test.ts
git -C <worktree> commit -m "feat(ui): Transfer sheet — setup, live steps, a readable result and failure"
```

---

### Task 6: `TransferChip.svelte` in the terminal header

**Files:**
- Create: `src/lib/TransferChip.svelte`
- Test: `src/lib/TransferChip.test.ts`
- Modify: `src/lib/TerminalView.svelte` (the header's `<span class="host">…</span>`, and the hub-client attach-line panel), `src/lib/TerminalView.test.ts`, `src/lib/TerminalView.hub.test.ts`

**Interfaces:**
- Consumes: Task 3 (`moves`, `transferSheetFor`, `stepNumber`), Task 4 (`canMoveSession`, `moveBlockedReason`), `hubStatus`, `hubConnection`, `SessionRow`.
- Produces: `<TransferChip session={SessionRow} />`. Its text always begins `on <host>` when idle, so the existing header assertions (`'on alpha'`) hold.

- [ ] **Step 1: Write the failing tests** — `src/lib/TransferChip.test.ts`:

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(() => new Promise(() => {})) }));
import TransferChip from './TransferChip.svelte';
import { transferSheetFor, startMove, applyMoveProgress, resetMovesForTest } from './moves';
import { MOVE_STEPS, type MoveProgress, type MoveStep, type MoveStepState } from './moveProgress';
import { sessions, type SessionRow } from './sessions';

const movable = {
  id: 5, tmux_name: 'dev-foo', host_alias: 'alpha', kind: 'work', worktree_id: 10,
  project_id: 1, claude_session_id: 'x', parent_session_id: null, tags: [],
} as unknown as SessionRow;

const ev = (step: MoveStep, state: MoveStepState): MoveProgress => ({
  session_id: 5, to_host: 'beta', step, index: MOVE_STEPS.indexOf(step) + 1, total: 9, state, detail: null,
});

beforeEach(() => {
  resetMovesForTest();
  sessions.set([movable]);
});

describe('TransferChip', () => {
  it('idle and movable: a button on the host name that opens the sheet', async () => {
    const { container } = render(TransferChip, { props: { session: movable } });
    expect(container.textContent?.replace(/\s+/g, ' ').trim()).toContain('on alpha');
    await fireEvent.click(screen.getByTestId('transfer-chip'));
    expect(get(transferSheetFor)).toBe(5);
  });

  it('not movable: plain text, no button', () => {
    const { container } = render(TransferChip, { props: { session: { ...movable, kind: 'shell' } as SessionRow } });
    expect(container.textContent?.replace(/\s+/g, ' ').trim()).toBe('on alpha');
    expect(screen.queryByTestId('transfer-chip')).toBeNull();
  });

  it('running: the live indicator counts the step and reopens the sheet', async () => {
    startMove(movable, 'beta', { keepSource: false });
    applyMoveProgress(ev('replay', 'started'));
    render(TransferChip, { props: { session: movable } });
    await tick();
    const live = screen.getByTestId('transfer-live');
    expect(live.textContent?.replace(/\s+/g, ' ').trim()).toBe('⇄ moving to beta · 5/9');
    await fireEvent.click(live);
    expect(get(transferSheetFor)).toBe(5);
  });

  it('settled and not dismissed: says how it ended', async () => {
    applyMoveProgress(ev('check', 'started'));
    applyMoveProgress(ev('check', 'failed'));
    render(TransferChip, { props: { session: movable } });
    await tick();
    expect(screen.getByTestId('transfer-live').textContent).toContain('move failed');
  });
});
```

And in `src/lib/TerminalView.test.ts`, directly after the test `'attaches to the selected session by host + name'` (it uses that file's own `makeSession`, `selectSession` and `settle`):

```ts
  it('puts the Transfer chip on the host name for a movable session', async () => {
    const movable = makeSession({
      id: 1, host_alias: 'alpha', kind: 'work', worktree_id: 10, claude_session_id: 'c-1',
    });
    render(TerminalView);
    selectSession(movable);
    await settle();
    const header = screen.getByTestId('terminal-header');
    expect(header.textContent?.replace(/\s+/g, ' ')).toContain('on alpha');
    expect(header.querySelector('[data-testid="transfer-chip"]')).not.toBeNull();
  });
```

In `src/lib/TerminalView.hub.test.ts`, inside `describe('the terminal tab against a hub', …)`, after `'offers the shell command for the selected session instead of a dead pane'`:

```ts
  it('still offers Transfer: the move is hub-routed even though the pane is not', async () => {
    hubStatus.set(remote);
    render(TerminalView);
    selectSession(makeSession({ kind: 'work', worktree_id: 10, claude_session_id: 'c-1' }));
    await settle();
    expect(screen.getByTestId('transfer-chip')).toBeTruthy();
  });
```

If `makeSession` in either file does not accept those fields as overrides, spread them over its result instead (`{ ...makeSession({}), kind: 'work', worktree_id: 10, claude_session_id: 'c-1' }`).

- [ ] **Step 2: Run and confirm failure**

Run: `npx vitest run src/lib/TransferChip.test.ts src/lib/TerminalView.test.ts src/lib/TerminalView.hub.test.ts`
Expected: `TransferChip.test.ts` fails on the unresolved import; the two new TerminalView tests fail on the missing `transfer-chip`.

- [ ] **Step 3: Implement `src/lib/TransferChip.svelte`**

```svelte
<script lang="ts">
  import { hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { canMoveSession, moveBlockedReason } from './moveEligibility';
  import { moves, stepNumber, transferSheetFor } from './moves';
  import type { SessionRow } from './sessions';

  // The terminal header's host name, which is also the Transfer button — and,
  // while this session is moving, the live indicator that reopens the sheet.
  let { session }: { session: SessionRow } = $props();

  const run = $derived($moves.get(session.id));
  const blocked = $derived(moveBlockedReason($hubStatus, $hubConnection));

  function open() {
    transferSheetFor.set(session.id);
  }
</script>

<span class="host">
  {#if run}
    <button class="chip live" data-state={run.status} onclick={open} data-testid="transfer-live">
      {#if run.status === 'running'}
        ⇄ moving to {run.toHost} · {stepNumber(run)}/{run.steps.length}
      {:else if run.status === 'done'}
        ⇄ moved to {run.toHost}
      {:else}
        ⇄ move failed
      {/if}
    </button>
  {:else if canMoveSession(session)}
    on
    <button
      class="chip"
      onclick={open}
      disabled={blocked !== null}
      title={blocked ?? 'Transfer this session to another host'}
      data-testid="transfer-chip"
    >
      {session.host_alias} ⇄
    </button>
  {:else}
    on {session.host_alias}
  {/if}
</span>

<style>
  .host { color: var(--fg-muted); font-size: 0.75rem; }
  .chip {
    font: inherit;
    color: inherit;
    background: none;
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 0.35rem;
    cursor: pointer;
  }
  .chip:disabled { cursor: default; opacity: 0.6; }
  .chip.live { color: var(--accent, inherit); border-color: currentColor; }
  .chip.live[data-state='failed'], .chip.live[data-state='partial'] { color: var(--danger, #d33); }
</style>
```

`--border`, `--accent` and `--fg-muted` exist in the app; `--danger` does not, hence its literal fallback.

- [ ] **Step 4: Put it in `TerminalView.svelte`**

- `import TransferChip from './TransferChip.svelte';` with the other component imports.
- In the header, replace
  ```svelte
      <span class="host">on {$selectedSession.host_alias}</span>
  ```
  with
  ```svelte
      <TransferChip session={$selectedSession} />
  ```
  and delete the now-unused `.host { … }` rule from this file's `<style>` (svelte-check reports unused selectors).
- In the hub-client branch (`{#if !ownsTheFleet($hubStatus)}` … the panel that renders `terminal-attach-line`), render the chip above the attach line whenever a session is selected:
  ```svelte
      {#if $selectedSession}
        <p class="transfer-row"><TransferChip session={$selectedSession} /></p>
      {/if}
  ```
  Place it directly before the `{#if $selectedSession && selectedSessionHostTransport === 'agent'}` chain, and add `.transfer-row { margin: 0 0 0.5rem; }` to the style block.

- [ ] **Step 5: Run** — `npx vitest run src/lib/TransferChip.test.ts src/lib/TerminalView.test.ts src/lib/TerminalView.hub.test.ts` → all pass (the existing `'on alpha'` / `'on beta'` assertions included); `npx svelte-check` → 0 errors.

- [ ] **Step 6: Commit**

```bash
git -C <worktree> add src/lib/TransferChip.svelte src/lib/TransferChip.test.ts src/lib/TerminalView.svelte src/lib/TerminalView.test.ts src/lib/TerminalView.hub.test.ts
git -C <worktree> commit -m "feat(ui): Transfer chip on the terminal header's host name, live while moving"
```

---

### Task 7: Rewire the details panel, mount the sheet, docs

**Files:**
- Modify: `src/lib/SessionDetails.svelte`, `src/lib/SessionDetails.test.ts`
- Modify: `src/App.svelte`
- Modify: `docs/adr/0002-move-carries-work-as-is.md`, `CLAUDE.md`
- Modify: `docs/superpowers/specs/2026-09-20-transfer-sheet-design.md` (Status line only)

**Interfaces:**
- Consumes: Task 3 (`transferSheetFor`, `applyMoveProgress`), Task 4 (`canMoveSession`, `moveBlockedReason`), Task 5 (`TransferSheet.svelte`), Task 1 (`onMoveProgress`).
- Produces: nothing new.

- [ ] **Step 1: Rewrite the details-panel test first.** In `src/lib/SessionDetails.test.ts`, replace the test `'offers Move to host… for resumable worktree sessions and invokes move_session'` with:

```ts
  it('offers Move to host… for resumable worktree sessions and opens the Transfer sheet', async () => {
    const { transferSheetFor, resetMovesForTest } = await import('./moves');
    const { get } = await import('svelte/store');
    resetMovesForTest();
    // No Claude session id → nothing to resume, no button.
    render(SessionDetails, { props: { session: { ...sampleSession, project_id: 1, worktree_id: 10 } } });
    await tick();
    expect(screen.queryByTestId('move-from-details')).toBeNull();

    const movable = {
      ...sampleSession, id: 5, project_id: 1, worktree_id: 10,
      claude_session_id: '550e8400-e29b-41d4-a716-446655440000',
    };
    render(SessionDetails, { props: { session: movable } });
    await tick();
    (await screen.findByTestId('move-from-details')).click();
    await tick();
    expect(get(transferSheetFor)).toBe(5);
    // The panel no longer owns a dialog: the app's one TransferSheet does.
    expect(screen.queryByTestId('move-dialog')).toBeNull();
    expect(screen.queryByTestId('confirm-move')).toBeNull();
  });
```

Leave `'hides Move to host… for shell sessions'` and the test listing `'move-from-details'` among the action buttons as they are.

- [ ] **Step 2: Run and confirm failure**

Run: `npx vitest run src/lib/SessionDetails.test.ts`
Expected: the rewritten test FAILS (`transferSheetFor` stays `null`, and `move-dialog` is found).

- [ ] **Step 3: Rewire `SessionDetails.svelte`**

- Imports: remove `import { moveSession } from './moveSession';`; add
  ```ts
  import { canMoveSession, moveBlockedReason } from './moveEligibility';
  import { transferSheetFor } from './moves';
  ```
- Replace `const moveBlocked = $derived(hubActionBlocked('move_session', $hubStatus, $hubConnection));` with
  `const moveBlocked = $derived(moveBlockedReason($hubStatus, $hubConnection));`
- Replace the whole block from the comment `// Move to host…: continue this conversation on another host` through the end of `doMove()` (the `canMove` and `moveTargets` deriveds, the four `move*` state variables, `openMove`, `closeMove`, `doMove`) with:
  ```ts
  // Move to host…: the app's one Transfer sheet does the work (TransferSheet
  // + moves.ts); this button and the terminal header's chip both open it.
  const canMove = $derived(canMoveSession(session));

  function openMove() {
    transferSheetFor.set(session.id);
  }
  ```
- Delete the `{#if moveOpen} <Modal … testid="move-dialog"> … </Modal> {/if}` block.
- Delete the `.move-note`, `.move-field` and `.move-buttons` rules from `<style>`.
- If `hosts`, `push`, `Modal` or `hubActionBlocked` are now unused in this file, remove their imports; keep any that another part of the file still uses (check each with a search before deleting).

- [ ] **Step 4: Mount the sheet and feed the store in `src/App.svelte`**

- Imports: `import TransferSheet from './lib/TransferSheet.svelte';` and `import { applyMoveProgress } from './lib/moves';`
- In the `subscribeToRowEvents({...})` call, below `onSyncProgress`: `onMoveProgress: applyMoveProgress,`
- Beside `<Toasts />` near the end of the markup: `<TransferSheet />`

- [ ] **Step 5: Run the frontend suite**

Run: `npx vitest run` — Expected: all pass (no test anywhere still looks for the old dialog inside `SessionDetails`). `npx svelte-check` → 0 errors. `pnpm run build` → succeeds.

- [ ] **Step 6: Docs**

- `docs/adr/0002-move-carries-work-as-is.md`: append a section

  ```markdown
  ## What the user sees (2026-09-20, slice 3a)

  The move is a button on the terminal header's host name (and "Move to
  host…" in the details panel); both open one Transfer sheet. The sheet shows
  the move's nine steps live, from `move:progress` events, and can be closed
  while the move continues — the header then shows `⇄ moving to <host> · n/9`.
  When the move ends the sheet shows what travelled and what did not, each
  warning on its own line, or — on failure — what failed in a sentence and
  whether the source was touched. A hub older than this slice sends no
  progress events: the sheet then shows no live steps and still ends with the
  correct result, which is one more reason to upgrade the hub before the
  desktops.
  ```
- `CLAUDE.md`, "Status & known issues": after the sentence that ends `docs/superpowers/specs/2026-09-19-move-carry-engine-design.md`., add: `The move's UI is the Transfer sheet (terminal-header chip + `moves.ts`, live steps from the `move:progress` event), per `docs/superpowers/specs/2026-09-20-transfer-sheet-design.md`.`
- The spec's header: `**Status:** Draft` → `**Status:** Implemented`.

- [ ] **Step 7: Full local CI**

Run: `scripts/ci-local.sh` (unpiped). Expected: every stage green. If it stops on the generated reference (`reference_is_current`), this plan added no Tauri command and no tool, so that would be a pre-existing drift — report it, do not regenerate blindly.

- [ ] **Step 8: Commit**

```bash
git -C <worktree> add src/lib/SessionDetails.svelte src/lib/SessionDetails.test.ts src/App.svelte docs/adr/0002-move-carries-work-as-is.md CLAUDE.md docs/superpowers/specs/2026-09-20-transfer-sheet-design.md
git -C <worktree> commit -m "feat(ui): the details panel and the app open the one Transfer sheet"
```
