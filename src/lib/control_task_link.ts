// Gap plan G7.8 (board MCTasks): "# link a task" in Control's composer.
// Typing `#` and part of a task's key or title lists the tasks that match;
// picking one puts its key in the box (`#TASK-219 `), the form Control's
// commands (`control_slash.ts`) and its agent read as a task reference.
// Only tasks with a key are offered: a key is what the message carries.
import type { WorkTask } from './work_view';

/** One task the menu offers. */
export interface TaskLink {
  key: string;
  title: string;
  status?: string | null;
}

/** How many tasks the menu lists at most. */
export const TASK_LINK_MAX = 6;

/** The `#…` being typed at the end of the draft (what follows the `#`,
 *  possibly empty), or `null` when the draft does not end in one. A `#`
 *  counts at the start of the draft or after whitespace, so `C#` or a URL
 *  fragment never opens the menu. */
export function taskLinkQuery(draft: string): string | null {
  const m = /(?:^|\s)#([^\s#]*)$/.exec(draft);
  return m ? m[1] : null;
}

/** The tasks a person can link, from a Work tree page. */
export function linkableTasks(tasks: readonly Pick<WorkTask, 'key' | 'title' | 'status_name'>[]): TaskLink[] {
  const out: TaskLink[] = [];
  const seen = new Set<string>();
  for (const t of tasks) {
    const key = t.key?.trim();
    if (!key || seen.has(key)) continue;
    seen.add(key);
    out.push({ key, title: t.title?.trim() || key, status: t.status_name ?? null });
  }
  return out;
}

/** Tasks whose key or title holds `q` (case-insensitive): a key that
 *  starts with it first, then the rest in their order. */
export function matchTaskLinks(q: string, tasks: readonly TaskLink[], max = TASK_LINK_MAX): TaskLink[] {
  const needle = q.toLowerCase();
  const starts: TaskLink[] = [];
  const holds: TaskLink[] = [];
  for (const t of tasks) {
    const key = t.key.toLowerCase();
    if (key.startsWith(needle)) starts.push(t);
    else if (key.includes(needle) || t.title.toLowerCase().includes(needle)) holds.push(t);
  }
  return [...starts, ...holds].slice(0, max);
}

/** The draft with its trailing `#…` replaced by the task's key and a space. */
export function completeTaskLink(draft: string, t: Pick<TaskLink, 'key'>): string {
  return draft.replace(/#[^\s#]*$/, `#${t.key} `);
}
