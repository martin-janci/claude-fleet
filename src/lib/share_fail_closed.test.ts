// F2d: not knowing is not permission.
//
// Four surfaces had each hand-rolled the same escape hatch — resolve the row a
// write names out of `$sessions`, and if it is not there, allow the write. On a
// standalone desktop that is harmless (the master owns every row), and on a
// paired one it is the whole bug: the hub fences rows this person may not see
// off the stream, so "not in `$sessions`" means *someone else's, or gone* —
// exactly the two cases a kill, a take-over or a cancel must not be offered for.
//
// `share.ts::sessionIdActionBlocked` is the one rule now: fail closed with
// `UNKNOWN_SESSION_REASON` on a fleet this client does not own, `null` on a
// standalone one. Every test below therefore comes in three parts:
//
//   1. the UNRESOLVABLE row on a PAIRED desktop — refused;
//   2. the same row on a STANDALONE desktop — allowed, because a single-user
//      install must be untouched;
//   3. the OWNER's positive control, so a gate that refused everything could
//      not pass.
//
// `share_sweep.test.ts` cannot reach any of this: it proves a gate was thought
// about at a call site, never that control flow passes through one. These are
// the behavioural half.
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';

import { resetAccessForTests, setMyGrants } from './access';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { sessions } from './sessions';
import { session } from './hosts_fixture';
import { tasks, type TaskRow } from './tasks';
import { EMPTY_REPORT, tidyReport, type TidyCandidate } from './tidy';
import { toasts } from './toasts';
import { sessionFocus } from './session_focus';
import type { ReviewItem } from './work_view';
import {
  moves,
  putRunForTest,
  resetMovesForTest,
  resolveMoveRun,
  retryMove,
  cancelWait,
  type MoveRun,
} from './moves';
import { MOVE_STEPS } from './moveProgress';
import { UNKNOWN_SESSION_REASON, sessionIdActionBlocked } from './share';
import TidyReview from './TidyReview.svelte';
import WorkReview from './WorkReview.svelte';
import TasksPanel from './TasksPanel.svelte';
import NameWorkDialog from './NameWorkDialog.svelte';
import ForkSheet from './ForkSheet.svelte';
import { applySessionRename } from './session_rename';

/** A desktop that is a window onto somebody else's fleet — the only mode in
 *  which anything here is refused at all (`access.ts::sessionAccess` rule 1). */
const PAIRED: HubStatus = {
  ...STANDALONE,
  remote: true,
  url: 'https://fleet.example.com',
  configured_url: 'https://fleet.example.com',
};

const ME = 5;
const dis = (el: Element) => (el as HTMLButtonElement | HTMLInputElement).disabled;

function calls(cmd: string) {
  return vi.mocked(invoke).mock.calls.filter((c) => c[0] === cmd);
}
async function flush() {
  for (let i = 0; i < 8; i++) await tick();
}

function paired() {
  hubStatus.set({ ...PAIRED });
  hubConnection.set({ state: 'connected' });
  setMyGrants(ME, []);
}
function standalone() {
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  resetAccessForTests();
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async () => null);
  resetAccessForTests();
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  sessions.set([]);
  tasks.set([]);
  toasts.set([]);
  sessionFocus.set(null);
  tidyReport.set(EMPTY_REPORT);
});

afterEach(() => {
  standalone();
  sessions.set([]);
  tasks.set([]);
});

// ── the predicate itself ──────────────────────────────────────────────────

describe('the one rule the four surfaces now share', () => {
  it('answers UNKNOWN_SESSION_REASON on a paired desktop and null on a standalone one', () => {
    // Pinned here as well as at each surface: a change to this sentence or to
    // which branch a missing row takes would otherwise be four separate
    // failures with no single place saying what the rule is.
    sessions.set([]);
    hubStatus.set({ ...PAIRED });
    setMyGrants(ME, []);
    // The last three are F2e's: the gate found three surfaces still resolving
    // the row by hand, and they reach this predicate through the same rule.
    const actions = [
      'tidy_apply',
      'decide_work_batch',
      'cancel_task',
      'move_session',
      'name_session_work',
      'rewind_conversation',
      'rename_session',
    ] as const;
    for (const action of actions) {
      expect(sessionIdActionBlocked(404, action)).toBe(UNKNOWN_SESSION_REASON);
      expect(sessionIdActionBlocked(null, action)).toBe(UNKNOWN_SESSION_REASON);
    }
    standalone();
    for (const action of actions) {
      expect(sessionIdActionBlocked(404, action)).toBeNull();
      expect(sessionIdActionBlocked(null, action)).toBeNull();
    }
  });
});

// ── 1. TidyReview: the hatch included SAFE KILL ───────────────────────────

describe('TidyReview: a candidate whose row this client cannot resolve', () => {
  const cand = (id: number, over: Partial<TidyCandidate> = {}): TidyCandidate => ({
    session_id: id,
    link_id: 100 + id,
    host_alias: 'h',
    tmux_name: `s${id}`,
    reason: 'idle_unlinked',
    action: 'safe_kill',
    since: 0,
    idle_secs: 5 * 3600,
    key: `ABC-${id}`,
    item_status: 'Done',
    branch: `abc-${id}`,
    ...over,
  });

  let candidates: TidyCandidate[] = [];

  beforeEach(() => {
    candidates = [cand(21)];
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case 'work_tidy':
          return { ...EMPTY_REPORT, candidates };
        case 'work_reopened':
          return [];
        case 'tidy_apply':
          return { results: [] };
        default:
          return null;
      }
    });
  });

  async function openSheet() {
    render(TidyReview);
    await waitFor(() =>
      expect(vi.mocked(invoke).mock.calls.some((c) => c[0] === 'work_reopened')).toBe(true),
    );
    await flush();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await flush();
  }

  it('is NOT tidyable on a paired desktop — Safe kill included', async () => {
    // The hatch was `mayTidy`'s `|| !rowById.has(c.session_id)`, and
    // `tidy_apply` can SAFE KILL, so an unresolvable candidate was a kill this
    // client could fire on a session it cannot even see.
    paired();
    await openSheet();
    expect(dis(screen.getByTestId('tidy-check'))).toBe(true);
    expect(dis(screen.getByTestId('tidy-safe-kill'))).toBe(true);
    expect(dis(screen.getByTestId('tidy-keep'))).toBe(true);
    expect(screen.getByTestId('tidy-safe-kill').title).toBe(UNKNOWN_SESSION_REASON);
    expect(screen.getByTestId('tidy-not-mine')).toBeInTheDocument();
    // …and reached directly: Safe kill is two clicks, ↵ applies the sheet.
    await fireEvent.click(screen.getByTestId('tidy-safe-kill'));
    await fireEvent.click(screen.getByTestId('tidy-safe-kill'));
    await fireEvent.keyDown(screen.getByTestId('tidy-sheet'), { key: 'Enter' });
    await flush();
    expect(calls('tidy_apply')).toEqual([]);
  });

  it('is tidyable on a standalone desktop, where the master owns every row', async () => {
    standalone();
    await openSheet();
    expect(dis(screen.getByTestId('tidy-safe-kill'))).toBe(false);
    expect(screen.queryByTestId('tidy-not-mine')).toBeNull();
    // `idle_unlinked` is never preselected, so the tick is the proof the row is
    // offered at all; ↵ then applies the sheet.
    await fireEvent.click(screen.getByTestId('tidy-check'));
    await flush();
    await fireEvent.keyDown(screen.getByTestId('tidy-sheet'), { key: 'Enter' });
    await flush();
    expect(calls('tidy_apply')).toHaveLength(1);
  });

  it('and the owner of a RESOLVABLE row keeps the sheet on a paired desktop', async () => {
    // The positive control for the paired case: failing closed must not be
    // "refuse everything once a hub is in the picture".
    sessions.set([session('h', 's21', { id: 21, owner_person_id: ME })]);
    paired();
    await openSheet();
    expect(dis(screen.getByTestId('tidy-safe-kill'))).toBe(false);
    expect(screen.queryByTestId('tidy-not-mine')).toBeNull();
    await fireEvent.click(screen.getByTestId('tidy-check'));
    await flush();
    await fireEvent.keyDown(screen.getByTestId('tidy-sheet'), { key: 'Enter' });
    await flush();
    expect(calls('tidy_apply')).toHaveLength(1);
  });
});

// ── 2. WorkReview: the hatch covered every per-session work write ──────────

describe('WorkReview: an item whose row this client cannot resolve', () => {
  const item = (over: Partial<ReviewItem> = {}): ReviewItem => ({
    review_id: 'link:42',
    kind: 'suggestion',
    session_id: 31,
    session_name: 'api',
    host: 'mefistos',
    link_id: 42,
    link_version: 2,
    task: { task_id: 'item:12', key: 'ABC-12', title: 'Login fails' },
    why: ['branch abc-12 since 09:05 · R3'],
    strength: 'strong',
    rule: 'R3',
    preselected: false,
    alternatives: [],
    created_at: 1790000000,
    ...over,
  });

  beforeEach(() => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case 'work_review':
          return { items: [item()], total: 1, next_cursor: null };
        case 'confirm_session_work':
          return session('mefistos', 'api', { id: 31 });
        case 'decide_work_batch':
          return { results: [] };
        case 'work_session_tasks':
          return { session_id: 31, links: [] };
        default:
          return null;
      }
    });
  });

  it('cannot be decided on a paired desktop, by button or by keyboard', async () => {
    paired();
    render(WorkReview);
    await flush();
    const row = screen.getAllByTestId('work-review-item')[0];
    expect(dis(within(row).getByTestId('work-review-confirm'))).toBe(true);
    expect(dis(within(row).getByTestId('work-review-reject'))).toBe(true);
    expect(dis(within(row).getByTestId('work-review-pick'))).toBe(true);
    const list = screen.getByRole('list', { name: /Review items/i });
    await fireEvent.keyDown(list, { key: 'y' });
    await fireEvent.keyDown(list, { key: 'x' });
    await flush();
    expect(calls('confirm_session_work')).toEqual([]);
    expect(calls('decide_work_batch')).toEqual([]);
  });

  it('can be decided on a standalone desktop', async () => {
    standalone();
    render(WorkReview);
    await flush();
    const row = screen.getAllByTestId('work-review-item')[0];
    expect(dis(within(row).getByTestId('work-review-confirm'))).toBe(false);
    await fireEvent.click(within(row).getByTestId('work-review-confirm'));
    await flush();
    expect(calls('confirm_session_work')).toHaveLength(1);
  });

  it('and the owner of a resolvable row decides it on a paired desktop too', async () => {
    sessions.set([session('mefistos', 'api', { id: 31, owner_person_id: ME })]);
    paired();
    render(WorkReview);
    await flush();
    const row = screen.getAllByTestId('work-review-item')[0];
    expect(dis(within(row).getByTestId('work-review-confirm'))).toBe(false);
    await fireEvent.click(within(row).getByTestId('work-review-confirm'));
    await flush();
    expect(calls('confirm_session_work')).toHaveLength(1);
  });
});

// ── 3. TasksPanel: `$tasks` is the FLEET-WIDE read ────────────────────────

describe('TasksPanel: a task party this client holds no row for', () => {
  const task = (over: Partial<TaskRow> = {}): TaskRow => ({
    id: 1,
    requester_session_id: 41,
    worker_session_id: 42,
    prompt: 'fix the flaky test',
    state: 'running',
    result: null,
    error: null,
    created_at: 100,
    started_at: 101,
    finished_at: null,
    ...over,
  });
  const btn = () => screen.getByTestId('task-cancel') as HTMLButtonElement;

  beforeEach(() => {
    vi.mocked(invoke).mockImplementation(async () => task({ state: 'cancelled' }));
  });

  it('cannot be cancelled on a paired desktop when NEITHER party resolves', async () => {
    // The hatch's worst case and the reason this panel is in the list:
    // `list_tasks` is fleet-wide, so a paired desktop is handed tasks between
    // sessions it holds no rows for — the hatch opened exactly there.
    tasks.set([task()]);
    paired();
    render(TasksPanel, {});
    await flush();
    expect(btn().disabled).toBe(true);
    expect(btn().title).toBe(UNKNOWN_SESSION_REASON);
    await fireEvent.click(btn());
    await flush();
    expect(screen.queryByTestId('confirm-cancel-task')).toBeNull();
    expect(calls('cancel_task')).toEqual([]);
  });

  it('refuses when ONE party resolves and the other does not', async () => {
    // The half-resolvable case: the requester is this person's, the worker is a
    // row the hub never sent. A cancel ends the task on both timelines.
    tasks.set([task()]);
    sessions.set([session('local', 'ctl', { id: 41, owner_person_id: ME })]);
    paired();
    render(TasksPanel, {});
    await flush();
    expect(btn().disabled).toBe(true);
    expect(btn().title).toBe(UNKNOWN_SESSION_REASON);
  });

  it('a party the task does not name at all is not a party, and does not refuse', async () => {
    // `requester_session_id` is nullable (`ON DELETE SET NULL`): "there is no
    // second party" is a different fact from "there is one and this app cannot
    // see whose it is", and only the second is a refusal.
    tasks.set([task({ requester_session_id: null })]);
    sessions.set([session('local', 'worker-a', { id: 42, owner_person_id: ME })]);
    paired();
    render(TasksPanel, {});
    await flush();
    expect(btn().disabled).toBe(false);
  });

  it('…but a task with NO party at all still refuses on a paired desktop', async () => {
    tasks.set([task({ requester_session_id: null, worker_session_id: null })]);
    paired();
    render(TasksPanel, {});
    await flush();
    expect(btn().disabled).toBe(true);
  });

  it('is cancellable on a standalone desktop', async () => {
    tasks.set([task()]);
    standalone();
    render(TasksPanel, {});
    await flush();
    expect(btn().disabled).toBe(false);
    await fireEvent.click(btn());
    await fireEvent.click(screen.getByTestId('confirm-cancel-task'));
    await flush();
    expect(calls('cancel_task')).toHaveLength(1);
  });

  it('and the owner of both resolvable parties cancels on a paired desktop', async () => {
    tasks.set([task()]);
    sessions.set([
      session('local', 'ctl', { id: 41, owner_person_id: ME }),
      session('local', 'worker-a', { id: 42, owner_person_id: ME }),
    ]);
    paired();
    render(TasksPanel, {});
    await flush();
    expect(btn().disabled).toBe(false);
    await fireEvent.click(btn());
    await fireEvent.click(screen.getByTestId('confirm-cancel-task'));
    await flush();
    expect(calls('cancel_task')).toHaveLength(1);
  });
});

// ── 4. moves.ts: the lifecycle outlives its row by construction ───────────

describe('the move lifecycle: a run whose row has left the store', () => {
  function run(over: Partial<MoveRun> = {}): MoveRun {
    return {
      sessionId: 51,
      sessionName: 'dev',
      fromHost: 'alpha',
      toHost: 'beta',
      keepSource: false,
      origin: 'local',
      steps: MOVE_STEPS.map((step) => ({ step, state: 'pending' as const, detail: null })),
      status: 'failed',
      report: null,
      error: null,
      resolveError: null,
      startedAt: Date.now(),
      settledAt: null,
      cleanTarget: false,
      forceCrossOrg: false,
      attempt: 1,
      resolving: false,
      awaitingStart: false,
      deadlineUnix: null,
      waitEnded: null,
      waitRefusal: null,
      ...over,
    };
  }
  /** A partial run whose report names the target `resolve_move` would act on. */
  const partial = (targetId: number) =>
    run({
      status: 'partial',
      report: { target_session_id: targetId } as MoveRun['report'],
    });

  beforeEach(() => {
    resetMovesForTest();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'move_session' || cmd === 'resolve_move') {
        throw { code: 'E_TEST', message: 'not in this test' };
      }
      return null;
    });
  });
  afterEach(() => resetMovesForTest());

  it('retryMove and cancelWait send nothing on a paired desktop', async () => {
    // A run outlives its row by design: a `partial` adopted after a restart, a
    // `waiting` run on a host that went quiet, a `failed` run still on screen
    // after a reconcile dropped the session. The gate used to hand
    // `sessionActionBlocked` an `undefined` row, which answers "allowed".
    paired();
    putRunForTest(run());
    retryMove(51);
    putRunForTest(run({ status: 'waiting' }));
    cancelWait(51);
    await flush();
    expect(calls('move_session')).toEqual([]);
  });

  it('…and both still work on a standalone desktop', async () => {
    standalone();
    putRunForTest(run());
    retryMove(51);
    await flush();
    expect(calls('move_session')).toHaveLength(1);
  });

  it('resolveMoveRun asks about the TARGET, which is the session it acts on', async () => {
    // The reviewer's second finding: the gate asked about `sessionId` — the
    // run's KEY, i.e. the SOURCE — while `resolve_move` is handed the target
    // id. The source here is this person's and fully resolvable; the target is
    // a row this client holds nothing for, and Finish/Undo kill a live session.
    sessions.set([session('alpha', 'dev', { id: 51, owner_person_id: ME })]);
    paired();
    putRunForTest(partial(99));
    resolveMoveRun(51, 'finish');
    await flush();
    expect(calls('resolve_move')).toEqual([]);
    // Not silent: the sheet's Finish/Undo are gated on the SOURCE row, which is
    // fine here, so a silent refusal would be a button that does nothing.
    expect(get(moves).get(51)?.resolveError?.message).toBe(UNKNOWN_SESSION_REASON);
    expect(get(moves).get(51)?.resolving).toBe(false);
    expect(get(moves).get(51)?.status).toBe('partial');
  });

  it('…and resolves when the target row is this person’s', async () => {
    sessions.set([
      session('alpha', 'dev', { id: 51, owner_person_id: ME }),
      session('beta', 'dev', { id: 99, owner_person_id: ME }),
    ]);
    paired();
    putRunForTest(partial(99));
    resolveMoveRun(51, 'finish');
    await flush();
    expect(calls('resolve_move')).toHaveLength(1);
  });

  it('…and refuses a target shared with this client at drive', async () => {
    // `resolve_move` is `own` in `share.ts::SESSION_TIER`: Finish and Undo each
    // kill a live session, so a driver is barred as well as a watcher.
    sessions.set([
      session('alpha', 'dev', { id: 51, owner_person_id: ME }),
      session('beta', 'dev', { id: 99, owner_person_id: 42 }),
    ]);
    hubStatus.set({ ...PAIRED });
    hubConnection.set({ state: 'connected' });
    setMyGrants(ME, [{ session_id: 99, level: 'drive' }]);
    putRunForTest(partial(99));
    resolveMoveRun(51, 'undo');
    await flush();
    expect(calls('resolve_move')).toEqual([]);
    expect(get(moves).get(51)?.resolveError?.message).toMatch(/only the session’s owner/i);
  });

  it('…and a standalone desktop resolves a target it holds no row for', async () => {
    standalone();
    putRunForTest(partial(99));
    resolveMoveRun(51, 'finish');
    await flush();
    expect(calls('resolve_move')).toHaveLength(1);
  });
});

// ── 4b. preflight.ts: a preview is a write too ────────────────────────────

describe('requestPreflight: the preview of a move it cannot vouch for', () => {
  beforeEach(() => {
    // `previewMove` reads `.kind` off the answer, so `null` would be an
    // unhandled rejection in whichever test happened to be running next.
    vi.mocked(invoke).mockImplementation(async (cmd: string) =>
      cmd === 'move_session' ? { kind: 'preview', warnings: [], unknowns: [] } : null,
    );
  });

  it('asks for nothing on a paired desktop when the row is not in the store', async () => {
    // `move_session { preview: true }` on somebody's session: the gate had the
    // same hand-rolled `find` and the same `undefined` ⇒ allowed answer.
    const { requestPreflight, resetPreflightsForTest } = await import('./preflight');
    resetPreflightsForTest();
    sessions.set([]);
    paired();
    requestPreflight(71, 'beta');
    await new Promise((r) => setTimeout(r, 400));
    expect(calls('move_session')).toEqual([]);
    resetPreflightsForTest();
  });

  it('…and asks on a standalone desktop, and for the owner’s own row', async () => {
    const { requestPreflight, resetPreflightsForTest } = await import('./preflight');
    resetPreflightsForTest();
    sessions.set([]);
    standalone();
    requestPreflight(71, 'beta');
    await new Promise((r) => setTimeout(r, 400));
    expect(calls('move_session')).toHaveLength(1);
    resetPreflightsForTest();

    vi.mocked(invoke).mockClear();
    sessions.set([session('alpha', 'dev', { id: 71, owner_person_id: ME })]);
    paired();
    requestPreflight(71, 'beta');
    await new Promise((r) => setTimeout(r, 400));
    expect(calls('move_session')).toHaveLength(1);
    resetPreflightsForTest();
  });
});

// ── 5. the outbox: the two enforcement points, end to end ─────────────────
//
// The round that added these is the one that showed what `share_sweep.test.ts`
// cannot do: a verifier deleted BOTH enforcement points in `outbox.ts` and the
// sweep stayed green, because the gate identifier is still lexically present
// elsewhere in the file. `share_write_paths.test.ts` pins each point through an
// INJECTED `blocked` dep; these four go through the REAL wiring — the app's own
// `outbox`, `sessionIdActionBlocked`, and `$sessions` — so the dep being pointed
// at the right predicate is part of what is measured.
//
// Which deletion each test catches:
//
//   - *never uploads or sends* → the gate in `pump`. Without it the upload goes
//     out, and `upload_attachments` is `same_in_both`: the hub never sees it, so
//     the desktop is the only wall and a file would land on somebody's host.
//   - *leaves the store DURING the upload* → the gate in `sendOne`, after the
//     upload. Without it the prompt goes out: `pump` passed, legitimately,
//     seconds earlier.
describe('the outbox asks at the dispatch, with the real predicate', () => {
  const TARGET = { id: 61, host_alias: 'mefistos', tmux_name: 'api' };
  const TILE = {
    id: 'a1',
    path: '/tmp/a.txt',
    name: 'a.txt',
    size: 10,
    kind: 'text' as const,
    thumb: null,
    state: 'ready' as const,
    error: null,
    pasted: false,
  };
  const mine = () => session('mefistos', 'api', { id: 61, owner_person_id: ME });

  let box: typeof import('./outbox').outbox;
  /** `upload_attachments` answers only when the test says so. */
  let releaseUpload: (() => void) | null = null;

  beforeEach(async () => {
    box = (await import('./outbox')).outbox;
    box.resetForTests();
    releaseUpload = null;
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'upload_attachments') {
        await new Promise<void>((r) => (releaseUpload = r));
        return ['/remote/a.txt'];
      }
      return null;
    });
  });
  afterEach(() => box.resetForTests());

  const msgs = () => get(box.store).msgs[TARGET.id] ?? [];

  it('never uploads or sends for a row a paired desktop cannot see', async () => {
    sessions.set([]);
    paired();
    box.enqueue(TARGET, { kind: 'prompt', text: 'look', attachments: [TILE] });
    await flush();
    expect(calls('upload_attachments')).toEqual([]);
    expect(calls('send_prompt')).toEqual([]);
    expect(msgs()[0]).toMatchObject({ state: 'failed', error: UNKNOWN_SESSION_REASON });
    // Retryable: the moment the row is back in the list, Retry works.
    expect(msgs()[0].retryable).toBe(true);
  });

  it('a row that leaves the store DURING the upload stops the prompt that follows', async () => {
    sessions.set([mine()]);
    paired();
    box.enqueue(TARGET, { kind: 'prompt', text: 'look', attachments: [TILE] });
    await flush();
    // `pump` let it through, correctly: the row was there and it was ours.
    expect(calls('upload_attachments')).toHaveLength(1);
    // A reconcile drops the row (or the hub stops sending it) while the upload
    // is in flight — seconds, for a real file.
    sessions.set([]);
    releaseUpload?.();
    await flush();
    expect(calls('send_prompt')).toEqual([]);
    expect(msgs()[0]).toMatchObject({ state: 'failed', error: UNKNOWN_SESSION_REASON });
  });

  it('a standalone desktop sends for a row that is not in the list at all', async () => {
    sessions.set([]);
    standalone();
    box.enqueue(TARGET, { kind: 'prompt', text: 'hello' });
    await flush();
    expect(calls('send_prompt')).toHaveLength(1);
  });

  it('and the owner’s own message goes out on a paired desktop', async () => {
    sessions.set([mine()]);
    paired();
    box.enqueue(TARGET, { kind: 'prompt', text: 'hello' });
    await flush();
    expect(calls('send_prompt')).toHaveLength(1);
  });
});


// ── 6. NameWorkDialog: the hatch F2d was told to delete and did not ────────
//
// `writableIds` was `[...chosen].filter((id) => writableRowIds.has(id) ||
// !targetRows.some((s) => s.id === id))` — an id with no row in `targetRows`
// passed the filter and was written, with a comment citing `TidyReview` and
// `WorkReview`, the two surfaces F2d had already fixed. The write is
// `name_session_work` + `link_session_work`: work-graph writes onto a session
// this client cannot see the owner of.
describe('NameWorkDialog: a ticked session whose row this client cannot resolve', () => {
  const nameProps = (ids: readonly number[]) => ({
    target: { mode: 'name' as const, sessions: ids.map((id) => ({ id, label: `s${id}` })) },
    onclose: vi.fn(),
  });
  const submit = () => screen.getByTestId('name-work-submit') as HTMLButtonElement;
  const named = (id: number) => session('h', `s${id}`, { id, owner_person_id: ME });

  async function type(value: string) {
    await fireEvent.input(screen.getByTestId('name-work-title'), { target: { value } });
    await flush();
  }

  it('is not named for on a paired desktop, and nothing is written', async () => {
    sessions.set([]);
    paired();
    render(NameWorkDialog, { props: nameProps([404]) });
    await flush();
    await type('Ops cleanup');
    expect(dis(submit())).toBe(true);
    expect(screen.getByTestId('name-work-blocked').textContent).toBe(UNKNOWN_SESSION_REASON);
    // Reached directly as well: the dialog stays open, so the submit handler
    // re-asks rather than trusting the disabled attribute.
    await fireEvent.click(submit());
    await flush();
    expect(calls('name_session_work')).toEqual([]);
    expect(calls('link_session_work')).toEqual([]);
  });

  it('is left OUT of a batch rather than failing the whole dialog', async () => {
    // The per-id narrowing: one unresolvable session must not cost the owner
    // the session beside it.
    sessions.set([named(21)]);
    paired();
    vi.mocked(invoke).mockImplementation(async () => named(21));
    render(NameWorkDialog, { props: nameProps([21, 404]) });
    await flush();
    await type('Ops cleanup');
    expect(screen.getByTestId('name-work-not-mine').textContent).toMatch(/1 of the ticked/);
    expect(dis(submit())).toBe(false);
    await fireEvent.click(submit());
    await waitFor(() => expect(calls('name_session_work')).toHaveLength(1));
    expect(calls('name_session_work')[0][1]).toMatchObject({ args: { session_id: 21 } });
    expect(calls('link_session_work')).toEqual([]);
  });

  it('is named for on a standalone desktop, where the master owns every row', async () => {
    sessions.set([]);
    standalone();
    vi.mocked(invoke).mockImplementation(async () => named(404));
    render(NameWorkDialog, { props: nameProps([404]) });
    await flush();
    await type('Ops cleanup');
    expect(dis(submit())).toBe(false);
    expect(screen.queryByTestId('name-work-blocked')).toBeNull();
    await fireEvent.click(submit());
    await waitFor(() => expect(calls('name_session_work')).toHaveLength(1));
  });

  it('and the owner of a RESOLVABLE row keeps it on a paired desktop', async () => {
    sessions.set([named(21)]);
    paired();
    vi.mocked(invoke).mockImplementation(async () => named(21));
    render(NameWorkDialog, { props: nameProps([21]) });
    await flush();
    await type('Ops cleanup');
    expect(dis(submit())).toBe(false);
    expect(screen.queryByTestId('name-work-not-mine')).toBeNull();
    await fireEvent.click(submit());
    await waitFor(() => expect(calls('name_session_work')).toHaveLength(1));
  });
});

// ── 7. ForkSheet: the sheet IS the confirmation ────────────────────────────
//
// `const row = $derived($sessions.find((s) => s.id === sessionId))` fed
// `$sessionBlocked(row, 'rewind_conversation')`, and a miss handed it an
// `undefined` — which answers `null` = allowed, so `blocked` collapsed to the
// hub half alone. `rewind_conversation` is `own`: a fork leaves a permanent
// verbatim copy of the transcript and a branch on the owner's host, and there is
// no second dialog behind this one.
describe('ForkSheet: no row for the id it was handed', () => {
  const forkProps = { sessionId: 404, anchor: 'a1', suggestedName: 'f', onclose: vi.fn() };
  const confirm = () => screen.getByTestId('fork-confirm') as HTMLButtonElement;
  const mine = () => session('h', 'api', { id: 404, owner_person_id: ME });

  beforeEach(() => {
    vi.mocked(invoke).mockImplementation(async () => mine());
  });

  it('is not forkable on a paired desktop, and the copy is never made', async () => {
    sessions.set([]);
    paired();
    render(ForkSheet, { props: forkProps });
    await flush();
    expect(dis(confirm())).toBe(true);
    expect(screen.getByText(UNKNOWN_SESSION_REASON)).toBeTruthy();
    await fireEvent.click(confirm());
    await flush();
    expect(calls('rewind_conversation')).toEqual([]);
  });

  it('is forkable on a standalone desktop with no row for the id at all', async () => {
    sessions.set([]);
    standalone();
    render(ForkSheet, { props: forkProps });
    await flush();
    expect(dis(confirm())).toBe(false);
    await fireEvent.click(confirm());
    await flush();
    expect(calls('rewind_conversation')).toHaveLength(1);
  });

  it('and the owner of a RESOLVABLE row forks on a paired desktop', async () => {
    sessions.set([mine()]);
    paired();
    render(ForkSheet, { props: forkProps });
    await flush();
    expect(dis(confirm())).toBe(false);
    await fireEvent.click(confirm());
    await flush();
    expect(calls('rewind_conversation')).toHaveLength(1);
  });
});

// ── 8. session_rename: the funnel the sweep trusts for both actions ────────
//
// `share_sweep.test.ts::funnelGated` names this module as the gate for
// `rename_session` (`own`) and `set_friendly_name` (`drive`), so nothing else in
// the sweep is asked to hold that line — and the editor also opens on a
// DOUBLE-CLICK, which consults no button. Its `accessBlocked` resolved the row
// by hand and failed open on a miss.
describe('session_rename: a rename of a row this client cannot resolve', () => {
  const target = { id: 404, host_alias: 'h', tmux_name: 'api', friendly_name: 'old' };
  const mine = () => session('h', 'api', { id: 404, owner_person_id: ME });

  beforeEach(() => {
    vi.mocked(invoke).mockImplementation(async () => mine());
  });

  it('is refused on a paired desktop, in both modes, with nothing sent', async () => {
    sessions.set([]);
    paired();
    for (const [mode, value] of [
      ['label', 'new label'],
      ['tmux', 'new-name'],
    ] as const) {
      const outcome = await applySessionRename(target, mode, value);
      expect(outcome.kind, mode).toBe('error');
      if (outcome.kind === 'error') expect(outcome.error.message).toBe(UNKNOWN_SESSION_REASON);
    }
    expect(calls('set_session_friendly_name')).toEqual([]);
    expect(calls('rename_session')).toEqual([]);
  });

  it('is allowed on a standalone desktop, where the master owns every row', async () => {
    sessions.set([]);
    standalone();
    expect((await applySessionRename(target, 'label', 'new label')).kind).toBe('ok');
    expect((await applySessionRename(target, 'tmux', 'new-name')).kind).toBe('ok');
    expect(calls('set_session_friendly_name')).toHaveLength(1);
    expect(calls('rename_session')).toHaveLength(1);
  });

  it('and the owner of a RESOLVABLE row renames on a paired desktop', async () => {
    sessions.set([mine()]);
    paired();
    expect((await applySessionRename(target, 'label', 'new label')).kind).toBe('ok');
    expect((await applySessionRename(target, 'tmux', 'new-name')).kind).toBe('ok');
    expect(calls('set_session_friendly_name')).toHaveLength(1);
    expect(calls('rename_session')).toHaveLength(1);
  });
});
