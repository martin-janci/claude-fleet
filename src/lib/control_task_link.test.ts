import { describe, it, expect } from 'vitest';
import { completeTaskLink, linkableTasks, matchTaskLinks, taskLinkQuery } from './control_task_link';

describe('# link a task (G7.8)', () => {
  it('reads the # being typed at the end of the draft, never inside a word', () => {
    expect(taskLinkQuery('#')).toBe('');
    expect(taskLinkQuery('done with #PD-2')).toBe('PD-2');
    expect(taskLinkQuery('C#')).toBeNull();
    expect(taskLinkQuery('see #PD-1 then')).toBeNull();
    expect(taskLinkQuery('plain text')).toBeNull();
  });

  it('offers keyed tasks once, a key that starts with the query first', () => {
    const tasks = linkableTasks([
      { key: 'TASK-219', title: 'Fix login' },
      { key: null, title: 'No key' },
      { key: 'PD-2592', title: 'Support access for TASK owners' },
      { key: 'TASK-219', title: 'dup' },
    ]);
    expect(tasks.map((t) => t.key)).toEqual(['TASK-219', 'PD-2592']);
    expect(matchTaskLinks('task', tasks).map((t) => t.key)).toEqual(['TASK-219', 'PD-2592']);
    expect(matchTaskLinks('login', tasks).map((t) => t.key)).toEqual(['TASK-219']);
    expect(matchTaskLinks('', tasks)).toHaveLength(2);
  });

  it('puts the key in place of what was typed', () => {
    expect(completeTaskLink('/done #pd', { key: 'PD-2592' })).toBe('/done #PD-2592 ');
    expect(completeTaskLink('#', { key: 'TASK-1' })).toBe('#TASK-1 ');
  });
});
