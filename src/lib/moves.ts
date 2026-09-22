// One run per moving session: what the Transfer sheet and the header chip
// render. A run started in this window ('local') is settled by the command's
// result; one seen only through `move:progress` events ('observed' — another
// window, the MCP API, a hub client) is settled by its events. Nothing here
// is persisted: the session timeline already records the move.
import { get, writable, type Readable } from 'svelte/store';
import { ALREADY_WAITING, UNDONE } from './moveErrors';
import { MOVE_STEPS, type MoveProgress, type MoveStep, type MoveStepState } from './moveProgress';
import {
  cancelMoveWait,
  moveSession,
  resolveMove,
  type MoveReport,
  type MoveWaitCancelled,
  type MoveWaiting,
  type ResolveAction,
  type ResolveMoveReport,
} from './moveSession';
import type { IpcError, Result } from './result';
import { sessions, type SessionRow } from './sessions';
import { selectedSession, selectSession } from './selection';
import { onTimelineEvent } from './live_events';
import { hubConnection, type HubConnection } from './hub_connection';
import {
  sessionHistory,
  unresolvedWait,
  type SessionEvent,
  type UnresolvedPartial,
  type UnresolvedWait,
} from './timeline';
import { push, pushError } from './toasts';

export type StepState = 'pending' | MoveStepState;
export type MoveStatus = 'running' | 'done' | 'failed' | 'partial' | 'waiting';

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
  /** Always the nine steps, in order, exactly as the events left them. */
  steps: MoveRunStep[];
  status: MoveStatus;
  report: MoveReport | null;
  error: IpcError | null;
  startedAt: number;
  /** When the command's result settled this run; null while it runs. */
  settledAt: number | null;
  /** Whether the last (re)try asked to replace a stale attempt's leftovers
   *  on the target instead of refusing with `E_MOVE_TARGET_DIRTY`. */
  cleanTarget: boolean;
  /** 1 for the original attempt; `retryMove` bumps it, on the same run. */
  attempt: number;
  /** A refusal from the last Finish/Undo attempt (`resolveMoveRun`), shown in
   *  the sheet rather than as a toast — the user is looking straight at it
   *  when the click comes back refused. Cleared on the next `resolveMoveRun`
   *  call for this run, and whenever the attempt settles either way. */
  resolveError: IpcError | null;
  /** True while a `resolve_move` (Finish/Undo) for this run is in flight.
   *  Finish and Undo each kill a live session, so a second click must not
   *  fire a second call: the first answer would settle the run and the
   *  second refusal would land on a run no view renders any more. Cleared
   *  the moment the call answers, either way. */
  resolving: boolean;
  /**
   * True from the moment `startMove` or `retryMove` (re)sets this run until
   * its own first `move:progress` event (`check`/`started`) arrives; every
   * other event is dropped while it is true.
   *
   * `move:progress` carries only the session id, not a per-move id, so a
   * straggler from the attempt this run replaced looks exactly like this
   * attempt's own next step: `applyMoveProgress` would otherwise apply it
   * straight onto the fresh (blank) step list, since the (re)start already
   * put `status: 'running'` and `settledAt: null` — the very shape that lets
   * ordinary events through. A per-move id on the wire would make this flag
   * unnecessary; that is backend work tracked separately, not part of this
   * task.
   *
   * `startMove` needs it whenever it REPLACES a settled run under the same
   * key, for the same reason: a straggler that marks a step `failed` on the
   * fresh run makes `endedInTheSteps` answer `'failed'`, and an
   * `E_HUB_UNREACHABLE` answer then settles a move still running on the hub
   * as a failure. A first move of a session has no earlier attempt to
   * straggle, so it does not wait.
   */
  awaitingStart: boolean;
  /** The wall-clock deadline (`when: 'idle'`'s `MoveWaiting.deadline_unix`) a
   *  `waiting` run will time out at; `null` off that status. */
  deadlineUnix: number | null;
  /** Why a `waiting` run's wait ended — the reason on its
   *  `session_move_wait_ended` (`cancelled`, `timed_out`, `session_gone`,
   *  `refused`, `hub_restarted`, or `moved` on a run settled `done` from
   *  that event alone), or `UNKNOWN_WAIT_END` when the backend said nothing
   *  was waiting any more and the timeline recorded no end; `null` while
   *  waiting, or for a run that never waited. */
  waitEnded: string | null;
  /** The refusal `record_wait_ended` recorded on a `refused`
   *  `session_move_wait_ended` (`crates/fleet-core/src/service/move_session/
   *  wait.rs`) — `code` and `message`, the same shape as `IpcError`'s own.
   *  `null` for every other reason, and for a `refused` one whose detail
   *  predates these fields or is otherwise unusable. */
  waitRefusal: { code: string; message: string } | null;
}

const DETAIL_MAX = 80;
const RANK: Record<StepState, number> = { pending: 0, started: 1, done: 2, warned: 2, failed: 2 };
const STATES: ReadonlySet<string> = new Set<MoveStepState>(['started', 'done', 'warned', 'failed']);

/**
 * How long after a local run settles its own queued events may still arrive.
 *
 * The result and the events are two streams: the command's answer can win the
 * race against the `move:progress` events it produced. Inside this window an
 * event may only move the step list forward — it can never revive the run or
 * overwrite the result the user is looking at. After it, a `check:started` is
 * a genuinely new move of the same session.
 */
export const SETTLE_GRACE_MS = 5000;

/** The codes the hub client answers with when the hub said nothing at all
 *  (no connection, a timed-out exchange, a proxy's 5xx). The move is very
 *  probably still running there, so it is not a failure. */
const NO_ANSWER: ReadonlySet<string> = new Set(['E_HUB_UNREACHABLE', 'E_HUB_TIMEOUT']);

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
  // A `waiting` run has no move running YET, but it is just as much "already
  // doing something with this session" as a running one: a second click must
  // not race it into the backend's own "already waiting" refusal.
  return run?.status === 'running' || run?.status === 'waiting' ? run : undefined;
}

/** 1-based number of the step the run has reached (at least 1). */
export function stepNumber(run: MoveRun): number {
  let n = 1;
  run.steps.forEach((s, i) => {
    if (s.state !== 'pending') n = i + 1;
  });
  return n;
}

/**
 * The steps as the sheet shows them — never what is stored.
 *
 * A finished run is the truth about its own outcome even when its events
 * never arrived (an old hub, a dropped stream, a result that simply won the
 * race), so the picture is completed here, at render time. Settling itself
 * leaves `steps` alone, so a late event can still name the step that failed.
 */
export function displaySteps(run: MoveRun): MoveRunStep[] {
  if (run.status === 'done') {
    return run.steps.map((s) => (s.state === 'warned' ? s : { ...s, state: 'done' as StepState }));
  }
  if (run.status === 'running' || run.status === 'waiting' || run.steps.some((s) => s.state === 'failed')) {
    return run.steps;
  }
  // Failed or partial with no `failed` event — the events lost the race, or
  // never came. The step that was running is the one that failed; failing
  // that, the one the move had got to (step 1 when nothing arrived at all).
  // Never step 1 when step 1 is known to have finished: showing a step the
  // user watched succeed as failed is worse than showing nothing.
  const pending = run.steps.findIndex((s) => s.state === 'pending');
  let at = pending < 0 ? run.steps.length - 1 : pending;
  run.steps.forEach((s, i) => {
    if (s.state === 'started') at = i;
  });
  return run.steps.map((s, i) => (i === at ? { ...s, state: 'failed' as StepState } : s));
}

/**
 * The run the chip should show beside `session`, if any.
 *
 * Keyed by the SOURCE session's id — but a row id is reused when a session is
 * re-discovered, so the run must still name the same session; otherwise an
 * unrelated session that inherited the id would wear someone else's move.
 * A session that a move PRODUCED finds the run through the report, under the
 * same guard: a target id is reused exactly as readily as a source one.
 */
export function runForSession(
  map: Map<number, MoveRun>,
  session: SessionRow,
): MoveRun | undefined {
  const own = map.get(session.id);
  // A run built from an event before this window had the row (`observed`)
  // knows no name or host — the id is all it ever had, so there is nothing
  // to hold it to. Every other run is held to both.
  if (own && (own.fromHost === '' || (own.sessionName === session.tmux_name && own.fromHost === session.host_alias))) {
    return own;
  }
  for (const run of map.values()) {
    if (
      run.report?.target_session_id === session.id &&
      run.report.tmux_name === session.tmux_name &&
      run.toHost === session.host_alias
    ) {
      return run;
    }
    // A partial's target, which has no report to be found through: a Finish
    // settles the run with `report: null` (the recovery's own report carries
    // no tmux name, so synthesising one would mean lying to the guard just
    // above), and by then the run is keyed to a source id the Finish reaped.
    // The partial error's details are what is left that names the target.
    const d = run.error?.details;
    if (typeof d === 'object' && d !== null) {
      const details = d as Record<string, unknown>;
      const name = details.target_tmux_name;
      if (
        details.target_session_id === session.id &&
        run.toHost === session.host_alias &&
        // A run adopted from a recorded partial has no tmux name to check —
        // it is held to the id and the host, like an `observed` run.
        (typeof name !== 'string' || name === session.tmux_name)
      ) {
        return run;
      }
    }
  }
  return undefined;
}

/** Start a move without waiting for it. A no-op while one is running. */
export function startMove(session: SessionRow, toHost: string, opts: { keepSource: boolean }): void {
  if (activeMoveFor(session.id)) return;
  // A run already under this key is a settled one this start replaces (a
  // failed attempt, a done move of a re-discovered row): its last events may
  // still be on their way, and this run has the very shape that lets them
  // through. A key nothing has used cannot have stragglers, and waiting for
  // an event that may never come (an old hub, a dropped stream) would leave
  // such a run blank — so the flag is only raised where the hazard is real.
  const replacing = get(store).has(session.id);
  unwatchWait(session.id);
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
    resolveError: null,
    startedAt: Date.now(),
    settledAt: null,
    cleanTarget: false,
    attempt: 1,
    resolving: false,
    // Same mechanism as `retryMove`. See `MoveRun.awaitingStart`.
    awaitingStart: replacing,
    deadlineUnix: null,
    waitEnded: null,
    waitRefusal: null,
  });
  void moveSession(session.id, toHost, { keepSource: opts.keepSource, when: 'idle' }).then((r) =>
    settleMoveResult(session.id, r),
  );
}

/** "1 warning" / "3 warnings". */
function warningCount(n: number): string {
  return `${n} warning${n === 1 ? '' : 's'}`;
}

/**
 * What the events already said about how the move ended, or null while they
 * have not said it. Only the answer can be lost; the events are a separate
 * stream and a terminal one that arrived is as good as a result.
 */
function endedInTheSteps(steps: MoveRunStep[]): MoveStatus | null {
  if (steps.some((s) => s.state === 'failed')) return 'failed';
  const last = steps[steps.length - 1].state;
  return last === 'done' || last === 'warned' ? 'done' : null;
}

function settle(sessionId: number, r: Result<MoveReport>): void {
  const run = get(store).get(sessionId);
  if (!run || run.origin !== 'local' || run.status !== 'running') return;
  const sheetOpen = get(transferSheetFor) === sessionId;
  if (r.ok) {
    put({ ...run, status: 'done', report: r.value, settledAt: Date.now() });
    // The source row is usually gone by now — killed, which cleared the
    // selection — so "nothing is selected" is the same case as "the source
    // is selected": both mean the user was watching this session. `follow`
    // keeps a move that finished behind a closed sheet from pulling them
    // out of whatever view they moved on to.
    const sel = get(selectedSession);
    if (sel === null || sel.id === sessionId) selectSession(r.value.target, { follow: true });
    if (!sheetOpen) {
      const warnings = r.value.warnings.length;
      push({
        kind: 'success',
        message:
          `Moved ${run.sessionName} to ${run.toHost}` +
          (warnings > 0 ? ` · ${warningCount(warnings)}` : '') +
          (r.value.source_killed ? '' : ' · the source keeps running'),
        // Warnings are the one case worth reading, so that toast waits.
        sticky: warnings > 0,
        action: { label: 'View', run: () => transferSheetFor.set(sessionId) },
      });
    }
    return;
  }
  if (r.error.code === 'E_INVALID_STATE' && r.error.message.includes('already in progress')) {
    // The CLICK was refused, not the move: another move of this session is
    // already running (this window let go of one, or another window owns
    // it). Settling a failure here would show a step failing that never did
    // — and the real move's events would then patch that failed run. Follow
    // the real move instead, and say why the Transfer did nothing. The
    // toast goes up even with the sheet open: the sheet shows a move
    // running, which is not an answer to "why was my click ignored".
    put({ ...run, origin: 'observed', keepSource: null, error: null, settledAt: null });
    pushError(r.error, `Transfer of ${run.sessionName}`);
    return;
  }
  if (NO_ANSWER.has(r.error.code)) {
    // The hub never answered. Unless its events already said how the move
    // ended, the move is most likely still running there and its events
    // still reach this window — so keep following it as if it had been
    // started elsewhere, and let the user stop following (F3).
    const ended = endedInTheSteps(run.steps);
    if (ended !== null) {
      put({ ...run, status: ended, error: r.error, settledAt: Date.now() });
      return;
    }
    put({ ...run, origin: 'observed', error: r.error });
    return;
  }
  if (r.error.code === 'E_INVALID_STATE' && r.error.message.includes(ALREADY_WAITING)) {
    // `begin_wait` refused: this session already has a wait (another window,
    // the MCP API). A Retry could only be refused again, so follow that wait
    // instead — found on the session's own timeline, like `adoptWait`.
    void followRecordedWait(sessionId, r.error);
    return;
  }
  settleFailed(run, r.error);
}

function settleFailed(run: MoveRun, error: IpcError): void {
  put({
    ...run,
    status: error.code === 'E_MOVE_PARTIAL' ? 'partial' : 'failed',
    error,
    settledAt: Date.now(),
  });
  if (get(transferSheetFor) !== run.sessionId) pushError(error, `Move of ${run.sessionName} failed`);
}

/** M5: turn a Transfer refused with "already waiting to move" into the wait
 *  the session's timeline records — its target and deadline, not this
 *  click's — or, when the timeline records none, the refusal itself. */
async function followRecordedWait(sessionId: number, error: IpcError): Promise<void> {
  const h = await sessionHistory(sessionId);
  const run = get(store).get(sessionId);
  if (!run || run.origin !== 'local' || run.status !== 'running') return;
  const w = h.ok && Array.isArray(h.value) ? unresolvedWait(h.value) : null;
  if (!w) {
    settleFailed(run, error);
    return;
  }
  const toHost = w.toHost !== '' ? w.toHost : run.toHost;
  put({
    ...run,
    toHost,
    keepSource: null,
    steps: blank(),
    status: 'waiting',
    error: null,
    awaitingStart: false,
    deadlineUnix: w.deadlineUnix,
    waitEnded: null,
    waitRefusal: null,
  });
  watchWait(sessionId);
  push({
    kind: 'info',
    message: `${run.sessionName} was already waiting to move to ${toHost} — following that wait.`,
  });
}

/** One subscription per session whose run belongs to a live wait, watching
 *  its own timeline for the `session_move_wait_ended` that ends it (see
 *  `watchWait`). It is kept through every attempt the waiter makes — a
 *  busy-again attempt returns the run to `waiting`, and only the wait's end
 *  says it is over — and torn down when the run settles, is dismissed, or is
 *  replaced, so a session is never listened to twice. */
const waitWatchers = new Map<number, () => void>();

function unwatchWait(sessionId: number): void {
  waitWatchers.get(sessionId)?.();
  waitWatchers.delete(sessionId);
}

/** `waitEnded` for a wait that is over with no recorded reason: the backend
 *  answered a Cancel with `was_waiting: false`, and the session's timeline
 *  holds no `session_move_wait_ended` for it (yet). Not a backend reason —
 *  it says only what this window knows. A real end arriving later replaces
 *  it (`waitOpen`). */
export const UNKNOWN_WAIT_END = 'unknown';

/** Whether `run` still belongs to a wait this window follows, so that the
 *  wait's end (or its next attempt) may settle it: parked `waiting`, running
 *  one of the waiter's own attempts, or settled only as "ended, reason
 *  unknown". */
function waitOpen(run: MoveRun): boolean {
  if (run.status === 'waiting') return true;
  if (!waitWatchers.has(run.sessionId)) return false;
  return run.status === 'running' || (run.status === 'failed' && run.waitEnded === UNKNOWN_WAIT_END);
}

/** `detail` on `session_move_wait_ended` — parsed as defensively as
 *  `timeline.ts`'s own `detailOf`, since this is the same wire shape crossing
 *  the same IPC boundary. */
function waitEndReason(detail: string | null): string | null {
  if (detail === null) return null;
  try {
    const v: unknown = JSON.parse(detail);
    if (typeof v !== 'object' || v === null) return null;
    const reason = (v as Record<string, unknown>).reason;
    return typeof reason === 'string' ? reason : null;
  } catch {
    return null;
  }
}

/** `code`/`message` on a `refused` `session_move_wait_ended` — only
 *  `record_wait_ended` (`crates/fleet-core/src/service/move_session/wait.rs`)
 *  writes them, and only for that one reason. `null` when either field is
 *  missing (an older backend's detail, or any other reason) so the caller can
 *  fall back to a plain sentence instead of showing half a refusal. */
function waitEndRefusal(detail: string | null): { code: string; message: string } | null {
  if (detail === null) return null;
  try {
    const v: unknown = JSON.parse(detail);
    if (typeof v !== 'object' || v === null) return null;
    const o = v as Record<string, unknown>;
    const code = o.code;
    const message = o.message;
    return typeof code === 'string' && typeof message === 'string' ? { code, message } : null;
  } catch {
    return null;
  }
}

/** The end of the session's newest wait, from its timeline (newest first):
 *  the first wait-related event decides — an end is the answer, a
 *  `session_move_waiting` means that wait is still recorded as pending.
 *  The list crossed IPC, so anything that is not one reads as "no end". */
function recordedWaitEnd(events: unknown): SessionEvent | null {
  if (!Array.isArray(events)) return null;
  for (const e of events as unknown[]) {
    if (typeof e !== 'object' || e === null) continue;
    const kind = (e as SessionEvent).kind;
    if (kind === 'session_move_wait_ended') return e as SessionEvent;
    if (kind === 'session_move_waiting') return null;
  }
  return null;
}

/**
 * Settle a run from its wait's `session_move_wait_ended`, however that event
 * reached this window (the live timeline, or a re-read of it). A no-op for a
 * run no longer following a wait — `move:progress` already settled it, so a
 * late `moved` never settles anything twice.
 *
 * `moved` settles `done`: the waiter's move finished even if none of its
 * `move:progress` reached this window. There is no report to show — the
 * sheet says so. Every other reason settles `failed` with that reason; an
 * unusable detail settles as `UNKNOWN_WAIT_END` rather than leaving the run
 * waiting for an end that has already come.
 */
function settleWaitEnd(sessionId: number, e: SessionEvent): void {
  const run = get(store).get(sessionId);
  if (!run || !waitOpen(run)) return;
  unwatchWait(sessionId);
  const reason = waitEndReason(e.detail) ?? UNKNOWN_WAIT_END;
  if (reason === 'moved') {
    put({
      ...run,
      status: 'done',
      waitEnded: 'moved',
      waitRefusal: null,
      error: null,
      awaitingStart: false,
      settledAt: Date.now(),
    });
    if (run.origin === 'local' && get(transferSheetFor) !== sessionId) {
      push({
        kind: 'success',
        message: `Moved ${run.sessionName} to ${run.toHost}`,
        action: { label: 'View', run: () => transferSheetFor.set(sessionId) },
      });
    }
    return;
  }
  // Only a `refused` end ever carries a refusal.
  const waitRefusal = reason === 'refused' ? waitEndRefusal(e.detail) : null;
  put({ ...run, status: 'failed', waitEnded: reason, waitRefusal, error: null, settledAt: Date.now() });
}

/**
 * Subscribe a run to its own session's live timeline for the end of its
 * wait — nothing else tells this window that a wait ended without a move,
 * and `move:progress` may never arrive for one that ended with a move.
 * Idempotent — a run already watched (an adopted one, a straggler retry) is
 * not subscribed twice.
 */
function watchWait(sessionId: number): void {
  if (waitWatchers.has(sessionId)) return;
  const unsub = onTimelineEvent(sessionId, (e: SessionEvent) => {
    if (e.kind === 'session_move_wait_ended') settleWaitEnd(sessionId, e);
  });
  waitWatchers.set(sessionId, unsub);
}

/**
 * Settle a `waiting` answer. The answer and `move:progress` are two streams,
 * and the backend may have emitted progress for this session before the
 * answer arrived: a stale-idle source's own failed check (`check:started`,
 * `check:failed`, then `Waiting`), or even the waiter's first real attempt.
 * So the step list decides: nothing past `check` means the run is parked at
 * `waiting` on a clean list; anything past it is a real move under way,
 * followed from its events like any move this window did not start — the
 * call that would have settled it has already answered.
 */
function settleWaiting(sessionId: number, w: MoveWaiting): void {
  const run = get(store).get(sessionId);
  if (!run || run.origin !== 'local' || run.status !== 'running') return;
  watchWait(sessionId);
  const base = { ...run, deadlineUnix: w.deadline_unix, awaitingStart: false, waitEnded: null, waitRefusal: null };
  const underWay = run.steps.some((s, i) => i > 0 && s.state !== 'pending');
  if (!underWay) {
    put({ ...base, status: 'waiting', steps: blank() });
    return;
  }
  const ended = endedInTheSteps(run.steps);
  if (ended !== null) {
    unwatchWait(sessionId);
    put({ ...base, origin: 'observed', status: ended, settledAt: Date.now() });
    return;
  }
  put({ ...base, origin: 'observed', status: 'running' });
}

/**
 * Re-read the timeline of every run following a wait, and settle each whose
 * wait the timeline says has ended. The live `session:event` push is the
 * only other way this window hears of an end, so one missed while the hub
 * link was down (or the window asleep) would otherwise leave the run
 * waiting for ever. Called when the hub link comes back and on window focus.
 */
export async function recheckWaitingRuns(): Promise<void> {
  const ids = [...get(store).values()]
    .filter((r) => r.status === 'waiting' || (r.status === 'running' && waitWatchers.has(r.sessionId)))
    .map((r) => r.sessionId);
  await Promise.all(
    ids.map(async (id) => {
      const h = await sessionHistory(id);
      if (!h.ok) return;
      const end = recordedWaitEnd(h.value);
      if (end) settleWaitEnd(id, end);
    }),
  );
}

// A hub link that comes back (`reconnecting`/`offline`/`connecting` ->
// `connected`) may have dropped a wait's end on the floor: re-check.
let lastLink: HubConnection['state'] | null = null;
hubConnection.subscribe((c) => {
  const prev = lastLink;
  lastLink = c.state;
  if (c.state === 'connected' && prev !== null && prev !== 'connected') void recheckWaitingRuns();
});

/**
 * Settle what `startMove`/`retryMove`'s own `move_session` call answered: a
 * `moved` outcome exactly as before, a `waiting` one parks the run at
 * `status: 'waiting'` instead (Task 7) — never reported as done, and never
 * settled as a failure the way a stray one used to be, back when both call
 * sites always asked for `when: 'now'`.
 */
function settleMoveResult(
  sessionId: number,
  r: Result<({ kind: 'moved' } & MoveReport) | ({ kind: 'waiting' } & MoveWaiting)>,
): void {
  if (!r.ok) {
    settle(sessionId, r);
    return;
  }
  if (r.value.kind === 'waiting') {
    settleWaiting(sessionId, r.value);
    return;
  }
  settle(sessionId, { ok: true, value: r.value });
}

/**
 * Run the same move again, on the same run entry. Only for a run that FAILED:
 * a running one is already going, and a partial needs `resolveMoveRun` — a
 * second `move_session` there would build a second target.
 */
export function retryMove(sessionId: number, opts: { cleanTarget?: boolean } = {}): void {
  const run = get(store).get(sessionId);
  if (!run || run.status !== 'failed' || run.origin !== 'local') return;
  const cleanTarget = opts.cleanTarget ?? false;
  unwatchWait(sessionId);
  put({
    ...run,
    steps: blank(),
    status: 'running',
    report: null,
    error: null,
    resolveError: null,
    cleanTarget,
    attempt: run.attempt + 1,
    resolving: false,
    startedAt: Date.now(),
    settledAt: null,
    // See the field comment on `MoveRun.awaitingStart`: a straggler from the
    // attempt this run replaces must not land on the fresh step list.
    awaitingStart: true,
    deadlineUnix: null,
    waitEnded: null,
    waitRefusal: null,
  });
  void moveSession(sessionId, run.toHost, {
    // `null` only for a run this window never started (an `observed` one —
    // which `retryMove` refuses above) or a recorded partial whose event
    // predates `kept_source`. Nothing better is knowable there; every run
    // that does know carries the answer (`adoptPartial`).
    keepSource: run.keepSource ?? false,
    cleanTarget,
    // Same as `startMove`: a source busy again on retry waits instead of
    // refusing outright, and a `waiting` answer parks this same run there.
    when: 'idle',
  }).then((r) => settleMoveResult(sessionId, r));
}

/** The TARGET session's id `resolve_move` needs, from whichever of the run's
 *  two places still names it: a finished report, or a partial error's
 *  details. `null` when neither does — there is nothing to act on. */
function targetIdOf(run: MoveRun): number | null {
  if (run.report) return run.report.target_session_id;
  const details = run.error?.details;
  if (typeof details === 'object' && details !== null) {
    const v = (details as Record<string, unknown>).target_session_id;
    if (typeof v === 'number') return v;
  }
  return null;
}

function settleResolve(
  sessionId: number,
  sessionName: string,
  action: ResolveAction,
  r: Result<ResolveMoveReport>,
): void {
  const run = get(store).get(sessionId);
  if (!run) {
    // The run is gone — the user closed the sheet and dismissed it (or a
    // dismissible status raced this call) before Finish/Undo came back. There
    // is no sheet left to carry `resolveError` on, and Finish/Undo each kill
    // a live session, so silence here would let the user believe it worked
    // when it did not: fall back to the toast. Do NOT make this path also
    // fire when `run` exists (below) — that is the sheet's job now (fix round
    // 1, finding 1), and doubling up would put the same refusal in both
    // places.
    if (!r.ok) pushError(r.error, `Resolving the transfer of ${sessionName}`);
    return;
  }
  if (!r.ok) {
    // Shown in the sheet, not as a toast (fix round 1, finding 1): the user
    // is looking straight at it — they just clicked Finish/Undo and are still
    // on the confirm they clicked through. A toast here would be the wrong
    // place, and would say nothing the sheet cannot say better in context.
    // The guard is released with it: a refusal leaves the run `partial`, and
    // the user may try the other action.
    put({ ...run, resolveError: r.error, resolving: false });
    return;
  }
  if (action === 'finish') {
    put({ ...run, status: 'done', resolveError: null, resolving: false, settledAt: Date.now() });
    return;
  }
  put({
    ...run,
    status: 'failed',
    resolving: false,
    error: {
      code: UNDONE,
      message: `The new session on ${run.toHost} was killed; ${run.sessionName} keeps running on ${run.fromHost}.`,
    },
    resolveError: null,
    settledAt: Date.now(),
  });
}

/**
 * Finish or undo a partial move (`E_MOVE_PARTIAL`). `sessionId` is the run's
 * key — the SOURCE session's id — but `resolve_move` itself takes the
 * TARGET's id, which `targetIdOf` finds in the run. Nothing to act on is a
 * toast, not a call.
 */
export function resolveMoveRun(sessionId: number, action: ResolveAction): void {
  const run = get(store).get(sessionId);
  // Finish/undo only mean something for a partial: on any other status the
  // backend would answer E_INVALID_STATE, so refuse the round-trip here,
  // mirroring retryMove's own status guard.
  if (!run || run.status !== 'partial') return;
  // One resolve at a time: both actions kill a live session, and the confirm
  // button stays on screen while the call is out, so a double click would
  // otherwise send two.
  if (run.resolving) return;
  const targetId = targetIdOf(run);
  if (targetId === null) {
    push({ kind: 'error', message: `${run.sessionName}: no target session to resolve.` });
    return;
  }
  // A stale refusal from a previous attempt must not linger through this one.
  put({ ...run, resolveError: null, resolving: true });
  const sessionName = run.sessionName;
  void resolveMove(targetId, action).then((r) => settleResolve(sessionId, sessionName, action, r));
}

/**
 * Rebuild a `partial` run from a recorded `session_move_partial` event, so
 * the sheet can offer Finish/Undo for a move this window never saw — usually
 * because the app was restarted before anyone resolved it. Keyed like a live
 * partial: the source id when known, the target id otherwise. A no-op when a
 * run already exists for that key — a live run is always the better picture.
 */
export function adoptPartial(p: UnresolvedPartial, sessionName: string): void {
  const key = p.sourceSessionId ?? p.targetSessionId;
  if (get(store).has(key)) return;
  const cutoff = MOVE_STEPS.indexOf('start');
  const steps: MoveRunStep[] = MOVE_STEPS.map((step, i) => ({
    step,
    state: (i <= cutoff ? 'done' : 'pending') as StepState,
    detail: null,
  }));
  put({
    sessionId: key,
    sessionName,
    fromHost: p.fromHost,
    toHost: p.toHost,
    // What the transfer was told to do with the source, as the event
    // recorded it — `null` when it did not say. A retry from this run must
    // not turn "leave the source running" into "kill it" (see `retryMove`).
    keepSource: p.keptSource,
    origin: 'local',
    steps,
    status: 'partial',
    report: null,
    error: {
      code: 'E_MOVE_PARTIAL',
      message: '',
      details: { step: p.step, target_session_id: p.targetSessionId, target_host: p.toHost },
    },
    resolveError: null,
    cleanTarget: false,
    attempt: 1,
    resolving: false,
    awaitingStart: false,
    startedAt: Date.now(),
    settledAt: Date.now(),
    deadlineUnix: null,
    waitEnded: null,
    waitRefusal: null,
  });
}

/**
 * Rebuild a `waiting` run from a recorded `session_move_waiting` event with
 * no later `session_move_wait_ended`, so the sheet can offer Cancel — and the
 * chip its deadline — for a wait this window never saw, usually because the
 * app was restarted while it was still pending. Keyed like a live wait: the
 * source session's id. A no-op when a run already exists for that key — a
 * live run is always the better picture.
 */
export function adoptWait(w: UnresolvedWait, sessionName: string): void {
  if (get(store).has(w.sessionId)) return;
  const row = get(sessions).find((s) => s.id === w.sessionId);
  put({
    sessionId: w.sessionId,
    sessionName,
    fromHost: row?.host_alias ?? '',
    toHost: w.toHost,
    // Not knowable from the wait's own event (`begin_wait` does not record
    // it) — same "unknown, not false" reasoning as `adoptPartial`'s `keptSource`.
    keepSource: null,
    origin: 'local',
    steps: blank(),
    status: 'waiting',
    report: null,
    error: null,
    resolveError: null,
    cleanTarget: false,
    attempt: 1,
    resolving: false,
    awaitingStart: false,
    startedAt: Date.now(),
    settledAt: null,
    deadlineUnix: w.deadlineUnix,
    waitEnded: null,
    waitRefusal: null,
  });
  watchWait(w.sessionId);
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
    resolveError: null,
    startedAt: Date.now(),
    settledAt: null,
    cleanTarget: false,
    attempt: 1,
    resolving: false,
    awaitingStart: false,
    deadlineUnix: null,
    waitEnded: null,
    waitRefusal: null,
  };
}

/**
 * Patch a run from one `move:progress` event.
 *
 * The payload crossed an IPC boundary from a backend that may be a version
 * ahead, so every field this reads is checked before anything is touched: a
 * malformed event changes nothing, and nothing here throws — a throw would
 * be inside the event batch and would lose the whole batch with it.
 */
export function applyMoveProgress(p: MoveProgress): void {
  if (typeof p !== 'object' || p === null) return;
  if (typeof p.session_id !== 'number' || typeof p.to_host !== 'string') return;
  if (!Number.isInteger(p.index)) return;
  const i = p.index - 1;
  if (i < 0 || i >= MOVE_STEPS.length || MOVE_STEPS[i] !== p.step) return;
  if (!STATES.has(p.state)) return;
  const detail = typeof p.detail === 'string' ? p.detail.slice(0, DETAIL_MAX) : null;

  let run = get(store).get(p.session_id);
  const isFreshStart = i === 0 && p.state === 'started';

  // See the field comment on `MoveRun.awaitingStart`: `retryMove` reset this
  // run's steps and put it back to `running` before the retry's own events
  // exist, so nothing here can tell a straggler from the replaced attempt
  // apart from the retry's real first step except waiting for that step by
  // name. Everything else is dropped until it arrives.
  if (run?.awaitingStart) {
    if (!isFreshStart) return;
    run = { ...run, awaitingStart: false };
  }

  let settledLocal = false;
  if (run && waitOpen(run)) {
    // An attempt of the waiter's own move. `check` failing is the source
    // found busy again (`run_wait` goes back to waiting); any other way the
    // check fails ends the wait with a `refused` `session_move_wait_ended`,
    // which the still-live watcher settles from. Either way the run is
    // parked at `waiting` again, deadline and Cancel intact, until that end
    // (if any) arrives.
    if (i === 0 && p.state === 'failed') {
      if (run.status !== 'waiting' || run.origin !== 'local') {
        put({ ...run, origin: 'local', status: 'waiting', steps: blank(), error: null, settledAt: null, waitEnded: null });
      }
      return;
    }
    if (run.status !== 'running') {
      // A new attempt begins — by its `check:started`, or by a later step if
      // that one was lost. The origin flips local -> observed on purpose: the
      // run's own `move_session` call already answered `Waiting`, so this
      // move has no local caller left to settle it — only its events, or the
      // wait's end. The watcher stays until one of those settles it.
      run = {
        ...run,
        origin: 'observed',
        status: 'running',
        steps: blank(),
        error: null,
        settledAt: null,
        waitEnded: null,
        waitRefusal: null,
        startedAt: Date.now(),
      };
    }
  } else if (run && run.status !== 'running') {
    if (
      run.origin === 'local' &&
      run.settledAt !== null &&
      Date.now() - run.settledAt < SETTLE_GRACE_MS
    ) {
      // Its own events, still arriving. They may name the step that failed;
      // they may not touch the outcome the user is already reading.
      settledLocal = true;
    } else if (isFreshStart) {
      run = undefined; // a NEW move of this session
    } else {
      return;
    }
  }
  run ??= observed(p);
  const steps = run.steps.map((s, n) => {
    if (n < i) return RANK[s.state] < 2 ? { ...s, state: 'done' as StepState } : s;
    if (n > i || RANK[p.state] <= RANK[s.state]) return s;
    return { ...s, state: p.state, detail };
  });
  if (settledLocal) {
    put({ ...run, steps });
    return;
  }
  let status: MoveStatus = run.status;
  if (run.origin === 'observed') {
    if (p.state === 'failed') status = 'failed';
    else if (i === MOVE_STEPS.length - 1 && (p.state === 'done' || p.state === 'warned')) status = 'done';
  }
  if (status !== 'running') unwatchWait(run.sessionId);
  put({ ...run, steps, status });
}

/**
 * End a pending wait. Only `move_session`'s own `when: 'cancel'` decides how
 * this settles — the answer's `was_waiting` tells this window whether there
 * was still anything to cancel, but it never settles the run itself: however
 * the cancel lands, the backend records its own `session_move_wait_ended`,
 * which the run's `watchWait` subscription (still active — a cancel does not
 * touch it) settles from, exactly like any other way a wait can end.
 */
export function cancelWait(sessionId: number): void {
  const run = get(store).get(sessionId);
  if (!run || run.status !== 'waiting') return;
  const sessionName = run.sessionName;
  void cancelMoveWait(sessionId, run.toHost).then(async (r: Result<MoveWaitCancelled>) => {
    if (!r.ok) {
      pushError(r.error, `Cancelling the transfer of ${sessionName}`);
      return;
    }
    if (r.value.was_waiting) return;
    // Nothing was pending any more: the wait ended on its own (moved, timed
    // out, …), or the waiter had already started the move. Say so rather
    // than claim credit — and do not leave the run waiting on the hope that
    // the end event still arrives: it may already have been missed.
    push({
      kind: 'info',
      message: `Nothing was left to cancel for ${sessionName} — the wait had already ended, or its move had started.`,
    });
    await settleAfterNothingToCancel(sessionId);
  });
}

/** I1(a): after a Cancel answered `was_waiting: false`. A run its move's
 *  progress already took over is left to that progress; one the timeline
 *  records an end for is settled from it; anything else is over for
 *  reasons this window cannot know, and says exactly that
 *  (`UNKNOWN_WAIT_END`) — the watcher stays, so a real end still names it,
 *  and a resumed wait's next attempt still revives it. */
async function settleAfterNothingToCancel(sessionId: number): Promise<void> {
  const h = await sessionHistory(sessionId);
  const run = get(store).get(sessionId);
  if (!run || run.status !== 'waiting') return;
  const end = h.ok ? recordedWaitEnd(h.value) : null;
  if (end) {
    settleWaitEnd(sessionId, end);
    return;
  }
  put({ ...run, status: 'failed', waitEnded: UNKNOWN_WAIT_END, waitRefusal: null, error: null, settledAt: Date.now() });
}

/** Whether a `waiting` run can no longer be shown to be pending: its
 *  deadline has passed (the wait has ended, whether or not this window heard
 *  of it), it never had one, or nothing is watching for its end. Such a run
 *  may be dismissed — otherwise a missed end would block every later
 *  Transfer of the session. */
export function waitIsStale(run: MoveRun, nowMs: number = Date.now()): boolean {
  if (run.status !== 'waiting') return false;
  if (run.deadlineUnix === null || nowMs / 1000 >= run.deadlineUnix) return true;
  return !waitWatchers.has(run.sessionId);
}

/**
 * Forget a run. A LOCAL running one cannot be dismissed — this window owns it
 * and its result is still coming; nor can a waiting one while its end is
 * still expected (`waitIsStale`). An observed run is only this window's view
 * of someone else's move, so it can always be let go of; the next event
 * re-creates it.
 */
export function dismissMove(sessionId: number): void {
  const run = get(store).get(sessionId);
  if (run && run.origin === 'local' && run.status === 'running') return;
  if (run && run.status === 'waiting' && !waitIsStale(run)) return;
  unwatchWait(sessionId);
  store.update((m) => {
    const next = new Map(m);
    next.delete(sessionId);
    return next;
  });
}

export function resetMovesForTest(): void {
  store.set(new Map());
  transferSheetFor.set(null);
  for (const unsub of waitWatchers.values()) unsub();
  waitWatchers.clear();
}

/** Test-only: inject an arbitrary run directly, bypassing the state machine —
 *  the Transfer sheet's failure-view tests need a `failed`/`done`/`partial`
 *  run with a specific error or report in place *before* the first render,
 *  which the public start/retry/resolve API cannot do synchronously. */
export function putRunForTest(run: MoveRun): void {
  put(run);
}
