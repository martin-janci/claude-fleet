import { describe, it, expect } from 'vitest';
import { link, task } from './work_view_fixture';
import {
  DONE_WINDOW_SECS,
  boardColumnOf,
  boardLaneOf,
  boardLiveSession,
  boardMoveRefusal,
  boardStep,
  displayTitle,
  groupTasksByStatus,
  groupTasksForBoard,
  taskColumnOf,
  type StatusSections,
} from './task_list';

const NOW = 1_790_700_000;

describe('groupTasksByStatus', () => {
  it('uses the effective status; a live session moves only a bare key', () => {
    const s = groupTasksByStatus(
      [
        task({ task_id: 'item:1', status_category: 'todo', counts: { active: 0, ended: 0, suggested: 0 }, last_activity_at: NOW - 10 }),
        task({ task_id: 'item:2', status_category: 'todo', counts: { active: 1, ended: 0, suggested: 0 }, last_activity_at: NOW }),
        task({ task_id: 'item:3', status_category: 'done', counts: { active: 1, ended: 0, suggested: 0 }, last_activity_at: NOW }),
        task({ task_id: 'item:4', status_category: 'done', counts: { active: 0, ended: 1, suggested: 0 }, last_activity_at: NOW - 60 }),
      ],
      NOW,
    );
    expect(s.todo.map((n) => n.task.task_id)).toEqual(['item:2', 'item:1']);
    expect(s.doing).toEqual([]);
    expect(s.done.map((n) => n.task.task_id)).toEqual(['item:3', 'item:4']);
  });

  it('Done keeps the last 7 days', () => {
    const s = groupTasksByStatus(
      [
        task({ task_id: 'item:1', status_category: 'done', counts: { active: 0, ended: 1, suggested: 0 }, last_activity_at: NOW - DONE_WINDOW_SECS + 1 }),
        task({ task_id: 'item:2', status_category: 'done', counts: { active: 0, ended: 1, suggested: 0 }, last_activity_at: NOW - DONE_WINDOW_SECS - 1 }),
      ],
      NOW,
    );
    expect(s.done.map((n) => n.task.task_id)).toEqual(['item:1']);
  });

  it('nests native children under a listed parent, else lists them alone', () => {
    const s = groupTasksByStatus(
      [
        task({ task_id: 'item:1', status_category: 'in_progress' }),
        task({ task_id: 'item:2', origin: 'agent', parent_task_id: 'item:1', status_category: 'done' }),
        task({ task_id: 'item:3', origin: 'manual', parent_task_id: 'item:99', status_category: 'todo', counts: { active: 0, ended: 0, suggested: 0 } }),
      ],
      NOW,
    );
    expect(s.doing[0].children.map((c) => c.task_id)).toEqual(['item:2']);
    expect(s.todo.map((n) => n.task.task_id)).toEqual(['item:3']);
  });
});

describe('List and Board share one status rule', () => {
  const live = { active: 1, ended: 0, suggested: 0 };
  const idle = { active: 0, ended: 0, suggested: 0 };
  const cases = [
    // TASK-224: a tracker ticket still in To do while a session works on it.
    task({ task_id: 'jira:TASK-224', kind: 'tracker', key: 'TASK-224', status_category: 'todo', counts: live }),
    task({ task_id: 'item:1', status_category: 'in_progress', counts: idle }),
    task({ task_id: 'item:2', status_category: 'done', counts: live }),
    task({ task_id: 'item:3', status_category: 'todo', counts: idle }),
    task({ task_id: 'ref:A-1', kind: 'ref', status_category: null, counts: live }),
    task({ task_id: 'ref:A-2', kind: 'ref', status_category: null, counts: idle }),
  ];
  const where = (sections: StatusSections, id: string) =>
    (Object.keys(sections) as (keyof StatusSections)[]).find((k) => sections[k].some((n) => n.task.task_id === id));
  // The board's lane may be a tracker's own name; its status is what List uses.
  const onBoard = (b: ReturnType<typeof groupTasksForBoard>, id: string) =>
    b.lanes.find((l) => b.cards[l.id].some((n) => n.task.task_id === id))?.status;

  it('every task lands in the same column in List and Board', () => {
    const list = groupTasksByStatus(cases.map((t) => ({ ...t, last_activity_at: NOW })), NOW);
    const board = groupTasksForBoard(cases.map((t) => ({ ...t, last_activity_at: NOW })), NOW);
    for (const t of cases) {
      expect(where(list, t.task_id), t.task_id).toBe(onBoard(board, t.task_id));
      expect(where(list, t.task_id), t.task_id).toBe(taskColumnOf(t));
    }
    expect(where(list, 'jira:TASK-224')).toBe('todo');
  });
});

describe('displayTitle', () => {
  it('title, then key, then id', () => {
    expect(displayTitle(task({ title: 'Login', key: 'ABC-1' }))).toBe('Login');
    expect(displayTitle(task({ title: '', key: 'ABC-1' }))).toBe('ABC-1');
    expect(displayTitle(task({ title: '', key: null, task_id: 'ref:x' }))).toBe('ref:x');
  });
});

describe('the board', () => {
  const idle = { active: 0, ended: 0, suggested: 0 };
  const native = (t: Parameters<typeof task>[0]) => task({ kind: 'local', status_name: null, ...t });

  it('places a card by its status, not its sessions; only a bare key follows its sessions', () => {
    // A tracker ticket in To do with a live session stays in To do (E11).
    expect(boardColumnOf(task({ status_category: 'todo', counts: { active: 1, ended: 0, suggested: 0 } }))).toBe('todo');
    expect(boardColumnOf(task({ status_category: 'in_progress', counts: idle }))).toBe('doing');
    expect(boardColumnOf(task({ status_category: 'done' }))).toBe('done');
    expect(boardColumnOf(task({ kind: 'ref', status_category: null, counts: { active: 1, ended: 0, suggested: 0 } }))).toBe('doing');
    expect(boardColumnOf(task({ kind: 'ref', status_category: null, counts: idle }))).toBe('todo');
  });

  it('hides old Done cards but keeps the ones moved here, and honours a pending move', () => {
    const old = NOW - DONE_WINDOW_SECS - 1;
    const cols = groupTasksForBoard(
      [
        native({ task_id: 'item:1', status_category: 'done', last_activity_at: old }),
        native({ task_id: 'item:2', status_category: 'todo', last_activity_at: old }),
        native({ task_id: 'item:3', status_category: 'done', last_activity_at: old }),
        native({ task_id: 'item:4', status_category: 'todo', last_activity_at: NOW }),
        native({ task_id: 'item:5', parent_task_id: 'item:4', status_category: 'done' }),
      ],
      NOW,
      new Map([['item:2', 'done']]),
      new Set(['item:2', 'item:3']),
    );
    expect(cols.cards.done.map((n) => n.task.task_id)).toEqual(['item:2', 'item:3']);
    expect(cols.doneHidden).toBe(1);
    expect(cols.cards.todo.map((n) => n.task.task_id)).toEqual(['item:4']);
    expect(cols.cards.todo[0].children.map((c) => c.task_id)).toEqual(['item:5']);
  });

  it('a native board keeps the three columns, empty ones included', () => {
    const cols = groupTasksForBoard([task({ task_id: 'item:1', kind: 'local', status_name: null, status_category: 'todo' })], NOW);
    expect(cols.lanes.map((l) => l.label)).toEqual(['To do', 'In progress', 'Done']);
    expect(cols.cards.doing).toEqual([]);
  });

  it('takes its columns from a Jira workflow: each status name in its status, by name', () => {
    const jira = (id: string, status_name: string, status_category: 'todo' | 'in_progress' | 'done') =>
      task({ task_id: id, kind: 'tracker', status_name, status_category, counts: idle, last_activity_at: NOW });
    const cols = groupTasksForBoard(
      [
        jira('jira:A-1', 'In Review', 'in_progress'),
        jira('jira:A-2', 'Backlog', 'todo'),
        jira('jira:A-3', 'To Do', 'todo'),
        jira('jira:A-4', 'In  progress', 'in_progress'),
        jira('jira:A-5', 'Done', 'done'),
        jira('jira:A-6', 'Selected for Development', 'todo'),
        task({ task_id: 'item:7', kind: 'local', status_name: null, status_category: 'in_progress', last_activity_at: NOW }),
      ],
      NOW,
    );
    expect(cols.lanes.map((l) => `${l.status}/${l.label}`)).toEqual([
      'todo/Backlog',
      'todo/Selected for Development',
      'todo/To do',
      'doing/In progress',
      'doing/In Review',
      'done/Done',
    ]);
    // "To Do", "In  progress" and "Done" are the status's own column, not a second one.
    expect(cols.cards.todo.map((n) => n.task.task_id)).toEqual(['jira:A-3']);
    expect(cols.cards.doing.map((n) => n.task.task_id).sort()).toEqual(['item:7', 'jira:A-4']);
    expect(cols.cards['doing:in review'].map((n) => n.task.task_id)).toEqual(['jira:A-1']);
    expect(cols.cards['todo:backlog'].map((n) => n.task.task_id)).toEqual(['jira:A-2']);
  });

  it('a status name on a native task or a bare key makes no column', () => {
    expect(boardLaneOf(task({ kind: 'local', status_name: 'Blocked', status_category: 'todo' })).id).toBe('todo');
    expect(boardLaneOf(task({ kind: 'ref', status_name: 'Blocked', status_category: null, counts: idle })).id).toBe('todo');
    expect(boardLaneOf(task({ kind: 'tracker', status_name: 'Blocked', status_category: 'todo' }))).toEqual({
      id: 'todo:blocked',
      label: 'Blocked',
      status: 'todo',
    });
  });

  it('← → step to the nearest column of another status, and stop at the edge', () => {
    const lanes = [
      { id: 'todo:backlog', label: 'Backlog', status: 'todo' as const },
      { id: 'todo', label: 'To do', status: 'todo' as const },
      { id: 'doing', label: 'In progress', status: 'doing' as const },
      { id: 'doing:in review', label: 'In Review', status: 'doing' as const },
      { id: 'done', label: 'Done', status: 'done' as const },
    ];
    expect(boardStep(lanes, lanes[0], 1)?.id).toBe('doing');
    expect(boardStep(lanes, lanes[1], 1)?.id).toBe('doing');
    expect(boardStep(lanes, lanes[2], 1)?.id).toBe('done');
    expect(boardStep(lanes, lanes[2], -1)?.id).toBe('todo');
    expect(boardStep(lanes, lanes[0], -1)).toBeNull();
    expect(boardStep(lanes, lanes[4], 1)).toBeNull();
  });

  it('only a native item moves; a ticket names its tracker, a key says why', () => {
    expect(boardMoveRefusal(task({ kind: 'local', item_id: 3 }))).toBeNull();
    expect(boardMoveRefusal(task({ key: 'ABC-12', tracker_name: 'Jira (acme)' }))).toBe(
      "ABC-12's status belongs to Jira (acme). Change it there.",
    );
    expect(boardMoveRefusal(task({ kind: 'ref', item_id: null, key: 'LOC-1' }))).toContain('bare key');
  });

  it('shows the primary live session and its host, never a suggestion or a past one', () => {
    expect(
      boardLiveSession(
        task({
          sessions: [
            link({ primary: false, session_id: 8, name: 'second', host: 'b' }),
            link({ primary: true, session_id: 7, name: 'first', host: 'a' }),
          ],
        }),
      ),
    ).toEqual({ name: 'first', host: 'a', session_id: 7 });
    expect(boardLiveSession(task({ sessions: [link({ state: 'suggested' }), link({ state: 'ended', session_id: null })] }))).toBeNull();
  });
});
