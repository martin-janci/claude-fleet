// Redesign 6.3 in the Work UI: a task's derived Blocked state shows as Needs
// you with its reason line ("Blocked on TASK-212", the plan's status-word
// decision), and its spend (summed from its sessions) shows on the list
// rows, the board's cards and the task's Details; the grouped tree's org and
// group rows carry their tasks' spend.
import { render, screen, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import TaskList from './TaskList.svelte';
import WorkBoard from './WorkBoard.svelte';
import WorkTaskDetail from './WorkTaskDetail.svelte';
import { task } from './work_view_fixture';
import {
  blockedOnLine,
  buildSections,
  dependencyName,
  taskSpend,
  workViewFilters,
  type WorkTreePage,
} from './work_view';

const NOW = Math.floor(Date.now() / 1000);
const dep = task({
  task_id: 'item:212',
  item_id: 212,
  key: 'TASK-212',
  title: 'Pairing protocol',
  kind: 'local',
  status_category: 'in_progress',
  sessions: [],
  last_activity_at: NOW,
});
const blocked = task({
  task_id: 'item:219',
  item_id: 219,
  key: 'TASK-219',
  title: 'Pair the phone',
  kind: 'local',
  status_category: 'todo',
  sessions: [],
  blocked: true,
  blocked_by: ['item:212'],
  cost_micros: 8_510_000,
  last_activity_at: NOW,
});
const page = (): WorkTreePage => ({
  tasks: [dep, blocked],
  groups: [],
  orgs: [],
  trackers: [],
  total: 2,
  next_cursor: null,
});
const flush = async () => {
  for (let i = 0; i < 8; i++) {
    await Promise.resolve();
    await tick();
  }
};

beforeEach(() => {
  workViewFilters.set({ tracker: 1 });
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
    const args = (raw as { args?: { task_id?: string } } | undefined)?.args;
    if (cmd === 'work_tree') return page();
    if (cmd === 'work_task') return { task: args?.task_id === 'item:212' ? dep : blocked };
    if (cmd === 'list_projects' || cmd === 'work_rules') return [];
    return null;
  });
});

describe('the words', () => {
  it('names what a blocked task waits for by key, two at most, then +N more', () => {
    const known = (id: string) => (id === 'item:212' ? dep : null);
    expect(blockedOnLine(blocked, known)).toBe('Blocked on TASK-212');
    expect(blockedOnLine({ blocked: true, blocked_by: ['item:212', 'item:5', 'item:6', 'item:7'] }, known)).toBe(
      'Blocked on TASK-212, task 5 +2 more',
    );
    expect(blockedOnLine({ blocked: false, blocked_by: ['item:212'] }, known)).toBeNull();
    expect(dependencyName('item:9', { key: null, title: 'Spec it' })).toBe('Spec it');
    expect(taskSpend(blocked)).toBe('$8.51');
    expect(taskSpend({ cost_micros: 0 })).toBeNull();
  });

  it('org rows carry the spend of their groups (spend on org rows)', () => {
    const g = (id: string) => ({ id, label: id, source: 'rule', rule_id: null });
    const sections = buildSections(
      [
        { org_id: 1, group: g('a'), count: 2, cost_micros: 3_000_000 },
        { org_id: 1, group: g('b'), count: 1, cost_micros: 1_500_000 },
        { org_id: null, group: g('none'), count: 1 },
      ],
      [{ id: 1, name: 'Acme' }],
      new Map(),
    );
    expect(sections.map((o) => [o.name, o.cost])).toEqual([
      ['Acme', 4_500_000],
      ['Unassigned', 0],
    ]);
    expect(sections[0].groups.map((x) => x.cost)).toEqual([3_000_000, 1_500_000]);
  });
});

describe('where it shows', () => {
  it('the Work list row: Needs you, the reason line and the spend', async () => {
    render(TaskList);
    await flush();
    const row = screen.getAllByTestId('task-row').find((r) => r.textContent?.includes('Pair the phone'))!;
    // The shared WorkTaskRow: the dot reads Needs you, the line names the
    // dependency, the cost chip carries the spend.
    expect(within(row).getByRole('img', { name: 'Needs you' })).toBeTruthy();
    expect(within(row).getByTestId('work-task-line').textContent).toContain('Blocked on TASK-212');
    expect(within(row).getByTestId('work-task-cost').textContent).toBe('$8.51');
    const other = screen.getAllByTestId('task-row').find((r) => r.textContent?.includes('Pairing protocol'))!;
    expect(within(other).getByTestId('work-task-line').textContent).not.toContain('Blocked');
    expect(within(other).queryByTestId('work-task-cost')).toBeNull();
  });

  it("the board's card", async () => {
    render(WorkBoard);
    await flush();
    const card = screen.getAllByTestId('work-board-card').find((c) => c.textContent?.includes('Pair the phone'))!;
    expect(within(card).getByTestId('work-board-card-blocked-reason').textContent).toBe('Blocked on TASK-212');
    expect(within(card).getByTestId('work-board-card-spend').textContent).toBe('$8.51');
  });

  it("the task's Details, with what it waits for as a link", async () => {
    render(WorkTaskDetail, { taskId: 'item:219' });
    await flush();
    expect(screen.getByTestId('work-task-blocked').textContent).toContain('Needs you');
    expect(screen.getByTestId('work-task-blocked-reason').textContent).toBe('Blocked on TASK-212');
    expect(screen.getByTestId('work-task-spend').textContent).toBe('$8.51');
    expect(screen.getByTestId('work-task-dependency').textContent).toBe('TASK-212');
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('work_task', { args: { task_id: 'item:212' } });
  });
});
