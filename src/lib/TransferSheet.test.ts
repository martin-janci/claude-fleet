import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import TransferSheet from './TransferSheet.svelte';
import { moves, transferSheetFor, startMove, applyMoveProgress, resetMovesForTest } from './moves';
import { MOVE_STEPS, type MoveProgress, type MoveStep, type MoveStepState } from './moveProgress';
import { sessions, type SessionRow } from './sessions';
import { hosts, type HostRow } from './hosts';
import { selectedSession, selectSession } from './selection';

const mockInvoke = invoke as ReturnType<typeof vi.fn>;
const flush = () => new Promise((r) => setTimeout(r, 0));

const source = {
  id: 5, tmux_name: 'dev-foo', host_alias: 'mefistos', kind: 'work', worktree_id: 10,
  project_id: 1, claude_session_id: '550e8400-e29b-41d4-a716-446655440000',
  parent_session_id: null, tags: [],
} as unknown as SessionRow;
const target = { ...source, id: 6, host_alias: 'turanga', parent_session_id: 5 } as SessionRow;
const host = (alias: string, over: Partial<HostRow> = {}) =>
  ({ alias, ssh_alias: alias, reachable: true, hidden: false, provisioned: true,
     claude_version: null, tmux_version: null, last_pinged_at: 1, account_uuid: null,
     transport: 'ssh', ...over }) as HostRow;

const report = {
  source_session_id: 5, target_session_id: 6, from_host: 'mefistos', to_host: 'turanga',
  tmux_name: 'dev-foo', claude_session_id: source.claude_session_id, branch: 'feat',
  target_cwd: '/r', transcript_bytes: 10, source_killed: true,
  warnings: ['origin was unreachable from turanga', 'replaced an existing 4-byte transcript'],
  carried: {
    commits: 2, bundle_bytes: 100,
    dirty_entries: [{ status: ' M', path: 'a.rs' }],
    ignored_carried: [{ path: '.env', bytes: 4 }],
    ignored_left_behind: [{ path: 'big.bin', bytes: 9000000, reason: 'over_cap' }],
    target_seeded: 'existing',
    session_state: { carried: [{ path: 'subagents/a.jsonl', bytes: 9 }], kept_target: ['custom-title.json'], left_behind: [] },
    memory: { carried: [{ path: 'note.md', bytes: 3 }], kept_target: ['Other.md'], identical: 1, index_lines_added: 1, left_behind: [] },
  },
  target,
};

const ev = (step: MoveStep, state: MoveStepState, detail: string | null = null): MoveProgress => ({
  session_id: 5, to_host: 'turanga', step, index: MOVE_STEPS.indexOf(step) + 1, total: 9, state, detail,
});

function pendingMove() {
  let resolve!: (v: unknown) => void;
  let reject!: (e: unknown) => void;
  mockInvoke.mockImplementation((cmd: string) =>
    cmd === 'move_session'
      ? new Promise((res, rej) => { resolve = res; reject = rej; })
      : Promise.resolve(undefined),
  );
  return { resolve: (v: unknown) => resolve(v), reject: (e: unknown) => reject(e) };
}

beforeEach(() => {
  mockInvoke.mockReset();
  resetMovesForTest();
  sessions.set([source]);
  hosts.set([host('mefistos'), host('turanga'), host('down', { reachable: false })]);
  selectSession(null);
});

describe('TransferSheet', () => {
  it('renders nothing while no sheet is open', () => {
    render(TransferSheet);
    expect(screen.queryByTestId('move-dialog')).toBeNull();
  });

  it('setup: offers eligible targets and starts the move', async () => {
    pendingMove();
    transferSheetFor.set(5);
    render(TransferSheet);
    await tick();
    const select = (await screen.findByTestId('move-target')) as HTMLSelectElement;
    expect(Array.from(select.options, (o) => o.value)).toEqual(['turanga']);
    await fireEvent.click(screen.getByTestId('move-keep-source'));
    await fireEvent.click(screen.getByTestId('confirm-move'));
    expect(mockInvoke).toHaveBeenCalledWith('move_session', {
      args: { session_id: 5, target_host_alias: 'turanga', keep_source: true, strict: false },
    });
    expect(await screen.findByTestId('transfer-steps')).toBeTruthy();
  });

  it('setup: with no eligible target the button is disabled and says why', async () => {
    hosts.set([host('mefistos')]);
    transferSheetFor.set(5);
    render(TransferSheet);
    expect(await screen.findByTestId('move-no-targets')).toBeTruthy();
    expect((screen.getByTestId('confirm-move') as HTMLButtonElement).disabled).toBe(true);
  });

  it('progress: shows the nine steps with their states and details; Close leaves the move running', async () => {
    pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    applyMoveProgress(ev('git', 'done', '2 commits'));
    applyMoveProgress(ev('replay', 'started'));
    await tick();
    const items = (await screen.findByTestId('transfer-steps')).querySelectorAll('li');
    expect(items).toHaveLength(9);
    expect(items[3].getAttribute('data-state')).toBe('done');
    expect(items[3].textContent).toContain('Carry the git work');
    expect(items[3].textContent).toContain('2 commits');
    expect(items[4].getAttribute('data-state')).toBe('started');
    expect(items[7].textContent).toContain('Start on turanga');
    expect(items[8].getAttribute('data-state')).toBe('pending');
    await fireEvent.click(screen.getByTestId('transfer-close'));
    expect(get(transferSheetFor)).toBeNull();
    expect(get(moves).get(5)!.status).toBe('running');
  });

  it('progress: an observed run says it was started elsewhere', async () => {
    applyMoveProgress(ev('check', 'started'));
    transferSheetFor.set(5);
    render(TransferSheet);
    expect((await screen.findByTestId('move-dialog')).textContent).toContain('Started elsewhere');
  });

  it('result: counts, one warning per line, details, open and done', async () => {
    const p = pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    p.resolve(report);
    await flush();
    await tick();
    const result = await screen.findByTestId('transfer-result');
    expect(result.textContent).toContain('2 commits');
    expect(result.textContent).toContain('1 uncommitted entry');
    expect(result.textContent).toContain('1 ignored file');
    expect(result.textContent).toContain('1 left behind');
    expect(result.querySelectorAll('[data-testid="transfer-warning"]')).toHaveLength(2);
    expect(result.textContent).not.toContain('big.bin');
    await fireEvent.click(screen.getByTestId('transfer-details'));
    expect(result.textContent).toContain('big.bin');
    expect(result.textContent).toContain('over the size cap');
    expect(result.textContent).toContain('Other.md');
    await fireEvent.click(screen.getByTestId('transfer-open-target'));
    expect(get(selectedSession)?.id).toBe(6);
    expect(get(transferSheetFor)).toBeNull();
    expect(get(moves).has(5)).toBe(false);
  });

  it('result: a clean pushed branch says there was nothing to carry', async () => {
    const p = pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    p.resolve({ ...report, warnings: [], carried: { ...report.carried, commits: 0, dirty_entries: [] } });
    await flush();
    await tick();
    expect((await screen.findByTestId('transfer-result')).textContent)
      .toContain('Nothing to carry — the branch was pushed and clean');
    expect(screen.queryByTestId('transfer-warning')).toBeNull();
  });

  it('failure: a sentence, where things stand, the failed step, raw details collapsed; Done dismisses', async () => {
    const p = pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    applyMoveProgress(ev('git', 'started'));
    p.reject({ code: 'E_MOVE_CARRY', message: 'raw backend text', details: { step: 'fetch', stderr: 'fatal: bad object' } });
    await flush();
    await tick();
    const failure = await screen.findByTestId('transfer-failure');
    expect(failure.textContent).toContain('The target could not take in the carried commits.');
    expect(failure.textContent).toContain('The source session was not touched.');
    expect(screen.getByTestId('transfer-steps').querySelectorAll('li')[3].getAttribute('data-state')).toBe('failed');
    const raw = failure.querySelector('details')!;
    expect(raw.open).toBe(false);
    expect(raw.textContent).toContain('E_MOVE_CARRY');
    expect(raw.textContent).toContain('fatal: bad object');
    await fireEvent.click(screen.getByTestId('transfer-done'));
    expect(get(moves).has(5)).toBe(false);
    expect(get(transferSheetFor)).toBeNull();
  });

  it('partial: links the new session', async () => {
    sessions.set([source, target]);
    const p = pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    p.reject({ code: 'E_MOVE_PARTIAL', message: 'x', details: { target_session_id: 6 } });
    await flush();
    await tick();
    expect((await screen.findByTestId('transfer-failure')).textContent).toContain('is running on turanga');
    await fireEvent.click(screen.getByTestId('transfer-open-target'));
    expect(get(selectedSession)?.id).toBe(6);
  });

  it('closes itself when the session is gone and there is no run', async () => {
    transferSheetFor.set(99);
    render(TransferSheet);
    await tick();
    expect(get(transferSheetFor)).toBeNull();
  });
});
