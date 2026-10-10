import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

vi.mock('@tauri-apps/api/event', () => {
  const handlers = new Map<string, (e: { payload: unknown }) => void>();
  return {
    listen: vi.fn(async (name: string, cb: (e: { payload: unknown }) => void) => {
      handlers.set(name, cb);
      return () => handlers.delete(name);
    }),
    emit: vi.fn(async (name: string, payload: unknown) => {
      handlers.get(name)?.({ payload });
    }),
  };
});

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import HandoffCards from './HandoffCards.svelte';
import { doneWhenWords, finishesWhen, resetHandoffsForTests, sessionState, splitTaskReceipts, taskSummary, taskUndoable, treeWaves, undoable, type ControlHandoff } from './handoffs';
import { sessions, type SessionRow } from './sessions';
import { sessionFocus } from './session_focus';
import { missionOpenRequest } from './missions';
import { get } from 'svelte/store';
import { motionPref } from './motion';
import { fliesIn } from './handoff_flight';

// Redesign steps 9.3 and 9.6: what Control's agent handed on, drawn as chips
// and cards that follow their target's live state.

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const now = Math.floor(Date.now() / 1000);

function session(id: number, status: SessionRow['claude_status'], name = `w${id}`): SessionRow {
  return { id, tmux_name: name, friendly_name: null, claude_status: status, stuck_kind: null } as SessionRow;
}

let receipts: ControlHandoff[] = [];

beforeEach(() => {
  resetHandoffsForTests();
  sessionFocus.set(null);
  missionOpenRequest.set(null);
  receipts = [];
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    if (cmd === 'control_handoffs') return receipts;
    if (cmd === 'accept_work_proposals' || cmd === 'undo_work_accept') return [];
    if (cmd === 'reject_work_proposal') return {};
    if (cmd === 'save_mission') return { id: 77 };
    if (cmd === 'set_mission_item') return { id: 77 };
    return null;
  });
});

const calls = (cmd: string) => inv.mock.calls.filter((c) => c[0] === cmd).map((c) => c[1]);

describe('handoff chips', () => {
  it("a session chip follows the session's live state and opens it", async () => {
    sessions.set([session(3, 'working', 'fix-ci')]);
    receipts = [{ id: 1, at: now, kind: 'session', tool: 'send_prompt', session_id: 3, preview: 'Fix CI' }];
    render(HandoffCards);
    const chip = await screen.findByTestId('handoff-session');
    expect(chip.textContent).toContain('Sent to a session');
    expect(chip.textContent).toContain('fix-ci');
    expect(chip.textContent).toContain('Working');
    sessions.set([session(3, 'completed', 'fix-ci')]);
    await waitFor(() => expect(screen.getByTestId('handoff-session').textContent).toContain('Done'));
    await fireEvent.click(screen.getByTestId('handoff-session'));
    expect(get(sessionFocus)?.id).toBe(3);
  });

  it('a session that is gone says it ended', async () => {
    sessions.set([]);
    receipts = [{ id: 1, at: now, kind: 'session', tool: 'dispatch_task', session_id: 9, preview: 'Review #12' }];
    render(HandoffCards);
    expect((await screen.findByTestId('handoff-session')).textContent).toContain('ended');
  });

  it('a mission chip shows its state and opens the mission', async () => {
    receipts = [
      { id: 2, at: now, kind: 'mission', tool: 'work_link', mission_id: 5, mission_name: 'Ship 9.x', mission_state: 'active' },
    ];
    render(HandoffCards);
    const chip = await screen.findByTestId('handoff-mission');
    expect(chip.textContent).toContain('Sent to a mission');
    expect(chip.textContent).toContain('Ship 9.x');
    expect(chip.textContent).toContain('Working');
    await fireEvent.click(chip);
    expect(get(missionOpenRequest)).toEqual({ id: 5 });
  });

  it('a new receipt shows up on handoff:changed', async () => {
    render(HandoffCards);
    await waitFor(() => expect(calls('control_handoffs')).toHaveLength(1));
    expect(screen.queryByTestId('handoffs')).toBeNull();
    receipts = [{ id: 3, at: now, kind: 'task', tool: 'work_link', item: { id: 20, title: 'Write docs', status: 'todo' } }];
    await emit('handoff:changed', {});
    const card = await screen.findByTestId('handoff-task');
    expect(card.textContent).toContain('Write docs');
  });
});

describe('the proposed tree', () => {
  const tree: ControlHandoff = {
    id: 4,
    at: now,
    kind: 'tree',
    tool: 'work_link',
    item: { id: 10, title: 'Ship 9.6', status: 'todo' },
    items: [
      { id: 11, title: 'Card', status: 'todo', proposal_state: 'proposed' },
      { id: 12, title: 'Commands', status: 'todo', proposal_state: 'proposed' },
      { id: 13, title: 'Docs', status: 'todo', proposal_state: 'proposed' },
    ],
  };

  it('creates the ticked tasks and rejects the unticked ones', async () => {
    receipts = [tree];
    render(HandoffCards);
    const boxes = await screen.findAllByTestId('handoff-tree-item');
    expect(boxes).toHaveLength(3);
    await fireEvent.click(boxes[2]);
    await fireEvent.click(screen.getByTestId('handoff-tree-create'));
    await waitFor(() => expect(calls('accept_work_proposals')).toEqual([{ args: { item_ids: [11, 12] } }]));
    expect(calls('reject_work_proposal')).toEqual([{ args: { item_id: 13 } }]);
    expect(calls('save_mission')).toEqual([]);
  });

  it('creates them as a mission rooted at the parent', async () => {
    receipts = [tree];
    render(HandoffCards);
    await fireEvent.click(await screen.findByTestId('handoff-tree-mission'));
    await waitFor(() => expect(calls('set_mission_item')).toHaveLength(3));
    expect(calls('save_mission')[0]).toEqual({ args: { mission: { name: 'Ship 9.6', goal: 'Ship 9.6' }, item_id: 10 } });
    expect(calls('reject_work_proposal')).toEqual([]);
  });

  it('offers Undo for ten minutes once created, and Undo takes them back', async () => {
    const accepted = (at: number): ControlHandoff => ({
      ...tree,
      items: tree.items!.map((i) => ({ ...i, proposal_state: 'accepted', accepted_at: at })),
    });
    expect(undoable(accepted(now - 60), now)).toHaveLength(3);
    expect(undoable(accepted(now - 601), now)).toHaveLength(0);
    receipts = [accepted(now - 60)];
    render(HandoffCards);
    const undo = await screen.findByTestId('handoff-tree-undo');
    expect(undo.textContent).toContain('9 min');
    await fireEvent.click(undo);
    await waitFor(() => expect(calls('undo_work_accept')).toEqual([{ args: { item_ids: [11, 12, 13] } }]));
  });
});

// Gap plan G3.11 (board MCTasks): tasks in chat.
describe('tasks in chat (G3.11)', () => {
  const plan: ControlHandoff = {
    id: 5,
    at: now,
    kind: 'tree',
    tool: 'work_link',
    item: { id: 20, title: 'Ship G3.11', status: 'todo' },
    items: [
      { id: 21, title: 'Schema', status: 'todo', proposal_state: 'proposed', done_when: ['ci:test'] },
      { id: 22, title: 'Card', status: 'todo', proposal_state: 'proposed', depends_on: [21], done_when: ['ci:test', 'review'] },
      { id: 23, title: 'Docs', status: 'todo', proposal_state: 'proposed', depends_on: [22, 999] },
    ],
  };

  it('waves come from the edges inside the tree; an edge outside does not hold an item back', () => {
    expect(treeWaves(plan.items!).map((w) => w.map((i) => i.id))).toEqual([[21], [22], [23]]);
    expect(treeWaves([{ id: 1, title: 'a', status: 'todo', depends_on: [999] }]).map((w) => w.length)).toEqual([1]);
    expect(doneWhenWords('test:pnpm test')).toBe('`pnpm test` passes');
    expect(finishesWhen(['ci:test', 'review', 'person'])).toBe('finishes when CI test passes, a review approves and a person checks it');
    expect(finishesWhen([])).toBe('');
  });

  it('the plan card lists its tasks by wave with what each finishes when, and Edit in Work opens the parent', async () => {
    receipts = [plan];
    render(HandoffCards);
    const waves = await screen.findAllByTestId('handoff-tree-wave');
    expect(waves.map((w) => w.textContent)).toEqual(['Wave 1', 'Wave 2', 'Wave 3']);
    expect(screen.getAllByTestId('handoff-tree-when').map((w) => w.textContent)).toEqual([
      'finishes when CI test passes',
      'finishes when CI test passes and a review approves',
    ]);
    expect(screen.getByTestId('handoff-tree-edit')).toBeTruthy();
  });

  it('a proposal that may duplicate a task offers Merge or Keep both', async () => {
    inv.mockImplementation(async (cmd: string, raw?: { args?: { task_id?: string } }) => {
      if (cmd === 'control_handoffs') return receipts;
      if (cmd === 'work_task' && raw?.args?.task_id === 'item:20')
        return {
          task: { task_id: 'item:20' },
          proposals: [{ item_id: 22, title: 'Card', at: 1, duplicate: { item_id: 7, task_id: 'item:7', key: 'TASK-236', title: 'Card', source: 'jev' } }],
        };
      if (cmd === 'reject_work_proposal') return {};
      return null;
    });
    receipts = [plan];
    render(HandoffCards);
    const dup = await screen.findByTestId('handoff-tree-dup');
    expect(dup.textContent).toContain('May duplicate TASK-236');
    await fireEvent.click(screen.getByTestId('handoff-tree-merge'));
    await waitFor(() => expect(calls('reject_work_proposal')).toEqual([{ args: { item_id: 22, merge_into: 7 } }]));
    await fireEvent.click(screen.getByTestId('handoff-tree-keep'));
    await waitFor(() => expect(screen.queryByTestId('handoff-tree-dup')).toBeNull());
  });

  it('a created task asks owner and due while new, shows its session, and moves to Done', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'control_handoffs') return receipts;
      if (cmd === 'edit_work_item' || cmd === 'set_work_status') return { id: 30, title: 'Fix login', status_category: 'todo' };
      if (cmd === 'verify_work_item') return { item_id: 30, changed: true };
      return null;
    });
    sessions.set([{ ...session(4, 'working', 'fix-login'), host_alias: 'mac', work: { link_id: 1, item_id: 30, key: null, title: 'Fix login', source: 'agent' } } as SessionRow]);
    receipts = [
      { id: 6, at: now - 30, kind: 'task', tool: 'work_link', item: { id: 30, title: 'Fix login', status: 'todo', done_when: ['person'] } },
    ];
    render(HandoffCards);
    expect((await screen.findByTestId('handoff-task-drafted')).textContent).toBe('Drafted from your message');
    expect(screen.getByTestId('handoff-task-live').textContent).toContain('fix-login on mac');
    expect(screen.getByTestId('handoff-task-when').textContent).toBe('finishes when a person checks it');
    expect(screen.queryByTestId('handoff-task-undo')).toBeNull();
    await fireEvent.input(screen.getByTestId('handoff-task-owner'), { target: { value: 'Ana' } });
    await fireEvent.input(screen.getByTestId('handoff-task-due'), { target: { value: '2026-10-16' } });
    await fireEvent.click(screen.getByTestId('handoff-task-save'));
    await waitFor(() => expect(calls('edit_work_item')).toEqual([{ args: { item_id: 30, assignees: ['Ana'], due_at: '2026-10-16' } }]));
    await fireEvent.click(screen.getByTestId('handoff-task-verify'));
    await waitFor(() => expect(calls('verify_work_item')).toEqual([{ args: { item_id: 30, line: 'person', ok: true } }]));
    await fireEvent.click(screen.getByTestId('handoff-task-done'));
    await waitFor(() => expect(calls('set_work_status')).toEqual([{ args: { item_id: 30, status: 'done' } }]));
  });

  it('Undo shows only while the backend can take an accept back', () => {
    const item = { id: 1, title: 't', status: 'todo', proposal_state: 'accepted', accepted_at: now - 60 };
    expect(taskUndoable(item, now)).toBe(true);
    expect(taskUndoable({ ...item, accepted_at: now - 700 }, now)).toBe(false);
    expect(taskUndoable({ ...item, proposal_state: null }, now)).toBe(false);
  });
});

describe('sessionState', () => {
  it("maps a row onto the manual's five states", () => {
    expect(sessionState(session(1, 'working'))).toBe('working');
    expect(sessionState(session(1, 'blocked'))).toBe('waiting');
    expect(sessionState(session(1, 'completed'))).toBe('done');
    expect(sessionState(session(1, 'failed'))).toBe('failed');
    expect(sessionState({ ...session(1, 'working'), stuck_kind: 'oom' } as SessionRow)).toBe('failed');
    expect(sessionState(session(1, 'idle'))).toBe('idle');
    expect(sessionState(undefined)).toBe('idle');
  });
});

// Step 9.13: a session receipt that arrives while the chat is open flies in.
describe('the comet onto a "Sent to a session" chip', () => {
  const fresh = (): ControlHandoff => ({ id: 7, at: now + 5, kind: 'session', tool: 'send_prompt', session_id: 3, preview: 'Fix CI' });

  it('the flight ends on the chip', async () => {
    motionPref.set('full');
    sessions.set([session(3, 'working', 'fix-ci')]);
    render(HandoffCards);
    await waitFor(() => expect(calls('control_handoffs')).toHaveLength(1));
    receipts = [fresh()];
    await emit('handoff:changed', {});
    const flight = await screen.findByTestId('handoff-flight');
    expect(flight.querySelector('[data-loader="comet"]')).not.toBeNull();
    expect(screen.getByTestId('handoff-session').getAttribute('data-flight')).toBe('flying');
    await fireEvent.animationEnd(flight);
    expect(screen.queryByTestId('handoff-flight')).toBeNull();
    expect(screen.getByTestId('handoff-session').getAttribute('data-flight')).toBe('landed');
  });

  it('with reduced motion the chip appears without it', async () => {
    motionPref.set('reduced');
    try {
      sessions.set([session(3, 'working', 'fix-ci')]);
      render(HandoffCards);
      await waitFor(() => expect(calls('control_handoffs')).toHaveLength(1));
      receipts = [fresh()];
      await emit('handoff:changed', {});
      const chip = await screen.findByTestId('handoff-session');
      expect(screen.queryByTestId('handoff-flight')).toBeNull();
      expect(chip.hasAttribute('data-flight')).toBe(false);
    } finally {
      motionPref.set('system');
    }
  });

  it('only a session receipt written since the chat opened flies, at full motion', () => {
    const h = fresh();
    expect(fliesIn(h, now, 'full')).toBe(true);
    expect(fliesIn({ ...h, at: now - 60 }, now, 'full')).toBe(false);
    expect(fliesIn({ ...h, kind: 'mission' }, now, 'full')).toBe(false);
    expect(fliesIn(h, now, 'off')).toBe(false);
  });
});

// Control chat UX (2026-10-10): six task cards used to take the whole chat.
describe('the task group', () => {
  const task = (id: number, status: string, at = now - 3600): ControlHandoff => ({
    id,
    at,
    kind: 'task',
    tool: 'work_link',
    item: { id: 100 + id, title: `Task ${id}`, status },
  });

  it('orders what needs a person first and finished work last, and counts them', () => {
    const { tasks, others } = splitTaskReceipts([
      task(1, 'done'),
      { id: 9, at: now, kind: 'session', tool: 'session_send', session_id: 1 } as ControlHandoff,
      task(2, 'in_progress'),
      task(3, 'todo'),
      task(4, 'blocked'),
      task(5, 'in_progress'),
    ]);
    expect(tasks.map((h) => h.id)).toEqual([4, 2, 5, 3, 1]);
    expect(others.map((h) => h.id)).toEqual([9]);
    expect(taskSummary(tasks.map((h) => h.item!.status))).toBe('5 tasks · 1 need you · 2 working · 1 idle · 1 done');
    expect(taskSummary(['todo'])).toBe('1 task · 1 idle');
  });

  it('opens by itself for a few tasks', async () => {
    receipts = [task(1, 'todo'), task(2, 'done')];
    render(HandoffCards);
    const toggle = await screen.findByTestId('handoff-task-toggle');
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(toggle.textContent).toContain('2 tasks · 1 idle · 1 done');
  });

  it('folds many tasks under the header until it is pressed', async () => {
    receipts = [1, 2, 3, 4, 5, 6].map((i) => task(i, i % 2 ? 'in_progress' : 'todo'));
    render(HandoffCards);
    const toggle = await screen.findByTestId('handoff-task-toggle');
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    expect(screen.getAllByTestId('handoff-task-card')[0].closest('ul')!.hidden).toBe(true);
    await fireEvent.click(toggle);
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getAllByTestId('handoff-task-card')[0].closest('ul')!.hidden).toBe(false);
  });

  it('never folds a new task that still asks owner and due', async () => {
    receipts = [1, 2, 3, 4, 5].map((i) => task(i, 'in_progress')).concat(task(6, 'todo', now - 30));
    render(HandoffCards);
    const toggle = await screen.findByTestId('handoff-task-toggle');
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByTestId('handoff-task-owner')).toBeTruthy();
  });
});
