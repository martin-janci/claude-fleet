import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

/** The task state machine (migration 020): queued → running → done | failed | cancelled. */
export const TASK_STATES = ['queued', 'running', 'done', 'failed', 'cancelled'] as const;
export type TaskState = (typeof TASK_STATES)[number];

/** One dispatched unit of work — mirrors `store::TaskRow` (the nonce is never sent). */
export interface TaskRow {
  id: number;
  requester_session_id: number | null;
  worker_session_id: number | null;
  prompt: string | null;
  state: TaskState;
  /** The paragraph the worker printed after its completion marker. */
  result: string | null;
  /** Failure / cancel reason. */
  error: string | null;
  created_at: number;
  started_at: number | null;
  finished_at: number | null;
}

export const tasks = writable<TaskRow[]>([]);

export function isTerminal(state: TaskState): boolean {
  return state === 'done' || state === 'failed' || state === 'cancelled';
}

/** Fetch every task (newest-first) into the store. */
export async function loadTasks(): Promise<Result<TaskRow[]>> {
  const r = await invokeCmd<TaskRow[]>('list_tasks', {});
  // Defensive: a mocked / older backend may answer with nothing.
  if (r.ok && Array.isArray(r.value)) tasks.set(r.value);
  return r;
}

/** Cancel a queued / running task. The worker session keeps running. */
export async function cancelTask(taskId: number): Promise<Result<TaskRow>> {
  const r = await invokeCmd<TaskRow>('cancel_task', { taskId });
  if (r.ok) mergeTask(r.value);
  return r;
}

/** Pure merge step: replace the row with the same id, or append. Newest-first
 *  ordering is preserved by sorting on (created_at, id) descending. */
function mergeInto(arr: TaskRow[], row: TaskRow): TaskRow[] {
  if (!row) return arr;
  const i = arr.findIndex((t) => t.id === row.id);
  const next = i === -1 ? [...arr, row] : arr.slice();
  if (i !== -1) next[i] = row;
  next.sort((a, b) => b.created_at - a.created_at || b.id - a.id);
  return next;
}

export function mergeTask(row: TaskRow): void {
  if (!row) return;
  tasks.update((arr) => mergeInto(arr, row));
}

/** One backend task event, as delivered by `events.ts`. Tasks are never
 *  removed — they only reach a terminal state — so there is one kind. */
export type TaskEvent = { type: 'updated'; row: TaskRow };

/** Apply a burst of task events in ONE store update. */
export function applyTaskEvents(events: readonly TaskEvent[]): void {
  if (events.length === 0) return;
  tasks.update((arr) => {
    let next = arr;
    for (const ev of events) next = mergeInto(next, ev.row);
    return next;
  });
}

/** First line of a prompt, trimmed and capped, for list rows. */
export function promptFirstLine(prompt: string | null, max = 120): string {
  if (!prompt) return '';
  const line = prompt.split('\n').find((l) => l.trim().length > 0) ?? '';
  const t = line.trim();
  return t.length > max ? `${t.slice(0, max - 1)}…` : t;
}

/** Elapsed time of a task: running → since started_at (or created_at);
 *  finished → started→finished; queued → since created_at. */
export function taskElapsed(t: TaskRow, nowSec: number): string {
  const start = t.started_at ?? t.created_at;
  const end = t.finished_at ?? nowSec;
  const secs = Math.max(0, end - start);
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
  return `${Math.floor(secs / 86400)}d ${Math.floor((secs % 86400) / 3600)}h`;
}
