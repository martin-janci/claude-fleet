import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import {
  controlViews,
  defaultLayout,
  fleetFolds,
  fleetLine,
  isNativeTask,
  matchesQuery,
  startable,
  taskSection,
  tasksBySection,
  moveView,
  needsYouList,
  normalizeLayout,
  openElsewhere,
  selectView,
  shownViews,
  toggleView,
} from './control_views';
import { inboxQueue } from './inbox';
import { destination } from './destination';
import { sidebarView, type WorkTask } from './work_view';
import { automationTab } from './automation';
import { session } from './hosts_fixture';

// Redesign step 9.4: Control's Views panel.

beforeEach(() => controlViews.set(defaultLayout()));

describe('control views layout', () => {
  it('starts on Needs you with every view shown, Library included (9.7)', () => {
    const l = defaultLayout();
    expect(l.order).toEqual(['needs-you', 'session', 'tasks', 'routines', 'prs', 'library', 'today']);
    expect(l.active).toBe('needs-you');
    expect(l.open).toBe(true);
  });

  it('makes a stored layout whole: unknown ids dropped, new views appended, a hidden active replaced', () => {
    const l = normalizeLayout({ order: ['today', 'bogus', 'today', 'library'], hidden: ['today', 'x'], active: 'today', open: false });
    expect(l.order).toEqual(['today', 'library', 'needs-you', 'session', 'tasks', 'routines', 'prs']);
    expect(l.hidden).toEqual(['today']);
    expect(l.active).toBe('library');
    expect(l.open).toBe(false);
    expect(normalizeLayout('junk')).toEqual(defaultLayout());
  });

  it('"+" turns views off and on, and never the last one', () => {
    toggleView('prs');
    expect(shownViews(get(controlViews)).map((v) => v.id)).toEqual(['needs-you', 'session', 'tasks', 'routines', 'library', 'today']);
    toggleView('needs-you');
    toggleView('session');
    toggleView('tasks');
    toggleView('routines');
    toggleView('library');
    expect(get(controlViews).active).toBe('today');
    toggleView('today');
    expect(shownViews(get(controlViews)).map((v) => v.id)).toEqual(['today']);
    toggleView('prs');
    expect(shownViews(get(controlViews)).map((v) => v.id)).toEqual(['prs', 'today']);
  });

  it('"+" reorders within the strip and stops at its ends', () => {
    moveView('today', -1);
    expect(get(controlViews).order).toEqual(['needs-you', 'session', 'tasks', 'routines', 'prs', 'today', 'library']);
    moveView('needs-you', -1);
    expect(get(controlViews).order[0]).toBe('needs-you');
  });

  it('selecting a hidden view shows it and opens the column', () => {
    controlViews.update((l) => ({ ...l, open: false, hidden: ['prs'] }));
    selectView('prs');
    expect(get(controlViews)).toMatchObject({ active: 'prs', open: true, hidden: [] });
  });

  it('Needs you is the Inbox query itself', () => {
    expect(needsYouList).toBe(inboxQueue);
  });

  it('links open where the view lives', () => {
    destination.set('control');
    openElsewhere('tasks');
    expect(get(sidebarView)).toBe('work');
    expect(get(destination)).toBe('session');
    destination.set('control');
    openElsewhere('hosts');
    expect(get(destination)).toBe('accounts');
    destination.set('control');
    openElsewhere('routines');
    expect(get(destination)).toBe('automation');
    expect(get(automationTab)).toBe('routines');
  });
});

// Gap plan G3.10 (boards MissionControl, MCTasks).
describe('Needs you folds and search (G3.10)', () => {
  const now = 1_790_000_000;
  const opts = { idleSecs: 600, now };
  const since = now - 3600;
  const rows = [
    session('mac', 'asking', { claude_status: 'blocked' }),
    session('mac', 'busy', { claude_status: 'working', last_activity_at: now - 5 }),
    session('trn', 'busy-too', { claude_status: 'working', last_activity_at: now - 50 }),
    session('mac', 'quiet', { claude_status: 'idle', last_stop_at: now - 7200, last_activity_at: now - 7200 }),
    session('mac', 'shipped', { claude_status: 'idle', last_stop_at: now - 60, last_activity_at: now - 60 }),
  ];

  it('splits the fleet into Needs you, Running, Idle and Done today, and counts it', () => {
    const f = fleetFolds(rows, opts, '', since);
    expect(f.needs.map((s) => s.tmux_name)).toEqual(['asking']);
    expect(f.running.map((s) => s.tmux_name)).toEqual(['busy', 'busy-too']);
    expect(f.doneToday.map((s) => s.tmux_name)).toEqual(['shipped']);
    expect(f.idle.map((s) => s.tmux_name)).toEqual(['quiet']);
    expect(fleetLine(f)).toBe('1 needs you · 2 running · 1 idle · 1 done today');
    expect(fleetLine(fleetFolds([], opts))).toBe('Nothing running');
  });

  it('the search narrows every fold by name, host or prompt', () => {
    expect(fleetFolds(rows, opts, 'TRN', since).running.map((s) => s.tmux_name)).toEqual(['busy-too']);
    const f = fleetFolds(rows, opts, 'ship', since);
    expect([f.needs.length, f.running.length, f.idle.length, f.doneToday.length]).toEqual([0, 0, 0, 1]);
    expect(matchesQuery(session('mac', 'x', { last_prompt: 'Fix the login bug' }), 'login')).toBe(true);
  });
});

describe('Tasks view sections (G3.10)', () => {
  const now = 1_790_000_000;
  const t = (id: string, over: Partial<WorkTask> = {}): WorkTask => ({
    task_id: id,
    kind: 'local',
    item_id: Number(id.slice(5)),
    group: { id: 'g', label: 'g', source: 'none' },
    ...over,
  });

  it('puts each task in Needs you, In progress, Up next or Done this week', () => {
    const tasks = [
      t('item:1', { needs_you: true, stage: 'in_progress' }),
      t('item:2', { stage: 'in_progress', counts: { active: 1 } }),
      t('item:3', { stage: 'backlog' }),
      t('item:4', { stage: 'done', last_activity_at: now - 86_400 }),
      t('item:5', { stage: 'done', last_activity_at: now - 30 * 86_400 }),
      t('item:6', { stage: 'backlog', parent_task_id: 'item:3' }),
    ];
    const by = tasksBySection(tasks, { mine: false, claude: false }, now);
    expect(by.needs_you.map((x) => x.task_id)).toEqual(['item:1']);
    expect(by.in_progress.map((x) => x.task_id)).toEqual(['item:2']);
    expect(by.up_next.map((x) => x.task_id)).toEqual(['item:3']);
    expect(by.done_week.map((x) => x.task_id)).toEqual(['item:4']);
    expect(taskSection(tasks[4], now)).toBeNull();
    // Claude: only tasks an agent session is on now.
    const claude = tasksBySection(tasks, { mine: false, claude: true }, now);
    expect(Object.values(claude).flat().map((x) => x.task_id)).toEqual(['item:2']);
  });

  it('Start new takes the tasks with no live session; Assign and Done only native items', () => {
    const tasks = [t('item:1'), t('item:2', { counts: { active: 1 } }), t('ref:X-1', { kind: 'ref', item_id: null, key: 'X-1' })];
    expect(startable(tasks).map((x) => x.task_id)).toEqual(['item:1', 'ref:X-1']);
    expect(tasks.filter(isNativeTask).map((x) => x.task_id)).toEqual(['item:1', 'item:2']);
  });
});
