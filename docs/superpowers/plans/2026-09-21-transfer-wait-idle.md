# Transfer slice 3c (wait for idle) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** "Transfer" on a busy session waits for its Claude to finish and then
moves it — on the hub in hub-client mode, so the desktop can close — with a
Cancel, a bounded wait, and a timeline record of how every wait ended.

**Architecture:** A new `move_session/wait.rs` holds an in-memory registry of
pending waits and one plain `async fn run_wait` — the wait loop — that the tests
`await` directly and production spawns. `move_session` gains `when: Now | Idle |
Cancel` and two outcomes (`Waiting`, `WaitCancelled`). The wire contract goes
2 → 3 and 3b's backend guard widens, because an older hub would treat a cancel as
a real move. The frontend always sends `when: "idle"`, shows a waiting run with
Cancel, and rebuilds it from the timeline after a reopen.

**Tech Stack:** Rust (tokio, `tokio_util::sync::CancellationToken`, serde,
rusqlite), Tauri 2, an rmcp tool, Svelte 5 runes + Vitest.

**Spec:** `docs/superpowers/specs/2026-09-21-transfer-wait-idle-design.md` — read
it before Task 1. The plan argues from the spec; where they disagree the spec wins
and the disagreement is a finding.

**Branch base:** `feature/transfer-preflight` (slice 3b). Its `gather()`, `preview()`,
`MoveOutcome`, contract-revision-2 gate and `require_confirmed_contract` are what
this plan builds on.

## Global Constraints

- **Never run a dev build of the desktop app** (`cargo tauri dev`, `pnpm tauri dev`,
  `cargo run -p claude-fleet`): it migrates the installed app's `state.db`
  irreversibly and its singleton guard kills the running app. UI is verified by
  Vitest only.
- **Never ssh anywhere.** Engine tests run over `FakeSsh`.
- **No new `E_*` codes**, in Rust or TypeScript.
- **Wire rule:** report/outcome types (`MoveWaiting`, `MoveWaitCancelled`) derive
  `Serialize + Deserialize` with **no** `#[serde(default)]`. An *argument* (`when`)
  may default. A hub-routed argument must be on `MoveSessionParams`, mapped through
  its single `into_args`, and have a non-default row in
  `src-tauri/src/backend/tests_routing.rs` — 3d shipped one that never reached the
  hub.
- **Never hold the `Store` mutex across an `.await`** — the wait sleeps for up to
  hours; this is the one place in the slice that rule is load-bearing.
- **The served MCP surface** is 57,673 of `BUDGET_BYTES = 57_700` (27 bytes of
  slack). Task 4 adds one enum parameter and makes **one** deliberate, measured
  raise of the constant, recorded in its doc comment with the numbers — nobody else
  touches it.
- **Generated files are regenerated, never hand-edited**: `docs/control-api-reference.md`
  (`REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`, then the same
  command without the variable), `src-tauri/src/backend/hub_contract.golden.json`
  (`REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract`, then without).
- **No raw control bytes in source files.** Twice in 3b an implementer wrote a
  literal NUL into a `.ts` file and git classified it as binary. Use escape
  sequences; byte-scan every file you touch before committing.
- **The test code in this plan was written without being run.** In 3b, four
  plan-written assertions could not fail. Treat the code below as the specification
  of *what* to assert; prove each test goes red when the behaviour it pins is
  broken, and report each proof.
- **RED is chronological**: tests written and run against the code as it stands,
  that output pasted, *then* the implementation. Never implement and revert.
- **Run every suite in the foreground, unpiped, no test-name filter on the final
  run**: `cargo test -p fleet-core`, `cargo test -p claude-fleet`, `cargo test -p
  fleet-hub`, `npx vitest run`, `npx svelte-check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo fmt --all`. Never background a test run and
  wait on it. `pnpm test`/`pnpm check` cannot find their binaries here; use `npx`.
- **Git:** every command is `git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 …`.
  Never `pull`, `push`, `rebase`, `checkout`, `switch`, **`stash`**, `merge` or
  `reset` — the stash is shared with every session on this machine. `.mcp.json` is
  modified by tooling and is not yours: stage files by name, never `-A`. No
  attribution lines. Never print full process command lines (`pgrep -fl`, `ps aux`).

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `crates/fleet-core/src/store/timeline.rs` | a cross-session query for unresolved waits | 1 |
| `crates/fleet-core/src/service/settings.rs` | the `move.wait_max_mins` key and its upper bound | 1 |
| `crates/fleet-core/src/service/move_session/wait.rs` | **new** — the registry, `run_wait`, the ended-event reasons, the startup sweep | 2 |
| `crates/fleet-core/src/service/move_session/mod.rs` | `When`, the two outcomes, the public entry's dispatch and spawn, the dry-run/idle interplay, the tests | 2 (tests + `pub mod wait;`), 3 |
| `crates/fleet-core/src/mcp/tools/{params,lifecycle,tests}.rs` | `when` on the params and `into_args`; the call site; the budget raise | 4 |
| `crates/fleet-core/src/wire_contract.rs`, `src-tauri/src/backend/{contract,tests_contract,tests_events,tests_routing}.rs`, `hub_contract.golden.json`, `src-tauri/src/commands/move_session.rs`, `docs/hub.md` | contract 2 → 3; the widened guard; the routing row | 4 |
| `src-tauri/src/bootstrap/tasks.rs`, `crates/fleet-hub/src/serve.rs` | calling the startup sweep | 5 |
| `src/lib/moveSession.ts`, `src/lib/moveErrors.ts` | the wire types, narrowing, `cancelMoveWait`, the "not idle" text | 6 |
| `src/lib/timeline.ts`, `src/lib/moves.ts` | `unresolvedWait`, the `waiting` run, `cancelWait`, `adoptWait`, Transfer sends `idle` | 7 |
| `src/lib/TransferSheet.svelte`, `src/lib/TransferChip.svelte` | the waiting view, Cancel, how a wait ended | 8 |

## Execution order

```
Rust:     1 → 2 → 3 → 4 → 5          (2 and 3 both write mod.rs; 4 needs 3's wire)
Frontend: 6 → 7 → 8                  (6 needs 4's wire shape)
```

---
### Task 1: A cross-session query for unresolved waits, and the setting

**Files:**
- Modify: `crates/fleet-core/src/store/timeline.rs` — one query + its test.
- Modify: `crates/fleet-core/src/service/settings.rs` — the key and its bound.

**Why the query is generic:** the store knows nothing about moves. It answers
"which *opened* events has no later *closed* event resolved, per session" for
kinds the caller names; Task 2 passes the two wait kinds.

**Interfaces — produces, for Task 2:**

```rust
/// Every event of kind `opened` that no LATER event of kind `closed` on the
/// same session has resolved, oldest first: `(session_id, event_id, detail)`.
pub fn unresolved_events(
    &self,
    opened: &str,
    closed: &str,
) -> Result<Vec<(i64, i64, Option<String>)>, IpcError>;
```

and in `service/settings.rs`: `pub const MOVE_WAIT_MAX_MINS: &str = "move.wait_max_mins";`
plus `pub const MOVE_WAIT_MAX_MINS_MAX: u64 = 10_080;` (a week), registered
wherever `MOVE_MAX_TRANSCRIPT_MB` is registered — read every reference to that
constant in `settings.rs` and mirror each one, so the new key is validated and
listed exactly like its siblings.

- [ ] **Step 1: Write the failing test** in `timeline.rs`'s test module, using
the same store setup its existing tests use (`session_events_insert_then_list_newest_first_with_limit`
shows it):

```rust
    #[test]
    fn unresolved_events_are_the_opened_ones_no_later_close_resolved() {
        let s = Store::open_in_memory().unwrap();
        // Events take bare session ids, as the file's other tests use them.
        let (a, b) = (7_i64, 99_i64);
        s.insert_session_event(a, "open", Some("a1")).unwrap(); // resolved below
        s.insert_session_event(a, "close", None).unwrap();
        s.insert_session_event(a, "open", Some("a2")).unwrap(); // NOT resolved
        s.insert_session_event(b, "open", Some("b1")).unwrap(); // NOT resolved
        s.insert_session_event(b, "other", None).unwrap(); // not a close
        let got: Vec<String> = s
            .unresolved_events("open", "close")
            .unwrap()
            .into_iter()
            .map(|(_, _, d)| d.unwrap())
            .collect();
        assert_eq!(got, vec!["a2".to_string(), "b1".to_string()]);
    }

    #[test]
    fn a_close_on_another_session_resolves_nothing() {
        let s = Store::open_in_memory().unwrap();
        let (a, b) = (7_i64, 99_i64);
        s.insert_session_event(a, "open", Some("a1")).unwrap();
        s.insert_session_event(b, "close", None).unwrap();
        assert_eq!(s.unresolved_events("open", "close").unwrap().len(), 1);
    }
```


- [ ] **Step 2: Watch them fail** (a compile error — the method does not exist).
Paste it.

- [ ] **Step 3: Implement** with one query over the events table (read
`list_session_events` for its name and columns):

```sql
SELECT w.session_id, w.id, w.detail
  FROM session_events w
 WHERE w.kind = ?1
   AND NOT EXISTS (SELECT 1 FROM session_events c
                    WHERE c.session_id = w.session_id
                      AND c.kind = ?2
                      AND c.id > w.id)
 ORDER BY w.id
```

Order by `id`, not `at`: two events in the same second must still be ordered.

- [ ] **Step 4: Full suite, clippy, fmt, commit**

```bash
cargo test -p fleet-core
```

```bash
cargo clippy -p fleet-core --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add crates/fleet-core/src/store/timeline.rs crates/fleet-core/src/service/settings.rs
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(store): find the waits no later event has closed, and a setting to bound them"
```

---

### Task 2: The waiter — registry, `begin_wait`, `run_wait`, the startup sweep

**Files:**
- Create: `crates/fleet-core/src/service/move_session/wait.rs`
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — `pub mod wait;`, a
  `SOURCE_NOT_IDLE` sentinel used by `require_source_idle`'s message, and the tests
  (they need `mod.rs`'s private fixtures).
- Test: `mod.rs`'s test module.

**The shape that makes this testable.** The fixture holds the store as a plain
`Mutex<Store>` and `FakeHooks` borrows the fake, so a spawned `'static` task cannot
be tested. Everything here takes borrowed handles and is `await`ed directly by the
tests; only Task 3's public entry point spawns.

**Interfaces — produces, for Task 3:**

```rust
pub const EVENT_MOVE_WAITING: &str = "session_move_waiting";
pub const EVENT_MOVE_WAIT_ENDED: &str = "session_move_wait_ended";
pub const DEFAULT_WAIT_MAX_MINS: u64 = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitEnd { Moved, Partial, Refused, Cancelled, TimedOut, SessionGone, HubRestarted }

/// Held for as long as a wait is pending; dropping it deregisters.
pub(super) struct WaitGuard { /* key + token */ }
impl WaitGuard { pub(super) fn token(&self) -> &CancellationToken; }

/// Register a wait for `args.session_id`, record `session_move_waiting`, and
/// return the guard, the deadline, and the `MoveWaiting` to answer with.
/// Refuses (E_INVALID_STATE) a session already waiting.
pub(super) fn begin_wait(
    args: &MoveSessionArgs,
    store: &Mutex<Store>,
) -> Result<(WaitGuard, tokio::time::Instant, MoveWaiting), IpcError>;

/// Cancel a pending wait. `true` if there was one.
pub(super) fn cancel_wait(store: &Mutex<Store>, session_id: i64) -> bool;

/// The wait loop. `args.when` must be `Now` — this is the move it will run.
/// Records `session_move_wait_ended` with the reason before returning it.
pub(super) async fn run_wait(
    args: MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    opts: MoveOptions,
    token: &CancellationToken,
    deadline: tokio::time::Instant,
    poll: Duration,
) -> WaitEnd;

/// Close every unresolved `session_move_waiting` as `hub_restarted`.
/// Returns how many it closed. Called once at startup (Task 5).
pub fn sweep_unresolved_waits(store: &Mutex<Store>) -> Result<usize, IpcError>;
```

`MoveWaiting` (`session_id`, `to_host`, `deadline_unix`) is defined here too, so
this task compiles on its own; Task 3 puts it into `MoveOutcome`.

- [ ] **Step 1: The sentinel.** In `mod.rs`, add
`pub const SOURCE_NOT_IDLE: &str = "the source Claude is not idle";` and make
`require_source_idle`'s message start with it (via `format!`, not a second copy of
the words). This is how `run_wait` tells "the source went busy again" from every
other refusal — matched with `contains`, per the sentinel rule. The existing
`require_source_idle` tests must pass unchanged.

- [ ] **Step 2: Write the failing tests** in `mod.rs`'s test module. The fixture's
source starts idle; `s.set_claude_status_by_session_id(SID, "working")` makes it
busy. A status flip runs *concurrently* with the wait through `tokio::join!` — which
works with borrowed values, unlike `tokio::spawn` — and it is also the proof that
`run_wait` never holds the store lock across its sleep: if it did, the flip could
not take the lock and the test would time out.

```rust
    /// Set the fixture's source Claude status.
    fn set_status(f: &Fixture, status: &str) {
        f.store.lock().unwrap().set_claude_status_by_session_id(SID, status).unwrap();
    }

    fn wait_events(f: &Fixture) -> Vec<(String, Option<String>)> {
        events(f, f.source_id)
            .into_iter()
            .filter(|(k, _)| k == wait::EVENT_MOVE_WAITING || k == wait::EVENT_MOVE_WAIT_ENDED)
            .collect()
    }

    const POLL: Duration = Duration::from_millis(10);

    #[tokio::test]
    async fn a_wait_moves_the_session_once_it_goes_idle() {
        let (f, _bus) = recorded_fixture();
        set_status(&f, "working");
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let a = args(&f, false);
        let (guard, deadline, _) = wait::begin_wait(&a, &f.store).expect("registered");
        let (end, ()) = tokio::join!(
            wait::run_wait(a, &f.store, &f.fake, &hooks, fast(), guard.token(), deadline, POLL),
            async {
                tokio::time::sleep(POLL * 5).await;
                set_status(&f, "idle");
            },
        );
        assert_eq!(end, wait::WaitEnd::Moved);
        assert!(events(&f, f.source_id).iter().any(|(k, _)| k == EVENT_MOVED), "the move happened");
        let w = wait_events(&f);
        assert_eq!(w.first().map(|(k, _)| k.as_str()), Some(wait::EVENT_MOVE_WAITING));
        let (_, ended) = w.last().unwrap();
        assert!(ended.as_deref().unwrap().contains("moved"), "{ended:?}");
    }

    #[tokio::test]
    async fn blocked_is_not_idle_and_does_not_end_the_wait() {
        let (f, _bus) = recorded_fixture();
        set_status(&f, "blocked");
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let a = args(&f, false);
        let (guard, deadline, _) = wait::begin_wait(&a, &f.store).unwrap();
        let (end, ()) = tokio::join!(
            wait::run_wait(a, &f.store, &f.fake, &hooks, fast(), guard.token(), deadline, POLL),
            async {
                tokio::time::sleep(POLL * 5).await;
                // Still waiting after a while spent blocked: no move yet.
                assert!(!events(&f, f.source_id).iter().any(|(k, _)| k == EVENT_MOVED));
                set_status(&f, "idle");
            },
        );
        assert_eq!(end, wait::WaitEnd::Moved);
    }

    #[tokio::test]
    async fn the_deadline_ends_the_wait_without_a_move() {
        let (f, _bus) = recorded_fixture();
        set_status(&f, "working");
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let a = args(&f, false);
        let (guard, _, _) = wait::begin_wait(&a, &f.store).unwrap();
        let deadline = tokio::time::Instant::now() + POLL * 3;
        let end =
            wait::run_wait(a, &f.store, &f.fake, &hooks, fast(), guard.token(), deadline, POLL).await;
        assert_eq!(end, wait::WaitEnd::TimedOut);
        assert!(!events(&f, f.source_id).iter().any(|(k, _)| k == EVENT_MOVED));
    }

    #[tokio::test]
    async fn cancel_ends_the_wait_without_a_move() {
        let (f, _bus) = recorded_fixture();
        set_status(&f, "working");
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let a = args(&f, false);
        let (guard, deadline, _) = wait::begin_wait(&a, &f.store).unwrap();
        let (end, cancelled) = tokio::join!(
            wait::run_wait(a, &f.store, &f.fake, &hooks, fast(), guard.token(), deadline, POLL),
            async {
                tokio::time::sleep(POLL * 3).await;
                wait::cancel_wait(&f.store, f.source_id)
            },
        );
        assert!(cancelled, "a pending wait was found and cancelled");
        assert_eq!(end, wait::WaitEnd::Cancelled);
        assert!(!events(&f, f.source_id).iter().any(|(k, _)| k == EVENT_MOVED));
        assert!(!wait::cancel_wait(&f.store, f.source_id) || true); // see note below
    }

    #[tokio::test]
    async fn a_second_wait_for_the_same_session_is_refused() {
        let (f, _bus) = recorded_fixture();
        let a = args(&f, false);
        let _first = wait::begin_wait(&a, &f.store).unwrap();
        let err = wait::begin_wait(&a, &f.store).err().expect("refused");
        assert_eq!(err.code, codes::E_INVALID_STATE);
    }

    #[tokio::test]
    async fn a_wait_is_refused_while_a_move_of_the_session_is_in_flight() {
        let (f, _bus) = recorded_fixture();
        let a = args(&f, false);
        let _claim = MoveClaim::acquire(&f.store, a.session_id).unwrap();
        let err = wait::begin_wait(&a, &f.store).err().expect("refused");
        assert_eq!(err.code, codes::E_INVALID_STATE);
    }

    #[tokio::test]
    async fn the_startup_sweep_closes_waits_nothing_is_honouring() {
        let (f, _bus) = recorded_fixture();
        let a = args(&f, false);
        let (guard, _, _) = wait::begin_wait(&a, &f.store).unwrap();
        drop(guard); // the process "restarted": the waiter is gone, the event is not
        assert_eq!(wait::sweep_unresolved_waits(&f.store).unwrap(), 1);
        let (_, ended) = wait_events(&f).last().cloned().unwrap();
        assert!(ended.unwrap().contains("hub_restarted"));
        assert_eq!(wait::sweep_unresolved_waits(&f.store).unwrap(), 0, "idempotent");
    }
```

Two things in that code to fix, not copy: the last line of
`cancel_ends_the_wait_without_a_move` is a deliberate tautology (`|| true`) — replace
it with the real assertion, that a second `cancel_wait` for the same session returns
`false` once the guard is dropped. And `the_startup_sweep_…` drops the guard before
sweeping; check what the sweep does to a wait that is **still registered** — it must
not close a live one — and add a test for that too (the sweep runs at startup, when
nothing is registered, but a live wait must never be marked `hub_restarted`).

Add, the same way: **the session disappearing** ends the wait `SessionGone` (delete
the source row mid-wait — find the store's real deletion call); and **busy again**:
the source reads idle, the move re-checks and is refused with `SOURCE_NOT_IDLE`,
the waiter goes back to waiting, and a later idle moves it. For that, add the
smallest `FakeHooks` switch that makes the source report busy **once** when the
move's `refresh_host` runs (e.g. an `AtomicBool` it clears after firing) — and
confirm it can fail by making `run_wait` treat that refusal as final.

- [ ] **Step 3: Watch them fail** — a compile error (`wait` does not exist). Paste it.

- [ ] **Step 4: Implement `wait.rs`.**

Registry: a process-wide `Mutex<HashMap<(usize, i64), CancellationToken>>` keyed
by `(store address, session id)` — the same keying as the move's in-flight guard
(`moves_in_flight`, read it) so parallel tests on separate in-memory stores cannot
collide. `WaitGuard::drop` removes its key.

`begin_wait`: refuse a key already present, and refuse a session with a move
**in flight** (`E_INVALID_STATE` — read how `MoveClaim` / `moves_in_flight` records
one and query it; do not take the claim); read `move.wait_max_mins` the way the
move reads its other numeric settings (`settings::get_string` parsed to a positive
`u64`, falling back to `DEFAULT_WAIT_MAX_MINS`, capped at `MOVE_WAIT_MAX_MINS_MAX`);
record `EVENT_MOVE_WAITING` with a JSON detail naming `to_host`, `keep_source`,
`strict`, `clean_target` and `deadline_unix`.

`run_wait`, in a loop:

```rust
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let idle = tokio::select! {
            _ = token.cancelled() => break WaitEnd::Cancelled,
            r = crate::service::tasks::wait_for_session_with(
                store, args.session_id, crate::service::tasks::WaitCond::Idle, left, poll,
            ) => r,
        };
        match idle {
            Err(e) if e.code == codes::E_NOTFOUND => break WaitEnd::SessionGone,
            Err(_) => break WaitEnd::Refused,
            Ok(o) if !o.satisfied => break WaitEnd::TimedOut,
            Ok(_) => {}
        }
        // Idle: run the real move. A cancel from here on lets it finish —
        // cancelling a running move is out of scope.
        match super::move_session_with(args.clone(), store, ssh, hooks, opts).await {
            Ok(MoveOutcome::Moved(_)) => break WaitEnd::Moved,
            Err(e) if e.code == codes::E_MOVE_PARTIAL => break WaitEnd::Partial,
            Err(e) if e.message.contains(super::SOURCE_NOT_IDLE) => continue,
            _ => break WaitEnd::Refused,
        }
    }
```

— then record `EVENT_MOVE_WAIT_ENDED` with a detail naming the reason (serialised
`WaitEnd`, snake_case) and, for `Refused`, the error's code and message. Record it
through a best-effort `if let Ok(s) = store.lock()` — never `?` after work is done,
the lesson 3d paid for. `move_session_with` must receive `args` with `when: Now`
(Task 3 guarantees that at its call site; `run_wait` does not re-dispatch).

`sweep_unresolved_waits`: `store.unresolved_events(EVENT_MOVE_WAITING,
EVENT_MOVE_WAIT_ENDED)`, skip any session currently registered, and insert an
ended event with reason `hub_restarted` for each of the rest.

Until Task 3 adds `When`, `args.when` does not exist — `run_wait` simply calls
`move_session_with` with `args` as given; Task 3 is where `when` appears.

- [ ] **Step 5: Full suite, clippy, fmt, commit**

```bash
cargo test -p fleet-core
```

```bash
cargo clippy -p fleet-core --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add crates/fleet-core/src/service/move_session/wait.rs crates/fleet-core/src/service/move_session/mod.rs
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(move): a bounded, cancellable wait for the source to go idle, and a sweep for waits a restart lost"
```

---

### Task 3: `when`, the two new outcomes, and the spawning entry point

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — `When`, `MoveSessionArgs.when`,
  `MoveOutcome::{Waiting, WaitCancelled}`, `MoveWaitCancelled`, the dispatch in
  `move_session_with` and in the public `move_session`, the dry-run/idle interplay.
- Modify: `crates/fleet-core/src/service/move_session/preview.rs` — the "busy now,
  the move will wait" `unknowns` line.
- Modify: `src-tauri/src/commands/move_session.rs` — only the routed helper's
  `store` parameter type, so it compiles against the new signature (Task 4 widens
  its guard).
- Test: `mod.rs`'s test module.

**Interfaces — produces, for Tasks 4–8:**

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum When { #[default] Now, Idle, Cancel }

// on MoveSessionArgs:
/// When to move: `now` (default — a busy source is refused), `idle` (wait for
/// the source and return `Waiting`), `cancel` (end a pending wait).
#[serde(default)]
pub when: When,

pub enum MoveOutcome {
    Moved(Box<MoveReport>),
    Preview(Box<preview::MovePreview>),
    Waiting(Box<wait::MoveWaiting>),
    WaitCancelled(Box<MoveWaitCancelled>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveWaitCancelled { pub session_id: i64, pub was_waiting: bool }

/// Public entry. Takes the store as an `Arc` because `when: idle` on a busy
/// source spawns a waiter that must outlive the call.
pub async fn move_session(
    args: MoveSessionArgs,
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<MoveOutcome, IpcError>;
```

- [ ] **Step 1: Write the failing tests**

```rust
    #[tokio::test]
    async fn when_idle_on_an_idle_source_moves_at_once() {
        let (f, _bus) = recorded_fixture(); // the fixture's source is idle
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let mut a = args(&f, false);
        a.when = When::Idle;
        let out = move_session_with(a, &f.store, &f.fake, &hooks, fast()).await.unwrap();
        assert!(matches!(out, MoveOutcome::Moved(_)), "{out:?}");
        assert!(wait_events(&f).is_empty(), "no wait was registered");
    }

    #[tokio::test]
    async fn cancel_with_nothing_pending_says_so_and_moves_nothing() {
        let (f, _bus) = recorded_fixture();
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let mut a = args(&f, false);
        a.when = When::Cancel;
        let out = move_session_with(a, &f.store, &f.fake, &hooks, fast()).await.unwrap();
        let MoveOutcome::WaitCancelled(c) = out else { panic!("{out:?}") };
        assert!(!c.was_waiting);
        assert!(!events(&f, f.source_id).iter().any(|(k, _)| k == EVENT_MOVED));
        assert!(f.fake.calls().is_empty(), "a cancel touches no host");
    }

    #[tokio::test]
    async fn a_dry_run_cancel_is_refused() {
        let (f, _bus) = recorded_fixture();
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let mut a = args(&f, false);
        a.when = When::Cancel;
        a.dry_run = true;
        let err = move_session_with(a, &f.store, &f.fake, &hooks, fast()).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
    }

    #[tokio::test]
    async fn a_dry_run_for_idle_on_a_busy_source_previews_and_says_it_will_wait() {
        let (f, _bus) = recorded_fixture();
        set_status(&f, "working");
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let mut a = args(&f, false);
        a.dry_run = true;
        a.when = When::Idle;
        let out = move_session_with(a, &f.store, &f.fake, &hooks, fast()).await.unwrap();
        let MoveOutcome::Preview(p) = out else { panic!("{out:?}") };
        assert!(p.unknowns.iter().any(|u| u.contains("busy")), "{:?}", p.unknowns);
    }

    #[tokio::test]
    async fn a_dry_run_for_now_on_a_busy_source_is_still_the_moves_refusal() {
        let (f, _bus) = recorded_fixture();
        set_status(&f, "working");
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let mut a = args(&f, false);
        a.dry_run = true; // when defaults to Now
        let err = move_session_with(a, &f.store, &f.fake, &hooks, fast()).await.unwrap_err();
        assert!(err.message.contains(SOURCE_NOT_IDLE), "{}", err.message);
    }

    #[test]
    fn every_outcome_round_trips_on_the_wire() {
        for o in [
            MoveOutcome::Waiting(Box::new(wait::MoveWaiting {
                session_id: 7, to_host: "beta".into(), deadline_unix: 1_700_000_000,
            })),
            MoveOutcome::WaitCancelled(Box::new(MoveWaitCancelled { session_id: 7, was_waiting: true })),
        ] {
            let v = serde_json::to_value(&o).unwrap();
            assert!(v["kind"].is_string(), "{v}");
            let back: MoveOutcome = serde_json::from_value(v.clone()).unwrap();
            assert_eq!(serde_json::to_value(&back).unwrap(), v);
        }
    }
```

- [ ] **Step 2: Watch them fail** — compile errors. Paste them.

- [ ] **Step 3: Implement.**

`move_session_with` — the test-facing entry, no spawning:

```rust
    if args.when == When::Cancel {
        if args.dry_run {
            return Err(IpcError::new(codes::E_INVALID, "there is nothing to preview about cancelling a wait"));
        }
        let was_waiting = wait::cancel_wait(store, args.session_id);
        return Ok(MoveOutcome::WaitCancelled(Box::new(MoveWaitCancelled {
            session_id: args.session_id, was_waiting,
        })));
    }
    if args.dry_run { /* 3b's preview path, unchanged */ }
    // `now`, and `idle` reaching this far, are today's move. The waiting half
    // of `idle` lives in the public `move_session`, the only entry point that
    // owns handles a spawned waiter can keep.
```

In `gather()` (read it — it is shared by the move and the preview), skip
`require_source_idle` **only** when `args.dry_run && args.when == When::Idle` — that
check is exactly what `idle` defers, and it is the preview's second sanctioned
divergence from the move (spec §4). In `preview()`, when that skip applied and the
source is not idle, add to `unknowns`: "the source Claude is busy now; the move will
wait for it to finish". A real move never skips it.

The public `move_session`:

```rust
pub async fn move_session(
    args: MoveSessionArgs,
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<MoveOutcome, IpcError> {
    if args.when == When::Idle && !args.dry_run && !source_is_idle(store, args.session_id)? {
        let (guard, deadline, waiting) = wait::begin_wait(&args, store)?;
        let (store, ssh) = (Arc::clone(store), Arc::clone(ssh));
        let mut now = args;
        now.when = When::Now;
        tokio::spawn(async move {
            let hooks = RealHooks { ssh: &ssh };
            // The guard lives as long as the task: dropping it deregisters.
            let token = guard.token().clone();
            wait::run_wait(now, &store, &*ssh, &hooks, MoveOptions::default(), &token, deadline, WAIT_POLL).await;
            drop(guard);
        });
        return Ok(MoveOutcome::Waiting(Box::new(waiting)));
    }
    let hooks = RealHooks { ssh };
    move_session_with(args, store, &**ssh, &hooks, MoveOptions::default()).await
}
```

`source_is_idle` reads the row and uses `tasks::session_satisfies(&row, WaitCond::Idle)` —
the same definition of idle as the waiter and the move. `WAIT_POLL` is a named
constant (use the poll interval `tasks` uses for `wait_for_session`, read it). Put a
comment on the spawn saying why it is here and not in `move_session_with`.

Fix the routed helper in `src-tauri/src/commands/move_session.rs` to take
`store: &Arc<Mutex<Store>>` and pass it through; the Tauri command already holds a
`State<Arc<Mutex<Store>>>`. The MCP tool passes `&self.store`, already an `Arc`.

- [ ] **Step 4: Full suites, clippy, fmt, commit**

```bash
cargo test -p fleet-core
```

```bash
cargo test -p claude-fleet
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add crates/fleet-core/src/service/move_session/mod.rs crates/fleet-core/src/service/move_session/preview.rs src-tauri/src/commands/move_session.rs
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(move): when=idle waits for the source, when=cancel ends the wait"
```

---
### Task 4: The wire — the MCP parameter, the contract bump, the widened guard

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/params.rs` — `when` on `MoveSessionParams`, mapped in `into_args`.
- Modify: `crates/fleet-core/src/mcp/tools/lifecycle.rs` — the handler: audit line, which requests skip the confirm gate.
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs` — mapping test, source pin, confirm-gate test, the **one** budget raise.
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — only `#[derive(schemars::JsonSchema)]` on `When`, if the params struct uses `When` directly.
- Modify: `crates/fleet-core/src/wire_contract.rs`, `src-tauri/src/backend/contract.rs`, and the tests pinning the revision (`tests_contract.rs`, `tests_events.rs`); regenerate `hub_contract.golden.json`.
- Modify: `src-tauri/src/commands/move_session.rs` — the widened guard.
- Modify: `src-tauri/src/backend/tests_routing.rs` — non-default `when`, and the `Waiting` answer.
- Modify: `docs/hub.md` — the version-skew paragraph names revision 3.
- Regenerate: `docs/control-api-reference.md`.

**Why one task across many files:** it is one change — the routed shape of
`move_session` and the contract that says which hubs may receive it. Split, the
build or the safety argument would be broken between pieces.

**The hazard this task exists to close (spec §3):** an older hub ignores `when`.
For `idle` that is harmless (a busy source is refused as today). For `cancel` it is
**not**: the old hub sees an ordinary move request and **moves the session** —
cancelling a wait would perform the move. Two defences, both required:

1. **Contract 2 → 3.** `fleet_core::wire_contract::CONTRACT_REVISION = 3`, with a
   history entry saying why (an older hub would run a real move for `when: cancel`).
   `MIN_HUB_CONTRACT = MAX_HUB_CONTRACT = 3` in `src-tauri/src/backend/contract.rs`;
   fix their doc comments. Update — never delete — every test pinning `2`, and keep a
   live "too old" test: a hub reporting `2` is `TooOld`. Regenerate the golden:
   `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract`, then without the
   variable. Only its `revision` line should change; if any type key changes, stop and
   say so.
2. **The guard widens.** In `routed::move_session`, 3b wrote
   `if args.dry_run { hub.require_confirmed_contract(..)?; }`. Make it
   `if args.dry_run || args.when != When::Now { … }` and update its comment: a
   request that an older hub would misread must wait for a positive in-range
   judgement on this launch. Plain moves stay ungated.

**The MCP parameter.** `when` on `MoveSessionParams`, `#[serde(default)]`, mapped in
`into_args` — 3d's lesson: a routed argument that is on the service and the command
but not on the params struct silently never reaches the hub. If `MoveSessionParams`
uses `When` directly it needs `JsonSchema`; derive it on `When` in `mod.rs` (a
one-line change to a finished task's file). Keep the doc comment short — it is
served to every MCP client.

**The handler.** Read `p.when` (and `p.dry_run`) **before** `p.into_args(..)`
consumes `p`, put `when` in the audit line, and skip `confirm_gate` for a dry run
**and for a cancel** — a cancel prevents a move; asking the user to confirm it would
put a dialog between them and the safe action. `when: idle` is a (deferred) move and
keeps the gate. The access checks (`resolve_target_row`, `require_move_hosts`) stay
unconditional, which is why a cancel still names the pending wait's target host.

**The budget.** The served surface is 57,673 of `BUDGET_BYTES = 57_700`. An enum
parameter will not fit. Measure the surface with `when` added, then raise the
constant **once**, deliberately, to the measured size plus a small headroom (≈100
bytes), and extend the constant's doc comment with the numbers: what 3c added, the
measurement, the headroom, and that trimming could not pay for a new parameter. This
is the use the test's own documentation describes. Report the numbers.

- [ ] **Step 1: Write the failing tests**

In `mcp/tools/tests.rs`: extend `move_session_params_carry_clean_target_into_the_service_args`
(or add a sibling) so `{"when": "cancel"}` and `{"when": "idle"}` arriving at the
tool reach `MoveSessionArgs.when`; extend its source pin so no args literal in
`lifecycle.rs` may set `when:`; and a confirm-gate test — with confirmation on,
`when: "cancel"` does **not** answer `E_CONFIRM_REQUIRED`, `when: "idle"` does. Prove
each can fail.

In `src-tauri`: a guard test — before any `ready` frame, `when: "cancel"` and
`when: "idle"` are refused with `E_HUB_CONTRACT`, `when: "now"` is not; after an
in-range `ready`, all three pass. A contract test: a hub reporting revision `2` is
`TooOld`. A routing row with a non-default `when`, and a `Waiting` answer from the
hub deserialising into `MoveOutcome::Waiting`.

- [ ] **Step 2: Watch them fail** — paste it.

- [ ] **Step 3: Implement** the parameter, the handler change, the contract bump,
the guard, the routing rows, the `docs/hub.md` note (desktop and hub must be upgraded
together; revision 3 from this change), then regenerate:

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
```

```bash
cargo test -p fleet-core reference_is_current
```

```bash
REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract
```

```bash
cargo test -p claude-fleet --lib contract
```

The second of each pair must pass; the regen run itself may report FAILED.

- [ ] **Step 4: Full suites, clippy, fmt, commit**

```bash
cargo test -p fleet-core
```

```bash
cargo test -p claude-fleet
```

```bash
cargo test -p fleet-hub
```

```bash
cargo test -p fleet-core the_served_definition_budget_stays_bounded -- --nocapture
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

Stage by name, never `-A`; commit:
`feat(move): when reaches the hub only once the hub is known to understand it`.

---

### Task 5: Run the startup sweep on the desktop and the hub

**Files:**
- Modify: `src-tauri/src/bootstrap/tasks.rs` — beside `start_reconcile_tick`.
- Modify: `crates/fleet-hub/src/serve.rs` — beside its `spawn_reconcile_tick` call.

**Why:** a waiter lives in memory. After a restart its `session_move_waiting` event
has no waiter behind it, and a reopened desktop would show a wait that no longer
exists. `wait::sweep_unresolved_waits` (Task 2) closes each as `hub_restarted`. It
must run **once, at startup, before** anything can register a new wait — otherwise
it could race a real one (Task 2's sweep also skips registered waits, as a second
guard).

**Interfaces — consumes:** `fleet_core::service::move_session::wait::sweep_unresolved_waits(&Mutex<Store>) -> Result<usize, IpcError>`.

- [ ] **Step 1:** Read both startup paths and find the point where the store is
open and no command or MCP tool can yet be served. Call the sweep there, log the
count at `info` when non-zero, and log (never panic, never abort startup) on error.
- [ ] **Step 2: Test.** If either startup path has a seam a test can drive (read the
existing tests beside it), add one: an unresolved `session_move_waiting` in the store
before startup is closed as `hub_restarted` after it. If neither has a seam, say so
in the report rather than inventing one — Task 2's sweep tests cover the logic, and
this task is two call sites.
- [ ] **Step 3: Full suites, clippy, fmt, commit**

```bash
cargo test -p claude-fleet
```

```bash
cargo test -p fleet-hub
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

Commit: `feat(move): a restart closes the waits it can no longer honour`.

---

### Task 6: `moveSession.ts` and `moveErrors.ts` — the new outcomes

**Files:**
- Modify: `src/lib/moveSession.ts`, `src/lib/moveSession.test.ts`
- Modify: `src/lib/moveErrors.ts`, `src/lib/moveErrors.test.ts`

**Interfaces — produces, for Tasks 7 and 8:**

```ts
export type When = 'now' | 'idle' | 'cancel';

export interface MoveWaiting { session_id: number; to_host: string; deadline_unix: number }
export interface MoveWaitCancelled { session_id: number; was_waiting: boolean }

export type MoveOutcome =
  | ({ kind: 'moved' } & MoveReport)
  | ({ kind: 'preview' } & MovePreview)
  | ({ kind: 'waiting' } & MoveWaiting)
  | ({ kind: 'wait_cancelled' } & MoveWaitCancelled);

/** A real transfer. `when` defaults to 'idle' — an idle source moves at once,
 *  a busy one yields a pending wait. Narrows to moved | waiting. */
export function moveSession(
  sessionId: number,
  targetHostAlias: string,
  opts?: { keepSource?: boolean; strict?: boolean; cleanTarget?: boolean; when?: 'now' | 'idle' },
): Promise<Result<({ kind: 'moved' } & MoveReport) | ({ kind: 'waiting' } & MoveWaiting)>>;

/** End a pending wait. Carries the wait's target host: the backend's access
 *  checks name both hosts even for a cancel. */
export function cancelMoveWait(sessionId: number, targetHostAlias: string): Promise<Result<MoveWaitCancelled>>;
```

`moveSession`'s return type changes from a bare report to the narrowed union — its
two callers are in `moves.ts` (Task 7). Keep `mergeSession(r.value.target)` for a
`moved` outcome only; a `waiting` outcome has no row to merge.

`previewMove` (3b) is unchanged in what it returns; pass `when: 'idle'` in it too,
so the setup view previews what the Transfer button will actually do (spec §4: a
busy source previews with a "will wait" line instead of stopping at the refusal).

`moveErrors.ts`: the `is not idle` arm (`E_INVALID_STATE`) currently says "Wait for
it to finish, then transfer." Transfer now waits by itself, so that text now only
appears for a caller that asked for `now`. Reword it to state the fact without
telling the user to do what the button already does — e.g. "The source Claude is in
the middle of a turn." — and update its test.

- [ ] **Step 1: Write the failing tests** — for each narrowing (a `waiting` answer
accepted by `moveSession`, a `preview` answer refused, `cancelMoveWait` narrowing to
`wait_cancelled` and refusing anything else, the args sent — `when: 'idle'` by
default, `when: 'cancel'` for a cancel), and that `mergeSession` is **not** called on
`waiting`. Build fixtures as complete typed objects. Prove each can fail.
- [ ] **Step 2: Watch them fail** — paste it.
- [ ] **Step 3: Implement.** Wrong-shape answers use the existing `E_PARSE`; no new
code, no new `E_`-prefixed literal.
- [ ] **Step 4:**

```bash
npx vitest run
```

```bash
npx svelte-check
```

Commit: `feat(ui): a transfer may answer "waiting", and a wait can be cancelled`.

---

### Task 7: The run store — a `waiting` run, Cancel, and a reopen that remembers

**Files:**
- Modify: `src/lib/timeline.ts`, `src/lib/timeline.test.ts` — `unresolvedWait`.
- Modify: `src/lib/moves.ts`, `src/lib/moves.test.ts`.

**Interfaces:**
- Produces, for Task 8:

```ts
// timeline.ts
export interface UnresolvedWait { sessionId: number; toHost: string; deadlineUnix: number | null }
/** The newest session_move_waiting with no later session_move_wait_ended. Pure. */
export function unresolvedWait(events: SessionEvent[]): UnresolvedWait | null;
/** The reason of the newest session_move_wait_ended, or null. Pure. */
export function lastWaitEnd(events: SessionEvent[]): string | null;

// moves.ts
export type MoveStatus = 'running' | 'done' | 'failed' | 'partial' | 'waiting';
// MoveRun gains: deadlineUnix: number | null; waitEnded: string | null
export function cancelWait(sessionId: number): void;
export function adoptWait(w: UnresolvedWait, sessionName: string): void;
```

**Behaviour:**
- `startMove` sends `when: 'idle'`. A `waiting` answer sets the run to `waiting`
  with `deadlineUnix`; a `moved` answer settles it as today.
- While a run is `waiting`, the waiter's eventual move reports through
  `move:progress`. A `check:started` for that session takes the run from `waiting`
  to `running` — the same way an observed run starts today. Read `applyMoveProgress`
  and its `awaitingStart` / `SETTLE_GRACE_MS` handling (3d) and make the `waiting`
  transition fit it; a straggler from an earlier attempt must still not land on it.
- A wait can end without a move (cancelled, timed out, session gone, refused, hub
  restarted). The run store learns this from the timeline: while a run is `waiting`,
  subscribe to that session's live timeline events (`onTimelineEvent` in
  `src/lib/live_events.ts`) and, on a `session_move_wait_ended` whose reason is not
  `moved`, settle the run as `failed` with `waitEnded` set to the reason. Unsubscribe
  when it settles. A `moved` reason needs no action — `move:progress` settles it.
- `cancelWait` calls `cancelMoveWait` with the run's `toHost`; on success with
  `was_waiting: false`, say so (the wait had already ended) rather than pretending it
  was cancelled.
- `adoptWait` rebuilds a `waiting` run after a reopen, keyed like 3d's
  `adoptPartial`; a no-op when a run already exists for the session.

The two transitions that can go wrong, as code (adapt the helper names to
`moves.test.ts`'s real ones — `row`, `ok`, `flush`, `invoked`, and how it feeds a
`MoveProgress` into `applyMoveProgress`):

```ts
  it('a waiting run becomes running when the waiter\'s move starts', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'waiting', session_id: 7, to_host: 'beta', deadline_unix: 2_000_000_000 }));
    startMove(row({ id: 7, tmux_name: 's', host_alias: 'alpha' }), 'beta', { keepSource: false });
    await flush();
    expect(get(moves).get(7)?.status).toBe('waiting');
    applyMoveProgress({ session_id: 7, to_host: 'beta', step: 'check', index: 0, total: 9, state: 'started', detail: null });
    expect(get(moves).get(7)?.status).toBe('running');
  });

  it('a wait that ended without a move settles the run with its reason', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'waiting', session_id: 7, to_host: 'beta', deadline_unix: 2_000_000_000 }));
    startMove(row({ id: 7, tmux_name: 's', host_alias: 'alpha' }), 'beta', { keepSource: false });
    await flush();
    // What the live timeline channel delivers for session 7:
    emitTimelineEvent(7, {
      id: 1, session_id: 7, at: 1, kind: 'session_move_wait_ended',
      detail: JSON.stringify({ reason: 'timed_out', to_host: 'beta' }), claude_session_id: null,
    });
    const run = get(moves).get(7)!;
    expect(run.status).toBe('failed');
    expect(run.waitEnded).toBe('timed_out');
  });
```

`emitTimelineEvent` stands for however `live_events.ts` lets a test deliver a
timeline event to `onTimelineEvent` subscribers — read it; if it has no test hook,
add the smallest one there (a `…ForTest` export, the `moves.ts` convention) rather
than mocking the module wholesale.

**Tests** — for `unresolvedWait`/`lastWaitEnd` over newest-first events including a
wait closed by an end, a newer wait after a closed one, and malformed details;
for `moves.ts`: `startMove` sending `idle`, a `waiting` answer, `check:started`
taking `waiting` to `running`, a non-`moved` end settling `failed` with the reason,
`cancelWait` sending the target, `was_waiting: false` handled, `adoptWait` a no-op
on an existing run. Each shown able to fail; RED chronological.

- [ ] **Step 1:** Write the tests. **Step 2:** Watch them fail, paste. **Step 3:**
Implement. **Step 4:**

```bash
npx vitest run
```

```bash
npx svelte-check
```

Commit: `feat(ui): the run store waits, cancels, and remembers a wait across a reopen`.

---

### Task 8: The sheet and the chip show a pending transfer

**Files:**
- Modify: `src/lib/TransferSheet.svelte`, `src/lib/TransferSheet.test.ts`
- Modify: `src/lib/TransferChip.svelte`, `src/lib/TransferChip.test.ts`
- Modify: `src/lib/SessionDetails.svelte` and its test — only to call `adoptWait`
  from the timeline it already receives (3d's `onEvents` path), as it calls
  `adoptPartial`.

**Behaviour:**
- A `waiting` run: the sheet shows "Waiting for {session} to finish — will transfer
  to {host}", when it gives up (from `deadlineUnix`, relative), and **Cancel**. Cancel
  is one click — it is the safe action.
- A run that ended with `waitEnded` set: say why, in plain words per reason
  (cancelled, timed out after the limit, the session disappeared, the move was
  refused — with the refusal — or the hub restarted), and offer Transfer again.
- The chip shows a waiting state distinct from running.
- The setup view's Transfer button is unchanged in when it is enabled (3b's rule:
  never gated by the preview); it now yields either a move or a wait.
- Svelte 5 runes in the file's existing style; no new colour literals; existing
  `data-testid` conventions.

**Tests:** the waiting view renders the host and a Cancel that calls `cancelWait`;
each `waitEnded` reason renders its sentence; the chip's waiting state; the details
panel adopting a wait from its timeline. Each shown able to fail; RED chronological.

- [ ] **Step 1:** Write the tests. **Step 2:** Watch them fail. **Step 3:** Implement.
**Step 4:**

```bash
npx vitest run
```

```bash
npx svelte-check
```

Byte-scan every touched file for control bytes. Commit:
`feat(ui): a pending transfer shows in the sheet and the chip, with Cancel`.

---

## Before the PR

1. `git fetch origin` and compare. This branch is stacked on
   `feature/transfer-preflight` (PR #219): if #219 has merged, rebase is forbidden —
   merge `origin/main` locally and re-run every suite on the merged tree; a clean
   text merge hid a budget break in 3b.
2. The whole-branch review on the most capable model, with the controller's worries
   listed — then ONE fix wave, one scoped re-review, residuals adjudicated. In 3b
   it found that a dry run could `git fetch` on the source; in 3d, two Criticals.
3. Ask before any `carry_e2e` run against a host. Never run a dev build.
