// New task (gap plan G2.1, the FormsWork board): the dialog opened from ⌘N
// while the Work view is open, and from the list's "+ New task" ▾.
//
// - ⌘N is New session's chord everywhere else. The registry's `work` scope
//   row shadows it, and while a Work view is mounted (`ownNewTaskChord`)
//   the quick switcher stands aside so the chord makes a task instead.
// - "Start a session for it now" opens the start menu of the new task next:
//   `requestStartAsk` names the task, and the first Work button of that task
//   to mount (its list row, or the task page's bar) opens its popover.
import { get, writable } from 'svelte/store';
import { matchShortcut, type KeyEventLike } from './shortcuts';

/** How many mounted Work views take ⌘N for New task. */
const owners = writable(0);

/** A Work view takes ⌘N while mounted. Returns the release. */
export function ownNewTaskChord(): () => void {
  owners.update((n) => n + 1);
  let released = false;
  return () => {
    if (released) return;
    released = true;
    owners.update((n) => Math.max(0, n - 1));
  };
}

/** Whether ⌘N belongs to New task right now (a Work view is open). */
export function newTaskOwnsChord(): boolean {
  return get(owners) > 0;
}

/** Whether `e` is New task's chord (⌘N; Ctrl+Shift+N off the Mac). */
export function isNewTaskChord(e: KeyEventLike, isMac: boolean): boolean {
  return matchShortcut('work', e, isMac) === 'work.new-task';
}

/** The task whose start menu opens next (`item:<id>`), or null. */
export const startAskRequest = writable<string | null>(null);

/** Open the start menu of `taskId` once a Work button of it is mounted. */
export function requestStartAsk(taskId: string): void {
  startAskRequest.set(taskId);
}

/** Take the request when it is `taskId`'s: true once, for one button. */
export function takeStartAsk(taskId: string): boolean {
  if (get(startAskRequest) !== taskId) return false;
  startAskRequest.set(null);
  return true;
}
