# Transfer sheet: one button, live progress, a readable result

**Date:** 2026-09-20
**Status:** Implemented
**Builds on:** `docs/superpowers/specs/2026-09-19-move-carry-engine-design.md`
(slice 1, PR #163), `docs/superpowers/specs/2026-09-20-move-carry-claude-state-design.md`
(slice 2, PR #194) and `docs/adr/0002-move-carries-work-as-is.md`
**Slice:** 3a of 3. Slice 3 is four parts, each with its own spec, plan and
PR: **3a the Transfer sheet (this document)**, 3b preflight, 3c wait-for-idle,
3d return trip and retry.

## Goal

Moving a session to another host is one visible button, and the person who
pressed it can see what is happening and what happened.

Today the move is a button at the bottom of the details panel that opens a
modal with a host picker. Pressing **Move** shows "Moving…" for as long as the
move takes — tens of seconds to minutes — with no indication of which stage is
running. The modal cannot be closed meanwhile. The result is a toast: the
`CarryReport` that slices 1 and 2 fill in (commits carried, files left behind
and why, memory kept on the target) is never shown, and the warnings arrive
joined by `;` in one line. A failure is an error toast carrying the raw
message.

## Decisions already made

- The Transfer control lives in the **terminal header**, on the host name:
  `on [ mefistos ⇄ ]`.
- The sheet is **closable while the move continues**. While a move runs, the
  header control becomes a live indicator (`⇄ moving to hetzner · 5/9`) that
  reopens the sheet.
- The details-panel button stays and opens the **same** sheet. Its own modal
  is removed.

## Non-goals

- Cancelling a running move, retrying a failed one, moving a session back
  (3d).
- A preview of what would travel before the move starts (3b).
- Waiting for a busy session to go idle (3c). A busy source is refused exactly
  as today; the sheet shows that refusal as a readable sentence.
- `strict` in the UI. It stays an MCP-only argument.
- Any change to `MoveReport`, to the `move_session` command's arguments, or to
  the MCP tool's arguments or description. The served tool surface is
  byte-budgeted; this slice adds nothing to it.
- Persisting move runs. A run lives in the window's memory and is gone after a
  restart; the session timeline (`session_history`) already records the move.

## 1. Backend: the `move:progress` event

### The steps

`move_session_inner` runs in numbered stages (0–7, with 3a–3e inside the
carry). Users do not need that granularity. The move has nine user-facing
steps, in order, a `snake_case`-serialized enum `MoveStep`:

| # | `MoveStep`     | Covers stage(s) in `move_session_inner`                  | Can only warn |
|---|----------------|----------------------------------------------------------|---------------|
| 1 | `check`        | 0 source idle, 1 source git state                        | no            |
| 2 | `transcript`   | 2 locate, cap, read                                      | no            |
| 3 | `workspace`    | 3 resolve the target's paths, 3a seed its main clone     | no            |
| 4 | `git`          | 3b snapshot, bundle, relay, fetch                        | no            |
| 5 | `replay`       | create / fast-forward the worktree and its refusals (target dirty, diverged, larger transcript), 3c replay, verify | no |
| 6 | `ignored`      | 3d small git-ignored files                               | yes           |
| 7 | `claude_state` | 3e session directory and project memory                  | yes           |
| 8 | `start`        | 4 start the target, 5 confirm                            | no            |
| 9 | `handoff`      | 6 source check and kill (or keep), 7 record on both rows | no            |

`MoveStep::ALL: [MoveStep; 9]` is the single source of the order and of
`total`. `index` is 1-based. The worktree is created AFTER the fetch (it is
fast-forwarded to a commit the fetch brings), so its setup and refusals belong
to `replay`, which the sheet labels "Set up the worktree, replay uncommitted
work".

### The payload

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MoveProgress {
    pub session_id: i64,        // the SOURCE session's row id
    pub to_host: String,        // target host alias
    pub step: MoveStep,
    pub index: u8,              // 1..=9
    pub total: u8,              // 9
    pub state: MoveStepState,   // started | done | warned | failed
    pub detail: Option<String>, // short, human; see below
}
```

No `#[serde(default)]` anywhere (the wire rule from `service::repo_read`).
`MoveProgress` and the two enums live in `events.rs` beside `SyncProgress`,
since `RowChange` owns its payload types; `progress.rs` holds the emitter and
the stage-to-step mapping.

`detail` is set only where it is cheap and useful: `git` done →
`"2 commits"` (or `"nothing to carry"`), `ignored` done/warned →
`"3 files"`, `claude_state` done/warned → `"46 files, 4 notes"`. It never
carries a path, a prompt or transcript text. On `failed` it is `None` — the
error itself reaches the caller through the command's `Err`, and the event
must not duplicate a message that may quote stderr.

### The event

- `RowChange::MoveProgress(MoveProgress)` → name `move:progress`, kind
  `move`.
- `EVENT_NAMES` grows from 18 to 19 and `EVENT_KINDS` from 10 to 11
  (`"move"`). The existing Rust tests that pin names against
  `src/lib/events.ts` and keep `EVENT_KINDS`, `EVENT_NAMES` and
  `RowChange::name` in step are extended, not bypassed.
- `EventBus` gains `fn move_progress(&self, p: &MoveProgress)`; `Store` gains
  `pub fn bus_move_progress(&self, p: &MoveProgress)`, next to
  `bus_sync_progress` in `store/`.
- The hub's `GET /events` forwards every `RowChange`, and the desktop bridge
  (`src-tauri/src/backend/events.rs`) re-emits names found in `EVENT_NAMES`,
  so a hub-routed desktop receives the event with no further change. The
  bridge's event table in `src-tauri/src/backend/tests_events.rs` gains the
  new variant. The hub contract golden pins row types only, not events, so it
  does not change.

### Emitting

`progress.rs` exposes one small type:

```rust
pub(super) struct Progress<'a> { store: &'a Mutex<Store>, session_id: i64, to_host: String, current: Option<MoveStep> }

impl Progress<'_> {
    pub(super) fn start(&mut self, step: MoveStep);                 // ends `current` as done first, if any
    pub(super) fn done(&mut self, detail: Option<String>);          // ends `current` as done
    pub(super) fn warned(&mut self, detail: Option<String>);        // ends `current` as warned
    pub(super) fn fail(&mut self);                                  // ends `current` as failed; no-op when none
}
```

- Each emit locks the store, calls `bus_move_progress`, and drops the guard —
  never across an `.await`. A poisoned mutex logs a `tracing::warn!` and
  returns, exactly like `emit_progress` in `service/catalog/sync/mod.rs`.
  Progress can never fail or slow a move.
- `move_session_steps` owns the `Progress`. It passes `&mut Progress` into
  `move_session_inner`; when `inner` returns `Err` it calls `fail()`, and when
  it returns `Ok` it calls `done(None)` to close step 9 — both before running
  `CarryCleanup`. So the failing step is whichever step was current, with no
  per-`?` bookkeeping inside `inner`.
- `inner` calls `start(step)` at the nine boundaries and `done`/`warned` where
  it has a detail or a warning to report. A step that pushed onto `warnings`
  during steps 6–7 ends `warned`; elsewhere warnings do not change the step
  state (they still reach the report).
- Refusals that happen **before** `move_session_steps` (unknown session, the
  `MoveClaim` already held, validation) emit nothing. The sheet learns of them
  from the command's `Err`.
- There is no separate terminal event. The run's end is the command's result
  for the window that started it, and the last step's `done`/`failed` for any
  other observer.

## 2. Frontend store: `src/lib/moves.ts`

```ts
export type StepState = 'pending' | 'started' | 'done' | 'warned' | 'failed';
export type MoveStatus = 'running' | 'done' | 'failed' | 'partial';

export interface MoveRun {
  sessionId: number;
  sessionName: string;      // tmux_name at start; the row may be gone later
  fromHost: string;
  toHost: string;
  keepSource: boolean | null;   // null for an observed run
  origin: 'local' | 'observed';
  steps: { step: MoveStep; state: StepState; detail: string | null }[];  // always 9, in order
  status: MoveStatus;
  report: MoveReport | null;
  error: IpcError | null;
  startedAt: number;
}

export const moves: Readable<Map<number, MoveRun>>;
export function startMove(session: Session, toHost: string, opts: { keepSource: boolean }): void;
export function applyMoveProgress(p: MoveProgress): void;
export function dismissMove(sessionId: number): void;
export function activeMoveFor(sessionId: number): MoveRun | undefined;   // status === 'running'
```

- The wire types (`MoveStep`, `MoveStepState`, `MoveProgress`), the constant
  `MOVE_STEPS` (the nine names in order) and `stepLabel(step, toHost)` live in
  `src/lib/moveProgress.ts`, which both `events.ts` and `moves.ts` import. A
  Rust test — the same trick that pins `EVENT_NAMES` against
  `src/lib/events.ts` — reads that file and asserts that the quoted names
  between its `// move-steps:begin` and `// move-steps:end` markers equal the
  serialized names of `MoveStep::ALL`, in order. A tenth step cannot be added
  on one side only.
- `startMove` creates the run (`origin: 'local'`, all steps `pending`,
  `status: 'running'`), calls `moveSession(...)` **without the caller awaiting
  it**, and settles the run from the result: `Ok` → `done` + `report`;
  `Err` with code `E_MOVE_PARTIAL` → `partial` + `error`; any other `Err` →
  `failed` + `error`. It refuses (returns without invoking) when a run for
  that session is already `running`.
- When a local run settles: if the source is still the selected session and
  the move succeeded, selection follows to `report.target` (today's
  behaviour); and if the sheet is not open on that run, a toast says how it
  ended ("Moved {name} to {host}" / the error toast "Move of {name} failed"),
  so a move finished behind a closed sheet is never silent.
- `dismissMove` removes only a settled run; a running one cannot be dismissed.
- A `check`/`started` event for a session whose run is already settled starts
  a fresh observed run in its place (someone moved that session again).
- `applyMoveProgress` patches the step at `index`. Rules, so out-of-order or
  duplicated events cannot corrupt the list:
  - a step's state only moves forward: `pending → started → done|warned|failed`;
  - a `started` for step *n* marks any earlier `pending`/`started` step `done`;
  - an event for a session with no run creates an `observed` run (from the
    sessions store for the name and source host; `keepSource: null`). This is
    a move started by another window, the MCP API or a hub client;
  - an observed run has no command result, so it settles from events: step 9
    `done` → `done`; any `failed` → `failed` with `error: null`;
  - events for a run that is already settled are ignored.
- `events.ts` gains `{ name: 'move:progress'; payload: MoveProgress }`, the
  `onMoveProgress` handler and the `sub('move:progress', …)` line, following
  `sync:progress`. `App.svelte`'s subscription wires it to
  `applyMoveProgress`.
- Eligibility moves out of `SessionDetails.svelte` into
  `src/lib/moveEligibility.ts` so both entry points share it:
  `canMoveSession(session)`, `moveTargetsFor(session, hosts)` and
  `moveBlockedReason(hubStatus, hubConnection)` — the same rules as today
  (`kind === 'work'`, a worktree, a Claude session id; targets are other,
  visible, reachable hosts that are provisioned or `local`;
  `hubActionBlocked('move_session', …)`).

## 3. UI

### `TransferChip.svelte` — the header control

Props: `session`. Renders one of:

- **Idle, movable:** `on` + a button showing the host alias and the
  arrows-left-right glyph, `data-testid="transfer-chip"`, title "Transfer this
  session to another host". Click opens the sheet.
- **Idle, not movable** (`canMoveSession` false): the plain `on {host}` text,
  as today. No disabled button — a shell session has nothing to explain.
- **Blocked** (`moveBlockedReason` non-null): the button, disabled, with the
  reason as its title.
- **Running:** `⇄ moving to {toHost} · {n}/9`, `data-testid="transfer-live"`,
  where *n* is the index of the latest `started` step. Click reopens the
  sheet.
- **Settled, not yet dismissed:** `⇄ moved to {toHost}` / `⇄ move failed`,
  same click. Dismissing the sheet's result view clears it.

It replaces `<span class="host">on {host_alias}</span>` in
`TerminalView.svelte`'s header. A desktop paired to a hub has no terminal
header (`!ownsTheFleet` renders the attach-line panel instead), so the chip is
also rendered at the top of that panel; the move itself is hub-routed and
works there.

The sheet's open state is one store, `transferSheetFor: Writable<number | null>`
(a session id) in `moves.ts`, so the chip, the details button and the sheet
agree. `App.svelte` mounts a single `<TransferSheet />`.

### `TransferSheet.svelte`

Built on the existing `Modal`. Which view shows is derived from the run for
`$transferSheetFor`:

**Setup** (no run). Title "Transfer {name}". From (fixed), To (a `<select
data-testid="move-target">` over `moveTargetsFor`), the checkbox "Keep this
session running" (`move-keep-source`), one sentence on what travels
("Uncommitted and unpushed work, small ignored files, subagents and project
memory travel too. Nothing is pushed or committed."), Cancel and **Transfer**
(`confirm-move`). With no eligible target the select is replaced by "No other
reachable, provisioned host." and Transfer is disabled. The existing test ids
are kept so the current tests move with the markup.

**Progress** (`running`). Title "Moving to {toHost}". The nine steps with
their labels — Check the source · Read the conversation · Prepare the target ·
Carry the git work · Replay uncommitted work · Ignored files · Subagents and
memory · Start on {toHost} · Hand over — each with a state icon and its
`detail`. One button: **Close**. Closing never affects the move. An observed
run adds a line: "Started elsewhere — this window is following along."

**Result** (`done`). Title "Moved to {toHost}". Four summary rows from
`report.carried`:

- git: `{commits} commits · {dirty_entries} uncommitted entries` (or "Nothing
  to carry — the branch was pushed and clean");
- ignored files: `{ignored_carried.length} carried`, `· {n} left behind`;
- session files: carried · kept on target · left;
- memory: notes carried · already there (kept + identical) · index lines.

Then the warnings, **one per line** — never joined. **Details** expands the
lists behind the counts: each left-behind file with its reason, the kept and
identical memory names, the index lines added. Buttons: **Open on {toHost}**
(selects `report.target_session_id`) and **Done** (dismisses the run). An observed
run that finished has no report: the view says "Moved to {toHost}" and
offers Open when the new session can be found — the row on {toHost} whose
parent is the moved session — otherwise only Done.

**Failure** (`failed`, `partial`). Title "The move did not finish". Three
parts:

1. *What failed* — a sentence from `src/lib/moveErrors.ts`, which maps the
   error code and `details.step` to text. Codes: `E_MOVE_DIRTY`,
   `E_MOVE_UNPUSHED`, `E_MOVE_MIDOP`, `E_MOVE_TARGET_DIRTY`,
   `E_MOVE_TOO_LARGE`, `E_MOVE_CARRY` (by `details.step`: `seed`, `haves`,
   `snapshot`, `download`, `upload`, `fetch`, `apply`, `verify`, `target`), `E_MOVE_PARTIAL`, `E_INVALID_STATE` (the busy /
   unknown-status refusal and the move already in progress), `E_LOCAL_ONLY`, and a fallback that shows the
   backend's message unchanged. The failed step is highlighted in the step
   list, which stays visible.
2. *Where things stand* — honest about what cleanup does (it removes only the
   transfer refs and the transfer directory). For `failed`, by the step
   reached: nothing past `transcript` → "Nothing was copied to {toHost}. The
   source session was not touched."; otherwise "The source session was not
   touched. Temporary transfer files were removed; what was already set up on
   {toHost} — the clone, the worktree, copied files — was left there." For
   `partial`, a neutral line — "A new session exists on {toHost} and the
   source is still there. Nothing was killed." — with links to both sessions
   (`details.target_session_id`); *what failed* then comes from
   `details.step` (confirm timeout; the source wrote after the copy → kill the
   TARGET and transfer again; the source could not be stopped; the target
   could not be confirmed), with a neutral fallback for a string this build
   does not know.
3. *Raw details* — collapsed: code, message, `details.stderr` when present.

Buttons: **Done**. (Retry is 3d.)

### `SessionDetails.svelte`

`move-from-details` stays, keeps its label and its disabled/title behaviour,
and now sets `transferSheetFor`. The `{#if moveOpen}` modal, `doMove`, and the
move-local state are deleted; the eligibility deriveds call
`moveEligibility.ts`.

## 4. Edge cases

- **The source row disappears mid-run** (a normal move retires it). The run
  holds `sessionName`/`fromHost`, and the sheet is keyed by the run, not the
  row. If the selected session changes because the row vanished, the sheet
  stays open when it was open.
- **Two windows.** Both show the run; one `local`, one `observed`. The
  `MoveClaim` refuses a second start; the sheet shows that as a failure
  sentence ("This session is already being moved.").
- **Events without a result** (the app was reloaded mid-move). The run is
  re-created as `observed` on the next event and settles from events.
- **A result without events** (event stream down, or an old hub that does not
  emit `move:progress`). `startMove` still settles the run; on `Ok` every
  step not already `warned` is marked `done`, and on `Err` with no `failed`
  step the step that was `started` is marked `failed`. The progress view then simply shows nothing
  moving until the end — today's behaviour, no worse. This is also why the
  hub should be upgraded before the desktops (ADR 0002 already says so).
- **`detail` from an untrusted hub.** Rendered as text, truncated at 80
  characters.

## 5. Testing

Rust (`fleet-core`):

- a clean move emits, in order, `started`/`done` for all nine steps with
  `index` 1..=9 and `total` 9, using a recording `EventBus`;
- a failure at each of several stages ends the stream with that step `failed`
  and nothing after it (table-driven over the existing fault-injection cases);
- a warning in stage 3d or 3e ends that step `warned` and the move still
  completes;
- a pre-claim refusal emits nothing;
- `EVENT_NAMES`/`EVENT_KINDS`/`RowChange::name` parity, the `events.ts` name
  check, the `moveProgress.ts` step-list check, the `/events` kind filter
  accepting `move`, and the desktop bridge carrying the new event.

Vitest:

- `moves.test.ts`: the reducer rules above (forward-only, gap filling,
  observed runs, settled runs ignore events, result-without-events,
  double-start refused);
- `moveErrors.test.ts`: every code and carry step has a sentence; unknown
  codes fall back to the message;
- `TransferSheet.test.ts`: each view from a fixture run; warnings render one
  per line; Details expands; Close leaves the run running; Done dismisses;
- `TransferChip.test.ts`: the five states; click opens the sheet;
- `SessionDetails` tests: the button opens the sheet and the old modal is
  gone; `TerminalView` tests: the chip is in the header, and in the
  attach-line panel in hub-client mode.

No dev build of the desktop app is run as part of this work: a dev build
migrates the production database and kills the installed app. UI behaviour is
verified by the component tests; the event stream by the Rust tests.

## 6. Files

New: `crates/fleet-core/src/service/move_session/progress.rs`,
`src/lib/moveProgress.ts`, `src/lib/moves.ts`, `src/lib/moveEligibility.ts`, `src/lib/moveErrors.ts`,
`src/lib/TransferSheet.svelte`, `src/lib/TransferChip.svelte`, and their
tests.

Changed: `crates/fleet-core/src/events.rs`, `crates/fleet-core/src/store/`
(one bus method), `crates/fleet-core/src/service/move_session/mod.rs` (the
nine `start` calls and the `Progress` plumbing),
`crates/fleet-core/src/mcp/events_route.rs` and
`src-tauri/src/backend/tests_events.rs` (tests only),
`src/lib/events.ts`, `src/App.svelte`, `src/lib/TerminalView.svelte`,
`src/lib/SessionDetails.svelte`, `docs/control-api.md` (the `/events` names list) and
`docs/adr/0002-move-carries-work-as-is.md` (a short "what the user sees"
note).

## 7. Revisions after the whole-branch review (2026-09-20)

The first implementation followed sections 1–6 as written; a whole-branch
review found places where the design itself was wrong. What changed, and why:

- **A settled run's steps are derived, not rewritten.** `settle()` stores
  `status`, `report`, `error` and `settledAt` and leaves `steps` alone;
  `displaySteps(run)` is what the UI renders (a `done` run shows every
  non-warned step done; a `failed`/`partial` run with no failed step shows the
  step that was running as failed — never a step the user watched succeed).
- **Results and events race.** They travel on separate channels and events
  wait out a 16 ms flush, so a fast refusal's own `check:started` can arrive
  AFTER the result. Section 2's rule "a `check`/`started` event replaces a
  settled run" would then swap the real error for an empty observed run. For
  `SETTLE_GRACE_MS` (5 s) after a LOCAL run settles, events only patch its
  steps forward; after that the replace rule applies. Cost: a new move of the
  same session started within 5 s of the last one ending is drawn on the old
  run.
- **Every field of an event is validated** before a run is touched
  (`session_id`, `to_host`, `index`/`step` agreement, `state` one of the four
  wire states, `detail` a string or null): `applyMoveProgress` never throws — a
  throw would lose the whole event batch — and the desktop's hub bridge checks
  the payload's shape before re-emitting it.
- **Observed runs always have a way out.** A dropped event stream would leave
  one at "moving…" forever and block `startMove`. `dismissMove` removes an
  observed run in any state; the running view offers "Stop following".
- **Hub-client mode.** The hub client bounded every tool call at 30 s, so a
  real move was reported as failed while it kept running on the hub.
  `move_session` now has its own 15-minute call bound; a lost connection
  (`E_HUB_UNREACHABLE`) is "outcome unknown" — the run becomes observed and
  keeps following events, with the note "Lost contact with the hub — the move
  may still be running there."; and a `Progress` dropped mid-step (the caller
  went away) still emits that step's `failed`.
- **Selection follows even though the source row is gone.** A normal move
  kills the source row before the result returns, which clears the selection,
  so "still the selected session" never held. Selection follows to the target
  when it is empty or still the source, with `{ follow: true }`.
- **A result behind a closed sheet stays reachable.** The success toast
  carries the warning count and the keep-source note, has a View action, and
  does not auto-dismiss when there are warnings. The NEW session's header chip
  finds the run (`runForSession`: by `report.target_session_id`) and shows
  "⇄ moved from {host}". A run attaches to a session id only while the name
  and source host still match, since row ids can be reused.
- **Smaller:** `partial` reads "⇄ move incomplete" on the chip; an unknown
  left-behind reason renders as its raw value; the live chip has a title and
  an aria-label; a busy source gets its own sentence.

Still open, by decision: a run whose hub goes quiet never settles on its own
(Stop following is the way out); the partial step strings are matched by value
with a neutral fallback rather than pinned against Rust.
