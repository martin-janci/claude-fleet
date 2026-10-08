// Redesign step 3.4: one selection store for sessions and tasks. A picked
// task never sits beside another task's session: the pane shows one of the
// task's live sessions or none, and the task's Close returns to the session
// the pick put away.
import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import {
  backSession,
  clearSelection,
  closeTask,
  pickTask,
  selectedSession,
  selectedTaskId,
  selectSession,
  selectSessionExplicitly,
  selection,
  taskFocused,
  taskLinksLoaded,
} from './selection';
import { sessions } from './sessions';
import { session } from './hosts_fixture';

const s225 = session('mercury', 'task-225', { id: 225, work: { link_id: 1, item_id: 225, key: 'TASK-225', title: 'Demo mission', source: 'manual' } });
const s223 = session('mercury', 'task-223', { id: 223 });
const s9 = session('mac', 'other', { id: 9 });

beforeEach(() => {
  sessions.set([s225, s223, s9]);
  clearSelection();
  closeTask();
  selectedTaskId.set(null);
});

describe('one selection for sessions and tasks', () => {
  it('picking TASK-223 never shows TASK-225\'s conversation', () => {
    selectSessionExplicitly(s225, { task: 'item:225' });
    expect(get(selectedTaskId)).toBe('item:225');

    pickTask('item:223', []);
    expect(get(selectedTaskId)).toBe('item:223');
    expect(get(taskFocused)).toBe(true);
    expect(get(selectedSession)).toBeNull();
    expect(get(backSession)?.id).toBe(225);

    pickTask('item:223', [{ session_id: 223, state: 'active' }]);
    expect(get(selectedSession)?.id).toBe(223);
  });

  it('a task with a live session opens it, and the open one stays when it is the task\'s', () => {
    selectSessionExplicitly(s9);
    pickTask('item:223', [
      { session_id: 77, state: 'active' }, // not in the store
      { session_id: 223, state: 'active' },
    ]);
    expect(get(selectedSession)?.id).toBe(223);
    expect(get(taskFocused)).toBe(true);
    expect(get(backSession)).toBeNull(); // the pane is not empty

    pickTask('item:225', [
      { session_id: 225, state: 'active' },
      { session_id: 223, state: 'secondary' },
    ]);
    expect(get(selectedSession)?.id).toBe(223);
    expect(get(selection).back).toBeNull();
  });

  it('ended links and ghost rows are not live sessions', () => {
    sessions.set([s225, { ...s223, status: 'ghost' }, s9]);
    selectSessionExplicitly(s9);
    pickTask('item:223', [
      { session_id: 225, state: 'ended' },
      { session_id: 223, state: 'active' },
    ]);
    expect(get(selectedSession)).toBeNull();
  });

  it('without its links a task keeps the session whose primary work it is', () => {
    selectSessionExplicitly(s225);
    pickTask('item:225');
    expect(get(selectedSession)?.id).toBe(225);
    pickTask('item:223');
    expect(get(selectedSession)).toBeNull();
  });

  it('the task\'s links arriving later open its live session', () => {
    selectSessionExplicitly(s225);
    pickTask('item:223');
    expect(get(selectedSession)).toBeNull();
    // Another task's detail answering late changes nothing.
    taskLinksLoaded('item:225', [{ session_id: 225, state: 'active' }]);
    expect(get(selectedSession)).toBeNull();
    taskLinksLoaded('item:223', [{ session_id: 223, state: 'active' }]);
    expect(get(selectedSession)?.id).toBe(223);
    expect(get(taskFocused)).toBe(true);
  });

  it('Close returns to the session the pick put away, across a chain of picks', () => {
    selectSessionExplicitly(s225, { task: 'item:225' });
    pickTask('item:223', []);
    pickTask('item:224', []);
    expect(get(backSession)?.id).toBe(225);
    closeTask();
    expect(get(taskFocused)).toBe(false);
    expect(get(selectedSession)?.id).toBe(225);
    expect(get(selectedTaskId)).toBe('item:224'); // still lit in the lists
  });

  it('Close leaves an open session in place', () => {
    selectSessionExplicitly(s9);
    pickTask('item:223', [{ session_id: 223, state: 'active' }]);
    closeTask();
    expect(get(selectedSession)?.id).toBe(223);
  });

  it('opening a session takes the focus; a follow reselect does not', () => {
    pickTask('item:223', [{ session_id: 223, state: 'active' }]);
    selectSession(s223, { follow: true });
    expect(get(taskFocused)).toBe(true);
    selectSessionExplicitly(s9);
    expect(get(taskFocused)).toBe(false);
    expect(get(selectedTaskId)).toBe('item:223');
    expect(get(backSession)).toBeNull();
  });

  it('a session that leaves the store empties the pane and keeps the task', () => {
    pickTask('item:223', [{ session_id: 223, state: 'active' }]);
    sessions.set([s225, s9]);
    expect(get(selectedSession)).toBeNull();
    expect(get(selectedTaskId)).toBe('item:223');
    expect(get(taskFocused)).toBe(true);
  });

  it('renaming the task (a key that became a ticket) leaves the session alone', () => {
    pickTask('ref:TASK-223', [{ session_id: 223, state: 'active' }]);
    selectedTaskId.set('item:223');
    expect(get(selectedSession)?.id).toBe(223);
    expect(get(taskFocused)).toBe(true);
  });
});
