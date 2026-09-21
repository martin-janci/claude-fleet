# Transfer slice 3c — wait for idle

**Date:** 2026-09-21
**Status:** design, approved in brainstorm
**Follows:** slices 1, 2, 3a, 3d and 3b (`specs/2026-09-21-transfer-preflight-design.md`)
**Roadmap:** `docs/superpowers/2026-09-20-transfer-roadmap.md` → "3c"
**Branch base:** `feature/transfer-preflight` (3b, PR #219) — 3c builds on the
preview and the contract gate 3b introduced.

## 1. What this slice changes

Today a Transfer of a session whose Claude is mid-turn is refused: "The source
Claude is in the middle of a turn. Wait for it to finish, then transfer." The
user has to watch the session and come back.

3c turns that into **"Transfer when it finishes"**: the move waits for the
source to go idle and then runs — and, in hub-client mode, it waits **on the
hub**, so the user can close the desktop and the always-on hub moves the session
when it is done.

## 2. Decisions

| # | Question | Decision |
|---|---|---|
| D1 | What must the wait survive? | **The desktop closing.** The wait runs where the move runs — inside the app in local mode, on the hub in hub-client mode — as an in-memory background task. A hub (or app) restart ends it, and says so (§5). |
| D2 | How is it exposed? | **One `when` argument on `move_session`**: `now` (default, today's behaviour), `idle`, `cancel`. A new `MoveOutcome::Waiting`. No new command, tool, verdict or routing row. |
| D3 | `blocked`, and how long? | **Wait through `blocked`**, bounded by a new setting `move.wait_max_mins`, default 240. |
| D4 | How does a reopened desktop learn a wait is pending? | **Timeline events** — `session_move_waiting` / `session_move_wait_ended` — **plus a startup sweep** that closes any the process can no longer honour. |

### Why these, and not the alternatives

- **Not a frontend-only wait (D1).** Cheapest by far, but in hub-client mode it
  dies with the desktop — exactly the case where waiting on the hub is worth
  having. **Not a durable, restart-surviving record** either: that is a
  migration, tick integration, new commands and tools, and the question of what
  a pending transfer means days later when the target host has changed.
- **Not two new commands (D2).** 3b showed that every verdict row, routing row
  and params struct is a place a flag can get lost, and that the served MCP
  surface has almost no room. One enum argument is the smallest routed surface
  that can both start and cancel a wait. The existing call-id cancellation
  registry does not fit: in the waiting case the call has already returned —
  deliberately, so the desktop can close — so there is no in-flight call to
  cancel.
- **Not "stop at `blocked`" (D3).** A question can be answered from anywhere,
  including a paired phone; the session then finishes and moves as asked.
  **Not an unbounded wait** either: a transfer asked for on Friday should not
  fire on Monday into a changed world with nothing to show it was still pending.
- **Not a live-only status query (D4).** It can never be stale, but it leaves no
  record of a wait that timed out or was lost to a restart. **Not a column on
  the session row:** a migration and a new field on the most-read wire type,
  with the same staleness problem and none of the history.

## 3. The hazard this slice inherits from 3b — and the contract bump

A hub ignores an argument it does not know (`MoveSessionParams` has no
`deny_unknown_fields`). 3b hit this with `dry_run`, which an old hub would have
run as a real move; 3b bumped the wire contract to revision 2 and made the
desktop refuse a dry run until this launch had confirmed an in-range hub.

3c has the same shape, and it is sharper for one of the three values:

- `when: "idle"` sent to a revision-2 hub is **harmless**: the hub ignores it,
  treats the request as `now`, and a busy source is refused as today.
- `when: "cancel"` sent to a revision-2 hub is **not**: the hub ignores it, sees
  an ordinary move request, and **moves the session immediately** — cancelling a
  wait would perform the very move it was meant to stop.

So:

1. **`fleet_core::wire_contract::CONTRACT_REVISION` goes 2 → 3**, with a history
   entry naming `when` and the cancel hazard; the desktop's `MIN_HUB_CONTRACT` and
   `MAX_HUB_CONTRACT` become `3`. As in 3b, a mismatched desktop and hub refuse
   each other loudly, and `docs/hub.md` says they must be upgraded together.
2. **3b's backend guard widens.** The desktop's routed `move_session` refuses,
   with `E_HUB_CONTRACT`, any request with `dry_run: true` **or `when` other than
   `now`** until this launch has positively seen an in-range `ready` frame.
   Plain moves are unaffected, as in 3b.

## 4. The wire

`MoveSessionArgs` gains an argument (arguments may default; report types may
not):

```rust
/// When to move. `now` (default): as before — a busy source is refused.
/// `idle`: move now if the source is idle, otherwise wait for it and return
/// `Waiting`. `cancel`: end a pending wait for this session.
#[serde(default)]
pub when: When,

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum When { #[default] Now, Idle, Cancel }
```

`MoveOutcome` gains a variant (boxed like the others, so the enum stays small):

```rust
Waiting(Box<MoveWaiting>),

/// A wait has been registered; the move will run when the source goes idle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveWaiting {
    pub session_id: i64,
    pub to_host: String,
    /// Unix seconds after which the wait gives up (`move.wait_max_mins`).
    pub deadline_unix: i64,
}
```

and `when: cancel` answers `Waiting`-free: a new `MoveOutcome::WaitCancelled {
session_id, was_waiting: bool }` — `was_waiting: false` when there was nothing
to cancel, so a cancel is never an error and never silent.

No `#[serde(default)]` on `MoveWaiting` or `WaitCancelled` (wire rule).

`when` is added to `MoveSessionParams` and mapped through its single `into_args`
(3d's lesson), with a non-default row in `tests_routing.rs`. It costs one enum
parameter in the served MCP schema; the budget test's constant gets **one
deliberate, measured raise**, recorded in its doc comment with the numbers, as
the test's own documentation asks.

**`dry_run` and `when` together:** a dry run with `when: idle` skips exactly one
check — the source-idle check, which `idle` exists to defer — and says "the
source is busy now; the move will wait for it" in `unknowns`. Every other check
runs as before. `when: cancel` with `dry_run: true` is refused (`E_INVALID`):
there is nothing to preview about a cancel.

## 5. The waiter

A new module, `move_session/wait.rs`.

- **Registry.** An in-memory map keyed by `(store address, session id)` — the
  same keying the move's in-flight guard uses — holding each waiter's
  `CancellationToken` and deadline. A second `when: idle` for a session already
  waiting is refused (`E_INVALID_STATE`, "a transfer of this session is already
  waiting"); so is `when: idle` for a session with a move in flight.
- **Start.** `when: idle`: if the source is idle, run the move exactly as `now`
  does and return `Moved`. Otherwise register, record `session_move_waiting`, spawn
  the waiter, and return `Waiting` at once. Spawning needs owned handles, so the
  public `move_session` takes the store as `&Arc<Mutex<Store>>` (today
  `&Mutex<Store>`); its callers already hold that `Arc`.
- **Wait.** `tasks::wait_for_session_with(store, id, WaitCond::Idle, …)` — the
  existing bounded poll, with the same notion of idle as the move's own
  `require_source_idle` and no lock held across its sleep — raced in a
  `tokio::select!` against the cancel token and the deadline.
- **On idle.** Run the real move with the original arguments (`when: now`),
  which re-checks everything. If it is refused **only** because the source went
  busy again (the `require_source_idle` refusal), go back to waiting, same
  deadline. Any other outcome ends the wait: `Moved`, a partial, or another
  refusal, each recorded.
- **End.** Always deregister, and record `session_move_wait_ended` with a
  reason: `moved`, `partial`, `refused`, `cancelled`, `timed_out`, `session_gone`.
- **Cancel.** `when: cancel` fires the token and returns `WaitCancelled`. A
  waiter that is mid-move when cancelled lets the move finish — 3c does not
  cancel a running move (a roadmap non-goal) — and records `moved` or whatever
  the move produced.

**Startup sweep.** On desktop and hub startup — beside the existing
`spawn_reconcile_tick` calls in `src-tauri/src/bootstrap/tasks.rs` and
`crates/fleet-hub/src/serve.rs` — every `session_move_waiting` with no later
`session_move_wait_ended` is closed as `hub_restarted`. History never claims a
wait that no longer exists.

**Setting.** `move.wait_max_mins`, default `240`, with an upper bound in
`service/settings.rs` beside the other `MOVE_*` constants.

## 6. Frontend

- `moveSession.ts`: `when` on the wire; `moveSession()` narrows to `moved` or
  `waiting`; a `cancelMoveWait(sessionId)` wrapper narrows to `wait_cancelled`.
- `moves.ts`: the run gains a `waiting` status (with `deadline`); Transfer always
  sends `when: "idle"`, so an idle source moves at once and a busy one yields a
  waiting run; `cancelWait(sessionId)`. When the waiter's move starts, its
  `move:progress` events take the run from `waiting` to `running` exactly as an
  observed run is taken today.
- `timeline.ts`: `unresolvedWait(events)` — the newest `session_move_waiting`
  not followed by `session_move_wait_ended` — and `moves.ts`'s `adoptWait`
  rebuilds a waiting run after a reopen, as 3d's `adoptPartial` does.
- `TransferSheet.svelte` and the chip: "Waiting for {session} to finish — will
  transfer to {host}", the deadline, and **Cancel**. A wait that ended without a
  move shows why, from the `session_move_wait_ended` reason.
- `moveErrors.ts`: the "is not idle" refusal stops being a dead end — Transfer
  now waits instead — so its text becomes the reason a *plain* move was refused,
  used only where a caller asked for `now`.

## 7. Testing

Engine, over `FakeSsh`, with a status the test can flip:

1. `when: idle` on an idle source moves at once and returns `Moved`.
2. On a busy source it returns `Waiting` immediately and records `session_move_waiting`.
3. Flipping the status to idle runs the move; the wait ends `moved`.
4. `blocked` does not end the wait; idle after `blocked` does.
5. A busy-again refusal resumes waiting within the same deadline.
6. The deadline ends the wait `timed_out` with no move.
7. The session disappearing ends it `session_gone`.
8. `when: cancel` ends it `cancelled`; cancel with nothing pending returns
   `was_waiting: false`.
9. A second `when: idle` is refused.
10. The startup sweep closes an unresolved `session_move_waiting` as `hub_restarted`.
11. No store lock is held across the wait (the existing poll's discipline).
12. `when` survives every hop: params → `into_args` → service; a non-default
    routing row; a desktop refuses `when: cancel` before a confirmed in-range hub.
13. The contract range: a revision-2 hub is refused.

Frontend: the waiting status, Cancel, `adoptWait`, the reason shown for a wait
that ended without a move, and Transfer sending `when: "idle"`.

Every test is shown able to fail — 3b found four plan-written assertions that
could not.

## 8. Non-goals

- Surviving a hub or app restart (the sweep records the loss honestly instead).
- Cancelling a move that has already started.
- Bulk or queued moves; more than one pending wait per session.
- A per-transfer timeout — the setting is global.

## 9. Risks

- **The cancel hazard (§3) is the one that matters.** The contract bump and the
  widened backend guard are what stand between a Cancel click and an unwanted
  move on an older hub.
- **Another contract bump means another lock-step upgrade** of desktop and hub.
  3b and 3c both bump it; shipping them together is one upgrade, not two.
- **The public `move_session` signature changes** (`&Arc<Mutex<Store>>`); every
  caller must be updated, and the compiler will find them.
- **A waiter holds memory and a task per pending wait** for up to
  `move.wait_max_mins`; bounded by one per session.
- **Freshness of idle.** The waiter reads the store's status, kept current by
  hooks and the reconcile tick; a status that lags only delays the move, and the
  real move's own `refresh_host` re-checks before anything is written.
