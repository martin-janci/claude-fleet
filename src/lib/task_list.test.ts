import { describe, it, expect } from 'vitest';
import { task } from './work_view_fixture';
import { DONE_WINDOW_SECS, displayTitle, groupTasksByStatus } from './task_list';

const NOW = 1_790_700_000;

describe('groupTasksByStatus', () => {
  it('uses the effective status, and a live session means Doing', () => {
    const s = groupTasksByStatus(
      [
        task({ task_id: 'item:1', status_category: 'todo', counts: { active: 0, ended: 0, suggested: 0 }, last_activity_at: NOW - 10 }),
        task({ task_id: 'item:2', status_category: 'todo', counts: { active: 1, ended: 0, suggested: 0 }, last_activity_at: NOW }),
        task({ task_id: 'item:3', status_category: 'done', counts: { active: 1, ended: 0, suggested: 0 }, last_activity_at: NOW }),
        task({ task_id: 'item:4', status_category: 'done', counts: { active: 0, ended: 1, suggested: 0 }, last_activity_at: NOW - 60 }),
      ],
      NOW,
    );
    expect(s.todo.map((n) => n.task.task_id)).toEqual(['item:1']);
    expect(s.doing.map((n) => n.task.task_id).sort()).toEqual(['item:2', 'item:3']);
    expect(s.done.map((n) => n.task.task_id)).toEqual(['item:4']);
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

describe('displayTitle', () => {
  it('title, then key, then id', () => {
    expect(displayTitle(task({ title: 'Login', key: 'ABC-1' }))).toBe('Login');
    expect(displayTitle(task({ title: '', key: 'ABC-1' }))).toBe('ABC-1');
    expect(displayTitle(task({ title: '', key: null, task_id: 'ref:x' }))).toBe('ref:x');
  });
});
