// The Work tab's List layout (design 2026-09-29): one `work_tree` read
// grouped by status in the client — To do, Doing, Done (last 7 days), by the
// same rule as the Board (`taskColumnOf`) — with
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

/** The one status rule List and Board share (plan step 1.6): a task sits
 *  where its status says, not where its sessions are. A tracker item sits in
 *  the column its tracker reports (sprints design §2, E11), a native item in
 *  its effective status (the hub has already lifted a live one to
 *  `in_progress`, and a person's setting is final). Only a bare key, which has
 *  no status at all, follows its sessions. */
export function taskColumnOf(t: WorkTask): keyof StatusSections {
  if (t.status_category === 'done') return 'done';
  if (t.status_category === 'in_progress') return 'doing';
  if (!t.status_category && (t.counts?.active ?? 0) > 0) return 'doing';
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
  for (const t of [...roots].sort(newestFirst)) {
    const s = taskColumnOf(t);
    if (s === 'done' && (t.last_activity_at ?? 0) < nowSecs - DONE_WINDOW_SECS) continue;
    out[s].push({ task: t, children: [...(kids.get(t.task_id) ?? [])].sort(newestFirst) });
  }
  return out;
}

export function displayTitle(t: WorkTask): string {
  return t.title || t.key || t.task_id;
}

// ── The board (sprints design 2026-09-28 §6c) ──
//
// Columns by status, through the same `taskColumnOf` rule the List uses.

export type BoardColumn = keyof StatusSections;
export const BOARD_COLUMNS: readonly BoardColumn[] = ['todo', 'doing', 'done'];
export const BOARD_COLUMN_LABELS: Record<BoardColumn, string> = {
  todo: 'To do',
  doing: 'Doing',
  done: 'Done',
};
/** The status a person sets by moving a card into a column. */
export const BOARD_COLUMN_STATUS: Record<BoardColumn, 'todo' | 'in_progress' | 'done'> = {
  todo: 'todo',
  doing: 'in_progress',
  done: 'done',
};

export const boardColumnOf: (t: WorkTask) => BoardColumn = taskColumnOf;

export interface BoardColumns extends StatusSections {
  /** Done tasks older than the window, left off the Done column. */
  doneHidden: number;
}

/** The board's columns: top-level tasks (subtasks ride on their parent's
 *  card), newest activity first. `overrides` places a card a person just
 *  moved before the hub's answer comes back; `keep` exempts those cards
 *  from the Done window, so a card dropped on Done stays where it landed. */
export function groupTasksForBoard(
  tasks: WorkTask[],
  nowSecs: number,
  overrides: ReadonlyMap<string, BoardColumn> = new Map(),
  keep: ReadonlySet<string> = new Set(),
): BoardColumns {
  const byId = new Map(tasks.map((t) => [t.task_id, t]));
  const kids = new Map<string, WorkTask[]>();
  const roots: WorkTask[] = [];
  for (const t of tasks) {
    const p = t.parent_task_id ? byId.get(t.parent_task_id) : undefined;
    if (p) kids.set(p.task_id, [...(kids.get(p.task_id) ?? []), t]);
    else roots.push(t);
  }
  const out: BoardColumns = { todo: [], doing: [], done: [], doneHidden: 0 };
  for (const t of [...roots].sort(newestFirst)) {
    const col = overrides.get(t.task_id) ?? boardColumnOf(t);
    if (col === 'done' && !keep.has(t.task_id) && (t.last_activity_at ?? 0) < nowSecs - DONE_WINDOW_SECS) {
      out.doneHidden++;
      continue;
    }
    out[col].push({ task: t, children: [...(kids.get(t.task_id) ?? [])].sort(newestFirst) });
  }
  return out;
}

/** Why a card cannot be moved by a person, or `null` when it can: only a
 *  native item has a status fleet owns. */
export function boardMoveRefusal(t: WorkTask): string | null {
  if (t.kind === 'local' && t.item_id != null) return null;
  const name = t.key || displayTitle(t);
  if (t.kind === 'ref') return `${name} is a bare key with no status of its own; it follows its sessions.`;
  const where = t.tracker_name || t.provider || 'its tracker';
  return `${name}'s status belongs to ${where}. Change it there.`;
}

/** The live session a card shows (the primary first): what no tracker's
 *  board can show. */
export function boardLiveSession(t: WorkTask): { name: string; host: string | null } | null {
  const live = (t.sessions ?? []).filter((s) => s.state === 'active' && s.session_id != null);
  const s = live.find((l) => l.primary) ?? live[0];
  if (!s) return null;
  return { name: s.name || `#${s.session_id}`, host: s.host ?? null };
}
