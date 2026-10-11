// The task board (sprints design 2026-09-28 §6c): one `work_tree` read with
// the Work view's filters (archived on, status off), columns by status, a
// native card moved by drag or ← →, a tracker card refused on the card.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WorkBoard from './WorkBoard.svelte';
import { expectAccessible } from './a11y_check';
import { link, task } from './work_view_fixture';
import { selectedTaskId, sidebarView, workViewFilters, type WorkTreePage } from './work_view';
import { activeHintId, hintDef, markSeen, resetHints } from './hints';
import { onboardingWelcomed } from './onboarding';
import { boardScope } from './work_buckets';

const NOW = Math.floor(Date.now() / 1000);
const page = (): WorkTreePage => ({
  tasks: [
    task({
      task_id: 'item:1',
      item_id: 1,
      key: 'TASK-1',
      title: 'Write notes',
      kind: 'local',
      origin: 'manual',
      status_category: 'todo',
      status_name: null,
      counts: { active: 0, ended: 0, suggested: 0 },
      sessions: [],
      last_activity_at: NOW,
    }),
    task({
      task_id: 'item:12',
      item_id: 12,
      key: 'ABC-12',
      title: 'Login fails',
      status_category: 'in_progress',
      sessions: [link({ name: 'abc-12 login', host: 'mefistos' })],
      last_activity_at: NOW,
    }),
  ],
  groups: [],
  orgs: [],
  trackers: [],
  total: 2,
  next_cursor: null,
});
const flush = async () => {
  for (let i = 0; i < 6; i++) {
    await Promise.resolve();
    await tick();
  }
};
const calls = (cmd: string) => (invoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === cmd);
const column = (c: string) => screen.getByTestId(`work-board-column-${c}`);
const card = (title: string) =>
  screen.getAllByTestId('work-board-card').find((el) => el.textContent?.includes(title)) as HTMLElement;

let setStatus: (args: { item_id: number; status: string }) => unknown;

beforeEach(() => {
  boardScope.set('all');
  workViewFilters.set({ tracker: 1, status: 'done' });
  setStatus = (a) => ({ id: a.item_id, source: 'local', key: 'TASK-1', title: 'Write notes', status_category: a.status });
  (invoke as ReturnType<typeof vi.fn>).mockReset();
  (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, payload?: { args: never }) => {
    if (cmd === 'work_tree') return page();
    if (cmd === 'set_work_status') return setStatus(payload!.args);
    return null;
  });
});

describe('WorkBoard', () => {
  it('says how to move a task once, as a hint, not as a line on every visit (redesign 1.4)', async () => {
    resetHints();
    onboardingWelcomed.set(true);
    render(WorkBoard);
    await flush();
    expect(screen.queryByTestId('work-board-hint')).toBeNull();
    expect(screen.getByTestId('work-board').querySelector('header')!.textContent).not.toContain('Drag a task');
    // The same words, offered once by the hint layer while the board is open…
    expect(get(activeHintId)).toBe('board-move');
    expect(hintDef('board-move')!.text).toContain('Drag a task to set its status');
    // …and gone for good once dismissed.
    markSeen('board-move');
    expect(get(activeHintId)).not.toBe('board-move');
    resetHints();
  });

  it('reads with the filters, archived on and the status filter off', async () => {
    render(WorkBoard);
    await flush();
    const f = (calls('work_tree')[0][1] as { args: { filters: Record<string, unknown> } }).args.filters;
    expect(f.tracker).toBe(1);
    expect(f.archived).toBe(true);
    expect(f.status).toBeUndefined();
  });

  it('puts each card in its status column and shows the live session and host', async () => {
    render(WorkBoard);
    await flush();
    expect(column('todo').textContent).toContain('Write notes');
    // ABC-12 is "In Review" in Jira: that status name is its own column.
    expect(column('doing:in review').textContent).toContain('Login fails');
    expect(column('doing:in review').querySelector('[data-testid="work-board-live"]')?.textContent).toContain(
      'abc-12 login · mefistos',
    );
  });

  it('says who has each card and when it is due ("You · Fri"); an overdue date is marked', async () => {
    const ymd = (d: Date) =>
      `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
    const day = (offset: number) => {
      const d = new Date();
      d.setDate(d.getDate() + offset);
      return d;
    };
    const p = page();
    p.tasks[0] = { ...p.tasks[0], assignees: ['Ana'], mine: false, due_at: ymd(day(1)) };
    p.tasks[1] = { ...p.tasks[1], assignees: ['Dana Dev'], mine: true, due_at: ymd(day(-3)) };
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => (cmd === 'work_tree' ? p : null));
    render(WorkBoard);
    await flush();
    const owner = (title: string) => card(title).querySelector('[data-testid="work-board-owner"]') as HTMLElement;
    expect(owner('Write notes').textContent).toBe('Ana · Tomorrow');
    expect(owner('Write notes').classList.contains('overdue')).toBe(false);
    expect(owner('Login fails').textContent).toMatch(/^You · [A-Z][a-z]{2} \d{1,2}$/);
    expect(owner('Login fails').classList.contains('overdue')).toBe(true);
  });

  it('dragging a native card to Doing sets its status and moves it at once', async () => {
    const elementFromPoint = vi.fn();
    Object.defineProperty(document, 'elementFromPoint', { value: elementFromPoint, configurable: true });
    render(WorkBoard);
    await flush();
    elementFromPoint.mockReturnValue(column('doing'));
    const c = card('Write notes');
    await fireEvent.pointerDown(c, { button: 0, clientX: 10, clientY: 10 });
    await fireEvent.pointerMove(window, { clientX: 300, clientY: 20 });
    await fireEvent.pointerUp(window, { clientX: 300, clientY: 20 });
    await flush();
    expect(calls('set_work_status')[0][1]).toEqual({ args: { item_id: 1, status: 'in_progress' } });
    expect(column('doing').textContent).toContain('Write notes');
    // The click that ends a drag does not open the task.
    selectedTaskId.set(null);
    await fireEvent.click(c);
    expect(get(selectedTaskId)).toBeNull();
  });

  it('a tracker card is refused on the card, and nothing is sent', async () => {
    render(WorkBoard);
    await flush();
    const c = card('Login fails');
    await fireEvent.keyDown(c, { key: 'ArrowRight' });
    await flush();
    expect(calls('set_work_status')).toHaveLength(0);
    expect(screen.getByTestId('work-board-card-error').textContent).toBe(
      "ABC-12's status belongs to Jira (acme). Change it there.",
    );
    expect(column('doing:in review').textContent).toContain('Login fails');
  });

  it('← → move a focused native card; a failed write puts it back and says why', async () => {
    setStatus = () => {
      throw { code: 'E_NOTFOUND', message: 'work item 1 not found' };
    };
    render(WorkBoard);
    await flush();
    await fireEvent.keyDown(card('Write notes'), { key: 'ArrowRight' });
    await flush();
    expect(calls('set_work_status')[0][1]).toEqual({ args: { item_id: 1, status: 'in_progress' } });
    expect(column('todo').textContent).toContain('Write notes');
    expect(screen.getByTestId('work-board-card-error').textContent).toContain('not found');
  });

  it("takes its columns from the tracker's status names, and a native drop there takes that status (redesign 6.1)", async () => {
    const elementFromPoint = vi.fn();
    Object.defineProperty(document, 'elementFromPoint', { value: elementFromPoint, configurable: true });
    render(WorkBoard);
    await flush();
    const heads = Array.from(screen.getByTestId('work-board').querySelectorAll('[data-board-column]')).map(
      (el) => el.getAttribute('data-board-column'),
    );
    expect(heads).toEqual(['todo', 'doing', 'doing:in review', 'done']);
    expect(column('doing:in review').textContent).toContain('In Review');
    expect(screen.getByTestId('work-board-note').textContent).toContain('Columns come from the tracker');
    elementFromPoint.mockReturnValue(column('doing:in review'));
    await fireEvent.pointerDown(card('Write notes'), { button: 0, clientX: 10, clientY: 10 });
    await fireEvent.pointerMove(window, { clientX: 300, clientY: 20 });
    await fireEvent.pointerUp(window, { clientX: 300, clientY: 20 });
    await flush();
    // Fleet's task has three statuses: the drop sets In progress and the card
    // sits in In progress, never in a column only Jira can put it in.
    expect(calls('set_work_status')[0][1]).toEqual({ args: { item_id: 1, status: 'in_progress' } });
    expect(column('doing').textContent).toContain('Write notes');
  });

  it('a click opens the task in the Work view', async () => {
    sidebarView.set('sessions');
    render(WorkBoard);
    await flush();
    await fireEvent.click(card('Write notes'));
    expect(get(selectedTaskId)).toBe('item:1');
    expect(get(sidebarView)).toBe('work');
  });

  it('closes from its button', async () => {
    const onclose = vi.fn();
    render(WorkBoard, { onclose });
    await flush();
    await fireEvent.click(screen.getByTestId('work-board-close'));
    expect(onclose).toHaveBeenCalledOnce();
  });

  it('is accessible', async () => {
    const { container } = render(WorkBoard);
    await flush();
    await expectAccessible(container);
  });
});

describe('WorkBoard editing', () => {
  it("opens the edit dialog from a native card's ✎ or E, and offers none on a ticket", async () => {
    render(WorkBoard, { props: { debounceMs: 0 } });
    await flush();
    const edits = screen.getAllByTestId('work-board-card-edit');
    expect(edits).toHaveLength(1);
    expect(edits[0].getAttribute('aria-label')).toBe('Edit Write notes');
    await fireEvent.click(edits[0]);
    await flush();
    expect(screen.getByTestId('edit-task-dialog')).toBeTruthy();
    expect(calls('work_task')).toEqual([['work_task', { args: { task_id: 'item:1' } }]]);
  });

  it('E on a focused tracker card opens nothing', async () => {
    render(WorkBoard, { props: { debounceMs: 0 } });
    await flush();
    await fireEvent.keyDown(card('Login fails'), { key: 'e' });
    await flush();
    expect(screen.queryByTestId('edit-task-dialog')).toBeNull();
    await fireEvent.keyDown(card('Write notes'), { key: 'e' });
    await flush();
    expect(screen.getByTestId('edit-task-dialog')).toBeTruthy();
  });
});

// Review round 13 (step 10.6): filters that hide every task say so and offer
// Clear filters, as the list does; an unfiltered empty board does not.
describe('WorkBoard with filters that match nothing (review r13)', () => {
  it('names the filters and clears them', async () => {
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) =>
      cmd === 'work_tree' ? { ...page(), tasks: [], total: 0 } : null,
    );
    workViewFilters.set({ query: 'nothing-like-this' });
    render(WorkBoard);
    await flush();
    expect(screen.getByTestId('work-board-no-match').textContent).toContain('nothing-like-this');
    await fireEvent.click(screen.getByTestId('work-board-clear'));
    expect(get(workViewFilters)).toEqual({});
  });

  it('an empty board with no filters keeps its columns quiet', async () => {
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) =>
      cmd === 'work_tree' ? { ...page(), tasks: [], total: 0 } : null,
    );
    workViewFilters.set({});
    render(WorkBoard);
    await flush();
    expect(screen.queryByTestId('work-board-no-match')).toBeNull();
  });
  describe('scoped to a sprint (sprints design §6c)', () => {
    const DAY = 86400;
    const sprintRow = (over: Record<string, unknown> = {}) => ({
      id: 4,
      kind: 'sprint',
      name: 'Sprint 24',
      state: 'active',
      goal: 'Login works',
      total: 2,
      done: 1,
      created_at: 0,
      updated_at: 0,
      version: 1,
      ...over,
    });
    let sprints: Record<string, unknown>[];
    beforeEach(() => {
      sprints = [sprintRow(), sprintRow({ id: 5, name: 'Sprint 25', state: 'planned', goal: null }), sprintRow({ id: 3, name: 'Sprint 23', state: 'closed' })];
      (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
        if (cmd === 'work_buckets') return sprints;
        if (cmd === 'work_bucket_admin') return { bucket: sprintRow({ id: 5, state: 'active' }) };
        if (cmd === 'work_tree') {
          const p = page();
          // An old done task: outside the 7-day window, inside the sprint.
          p.tasks.push(task({ task_id: 'item:30', item_id: 30, key: 'TASK-30', title: 'Shipped long ago', kind: 'local', status_category: 'done', sessions: [], last_activity_at: NOW - 30 * DAY }));
          return p;
        }
        return null;
      });
    });
    const lastFilters = () => {
      const c = calls('work_tree');
      return (c[c.length - 1][1] as { args: { filters: Record<string, unknown> } }).args.filters;
    };

    it('offers the open sprints and the backlog, and reads one sprint as its section', async () => {
      render(WorkBoard);
      await flush();
      const pick = screen.getByTestId('work-board-scope') as HTMLSelectElement;
      expect(Array.from(pick.options).map((o) => o.textContent)).toEqual([
        'All tasks',
        'Sprint 24 · active',
        'Sprint 25 · planned',
        'No sprint (backlog)',
      ]);
      expect(lastFilters().group_by).toBeUndefined();
      // Every task: Done is the last week.
      expect(screen.getByTestId('work-board-done-hidden').textContent).toContain('1 older');

      pick.value = '4';
      await fireEvent.change(pick);
      await flush();
      expect(lastFilters()).toMatchObject({ tracker: 1, archived: true, group_by: 'sprint', group: 'sprint:4' });
      expect(lastFilters().status).toBeUndefined();
      expect(get(boardScope)).toBe(4);
      // The sprint's own line, and everything it delivered in Done.
      expect(screen.getByTestId('work-board-sprint').textContent).toContain('Active · 1/2 done');
      expect(screen.getByTestId('work-board-sprint').textContent).toContain('Login works');
      expect(column('done').textContent).toContain('Shipped long ago');
      expect(screen.queryByTestId('work-board-done-hidden')).toBeNull();
      expect(screen.queryByTestId('work-board-sprint-start')).toBeNull();

      pick.value = 'none';
      await fireEvent.change(pick);
      await flush();
      expect(lastFilters()).toMatchObject({ group_by: 'sprint', group: 'none' });
      expect(screen.queryByTestId('work-board-sprint')).toBeNull();
    });

    it('starts a planned sprint from its line', async () => {
      boardScope.set(5);
      render(WorkBoard);
      await flush();
      await fireEvent.click(screen.getByTestId('work-board-sprint-start'));
      await flush();
      expect(calls('work_bucket_admin').map((c) => c[1])).toEqual([
        { args: { action: 'bucket_update', bucket_id: 5, expected_version: 1, state: 'active' } },
      ]);
    });

    it('falls back to every task when the chosen sprint closed', async () => {
      boardScope.set(3);
      render(WorkBoard);
      await flush();
      await flush();
      expect((screen.getByTestId('work-board-scope') as HTMLSelectElement).value).toBe('all');
      expect(lastFilters().group).toBeUndefined();
      expect(screen.queryByTestId('work-board-sprint')).toBeNull();
    });
  });
});

describe('WorkBoard G3.5: add per column, select, start, PR chip', () => {
  it('+ on a status column adds a task in that status; a tracker column has none', async () => {
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, payload?: { args: never }) => {
      if (cmd === 'work_tree') return page();
      if (cmd === 'create_work_task') return { id: 99, source: 'local', title: 'New one', status_category: 'todo' };
      if (cmd === 'set_work_status') return setStatus(payload!.args);
      return null;
    });
    render(WorkBoard);
    await flush();
    expect(screen.queryByTestId('work-board-add-doing:in review')).toBeNull();
    await fireEvent.click(screen.getByTestId('work-board-add-doing'));
    const input = screen.getByTestId('work-board-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'New one' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await flush();
    expect(calls('create_work_task')[0][1]).toEqual({ args: { title: 'New one' } });
    expect(calls('set_work_status')[0][1]).toEqual({ args: { item_id: 99, status: 'in_progress' } });
    expect(screen.queryByTestId('work-board-add-input')).toBeNull();
  });

  it('+ on To do creates without a status write', async () => {
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_tree') return page();
      if (cmd === 'create_work_task') return { id: 98, source: 'local', title: 'x', status_category: 'todo' };
      return null;
    });
    render(WorkBoard);
    await flush();
    await fireEvent.click(screen.getByTestId('work-board-add-todo'));
    const input = screen.getByTestId('work-board-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'x' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await flush();
    expect(calls('create_work_task')).toHaveLength(1);
    expect(calls('set_work_status')).toHaveLength(0);
  });

  it('x selects cards and the bar moves them together; refused ones stay selected with their reason', async () => {
    render(WorkBoard);
    await flush();
    await fireEvent.keyDown(card('Write notes'), { key: 'x' });
    await fireEvent.click(card('Login fails'), { shiftKey: true });
    expect(screen.getByTestId('work-board-pickbar').textContent).toContain('2 selected');
    expect(card('Write notes').getAttribute('aria-pressed')).toBe('true');
    // Shift+click selects; it does not open.
    expect(get(selectedTaskId)).not.toBe('item:12');
    const sel = screen.getByTestId('work-board-pick-move') as HTMLSelectElement;
    sel.value = 'done';
    await fireEvent.change(sel);
    await flush();
    expect(calls('set_work_status').map((c) => c[1])).toEqual([{ args: { item_id: 1, status: 'done' } }]);
    expect(column('done').textContent).toContain('Write notes');
    // The ticket is refused on its card and stays picked.
    expect(screen.getByTestId('work-board-pickbar').textContent).toContain('1 selected');
    await fireEvent.keyDown(card('Login fails'), { key: 'x' });
    expect(screen.queryByTestId('work-board-pickbar')).toBeNull();
  });

  it('the open card carries its Work button; s on a card opens it and asks to start new', async () => {
    selectedTaskId.set(null);
    render(WorkBoard);
    await flush();
    expect(screen.queryByTestId('work-board-card-work')).toBeNull();
    await fireEvent.keyDown(card('Write notes'), { key: 's' });
    await flush();
    expect(get(selectedTaskId)).toBe('item:1');
    expect(screen.getByTestId('work-board-card-work')).toBeTruthy();
    // Start new asks for a preview of the start.
    expect(calls('preview_start_work').length + calls('start_work').length).toBeGreaterThan(0);
  });

  it('asks the hub for missions, and a card names its mission and wave (G7.6)', async () => {
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd !== 'work_tree') return null;
      const p = page();
      p.tasks[0] = { ...p.tasks[0], mission: { id: 3, name: 'Windows release', wave: 2 } };
      p.tasks[1] = { ...p.tasks[1], mission: { id: 4, name: 'Demo mission' } };
      return p;
    });
    render(WorkBoard);
    await flush();
    expect((calls('work_tree')[0][1] as { args: Record<string, unknown> }).args.with_missions).toBe(true);
    expect(card('Write notes').querySelector('[data-testid="work-board-mission"]')?.textContent).toBe('Windows release · wave 2');
    expect(card('Login fails').querySelector('[data-testid="work-board-mission"]')?.textContent).toBe('Demo mission');
  });

  it('a card with no session offers Start new without being opened; one with a session does not (G7.6)', async () => {
    selectedTaskId.set(null);
    render(WorkBoard);
    await flush();
    const starts = screen.getAllByTestId('work-board-card-start');
    expect(starts).toHaveLength(1);
    expect(starts[0].closest('li')?.textContent).toContain('Write notes');
    expect(starts[0].querySelector('[data-testid="work-button-primary"]')?.textContent).toBe('Start new');
  });

  it('the open card says when it finishes, from its done-when lines (G7.6)', async () => {
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd !== 'work_tree') return null;
      const p = page();
      p.tasks[1] = { ...p.tasks[1], done_when: ['ci', 'review'] };
      return p;
    });
    selectedTaskId.set('item:12');
    render(WorkBoard);
    await flush();
    expect(screen.getByTestId('work-board-finishes').textContent).toBe('Finishes when CI passes and a review approves');
    selectedTaskId.set('item:1');
    await flush();
    expect(screen.queryByTestId('work-board-finishes')).toBeNull();
    selectedTaskId.set(null);
  });

  it('a card shows its live pull request with failing checks', async () => {
    const { sessions } = await import('./sessions');
    const { session } = await import('./hosts_fixture');
    sessions.set([
      session('mefistos', 'abc-12 login', {
        id: 7,
        pr_evidence: {
          head_oid: 'a', local_head: 'a', ahead: 0, dirty: false, draft: false, state: 'OPEN',
          checks: { total: 3, pending: 0, skipped: 0, failing_total: 1, failing: [{ name: 'rust' }] },
        },
      }),
    ]);
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd !== 'work_tree') return null;
      const p = page();
      p.tasks[1] = { ...p.tasks[1], sessions: [link({ session_id: 7, name: 'abc-12 login', pr_url: 'https://github.com/o/r/pull/478' })] };
      return p;
    });
    render(WorkBoard);
    await flush();
    expect(card('Login fails').querySelector('[data-testid="work-board-pr"]')?.textContent).toBe('PR #478 ✕ 1');
    // The agent that runs in its live session.
    expect(card('Login fails').querySelector('[data-testid="work-board-agent"]')?.textContent).toBe(' · Claude Code');
    sessions.set([]);
  });
});
