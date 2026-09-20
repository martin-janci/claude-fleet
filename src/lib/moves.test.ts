import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('./result', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./result')>();
  return { ...actual, invokeCmd: vi.fn() };
});
import { invokeCmd, type Result } from './result';
import {
  moves, transferSheetFor, startMove, applyMoveProgress, dismissMove,
  activeMoveFor, stepNumber, displaySteps, runForSession, resetMovesForTest,
  SETTLE_GRACE_MS, retryMove, resolveMoveRun, adoptPartial,
} from './moves';
import type { MoveProgress, MoveStep, MoveStepState } from './moveProgress';
import { MOVE_STEPS } from './moveProgress';
import type { MoveReport } from './moveSession';
import { sessions, type SessionRow } from './sessions';
import { selectSession, selectedSession } from './selection';
import { toasts, clearToasts } from './toasts';

const invoked = invokeCmd as ReturnType<typeof vi.fn>;

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

/** The report a completed `move_session` (or `retryMove`) settles the run
 *  with. A function, like `row`, so a test can override just what it cares
 *  about. */
function report(over: Partial<MoveReport> = {}): MoveReport {
  return {
    source_session_id: 5, target_session_id: 6, from_host: 'mefistos', to_host: 'turanga',
    tmux_name: 'dev-foo', claude_session_id: source.claude_session_id!, branch: 'feat',
    target_cwd: '/r/.claude/worktrees/feat', transcript_bytes: 10, source_killed: true,
    warnings: [],
    carried: {
      commits: 0, bundle_bytes: 0, dirty_entries: [], ignored_carried: [],
      ignored_left_behind: [], target_seeded: 'existing',
      session_state: { carried: [], kept_target: [], left_behind: [] },
      memory: { carried: [], kept_target: [], identical: 0, index_lines_added: 0, left_behind: [] },
    },
    target,
    ...over,
  };
}

/** What `invokeCmd` resolves to on success / on failure — it never rejects,
 *  it always answers with a `Result`. */
function ok<T>(value: T): Result<T> {
  return { ok: true, value };
}
function err<T = never>(code: string, message: string, details?: unknown): Result<T> {
  return { ok: false, error: { code, message, details } };
}

const ev = (step: MoveStep, state: MoveStepState, over: Partial<MoveProgress> = {}): MoveProgress => ({
  session_id: 5, to_host: 'turanga', step, index: MOVE_STEPS.indexOf(step) + 1,
  total: 9, state, detail: null, ...over,
});

const states = (id = 5) => get(moves).get(id)!.steps.map((s) => s.state);

/** A `move_session` call the test resolves by hand. */
function pending() {
  let doResolve!: (v: unknown) => void;
  invoked.mockImplementation(
    (cmd: string) =>
      cmd === 'move_session'
        ? new Promise((res) => { doResolve = res; })
        : Promise.resolve(ok(undefined)),
  );
  return {
    resolve: (v: unknown) => doResolve(ok(v)),
    reject: (e: { code: string; message: string; details?: unknown }) =>
      doResolve(err(e.code, e.message, e.details)),
  };
}
const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  invoked.mockReset();
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
      cleanTarget: false, attempt: 1,
    });
    expect(run.steps.map((s) => s.step)).toEqual([...MOVE_STEPS]);
    expect(states()).toEqual(Array(9).fill('pending'));
    expect(activeMoveFor(5)).toBe(run);
    expect(invoked).toHaveBeenCalledWith('move_session', {
      args: {
        session_id: 5, target_host_alias: 'turanga', keep_source: false, strict: false,
        clean_target: false,
      },
    });
  });

  it('refuses a second start while one is running', () => {
    pending();
    startMove(source, 'turanga', { keepSource: false });
    startMove(source, 'turanga', { keepSource: true });
    expect(invoked.mock.calls.filter((c) => c[0] === 'move_session')).toHaveLength(1);
    expect(get(moves).get(5)!.keepSource).toBe(false);
  });

  it('settles done from the result, keeps warned steps, and follows the selection', async () => {
    const p = pending();
    selectSession(source);
    transferSheetFor.set(5);
    startMove(source, 'turanga', { keepSource: false });
    applyMoveProgress(ev('ignored', 'warned', { detail: '0 files' }));
    p.resolve(report());
    await flush();
    const run = get(moves).get(5)!;
    expect(run.status).toBe('done');
    expect(run.report?.target_session_id).toBe(6);
    expect(displaySteps(run).map((s) => s.state))
      .toEqual(['done', 'done', 'done', 'done', 'done', 'warned', 'done', 'done', 'done']);
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
    p.resolve(report());
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
    expect(displaySteps(run).map((s) => s.state))
      .toEqual(['done', 'done', 'done', 'failed', 'pending', 'pending', 'pending', 'pending', 'pending']);
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

// F7 + P-T3: the payload crosses an IPC boundary from a hub that may be a
// version ahead, and `applyMoveProgress` indexes an array with `index` and
// keys the run map with `session_id`. One malformed event must change
// nothing and must never throw — a throw inside the event batch loses the
// whole batch.
describe('applyMoveProgress validates the event', () => {
  const started = () => {
    pending();
    startMove(source, 'turanga', { keepSource: false });
  };

  it('drops an event whose state is not one of the four', () => {
    started();
    applyMoveProgress(ev('git', 'done'));
    applyMoveProgress(ev('git', 'bogus' as never));
    applyMoveProgress(ev('git', 'pending' as never));
    expect(states()).toEqual(['done', 'done', 'done', 'done', 'pending', 'pending', 'pending', 'pending', 'pending']);
  });

  it('never lets a `pending` state fill the gap before a step', () => {
    started();
    applyMoveProgress(ev('claude_state', 'pending' as never));
    expect(states()).toEqual(Array(9).fill('pending'));
  });

  it('stores null for a detail that is not a string, without throwing', () => {
    started();
    expect(() => applyMoveProgress(ev('git', 'done', { detail: undefined as never }))).not.toThrow();
    expect(get(moves).get(5)!.steps[3].detail).toBeNull();
    expect(() => applyMoveProgress(ev('replay', 'done', { detail: 5 as never }))).not.toThrow();
    expect(get(moves).get(5)!.steps[4].detail).toBeNull();
  });

  it('drops an event whose session_id or to_host has the wrong type', () => {
    applyMoveProgress(ev('check', 'started', { session_id: '5' as never }));
    applyMoveProgress(ev('check', 'started', { session_id: 7, to_host: null as never }));
    expect(get(moves).size).toBe(0);
  });

  it('never throws on a payload that is not an object at all', () => {
    expect(() => applyMoveProgress(null as never)).not.toThrow();
    expect(() => applyMoveProgress(undefined as never)).not.toThrow();
    expect(get(moves).size).toBe(0);
  });
});

// F2: settling used to rewrite the step list, so a result that arrived
// before the events it raced left the sheet showing the wrong step — or, for
// a failure, the step BEFORE the one that actually failed.
describe('displaySteps and the settle grace', () => {
  it('shows step 1 failed when the result beat every event, and keeps the real error', async () => {
    vi.useFakeTimers();
    try {
      const p = pending();
      startMove(source, 'turanga', { keepSource: false });
      p.reject({ code: 'E_MOVE_CARRY', message: 'x', details: { step: 'seed' } });
      await vi.advanceTimersByTimeAsync(0);
      expect(displaySteps(get(moves).get(5)!).map((s) => s.state)[0]).toBe('failed');
      applyMoveProgress(ev('check', 'started'));
      applyMoveProgress(ev('check', 'failed'));
      const run = get(moves).get(5)!;
      expect(run.origin).toBe('local');
      expect(run.status).toBe('failed');
      expect(run.error?.code).toBe('E_MOVE_CARRY');
      expect(displaySteps(run).map((s) => s.state)[0]).toBe('failed');
    } finally {
      vi.useRealTimers();
    }
  });

  it('lets queued events name the step that failed, without touching the result', async () => {
    vi.useFakeTimers();
    try {
      const p = pending();
      startMove(source, 'turanga', { keepSource: false });
      p.reject({ code: 'E_MOVE_CARRY', message: 'x', details: { step: 'fetch' } });
      await vi.advanceTimersByTimeAsync(0);
      applyMoveProgress(ev('git', 'started'));
      applyMoveProgress(ev('git', 'failed'));
      const run = get(moves).get(5)!;
      expect(run.status).toBe('failed');
      expect(run.error?.code).toBe('E_MOVE_CARRY');
      expect(displaySteps(run).map((s) => s.state)).toEqual([
        'done', 'done', 'done', 'failed', 'pending', 'pending', 'pending', 'pending', 'pending',
      ]);
    } finally {
      vi.useRealTimers();
    }
  });

  it('never marks a step the user watched finish as the one that failed', async () => {
    vi.useFakeTimers();
    try {
      const p = pending();
      startMove(source, 'turanga', { keepSource: false });
      applyMoveProgress(ev('git', 'done', { detail: '2 commits' }));
      p.reject({ code: 'E_MOVE_CARRY', message: 'x', details: { step: 'target' } });
      await vi.advanceTimersByTimeAsync(0);
      // Nothing is `started`, and the first four steps are known to be done:
      // the failure belongs to the step the move had got to.
      expect(displaySteps(get(moves).get(5)!).map((s) => s.state)).toEqual([
        'done', 'done', 'done', 'done', 'failed', 'pending', 'pending', 'pending', 'pending',
      ]);
    } finally {
      vi.useRealTimers();
    }
  });

  it('shows every step done for a result with no events at all, keeping warned', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    applyMoveProgress(ev('ignored', 'warned', { detail: '0 files' }));
    p.resolve(report());
    await flush();
    const run = get(moves).get(5)!;
    // The stored steps are left alone; only what the sheet renders is filled in.
    expect(run.steps[8].state).toBe('pending');
    expect(displaySteps(run).map((s) => s.state)).toEqual([
      'done', 'done', 'done', 'done', 'done', 'warned', 'done', 'done', 'done',
    ]);
  });

  it('replaces a settled local run with a fresh observed one once the grace is over', async () => {
    vi.useFakeTimers();
    try {
      const p = pending();
      startMove(source, 'turanga', { keepSource: false });
      p.resolve(report());
      await vi.advanceTimersByTimeAsync(0);
      expect(get(moves).get(5)!.status).toBe('done');
      vi.setSystemTime(Date.now() + SETTLE_GRACE_MS + 1000);
      applyMoveProgress(ev('check', 'started'));
      expect(get(moves).get(5)).toMatchObject({ status: 'running', origin: 'observed' });
    } finally {
      vi.useRealTimers();
    }
  });
});

// F1: the hub answered nothing. The move is very probably still running
// there, so calling it failed is a lie — and the user must still be able to
// stop following it (F3).
describe('a lost hub connection', () => {
  const lost = { code: 'E_HUB_UNREACHABLE', message: 'hub did not answer', details: null };

  it('keeps the run running as an observed one, with no error toast', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    p.reject(lost);
    await flush();
    const run = get(moves).get(5)!;
    expect(run.status).toBe('running');
    expect(run.origin).toBe('observed');
    expect(run.error?.code).toBe('E_HUB_UNREACHABLE');
    expect(get(toasts).some((t) => t.kind === 'error')).toBe(false);
    // From here events settle it like any other observed run.
    applyMoveProgress(ev('handoff', 'done'));
    expect(get(moves).get(5)!.status).toBe('done');
  });

  // m4: the outcome is only unknown while the events have not said it. If
  // they already did, following a move that is over is the wrong thing.
  it('is not "unknown" when the events already reported a failed step', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    applyMoveProgress(ev('git', 'started'));
    applyMoveProgress(ev('git', 'failed'));
    p.reject(lost);
    await flush();
    const run = get(moves).get(5)!;
    expect(run.status).toBe('failed');
    expect(run.error?.code).toBe('E_HUB_UNREACHABLE');
    expect(activeMoveFor(5)).toBeUndefined();
  });

  it('is not "unknown" when the events already reported the last step done', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    applyMoveProgress(ev('handoff', 'warned', { detail: '1 file' }));
    p.reject(lost);
    await flush();
    const run = get(moves).get(5)!;
    expect(run.status).toBe('done');
    expect(run.report).toBeNull();
    expect(activeMoveFor(5)).toBeUndefined();
  });

  it('is still "unknown" in the middle of the move', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    applyMoveProgress(ev('git', 'started'));
    p.reject(lost);
    await flush();
    expect(get(moves).get(5)!.status).toBe('running');
  });
});

// R2: this window's Transfer was refused because ANOTHER move of the same
// session is already running (reachable after Stop following, or after a
// lost-hub run was dismissed, then Transfer again). The refusal is about the
// click, not about the move — settling a failed run would show a step failing
// that never did, and would then be patched by the REAL move's events.
describe('a Transfer refused because a move is already running', () => {
  const busy = {
    code: 'E_INVALID_STATE',
    message: 'a move of session 5 is already in progress',
    details: null,
  };

  it('follows the real move instead of inventing a failure, and says why', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    p.reject(busy);
    await flush();
    const run = get(moves).get(5)!;
    expect(run).toMatchObject({
      status: 'running',
      origin: 'observed',
      error: null,
      keepSource: null,
      settledAt: null,
    });
    expect(get(toasts).some((t) => t.kind === 'error' && t.message.includes('already in progress')))
      .toBe(true);
    // The real move's events now patch it like any other observed run, and
    // nothing is shown as failed.
    applyMoveProgress(ev('replay', 'started'));
    const patched = get(moves).get(5)!;
    expect(displaySteps(patched).some((s) => s.state === 'failed')).toBe(false);
    expect(displaySteps(patched).map((s) => s.state).slice(0, 5))
      .toEqual(['done', 'done', 'done', 'done', 'started']);
  });

  it('tells the user even when the sheet is open on the run', async () => {
    const p = pending();
    transferSheetFor.set(5);
    startMove(source, 'turanga', { keepSource: false });
    p.reject(busy);
    await flush();
    expect(get(toasts).some((t) => t.kind === 'error')).toBe(true);
  });
});

describe('dismissMove', () => {
  it('removes a settled run and refuses a running local one', () => {
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

  // F3: an observed run is only this window's view of someone else's move.
  // Refusing to let go of it left a running observed run on screen forever.
  it('removes a RUNNING observed run, which the next event re-creates', () => {
    applyMoveProgress(ev('check', 'started'));
    expect(get(moves).get(5)!.status).toBe('running');
    dismissMove(5);
    expect(get(moves).has(5)).toBe(false);
    applyMoveProgress(ev('git', 'started'));
    expect(get(moves).get(5)!.status).toBe('running');
  });
});

// F4 / F5: by the time the result arrives the source row is usually gone.
describe('what a finished move leaves behind', () => {
  it('selects the target even though the source row (and the selection) went away', async () => {
    const p = pending();
    selectSession(source);
    startMove(source, 'turanga', { keepSource: false });
    sessions.set([]); // the kill removed the row, which clears the selection
    expect(get(selectedSession)).toBeNull();
    p.resolve(report());
    await flush();
    expect(get(selectedSession)?.id).toBe(6);
  });

  it('toasts a result with a View action, sticky only when there are warnings', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    p.resolve(report({ warnings: ['one thing'], source_killed: false }));
    await flush();
    const t = get(toasts).find((x) => x.kind === 'success')!;
    expect(t.message).toBe('Moved dev-foo to turanga · 1 warning · the source keeps running');
    expect(t.sticky).toBe(true);
    expect(t.action?.label).toBe('View');
    t.action!.run();
    expect(get(transferSheetFor)).toBe(5);
  });

  it('does not stick a clean result, and says nothing about the source', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    p.resolve(report());
    await flush();
    const t = get(toasts).find((x) => x.kind === 'success')!;
    expect(t.message).toBe('Moved dev-foo to turanga');
    expect(t.sticky).toBe(false);
  });
});

// F5: session row ids are reused, so a run must prove it belongs to the row
// the chip is rendering for.
describe('runForSession', () => {
  it('finds the session\'s own run only when the name and host still match', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    expect(runForSession(get(moves), source)?.sessionId).toBe(5);
    expect(runForSession(get(moves), row({ id: 5, tmux_name: 'something-else' }))).toBeUndefined();
    expect(runForSession(get(moves), row({ id: 5, host_alias: 'elsewhere' }))).toBeUndefined();
    p.resolve(report());
    await flush();
    // The session the move produced finds the source's run.
    expect(runForSession(get(moves), target)?.sessionId).toBe(5);
  });

  // m2a: the target id is reused too — the report has to name this row.
  it('holds the target branch to the same guard as the source branch', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    p.resolve(report());
    await flush();
    expect(runForSession(get(moves), row({ id: 6, tmux_name: 'someone-else', host_alias: 'turanga' })))
      .toBeUndefined();
    expect(runForSession(get(moves), row({ id: 6, tmux_name: 'dev-foo', host_alias: 'elsewhere' })))
      .toBeUndefined();
  });

  // m2b: an observed run created before its row was known has no name or
  // host to compare — the id is all it ever had, so the guard cannot apply.
  it('still finds a placeholder observed run once its row turns up', () => {
    applyMoveProgress(ev('check', 'started', { session_id: 77 }));
    expect(get(moves).get(77)).toMatchObject({ sessionName: 'session 77', fromHost: '' });
    expect(runForSession(get(moves), row({ id: 77, tmux_name: 'late-row', host_alias: 'alpha' }))?.sessionId)
      .toBe(77);
  });
});

// Task 8: retrying a failed transfer re-runs move_session on the SAME run
// entry; resolving a partial calls resolve_move with the TARGET session's id
// (the run is keyed by the source's id) and settles the run as done/undone.
describe('retryMove', () => {
  it('retries the same target and options, on the same run', async () => {
    const session = row({ id: 7, tmux_name: 's', host_alias: 'alpha' });
    invoked.mockResolvedValueOnce(err('E_MOVE_TARGET_DIRTY', 'dirty', { leftovers: 'ours', ours: ['a.txt'] }));
    startMove(session, 'beta', { keepSource: false });
    await flush();
    expect(get(moves).get(7)!.status).toBe('failed');

    invoked.mockResolvedValueOnce(ok(report({ target_session_id: 8 })));
    retryMove(7);
    await flush();
    const run = get(moves).get(7)!;
    expect(run.status).toBe('done');
    expect(run.attempt).toBe(2);
    expect(run.toHost).toBe('beta');
    // The second call carried the same options and no cleanup.
    expect(invoked.mock.calls.at(-1)![1].args).toMatchObject({
      session_id: 7,
      target_host_alias: 'beta',
      keep_source: false,
      clean_target: false,
    });
  });

  it('retries with clean_target when asked, and records it on the run', async () => {
    const session = row({ id: 7, tmux_name: 's', host_alias: 'alpha' });
    invoked.mockResolvedValueOnce(err('E_MOVE_TARGET_DIRTY', 'dirty', { leftovers: 'ours', ours: ['a.txt'] }));
    startMove(session, 'beta', { keepSource: false });
    await flush();
    invoked.mockResolvedValueOnce(ok(report({ target_session_id: 8 })));
    retryMove(7, { cleanTarget: true });
    await flush();
    expect(invoked.mock.calls.at(-1)![1].args).toMatchObject({ clean_target: true });
    expect(get(moves).get(7)!.cleanTarget).toBe(true);
  });

  it('refuses to retry a run that is running or partial', async () => {
    const session = row({ id: 7, tmux_name: 's', host_alias: 'alpha' });
    let resolveIt: (v: unknown) => void = () => {};
    invoked.mockReturnValueOnce(new Promise((r) => (resolveIt = r)));
    startMove(session, 'beta', { keepSource: false });
    await flush();
    const calls = invoked.mock.calls.length;
    retryMove(7);
    expect(invoked.mock.calls.length).toBe(calls);
    resolveIt(err('E_MOVE_PARTIAL', 'partial', { step: 'killing the source s on alpha', target_session_id: 8 }));
    await flush();
    expect(get(moves).get(7)!.status).toBe('partial');
    retryMove(7);
    expect(invoked.mock.calls.length).toBe(calls); // unchanged
  });

  // Fix round 1, Finding 1: `move:progress` carries only the session id, so
  // once `retryMove` puts the run back to `running` with blank steps, a
  // straggler from the FIRST attempt (still in flight when the retry began)
  // looks exactly like the retry's own next step to `applyMoveProgress`.
  it("drops a late event from the attempt it replaced, and applies its own from the first step", async () => {
    const session = row({ id: 7, tmux_name: 's', host_alias: 'alpha' });
    invoked.mockResolvedValueOnce(err('E_MOVE_CARRY', 'x', { step: 'fetch' }));
    startMove(session, 'beta', { keepSource: false });
    await flush();
    expect(get(moves).get(7)!.status).toBe('failed');

    invoked.mockReturnValueOnce(new Promise(() => {})); // the retry's own call never settles here
    retryMove(7);
    // A straggler from the FIRST attempt, naming the LAST step failed.
    applyMoveProgress(ev('handoff', 'failed', { session_id: 7, to_host: 'beta' }));
    let run = get(moves).get(7)!;
    expect(run.status).toBe('running');
    expect(run.steps.map((s) => s.state)).toEqual(Array(9).fill('pending'));

    // The retry's own first event is accepted, from its own first step.
    applyMoveProgress(ev('check', 'started', { session_id: 7, to_host: 'beta' }));
    run = get(moves).get(7)!;
    expect(run.steps[0].state).toBe('started');
    expect(run.steps.slice(1).every((s) => s.state === 'pending')).toBe(true);
  });
});

describe('resolveMoveRun and adoptPartial', () => {
  it('finishing a partial settles the run as done', async () => {
    const session = row({ id: 7, tmux_name: 's', host_alias: 'alpha' });
    invoked.mockResolvedValueOnce(
      err('E_MOVE_PARTIAL', 'partial', { step: 'killing the source s on alpha', target_session_id: 8 }),
    );
    startMove(session, 'beta', { keepSource: false });
    await flush();
    invoked.mockResolvedValueOnce(
      ok({
        action: 'finish',
        source_session_id: 7,
        target_session_id: 8,
        from_host: 'alpha',
        to_host: 'beta',
        source_killed: true,
        target_killed: false,
        warnings: [],
      }),
    );
    resolveMoveRun(7, 'finish');
    await flush();
    expect(invoked.mock.calls.at(-1)![0]).toBe('resolve_move');
    expect(invoked.mock.calls.at(-1)![1].args).toEqual({ session_id: 8, action: 'finish' });
    expect(get(moves).get(7)!.status).toBe('done');
  });

  it('adopts a recorded partial so the sheet can act on it after a restart', () => {
    adoptPartial(
      { targetSessionId: 8, sourceSessionId: 7, fromHost: 'alpha', toHost: 'beta', step: 'killing the source s on alpha' },
      's',
    );
    const run = get(moves).get(7)!;
    expect(run.status).toBe('partial');
    expect(run.toHost).toBe('beta');
    expect(run.error?.details).toMatchObject({ target_session_id: 8 });
  });

  it('undoing a partial leaves the run failed and says so', async () => {
    const session = row({ id: 7, tmux_name: 's', host_alias: 'alpha' });
    invoked.mockResolvedValueOnce(
      err('E_MOVE_PARTIAL', 'partial', { step: 'killing the source s on alpha', target_session_id: 8 }),
    );
    startMove(session, 'beta', { keepSource: false });
    await flush();
    invoked.mockResolvedValueOnce(
      ok({
        action: 'undo',
        source_session_id: 7,
        target_session_id: 8,
        from_host: 'alpha',
        to_host: 'beta',
        source_killed: false,
        target_killed: true,
        warnings: [],
      }),
    );
    resolveMoveRun(7, 'undo');
    await flush();
    const run = get(moves).get(7)!;
    expect(run.status).toBe('failed');
    expect(run.error?.code).toBe('E_MOVE_UNDONE');
  });

  // Fix round 1, Finding 2: the backend really can answer E_MOVE_PARTIAL
  // with `target_session_id: null` (crates/fleet-core/.../move_session/mod.rs
  // :2609, :2693). This is the guard that stops the SOURCE id ever reaching
  // `resolve_move`, so it gets its own test.
  it('refuses to resolve when the partial names no target, and toasts instead', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    p.reject({ code: 'E_MOVE_PARTIAL', message: 'partial', details: { step: 'x', target_session_id: null } });
    await flush();
    expect(get(moves).get(5)!.status).toBe('partial');
    const calls = invoked.mock.calls.length;
    resolveMoveRun(5, 'finish');
    await flush();
    expect(invoked.mock.calls.length).toBe(calls);
    expect(get(toasts).some((t) => t.kind === 'error')).toBe(true);
  });

  // Fix round 1, Finding 3: called on anything but a partial, `resolveMoveRun`
  // would fire `resolve_move` at a move that is not partial (e.g. `report`'s
  // target on a DONE run) and the backend would answer E_INVALID_STATE — a
  // wasted round-trip and a confusing toast. Mirrors `retryMove`'s own guard.
  it('refuses to resolve a run that is not partial', async () => {
    const p = pending();
    startMove(source, 'turanga', { keepSource: false });
    p.resolve(report());
    await flush();
    expect(get(moves).get(5)!.status).toBe('done');
    const calls = invoked.mock.calls.length;
    resolveMoveRun(5, 'finish');
    await flush();
    expect(invoked.mock.calls.length).toBe(calls);
  });
});
