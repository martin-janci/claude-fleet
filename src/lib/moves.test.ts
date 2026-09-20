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
