import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';

// Task 7: `requestPreflight` becomes a bare spy so the setup-view tests can
// see how the component called it, without a real debounced round trip
// through `previewMove`/`invoke`. Everything else — `preflights`,
// `preflightFor`, `preflightAge`, `putPreflightForTest`, the constants — stays
// the real module, so seeding an entry goes through the module's own private
// key function rather than a copy of it in this file.
vi.mock('./preflight', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./preflight')>();
  return { ...actual, requestPreflight: vi.fn() };
});

// Only `retryMove`, `resolveMoveRun` and `startMove` are replaced: `startMove`
// keeps its real behaviour (wrapped so its calls can still be asserted on) —
// the pre-existing tests below drive it for real through the mocked `invoke` —
// while `retryMove`/`resolveMoveRun` become bare spies, since the failure-view
// tests only need to see how they were called, never a real round trip.
vi.mock('./moves', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./moves')>();
  return {
    ...actual,
    retryMove: vi.fn(),
    resolveMoveRun: vi.fn(),
    startMove: vi.fn(actual.startMove),
  };
});
import TransferSheet from './TransferSheet.svelte';
import {
  moves,
  transferSheetFor,
  startMove,
  retryMove,
  resolveMoveRun,
  putRunForTest,
  applyMoveProgress,
  resetMovesForTest,
  type MoveRun,
} from './moves';
import { UNDONE, describeMoveError } from './moveErrors';
import { MOVE_STEPS, type MoveProgress, type MoveStep, type MoveStepState } from './moveProgress';
import type { MovePreview, MoveReport } from './moveSession';
import type { IpcError } from './result';
import { sessions, type SessionRow } from './sessions';
import { hosts, type HostRow } from './hosts';
import { selectedSession, selectSession } from './selection';
import { toasts, clearToasts } from './toasts';
import { hubConnection } from './hub_connection';
import type { HubConnection } from './hub_connection';
import {
  requestPreflight,
  resetPreflightsForTest,
  putPreflightForTest,
  PREFLIGHT_DEBOUNCE_MS,
  PREFLIGHT_STALE_MS,
  type PreflightEntry,
} from './preflight';

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
      args: {
        session_id: 5, target_host_alias: 'turanga', keep_source: true, strict: false,
        clean_target: false, dry_run: false, when: 'now',
      },
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
    p.resolve({ kind: 'moved', ...report });
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
    p.resolve({ kind: 'moved', ...report, warnings: [], carried: { ...report.carried, commits: 0, dirty_entries: [] } });
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
    expect((await screen.findByTestId('transfer-failure')).textContent)
      .toContain('A new session exists on turanga and the source is still there.');
    await fireEvent.click(screen.getByTestId('transfer-open-target'));
    expect(get(selectedSession)?.id).toBe(6);
  });

  // F3: an observed run is someone else's move. Without a way out, a window
  // that had seen one running was stuck showing it.
  it('progress: an observed run can be stopped following', async () => {
    applyMoveProgress(ev('check', 'started'));
    transferSheetFor.set(5);
    render(TransferSheet);
    await fireEvent.click(await screen.findByTestId('transfer-stop-following'));
    expect(get(moves).has(5)).toBe(false);
    expect(get(transferSheetFor)).toBeNull();
  });

  // B3/F1: the hub never answered. Saying "started elsewhere" would be wrong
  // — this window started it, and nobody knows how it ended.
  it('progress: a run whose hub went quiet says so instead', async () => {
    const p = pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    p.reject({
      code: 'E_HUB_UNREACHABLE',
      message: 'https://hub.example/mcp did not answer: connect: connection refused',
      details: null,
    });
    await flush();
    await tick();
    const dialog = screen.getByTestId('move-dialog');
    expect(dialog.textContent).toContain('Lost contact with the hub — the move may still be running there.');
    expect(dialog.textContent).not.toContain('Started elsewhere');
    expect(screen.getByTestId('transfer-stop-following')).toBeTruthy();
    // R3: a connection refused and a 404 come back under the same code, and
    // those are definite failures. The note must not hide what was said.
    expect(screen.getByTestId('transfer-lost-contact-detail').textContent)
      .toContain('https://hub.example/mcp did not answer: connect: connection refused');
  });

  // m3: the same run, once its events finish it. "Started elsewhere" is
  // wrong — this window started it and then lost the answer.
  it('result: a run that lost its hub says so instead of blaming another window', async () => {
    const p = pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    p.reject({ code: 'E_HUB_UNREACHABLE', message: 'no answer', details: null });
    await flush();
    applyMoveProgress(ev('handoff', 'done'));
    await tick();
    const result = await screen.findByTestId('transfer-result');
    expect(result.textContent)
      .toContain('The connection to the hub was lost during the move, so this window has no report for it.');
    expect(result.textContent).not.toContain('Started elsewhere');
  });

  // P-T5: the result view of a move this window only watched.
  it('result: an observed run has no report, and offers the new session when it can find it', async () => {
    applyMoveProgress(ev('check', 'started'));
    applyMoveProgress(ev('handoff', 'done'));
    transferSheetFor.set(5);
    render(TransferSheet);
    await tick();
    expect((await screen.findByTestId('transfer-result')).textContent)
      .toContain('Started elsewhere — this window has no report for it.');
    expect(screen.queryByTestId('transfer-details')).toBeNull();
    expect(screen.queryByTestId('transfer-open-target')).toBeNull();
    expect(screen.getByTestId('transfer-done')).toBeTruthy();

    sessions.set([source, target]);
    await tick();
    expect((screen.getByTestId('transfer-open-target') as HTMLElement).textContent)
      .toContain('Open on turanga');
  });

  // F6: the sheet passes the step the move reached, so the standing sentence
  // can tell "nothing was copied" from "the clone is still over there".
  it('failure: a refusal in the check says nothing was copied', async () => {
    const p = pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    p.reject({ code: 'E_MOVE_DIRTY', message: 'dirty', details: null });
    await flush();
    await tick();
    const failure = await screen.findByTestId('transfer-failure');
    expect(failure.textContent).toContain('Nothing was copied to turanga.');
    expect(failure.textContent).not.toContain('was left there');
  });

  // P-T5: two identical warnings, and one path left behind by two different
  // lists, are both legal — a keyed `{#each}` on the value alone throws.
  it('result: duplicate warnings and a repeated path still render', async () => {
    const p = pendingMove();
    startMove(source, 'turanga', { keepSource: false });
    transferSheetFor.set(5);
    render(TransferSheet);
    const both = { path: 'same.bin', bytes: 1, reason: 'over_cap' as const };
    p.resolve({
      kind: 'moved',
      ...report,
      warnings: ['the very same warning', 'the very same warning'],
      carried: {
        ...report.carried,
        ignored_left_behind: [both],
        session_state: { ...report.carried.session_state, left_behind: [both] },
        memory: { ...report.carried.memory, left_behind: [{ ...both, reason: 'from_a_newer_backend' }] },
      },
    });
    await flush();
    await tick();
    const result = await screen.findByTestId('transfer-result');
    expect(result.querySelectorAll('[data-testid="transfer-warning"]')).toHaveLength(2);
    await fireEvent.click(screen.getByTestId('transfer-details'));
    expect(result.textContent?.match(/same\.bin/g)).toHaveLength(3);
    // F11: a reason this build does not know is shown as it came.
    expect(result.textContent).toContain('from_a_newer_backend');
  });

  it('closes itself when the session is gone and there is no run', async () => {
    transferSheetFor.set(99);
    render(TransferSheet);
    await tick();
    expect(get(transferSheetFor)).toBeNull();
  });
});

// Task 11: the four recovery actions in the failure/result views. These
// fixtures build a `MoveRun` directly and seed it with `putRunForTest` — the
// public start/retry/resolve API cannot produce an arbitrary failed/done/
// partial run synchronously, and the assertions below need the button to
// already be in the DOM the moment `renderSheet` returns, with no `await` in
// between.
describe('TransferSheet: recovery actions', () => {
  function blankSteps() {
    return MOVE_STEPS.map((step) => ({ step, state: 'pending' as const, detail: null }));
  }

  function renderSheet(run: MoveRun) {
    putRunForTest(run);
    transferSheetFor.set(run.sessionId);
    return render(TransferSheet);
  }

  function failedRun(err: { code: string; message?: string; details?: unknown }): MoveRun {
    return {
      sessionId: 7,
      sessionName: 'sess7',
      fromHost: 'alpha',
      toHost: 'beta',
      keepSource: false,
      origin: 'local',
      steps: blankSteps(),
      status: 'failed',
      report: null,
      error: { code: err.code, message: err.message ?? 'x', details: err.details ?? null },
      resolveError: null,
      startedAt: Date.now(),
      settledAt: Date.now(),
      cleanTarget: false,
      attempt: 1,
      resolving: false,
      awaitingStart: false,
    };
  }

  function doneRun(opts: { fromHost: string; toHost: string; sourceKilled?: boolean }): MoveRun {
    const targetRow = {
      ...source, id: 8, tmux_name: 'sess7', host_alias: opts.toHost, parent_session_id: 7,
    } as SessionRow;
    sessions.set([targetRow]);
    return {
      sessionId: 7,
      sessionName: 'sess7',
      fromHost: opts.fromHost,
      toHost: opts.toHost,
      keepSource: false,
      origin: 'local',
      steps: blankSteps(),
      status: 'done',
      report: {
        ...report,
        source_session_id: 7,
        target_session_id: 8,
        from_host: opts.fromHost,
        to_host: opts.toHost,
        source_killed: opts.sourceKilled ?? true,
        target: targetRow,
      } as MoveReport,
      error: null,
      resolveError: null,
      startedAt: Date.now(),
      settledAt: Date.now(),
      cleanTarget: false,
      attempt: 1,
      resolving: false,
      awaitingStart: false,
    };
  }

  function partialRun(
    opts: { resolveError?: IpcError | null; details?: unknown; resolving?: boolean } = {},
  ): MoveRun {
    return {
      sessionId: 7,
      sessionName: 'sess7',
      fromHost: 'alpha',
      toHost: 'beta',
      keepSource: null,
      origin: 'local',
      steps: blankSteps(),
      status: 'partial',
      report: null,
      error: {
        code: 'E_MOVE_PARTIAL',
        message: '',
        details:
          'details' in opts
            ? opts.details
            : { step: 'confirming the target is running', target_session_id: 8 },
      },
      resolveError: opts.resolveError ?? null,
      startedAt: Date.now(),
      settledAt: Date.now(),
      cleanTarget: false,
      attempt: 1,
      resolving: opts.resolving ?? false,
      awaitingStart: false,
    };
  }

  beforeEach(() => {
    mockInvoke.mockReset();
    vi.mocked(retryMove).mockClear();
    vi.mocked(resolveMoveRun).mockClear();
    vi.mocked(startMove).mockClear();
    resetMovesForTest();
    sessions.set([source]);
    hosts.set([host('alpha'), host('beta')]);
    selectSession(null);
  });

  it('offers Retry on a plain failure and re-runs the same move', async () => {
    const { getByTestId } = renderSheet(
      failedRun({ code: 'E_MOVE_CARRY', details: { step: 'apply' } }),
    );
    await fireEvent.click(getByTestId('transfer-retry'));
    expect(retryMove).toHaveBeenCalledWith(7, { cleanTarget: false });
  });

  it('offers a cleanup that names what it would replace, and needs two clicks', async () => {
    const { getByTestId, queryByTestId } = renderSheet(
      failedRun({
        code: 'E_MOVE_TARGET_DIRTY',
        details: { leftovers: 'ours', ours: ['src/lib.rs', 'notes.txt'] },
      }),
    );
    await fireEvent.click(getByTestId('transfer-clean'));
    // Nothing has run yet: the first click only reveals what it would remove.
    // (The sentence above the buttons already names the same paths, so scope
    // the check to the failure body rather than matching text anywhere.)
    expect(retryMove).not.toHaveBeenCalled();
    expect(getByTestId('transfer-failure').textContent).toContain('src/lib.rs');
    await fireEvent.click(getByTestId('transfer-clean-confirm'));
    expect(retryMove).toHaveBeenCalledWith(7, { cleanTarget: true });
    expect(queryByTestId('transfer-clean-confirm')).toBeNull();
  });

  // Fix round 1, finding 2: `carry.rs` caps leftovers at 50 and reports the
  // rest in `more_ours`; a confirmation that only counts the 50 it was shown
  // would silently understate what it is about to delete on another machine.
  it('names the true total, and how many were left out, when the cleanup was capped', async () => {
    const shown = Array.from({ length: 50 }, (_, i) => `f${i}.txt`);
    const { getByTestId } = renderSheet(
      failedRun({
        code: 'E_MOVE_TARGET_DIRTY',
        details: { leftovers: 'ours', ours: shown, more_ours: 150 },
      }),
    );
    await fireEvent.click(getByTestId('transfer-clean'));
    // (The `.what` sentence above already says "150 more" too, so scope the
    // path-list check to the failure body rather than matching anywhere.)
    expect(getByTestId('transfer-failure').textContent).toContain('150 more');
    // 50 shown + 150 left out — the confirm button must not claim only 50.
    expect(getByTestId('transfer-clean-confirm').textContent).toContain('200');
  });

  it('never offers a cleanup for the target own work', () => {
    const { queryByTestId } = renderSheet(
      failedRun({ code: 'E_MOVE_TARGET_DIRTY', details: { leftovers: 'theirs', theirs: ['x.md'] } }),
    );
    expect(queryByTestId('transfer-clean')).toBeNull();
    expect(queryByTestId('transfer-retry')).toBeNull();
  });

  it('offers Move back on a finished move', async () => {
    // Clicking "Move back" starts a genuine (mocked-invoke) move of session 8:
    // give it something to resolve with so that real flow does not throw.
    mockInvoke.mockImplementation(() => Promise.resolve({ kind: 'moved', ...report }));
    const { getByTestId } = renderSheet(doneRun({ fromHost: 'alpha', toHost: 'beta' }));
    const back = getByTestId('transfer-move-back');
    expect(back.textContent).toContain('alpha');
    await fireEvent.click(back);
    expect(startMove).toHaveBeenCalledWith(
      expect.objectContaining({ id: 8 }),
      'alpha',
      { keepSource: false },
    );
  });

  // Whole-branch review, finding 2: a `keep_source` move leaves the ORIGIN
  // running the same conversation in the same worktree. "Move back" there
  // would aim the transfer at that live session's own worktree — the engine
  // now refuses it, and the sheet must not offer it in the first place.
  it('offers Move back only when the move actually killed the source', () => {
    const { queryByTestId } = renderSheet(
      doneRun({ fromHost: 'alpha', toHost: 'beta', sourceKilled: false }),
    );
    expect(queryByTestId('transfer-move-back')).toBeNull();
    // The rest of the result view is unaffected.
    expect(queryByTestId('transfer-done')).toBeTruthy();
  });

  // Whole-branch review, finding 6: the two earliest partial steps record no
  // target id at all, so `resolve_move` has nothing to act on and refuses.
  // Offering a red "Kill the new session on beta" that can only answer "no
  // target session to resolve" is worse than offering nothing.
  it('offers no Finish or Undo for a partial nothing can resolve', () => {
    const { queryByTestId } = renderSheet(
      partialRun({ details: { step: 'reconciling the target host', target_session_id: null } }),
    );
    expect(queryByTestId('transfer-finish')).toBeNull();
    expect(queryByTestId('transfer-undo')).toBeNull();
    expect(queryByTestId('transfer-done')).toBeTruthy();
  });

  // …and finding 8's UI half: while one resolve is out, the confirm the user
  // is still looking at must not take a second click.
  it('disables the confirm while a resolve is in flight', async () => {
    const { getByTestId } = renderSheet(partialRun({ resolving: true }));
    await fireEvent.click(getByTestId('transfer-finish'));
    const confirm = getByTestId('transfer-finish-confirm') as HTMLButtonElement;
    expect(confirm.disabled).toBe(true);
    // (jsdom dispatches a click on a disabled button all the same, so the
    // second click is pinned where it is actually refused: `resolveMoveRun`'s
    // own `resolving` guard, in moves.test.ts.)
    expect((getByTestId('transfer-undo') as HTMLButtonElement).disabled).toBe(false);
  });

  it('offers Finish and Undo on a partial, each behind its own confirm', async () => {
    const { getByTestId } = renderSheet(partialRun());
    await fireEvent.click(getByTestId('transfer-finish'));
    expect(resolveMoveRun).not.toHaveBeenCalled();
    await fireEvent.click(getByTestId('transfer-finish-confirm'));
    expect(resolveMoveRun).toHaveBeenCalledWith(7, 'finish');

    await fireEvent.click(getByTestId('transfer-undo'));
    await fireEvent.click(getByTestId('transfer-undo-confirm'));
    expect(resolveMoveRun).toHaveBeenCalledWith(7, 'undo');
  });

  // Fix round 1, finding 1: this drives the REAL `resolveMoveRun` (moves.ts)
  // through the mocked `invoke`, so a genuine resolve failure reaches the run
  // and the sheet, rather than a fixture pre-baking the text `describeMoveError`
  // would show anyway. Deleting the rendering in TransferSheet.svelte (the
  // `{#if resolveError}` block) must fail this test — verified below by doing
  // exactly that and watching it fail before restoring it.
  it('shows a refusal from Finish in place, not as a toast', async () => {
    clearToasts();
    const real = await vi.importActual<typeof import('./moves')>('./moves');
    vi.mocked(resolveMoveRun).mockImplementationOnce(real.resolveMoveRun);
    mockInvoke.mockImplementation((cmd: string) =>
      cmd === 'resolve_move'
        ? Promise.reject({ code: 'E_INVALID_STATE', message: 'the target took a turn', details: null })
        : Promise.resolve(undefined),
    );
    const { getByTestId } = renderSheet(partialRun());
    await fireEvent.click(getByTestId('transfer-finish'));
    await fireEvent.click(getByTestId('transfer-finish-confirm'));
    await flush();
    await tick();
    expect(getByTestId('transfer-resolve-error').textContent).toContain('took a turn');
    // The buttons stay up: the user can still act on what the refusal says.
    expect(getByTestId('transfer-finish')).toBeTruthy();
    expect(get(toasts)).toHaveLength(0);
  });

  // Fix round 2: the asymmetry is the point. When the run is still present
  // (above) the sheet carries the refusal and no toast fires; when the sheet
  // is closed and its run dismissed before the call settles, there is no
  // sheet left to show anything, so the refusal must fall back to a toast
  // instead of vanishing — Finish/Undo each kill a live session, so "clicked
  // it, saw nothing" must never mean "it worked".
  it('a refusal after the sheet is dismissed becomes a toast instead of vanishing', async () => {
    clearToasts();
    const real = await vi.importActual<typeof import('./moves')>('./moves');
    vi.mocked(resolveMoveRun).mockImplementationOnce(real.resolveMoveRun);
    let reject!: (e: unknown) => void;
    mockInvoke.mockImplementation((cmd: string) =>
      cmd === 'resolve_move'
        ? new Promise((_res, rej) => {
            reject = rej;
          })
        : Promise.resolve(undefined),
    );
    const { getByTestId } = renderSheet(partialRun());
    await fireEvent.click(getByTestId('transfer-finish'));
    await fireEvent.click(getByTestId('transfer-finish-confirm'));
    // Close the sheet — and dismiss its run — before the call settles.
    await fireEvent.click(getByTestId('transfer-done'));
    expect(get(moves).has(7)).toBe(false);
    reject({ code: 'E_INVALID_STATE', message: 'the target took a turn', details: null });
    await flush();
    const errorToasts = get(toasts).filter((t) => t.kind === 'error');
    expect(errorToasts).toHaveLength(1);
    expect(errorToasts[0].message).toContain('took a turn');
  });

  it('an undone run reads as undone', () => {
    const { getByText, queryByTestId } = renderSheet(
      failedRun({ code: UNDONE, details: null }),
    );
    expect(getByText(/undid/)).toBeTruthy();
    expect(queryByTestId('transfer-retry')).toBeNull();
  });

  // Fix round 1, finding 3: the brief requires a confirm — and now a
  // refusal — to never survive the sheet moving to another session.
  it('resets an armed confirm and any in-place refusal when the sheet moves to another session', async () => {
    const a = partialRun({
      resolveError: { code: 'E_INVALID_STATE', message: 'the target took a turn', details: null },
    });
    const b = { ...partialRun(), sessionId: 9, sessionName: 'sess9' };
    putRunForTest(a);
    putRunForTest(b);
    transferSheetFor.set(a.sessionId);
    const { getByTestId, queryByTestId } = render(TransferSheet);
    expect(getByTestId('transfer-resolve-error')).toBeTruthy();
    await fireEvent.click(getByTestId('transfer-finish'));
    expect(getByTestId('transfer-finish-confirm')).toBeTruthy();

    transferSheetFor.set(b.sessionId);
    await tick();
    expect(queryByTestId('transfer-finish-confirm')).toBeNull();
    expect(queryByTestId('transfer-resolve-error')).toBeNull();
  });
});

// Task 7: the setup view previews what a Transfer would carry before it is
// pressed. The one rule this must not break: Transfer's `disabled` stays
// exactly `!target || blocked !== null` — never gated by the preflight, so
// three of the tests below arm the preview into loading/refused/stale and
// simply check the button is not disabled.
describe('TransferSheet: preflight', () => {
  const pfSource = {
    id: 7, tmux_name: 'sess7', host_alias: 'alpha', kind: 'work', worktree_id: 20,
    project_id: 1, claude_session_id: '660e8400-e29b-41d4-a716-446655440000',
    parent_session_id: null, tags: [],
  } as unknown as SessionRow;

  const previewFixture: MovePreview = {
    session_id: 7,
    from_host: 'alpha',
    to_host: 'beta',
    branch: 'feat',
    source_cwd: '/work/feat',
    unpushed_commits: 2,
    commits_ahead: 1,
    dirty: [{ status: ' M', path: 'src/lib.rs' }],
    ignored_carried: [{ path: '.env', bytes: 12 }],
    ignored_left_behind: [{ path: 'node_modules', bytes: 900000000, reason: 'over_cap' }],
    transcript_bytes: 500,
    session_state_files: 2,
    session_state_bytes: 40,
    memory_files: 1,
    memory_bytes: 10,
    target_path: '/home/beta/work/feat',
    target: { state: 'clean', head: 'abc1234' },
    unknowns: ['bundle size is an estimate'],
  };

  const refusalFixture: IpcError = {
    code: 'E_MOVE_TARGET_DIRTY',
    message: 'x',
    details: { leftovers: 'theirs', theirs: ['x.md'] },
  };

  function seed(toHost: string, kind: 'loading' | 'ready' | 'refused' | 'stale'): PreflightEntry {
    if (kind === 'loading') {
      return { sessionId: pfSource.id, toHost, status: 'loading', preview: null, error: null, at: null };
    }
    if (kind === 'refused') {
      return {
        sessionId: pfSource.id, toHost, status: 'refused', preview: null, error: refusalFixture,
        at: Date.now(),
      };
    }
    // 'ready' and 'stale' both carry the same preview; 'stale' only differs
    // in how long ago it arrived.
    const at = kind === 'stale' ? Date.now() - PREFLIGHT_STALE_MS - 5000 : Date.now();
    return { sessionId: pfSource.id, toHost, status: 'ready', preview: previewFixture, error: null, at };
  }

  function renderSetup(opts: {
    targets: string[];
    preflight?: 'loading' | 'ready' | 'refused' | 'stale';
    connection?: HubConnection;
  }) {
    resetMovesForTest();
    resetPreflightsForTest();
    sessions.set([pfSource]);
    hosts.set([host('alpha'), ...opts.targets.map((t) => host(t))]);
    hubConnection.set(opts.connection ?? { state: 'standalone' });
    if (opts.preflight) {
      const toHost = opts.targets[0];
      putPreflightForTest(seed(toHost, opts.preflight));
    }
    transferSheetFor.set(pfSource.id);
    return render(TransferSheet);
  }

  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(requestPreflight).mockClear();
    selectSession(null);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('asks for a preview for the selected target', async () => {
    renderSetup({ targets: ['beta', 'gamma'] });
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    expect(requestPreflight).toHaveBeenLastCalledWith(7, 'beta');
  });

  it('asks again when the target changes', async () => {
    const { getByTestId } = renderSetup({ targets: ['beta', 'gamma'] });
    await fireEvent.change(getByTestId('move-target'), { target: { value: 'gamma' } });
    expect(requestPreflight).toHaveBeenLastCalledWith(7, 'gamma');
  });

  it.each(['loading', 'refused', 'stale'] as const)('keeps Transfer enabled while %s', (state) => {
    const { getByTestId } = renderSetup({ targets: ['beta'], preflight: state });
    expect((getByTestId('confirm-move') as HTMLButtonElement).disabled).toBe(false);
  });

  it('renders what would travel and what would be left behind, with reasons', () => {
    const { getByTestId } = renderSetup({ targets: ['beta'], preflight: 'ready' });
    const view = getByTestId('transfer-preflight').textContent ?? '';
    expect(view).toContain('src/lib.rs'); // a dirty entry
    expect(view).toContain('.env'); // an ignored file carried
    expect(view).toContain('node_modules'); // one left behind…
    expect(view).toMatch(/too large|deny/i); // …and why
  });

  it('shows a refusal in the words the failure view would use', () => {
    const { getByTestId } = renderSetup({ targets: ['beta'], preflight: 'refused' });
    expect(getByTestId('transfer-preflight-refusal').textContent).toContain(
      describeMoveError(refusalFixture, 'failed', 'beta', null).what,
    );
  });

  it('names what it cannot know', () => {
    const { getByTestId } = renderSetup({ targets: ['beta'], preflight: 'ready' });
    expect(getByTestId('transfer-preflight-unknowns').textContent).toMatch(/bundle/i);
  });

  it('shows the age of a stale preview', () => {
    const { getByTestId } = renderSetup({ targets: ['beta'], preflight: 'stale' });
    expect(getByTestId('transfer-preflight-age').textContent).toMatch(/\d+\s*s/);
  });

  // The safety guard added after the brief: a hub that has not yet passed
  // the `ready`-frame contract check must never see a preview request — an
  // old hub would silently treat `dry_run` as a real move. No request while
  // connecting (or reconnecting/offline); one request once connected.
  it('requests nothing while the hub is still connecting, then asks once connected', async () => {
    renderSetup({ targets: ['beta'], connection: { state: 'connecting' } });
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    expect(requestPreflight).not.toHaveBeenCalled();
    hubConnection.set({ state: 'connected' });
    await tick();
    expect(requestPreflight).toHaveBeenLastCalledWith(7, 'beta');
  });
});
