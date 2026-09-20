// One run per moving session: what the Transfer sheet and the header chip
// render. A run started in this window ('local') is settled by the command's
// result; one seen only through `move:progress` events ('observed' — another
// window, the MCP API, a hub client) is settled by its events. Nothing here
// is persisted: the session timeline already records the move.
import { get, writable, type Readable } from 'svelte/store';
import { UNDONE } from './moveErrors';
import { MOVE_STEPS, type MoveProgress, type MoveStep, type MoveStepState } from './moveProgress';
import { moveSession, resolveMove, type MoveReport, type ResolveAction, type ResolveMoveReport } from './moveSession';
import type { IpcError, Result } from './result';
import { sessions, type SessionRow } from './sessions';
import { selectedSession, selectSession } from './selection';
import type { UnresolvedPartial } from './timeline';
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
  /**
   * True from the moment `retryMove` resets this run until its own first
   * `move:progress` event (`check`/`started`) arrives; every other event is
   * dropped while it is true.
   *
   * `move:progress` carries only the session id, not a per-move id, so a
   * straggler from the attempt this run replaced looks exactly like this
   * attempt's own next step: `applyMoveProgress` would otherwise apply it
   * straight onto the fresh (blank) step list, since `retryMove` already put
   * `status: 'running'` and `settledAt: null` — the very shape that lets
   * ordinary events through. A per-move id on the wire would make this flag
   * unnecessary; that is backend work tracked separately, not part of this
   * task.
   */
  awaitingStart: boolean;
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

/** The code the hub client answers with when the hub said nothing at all
 *  (no connection, a timed-out exchange, a proxy's 5xx). The move is very
 *  probably still running there, so it is not a failure. */
const NO_ANSWER = 'E_HUB_UNREACHABLE';

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
  if (run.status === 'running' || run.steps.some((s) => s.state === 'failed')) return run.steps;
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
  }
  return undefined;
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
    resolveError: null,
    startedAt: Date.now(),
    settledAt: null,
    cleanTarget: false,
    attempt: 1,
    awaitingStart: false,
  });
  void moveSession(session.id, toHost, { keepSource: opts.keepSource }).then((r) =>
    settle(session.id, r),
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
  if (r.error.code === NO_ANSWER) {
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
  put({
    ...run,
    status: r.error.code === 'E_MOVE_PARTIAL' ? 'partial' : 'failed',
    error: r.error,
    settledAt: Date.now(),
  });
  if (!sheetOpen) pushError(r.error, `Move of ${run.sessionName} failed`);
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
  put({
    ...run,
    steps: blank(),
    status: 'running',
    report: null,
    error: null,
    resolveError: null,
    cleanTarget,
    attempt: run.attempt + 1,
    startedAt: Date.now(),
    settledAt: null,
    // See the field comment on `MoveRun.awaitingStart`: a straggler from the
    // attempt this run replaces must not land on the fresh step list.
    awaitingStart: true,
  });
  void moveSession(sessionId, run.toHost, {
    keepSource: run.keepSource ?? false,
    cleanTarget,
  }).then((r) => settle(sessionId, r));
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

function settleResolve(sessionId: number, action: ResolveAction, r: Result<ResolveMoveReport>): void {
  const run = get(store).get(sessionId);
  if (!run) return;
  if (!r.ok) {
    // Shown in the sheet, not as a toast (fix round 1, finding 1): the user
    // is looking straight at it — they just clicked Finish/Undo and are still
    // on the confirm they clicked through. A toast here would be the wrong
    // place, and would say nothing the sheet cannot say better in context.
    put({ ...run, resolveError: r.error });
    return;
  }
  if (action === 'finish') {
    put({ ...run, status: 'done', resolveError: null, settledAt: Date.now() });
    return;
  }
  put({
    ...run,
    status: 'failed',
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
  const targetId = targetIdOf(run);
  if (targetId === null) {
    push({ kind: 'error', message: `${run.sessionName}: no target session to resolve.` });
    return;
  }
  // A stale refusal from a previous attempt must not linger through this one.
  if (run.resolveError) put({ ...run, resolveError: null });
  void resolveMove(targetId, action).then((r) => settleResolve(sessionId, action, r));
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
    keepSource: null,
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
    awaitingStart: false,
    startedAt: Date.now(),
    settledAt: Date.now(),
  });
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
    awaitingStart: false,
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
  if (run && run.status !== 'running') {
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
  put({ ...run, steps, status });
}

/**
 * Forget a run. A LOCAL running one cannot be dismissed — this window owns
 * it and its result is still coming. An observed run is only this window's
 * view of someone else's move, so it can always be let go of; the next event
 * re-creates it.
 */
export function dismissMove(sessionId: number): void {
  const run = get(store).get(sessionId);
  if (run && run.origin === 'local' && run.status === 'running') return;
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

/** Test-only: inject an arbitrary run directly, bypassing the state machine —
 *  the Transfer sheet's failure-view tests need a `failed`/`done`/`partial`
 *  run with a specific error or report in place *before* the first render,
 *  which the public start/retry/resolve API cannot do synchronously. */
export function putRunForTest(run: MoveRun): void {
  put(run);
}
