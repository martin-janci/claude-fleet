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
