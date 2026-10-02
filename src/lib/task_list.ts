// The Work tab's List layout (design 2026-09-29): one `work_tree` read
// grouped by status in the client — To do, Doing, Done (last 7 days) — with
// native subtasks and agent jobs nested under a listed parent.
import type { WorkTask } from './work_view';

export const DONE_WINDOW_SECS = 7 * 86_400;

export interface TaskNode {
  task: WorkTask;
  children: WorkTask[];
}
export interface StatusSections {
  todo: TaskNode[];
  doing: TaskNode[];
  done: TaskNode[];
}

function sectionOf(t: WorkTask): keyof StatusSections {
  const live = (t.counts?.active ?? 0) > 0;
  if (live || t.status_category === 'in_progress') return 'doing';
  if (t.status_category === 'done') return 'done';
  // Archived with nothing live is finished work, so it belongs in Done and
  // falls under the 7-day window. A bare-key task (`ref:<KEY>`) has no item
  // and therefore NO `status_category` at all, so it used to land in To do and
  // stay there for ever — no age cut-off and no Start button, since there is
  // no item to start.
  if (t.archived) return 'done';
  return 'todo';
}

const newestFirst = (a: WorkTask, b: WorkTask) => (b.last_activity_at ?? 0) - (a.last_activity_at ?? 0);

export function groupTasksByStatus(tasks: WorkTask[], nowSecs: number): StatusSections {
  const byId = new Map(tasks.map((t) => [t.task_id, t]));
  const kids = new Map<string, WorkTask[]>();
  const roots: WorkTask[] = [];
  for (const t of tasks) {
    const p = t.parent_task_id ? byId.get(t.parent_task_id) : undefined;
    if (p) kids.set(p.task_id, [...(kids.get(p.task_id) ?? []), t]);
    else roots.push(t);
  }
  const out: StatusSections = { todo: [], doing: [], done: [] };
  const stale = (t: WorkTask) =>
    sectionOf(t) === 'done' && (t.last_activity_at ?? 0) < nowSecs - DONE_WINDOW_SECS;
  for (const t of roots) {
    const children = [...(kids.get(t.task_id) ?? [])].sort(newestFirst);
    if (stale(t)) {
      // The parent is done and outside the window, but a subtask of it is
      // separate work with its own status: promote every one the window
      // keeps, instead of dropping it with the parent into no section at all.
      for (const k of children) {
        if (!stale(k)) out[sectionOf(k)].push({ task: k, children: [] });
      }
      continue;
    }
    out[sectionOf(t)].push({ task: t, children });
  }
  for (const k of ['todo', 'doing', 'done'] as const) {
    out[k].sort((a, b) => newestFirst(a.task, b.task));
  }
  return out;
}

export function displayTitle(t: WorkTask): string {
  return t.title || t.key || t.task_id;
}
