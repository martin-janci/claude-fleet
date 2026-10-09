// Control's Tasks view (board MCTasks): the tasks Work shows, under Work's
// own filters, grouped by what they need from the person: Needs you, In
// progress, Up next and Done this week. Read-only: a row opens the task in
// Work, where it is changed. The status rule is the List's and the Board's
// (`groupTasksByStatus`), so a task sits here where it sits there.
import { displayTitle, groupTasksByStatus } from './task_list';
import type { WorkTask } from './work_view';

export type TaskGroupId = 'needs-you' | 'doing' | 'next' | 'done';

export interface TaskGroup {
  id: TaskGroupId;
  label: string;
  tasks: WorkTask[];
}

export const TASK_GROUP_LABELS: Record<TaskGroupId, string> = {
  'needs-you': 'Needs you',
  doing: 'In progress',
  next: 'Up next',
  done: 'Done this week',
};

/** A task asks for the person: the hub says so, or an agent proposal under
 *  it waits for a decision. Done work asks nothing. */
export function taskNeedsYou(t: WorkTask): boolean {
  return t.status_category !== 'done' && (t.needs_you === true || (t.open_proposals ?? 0) > 0);
}

/** The four groups, top-level tasks only (a subtask rides on its parent in
 *  Work), each newest first. Empty groups are kept: the view draws them
 *  folded with a 0. */
export function controlTaskGroups(tasks: WorkTask[], nowSecs: number): TaskGroup[] {
  const s = groupTasksByStatus(tasks, nowSecs);
  const roots = (nodes: { task: WorkTask }[]) => nodes.map((n) => n.task);
  const open = [...roots(s.doing), ...roots(s.todo)];
  const needs = open.filter(taskNeedsYou);
  const rest = (xs: WorkTask[]) => xs.filter((t) => !taskNeedsYou(t));
  return [
    { id: 'needs-you', label: TASK_GROUP_LABELS['needs-you'], tasks: needs },
    { id: 'doing', label: TASK_GROUP_LABELS.doing, tasks: rest(roots(s.doing)) },
    { id: 'next', label: TASK_GROUP_LABELS.next, tasks: rest(roots(s.todo)) },
    { id: 'done', label: TASK_GROUP_LABELS.done, tasks: roots(s.done) },
  ];
}

/** "TASK-232 Rotate the NAS sudo password" — the key once. */
export function taskTitle(t: WorkTask): string {
  const title = displayTitle(t);
  return t.key && title !== t.key ? `${t.key} ${title}` : title;
}

/** The row's second line: "Yours · In review · Home lab · 1 session". */
export function taskLine(t: WorkTask): string {
  const who = t.mine ? 'Yours' : (t.assignees ?? []).join(', ');
  const status = t.status_name?.trim() || '';
  const group = t.group && t.group.source !== 'none' ? t.group.label?.trim() || '' : '';
  const active = t.counts?.active ?? 0;
  const live = active > 0 ? `${active} session${active === 1 ? '' : 's'}` : 'no session';
  const proposals = (t.open_proposals ?? 0) > 0 ? `${t.open_proposals} to review` : '';
  return [who, status, group, proposals, live].filter(Boolean).join(' · ');
}
