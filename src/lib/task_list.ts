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

/** Every task under the topmost ancestor the list has loaded: a subtask of
 *  a subtask (local work goes three levels deep) shows under the card or
 *  row of its root, never lost under a child that is not drawn itself. */
export function nestUnderRoots(tasks: readonly WorkTask[]): { roots: WorkTask[]; kids: Map<string, WorkTask[]> } {
  const byId = new Map(tasks.map((t) => [t.task_id, t]));
  const kids = new Map<string, WorkTask[]>();
  const roots: WorkTask[] = [];
  for (const t of tasks) {
    let top: WorkTask | undefined;
    let cur = t;
    // Bounded: a chain is at most three deep, and a cycle never loops.
    for (let i = 0; i < 4 && cur.parent_task_id; i++) {
      const p = byId.get(cur.parent_task_id);
      if (!p || p === t) break;
      top = p;
      cur = p;
    }
    if (top) kids.set(top.task_id, [...(kids.get(top.task_id) ?? []), t]);
    else roots.push(t);
  }
  return { roots, kids };
}

const newestFirst = (a: WorkTask, b: WorkTask) => (b.last_activity_at ?? 0) - (a.last_activity_at ?? 0);

export function groupTasksByStatus(tasks: WorkTask[], nowSecs: number): StatusSections {
  const { roots, kids } = nestUnderRoots(tasks);
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

// ── The board (sprints design 2026-09-28 §6c; redesign 6.1) ──
//
// Columns come from the trackers: a ticket sits under the status name its
// tracker reports (Jira's "In Review", an Asana section), and every one of
// those columns belongs to one of the three statuses through the same
// `taskColumnOf` rule the List uses. So List and Board place a task alike
// (one mapping, shared): a column is a status, or a tracker's name for part
// of one. A native task, and a ticket with no status name, sits in its
// status's own column: To do, In progress, Done.

export type BoardColumn = keyof StatusSections;
export const BOARD_COLUMNS: readonly BoardColumn[] = ['todo', 'doing', 'done'];
export const BOARD_COLUMN_LABELS: Record<BoardColumn, string> = {
  todo: 'To do',
  doing: 'In progress',
  done: 'Done',
};
/** The status a person sets by moving a card into a column. */
export const BOARD_COLUMN_STATUS: Record<BoardColumn, 'todo' | 'in_progress' | 'done'> = {
  todo: 'todo',
  doing: 'in_progress',
  done: 'done',
};

export const boardColumnOf: (t: WorkTask) => BoardColumn = taskColumnOf;

/** One column of the board: a status's own (`id` is the status), or a
 *  tracker's status name under it (`id` is `<status>:<name, lower case>`). */
export interface BoardLane {
  id: string;
  label: string;
  status: BoardColumn;
}

/** Names that differ only in case or spacing are one column ("To Do" is
 *  To do). */
const laneKey = (name: string) => name.trim().replace(/\s+/g, ' ').toLowerCase();

/** The column a task sits in: its tracker's status name when it has one,
 *  else its status's own. */
export function boardLaneOf(t: WorkTask): BoardLane {
  const status = taskColumnOf(t);
  const name = t.kind !== 'local' && t.kind !== 'ref' ? t.status_name?.trim() : '';
  if (!name || laneKey(name) === laneKey(BOARD_COLUMN_LABELS[status])) {
    return { id: status, label: BOARD_COLUMN_LABELS[status], status };
  }
  return { id: `${status}:${laneKey(name)}`, label: name, status };
}

export interface BoardColumns {
  lanes: BoardLane[];
  /** The cards of each lane, by `BoardLane.id`. */
  cards: Record<string, TaskNode[]>;
  /** Done tasks older than the window, left off the Done columns. */
  doneHidden: number;
}

/** The board's columns: the three statuses' own, and every status name a
 *  shown task's tracker reports, in status order (To do, In progress, Done)
 *  and by name within one. Top-level tasks only (subtasks ride on their
 *  parent's card), newest activity first. `overrides` places a card a person
 *  just moved (by lane id) before the hub's answer comes back; `keep`
 *  exempts those cards from the Done window, so a card dropped on Done stays
 *  where it landed. */
export function groupTasksForBoard(
  tasks: WorkTask[],
  nowSecs: number,
  overrides: ReadonlyMap<string, string> = new Map(),
  keep: ReadonlySet<string> = new Set(),
  /** Done shows the last 7 days only; off for a sprint, whose Done is
   *  everything it delivered. */
  doneWindow = true,
): BoardColumns {
  const { roots, kids } = nestUnderRoots(tasks);
  const lanes = new Map<string, BoardLane>(
    BOARD_COLUMNS.map((c) => [c, { id: c, label: BOARD_COLUMN_LABELS[c], status: c }]),
  );
  const out: BoardColumns = { lanes: [], cards: {}, doneHidden: 0 };
  for (const t of [...roots].sort(newestFirst)) {
    const own = boardLaneOf(t);
    if (!lanes.has(own.id)) lanes.set(own.id, own);
    const lane = lanes.get(overrides.get(t.task_id) ?? own.id) ?? lanes.get(own.id)!;
    if (doneWindow && lane.status === 'done' && !keep.has(t.task_id) && (t.last_activity_at ?? 0) < nowSecs - DONE_WINDOW_SECS) {
      out.doneHidden++;
      continue;
    }
    (out.cards[lane.id] ??= []).push({ task: t, children: [...(kids.get(t.task_id) ?? [])].sort(newestFirst) });
  }
  const rank = (l: BoardLane) => BOARD_COLUMNS.indexOf(l.status);
  out.lanes = [...lanes.values()].sort(
    (a, b) => rank(a) - rank(b) || a.label.localeCompare(b.label, undefined, { sensitivity: 'base' }),
  );
  for (const l of out.lanes) out.cards[l.id] ??= [];
  return out;
}

/** Where ← / → takes a native card from `from`: the nearest column of
 *  another status that way (a native task has three statuses, so a column
 *  of its own status is no move), or `null` at the edge. */
export function boardStep(lanes: readonly BoardLane[], from: BoardLane, dir: 1 | -1): BoardLane | null {
  for (let i = lanes.findIndex((l) => l.id === from.id) + dir; i >= 0 && i < lanes.length; i += dir) {
    if (lanes[i].status !== from.status) return lanes[i];
  }
  return null;
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
export function boardLiveSession(t: WorkTask): { name: string; host: string | null; session_id: number } | null {
  const live = (t.sessions ?? []).filter((s) => s.state === 'active' && s.session_id != null);
  const s = live.find((l) => l.primary) ?? live[0];
  if (!s) return null;
  return { name: s.name || `#${s.session_id}`, host: s.host ?? null, session_id: s.session_id as number };
}
