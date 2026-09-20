# Transfer slice 3d — retry and the return trip

**Date:** 2026-09-20
**Status:** design, approved in brainstorm
**Follows:** `2026-09-19-move-carry-engine-design.md` (slice 1),
`2026-09-20-move-carry-claude-state-design.md` (slice 2),
`2026-09-20-transfer-sheet-design.md` (slice 3a)
**Roadmap:** `docs/superpowers/2026-09-20-transfer-roadmap.md` → "3d"
**ADRs:** `0002-move-carries-work-as-is.md`

## 1. What this slice fixes

A move that fails after the target worktree has been written leaves that work
in place. The next attempt — a retry, or a deliberate trip back to the host the
session came from — meets its own leftovers and is refused with
`E_MOVE_TARGET_DIRTY`. So the one thing a failed transfer most needs is the one
thing it cannot do.

Slice 3a's sheet has the matching gap: every failure ends in a single **Done**,
and a partial (`E_MOVE_PARTIAL` — the target session exists, both are alive)
ends there too, with prose telling the user to go and kill something by hand.

3d closes both: the engine learns when a dirty target is *the state the move
was about to create anyway*, and the sheet grows the four actions that follow
from that — Retry, Clean up and retry, Move back, and Finish / Undo for a
partial.

## 2. Decisions

| # | Question | Decision |
|---|---|---|
| D1 | When are a target's leftovers ours? | **Content-exact adopt** (§3) plus an explicit cleanup action (§4). Not porcelain equality — it compares status+path only, so it would adopt a target whose content differs and then call the move verified. Not a lineage marker — it asserts lineage where the content can be verified, and it would mean leaving refs on hosts after a failure. |
| D2 | Which failures get Retry? | Clean failures re-run `move_session` unchanged. A partial gets **full recovery**: Finish or Undo (§5). |
| D3 | How are cleanup / finish / undo exposed? | A `clean_target` flag on `move_session`, and **one** new command `resolve_move { session_id, action }`. |
| D4 | Where does "Move back to {host}" get its host? | From the target's own `session_moved` timeline event, which outlives the reaped source row. Offered on the result view and in the details panel — **not** on every sidebar chip. |
| D5 | What may Undo destroy? | Only the target's tmux session, and only while the target has taken no turn. Never its worktree, transcript or carried files. |

D4 deliberately narrows the roadmap's sketch, which said "on the chip of a
session whose `parent_session_id` points at a moved source". `parent_session_id`
points at a row the kill path ghosts and the reconcile reaps, so the button
would vanish minutes after the move; and reading a timeline per sidebar row to
replace it costs a fetch per row. The durable record is the event.

## 3. Content-exact adopt

### 3.1 The check

New `carry::verify_replayed_script(cwd, claude_id, want_head)`. It runs **only**
when `carry::apply_script` has already refused with `carry::TARGET_DIRTY`, and
answers one question: *is this dirty worktree already exactly the snapshot we
were about to replay?*

The snapshot's `wt` tree is `git add -A` over a copy of the source index
(`carry.rs`, `snapshot_script`), so it is the whole non-ignored working tree,
not a diff — which is what makes an exhaustive answer possible. A throwaway
`GIT_INDEX_FILE` carries the comparison so the target's real index is never
written:

```sh
dir="$HOME/.cache/claude-fleet/transfer/$id"   # id_guard + home_guard first
mkdir -p -- "$dir"; umask 077
tmp="$dir/verify.ix"; rm -f -- "$tmp"
GIT_INDEX_FILE="$tmp" git read-tree "refs/fleet/transfer/$id/wt^{tree}"
GIT_INDEX_FILE="$tmp" git update-index -q --refresh          # hash where stat is unknown
ours_wt=$(GIT_INDEX_FILE="$tmp" git diff-index --name-only "refs/fleet/transfer/$id/wt^{tree}" --)
theirs=$(GIT_INDEX_FILE="$tmp" git ls-files --others --exclude-standard)
ours_ix=$(git diff-index --cached --name-only "refs/fleet/transfer/$id/ix^{tree}" --)
```

- `ours_wt` — paths the snapshot holds whose content, mode or presence on the
  target differs. **Ours**: the snapshot writes them.
- `ours_ix` — paths whose *staging* differs from the snapshot's index tree.
  **Ours**, same reason.
- `theirs` — non-ignored paths on the target that the snapshot does not hold at
  all. **Theirs**: nothing in this move would ever write them.

`--exclude-standard` keeps git-ignored files out of `theirs`: they are the
ignored-carry step's business (never a failure, only a warning), not the
replay's.

All three empty ⇒ **adopt**. The script then prints `OUT_MARKER` followed by
`carry::STATUS_PORCELAIN`, exactly as `apply_script` does on success, so the
existing verification in `move_session` — the target's porcelain must equal the
source's dirty set — runs unchanged over the adopted state. Adoption skips the
`read-tree`; it does not skip being checked.

Not empty ⇒ exit non-zero with a new sentinel `carry::LEFTOVERS_DIFFER`, then
the two sets on stderr as `ours <NUL-joined>` / `theirs <NUL-joined>`, capped
at 50 paths each with a `+N more` count.

`want_head` is re-checked first: a target at another commit is `HEAD_MISMATCH`
as before, never an adopt.

### 3.2 What the move does with it

In `move_session` step 3c, where `apply_script`'s `TARGET_DIRTY` currently
becomes `target_dirty(&cwd, &target)`:

1. Run `verify_replayed_script`.
2. Clean ⇒ emit the `Replay` step as `Done` with detail `already in place`,
   push the report warning
   `"{cwd} on {target} already held exactly this work; nothing was replayed"`,
   and continue with the porcelain the script printed.
3. `LEFTOVERS_DIFFER` with `theirs` empty ⇒ `E_MOVE_TARGET_DIRTY` with
   `details.leftovers = "ours"` and `details.ours = [paths]`. The message says
   the work an unfinished attempt left differs from what is being carried now,
   and that a cleanup can replace it.
4. `LEFTOVERS_DIFFER` with `theirs` non-empty ⇒ `E_MOVE_TARGET_DIRTY` with
   `details.leftovers = "theirs"`, `details.theirs = [paths]` (and `ours` when
   both). The message names those paths as the target's own and refuses
   outright: no cleanup can be safe there.
5. The verify script failing for any other reason ⇒ today's `target_dirty`
   message verbatim, with `details.leftovers = "unknown"`. A broken check must
   never widen what the move is willing to overwrite.

The `TARGET_DIRTY` that `target_prep_script` can return before the replay (the
fast-forward path) is **not** classified — it keeps today's refusal. Corrected
during Task 2, where the review showed the original claim was unreachable: prep
emits that sentinel only when the target HEAD is a strict ancestor of the source
HEAD, while `verify_replayed_script` requires `HEAD == want_head`, so the verify
could only ever answer `HEAD_MISMATCH` there. It is unreachable in principle
too — a target an earlier attempt fast-forwarded has `HEAD == want` on the retry
and prep does not refuse, and a target that was never fast-forwarded never ran
the replay, so it holds no leftovers of ours. Calling the verifier there bought
nothing and added an `E_PARSE` path in a fast-forward race.

## 4. `clean_target`

`MoveSessionArgs` gains:

```rust
/// Replace what an unfinished earlier attempt left in the target worktree.
/// Refused when the target also holds work of its own. Default false.
#[serde(default)]
pub clean_target: bool,
```

`Serialize` is already derived; the hub-routed argument rule means a
non-default row in `src-tauri/src/backend/tests_routing.rs`.

`apply_script`'s inner `recover()` — reset to `HEAD`, then delete exactly the
paths the snapshot tree *adds*, never `git clean` — is lifted into
`carry::recover_script(cwd, claude_id)` and called from `apply_script` through
the same text, so the two cannot drift.

With the flag set, and only after §3.2 case 3 (`theirs` empty), the move runs
`recover_script` and then `apply_script` again, inside the same call — so the
target is never left cleaned-but-not-moved. The `Replay` step reports
`Warned` with detail `replaced an unfinished attempt's work`, and the report
gains a warning naming the count of paths removed.

`clean_target` with `theirs` non-empty is refused, flag or not: `E_MOVE_TARGET_DIRTY`
with `details.leftovers = "theirs"`. The flag can never reach a path the check
did not classify as ours.

## 5. `resolve_move`

```rust
pub struct ResolveMoveArgs {
    /// The TARGET session of the partial move.
    pub session_id: i64,
    pub action: ResolveMoveAction,   // Finish | Undo
}
pub enum ResolveMoveAction { Finish, Undo }
```

Returns `ResolveMoveReport { action, source_session_id, target_session_id,
from_host, to_host, source_killed, target_killed, warnings }` —
`Serialize + Deserialize`, no `serde(default)`, per the wire rule.

### 5.1 The partial event becomes the durable handle

`record_partial` is reached through `move_session_with`'s `inspect_err`, so it
sees nothing but the error — every new fact has to travel in the error's
`details`. `partial()` therefore takes a `PartialCtx` built once, just after the
target row is confirmed, and every `partial()` call site passes it:

```rust
struct PartialCtx {
    from_host: String,
    to_tmux_name: String,
    claude_session_id: String,
    branch: String,
    /// The source transcript as the copy was taken; None before the copy.
    source_transcript: Option<Located>,
    /// The target row's counters as the move last saw them.
    to_turn_seq: Option<i64>,
    to_last_turn_at: Option<i64>,
}
```

`record_partial` copies those through into the event detail as `from_host`,
`to_tmux_name`, `claude_session_id`, `branch`, `source_transcript_size`,
`source_transcript_mtime`, `to_turn_seq`, `to_last_turn_at` — alongside the
`step`, `to_host` and ids it already writes. A field whose value the move never
reached is written as `null`, and `resolve_move` refuses on a `null` it needs
rather than guessing (§5.2, §5.3).

This mirrors D4: `session_moved` is the durable handle for the return trip,
`session_move_partial` is the durable handle for recovery.

`resolve_move` reads the newest `session_move_partial` on the target row that
is not already followed by a `session_moved` or `session_move_undone`, and
refuses with `E_INVALID_STATE` when there is none ("this session is not a
partial move"). The source is that event's `from_session_id`; a partial never
killed the source, so the row is alive.

### 5.2 Finish

Step 4 of the move — re-locate the source transcript, compare it with what was
copied, snapshot usage, `hooks.kill_source`, one last look, then `session_moved`
on both rows — is extracted from `move_session` into
`finalise_source(...) -> Result<FinaliseOutcome, IpcError>` in a new
`move_session/finalise.rs`, and called by both paths. The move keeps today's
behaviour byte for byte; Finish gets it without a second implementation.

Finish refuses when the source transcript's size or mtime differs from the
`source_transcript_size` / `_mtime` in the partial event: the source took a turn
the target does not have, so killing it would lose that turn
(`E_INVALID_STATE`, naming both pairs, telling the user to move again once the
source is idle). It also refuses when the target row is not `running`.

On success it records `session_moved` on both rows with the same detail shape
the move writes, plus `"finished_from_partial": true`.

### 5.3 Undo

Kills the target through the same normal kill path and records
`session_move_undone` on both rows (detail: the partial's step, both hosts,
both ids, `claude_session_id`).

It refuses (`E_INVALID_STATE`, naming what it saw) when:

- the target's `turn_seq` or `last_turn_at` has moved past the values in the
  partial event — the target holds a turn the source does not; or
- the target's `claude_status` is not `idle` (a `working` or `blocked` target
  is mid-turn, and `blocked` is not idle).

It never touches the target's worktree, its copied transcript, its merged
Claude state or the carried ignored files. Those are exactly what lets a later
retry adopt in one step under §3 — and removing a worktree is `delete_worktree`'s
job, which has its own refusals.

The `MoveHooks::kill_source` hook is renamed `kill_tmux_session` (same
signature): Undo kills the target with it, and a hook named `kill_source` doing
that would be a lie. The rename touches the fakes in
`move_session/mod.rs`'s tests only.

## 6. Frontend

### 6.1 `moves.ts`

- `startMove` gains an optional `{ cleanTarget?: boolean }`, passed through
  `moveSession`.
- New `retryMove(sessionId)`: re-runs the run's own target and options over
  the same run entry, resetting its steps to pending. It requires the run to
  be `failed` (never `running`, never `partial`).
- New `resolveMove(sessionId, action)` wrapper over the command; on success it
  settles the run as `done` (Finish) or `failed`-with-undone text (Undo) and
  lets the row events do the rest.
- `MoveRun` gains `cleanTarget: boolean` (what the last attempt asked for) and
  `attempt: number` (so the sheet can say "attempt 2").

### 6.2 `moveErrors.ts`

`E_MOVE_TARGET_DIRTY` splits on `details.leftovers`:

| value | text |
|---|---|
| `ours` | `{host} still holds the work an earlier transfer attempt left behind, and it differs from what is being carried now.` + the action |
| `theirs` | `{host} has uncommitted work of its own in this worktree: {paths}. Commit or discard it there first.` |
| `unknown` / absent | today's sentence, unchanged |

Plus the `session_move_undone` result line and Finish's success line. The
`ALWAYS_TOUCHED_THE_TARGET` set keeps `E_MOVE_TARGET_DIRTY`, since adoption
implies nothing was written.

### 6.3 `TransferSheet.svelte`

- **failed** view: `Retry` (always, for a clean failure) and — only when
  `details.leftovers === 'ours'` — `Clean up {host} and retry`, which first
  shows the paths it will remove and needs a second click.
- **done** view: `Move back to {run.fromHost}`.
- **partial** view: `Finish the move` and `Undo`, each with its own one-line
  confirm; a refusal from either is rendered in place, not as a toast.
- The setup view is unchanged.

The `sessionHistory` read happens inside `Timeline.svelte` (`Timeline.svelte:43`),
not in `SessionDetails.svelte` — and `Timeline` also re-fetches on live timeline
events. So `Timeline` gains an `onEvents` callback prop and hands its events up,
rather than the panel making a second `sessionHistory` call (an extra hub round
trip per panel open, and two sources of truth for freshness). Either way: no new
IPC.

Two new pure helpers in `timeline.ts` read those events: `moveOrigin(events)`
returns `{ fromHost, claudeSessionId } | null` from the newest `session_moved`,
and `unresolvedPartial(events)` returns the partial that Finish/Undo apply to —
`null` once a `session_moved` or `session_move_undone` follows it.

The panel shows `Move back to {host}` and, for an unresolved partial,
`Finish the move` / `Undo`. Both partial buttons open the **sheet**, which owns
the confirmations and the refusal text, so there is exactly one place a
destructive recovery can be triggered from. Because the app has usually been
restarted since the partial, the sheet needs a run to show: `moves.ts` gains
`adoptPartial(...)`, which rebuilds a `partial` run from the recorded event.

## 7. Generated files and surfaces

- New command `resolve_move`: a row in `src-tauri/src/backend/verdicts.rs`
  (`Verdict::Routed { tool: "resolve_move" }` — the hub owns the hosts, so
  parity, not refusal), the `generate_handler!` entry, a routed call and a
  non-default args row in `tests_routing.rs`, then
  `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` and
  `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
- New MCP tool: **yes, and it has to be.** `Verdict::Routed` resolves its hub
  tool through the MCP table (`backend/remote.rs`, `tool_for`), so a routed
  command without a tool of that name fails closed — and `LocalOnly` would leave
  a hub-client desktop unable to recover a partial at all, which is the opposite
  of this slice's point. So `resolve_move` gets a deliberately slim
  `#[tool(...)]` in `mcp/tools/lifecycle.rs`: two parameters, one sentence, the
  reasoning in this spec rather than in the description. That means
  - a `TOOL_POLICIES` row in `mcp/guard.rs` — `Access::Client`, `readonly:
    false`, `confirm: true`, `Deadline::Lifecycle`, matching `move_session`'s
    (classification is mandatory; a tool with no row fails an exhaustiveness
    test);
  - `assert_eq!(served, 73)` in `mcp/tools/tests.rs` becomes `74`;
  - the surface stays under `BUDGET_BYTES = 56_000` — the test prints the
    number, and the budget is not to be raised for this.

  `move_session`'s own description gains `clean_target` in one clause.
- `clean_target` on `move_session`: a non-default row in `tests_routing.rs`.
- `ResolveMoveReport` is a report type, not a stored row, so
  `REGEN_HUB_CONTRACT` is not involved. No migration: every new fact lives in
  `session_events` details.

## 8. Testing

Engine (`FakeSsh`, `move_session/mod.rs` tests plus `carry.rs` script tests):

1. adopt — target dirty, content identical ⇒ move completes, `Replay` is
   `Done` with `already in place`, report warning present, no `read-tree` ran.
2. adopt refused, ours — a snapshot path differs ⇒ `E_MOVE_TARGET_DIRTY`,
   `details.leftovers == "ours"`, the path listed.
3. adopt refused, theirs — an extra non-ignored path ⇒ `leftovers == "theirs"`,
   and `clean_target: true` is still refused.
4. an extra *ignored* file on the target does not make it `theirs`.
5. `clean_target` happy path ⇒ recover then apply, `Replay` is `Warned`, the
   move completes.
6. the verify script failing for an unrelated reason ⇒ `leftovers == "unknown"`
   and today's message.
7. `HEAD_MISMATCH` still wins over any adopt.
8. Finish ⇒ source killed, `session_moved` on both rows with
   `finished_from_partial`, and the extracted `finalise_source` is the same
   code the move runs (a test asserts the move's own path still produces
   today's events).
9. Finish refused when the source transcript grew after the partial.
10. Undo ⇒ target killed, `session_move_undone` on both rows, worktree and
    transcript untouched (the fake records no cleanup call).
11. Undo refused when the target's `turn_seq` moved, and when its
    `claude_status` is `working` or `blocked`.
12. `resolve_move` on a session with no unresolved partial ⇒ `E_INVALID_STATE`.
13. a partial recorded before the transcript copy (a `null`
    `source_transcript_size`) ⇒ Finish refuses rather than killing on a fact it
    does not have; Undo on a `null` `to_turn_seq` refuses the same way.

Frontend (Vitest): `moveErrors` text for each `leftovers` value; `retryMove`
re-running with the same target and refusing on a `running`/`partial` run; the
cleanup button's two-click confirm and the paths it lists; `Move back` using
`fromHost`; Finish/Undo buttons and their in-place refusals; `moveOrigin` /
`unresolvedPartial` over event fixtures including the
`session_moved`-after-partial ordering.

Not covered by tests, and deliberately: no dev build (it migrates the
production `state.db` and kills the installed app). The real end-to-end path
needs `cargo run -p fleet-core --example carry_e2e -- <host>`, which is asked
for before it is pointed anywhere.

## 9. Non-goals

- Cancelling a running move (needs cancel-safe `CarryCleanup`; a roadmap debt).
- Bulk or queued moves.
- Removing the target worktree as part of Undo (§5.3).
- A preflight of what would travel — that is slice 3b.
- Waiting for a busy source — slice 3c.

## 10. Risks

- **The adopt check is the safety boundary.** If it says clean when the content
  differs, work is silently lost. It is deliberately built from `git`'s own
  comparisons over a throwaway index rather than from parsed text, and the
  existing porcelain verification still runs over the adopted state.
- **`Ours` cannot tell an earlier attempt's leftover from the target's own edit
  to a file the snapshot also writes.** The paths are identical, so the
  classification cannot separate them, and `clean_target`'s reset to `HEAD`
  discards the target's version. This is the one way the cleanup can still
  destroy work someone wanted. What stands against it: the flag is never
  implied (a flagless `Ours` refuses and says to inspect first), the sheet names
  every path before the second click, and a single path the snapshot does NOT
  write makes the whole target `Theirs` and refuses outright.
- **A retried apply that fails after a cleanup leaves the target cleaned but not
  replayed.** The source is untouched and the message says so, so this is
  disclosed rather than hidden; recovering is another retry.
- **`finalise_source` extraction touches the most delicate part of the move**
  (the kill and the events). The extraction is behaviour-preserving by
  construction: the move's existing tests for the kill, the usage carry and the
  post-kill warning must pass unchanged, and that is stated as a task
  acceptance criterion rather than left to the reviewer to notice.
- **Two writers on `mod.rs`.** The adopt path and the `finalise_source`
  extraction both edit `move_session/mod.rs`. Per the roadmap's rule they are
  sequenced in the plan, never parallel.
