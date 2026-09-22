import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(() => new Promise(() => {})) }));
import { invoke } from '@tauri-apps/api/core';
import TransferChip from './TransferChip.svelte';
import {
  transferSheetFor,
  startMove,
  applyMoveProgress,
  resetMovesForTest,
  putRunForTest,
  type MoveRun,
} from './moves';
import { MOVE_STEPS, type MoveProgress, type MoveStep, type MoveStepState } from './moveProgress';
import { sessions, type SessionRow } from './sessions';
import { hubStatus } from './hub';
import { hubConnection } from './hub_connection';

const movable = {
  id: 5, tmux_name: 'dev-foo', host_alias: 'alpha', kind: 'work', worktree_id: 10,
  project_id: 1, claude_session_id: 'x', parent_session_id: null, tags: [],
} as unknown as SessionRow;
/** The session a finished move produced on beta. */
const moved = { ...movable, id: 6, host_alias: 'beta', parent_session_id: 5 } as SessionRow;

const ev = (step: MoveStep, state: MoveStepState): MoveProgress => ({
  session_id: 5, to_host: 'beta', step, index: MOVE_STEPS.indexOf(step) + 1, total: 9, state, detail: null,
});

const report = {
  source_session_id: 5, target_session_id: 6, from_host: 'alpha', to_host: 'beta',
  tmux_name: 'dev-foo', claude_session_id: 'x', branch: 'feat', target_cwd: '/r',
  transcript_bytes: 1, source_killed: true, warnings: [],
  carried: {
    commits: 0, bundle_bytes: 0, dirty_entries: [], ignored_carried: [],
    ignored_left_behind: [], target_seeded: 'existing',
    session_state: { carried: [], kept_target: [], left_behind: [] },
    memory: { carried: [], kept_target: [], identical: 0, index_lines_added: 0, left_behind: [] },
  },
  target: moved,
};
const flush = () => new Promise((r) => setTimeout(r, 0));

/** A `move_session` call this test settles by hand. */
function donePromise() {
  let resolve!: (v: unknown) => void;
  let reject!: (e: unknown) => void;
  vi.mocked(invoke).mockImplementation((cmd: string) =>
    cmd === 'move_session'
      ? new Promise((res, rej) => { resolve = res; reject = rej; })
      : Promise.resolve(undefined),
  );
  return { resolve: (v: unknown) => resolve(v), reject: (e: unknown) => reject(e) };
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(() => new Promise(() => {}));
  resetMovesForTest();
  sessions.set([movable]);
  hubStatus.set({ ...get(hubStatus), remote: false, unavailable: null, url: null });
  hubConnection.set({ state: 'standalone' });
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

  // F12: the live chip is a button with a glyph and a count — nothing says
  // what clicking it does.
  it('running: the live indicator says what it is for', async () => {
    startMove(movable, 'beta', { keepSource: false });
    render(TransferChip, { props: { session: movable } });
    await tick();
    const live = screen.getByTestId('transfer-live');
    expect(live.getAttribute('title')).toBe('Show the transfer to beta');
    expect(live.getAttribute('aria-label')).toBe('Show the transfer to beta');
  });

  it('done: says where it went', async () => {
    const p = donePromise();
    startMove(movable, 'beta', { keepSource: false });
    render(TransferChip, { props: { session: movable } });
    p.resolve({ kind: 'moved', ...report });
    await flush();
    await tick();
    expect(screen.getByTestId('transfer-live').textContent?.replace(/\s+/g, ' ').trim())
      .toBe('⇄ moved to beta');
  });

  // Task 8: a pending wait reads distinctly from a running or a failed move.
  it('waiting: reads as waiting, not running or failed', async () => {
    const run: MoveRun = {
      sessionId: 5, sessionName: 'dev-foo', fromHost: 'alpha', toHost: 'beta',
      keepSource: null, origin: 'local',
      steps: MOVE_STEPS.map((step) => ({ step, state: 'pending' as const, detail: null })),
      status: 'waiting', report: null, error: null, resolveError: null,
      startedAt: Date.now(), settledAt: null, cleanTarget: false, attempt: 1,
      resolving: false, awaitingStart: false,
      deadlineUnix: Math.floor(Date.now() / 1000) + 600, waitEnded: null, waitRefusal: null,
    };
    putRunForTest(run);
    render(TransferChip, { props: { session: movable } });
    await tick();
    const live = screen.getByTestId('transfer-live');
    expect(live.getAttribute('data-state')).toBe('waiting');
    expect(live.textContent?.replace(/\s+/g, ' ').trim()).toBe('⇄ waiting to move to beta');
    expect(live.textContent).not.toContain('failed');
  });

  // F10: a partial move is not a failed one — both sessions are alive.
  it('partial: reads as incomplete, not failed', async () => {
    const p = donePromise();
    startMove(movable, 'beta', { keepSource: false });
    render(TransferChip, { props: { session: movable } });
    p.reject({ code: 'E_MOVE_PARTIAL', message: 'x', details: {} });
    await flush();
    await tick();
    const text = screen.getByTestId('transfer-live').textContent?.replace(/\s+/g, ' ').trim();
    expect(text).toBe('⇄ move incomplete');
  });

  // F5: the source row is gone by the time the move finishes; the chip on
  // the session it PRODUCED is the only one left to show the result.
  it('the target session wears the source\'s run and opens its sheet', async () => {
    const p = donePromise();
    startMove(movable, 'beta', { keepSource: false });
    p.resolve({ kind: 'moved', ...report });
    await flush();
    sessions.set([moved]);
    render(TransferChip, { props: { session: moved } });
    await tick();
    const live = screen.getByTestId('transfer-live');
    expect(live.textContent?.replace(/\s+/g, ' ').trim()).toBe('⇄ moved from alpha');
    await fireEvent.click(live);
    expect(get(transferSheetFor)).toBe(5);
  });

  // P-T6: a hub client whose link is down cannot start a move, and the chip
  // has to say why rather than fail on click.
  it('blocked: the chip is disabled and its title is the reason', async () => {
    hubStatus.set({ ...get(hubStatus), remote: true, url: 'https://hub.example' });
    hubConnection.set({ state: 'offline', attempt: 2, retry_in_secs: 5, reason: 'no route' });
    render(TransferChip, { props: { session: movable } });
    await tick();
    const chip = screen.getByTestId('transfer-chip') as HTMLButtonElement;
    expect(chip.disabled).toBe(true);
    expect(chip.title).toContain('unreachable right now');
  });
});
